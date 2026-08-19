//! `specline-daemon` — argument parsing and process lifecycle.
//!
//! This was `src/main.rs` until KEEL-208. The binary now lives in the `specline`
//! package, because `dist` builds one installer per package that owns binaries
//! and the Phase 10 spec §1 promises exactly one. What is *in* the daemon did
//! not move: this module did, so the entry point is a shim over [`run`] rather
//! than 350 lines in a package whose other binary has nothing to do with
//! serving.
//!
//! Everything below the argument parsing lives in the rest of this crate, so
//! integration tests can drive the real router rather than a re-implementation
//! of it.

use crate::{AppState, router};
use anyhow::{Context, Result};
use clap::Parser;
use std::net::SocketAddr;
use std::path::PathBuf;

/// The Specline daemon.
#[derive(Parser, Debug)]
#[command(
    name = "specline-daemon",
    version,
    about = "Specline's MCP and local API daemon"
)]
struct Args {
    /// Where the store lives. Defaults to `~/.specline`.
    #[arg(long, env = "SPECLINE_HOME")]
    home: Option<PathBuf>,

    /// Address to bind.
    ///
    /// Loopback by default, and anything else is refused unless
    /// `--allow-network-access` says otherwise: the daemon has no
    /// authentication, and the MCP transport requires `Origin` validation
    /// precisely because a local server is reachable from any web page the user
    /// happens to have open.
    ///
    /// This used to read "until Phase 5", which was the phase that would have
    /// added authentication. Phase 5 is cut, so there is no later phase in which
    /// this relaxes on its own — hence the flag, which makes it a decision
    /// somebody takes rather than a default that drifts.
    // `SPECLINE_BIND` rather than `SPECLINE_DAEMON_URL`: this is a socket address, not
    // a URL, and conflating the two is how a client ends up trying to connect
    // to "127.0.0.1:7654" without a scheme. One name each, and both documented.
    #[arg(long, env = "SPECLINE_BIND", default_value = "127.0.0.1:7654")]
    bind: SocketAddr,

    /// Do not load the embedding model. Search will be keyword-only.
    ///
    /// On by default since B-95: it was opt-in, and the thing that actually
    /// happened is that nobody opted in — including on the machine this is
    /// written on, where every search ran keyword-only against a store whose
    /// vectors were all present and correct.
    /// The first start downloads 127 MB, which is the reason this flag exists —
    /// "not on this machine" is a real thing to want, and it should be one word
    /// rather than a rebuild.
    ///
    /// Keyword search covers every artifact either way, so this degrades rather
    /// than breaking.
    #[arg(long)]
    no_embeddings: bool,

    /// Accepted and ignored: embeddings are on by default now.
    ///
    /// Kept because service files on disk say `--embeddings`, and a daemon that
    /// refuses to start after an upgrade — `unexpected argument` from launchd,
    /// into a log nobody is watching — is a worse outcome than a dead flag.
    #[arg(long, hide = true)]
    embeddings: bool,

    /// Serve to the network, not just this machine. Read the whole sentence.
    ///
    /// The API has no authentication and it can write. Binding it anywhere but
    /// loopback hands every machine that can reach the address the ability to
    /// create, edit and archive anything in the store, with no credential and
    /// no audit beyond Specline's own event log.
    ///
    /// It exists because the honest alternative to a flag is somebody setting
    /// `SPECLINE_BIND=0.0.0.0:7654` to read the site from a laptop and never being
    /// told what they just did. A refusal with a flag named after the risk is a
    /// decision; a silent bind is an accident.
    #[arg(long)]
    allow_network_access: bool,
}

