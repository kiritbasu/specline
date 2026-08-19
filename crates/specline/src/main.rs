//! `specline` — the command-line client.
//!
//! Deliberately thin. Everything it does lives in `specline-core`; this crate
//! resolves a store path, parses arguments, and prints. That split is what
//! lets the daemon expose the same operations without either of them growing a
//! dependency on the other.
//!
//! Phase 0 gives it `fsck`, `backup`, `restore` and `fixture`. `render-status`
//! arrives in Phase 1, with the dogfooding switch.

mod bootstrap;
mod doctor;
mod gate;
mod generate;
mod hook;
mod import;
mod rubric;
mod work;
mod writes;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use specline_core::{Store, backup, fixture, fsck};
use std::path::{Path, PathBuf};

/// Specline's command-line client.
#[derive(Parser)]
#[command(name = "specline", version, about = "Specline — the project spine", long_about = None)]
struct Cli {
    /// Where the store lives. Defaults to `~/.specline`.
    ///
    /// `specline-core` never reads the environment; resolving this is the CLI's
    /// job, which is why the flag exists here and not there.
    #[arg(long, global = true, env = "SPECLINE_HOME")]
    home: Option<PathBuf>,

    /// Print machine-readable JSON instead of prose.
    #[arg(long, global = true)]
    json: bool,

    /// Write to the store even though a daemon appears to be running.
    ///
    /// Every command that writes probes for a daemon first and refuses if one
    /// answers, because the daemon owns the single write path and a write that
    /// goes round it skips six of the seven steps. This is the escape for the
    /// cases where a person knows better — a wedged daemon, a store being
    /// repaired. A flag rather than an environment variable, so that using it
    /// shows up in a shell history.
    #[arg(long, global = true)]
    force: bool,

    #[command(subcommand)]
    command: Command,
}

/// Which session hook is being run.
///
/// Separate from the daemon-facing commands because these have a different
/// caller and a different contract: Claude Code invokes them, the payload
/// arrives on stdin, and the exit code is always 0.
#[derive(Subcommand)]
enum HookCommand {
    /// Put the project digest into a session as it starts.
    ///
    /// Orientation stops being a decision the model makes and becomes something
    /// that happens to the session. Whether to *write* is still judgement, and
    /// that part stays in the skill.
    SessionStart {
        /// Daemon base URL. Defaults to `$SPECLINE_DAEMON_URL`, then the local daemon.
        #[arg(
            long,
            env = "SPECLINE_DAEMON_URL",
            default_value = "http://127.0.0.1:7654"
        )]
        daemon: String,
    },

    /// Ask, once, whether anything from this session should have been recorded.
    ///
    /// Silent for a session that already wrote, for a directory Specline does not
    /// know, and for a daemon it cannot reach.
    Stop {
        /// Daemon base URL. Defaults to `$SPECLINE_DAEMON_URL`, then the local daemon.
        #[arg(
            long,
            env = "SPECLINE_DAEMON_URL",
            default_value = "http://127.0.0.1:7654"
        )]
        daemon: String,
    },
}

