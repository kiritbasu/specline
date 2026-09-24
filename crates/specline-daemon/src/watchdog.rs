//! A daemon that has stopped answering exits, so its service manager restarts it.
//!
//! On 2026-09-24 the daemon went quiet three times in one morning: the port
//! stayed open, connections completed their handshake and were never accepted,
//! and every `specline_*` call and hook failed silently. launchd saw a live
//! process and did nothing, because a live process is all it can see. The
//! cause was one blocked file read on the worker that owned tokio's I/O driver
//! (KEEL-403); moving handlers onto the blocking pool fixes that cause, and
//! this is for the next one nobody has thought of yet.
//!
//! It is a plain OS thread, not a tokio task, and that is the point of it: a
//! runtime that has stopped polling cannot run the task that would notice.
//! It asks the daemon the question a client would — connect, `GET
//! /api/health`, read a status line — rather than checking some internal flag
//! that could say "fine" while the socket says otherwise.

use crate::state::AppState;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Set once a deliberate shutdown begins.
///
/// After SIGTERM the listener closes and the probe starts failing, which is
/// exactly what it should do — and without this, a shutdown that took longer
/// than two probes would end in exit 1, which launchd reads as a crash and
/// restarts. A daemon told to stop would come back.
static STANDING_DOWN: AtomicBool = AtomicBool::new(false);

/// Stop watching, because the daemon is shutting down on purpose.
pub fn stand_down() {
    STANDING_DOWN.store(true, Ordering::SeqCst);
}

/// How often the daemon asks itself whether it is answering.
pub const INTERVAL: Duration = Duration::from_secs(30);

/// How long one probe may take, for the connect and again for the answer.
///
/// `/api/health` never takes the store lock, so a healthy daemon answers in
/// milliseconds even mid-write. Five seconds is slack for a loaded machine.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// How many probes in a row must fail before the daemon gives up.
///
/// Two, so one slow moment (a laptop waking, a busy disk) is not a restart,
/// while a real stall ends within about a minute and a quarter.
pub const STRIKES: u32 = 2;

/// How long the store may stay locked, without a single free moment at any
/// probe, before the daemon counts as wedged.
///
/// Once handlers left the async workers, a stalled read no longer freezes the
/// runtime: `/api/health` keeps answering, with `store_busy: true`, while
/// every real request queues behind a lock that will never come free. The
/// probe cannot see that, so the watchdog also looks at the lock itself.
/// Ten minutes is far above any write the daemon does — a bulk import runs in
/// the CLI with the daemon stopped — and short enough that a wedged morning is
/// a restart rather than a morning.
pub const WEDGED_AFTER: Duration = Duration::from_secs(10 * 60);

/// Tracks how long the store has been locked at every look.
#[derive(Debug, Default)]
pub struct LockWatch {
    busy_since: Option<Instant>,
}

impl LockWatch {
    /// Record whether the lock was free at this look, and say whether it has
    /// now been held for longer than [`WEDGED_AFTER`] without a gap.
    ///
    /// Sampled, so a store that is busy but moving will be seen free at some
    /// probe and reset; only one that is never free accumulates.
    pub fn record(&mut self, free: bool, now: Instant) -> bool {
        if free {
            self.busy_since = None;
            return false;
        }
        let since = *self.busy_since.get_or_insert(now);
        now.duration_since(since) >= WEDGED_AFTER
    }
}

/// Counts consecutive failed probes.
#[derive(Debug, Default)]
pub struct Strikes {
    in_a_row: u32,
}

impl Strikes {
    /// Record one probe's outcome, and say whether it is time to give up.
    ///
    /// A success clears the count: two failures a day apart are two slow
    /// moments, not a stall.
    pub fn record(&mut self, answered: bool) -> bool {
        if answered {
            self.in_a_row = 0;
        } else {
            self.in_a_row = self.in_a_row.saturating_add(1);
        }
        self.in_a_row >= STRIKES
    }
}

/// The address to probe for a daemon bound to `bound`.
///
/// A daemon bound to `0.0.0.0` or `::` is not reachable *at* that address, so
/// the probe goes to loopback on the same port.
pub fn probe_address(bound: SocketAddr) -> SocketAddr {
    match bound.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), bound.port())
        }
        IpAddr::V6(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), bound.port())
        }
        _ => bound,
    }
}