/// Run the daemon: parse arguments, open the store, serve, shut down cleanly.
///
/// Public because the binary that calls it lives in another package now. It
/// takes no arguments and reads the real environment on purpose — this is the
/// process, not a testable unit, and the parts worth testing (the bind refusal,
/// the router, the restart behaviour) are reachable without it.
///
/// `#[tokio::main]` builds the runtime and blocks, so a caller needs no async
/// of its own.
#[tokio::main]
pub async fn run() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "specline=info,specline_daemon=info,tower_http=warn".into()),
        )
        .init();

    let args = Args::parse();
    let home = match args.home {
        Some(h) => h,
        None => {
            let base = std::env::var_os("HOME")
                .map(PathBuf::from)
                .context("HOME is not set; pass --home")?;
            let home = base.join(specline_core::relocate::HOME_DIR);
            // Before the store is opened, and before this process takes the
            // lock on it — the relocation takes that same lock to prove nobody
            // else is writing, and a daemon that had already claimed the store
            // would be refused by its own guard.
            //
            // An explicit `--home` is never relocated. See `resolve_home` in
            // the CLI for why.
            if let Some(moved) = specline_core::relocate::relocate(
                &base.join(specline_core::relocate::LEGACY_HOME_DIR),
                &home,
            )? {
                tracing::info!(from = %moved.from.display(), to = %moved.to.display(), "relocated the store");
            }
            home
        }
    };

    // Before anything at all, because it may replace this process. A staged
    // update is applied at startup rather than when it is downloaded: swapping
    // the executable of a running daemon and carrying on leaves a process whose
    // binary no longer exists at its own path, and this is the one moment where
    // there is no half-updated state to reason about.
    apply_staged_update();

    // Before the store is opened, because it costs nothing and because taking
    // the store's exclusive lock only to reject the address a moment later
    // would leave a second daemon unable to start for a reason that has nothing
    // to do with it.
    check_bind_address(args.bind, args.allow_network_access)?;

    // Before anything is opened, because it is a statement about the location
    // rather than about the store, and because a person reading a log after a
    // corruption should find it above the failure rather than below it.
    for hazard in specline_core::hazards(&home) {
        tracing::warn!(
            home = %home.display(),
            "{}. {}",
            hazard.detail(),
            hazard.remedy()
        );
    }

    let state = match AppState::open(&home, !args.no_embeddings) {
        Ok(state) => state,
        // Exit zero on a store that will not open, and do not pretend that is
        // success — say exactly what happened first.
        //
        // The audience for the exit code is launchd, whose `KeepAlive` with
        // `SuccessfulExit: false` restarts on a *non-zero* exit. A store that
        // cannot be opened will not open on the next attempt either, so exiting
        // non-zero produces a restart every few seconds: a loop that re-runs
        // migration and re-attempts the model download forever, buries the real
        // error under thousands of copies of itself, and looks from the outside
        // like a crashing daemon rather than a store that needs attention.
        //
        // Staying down is the correct response to an unrecoverable condition,
        // and zero is how you say "stay down" to launchd.
        Err(e) => {
            tracing::error!(
                home = %home.display(),
                error = %format!("{e:#}"),
                "the store could not be opened, and this will not fix itself on a retry. \
                 Exiting without restarting — run `specline doctor` to see what is wrong, and \
                 `specline restore` if the file is damaged"
            );
            return Ok(());
        }
    };

    let app = router(state.clone());
    // The store's exclusive lock is already held by this point, so no other
    // specline-daemon can be serving *this* store. That narrows what a busy port
    // can mean to exactly one thing — something else is on it — and lets the
    // message say so rather than listing possibilities.
    //
    // No walking up the range to find a free port. The plugin's MCP config
    // expands its environment when Claude Code starts and its settings are
    // written at install time, so a daemon that quietly moved to 7655 would
    // leave both stale and MCP would fail with no explanation. A wandering port
    // and a static configuration file cannot both be right, and of the two the
    // configuration file is the one people can see.
    let listener = match tokio::net::TcpListener::bind(args.bind).await {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            anyhow::bail!(
                "{} is already in use, and it is not another Specline daemon holding this \
                 store — this process took that claim before reaching the socket.\n\n\
                 So something else is on the port. Specline does not go looking for a free one: \
                 the Claude Code plugin is configured with an address at install time and \
                 reads it at startup, so a daemon that moved would be a daemon nothing could \
                 find.\n\n\
                 Pick a port and tell both ends: --bind 127.0.0.1:<port>, or SPECLINE_BIND in \
                 the environment.",
                args.bind
            )
        }
        Err(e) => return Err(e).with_context(|| format!("bind {}", args.bind)),
    };

    // The address that was actually bound, not the one that was asked for.
    //
    // They differ whenever the port is 0, which is how you ask the operating
    // system to pick one — and a daemon that answers "I am listening on port 0"
    // has told you nothing. It falls back to the requested address if the
    // socket cannot say, because a slightly wrong log line is better than
    // failing to start over one.
    let bound = listener.local_addr().unwrap_or(args.bind);

    tracing::info!(
        home = %home.display(),
        bind = %bound,
        protocol = specline_mcp::PROTOCOL_VERSION,
        "specline-daemon listening"
    );
    tracing::info!("  MCP endpoint  http://{bound}/mcp");
    tracing::info!("  local API     http://{bound}/api");

    // Tell the tool layer where the interface it is describing actually is, so
    // a result can carry a link into it (KEEL-226). After the bind, because the
    // port may have been 0 — and a link to the port somebody asked for rather
    // than the one they got is exactly the wrong-by-default this exists to
    // avoid.
    specline_mcp::links::set_interface(&format!("http://{bound}"));

    // Record where we actually landed, so the CLI can find a daemon that is not
    // on the default port without being told.
    //
    // Written after the bind succeeded rather than before it, because the file
    // is meant to describe a daemon that exists. Best-effort: a home that is
    // read-only is a reason to log and keep serving, not a reason to refuse to
    // start — the file is a convenience for other processes, and the daemon's
    // actual job does not depend on it.
    let endpoint = home.join(specline_core::DAEMON_ENDPOINT_FILE);
    match serde_json::to_vec_pretty(&serde_json::json!({
        "url": format!("http://{bound}"),
        "pid": std::process::id(),
        "home": home.display().to_string(),
        "version": env!("CARGO_PKG_VERSION"),
    })) {
        Ok(bytes) => {
            if let Err(e) = std::fs::write(&endpoint, bytes) {
                tracing::warn!(
                    path = %endpoint.display(),
                    error = %e,
                    "could not record the endpoint; the CLI will fall back to the default port"
                );
            }
        }
        Err(e) => tracing::warn!(error = %e, "could not serialise the endpoint record"),
    }

    // Graceful shutdown, but on a deadline.
    //
    // `with_graceful_shutdown` waits for in-flight connections, and `/api/events`
    // is a Server-Sent Events stream that by design never ends. So the daemon
    // would sit there after SIGTERM until someone lost patience and sent
    // SIGKILL — which is how an ART index ends up disagreeing with its table,
    // and how this project spent an evening chasing a store that looked
    // corrupt while `fsck` insisted it was clean.
    spawn_update_check(state.clone());

    let deadline = std::time::Duration::from_secs(5);
    let serving = axum::serve(listener, app).with_graceful_shutdown(shutdown());
    tokio::select! {
        result = serving => result.context("serve")?,
        () = expire(deadline) => tracing::warn!(
            "graceful shutdown exceeded {deadline:?} — an open SSE stream is the usual \
             reason. Closing anyway, after a checkpoint."
        ),
    }

    // Take the endpoint record down before the store, so nothing reads it in
    // the window where the daemon is finishing up.
    //
    // This only runs on a graceful exit, and that is not a gap this tries to
    // close. A `SIGKILL` leaves the file behind by construction, so a stale one
    // is a case every reader has to handle anyway — which is why the constant's
    // documentation says presence is not liveness, and why the CLI probes
    // health rather than trusting the file. Cleaning up here keeps the ordinary
    // case tidy; it does not make the file trustworthy.
    if let Err(e) = std::fs::remove_file(&endpoint)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %endpoint.display(), error = %e, "could not remove the endpoint record");
    }

    // The last thing, always. An unflushed write is the whole failure mode.
    match state.store().checkpoint() {
        Ok(()) => tracing::info!("checkpointed; the write handle is released cleanly"),
        Err(e) => tracing::error!(error = %e, "checkpoint failed — the store may need a restore"),
    }
    Ok(())
}