#[derive(Subcommand)]
enum Command {
    /// Check the store's referential integrity.
    ///
    /// Exits non-zero if anything is actually broken, so it can gate a backup
    /// or a deploy.
    Fsck {
        /// Daemon base URL. Defaults to `$SPECLINE_DAEMON_URL`, then the local daemon.
        ///
        /// Read through the daemon when one is running. It is the single
        /// writer, so going through it is what guarantees a consistent read.
        #[arg(
            long,
            env = "SPECLINE_DAEMON_URL",
            default_value = "http://127.0.0.1:7654"
        )]
        daemon: String,
    },

    /// Ask whether anything has quietly gone wrong.
    ///
    /// Composes every read-only check there is — the file's own integrity,
    /// referential integrity, whether search has any vectors, whether the
    /// committed markdown still matches the store, how old the backup is, and
    /// whether the clock has stepped — into one page.
    ///
    /// Exits non-zero only for a real problem. A degraded store says so
    /// without failing, because a check that cannot tell the two apart is one
    /// nobody can put in a hook.
    Doctor {
        /// Daemon base URL. Defaults to `$SPECLINE_DAEMON_URL`, then the local daemon.
        #[arg(
            long,
            env = "SPECLINE_DAEMON_URL",
            default_value = "http://127.0.0.1:7654"
        )]
        daemon: String,
    },

    /// Open the interface in a browser.
    ///
    /// The daemon serves the read surface itself, compiled in, so this needs no
    /// Node and no second process — it works out where the daemon is listening
    /// and opens that. Which matters more than it sounds: the daemon records
    /// the address it actually bound, so a daemon told to use another port is
    /// still found without anyone remembering the number.
    ///
    /// It refuses rather than opening a browser at a dead port. A tab showing
    /// "cannot connect" is a worse answer than a sentence saying the daemon is
    /// not running.
    Ui {
        /// Daemon base URL. Defaults to `$SPECLINE_DAEMON_URL`, then the address the
        /// running daemon recorded, then the local default.
        #[arg(long, env = "SPECLINE_DAEMON_URL")]
        daemon: Option<String>,

        /// Print the address instead of opening anything.
        ///
        /// For a machine with no browser, and for anyone who would rather paste
        /// it themselves.
        #[arg(long)]
        print: bool,
    },

    /// Run a Claude Code session hook. Called by the plugin, not by a person.
    ///
    /// These were shell scripts until KEEL-206, and they needed `python3` and
    /// `curl` — neither declared anywhere, and `python3` absent on a Mac until
    /// the Xcode command line tools arrive. Every failure path exited 0
    /// silently, so on a fresh machine they did nothing and it looked exactly
    /// like Specline not working.
    ///
    /// Both read a JSON payload on stdin and write JSON on stdout. Both exit 0
    /// whatever happens: a hook that can block a session is worse than a hook
    /// that misses a record.
    #[command(subcommand)]
    Hook(HookCommand),

    /// Give every current revision that has no vector one.
    ///
    /// Embedding happens on the way into a new revision and nowhere else, so
    /// turning the feature on leaves the existing corpus invisible to the
    /// vector half of hybrid search — permanently, because nothing would ever
    /// rewrite those rows. This is the pass that fixes that.
    ///
    /// The first run downloads the model, which needs network access.
    Reembed {
        /// Only revisions with no vector at all. The default and, for now, the
        /// only mode: re-embedding rows that already have a vector is TQ-3,
        /// which is still open.
        #[arg(long, default_value_t = true)]
        missing: bool,
    },

    /// Back up the store: one consistent snapshot, plus a manifest.
    Backup {
        /// Where to write it. Defaults to `<home>/backups/<timestamp>`.
        #[arg(long)]
        dest: Option<PathBuf>,
    },

    /// Restore a backup into an empty directory.
    Restore {
        /// The backup directory to read.
        source: PathBuf,
        /// Where to restore to. Must not already contain a store.
        target: PathBuf,
    },

    /// Apply any schema migrations the store is missing.
    ///
    /// Nothing else applies them to a store that already exists. A migration
    /// changes what every process believes the tables look like, so it happens
    /// when someone asks for it and the daemon is stopped — not as a side
    /// effect of whichever command opened the store first after an upgrade.
    Migrate {
        /// The daemon to check for before touching anything.
        ///
        /// Same default and same environment variable as every other command
        /// with this flag. A migrate pointed at a port nothing is listening on
        /// would conclude no daemon is running and change the schema under one
        /// that is — which is the exact failure this command exists to prevent,
        /// arriving through a typo in its own default.
        #[arg(
            long,
            env = "SPECLINE_DAEMON_URL",
            default_value = "http://127.0.0.1:7654"
        )]
        daemon: String,
        /// Say what would be applied, and apply nothing.
        #[arg(long)]
        dry_run: bool,
    },

    /// Load the realistic fixture corpus into an empty store.
    Fixture,

    /// Import markdown files into Specline as versioned documents.
    ///
    /// Re-importing is safe: the same file lands on the same artifact, and
    /// unchanged content appends no revision. So the repo copy can stay
    /// authoritative for as long as you like with Specline kept in step.
    Import {
        /// Markdown files to import.
        files: Vec<PathBuf>,
        /// Project id, slug or name.
        #[arg(long)]
        project: String,
        /// What to store them as.
        #[arg(long, default_value = "spec")]
        r#as: String,
        /// Override the inferred spec kind: prd, spec, rfc, design-doc, feature, note.
        #[arg(long)]
        kind: Option<String>,
        /// Override the title, which otherwise comes from the first heading.
        #[arg(long)]
        title: Option<String>,
        /// Say what would land and write nothing.
        ///
        /// Worth using first on a repository you have not imported before.
        /// Soft delete is the only delete there is, so an artifact created by
        /// a wrong guess is archived rather than removed — it stays on disk,
        /// out of every view, for good.
        #[arg(long)]
        dry_run: bool,
    },

    /// Seed Specline's own project — the dogfooding switch.
    ///
    /// Imports the real state from the product docs: phases as milestones,
    /// the actual task list, the decision log, the open questions and the
    /// glossary. After this, `specline render-status specline` generates
    /// `product/STATUS.md` rather than a human maintaining it.
    Bootstrap {
        /// Repository path to record on the project, for the markdown mirror.
        #[arg(long)]
        repo: Option<String>,
        /// Archive every other project, leaving only Specline visible.
        ///
        /// Soft delete — the rows stay on disk, they just stop appearing.
        #[arg(long)]
        only: bool,
    },

    /// Print what a release of this binary promises, as JSON.
    ///
    /// The updater has to decide whether a new version is safe to apply
    /// *before* it downloads it, and it cannot ask the candidate binary —
    /// running the thing you are deciding whether to run is the whole problem.
    /// So each release publishes this alongside its artifacts and the updater
    /// reads it first.
    ///
    /// Opens no store, which is the point: it is run in a release job against
    /// a freshly built binary on a machine that has no Specline home at all.
    ReleaseManifest,

    /// Install the latest release, if it cannot change the store's shape.
    ///
    /// A release that agrees with this one about the schema is interchangeable
    /// as far as your store is concerned, so it is applied without asking. One
    /// that moves the schema stops here and waits for you, because a migration
    /// rewrites data and `--rollback` puts binaries back, not rows.
    ///
    /// Downloaded from the latest release over plain HTTPS — no account and no
    /// token — and checked against the SHA-256 in the release manifest before
    /// anything is moved into place.
    Update {
        /// Say what would happen and change nothing.
        #[arg(long)]
        check: bool,
        /// Put back the binaries the last update replaced.
        ///
        /// One generation only, and binaries only. If the update you are
        /// undoing migrated the store, this does not undo that — restore a
        /// backup.
        #[arg(long, conflicts_with = "check")]
        rollback: bool,
        /// Daemon base URL. Defaults to `$SPECLINE_DAEMON_URL`, then the local daemon.
        ///
        /// Replacing the binaries is half of an update: the daemon is another
        /// process and goes on running what it loaded at startup. This is the
        /// one asked to restart into the new version afterwards, and the one
        /// checked to see which version came back.
        #[arg(
            long,
            env = "SPECLINE_DAEMON_URL",
            default_value = "http://127.0.0.1:7654"
        )]
        daemon: String,
    },

    /// Print a one-line summary of what is in the store.
    Status {
        /// Daemon base URL. Defaults to `$SPECLINE_DAEMON_URL`, then the local daemon.
        ///
        /// Read through the daemon when one is running. It is the single
        /// writer, so going through it is what guarantees a consistent read.
        #[arg(
            long,
            env = "SPECLINE_DAEMON_URL",
            default_value = "http://127.0.0.1:7654"
        )]
        daemon: String,
    },

    /// Regenerate a project's repository files from Specline.
    ///
    /// Specline is the source of truth; the markdown in the repo is an output.
    /// This writes the adopted prose files at their recorded paths, the
    /// `.specline/` mirror for everything born in Specline, and the tracker.
    ///
    /// One-directional: nothing here reads a generated file back into the
    /// store. It goes through the running daemon, which owns the store —
    /// falling back to opening the store directly only when no daemon is up.
    Generate {
        /// Project id, slug or name.
        project: String,
        /// Repository root. Defaults to the project's recorded `root_path`.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Report what would change and exit non-zero if anything would.
        ///
        /// For a pre-commit hook or CI: makes a hand edit to a generated file
        /// a failure someone sees rather than work someone silently loses.
        #[arg(long)]
        check: bool,
        /// Daemon base URL. Defaults to `$SPECLINE_DAEMON_URL`, then the local daemon.
        #[arg(
            long,
            env = "SPECLINE_DAEMON_URL",
            default_value = "http://127.0.0.1:7654"
        )]
        daemon: String,
    },

    /// Score Phase 2's exit criterion from the event log.
    ///
    /// Does not run the sessions — "unprompted" is the whole claim and a test
    /// that calls the tool has prompted it. This scores what the sessions did.
    Gate {
        /// Restrict to one project.
        #[arg(long)]
        project: Option<String>,
        /// Only count activity after this instant (RFC 3339).
        #[arg(long)]
        since: Option<String>,
        /// Score from an archived run directory instead of the event log.
        ///
        /// Transcript-based: one file per session, so ids cannot collide, and
        /// a session that only *offered* to write is visible. This is the mode
        /// to use for a real run.
        #[arg(long)]
        run: Option<PathBuf>,
        /// How many sessions were run. The denominator.
        ///
        /// Not derived from the log: a session that wrote nothing leaves no
        /// event, so the log cannot tell you it happened.
        #[arg(long, default_value_t = 10)]
        sessions: usize,
        /// Daemon base URL. Defaults to `$SPECLINE_DAEMON_URL`, then the local daemon.
        #[arg(
            long,
            env = "SPECLINE_DAEMON_URL",
            default_value = "http://127.0.0.1:7654"
        )]
        daemon: String,
    },

    /// Print the generated tracker for a project to standard output.
    RenderStatus {
        /// Project id, slug or name. Must match exactly one project.
        project: String,
        /// Daemon base URL. Defaults to `$SPECLINE_DAEMON_URL`, then the local daemon.
        #[arg(
            long,
            env = "SPECLINE_DAEMON_URL",
            default_value = "http://127.0.0.1:7654"
        )]
        daemon: String,
        /// Write here instead of standard output.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Write even if the result is dramatically smaller than what is there.
        #[arg(long)]
        force: bool,
    },

    /// Append to a row's running commentary, or read it back.
    ///
    /// The commentary is what the tracker's prose used to carry. Having it on
    /// the CLI matters beyond convenience: it is the only write path that does
    /// not go through MCP, so a note can still be recorded when the MCP surface
    /// is unavailable.
    Note {
        #[command(subcommand)]
        action: NoteAction,
    },

    /// Archive a row. Soft delete — it stays on disk and stops appearing.
    ///
    /// Needed for the same reason `task` is: with MCP down there was no way to
    /// retire a row, and a document that has outlived its purpose keeps owning
    /// the file path it adopted.
    Archive {
        /// The entity id.
        id: String,
        /// The version you believe is current, for optimistic concurrency.
        #[arg(long)]
        version: i32,
    },

    /// Report the rows a reader would struggle with. Never rewrites one.
    ///
    /// Three rules arrived after most of this store existed — a task needs a
    /// summary, a close needs a reason, prose should not lean on a bare
    /// identifier — and none of them can be enforced backwards. This is the
    /// list a person works through.
    ///
    /// It does not fix anything, and that is the design: a machine filling in a
    /// missing summary would write exactly the confident, plausible, wrong
    /// prose the requirement exists to prevent.
    Lint {
        /// Project id, slug or name.
        project: String,
        /// Only this rule: task_without_summary, unexpanded_identifier,
        /// closed_without_reason.
        #[arg(long)]
        check: Option<String>,
        /// How many findings to print. The total is reported either way.
        #[arg(long, default_value_t = 40)]
        limit: usize,
        /// Daemon base URL. Defaults to `$SPECLINE_DAEMON_URL`, then the local daemon.
        #[arg(
            long,
            env = "SPECLINE_DAEMON_URL",
            default_value = "http://127.0.0.1:7654"
        )]
        daemon: String,
    },

    /// What can be worked on right now, best first.
    ///
    /// Open work with nothing live in its way, grouped and then oldest first.
    /// What a task unblocks still leads where that means something; on a store
    /// where nothing blocks anything it does not, so the group decides (B-83).
    ///
    /// The same computation the MCP tool and the app read. There is one
    /// ranking, so the three cannot disagree.
    ///
    /// `ready` still works. It was the name until B-85, and a verb somebody has
    /// in a shell history should not start erroring over a rename.
    #[command(alias = "ready")]
    Next {
        /// Project id, slug or name.
        project: String,
        /// Only work nobody is holding.
        #[arg(long)]
        unclaimed: bool,
        /// Only tasks carrying all of these labels. Repeatable.
        #[arg(long = "label")]
        labels: Vec<String>,
        /// Skip tasks carrying any of these labels. Repeatable.
        #[arg(long = "no-label")]
        no_labels: Vec<String>,
        /// Only work under this milestone. An id, or a name like "Phase 8".
        #[arg(long)]
        milestone: Option<String>,
        /// How many to show.
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Daemon base URL. Defaults to `$SPECLINE_DAEMON_URL`, then the local daemon.
        #[arg(
            long,
            env = "SPECLINE_DAEMON_URL",
            default_value = "http://127.0.0.1:7654"
        )]
        daemon: String,
    },

    /// Take a task: move it to in_progress and record who is on it.
    ///
    /// Refused if another session holds it, unless that claim has gone stale
    /// after three days or `--force` is passed. Closing releases it.
    Claim {
        /// The task — `KEEL-42` or a `tsk_…` id.
        task: String,
        /// Take a claim another session still holds.
        #[arg(long)]
        force: bool,
        /// The session doing the work. Specline never invents one.
        #[arg(long, env = "SPECLINE_SESSION")]
        session: Option<String>,
        /// Daemon base URL. Defaults to `$SPECLINE_DAEMON_URL`, then the local daemon.
        #[arg(
            long,
            env = "SPECLINE_DAEMON_URL",
            default_value = "http://127.0.0.1:7654"
        )]
        daemon: String,
    },

    /// Close a task, saying why and showing the work.
    ///
    /// `done` needs a message and at least one piece of evidence; `wont_do` and
    /// `no_change` need a message; `duplicate` and `superseded` name the other
    /// task and draw the edge themselves.
    Close {
        /// The task — `KEEL-42` or a `tsk_…` id.
        task: String,
        /// done, wont_do, duplicate, superseded, no_change.
        #[arg(long)]
        reason: String,
        /// What actually happened, in a sentence or two.
        #[arg(long, short)]
        message: String,
        /// Typed proof. `commit:<sha>`, `pr:<url>`, `test:<command>`,
        /// `doc:<entity-id>`, `url:<url>`, `image:<blob-id>`. Repeatable.
        #[arg(long = "evidence")]
        evidence: Vec<String>,
        /// For `duplicate` and `superseded`: the other task.
        #[arg(long)]
        other: Option<String>,
        /// The session closing it, for attribution.
        #[arg(long, env = "SPECLINE_SESSION")]
        session: Option<String>,
        /// Daemon base URL. Defaults to `$SPECLINE_DAEMON_URL`, then the local daemon.
        #[arg(
            long,
            env = "SPECLINE_DAEMON_URL",
            default_value = "http://127.0.0.1:7654"
        )]
        daemon: String,
    },

    /// Create a task row.
    ///
    /// Exists because until now the only ways to bring a row into being were
    /// MCP and the one-shot `bootstrap`/`import` migrations. With MCP down
    /// there was no way at all, which is how four completed pieces of work
    /// ended up as lines in a markdown table and nowhere else.
    Task {
        /// Project id, slug or name.
        #[arg(long)]
        project: String,
        /// The task title.
        title: String,
        /// Longer description.
        #[arg(long)]
        body: Option<String>,
        /// todo, in_progress, blocked, done, dropped.
        #[arg(long, default_value = "todo")]
        status: String,
        /// p0, p1, p2, p3.
        #[arg(long, default_value = "p2")]
        priority: String,
    },
    /// File a signal into the Inbox — something somebody wants, before
    /// anybody has decided whether to build it.
    ///
    /// A signal is not a task. Nothing has been committed to, there is
    /// nothing to claim, and it stays out of `next` and out of the open
    /// count until somebody triages it. The only required argument is
    /// what was said, because capture that costs more than the thought
    /// did is capture that does not happen.
    Signal {
        /// Project id, slug or name.
        #[arg(long)]
        project: String,
        /// What was said, in their words. Not a title — a signal has none.
        summary: String,
        /// interview, support, sales, idea, competitor, observation.
        #[arg(long, default_value = "idea")]
        kind: String,
        /// Who said it, or where it came from.
        #[arg(long)]
        source: Option<String>,
        /// How to reach them, if closing the loop will need it.
        #[arg(long)]
        contact: Option<String>,
        /// When it was said, if that was not today.
        #[arg(long)]
        occurred_at: Option<String>,
        /// The verbatim, or the context. Optional, and kept when given.
        #[arg(long)]
        body: Option<String>,
    },
    /// Triage a signal: pick it up, or set it down with the argument.
    ///
    /// A signal cannot leave the Inbox without an outcome, which is why this
    /// is a verb of its own rather than a field somebody sets. Setting one
    /// down is not deleting it: the argument is written onto the signal where
    /// search will find it, and the same idea arriving in four months finds
    /// the reasoning instead of silence.
    Triage {
        /// The signal, `fbk_…`.
        id: String,
        /// Pick it up: the `spc_…` feature spec making the case for building it.
        #[arg(long, conflicts_with = "set_down")]
        feature: Option<String>,
        /// Set it down: why, in a sentence worth finding later.
        #[arg(long, conflicts_with = "feature")]
        set_down: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum NoteAction {
    /// Append a note to a row.
    Add {
        /// The row to annotate. Any entity id.
        entity: String,
        /// The note. A finding, a decision, an observation.
        body: String,
        /// The conversation responsible, so the note stays traceable.
        #[arg(long)]
        session: Option<String>,
    },
    /// Print a row's notes, oldest first.
    Ls {
        /// The row whose commentary to read.
        entity: String,
        /// Include retracted notes.
        #[arg(long)]
        all: bool,
    },
    /// Retract a note. Soft, like every other removal in the store.
    Retract {
        /// The note id, `nte_…`.
        id: String,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "specline=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    // The hooks are dispatched before the store location is resolved, and that
    // ordering is the whole point rather than a tidiness.
    //
    // `resolve_home` fails when `HOME` is unset — correctly, for every command
    // that needs a store. A hook needs none: it talks to the daemon over HTTP
    // and never opens the file. Leaving it below the resolution meant
    // `specline hook session-start` exited 1 with "HOME is not set" in any stripped
    // environment, and a hook that exits non-zero is the one thing these must
    // never do. Caught by `tests/hooks.rs` running the binary under `env -i`,
    // which is exactly the shape of the environment the bash version was
    // silently failing in for a different reason.
    if let Command::Hook(which) = &cli.command {
        match which {
            HookCommand::SessionStart { daemon } => hook::session_start(daemon),
            HookCommand::Stop { daemon } => hook::stop(daemon),
        }
        return Ok(());
    }

    let home = resolve_home(cli.home.clone())?;

    match &cli.command {
        Command::Fsck { daemon } => run_fsck(&home, daemon, cli.json),
        Command::Doctor { daemon } => doctor::run(&home, daemon, cli.json),
        Command::Ui { daemon, print } => run_ui(&home, daemon.as_deref(), *print, cli.json),
        // Handled above, before the store location is resolved.
        Command::Hook(_) => Ok(()),
        Command::Reembed { missing } => run_reembed(&home, *missing, cli.force, cli.json),
        Command::Backup { dest } => run_backup(&home, dest.clone(), cli.json),
        Command::Restore { source, target } => run_restore(source, target, cli.json),
        Command::Migrate { daemon, dry_run } => {
            run_migrate(&home, daemon, *dry_run, cli.force, cli.json)
        }
        Command::Fixture => run_fixture(&home, cli.force, cli.json),
        Command::ReleaseManifest => run_release_manifest(),
        Command::Update {
            check,
            rollback,
            daemon,
        } => specline_update::run(*check, *rollback, cli.json, daemon, &home),
        Command::Status { daemon } => run_status(&home, daemon, cli.json),
        Command::RenderStatus {
            project,
            daemon,
            out,
            force,
        } => run_render_status(&home, daemon, project, out.clone(), *force),
        Command::Note { action } => run_note(&home, action, cli.force, cli.json),
        Command::Archive { id, version } => {
            use specline_core::{Actor, EntityId, EntityStore, Provenance, Surface};
            let mut store = writes::open_for_write(
                &home,
                &writes::daemon_url_for(&home),
                cli.force,
                "archive a row",
            )?;
            let prov = Provenance::anonymous(Actor::Human).with_surface(Surface::Cli);
            let archived = store.archive(&EntityId::parse(id)?, *version, &prov)?;
            println!("{} — archived", archived.id());
            Ok(())
        }
        Command::Lint {
            project,
            check,
            limit,
            daemon,
        } => work::lint(daemon, project, check.as_deref(), *limit, cli.json),
        Command::Next {
            project,
            unclaimed,
            labels,
            no_labels,
            milestone,
            limit,
            daemon,
        } => work::ready(
            &home,
            daemon,
            project,
            *unclaimed,
            labels,
            no_labels,
            milestone.as_deref(),
            *limit,
            cli.json,
        ),
        Command::Claim {
            task,
            force,
            session,
            daemon,
        } => work::claim(&home, daemon, task, *force, session.as_deref(), cli.json),
        Command::Close {
            task,
            reason,
            message,
            evidence,
            other,
            session,
            daemon,
        } => work::close(
            &home,
            daemon,
            task,
            reason,
            message,
            evidence,
            other.as_deref(),
            session.as_deref(),
            cli.json,
        ),
        Command::Task {
            project,
            title,
            body,
            status,
            priority,
        } => run_task_add(
            &home,
            TaskDraft {
                project,
                title,
                body: body.clone(),
                status,
                priority,
            },
            cli.force,
            cli.json,
        ),
        Command::Signal {
            project,
            summary,
            kind,
            source,
            contact,
            occurred_at,
            body,
        } => run_signal_add(
            &home,
            SignalDraft {
                project,
                summary,
                kind,
                source: source.clone(),
                contact: contact.clone(),
                occurred_at: occurred_at.clone(),
                body: body.clone(),
            },
            cli.force,
            cli.json,
        ),
        Command::Triage {
            id,
            feature,
            set_down,
        } => run_triage(
            &home,
            id,
            feature.as_deref(),
            set_down.as_deref(),
            cli.force,
            cli.json,
        ),
        Command::Gate {
            project,
            since,
            run,
            sessions,
            daemon,
        } => match run {
            Some(dir) => gate::score_run(dir, cli.json),
            None => gate::run(
                daemon,
                project.as_deref(),
                since.as_deref(),
                *sessions,
                cli.json,
            ),
        },
        Command::Generate {
            project,
            repo,
            check,
            daemon,
        } => generate::run(&home, project, repo.clone(), *check, daemon, cli.json),
        Command::Bootstrap { repo, only } => run_bootstrap(&home, repo.clone(), *only, cli.json),
        Command::Import {
            files,
            project,
            r#as,
            kind,
            title,
            dry_run,
        } => run_import(
            &home,
            files,
            project,
            r#as,
            kind.clone(),
            title.clone(),
            *dry_run,
            cli.force,
            cli.json,
        ),
    }
}

/// Print what an import would do, and do none of it.
///
/// The columns are chosen for the question an adopter is actually asking:
/// *have I pointed this at the right files, and will it land where I think*.
/// So the outcome comes first, the title second — because a wrong title is a
/// wrong artifact — and the adopted path is called out only when importing
/// would change it, which is the surprise that costs someone a file the next
/// time `specline generate` runs.
fn preview_import(
    store: &Store,
    files: &[PathBuf],
    project_id: &specline_core::EntityId,
    entity_type: specline_core::EntityType,
    title: Option<String>,
    json: bool,
) -> Result<()> {
    let mut rows = Vec::new();
    for path in files {
        let p = import::preview(store, path, project_id, entity_type, title.clone())?;
        rows.push((path.clone(), p));
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!(
                rows.iter()
                    .map(|(path, p)| serde_json::json!({
                        "file": path.display().to_string(),
                        "outcome": p.outcome.word(),
                        "id": p.entity_id.as_ref().map(|i| i.as_str()),
                        "title": p.title,
                        "bytes": p.bytes,
                        "mirror_path": p.mirror_path,
                        "mirror_path_now": p.mirror_path_now,
                    }))
                    .collect::<Vec<_>>()
            ))?
        );
        return Ok(());
    }

    for (path, p) in &rows {
        println!(
            "  {:<9}  {}  →  {}",
            p.outcome.word(),
            path.display(),
            p.title
        );
        if let Some(from) = &p.mirror_path_now {
            println!(
                "             adopted path changes: {from} → {}",
                p.mirror_path.as_deref().unwrap_or("(none)")
            );
        }
    }

    let creates = rows
        .iter()
        .filter(|(_, p)| p.outcome == import::Outcome::Create)
        .count();
    let unchanged = rows
        .iter()
        .filter(|(_, p)| matches!(p.outcome, import::Outcome::Unchanged { .. }))
        .count();
    let revises = rows.len() - creates - unchanged;
    println!(
        "\n{} file(s): {creates} would create, {revises} would revise, {unchanged} unchanged",
        rows.len()
    );
    println!("nothing was written — drop --dry-run to import");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_import(
    home: &Path,
    files: &[PathBuf],
    project: &str,
    as_type: &str,
    kind: Option<String>,
    title: Option<String>,
    dry_run: bool,
    force: bool,
    json: bool,
) -> Result<()> {
    use specline_core::{EntityType, SpecKind};

    if files.is_empty() {
        bail!("no files given. Pass one or more markdown paths");
    }
    if title.is_some() && files.len() > 1 {
        bail!(
            "--title applies to a single file; {} were given",
            files.len()
        );
    }

    let entity_type = EntityType::parse(as_type)?;
    let kind = match kind {
        Some(k) => Some(SpecKind::parse(&k)?),
        None => None,
    };

    // A dry run opens read-only: no advisory lock, no daemon probe, nothing to
    // refuse. Previewing an import is a read, and the machine it matters on is
    // one with a daemon already running.
    //
    // The real import goes through `open_for_write` like every other command
    // that writes. It did not before this — it called `open` directly, so it
    // was the one writer that went round both the probe and the lock, which is
    // exactly what hard constraint 1 is about. `--force` is the same escape it
    // is everywhere else.
    let mut store = if dry_run {
        open(home)?
    } else {
        writes::open_for_write(home, &writes::daemon_url_for(home), force, "import files")?
    };
    let found = resolve_project(&store, project)?;
    let project_id = found.id().clone();

    if dry_run {
        return preview_import(&store, files, &project_id, entity_type, title, json);
    }

    let mut rows = Vec::new();
    for path in files {
        let imported = import::file(
            &mut store,
            path,
            &project_id,
            entity_type,
            kind,
            title.clone(),
        )?;
        rows.push((path.clone(), imported));
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!(
                rows.iter()
                    .map(|(path, i)| serde_json::json!({
                        "file": path.display().to_string(),
                        "id": i.entity_id.as_str(),
                        "title": i.title,
                        "version": i.version,
                        "created": i.created,
                        "revised": i.revised,
                        "bytes": i.bytes,
                        "mirror_path": i.mirror_path,
                    }))
                    .collect::<Vec<_>>()
            ))?
        );
    } else {
        for (path, i) in &rows {
            let what = if i.created {
                "created"
            } else if i.revised {
                "revised"
            } else {
                "unchanged"
            };
            // Naming the adopted path is the point of the line: it is what
            // `specline generate` will write back over, and a surprise there is
            // the one that costs someone a file.
            let adopted = match &i.mirror_path {
                Some(p) => format!("  → generates {p}"),
                None => "  → no repo path; goes to the .keel mirror".to_owned(),
            };
            println!(
                "{what:>9}  {}  v{}  {} bytes  {}{adopted}",
                i.title,
                i.version,
                i.bytes,
                path.display()
            );
        }
    }
    Ok(())
}

