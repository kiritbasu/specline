//! A handler stuck on the store must not stop the daemon answering (KEEL-403).
//!
//! On 2026-09-24 one blocked file read inside `/api/generate` froze the whole
//! daemon: the handler ran on the tokio worker that owned the I/O driver, so
//! nothing accepted connections, fired timers or heard SIGTERM until it
//! returned. The fix runs every store-touching handler on the blocking pool.
//!
//! A runtime with exactly one worker makes that deterministic. Before the fix
//! the stuck handler occupied the only worker and `/api/health` could not be
//! answered at all; after it, the worker stays free.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use specline_daemon::{AppState, http::router, watchdog};
use std::net::SocketAddr;
use std::sync::mpsc;
use std::time::Duration;

async fn daemon() -> (String, AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::open(dir.path(), false).expect("open the store");
    let app = router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), state, dir)
}

/// Hold the store's mutex from an OS thread until told to let go — a write
/// that is taking its time, or a read of a file on a drive that has stalled.
fn hold_the_store(state: AppState) -> (mpsc::Sender<()>, std::thread::JoinHandle<()>) {
    let (held, wait_until_held) = mpsc::channel();
    let (release, wait_for_release) = mpsc::channel::<()>();
    let thread = std::thread::spawn(move || {
        let guard = state.store();
        held.send(()).unwrap();
        let _ = wait_for_release.recv();
        drop(guard);
    });
    wait_until_held.recv().unwrap();
    (release, thread)
}

/// Ask `/api/health` from a plain OS thread with socket timeouts.
///
/// Not through tokio, and that matters: when the bug is present the runtime
/// is frozen, so a tokio timer or reqwest's own timeout never fires and the
/// test hangs instead of failing. Measured: the first version of this file
/// sat for ten minutes against the old code.
fn answers(base: &str) -> Result<(), String> {
    let addr: SocketAddr = base.trim_start_matches("http://").parse().unwrap();
    std::thread::spawn(move || watchdog::probe(addr, Duration::from_secs(2)))
        .join()
        .unwrap()
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn a_request_stuck_on_the_store_does_not_stop_the_daemon_answering() {
    let (base, state, _dir) = daemon().await;
    let http = client();
    assert!(
        http.get(format!("{base}/api/health")).send().await.is_ok(),
        "the daemon answers before anything is stuck"
    );

    let (release, holder) = hold_the_store(state.clone());

    // A request that needs the store, and so waits on the held lock.
    let stuck = tokio::spawn({
        let http = http.clone();
        let url = format!("{base}/api/projects");
        async move { http.get(url).send().await.map(|r| r.status()) }
    });
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        !stuck.is_finished(),
        "the store request should be waiting on the lock"
    );

    // The whole point: with the only worker's handler stuck, a new connection
    // must still be accepted and answered.
    if let Err(reason) = answers(&base) {
        // Let the stuck handler finish before failing, so the runtime can
        // shut down and the failure is reported rather than hung.
        let _ = release.send(());
        let _ = holder.join();
        panic!("the daemon stopped answering while one request waited on the store: {reason}");
    }

    let _ = release.send(());
    holder.join().unwrap();
    let finished = stuck.await.unwrap();
    assert!(
        finished.as_ref().is_ok_and(|s| s.is_success()),
        "the stuck request completes once the store is free: {finished:?}"
    );
}

/// The MCP side of the same thing: a tool call waiting on the store must not
/// take the REST surface down with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn a_tool_call_stuck_on_the_store_does_not_stop_the_daemon_answering() {
    let (base, state, _dir) = daemon().await;
    let http = client();
    let (release, holder) = hold_the_store(state.clone());

    let stuck = tokio::spawn({
        let http = http.clone();
        let url = format!("{base}/mcp");
        async move {
            http.post(url)
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .body(
                    r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"specline_projects","arguments":{}}}"#,
                )
                .send()
                .await
                .map(|r| r.status())
        }
    });
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        !stuck.is_finished(),
        "the tool call should be waiting on the lock"
    );

    if let Err(reason) = answers(&base) {
        // Let the stuck handler finish before failing, so the runtime can
        // shut down and the failure is reported rather than hung.
        let _ = release.send(());
        let _ = holder.join();
        panic!("the daemon stopped answering while a tool call waited on the store: {reason}");
    }

    let _ = release.send(());
    holder.join().unwrap();
    let finished = stuck.await.unwrap();
    assert!(
        finished.as_ref().is_ok_and(|s| s.is_success()),
        "the tool call completes once the store is free: {finished:?}"
    );
}