/// Refuse to publish an unauthenticated write API to the network.
///
/// A pure function with the decision in it, rather than an `if` inside `main`,
/// because this is a security boundary and a security boundary nobody can write
/// a test against is a comment with a syntax highlighter.
///
/// The rule is deliberately about the *address* and not about the interface or
/// the subnet. `127.0.0.1` and `::1` pass; everything else — a LAN address, a
/// VPN address, `0.0.0.0`, `::` — is the same decision with the same
/// consequence, and drawing finer distinctions here would only produce cases
/// where the refusal is surprising.
fn check_bind_address(bind: SocketAddr, allowed: bool) -> Result<()> {
    if bind.ip().is_loopback() || allowed {
        return Ok(());
    }
    anyhow::bail!(
        "refusing to bind {bind} — that is not loopback, and this API has no \
         authentication.\n\n\
         Anything that can reach that address could create, edit and archive rows in \
         your store without a credential. The Origin and Host checks stop a web page \
         doing it through your browser; they do not stop a machine on your network \
         doing it directly.\n\n\
         If that is genuinely what you want — a home server, a machine whose whole \
         network you trust — pass --allow-network-access and it will bind. Otherwise \
         leave --bind on 127.0.0.1 and reach it over SSH port forwarding, which gets \
         you the same access without publishing the socket."
    )
}