fn run_bootstrap(home: &Path, repo: Option<String>, only: bool, json: bool) -> Result<()> {
    // One of the two commands that may make a store rather than find one.
    let mut store = create_or_open(home)?;
    let summary = bootstrap::run(&mut store, repo)?;

    let archived = if only {
        bootstrap::archive_other_projects(&mut store, &summary.project_id)?
    } else {
        0
    };

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "project_id": summary.project_id.as_str(),
                "entities": summary.entities,
                "links": summary.links,
                "revisions": summary.revisions,
                "archived": archived,
            }))?
        );
    } else {
        println!(
            "seeded Specline: {} entities, {} links, {} document revisions",
            summary.entities, summary.links, summary.revisions
        );
        println!("  project {}", summary.project_id);
        if only {
            println!("  archived {archived} artifact(s) belonging to other projects");
        }
        println!();
        println!("Specline now tracks itself. Regenerate the tracker with:");
        println!("  specline render-status specline --out product/STATUS.md");
    }
    Ok(())
}

/// Resolve a project by id, slug or name.
pub(crate) fn resolve_project(store: &Store, reference: &str) -> Result<specline_core::Entity> {
    use specline_core::{Entity, EntityQuery, EntityStore, EntityType};
    let projects = store.list(&EntityQuery::default().of_type(EntityType::Project))?;
    let needle = reference.to_lowercase();
    let mut matches: Vec<specline_core::Entity> = projects
        .items
        .into_iter()
        .filter(|p| match p {
            Entity::Project(pr) => {
                pr.id.as_str() == reference
                    || pr.slug.eq_ignore_ascii_case(reference)
                    || pr.name.to_lowercase() == needle
            }
            _ => false,
        })
        .collect();

    // More than one match is refused rather than resolved by taking the first.
    // Silently picking one is how a render lands in the wrong project's file.
    if matches.len() > 1 {
        let names: Vec<String> = matches
            .iter()
            .map(|p| match p {
                Entity::Project(pr) => format!("{} ({})", pr.slug, pr.id),
                other => other.id().to_string(),
            })
            .collect();
        anyhow::bail!(
            "`{reference}` matches {} projects: {}. Name one exactly.",
            names.len(),
            names.join(", ")
        );
    }

    matches
        .pop()
        .with_context(|| format!("no project matches `{reference}`"))
}