/// Ask the daemon at `addr` whether it is answering, the way a client would.
///
/// `Err` carries what went wrong, for the log line written before exiting —
/// "connected and got nothing" and "could not connect" are different stalls.
pub fn probe(addr: SocketAddr, timeout: Duration) -> Result<(), String> {
    let mut stream = TcpStream::connect_timeout(&addr, timeout)
        .map_err(|e| format!("could not connect to {addr}: {e}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|()| stream.set_write_timeout(Some(timeout)))
        .map_err(|e| format!("could not set a timeout on the probe: {e}"))?;
    stream
        .write_all(
            format!(
                "GET /api/health HTTP/1.1\r\nHost: {addr}\r\nAccept: application/json\r\n\
                 Connection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .map_err(|e| format!("connected to {addr} but could not send: {e}"))?;

    // The status line is all the answer needed, and it fits in the first read.
    let mut head = [0u8; 64];
    let read = stream
        .read(&mut head)
        .map_err(|e| format!("connected to {addr} and got no answer: {e}"))?;
    let line = String::from_utf8_lossy(&head[..read]);
    if line.starts_with("HTTP/1.1 200") || line.starts_with("HTTP/1.0 200") {
        Ok(())
    } else if read == 0 {
        Err(format!(
            "connected to {addr} and the connection closed unanswered"
        ))
    } else {
        Err(format!(
            "connected to {addr} and got {:?}",
            line.lines().next().unwrap_or("")
        ))
    }
}

/// Start watching. Returns once the thread is running.
///
/// On giving up it logs why and exits 1. It does not checkpoint first: the
/// stall it is escaping is quite possibly the disk, and a checkpoint that
/// blocked there would keep a silent process alive, which is the one outcome
/// this exists to prevent. SQLite in WAL mode recovers on the next open.
/// Non-zero is deliberate: the launchd job restarts on an unsuccessful exit
/// only, and exit 0 is how this daemon says "stay down".
pub fn spawn(bound: SocketAddr, state: AppState) -> std::io::Result<()> {
    let addr = probe_address(bound);
    std::thread::Builder::new()
        .name("specline-watchdog".into())
        .spawn(move || {
            let mut strikes = Strikes::default();
            let mut lock = LockWatch::default();
            loop {
                std::thread::sleep(INTERVAL);
                if STANDING_DOWN.load(Ordering::SeqCst) {
                    return;
                }
                let outcome = probe(addr, PROBE_TIMEOUT);
                if let Err(reason) = &outcome {
                    tracing::warn!(%reason, "the daemon did not answer its own health check");
                }
                let unanswered = strikes.record(outcome.is_ok());
                let free = state.try_store().is_some();
                let wedged = lock.record(free, Instant::now());
                if STANDING_DOWN.load(Ordering::SeqCst) {
                    return;
                }
                if unanswered || wedged {
                    tracing::error!(
                        unanswered,
                        wedged,
                        "the daemon has stopped serving; exiting so the service manager \
                         restarts it (KEEL-403)"
                    );
                    std::process::exit(1);
                }
            }
        })
        .map(|_| ())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    const QUICK: Duration = Duration::from_millis(500);

    #[test]
    fn two_failures_in_a_row_give_up_and_one_does_not() {
        let mut strikes = Strikes::default();
        assert!(!strikes.record(false), "one slow moment is not a stall");
        assert!(strikes.record(false), "two in a row is");
    }

    #[test]
    fn an_answer_in_between_clears_the_count() {
        let mut strikes = Strikes::default();
        assert!(!strikes.record(false));
        assert!(!strikes.record(true));
        assert!(!strikes.record(false), "the count started again");
    }

    #[test]
    fn a_lock_held_at_every_look_for_long_enough_is_wedged() {
        let mut lock = LockWatch::default();
        let start = Instant::now();
        assert!(!lock.record(false, start));
        assert!(!lock.record(false, start + WEDGED_AFTER / 2));
        assert!(lock.record(false, start + WEDGED_AFTER));
    }

    /// Busy but moving: seen free once, and the clock starts again.
    #[test]
    fn one_free_look_resets_the_lock_clock() {
        let mut lock = LockWatch::default();
        let start = Instant::now();
        assert!(!lock.record(false, start));
        assert!(!lock.record(true, start + WEDGED_AFTER / 2));
        assert!(!lock.record(false, start + WEDGED_AFTER));
        assert!(lock.record(false, start + WEDGED_AFTER * 2));
    }

    #[test]
    fn an_unspecified_bind_is_probed_on_loopback() {
        let v4: SocketAddr = "0.0.0.0:7654".parse().unwrap();
        assert_eq!(probe_address(v4), "127.0.0.1:7654".parse().unwrap());
        let v6: SocketAddr = "[::]:7654".parse().unwrap();
        assert_eq!(probe_address(v6), "[::1]:7654".parse().unwrap());
        let exact: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        assert_eq!(probe_address(exact), exact);
    }

    /// The healthy case, answered the way the real endpoint answers.
    #[test]
    fn a_daemon_that_answers_200_passes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut buf = [0u8; 512];
            let _ = socket.read(&mut buf);
            let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
        });
        assert_eq!(probe(addr, QUICK), Ok(()));
    }

    /// The KEEL-403 shape: the kernel completes the handshake, nobody accepts,
    /// nothing comes back. A listener that is never `accept`ed reproduces it
    /// exactly, because the kernel queues the connection on its own.
    #[test]
    fn a_socket_that_connects_but_never_answers_fails() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let reason = probe(addr, QUICK).unwrap_err();
        assert!(reason.contains("no answer"), "{reason}");
        drop(listener);
    }

    #[test]
    fn nothing_listening_fails() {
        // Bind and drop, so the port is known to be free.
        let addr = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let reason = probe(addr, QUICK).unwrap_err();
        assert!(reason.contains("could not connect"), "{reason}");
    }

    #[test]
    fn an_error_status_is_not_health() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut buf = [0u8; 512];
            let _ = socket.read(&mut buf);
            let _ = socket.write_all(b"HTTP/1.1 503 Service Unavailable\r\n\r\n");
        });
        let reason = probe(addr, QUICK).unwrap_err();
        assert!(reason.contains("503"), "{reason}");
    }
}