/// Wait for the shutdown signal, then allow `grace` for in-flight work.
/// Swap in a release staged by an earlier run, then restart into it.
///
/// Every failure here is a warning and not a refusal. The daemon's job is to
/// serve the store; an update that cannot be applied is a reason to carry on
/// with the version that already works, not a reason to be unavailable. The one
/// thing it must never do is come up *half* updated, which `apply_staged`
/// prevents by treating a marker without both binaries as something to discard.
fn apply_staged_update() {
    let dir = match specline_update::install_dir() {
        Ok(dir) => dir,
        Err(e) => {
            tracing::warn!(
                "cannot tell where this binary is installed, so no staged update was \
                            looked for: {e:#}"
            );
            return;
        }
    };

    match specline_update::staged_version(&dir) {
        Ok(None) => {}
        Ok(Some(version)) => tracing::info!(
            %version,
            "Specline {version} is downloaded and waiting. It is not applied automatically — \
             applying it restarts the daemon, and that is yours to decide. Take it with: specline \
             update"
        ),
        Err(e) => tracing::warn!("could not read the staged update: {e:#}"),
    }
}

/// Replace this process with the binary now at its own path.
///
/// Returns only on failure. `exec` keeps the pid, which matters more than it
/// looks: launchd and systemd are watching this process, and exiting to be
/// restarted would work but would count as a crash against whatever restart
/// throttling they apply.
///
/// Called from the apply endpoint (B-75) rather than at startup. Startup
/// reports what is staged and applies nothing — a restart is agreed to, not
/// arranged (KEEL-225).
pub(crate) fn reexec() {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                tracing::error!(
                    "updated, but cannot find this binary's path to restart into it: {e}. \
                     Running the previous version until restarted."
                );
                return;
            }
        };
        // Only `exec` can fail here; on success this process is gone.
        let e = std::process::Command::new(exe)
            .args(std::env::args_os().skip(1))
            .exec();
        tracing::error!(
            "updated, but could not restart into the new binary: {e}. Running the previous \
             version until restarted."
        );
    }
    #[cfg(not(unix))]
    tracing::info!("updated. Restart the daemon to run the new version.");
}