/// How much smaller than the file it replaces a render may be before it stops
/// and asks.
///
/// A tracker does not lose half its content by accident. Pointed at the wrong
/// project — one that is near-empty — this is what stands between a mistyped
/// argument and a file nobody kept a copy of.
const SHRINK_FLOOR: f64 = 0.5;

fn run_render_status(
    home: &Path,
    daemon: &str,
    project: &str,
    out: Option<PathBuf>,
    force: bool,
) -> Result<()> {
    // Percent-encode by hand rather than adding a crate for it. A project
    // reference is a slug, key or name, so the reachable characters are few —
    // but a name with a space or an ampersand would otherwise truncate the
    // query and render the wrong project's tracker, which is silent and wrong.
    let encoded: String = project
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect();
    let path_and_query = format!("/api/render-status?project={encoded}");
    let markdown = match read_via_daemon(daemon, &path_and_query)? {
        Some(v) => v
            .get("markdown")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .context("the daemon's render-status response had no markdown")?,
        None => {
            let store = open(home)?;
            let found = resolve_project(&store, project)?;
            specline_core::render_status::render(&store, found.id())?
        }
    };
    let Some(path) = out else {
        print!("{markdown}");
        return Ok(());
    };

    // Compare before writing. The previous version wrote unconditionally, which
    // meant a regeneration that changed nothing still dirtied the tree, and a
    // regeneration that destroyed everything looked exactly the same.
    //
    // Compared with the banner stripped, because the banner carries a
    // generation timestamp: byte equality would never hold and the comparison
    // would be decoration. This is the same rule `specline generate --check` uses,
    // and using a different one here is how the two would disagree about
    // whether a file had changed.
    let existing = std::fs::read_to_string(&path).ok();
    if let Some(before) = &existing
        && specline_core::generate::strip_banner_public(before)
            == specline_core::generate::strip_banner_public(&markdown)
    {
        println!("{} is already up to date", path.display());
        return Ok(());
    }

    if let Some(before) = &existing
        && !force
        && !before.is_empty()
        && (markdown.len() as f64) < before.len() as f64 * SHRINK_FLOOR
    {
        anyhow::bail!(
            "refusing to write {}: the new tracker is {} bytes and the file there is {}. \
             That is the shape of a render pointed at the wrong project — check `{}` is the \
             one you meant, or pass --force if the shrink is real.",
            path.display(),
            markdown.len(),
            before.len(),
            project
        );
    }

    // Atomic, like every other generated file: a torn tracker is a file that
    // opens, reads as plausible and stops mid-table.
    specline_core::atomic::write(&path, &markdown)
        .with_context(|| format!("write {}", path.display()))?;
    println!("wrote {} ({} bytes)", path.display(), markdown.len());
    Ok(())
}