/// How long to wait between update checks, given `SPECLINE_UPDATE_INTERVAL`.
///
/// A day was the wrong number and was chosen for the wrong project: it suits a
/// tool whose releases are rare, and Specline's currently land every few hours.
/// Half an hour is the default, and the variable takes seconds, because the
/// right number now is not the right number in six months and neither is worth
/// a release to change.
///
/// Nothing is applied by finding an update, so a short interval costs a request
/// and a staged file rather than a surprise restart — which is what made a day
/// feel like the safe choice in the first place.
///
/// The cost was measured rather than guessed before halving it again
/// (KEEL-317). A check that finds nothing is one unauthenticated GET of
/// `specline-release.json` — 1,545 bytes, around 0.3s — because
/// `check_and_stage` reads the manifest first and only downloads the 11 MB
/// archive on `Plan::Apply`. So the download is once per release rather than
/// once per check, and half-hourly is 48 requests and ~74 KB a day.
///
/// The floor is a minute and it is deliberate: this makes an unauthenticated
/// request to somebody else's server, and a mistyped variable should not turn
/// a laptop into a scraper. A value under it is ignored rather than clamped,
/// because the person who wrote `1` meant something the floor cannot honour
/// and should get the default rather than a number they did not choose.
///
/// Extracted from the task it runs in so the three ways it can be wrong — the
/// default, the parse and the floor — can be tested without waiting half an
/// hour for any of them.
fn check_interval(raw: Option<&str>) -> std::time::Duration {
    const DEFAULT: u64 = 30 * 60;
    const FLOOR: u64 = 60;

    let seconds = raw
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|seconds| *seconds >= FLOOR)
        .unwrap_or(DEFAULT);
    std::time::Duration::from_secs(seconds)
}

/// Look for a newer release on a schedule, and stage one when it is safe.
///
/// **This is the daemon's only outbound request.** Nothing else in this process
/// talks to anything but the loopback address, so it is worth it being one
/// obvious thing in one place rather than a capability spread around. It sends
/// nothing from the store — it fetches a file and compares two numbers.
///
/// `SPECLINE_AUTO_UPDATE=0` turns it off entirely, checked once here rather than
/// per tick so that switching it off means no task rather than a task that
/// wakes daily to decide it has nothing to do.
///
/// The first check waits five minutes. Starting the daemon should not depend on
/// the network being up, and a machine that has just booted is the case where
/// it most often is not.
fn spawn_update_check(state: AppState) {
    if !specline_update::auto_update_enabled() {
        tracing::info!("automatic update checks are off (SPECLINE_AUTO_UPDATE=0)");
        return;
    }

    let target = match specline_update::target() {
        Ok(target) => target,
        Err(e) => {
            tracing::warn!("not checking for updates: {e:#}");
            return;
        }
    };
    let dir = match specline_update::install_dir() {
        Ok(dir) => dir,
        Err(e) => {
            tracing::warn!("not checking for updates: {e:#}");
            return;
        }
    };

    tokio::spawn(async move {
        let settle = std::time::Duration::from_secs(5 * 60);
        let interval = check_interval(std::env::var("SPECLINE_UPDATE_INTERVAL").ok().as_deref());
        tokio::time::sleep(settle).await;

        let stamp_dir = dir.clone();
        loop {
            let dir = dir.clone();
            // Blocking: an HTTP fetch, a hash over 11 MB, and `tar`. On the
            // runtime's worker threads that would stall every request the
            // daemon is meant to be answering.
            let outcome =
                tokio::task::spawn_blocking(move || specline_update::check_and_stage(&dir, target))
                    .await;

            // Stamped whatever happened, because "when did this last check"
            // is the question a version on its own cannot answer, and a check
            // that has been failing quietly for a month looks exactly like one
            // that keeps finding nothing (KEEL-227).
            let failure = match &outcome {
                Ok(Ok(_)) => None,
                Ok(Err(e)) => Some(format!("{e:#}")),
                Err(e) => Some(e.to_string()),
            };
            if let Err(e) = specline_update::record_check(&stamp_dir, failure) {
                // A check that ran is a check that ran. Failing to write the
                // note about it is not a reason to stop checking.
                tracing::debug!("could not record the update check: {e:#}");
            }

            match outcome {
                Ok(Ok(specline_update::Plan::Apply { version, .. })) => {
                    tracing::info!(
                        %version,
                        "staged Specline {version}; it will be applied the next time the daemon \
                         starts"
                    );
                    // Told, not just logged. The app refetches health on any
                    // announced change, and this is the only thing that makes
                    // an update appear in the footer without waiting for an
                    // unrelated write to the store (KEEL-317).
                    state.announce_update(&version);
                }
                // Already staged, so nothing was downloaded and nothing is new.
                // Not announced: the app was told when it was staged, and
                // repeating it every interval would make the footer refetch
                // health for the rest of the day over one update.
                Ok(Ok(specline_update::Plan::AlreadyStaged { version })) => tracing::debug!(
                    %version,
                    "Specline {version} is already staged; waiting for a restart"
                ),
                Ok(Ok(specline_update::Plan::NeedsAPerson { version, from, to })) => {
                    tracing::info!(
                        %version,
                        "Specline {version} is available but changes the store's shape (schema {from} → \
                         {to}), so it is left for you: run `specline update` to see what it involves"
                    )
                }
                Ok(Ok(_)) => tracing::debug!("no update to take"),
                // A failed check is ordinary — a laptop asleep, no network, a
                // release without a manifest. It says so once and tries again
                // tomorrow rather than retrying into a log nobody can read.
                Ok(Err(e)) => tracing::info!("update check did not complete: {e:#}"),
                Err(e) => tracing::warn!("the update check task failed: {e}"),
            }

            tokio::time::sleep(interval).await;
        }
    });
}

async fn expire(grace: std::time::Duration) {
    shutdown().await;
    tokio::time::sleep(grace).await;
}

/// Wait for Ctrl-C or SIGTERM.
///
/// Graceful shutdown matters more here than in most servers: the daemon holds
/// the only write handle, and killing it mid-write is the one way to leave the
/// two engines disagreeing.
async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sig.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutting down; the write handle is released cleanly");
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn the_default_is_half_an_hour() {
        assert_eq!(check_interval(None).as_secs(), 30 * 60);
    }

    #[test]
    fn a_number_of_seconds_is_honoured() {
        assert_eq!(check_interval(Some("900")).as_secs(), 900);
        // Whitespace is what a shell export picks up by accident.
        assert_eq!(check_interval(Some("  120  ")).as_secs(), 120);
    }

    #[test]
    fn the_floor_is_a_minute_exactly() {
        assert_eq!(check_interval(Some("60")).as_secs(), 60);
    }

    /// The failure cases, which are the point of the floor and the parse.
    ///
    /// Every one of these falls back to the default rather than to something
    /// smaller. A mistyped variable that produced a one-second interval would
    /// make this daemon hammer github.com, and it would do it quietly.
    #[test]
    fn anything_unusable_falls_back_to_the_default() {
        for raw in [
            "0",
            "1",
            "59",
            "-30",
            "",
            "   ",
            "half an hour",
            "30m",
            "1e3",
        ] {
            assert_eq!(
                check_interval(Some(raw)).as_secs(),
                30 * 60,
                "{raw:?} should have fallen back to the default"
            );
        }
    }

    /// The default, and the only shape that should ever be silent.
    #[test]
    fn loopback_binds_without_being_asked_twice() {
        check_bind_address(addr("127.0.0.1:7654"), false).unwrap();
        check_bind_address(addr("[::1]:7654"), false).unwrap();
    }

    /// The case this exists for: one environment variable used to be enough to
    /// publish an unauthenticated write API to the network.
    #[test]
    fn a_network_address_is_refused_and_says_why() {
        for a in ["0.0.0.0:7654", "192.168.1.10:7654", "[::]:7654"] {
            let err = check_bind_address(addr(a), false)
                .expect_err("{a} is not loopback and should be refused");
            let message = err.to_string();
            assert!(
                message.contains("no authentication"),
                "the refusal has to say what the risk is, not just that it refused: {message}"
            );
            assert!(
                message.contains("--allow-network-access"),
                "and how to proceed if it really is what you want: {message}"
            );
        }
    }

    /// The escape works. A refusal with no way past it is one people route
    /// around by editing the source, which is worse than the bind.
    #[test]
    fn the_flag_is_a_real_escape() {
        check_bind_address(addr("0.0.0.0:7654"), true).unwrap();
    }
}