fn run_note(home: &Path, action: &NoteAction, force: bool, json: bool) -> Result<()> {
    use specline_core::{Actor, EntityId, EntityStore, NewNote, NoteId, Provenance, Surface};

    // `Ls` only reads, but it shares this store handle with `Add` and
    // `Retract`, and a read is safe alongside a daemon in WAL mode. Probing for
    // all three keeps the funnel one function rather than three.
    let mut store =
        writes::open_for_write(home, &writes::daemon_url_for(home), force, "write a note")?;
    // `cli` rather than `code`: this is a person at a terminal, and the whole
    // point of `surface` is telling those apart when reading the history back.
    let prov = Provenance::anonymous(Actor::Human).with_surface(Surface::Cli);

    match action {
        NoteAction::Add {
            entity,
            body,
            session,
        } => {
            let id = EntityId::parse(entity)?;
            let mut note = NewNote::new(id, body.clone(), Actor::Human);
            if let Some(s) = session {
                note = note.in_session(s.clone());
            }
            let written = store.add_note(note, &prov)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&written)?);
            } else {
                println!("{} — noted on {}", written.id, written.entity_id);
            }
        }
        NoteAction::Ls { entity, all } => {
            let id = EntityId::parse(entity)?;
            let notes = store.notes_for(&id, *all)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&notes)?);
            } else if notes.is_empty() {
                println!("no notes on {id}");
            } else {
                for n in &notes {
                    let mark = if n.is_live() { " " } else { "×" };
                    println!(
                        "{mark} {} · {} · {}\n  {}",
                        n.id,
                        n.author,
                        n.created_at.format("%Y-%m-%d %H:%M"),
                        n.body.replace('\n', "\n  ")
                    );
                }
            }
        }
        NoteAction::Retract { id } => {
            let note = store.retract_note(&NoteId::parse(id)?, &prov)?;
            println!("{} — retracted", note.id);
        }
    }
    Ok(())
}

/// What `specline task` was asked to create.
///
/// A struct rather than six parameters because it was already at the edge of
/// readable and `--force` pushed it over — and because the fields arrive
/// together from one clap variant, so they belong together here too.
struct TaskDraft<'a> {
    project: &'a str,
    title: &'a str,
    body: Option<String>,
    status: &'a str,
    priority: &'a str,
}

/// Refuse unless the Inbox is switched on.
///
/// Off by default (KEEL-341): v0.4.0 shipped filing and the nav item without
/// triage, so signals could go in and not come out. The refusal names the
/// variable, because somebody who typed `specline signal` meant to file one
/// and "unknown command" would be a worse answer than "not yet, here is how".
fn require_inbox() -> Result<()> {
    if !specline_mcp::dispatch::surfaces().inbox {
        anyhow::bail!(
            "the Inbox is switched off. Set SPECLINE_INBOX=1 to switch it on.\n\n\
             It is off by default while the feature-request lifecycle is unfinished. \
             Switching it on hides nothing and loses nothing — signals already in the store \
             are untouched either way."
        );
    }
    Ok(())
}

/// Read `--occurred-at`, accepting a bare date as well as a full timestamp.
///
/// A bare date is the common case by a wide margin — somebody recording what
/// was said in a conversation last Tuesday knows the day and not the minute —
/// and it becomes midnight UTC rather than being refused. Refusing it would
/// push callers towards inventing a time, which is worse than a coarse one:
/// an invented time cannot be told apart from a real one afterwards.
fn parse_occurred_at(raw: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    use chrono::{DateTime, NaiveDate, TimeZone, Utc};

    if let Ok(t) = DateTime::parse_from_rfc3339(raw) {
        return Ok(t.with_timezone(&Utc));
    }
    if let Ok(d) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        return Utc
            .from_local_datetime(&d.and_hms_opt(0, 0, 0).context(
                "a date with no valid midnight, which should not be reachable for a real date",
            )?)
            .single()
            .context("that date has no single UTC midnight");
    }
    anyhow::bail!(
        "could not read `{raw}` as a time. Give a date like 2026-08-18, or a full \
         timestamp like 2026-08-18T17:03:00Z"
    )
}

/// The arguments of a signal, for the same reason [`TaskDraft`] exists: they
/// arrive together from one clap variant.
struct SignalDraft<'a> {
    project: &'a str,
    summary: &'a str,
    kind: &'a str,
    source: Option<String>,
    contact: Option<String>,
    occurred_at: Option<String>,
    body: Option<String>,
}

/// File a signal, and say what it is and is not on the way out.
///
/// Written through `create_with_document` rather than `create` so that a
/// verbatim supplied with `--body` lands as revision 1 — the same path the
/// MCP surface takes, so a signal filed from a terminal is indistinguishable
/// from one filed from a session apart from its `surface`.
///
/// The human-readable line says the row is not on the board, because the whole
/// point of a signal is that it does not compete with committed work, and
/// somebody who has just filed one will otherwise go looking for it there.
fn run_signal_add(home: &Path, draft: SignalDraft<'_>, force: bool, json: bool) -> Result<()> {
    use specline_core::{Actor, Feedback, FeedbackKind, Provenance, Surface};

    require_inbox()?;

    let mut store =
        writes::open_for_write(home, &writes::daemon_url_for(home), force, "file a signal")?;
    let found = resolve_project(&store, draft.project)?;
    let prov = Provenance::anonymous(Actor::Human).with_surface(Surface::Cli);

    let mut signal = Feedback::new(found.id().clone(), draft.summary);
    signal.kind = FeedbackKind::parse(draft.kind)?;
    signal.source = draft.source;
    signal.contact = draft.contact;
    signal.occurred_at = draft
        .occurred_at
        .as_deref()
        .map(parse_occurred_at)
        .transpose()?;

    let created = store.create_with_document(signal.into(), draft.body, None, &prov)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&created.entity)?);
    } else if created.created {
        println!(
            "{} — filed in the Inbox, untriaged. Not on the board and not in `next`.",
            created.entity.id()
        );
    } else {
        println!(
            "{} — already existed, returned unchanged",
            created.entity.id()
        );
    }
    Ok(())
}

/// Triage a signal, and say what became of it.
///
/// Exactly one of `--feature` and `--set-down` is required, and clap enforces
/// that they are mutually exclusive. Neither being given is caught here rather
/// than by clap, because the message worth printing is about the two outcomes
/// rather than about arguments.
fn run_triage(
    home: &Path,
    id: &str,
    feature: Option<&str>,
    set_down: Option<&str>,
    force: bool,
    json: bool,
) -> Result<()> {
    use specline_core::{Actor, EntityId, EntityType, Provenance, Surface, work};

    require_inbox()?;

    let outcome = match (feature, set_down) {
        (Some(spec), None) => work::TriageOutcome::PickedUp {
            feature: Some(EntityId::parse_as(spec, EntityType::Spec)?),
        },
        (None, Some(reason)) => work::TriageOutcome::SetDown {
            reason: reason.to_owned(),
        },
        _ => anyhow::bail!(
            "say what became of it: --feature <spc_…> to pick it up, or --set-down \
             \"<why>\" to set it down. A signal does not leave the Inbox without an outcome"
        ),
    };

    let mut store = writes::open_for_write(
        home,
        &writes::daemon_url_for(home),
        force,
        "triage a signal",
    )?;
    let signal = EntityId::parse_as(id, EntityType::Feedback)?;
    let prov = Provenance::anonymous(Actor::Human).with_surface(Surface::Cli);

    let triaged = work::triage(&mut store, &signal, &outcome, &prov)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&triaged.signal)?);
    } else {
        match &triaged.linked {
            Some((_, feature)) => println!("{signal} — picked up as {feature}"),
            None => println!(
                "{signal} — set down. The argument is on the signal, and search will find it."
            ),
        }
    }
    Ok(())
}

fn run_task_add(home: &Path, draft: TaskDraft<'_>, force: bool, json: bool) -> Result<()> {
    use specline_core::{Actor, EntityStore, Provenance, Surface, Task, TaskPriority, TaskStatus};

    let mut store =
        writes::open_for_write(home, &writes::daemon_url_for(home), force, "add a task")?;
    let found = resolve_project(&store, draft.project)?;
    let prov = Provenance::anonymous(Actor::Human).with_surface(Surface::Cli);

    let mut task = Task::new(
        found.id().clone(),
        draft.title,
        "A row this test needs in the store.",
    );
    task.status = TaskStatus::parse(draft.status)?;
    task.priority = TaskPriority::parse(draft.priority)?;
    task.body = draft.body;

    let created = store.create(task.into(), &prov)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&created)?);
    } else {
        println!(
            "{} — {}",
            created.entity.id(),
            if created.created {
                "created"
            } else {
                "already existed, returned unchanged"
            }
        );
    }
    Ok(())
}

/// Resolve the store directory, moving a Keel one across on the way.
///
/// An explicit `--home` is taken as given and never relocated: somebody naming
/// a directory has said which one they mean, and quietly moving a different one
/// because it happens to sit beside it would be the opposite of what they
/// asked. The relocation only applies to the default, which is the only path
/// the rename actually changed.
fn resolve_home(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p);
    }
    // Q-2's working assumption: `~/.specline`, local, no remote.
    let base = std::env::var_os("HOME").map(PathBuf::from).context(
        "HOME is not set, so the default store location cannot be resolved. Pass --home",
    )?;
    let home = base.join(specline_core::relocate::HOME_DIR);
    if let Some(moved) = specline_core::relocate::relocate(
        &base.join(specline_core::relocate::LEGACY_HOME_DIR),
        &home,
    )? {
        // Standard error, not standard output, and the distinction is not
        // cosmetic. Every command below this line can be asked for `--json`,
        // and stdout is then a payload somebody parses. A relocation notice
        // printed above it makes the whole document unparseable — once, on the
        // first run after an upgrade, which is exactly the run somebody is
        // watching to see whether the upgrade worked.
        //
        // The session hook is *not* in that set, and the reason is worth
        // knowing rather than assuming: it is dispatched above, before the
        // home is resolved at all, so it never reaches this line. That
        // ordering was put there for a different reason — a hook must not exit
        // non-zero when `HOME` is unset — and it happens to cover this too.
        // Relying on a happy accident for the payload nobody can afford to
        // corrupt is not a plan, so this goes to stderr on its own merits.
        //
        // Still printed rather than logged: a person at a terminal reads
        // stderr, and being told your store moved is the point.
        eprintln!("specline: {}", moved.describe());
    }
    Ok(home)
}

/// Open a store that is already there.
///
/// `home` is the directory; the store is one file inside it. Every caller goes
/// through `store_path` rather than joining a filename, because a surface that
/// picks the wrong name gets a brand-new empty store instead of an error.
///
/// **The existence check is the point.** `Store::open` creates and migrates when
/// the file is absent, which is right for the two commands whose job is to make
/// a store and wrong for everything else. A read that fell back to the store
/// because no daemon answered used to leave an empty `keel.sqlite` behind in a
/// directory nobody asked it to write to, and then report `no project matches
/// specline. Expected: one of: ` — blaming the project name for a store that does
/// not exist. The empty list was the only tell (KEEL-137).
pub(crate) fn open(home: &Path) -> Result<Store> {
    let path = specline_core::store_path(home);
    if !path.exists() {
        bail!(
            "there is no Specline store at {}.\n\n\
             Nothing was created: a command that reads does not get to make one, or a mistyped \
             --home silently becomes an empty store that answers every question with \"nothing \
             here\".\n\n\
             `specline bootstrap` makes a store for this project, `specline fixture` fills one with demo \
             data, and --home points at a different one.",
            path.display()
        );
    }
    Store::open(&path).with_context(|| format!("open the store at {}", path.display()))
}

/// Open the store under a home directory, making one if there is none.
///
/// The other half of [`open`], for the commands that are *asked* to produce a
/// store. Separate rather than a boolean argument, so that a call site creating
/// a store says so where it is read.
fn create_or_open(home: &Path) -> Result<Store> {
    let path = specline_core::store_path(home);
    Store::open(&path).with_context(|| format!("open the store at {}", path.display()))
}

/// Ask the daemon for a read, returning `None` if it is not answering.
///
/// The daemon is the single writer, so a read-shaped command has two choices:
/// go through the daemon, or read the file underneath it. The second is what
/// `fsck` used to do only when the daemon was stopped, and an integrity check
/// you must stop the thing you want to check in order to run is not much of a
/// check (TQ-15, KEEL-57). SQLite in WAL mode would now permit reading behind
/// the daemon's back, but the daemon's answer is the one that has seen every
/// write, so it stays the front door.
///
/// Thirty seconds rather than `writes::read`'s caller-chosen default, because
/// these are whole-store reports and a busy daemon should be waited for rather
/// than gone around.
fn read_via_daemon(base: &str, path: &str) -> Result<Option<serde_json::Value>> {
    writes::read(base, path, std::time::Duration::from_secs(30))
}

fn run_fsck(home: &Path, daemon: &str, json: bool) -> Result<()> {
    let report: fsck::FsckReport = match read_via_daemon(daemon, "/api/fsck")? {
        Some(v) => serde_json::from_value(v).context("parse the daemon's fsck report")?,
        None => fsck::check(&open(home)?)?,
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if report.findings.is_empty() {
        println!("clean — {} checks, nothing found", report.checks_run);
    } else {
        println!(
            "{} checks run, {} finding(s):\n",
            report.checks_run,
            report.findings.len()
        );
        for f in &report.findings {
            let marker = match f.severity {
                fsck::Severity::Error => "ERROR",
                fsck::Severity::Warning => "warn ",
            };
            println!("{marker}  {}", f.check);
            println!("        {}", f.detail);
            println!("        → {}\n", f.remedy);
        }
    }

    if !report.is_clean() {
        // Non-zero so this can gate a backup or a deploy. Warnings do not
        // fail: an orphaned task under an archived project is expected.
        bail!(
            "{} error-level finding(s); the store is not consistent",
            report.errors().count()
        );
    }
    Ok(())
}

/// Backfill the vectors that were never written.
///
/// The one command whose whole job is the model, so it is the one that has to
/// say something honest when the build has none. Hidden or silently successful
/// would both be worse: a person running this expects vectors afterwards, and
/// the way to find out they did not get any is otherwise a search that quietly
/// stays keyword-only (KEEL-220).
#[cfg(not(feature = "embeddings"))]
fn run_reembed(_home: &Path, _missing: bool, _force: bool, _json: bool) -> Result<()> {
    bail!(
        "this build of Specline has no embedding model in it, so there is nothing to re-embed with.\n\n\
         Two of the three release targets cannot link the ONNX runtime the model needs — Intel \
         macOS has no prebuilt one, and the Linux build wants a newer glibc than the binaries \
         are built against — so those builds ship without it. Keyword search covers every \
         artifact either way, prose included, and it is what this store has been using.\n\n\
         `specline doctor` reports which build you are running. The arm64 macOS release is the one \
         that carries a model."
    )
}

/// Backfill the vectors that were never written.
#[cfg(feature = "embeddings")]
fn run_reembed(home: &Path, missing: bool, force: bool, json: bool) -> Result<()> {
    if !missing {
        bail!(
            "only `--missing` is supported, and since B-59 it is also the answer to a changed \
             model: `missing` means a revision with no passages from the model now configured, \
             so swapping the model makes every document missing and this pass rebuilds them"
        );
    }

    let mut store = writes::open_for_write(home, &writes::daemon_url_for(home), force, "re-embed")?;

    // The model loads before the count, which looks like the wrong order and is
    // not. "Missing" means "has no passages from *this* model" (B-59), and
    // there is no way to ask that until the model has said what it is called.
    // After the first run this is a local load of an already-cached file.
    let models = home.join("models");
    std::fs::create_dir_all(&models)
        .with_context(|| format!("create the model cache at {}", models.display()))?;
    if !json {
        println!(
            "loading the model from {} — the first run downloads it, which needs network access",
            models.display()
        );
    }
    let embedder = specline_embed::FastEmbedder::new(&models)
        .context("load the embedding model. The first run downloads it")?;

    let (current, absent) =
        store.documents_missing_embeddings(Some(specline_core::Embedder::model_name(&embedder)))?;
    if absent == 0 {
        if json {
            println!(
                "{}",
                serde_json::json!({"missing": 0, "embedded": 0, "failed": 0})
            );
        } else {
            println!("nothing to do — all {current} current document(s) have a vector");
        }
        return Ok(());
    }
    if !json {
        println!("embedding {absent} of {current} current document(s)");
    }

    let report = store.reembed_missing(&embedder, None, |done, total| {
        if !json {
            println!("  {done}/{total}");
        }
    })?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "missing": report.missing,
                "embedded": report.embedded,
                "failed": report.failed,
            })
        );
    } else if report.failed > 0 {
        println!(
            "{} embedded, {} refused by the model and left keyword-searchable",
            report.embedded, report.failed
        );
    } else {
        println!("{} document(s) embedded", report.embedded);
    }
    Ok(())
}

fn run_backup(home: &Path, dest: Option<PathBuf>, json: bool) -> Result<()> {
    let store = open(home)?;
    let dest = dest.unwrap_or_else(|| backup::default_backup_dir(home, chrono::Utc::now()));

    let manifest = backup::backup(&store, &dest)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&manifest)?);
    } else {
        println!(
            "backed up {} rows to {}",
            manifest.total_rows(),
            dest.display()
        );
        println!(
            "  store    → {}/{}",
            dest.display(),
            specline_core::store::STORE_FILE
        );
        println!("  manifest → {}/manifest.json", dest.display());
    }
    Ok(())
}

/// Apply the schema migrations the store is missing.
///
/// The whole command is the ceremony. There is nothing here that `Store::open`
/// could not have done silently, and doing it silently is the bug: an upgrade
/// leaves a running daemon holding beliefs about the tables, and the next CLI
/// call to open the store would rewrite them underneath it.
fn run_migrate(home: &Path, daemon: &str, dry_run: bool, force: bool, json: bool) -> Result<()> {
    let path = specline_core::store_path(home);
    if !path.exists() {
        bail!(
            "there is no store at {} to migrate.\n\n\
             A store is created already migrated, so this command has nothing to do until one \
             exists. `specline bootstrap` or `specline fixture` makes one.",
            path.display()
        );
    }

    // Read the ledger without opening a Store, because a Store is exactly what
    // cannot be opened here: `Store::open` refuses a store with migrations
    // pending, and its refusal names this command. Asking the ledger directly
    // is the way out of that loop, and it is a read, so it is safe alongside a
    // live daemon.
    let pending = specline_core::pending_migrations_at(&path)?;

    if pending.is_empty() {
        let version = specline_core::shipped_schema_version();
        if json {
            println!(
                "{}",
                serde_json::json!({"schema": version, "pending": [], "applied": []})
            );
        } else {
            println!("nothing to do — the store is at schema {version}");
        }
        return Ok(());
    }

    if dry_run {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "pending": pending.iter().map(|(id, name)| serde_json::json!({"id": id, "name": name})).collect::<Vec<_>>(),
                    "applied": [],
                })
            );
        } else {
            println!("{} migration(s) pending:", pending.len());
            for (id, name) in &pending {
                println!("  {id}  {name}");
            }
            println!("\nrun `specline migrate` without --dry-run to apply them");
        }
        return Ok(());
    }

    // The same probe that guards every other write from a non-daemon process,
    // for the same reason and more sharply: a schema change under a live reader
    // is worse than a poorly-attributed row.
    writes::refuse_if_daemon_is_running(daemon, home, force, "migrate the store")?;
    // Exclusive unless forced, for the same reason as every other write and one
    // more: this is the operation that changes the shape of the tables, so a
    // second process inside it is the worst version of the problem. `--force`
    // opts out, because a wedged daemon holding the lock is exactly when
    // somebody needs to migrate by hand.
    let migrated = if force {
        specline_core::Store::open_and_migrate(&path)
    } else {
        specline_core::Store::open_and_migrate_exclusive(&path)
    };
    migrated.with_context(|| format!("migrate the store at {}", path.display()))?;
    let applied = pending;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "schema": specline_core::shipped_schema_version(),
                "pending": [],
                "applied": applied.iter().map(|(id, name)| serde_json::json!({"id": id, "name": name})).collect::<Vec<_>>(),
            })
        );
    } else {
        println!("applied {} migration(s):", applied.len());
        for (id, name) in &applied {
            println!("  {id}  {name}");
        }
        println!(
            "the store is at schema {}",
            specline_core::shipped_schema_version()
        );
    }
    Ok(())
}

fn run_restore(source: &PathBuf, target: &PathBuf, json: bool) -> Result<()> {
    // `target` is a home directory, the same thing `--home` names, and the
    // store file goes inside it. Naming the file here instead would make a
    // restore the one command whose path argument means something else.
    let manifest = backup::restore(source, specline_core::store_path(target))?;

    // Re-open and verify rather than trusting the restore. "Assert equality,
    // don't eyeball it" is the exit criterion, and a restore that silently
    // dropped a table is exactly the failure a backup exists to prevent.
    //
    // One of the three places allowed to migrate: a snapshot may have been
    // written by an older binary, migrations are forward-only, and a restore
    // that stopped to tell you to run another command would be a poor thing to
    // meet in the middle of a recovery. The target is a directory the restore
    // itself just made, so nothing else can be holding it.
    let path = specline_core::store_path(target);
    let restored = specline_core::Store::open_and_migrate(&path)
        .with_context(|| format!("open the restored store at {}", path.display()))?;
    let problems: Vec<String> = match backup::verify_restore(&restored, &manifest) {
        Ok(()) => Vec::new(),
        Err(e) => vec![e.to_string()],
    };

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "manifest": manifest,
                "problems": problems,
            }))?
        );
    } else if problems.is_empty() {
        println!(
            "restored {} rows to {} and verified every table",
            manifest.total_rows(),
            target.display()
        );
    } else {
        println!("restored with {} discrepancy(ies):", problems.len());
        for p in &problems {
            println!("  {p}");
        }
    }

    if !problems.is_empty() {
        bail!("the restored store does not match the backup manifest");
    }

    // A restored store must be a git repository, or the restore has quietly
    // cost you a recovery tier.
    //
    // SPEC §11 names three: the store's own git history (full fidelity,
    // including every revision), the Parquet backup, and the markdown mirror.
    // `restore` rebuilds from tier 2 into a fresh directory — and until now
    // handed back a store with no `.git`, so tier 1 was silently gone. Found
    // the hard way, one command before deleting the only copy that still had
    // it.
    match init_store_git(target) {
        Ok(true) => println!("  initialised {} as a git repository", target.display()),
        Ok(false) => {}
        // Never fail the restore for this. The rows are back and verified;
        // a missing git binary is a smaller problem than pretending the
        // restore did not happen.
        Err(e) => eprintln!(
            "  warning: could not initialise {} as a git repository: {e}\n  \
             The data is restored and verified, but SPEC §11's recovery tier 1 \
             is missing until you run: git -C {} init",
            target.display(),
            target.display()
        ),
    }
    Ok(())
}

/// Make a restored store its own git repository, as `plugin/install.sh` does
/// for a fresh one.
///
/// Returns whether it created anything. Deliberately no remote — that is Q-2
/// and it is KB's call.
fn init_store_git(target: &std::path::Path) -> Result<bool> {
    if target.join(".git").exists() {
        return Ok(false);
    }
    if std::process::Command::new("git")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_err()
    {
        bail!("git is not on PATH");
    }

    let git = |args: &[&str]| -> Result<()> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(target)
            .args(args)
            .output()
            .with_context(|| format!("run git {}", args.join(" ")))?;
        if !out.status.success() {
            bail!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    };

    git(&["init", "-q"])?;
    std::fs::write(
        target.join(".gitignore"),
        "# Model weights are large and re-downloadable.\nmodels/\n",
    )
    .with_context(|| format!("write {}/.gitignore", target.display()))?;
    git(&["add", "-A"])?;
    // An empty repository restores nothing, so the restored state is the first
    // commit. Identity is set per-invocation rather than relying on a global
    // config that may not exist.
    git(&[
        "-c",
        "user.name=specline",
        "-c",
        "user.email=specline@localhost",
        "commit",
        "-q",
        "-m",
        "chore: store restored from a Parquet backup",
    ])?;
    Ok(true)
}

fn run_fixture(home: &Path, force: bool, json: bool) -> Result<()> {
    let mut store = writes::open_for_write(
        home,
        &writes::daemon_url_for(home),
        force,
        "load the fixture corpus",
    )?;
    let summary = fixture::load(&mut store)?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "entities": summary.entities,
                "links": summary.links,
                "revisions": summary.revisions,
                "total_entities": summary.total_entities(),
                "total_links": summary.total_links(),
            }))?
        );
    } else {
        println!(
            "loaded {} entities, {} links, {} document revisions",
            summary.total_entities(),
            summary.total_links(),
            summary.revisions
        );
        for (ty, n) in &summary.entities {
            println!("  {n:>4}  {ty}");
        }
    }
    Ok(())
}

/// What a release of this binary promises, as JSON.
///
/// Three numbers and what each is for:
///
/// - `version` moves for any release, including one that changes nothing a
///   caller can see. On its own it says almost nothing about safety.
/// - `schema_version` moves only when the shape of the stored data moves, and
///   is the one the updater actually splits on: equal means the update can be
///   applied without asking, different means somebody's data is about to be
///   rewritten and a person decides.
/// - `min_plugin_version` is the daemon's half of the handshake with the
///   Claude Code plugin, which updates over git on its own schedule.
///
/// Checksums are deliberately *not* here. This is what the binary knows about
/// itself; what the release contains is what the release job knows, and the two
/// are merged there. A binary that tried to state its own artifact hash would
/// be describing a file it has never seen.
///
/// Always JSON, ignoring `--json`. Nothing reads this but a script.
fn run_release_manifest() -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "schema_version": specline_core::shipped_schema_version(),
            "min_plugin_version": specline_core::MIN_PLUGIN_VERSION,
            "protocol": specline_mcp::PROTOCOL_VERSION,
        }))?
    );
    Ok(())
}

/// Open the daemon's interface in a browser.
///
/// The whole command is "find the daemon, check it is alive, hand the address
/// to the platform's opener" — and each of those three is a thing that used to
/// have to be done by hand.
///
/// It refuses when nothing answers, rather than opening a browser at a dead
/// port. A tab reading "cannot connect" is indistinguishable from a broken
/// interface, and this project has spent enough time on failures that look like
/// something else.
fn run_ui(home: &Path, daemon: Option<&str>, print: bool, json: bool) -> Result<()> {
    // An explicit `--daemon` wins. Otherwise `daemon_url_for` reads the address
    // the running daemon recorded and probes it before believing it, which is
    // what finds a daemon on a non-default port without being told.
    let base = match daemon {
        Some(explicit) => explicit.trim_end_matches('/').to_owned(),
        None => writes::daemon_url_for(home)
            .trim_end_matches('/')
            .to_owned(),
    };

    if writes::probe(&base) != writes::Daemon::Listening {
        anyhow::bail!(
            "nothing is listening at {base}, so there is no interface to open.\n\n\
             Start the daemon and try again:\n    specline-daemon\n\n\
             If it is running on another address, pass it: specline ui --daemon http://127.0.0.1:<port>"
        );
    }

    if json {
        println!("{}", serde_json::json!({ "url": base }));
        return Ok(());
    }

    if print {
        println!("{base}");
        return Ok(());
    }

    // `open` on macOS, `xdg-open` on everything else. Not a crate: this is one
    // process spawn per platform, and the alternatives pull in a dependency to
    // do exactly this.
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };

    match std::process::Command::new(opener).arg(&base).status() {
        Ok(status) if status.success() => {
            println!("Opened {base}");
            Ok(())
        }
        // A failed opener is not a failed command. The address is the useful
        // part and the user can paste it; exiting non-zero here would fail a
        // script over a missing `xdg-open` on a headless box.
        Ok(status) => {
            println!("Could not open a browser ({opener} exited {status}). The interface is at:");
            println!("    {base}");
            Ok(())
        }
        Err(e) => {
            println!("Could not run {opener} ({e}). The interface is at:");
            println!("    {base}");
            Ok(())
        }
    }
}

fn run_status(home: &Path, daemon: &str, json: bool) -> Result<()> {
    use specline_core::{EntityQuery, EntityStore, EntityType};

    let counts = match read_via_daemon(daemon, "/api/status")? {
        Some(v) => v,
        None => {
            let store = open(home)?;
            serde_json::json!({
                "projects": store
                    .list(&EntityQuery::default().of_type(EntityType::Project))?
                    .total,
                "open_tasks": store
                    .list(
                        &EntityQuery::default()
                            .of_type(EntityType::Task)
                            .with_status(["todo", "in_progress", "review"]),
                    )?
                    .total,
                "open_questions": store
                    .list(
                        &EntityQuery::default()
                            .of_type(EntityType::Question)
                            .with_status(["open"]),
                    )?
                    .total,
            })
        }
    };
    let n = |k: &str| {
        counts
            .get(k)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    let (projects, open_tasks, open_questions) =
        (n("projects"), n("open_tasks"), n("open_questions"));

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "home": home.display().to_string(),
                "projects": projects,
                "open_tasks": open_tasks,
                "open_questions": open_questions,
            }))?
        );
    } else {
        println!("{}", home.display());
        println!("  {projects} project(s)");
        println!("  {open_tasks} open task(s)");
        println!("  {open_questions} open question(s)");
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod cli_definition_tests {
    /// Clap's own audit of the argument definitions.
    ///
    /// It catches what review does not: two arguments claiming the same short
    /// flag, a `default_value` that fails its own parser, a positional after a
    /// variadic one. Every one of those is a panic at *runtime*, on the first
    /// invocation of the affected subcommand — so without this the failure
    /// arrives in front of whoever typed the command rather than in CI.
    #[test]
    fn the_argument_definitions_are_internally_consistent() {
        use clap::CommandFactory;
        super::Cli::command().debug_assert();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod restore_git_tests {
    use super::init_store_git;

    fn have_git() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[test]
    fn a_restored_store_becomes_a_git_repository_with_its_state_committed() {
        // SPEC §11 tier 1. `restore` rebuilds from tier 2 into a fresh
        // directory, and used to hand back a store with no `.git` — so a
        // restore silently cost you the recovery tier with the most fidelity.
        if !have_git() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("specline.sqlite"), b"not really a database").unwrap();

        assert!(init_store_git(dir.path()).unwrap(), "it created the repo");
        assert!(dir.path().join(".git").exists());
        assert!(dir.path().join(".gitignore").exists());

        // An empty repository restores nothing, so the state must be committed.
        let log = std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["log", "--oneline"])
            .output()
            .unwrap();
        assert!(log.status.success(), "the repo has a HEAD");
        assert!(
            String::from_utf8_lossy(&log.stdout).contains("restored"),
            "the restored state is the first commit"
        );

        // Model weights are large and re-downloadable; committing them would
        // make the recovery tier unusable.
        assert!(
            std::fs::read_to_string(dir.path().join(".gitignore"))
                .unwrap()
                .contains("models/")
        );
    }

    #[test]
    fn an_existing_repository_is_left_alone() {
        // Restoring into a directory that is already a repo must not reinit it
        // and lose its history — which is exactly the loss this whole fix is
        // about, just in the other direction.
        if !have_git() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        assert!(!init_store_git(dir.path()).unwrap(), "it did nothing");
        assert!(
            !dir.path().join(".gitignore").exists(),
            "and wrote nothing over the existing repo"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod render_status_tests {
    //! The one genuine data-loss path in the CLI.
    //!
    //! `render-status --out` used to write unconditionally. Point it at a
    //! near-empty project by accident and it replaced a real tracker with that
    //! project's stub — no comparison, no backup, no way to tell afterwards.

    /// A port nothing listens on, so these exercise the direct-store path.
    ///
    /// Pinned rather than left to the default: the default is the real daemon,
    /// and a test that quietly passes only when the developer's daemon happens
    /// to be stopped is a test that fails in CI for reasons nobody can see.
    const NO_DAEMON: &str = "http://127.0.0.1:9";

    use super::*;
    use specline_core::{Actor, EntityStore, Project, Provenance};

    fn store_with(slugs: &[(&str, &str)]) -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("specline.sqlite")).unwrap();
        for (slug, name) in slugs {
            store
                .create(
                    Project::new(*slug, *name).into(),
                    &Provenance::anonymous(Actor::Human),
                )
                .unwrap();
        }
        (dir, store)
    }

    #[test]
    fn an_ambiguous_reference_is_refused_rather_than_resolved_to_the_first() {
        // One project answers to `specline` as its slug, another as its name.
        // Two projects sharing a *name* cannot both exist — near-duplicate
        // detection catches that on create — so this is the collision that is
        // actually reachable, and picking one of the two silently is how a
        // render lands in the wrong project's file.
        let (_d, store) = store_with(&[("specline", "Specline Project"), ("other", "specline")]);
        let err = resolve_project(&store, "specline").unwrap_err();
        let message = err.to_string();
        assert!(message.contains("matches 2 projects"), "{message}");
        assert!(message.contains("specline"), "it must name them: {message}");
    }

    #[test]
    fn an_exact_slug_still_resolves_when_a_similar_one_exists() {
        let (_d, store) = store_with(&[("specline", "Specline"), ("specline-web", "Specline Web")]);
        let found = resolve_project(&store, "specline").unwrap();
        assert_eq!(found.label(), "Specline");
    }

    #[test]
    fn a_reference_that_names_nothing_says_so() {
        let (_d, store) = store_with(&[("specline", "Specline")]);
        assert!(resolve_project(&store, "harbour").is_err());
    }

    #[test]
    fn a_dramatically_smaller_render_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("STATUS.md");
        std::fs::write(&path, "x".repeat(10_000)).unwrap();

        let (home, _store) = store_with(&[("empty", "Empty")]);
        let err = run_render_status(home.path(), NO_DAEMON, "empty", Some(path.clone()), false)
            .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("refusing to write"), "{message}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap().len(),
            10_000,
            "and the file it refused to write is untouched"
        );
    }

    #[test]
    fn force_writes_the_smaller_file_anyway() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("STATUS.md");
        std::fs::write(&path, "x".repeat(10_000)).unwrap();

        let (home, _store) = store_with(&[("empty", "Empty")]);
        run_render_status(home.path(), NO_DAEMON, "empty", Some(path.clone()), true).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().len() < 10_000);
    }

    #[test]
    fn an_unchanged_render_does_not_rewrite_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("STATUS.md");
        let (home, _store) = store_with(&[("specline", "Specline")]);
        let home = home.path().to_path_buf();

        run_render_status(&home, NO_DAEMON, "specline", Some(path.clone()), false).unwrap();
        let first = std::fs::metadata(&path).unwrap().modified().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(20));
        run_render_status(&home, NO_DAEMON, "specline", Some(path.clone()), false).unwrap();
        let second = std::fs::metadata(&path).unwrap().modified().unwrap();

        assert_eq!(
            first, second,
            "regenerating an unchanged tracker must not dirty the tree"
        );
    }
}
