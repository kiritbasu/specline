//! Replacing the installed binaries with a newer release.
//!
//! # What this is allowed to decide on its own
//!
//! The split is on **schema version, not on how the update arrives**. Two
//! releases that agree about the shape of the stored data are interchangeable
//! as far as anybody's store is concerned, so applying one is a file move and
//! needs no permission. A release that moves the schema is going to rewrite
//! somebody's data on next open, and that is a person's decision every time —
//! not least because a migrated store cannot be un-migrated, so the rollback
//! below would not undo it.
//!
//! # What "verified" means here, exactly
//!
//! The SHA-256 in the release manifest, and — from the first release built
//! after 2026-08-15 — nothing more yet. The manifest is the trust root, so a
//! missing one is a hard failure rather than a reason to skip the check.
//!
//! That guarantee is real but narrower than provenance: it catches a corrupt,
//! truncated or substituted artifact, and it does not independently establish
//! that GitHub built those bytes from this commit.
//!
//! Provenance is now *available* — the repository went public on 2026-08-15, so
//! `release.yml`'s attestation step stops being skipped and releases cut from
//! here carry one. Checking it is not built yet and is the open half of B-73;
//! until it is, a release carrying an attestation and one not carrying it are
//! treated identically, which is the weakness worth naming rather than leaving
//! for somebody to infer from the absence of code.
//!
//! # How it fetches
//!
//! A plain unauthenticated GET of `releases/latest/download/<name>`. That is
//! what going public bought: no token, no `gh`, no asset-id lookup, and an
//! install path that works for somebody who is not the author. While the
//! repository was private that URL returned 404 with a valid token as readily
//! as without one (KEEL-221), and the only route was `api.github.com` with an
//! asset id — which is what B-73 first chose and what it no longer has to.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The two executables a release installs. Both are replaced together.
///
/// Updating one and not the other is the drift this task exists to stop: they
/// share a store whose shape they each believe something about, and a mismatch
/// between them is not a state anything else in the system checks for.
const BINARIES: [&str; 2] = ["specline", "specline-daemon"];

/// What a release says about itself, as published beside its artifacts.
///
/// Deserialised rather than constructed, so the fields are the contract with
/// `specline release-manifest` and the release job that merges checksums into it.
#[derive(Debug, Deserialize)]
pub struct ReleaseManifest {
    /// The release's own version, as in `0.1.2`.
    pub version: String,
    /// The shape of the store this release believes in. The field the decision
    /// actually turns on.
    pub schema_version: i32,
    /// SHA-256 by artifact filename. Absent on a manifest published before the
    /// release job learned to merge them in, which is a refusal rather than a
    /// warning — see [`verify`].
    #[serde(default)]
    pub artifacts: BTreeMap<String, String>,
}

/// What should happen about a candidate release.
///
/// A value rather than a branch taken inline, so the decision can be tested
/// without a network, a GitHub account or a release existing.
#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    /// The installed version is the published one.
    UpToDate,
    /// The installed version is *newer* than the published one.
    ///
    /// Reachable in ordinary use: `releases/latest` resolves to the latest
    /// non-prerelease, so anybody running an `-rc` is ahead of it.
    /// Treated as its own outcome because applying it would be a silent
    /// downgrade, which looks exactly like a successful update.
    Ahead {
        /// The version that is published.
        published: String,
    },
    /// Safe to apply without asking: same schema, higher version.
    Apply {
        /// The version to install.
        version: String,
        /// The archive to fetch.
        artifact: String,
    },
    /// This exact version is already staged and waiting for a restart.
    ///
    /// [`plan`] cannot reach this — it compares the *running* binary against
    /// the manifest, and staging does not change the running binary, so once
    /// something is staged every later check planned `Apply` for it again and
    /// re-downloaded, re-verified, re-unpacked and re-staged bytes already on
    /// disk. Harmless-looking, and it meant a daemon left running overnight
    /// with an update pending fetched the same 11 MB archive every interval
    /// until somebody restarted it (KEEL-317).
    ///
    /// So this is decided by [`check_and_stage`], which is the only layer that
    /// can see the install directory. Its own outcome rather than `UpToDate`
    /// because the two are opposite advice: one means there is nothing to take
    /// and the other means there is something to take and it is already here.
    AlreadyStaged {
        /// The version sitting staged.
        version: String,
    },
    /// The schema moves, so a person decides.
    NeedsAPerson {
        /// The version that is published.
        version: String,
        /// The schema the installed binaries believe in.
        from: i32,
        /// The schema the candidate believes in.
        to: i32,
    },
}

/// A version as three numbers plus whether it is a final release.
///
/// Deliberately not a semver dependency. The only comparison this makes is
/// "strictly newer than what is installed", over versions this project itself
/// produces, and the tag filter in `release.yml` already constrains those to
/// `MAJOR.MINOR.PATCH` with an optional suffix.
///
/// A prerelease sorts *below* the release with the same numbers, which is the
/// one rule that stops `0.2.0` being considered older than `0.2.0-rc.1`.
fn parse_version(raw: &str) -> Option<(u64, u64, u64, bool)> {
    let raw = raw.trim().trim_start_matches('v');
    let (core, is_release) = match raw.split_once('-') {
        Some((core, _suffix)) => (core, false),
        None => (raw, true),
    };
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch, is_release))
}

/// Decide what to do, given what is installed and what is published.
///
/// Pure, and the whole of the policy. Every refusal in here is a refusal to do
/// something that would look like it worked.
pub fn plan(
    installed_version: &str,
    installed_schema: i32,
    manifest: &ReleaseManifest,
    target: &str,
) -> Result<Plan> {
    let installed = parse_version(installed_version).with_context(|| {
        format!("cannot read the installed version {installed_version:?} as MAJOR.MINOR.PATCH")
    })?;
    let published = parse_version(&manifest.version).with_context(|| {
        format!(
            "the release manifest's version {:?} is not MAJOR.MINOR.PATCH, so there is no way to \
             tell whether it is newer than {installed_version}",
            manifest.version
        )
    })?;

    if published == installed {
        return Ok(Plan::UpToDate);
    }
    if published < installed {
        return Ok(Plan::Ahead {
            published: manifest.version.clone(),
        });
    }
    if manifest.schema_version != installed_schema {
        return Ok(Plan::NeedsAPerson {
            version: manifest.version.clone(),
            from: installed_schema,
            to: manifest.schema_version,
        });
    }
    Ok(Plan::Apply {
        version: manifest.version.clone(),
        artifact: archive_name(target),
    })
}

/// The directory `dist` puts inside an archive, and the stem of the archive
/// itself.
///
/// `dist` names both after the package that owns the binaries, so this tracks
/// `crates/specline/Cargo.toml`'s `name` and nothing else. It is a constant
/// because the two places that need it are 450 lines apart and were the two
/// sites a rename of the package silently missed: neither is a literal
/// filename, so grepping for the old archive name found nothing while the
/// updater went on asking GitHub for an artifact that no longer exists.
pub const ARCHIVE_STEM: &str = "specline";

/// The archive filename for a target, as `dist` names it.
fn archive_name(target: &str) -> String {
    format!("{ARCHIVE_STEM}-{target}.tar.xz")
}

/// The repository releases come from.
///
/// Same name and same default as `plugin/scripts/setup.sh`, so a scratch
/// install and its updates cannot end up pointed at different repositories.
fn repo() -> String {
    std::env::var("SPECLINE_REPO").unwrap_or_else(|_| "kiritbasu/specline".to_owned())
}

/// Where to read what a version contains.
///
/// The release page for the tag, which is the changelog for that version: the
/// release job builds it with `--generate-notes`, so it is the one place that
/// says what changed and it is public.
///
/// Minted here rather than composed by whoever is displaying it, for the same
/// reason the daemon mints artifact links: the repository comes from
/// `SPECLINE_REPO`, and a caller building the URL from a template would be right
/// only for the default. It is a pure string function so the interface can show
/// the link without another request.
///
/// The `v` prefix is the tag convention `release.yml` filters on, so a version
/// of `0.1.3` is the tag `v0.1.3`. Passing a version that was never released
/// gives a URL that 404s — this cannot check, and a link to a missing release
/// is a better failure than no link.
pub fn release_notes_url(version: &str) -> String {
    format!(
        "https://github.com/{}/releases/tag/v{}",
        repo(),
        version.trim_start_matches('v')
    )
}

/// Fetch one asset from the latest release.
///
/// A plain unauthenticated GET, which is what the repository going public buys:
/// `releases/latest/download/<name>` is served to anybody, so the updater needs
/// no token, no `gh`, and no asset-id lookup. While the repository was private
/// this same URL returned 404 with a valid token as readily as without one
/// (KEEL-221), and the only route was the API — which is why B-73 originally
/// shelled out and why that is no longer the trade.
///
/// `latest` deliberately excludes prereleases, which is what makes
/// [`Plan::Ahead`] reachable rather than theoretical.
fn download(dir: &Path, name: &str) -> Result<PathBuf> {
    let repo = repo();
    let url = format!("https://github.com/{repo}/releases/latest/download/{name}");

    let response = match ureq::get(&url).call() {
        Ok(response) => response,
        Err(ureq::Error::Status(404, _)) => bail!(
            "the latest release of {repo} has no asset called {name}.\n\nA release published \
             before the job learned to attach it looks exactly like this. Check what it \
             carries:\n    https://github.com/{repo}/releases/latest"
        ),
        Err(ureq::Error::Status(code, _)) => bail!(
            "fetching {url} returned HTTP {code}, so there is nothing to install. Nothing has \
             been changed."
        ),
        Err(e) => bail!("could not reach {url}: {e}\n\nNothing has been changed."),
    };

    let path = dir.join(name);
    let mut file = std::fs::File::create(&path)
        .with_context(|| format!("creating {} for the download", path.display()))?;
    std::io::copy(&mut response.into_reader(), &mut file)
        .with_context(|| format!("writing the download to {}", path.display()))?;
    Ok(path)
}

/// Read the latest release's manifest.
pub fn fetch_manifest(dir: &Path) -> Result<ReleaseManifest> {
    let path = download(dir, "specline-release.json")?;
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("reading the release manifest at {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| {
        format!(
            "the release manifest at {} is not the JSON `specline release-manifest` produces",
            path.display()
        )
    })
}

/// Check a downloaded file against the hash the manifest states for it.
///
/// The absence of an entry is a failure, not a skip. A manifest that does not
/// mention the artifact cannot vouch for it, and proceeding anyway is the
/// unverified fallback B-73 rules out.
pub fn verify(path: &Path, artifact: &str, manifest: &ReleaseManifest) -> Result<()> {
    use sha2::{Digest, Sha256};

    let Some(expected) = manifest.artifacts.get(artifact) else {
        bail!(
            "the release manifest states no checksum for {artifact}, so there is nothing to \
             verify it against. Refusing to install it. This is what a release published before \
             the manifest carried checksums looks like."
        );
    };

    let bytes = std::fs::read(path)
        .with_context(|| format!("reading {} to check its hash", path.display()))?;
    // Not `format!("{:x}", …)`: sha2 0.11 returns a `hybrid-array` `Array`,
    // which does not implement `LowerHex`. The encoding has to stay lowercase
    // — the comparison below is a string comparison against a manifest written
    // by `sha256sum`.
    let actual = specline_core::hex::encode(&Sha256::digest(&bytes));

    if &actual != expected {
        bail!(
            "{artifact} does not match the checksum in the release manifest.\n  expected \
             {expected}\n  got      {actual}\n\nNothing has been installed. A truncated download \
             is the usual cause; a repeat is worth investigating rather than retrying."
        );
    }
    Ok(())
}

/// Where the running executables live.
pub fn install_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("finding the running executable's own path")?;
    let dir = exe
        .parent()
        .context("the running executable has no parent directory")?;
    Ok(dir.to_path_buf())
}

/// Put the new binaries in place, keeping the ones they replace.
///
/// Renaming over a running executable is safe on Unix — the running process
/// holds its inode and carries on with the bytes it started from — which is why
/// this can replace `specline` while `specline` is the thing doing the replacing.
///
/// The previous copies are kept as `<name>.previous` for [`rollback`]. One
/// generation, not a history: the case it exists for is "the release I just
/// took is bad", and keeping more would mostly ensure that the copy somebody
/// eventually reaches for is one nobody has run.
pub fn install_from(unpacked: &Path, into: &Path) -> Result<()> {
    for name in BINARIES {
        let fresh = unpacked.join(name);
        if !fresh.is_file() {
            bail!(
                "the release archive does not contain {name}. Refusing to install a partial \
                 release — {} and {} have to move together.",
                BINARIES[0],
                BINARIES[1]
            );
        }
    }

    for name in BINARIES {
        let fresh = unpacked.join(name);
        let live = into.join(name);
        let kept = into.join(format!("{name}.previous"));

        if live.exists() {
            std::fs::rename(&live, &kept).with_context(|| {
                format!(
                    "keeping the current {name} as {} before replacing it",
                    kept.display()
                )
            })?;
        }
        std::fs::copy(&fresh, &live)
            .with_context(|| format!("installing {name} to {}", live.display()))?;
        make_executable(&live)?;
    }
    Ok(())
}

/// Put the new binaries beside the current ones without taking effect.
///
/// The daemon's route, and the reason it differs from [`install_from`]: a
/// daemon replacing its own executable and then carrying on is running code
/// from a file that no longer exists at that path, so anything it re-reads at
/// runtime — and any crash handler that re-execs — sees a version it never
/// started. Staging keeps the swap to one moment, at a startup, where there is
/// no half-updated process to reason about.
///
/// Written to a scratch name and renamed into place, so a `.staged` file is
/// always complete. A partial one is what a `SIGKILL` mid-copy would otherwise
/// leave, and [`apply_staged`] cannot tell a truncated binary from a whole one.
pub fn stage(unpacked: &Path, into: &Path, version: &str) -> Result<()> {
    for name in BINARIES {
        if !unpacked.join(name).is_file() {
            bail!(
                "the release archive does not contain {name}. Refusing to stage a partial \
                 release — {} and {} have to move together.",
                BINARIES[0],
                BINARIES[1]
            );
        }
    }

    for name in BINARIES {
        let staged = into.join(format!("{name}.staged"));
        let partial = into.join(format!("{name}.staged.partial"));
        std::fs::copy(unpacked.join(name), &partial)
            .with_context(|| format!("staging {name} to {}", partial.display()))?;
        make_executable(&partial)?;
        std::fs::rename(&partial, &staged)
            .with_context(|| format!("putting {} in place", staged.display()))?;
    }

    // Last, and only once both binaries are whole. `apply_staged` keys off this
    // file, so writing it earlier would advertise an update that is still being
    // copied.
    std::fs::write(into.join(STAGED_VERSION), version)
        .with_context(|| format!("recording the staged version in {}", into.display()))?;
    Ok(())
}

/// The file naming what has been staged. Its presence is the signal.
const STAGED_VERSION: &str = ".specline-staged-version";

/// What is staged and waiting, if anything.
///
/// Reading without applying, which is the whole of the difference B-75 and
/// KEEL-225 introduced: the daemon used to call [`apply_staged`] at startup and
/// swap the binary under whoever was using it. Now it reports, and applying is
/// something a person agrees to — because agreeing means the daemon restarts.
pub fn staged_version(dir: &Path) -> Result<Option<String>> {
    let marker = dir.join(STAGED_VERSION);
    if !marker.is_file() {
        return Ok(None);
    }
    let version = std::fs::read_to_string(&marker)
        .with_context(|| format!("reading {}", marker.display()))?
        .trim()
        .to_owned();
    Ok(Some(version))
}

/// The file recording when a check last ran, and how it went.
const LAST_CHECK: &str = ".specline-update-check";

/// What the last update check did.
///
/// Nothing here is about *this* check — it is about whether checking is
/// happening at all, which is a different question and the one nobody could
/// answer. A version with no "checked at" beside it cannot be told apart from a
/// version whose check has been failing quietly for a month, and both look
/// exactly like being up to date (KEEL-227).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct LastCheck {
    /// When the attempt finished, RFC 3339.
    pub at: String,
    /// Why it did not complete, or `None` if it did.
    ///
    /// A failed check is ordinary — a laptop asleep, no network, a release
    /// without a manifest — so this is reported rather than raised. What it
    /// must not do is stay invisible: an ordinary failure repeated for a month
    /// is not ordinary any more.
    pub error: Option<String>,
}

/// Record that a check happened, whatever came of it.
///
/// Written to the install directory beside the staged marker, rather than held
/// in memory, for two reasons: the daemon that did the checking is the one
/// process that restarts when an update is applied, and a stamp that resets on
/// restart would read as "never checked" exactly when it had just succeeded.
///
/// A failure to write is not a failure to check. The caller logs and carries
/// on: the point of the stamp is to make a quiet failure visible, and taking
/// the daemon down over it would be a loud one.
pub fn record_check(dir: &Path, error: Option<String>) -> Result<()> {
    let stamp = LastCheck {
        at: chrono::Utc::now().to_rfc3339(),
        error,
    };
    let path = dir.join(LAST_CHECK);
    std::fs::write(
        &path,
        serde_json::to_string(&stamp).context("serialising the update-check stamp")?,
    )
    .with_context(|| format!("recording the update check in {}", path.display()))?;
    Ok(())
}

/// When a check last ran, if one ever has.
///
/// `None` covers both "no check has completed" and "the stamp is unreadable",
/// which are the same thing to a reader: nothing here can vouch for the version
/// being current.
pub fn last_check(dir: &Path) -> Option<LastCheck> {
    let raw = std::fs::read_to_string(dir.join(LAST_CHECK)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Swap in a staged release, if there is one. Returns the version applied.
///
/// Called at startup, before anything is served. Renaming over a running
/// executable is safe on Unix — the process holds its own inode — but this runs
/// early precisely so that nothing has happened yet that a version change could
/// be inconsistent with.
///
/// Leaves the previous binaries as `<name>.previous`, same as [`install_from`],
/// so `specline update --rollback` undoes an unattended update exactly as it undoes
/// a deliberate one.
pub fn apply_staged(dir: &Path) -> Result<Option<String>> {
    let marker = dir.join(STAGED_VERSION);
    if !marker.is_file() {
        return Ok(None);
    }

    // A marker without both binaries beside it means a staging run died between
    // the two. Clear it rather than half-applying: the next check will stage
    // again, and the alternative is a daemon that fails to start for good.
    for name in BINARIES {
        if !dir.join(format!("{name}.staged")).is_file() {
            let _ = std::fs::remove_file(&marker);
            bail!(
                "a staged update was recorded but {name}.staged is missing, so it was discarded \
                 rather than applied in half. The next check will stage it again."
            );
        }
    }

    let version = std::fs::read_to_string(&marker)
        .with_context(|| format!("reading {}", marker.display()))?
        .trim()
        .to_owned();

    for name in BINARIES {
        let live = dir.join(name);
        let staged = dir.join(format!("{name}.staged"));
        let kept = dir.join(format!("{name}.previous"));

        if live.exists() {
            std::fs::rename(&live, &kept)
                .with_context(|| format!("keeping the current {name} as {}", kept.display()))?;
        }
        std::fs::rename(&staged, &live)
            .with_context(|| format!("applying the staged {name} to {}", live.display()))?;
        make_executable(&live)?;
    }

    std::fs::remove_file(&marker)
        .with_context(|| format!("clearing {} after applying it", marker.display()))?;
    Ok(Some(version))
}

/// Whether the unattended check may run at all.
///
/// `SPECLINE_AUTO_UPDATE=0` turns it off. This is the smaller half of KEEL-204,
/// landed here rather than after it because the alternative is shipping a
/// daily outbound request from a local-first tool with no way to stop it —
/// which is the thing KEEL-204 exists to avoid, not a detail of how it is
/// announced. The prompt at setup time and `specline doctor` reporting it are
/// still that task's.
///
/// Anything other than `0` is on, including nonsense, because a typo in this
/// variable should not silently disable an update path.
pub fn auto_update_enabled() -> bool {
    enabled_from(std::env::var("SPECLINE_AUTO_UPDATE").ok().as_deref())
}

/// The rule behind [`auto_update_enabled`], separated so it can be tested.
///
/// Setting an environment variable in a test is `unsafe` under edition 2024 and
/// the workspace denies `unsafe_code`, so the alternative to this split is not
/// testing the rule at all.
fn enabled_from(value: Option<&str>) -> bool {
    value.map(|v| v.trim() != "0").unwrap_or(true)
}

/// Look for a newer release and stage it if it is safe to apply.
///
/// The daemon's whole job here. Returns what it decided, so the caller can log
/// one line rather than this crate deciding how a daemon talks.
pub fn check_and_stage(install_dir: &Path, target: &str) -> Result<Plan> {
    let work = tempfile::tempdir().context("making a scratch directory for the download")?;
    let manifest = fetch_manifest(work.path())?;
    let decision = plan(
        env!("CARGO_PKG_VERSION"),
        specline_core::shipped_schema_version(),
        &manifest,
        target,
    )?;

    if let Plan::Apply { version, artifact } = &decision {
        // Already here? Then stop, before the 11 MB.
        //
        // `plan` compares the running binary against the manifest and cannot
        // see the install directory, so it goes on planning `Apply` for a
        // version already staged — every interval, for as long as the daemon
        // runs without being restarted. Checking here rather than teaching
        // `plan` about the filesystem keeps that function pure and testable,
        // which is the property that made the policy worth separating in the
        // first place.
        //
        // A failure to read the marker is deliberately *not* fatal: the worst
        // it costs is the redundant download this avoids, and refusing to
        // check for updates because a file could not be read would be the
        // larger fault.
        if already_staged(install_dir, version) {
            return Ok(Plan::AlreadyStaged {
                version: version.clone(),
            });
        }

        let archive = download(work.path(), artifact)?;
        verify(&archive, artifact, &manifest)?;
        let unpacked = unpack(&archive, work.path(), target)?;
        stage(&unpacked, install_dir, version)?;
    }
    Ok(decision)
}

/// Is this exact version already staged and waiting?
///
/// The guard that stops [`check_and_stage`] re-fetching an archive already on
/// disk. Its own function so the filesystem case can be tested — the call site
/// is behind a network fetch, so a test there would need GitHub.
///
/// **Unreadable is answered `false`**, deliberately. The cost of being wrong
/// that way is one redundant download; the cost of the other way is an update
/// that silently never gets staged because a marker file could not be read.
/// Between a wasted 11 MB and an update that never arrives, the download wins.
fn already_staged(install_dir: &Path, version: &str) -> bool {
    match staged_version(install_dir) {
        Ok(Some(staged)) => staged == version,
        Ok(None) => false,
        Err(e) => {
            tracing::debug!("could not read what is staged, so re-staging: {e:#}");
            false
        }
    }
}

/// Restore the binaries kept by the last [`install_from`].
pub fn rollback(dir: &Path) -> Result<()> {
    for name in BINARIES {
        if !dir.join(format!("{name}.previous")).is_file() {
            bail!(
                "there is no {name}.previous in {}, so there is no earlier version to go back \
                 to. Only an update taken by `specline update` leaves one.",
                dir.display()
            );
        }
    }

    for name in BINARIES {
        let live = dir.join(name);
        let kept = dir.join(format!("{name}.previous"));
        std::fs::rename(&kept, &live)
            .with_context(|| format!("restoring {} from {}", live.display(), kept.display()))?;
        make_executable(&live)?;
    }
    Ok(())
}

/// Give a freshly written binary the execute bit.
///
/// `fs::copy` carries the mode on Unix, so this is belt and braces for the
/// case where the source came out of an archive that did not record one.
fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)
            .with_context(|| format!("reading the mode of {}", path.display()))?
            .permissions();
        perms.set_mode(perms.mode() | 0o755);
        std::fs::set_permissions(path, perms)
            .with_context(|| format!("making {} executable", path.display()))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Unpack a release archive and return the directory holding its binaries.
///
/// `tar` rather than a crate: it is present on both platforms this ships to,
/// and the alternative is two dependencies (tar and xz) to do what one process
/// already does. The archive's internal directory is named for the target,
/// which is a published layout — checked against the real v0.1.1 asset rather
/// than assumed.
pub fn unpack(archive: &Path, into: &Path, target: &str) -> Result<PathBuf> {
    let status = Command::new("tar")
        .arg("-xJf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .status()
        .with_context(|| format!("running tar to unpack {}", archive.display()))?;
    if !status.success() {
        bail!(
            "tar could not unpack {}, so the download is not a usable release archive",
            archive.display()
        );
    }

    let dir = into.join(format!("{ARCHIVE_STEM}-{target}"));
    if !dir.is_dir() {
        bail!(
            "the archive did not contain the directory {ARCHIVE_STEM}-{target}. Its layout has \
             changed, and installing from a guess about where the binaries are is not worth doing."
        );
    }
    Ok(dir)
}

/// The triple this binary was built for, recorded by `build.rs`.
pub fn target() -> Result<&'static str> {
    let target = env!("SPECLINE_TARGET");
    if target.is_empty() {
        bail!(
            "this binary was built without a target triple recorded, so there is no way to know \
             which release archive belongs to it. Rebuild with a normal `cargo build`."
        );
    }
    Ok(target)
}

/// What happened when the new binaries were put in front of a running daemon.
///
/// A value rather than a printed line, so `run` says it once, `--json` says the
/// same thing in its own shape, and the awkward outcome — a daemon that came
/// back on the version it started with — is a case somebody has to handle
/// rather than a string nobody reads.
#[derive(Debug, PartialEq, Eq)]
pub enum Restart {
    /// Nothing was listening, so there was nothing to restart.
    NoDaemon,
    /// It went away and came back on the version that was just installed.
    Restarted {
        /// The version now serving.
        version: String,
    },
    /// It restarted, and came back on something other than what was installed.
    ///
    /// The interesting failure. `specline update` writes into the directory holding
    /// the `specline` being run; a daemon started from somewhere else has a
    /// different binary at its own path and is untouched by the update. Silence
    /// here would leave somebody looking at a version banner that never moves.
    Elsewhere {
        /// The version now serving.
        version: String,
        /// The version that was installed.
        installed: String,
    },
    /// It was asked and something went wrong, with what went wrong.
    Failed {
        /// What to tell the person, in a sentence.
        why: String,
    },
}

impl Restart {
    /// The sentence to print after the update line.
    fn sentence(&self, daemon: &str) -> String {
        match self {
            Restart::NoDaemon => {
                "No daemon was running, so there was nothing to restart. Start one with \
                 `specline-daemon` when you want it."
                    .to_owned()
            }
            Restart::Restarted { version } => {
                format!("The daemon restarted and is now serving {version}.")
            }
            Restart::Elsewhere { version, installed } => format!(
                "The daemon restarted but came back on {version}, not {installed}. It is running \
                 from a different directory than the one that was just updated, so it did not \
                 pick this up — find it with `pgrep -fl specline-daemon` and update that copy."
            ),
            Restart::Failed { why } => format!(
                "The daemon could not be restarted: {why}\n\nIt is still running the old version. \
                 Restart it yourself with:\n\n    pkill -f specline-daemon && specline-daemon\n\nThe \
                 daemon it tried was {daemon} — pass `--daemon` if yours is elsewhere."
            ),
        }
    }

    /// The same thing for `--json`.
    fn as_json(&self) -> serde_json::Value {
        match self {
            Restart::NoDaemon => serde_json::json!({ "restarted": false, "reason": "no_daemon" }),
            Restart::Restarted { version } => {
                serde_json::json!({ "restarted": true, "version": version })
            }
            Restart::Elsewhere { version, installed } => serde_json::json!({
                "restarted": true,
                "reason": "different_install",
                "version": version,
                "installed": installed,
            }),
            Restart::Failed { why } => {
                serde_json::json!({ "restarted": false, "reason": "failed", "detail": why })
            }
        }
    }
}

/// Read the version a daemon is serving, or `None` if none answers.
///
/// Short timeouts throughout: this runs against loopback, and a daemon that
/// takes seconds to answer its own health check is one this should stop waiting
/// for rather than one to be patient with.
fn daemon_version(daemon: &str) -> Option<String> {
    let response = ureq::get(&format!("{}/api/health", daemon.trim_end_matches('/')))
        .timeout(std::time::Duration::from_secs(2))
        .call()
        .ok()?;
    let body: serde_json::Value = response.into_json().ok()?;
    body.get("version")?.as_str().map(str::to_owned)
}

/// Ask the daemon to restart into the binaries that were just installed, and
/// wait to see what comes back.
///
/// The waiting is the point. "Asked it to restart" is a claim about a request;
/// "it is now serving 0.1.3" is a claim about the thing somebody cares about,
/// and the difference between them is the whole class of bug this project keeps
/// meeting. So this polls health until the version it reads is one it can
/// report, rather than returning as soon as the POST succeeds.
pub fn restart_daemon(daemon: &str, installed: Option<&str>, token: &str) -> Restart {
    let base = daemon.trim_end_matches('/');

    // Nothing listening is the ordinary case for anyone who does not leave a
    // daemon up, and it is not a failure.
    let before = match daemon_version(base) {
        Some(version) => version,
        None => return Restart::NoDaemon,
    };

    if let Err(e) = ureq::post(&format!("{base}/api/update/restart"))
        // Restarting is a mutating request, so it carries the daemon's token
        // (KEEL-238). Read from the store's directory, because that is where
        // the daemon wrote it.
        .set("x-specline-token", token)
        .timeout(std::time::Duration::from_secs(5))
        .call()
    {
        return match e {
            ureq::Error::Status(401, _) => Restart::Failed {
                why: format!(
                    "the daemon on {base} refused the token. It is a different daemon from the \
                     one that wrote the token file — check `--home`, or restart it by hand"
                ),
            },
            ureq::Error::Status(404, _) => Restart::Failed {
                why: format!(
                    "the daemon on {base} is running {before}, which is too old to know how to \
                     restart itself"
                ),
            },
            other => Restart::Failed {
                why: format!("{other}"),
            },
        };
    }

    // It replies before it execs, so the process is still the old one for a
    // moment. Poll until something answers with a version, then say what that
    // version is — including when it is the one we started with, which means
    // the restart happened and changed nothing.
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(250));
        if let Some(version) = daemon_version(base) {
            return match installed {
                Some(installed) if version != installed => Restart::Elsewhere {
                    version,
                    installed: installed.to_owned(),
                },
                _ => Restart::Restarted { version },
            };
        }
    }

    Restart::Failed {
        why: format!("it stopped answering on {base} and did not come back within ten seconds"),
    }
}

/// The daemon's token, or an empty string.
///
/// Empty rather than an error: the daemon is the authority on whether a request
/// is acceptable, and its refusal says far more than a local guess about a
/// missing file could. A daemon that is not running has no token and needs
/// none.
fn read_token(home: &Path) -> String {
    specline_core::token::read(home)
        .unwrap_or_default()
        .unwrap_or_default()
}

/// `specline update`.
///
/// One line when there is nothing to do, one line when something is done, and
/// a paragraph only when a person has to decide something. When the daemon
/// eventually runs this check on its own schedule — the open half of KEEL-203 —
/// it will report through a log nobody reads, so the terminal is where any of
/// this is legible and the wording is worth the care.
///
/// Replacing the binaries is only half of an update, because the daemon is a
/// separate process that goes on running what it loaded at startup. It is asked
/// to restart itself here, and what comes back is reported — see
/// [`restart_daemon`].
pub fn run(
    check_only: bool,
    rollback_requested: bool,
    json: bool,
    daemon: &str,
    home: &Path,
) -> Result<()> {
    // Each daemon mints its own token, so this is read now rather than cached.
    let token = read_token(home);
    let dir = install_dir()?;

    if rollback_requested {
        rollback(&dir)?;
        // No expected version: what is being restored is whatever was there
        // before, and this process is the *new* binary asking, so its own
        // version is the wrong thing to compare against. Whatever comes back is
        // the answer, and reporting it is how you find out the rollback took.
        let restart = restart_daemon(daemon, None, &token);
        if json {
            println!(
                "{}",
                serde_json::json!({ "rolled_back": true, "daemon": restart.as_json() })
            );
        } else {
            println!(
                "Put the previous binaries back in {}.\n{}",
                dir.display(),
                restart.sentence(daemon)
            );
        }
        return Ok(());
    }

    let target = target()?;
    let work = tempfile::tempdir().context("making a scratch directory for the download")?;
    let manifest = fetch_manifest(work.path())?;

    let installed_version = env!("CARGO_PKG_VERSION");
    let installed_schema = specline_core::shipped_schema_version();
    let plan = plan(installed_version, installed_schema, &manifest, target)?;

    // One JSON object per run, so anything parsing this reads a line rather
    // than a stream. The branch that actually installs prints its own, with the
    // daemon's fate merged in — it cannot be printed here because it has not
    // happened yet.
    let installing = !check_only && matches!(plan, Plan::Apply { .. });
    if json && !installing {
        println!("{}", serde_json::to_string(&describe(&plan))?);
    }

    match &plan {
        Plan::UpToDate => {
            if !json {
                println!("Specline {installed_version} is the current release.");
            }
            Ok(())
        }
        Plan::Ahead { published } => {
            if !json {
                println!(
                    "Specline {installed_version} is newer than the published release ({published}). \
                     Leaving it alone."
                );
            }
            Ok(())
        }
        // Only `check_and_stage` produces this, and `run` calls `plan` directly,
        // so the CLI cannot currently reach it. Answered properly anyway rather
        // than with a catch-all: if the two ever converge, a wrong-but-plausible
        // "up to date" is exactly the failure this crate keeps meeting.
        Plan::AlreadyStaged { version } => {
            if !json {
                println!(
                    "Specline {version} is already downloaded and waiting. Restart the daemon to \
                     run it."
                );
            }
            Ok(())
        }
        Plan::NeedsAPerson { version, from, to } => {
            if !json {
                println!(
                    "Specline {version} is available and changes the store's shape (schema {from} → \
                     {to}), so it is not applied automatically.\n\nA migration rewrites your \
                     store and cannot be undone — `specline update --rollback` puts the binaries \
                     back, not the data. Take it deliberately, with the daemon stopped:\n\n    \
                     specline backup --dest <dir>\n    # install {version} the way you installed Specline the \
                     first time\n    specline migrate\n\nThere is no flag here that will do it for \
                     you. That is the point of this refusal, not a gap in it."
                );
            }
            Ok(())
        }
        Plan::Apply { version, artifact } => {
            if check_only {
                if !json {
                    println!(
                        "Specline {version} is available and safe to apply (same store shape). Run \
                         `specline update` to take it."
                    );
                }
                return Ok(());
            }

            let archive = download(work.path(), artifact)?;
            verify(&archive, artifact, &manifest)?;
            let unpacked = unpack(&archive, work.path(), target)?;
            install_from(&unpacked, &dir)?;

            let restart = restart_daemon(daemon, Some(version), &token);

            if json {
                let mut out = describe(&plan);
                if let Some(fields) = out.as_object_mut() {
                    fields.insert("daemon".to_owned(), restart.as_json());
                }
                println!("{}", serde_json::to_string(&out)?);
            } else {
                println!(
                    "Updated Specline {installed_version} → {version}.\n{}\n\nTo go back: `specline \
                     update --rollback`.",
                    restart.sentence(daemon)
                );
            }
            Ok(())
        }
    }
}

/// The plan as JSON, for `--json`.
///
/// Hand-written rather than derived: the wire shape is read by whatever watches
/// this, and deriving it would tie that contract to the enum's field names.
fn describe(plan: &Plan) -> serde_json::Value {
    match plan {
        Plan::UpToDate => serde_json::json!({ "action": "none", "reason": "up_to_date" }),
        Plan::Ahead { published } => {
            serde_json::json!({ "action": "none", "reason": "ahead", "published": published })
        }
        Plan::AlreadyStaged { version } => serde_json::json!({
            "action": "none",
            "reason": "already_staged",
            "version": version,
        }),
        Plan::NeedsAPerson { version, from, to } => serde_json::json!({
            "action": "needs_a_person",
            "reason": "schema_change",
            "version": version,
            "schema_from": from,
            "schema_to": to,
        }),
        Plan::Apply { version, artifact } => serde_json::json!({
            "action": "apply",
            "version": version,
            "artifact": artifact,
        }),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn manifest(version: &str, schema: i32) -> ReleaseManifest {
        ReleaseManifest {
            version: version.to_owned(),
            schema_version: schema,
            artifacts: BTreeMap::new(),
        }
    }

    /// The guard that stops an update being downloaded over and over.
    ///
    /// `plan` compares the *running* binary against the manifest and cannot see
    /// the install directory, so once something is staged it goes on planning
    /// `Apply` for it — and before this guard existed, a daemon left running
    /// with an update pending re-downloaded, re-verified, re-unpacked and
    /// re-staged the same 11 MB archive every interval until somebody
    /// restarted it (KEEL-317). Halving the check interval doubled that.
    #[test]
    fn a_version_already_staged_is_not_staged_again() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(STAGED_VERSION), "0.2.0").unwrap();

        assert!(
            already_staged(dir.path(), "0.2.0"),
            "the staged version is the candidate, so there is nothing to fetch"
        );
    }

    /// The cases that must still download, which are what makes the guard safe
    /// rather than merely cheap. Getting any of these wrong means an update
    /// that never arrives — a far worse failure than a wasted download, and an
    /// invisible one.
    #[test]
    fn anything_other_than_an_exact_match_still_stages() {
        let dir = tempfile::tempdir().unwrap();

        // Nothing staged at all: the ordinary first-time case.
        assert!(
            !already_staged(dir.path(), "0.2.0"),
            "no marker means nothing is waiting"
        );

        // A *different* version staged. The newer release must replace it, not
        // be skipped because the directory happens to be non-empty.
        std::fs::write(dir.path().join(STAGED_VERSION), "0.1.9").unwrap();
        assert!(
            !already_staged(dir.path(), "0.2.0"),
            "0.1.9 is staged and 0.2.0 is the candidate, so 0.2.0 must still be fetched"
        );

        // A marker that cannot be parsed as a version is still not a match.
        std::fs::write(dir.path().join(STAGED_VERSION), "").unwrap();
        assert!(
            !already_staged(dir.path(), "0.2.0"),
            "an empty marker vouches for nothing"
        );
    }

    /// `AlreadyStaged` must not be confused with `UpToDate`.
    ///
    /// They are opposite advice — one means there is nothing to take, the other
    /// means there is something to take and it is already here — and the JSON
    /// is read by whatever watches `specline update --json`.
    #[test]
    fn already_staged_describes_itself_as_its_own_reason() {
        let described = describe(&Plan::AlreadyStaged {
            version: "0.2.0".to_owned(),
        });
        assert_eq!(described["action"], "none");
        assert_eq!(described["reason"], "already_staged");
        assert_eq!(described["version"], "0.2.0");

        let up_to_date = describe(&Plan::UpToDate);
        assert_ne!(
            described["reason"], up_to_date["reason"],
            "these are opposite advice and must not render the same"
        );
    }

    /// A daemon-shaped thing on loopback, for the restart tests.
    ///
    /// Enough HTTP to answer `/api/health` with a version and to take the
    /// restart POST. It cannot be the real daemon: the endpoint under test ends
    /// in `exec`, and a test that reached it would replace the test binary with
    /// itself. So what is tested here is the caller's half — which is the half
    /// that decides what a person is told.
    ///
    /// `after` is the version health reports once the restart has been asked
    /// for, which is how a real restart looks from outside. Setting it to the
    /// version it already served is how a daemon running from somewhere else
    /// looks, and that is a case with its own sentence.
    fn stub_daemon(before: &str, after: Option<&str>, restart_status: u16) -> String {
        use std::io::{Read, Write};
        use std::sync::{Arc, Mutex};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let version = Arc::new(Mutex::new(before.to_owned()));
        let after = after.map(str::to_owned);

        // Detached: the harness ends the process and the thread with it. A stop
        // flag here would be ceremony around a listener nothing else can reach.
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 1024];
                let read = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..read]).to_string();

                let response = if request.starts_with("POST") {
                    if restart_status == 200 {
                        if let Some(next) = &after {
                            *version.lock().unwrap() = next.clone();
                        }
                        "HTTP/1.1 200 OK\r\nContent-Length: 20\r\n\r\n{\"restarting\":true}\n"
                            .to_owned()
                    } else {
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_owned()
                    }
                } else {
                    let body = format!("{{\"version\":\"{}\"}}", version.lock().unwrap());
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: \
                         {}\r\n\r\n{body}",
                        body.len()
                    )
                };
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });

        format!("http://{addr}")
    }

    /// The link the interface shows beside a version. A release page rather
    /// than a file in the repository, because it is the one place that says
    /// what a *published* version contains and it is reachable without a
    /// checkout.
    #[test]
    fn a_version_becomes_the_url_of_its_release() {
        assert_eq!(
            release_notes_url("0.1.3"),
            "https://github.com/kiritbasu/specline/releases/tag/v0.1.3"
        );
    }

    /// The tag carries a `v` and the version does not, so a caller that passes
    /// either gets the same link rather than `.../tag/vv0.1.3`.
    #[test]
    fn a_leading_v_is_not_doubled() {
        assert_eq!(release_notes_url("v0.1.3"), release_notes_url("0.1.3"));
    }

    /// Nobody home is the ordinary case, not a failure, and it must not be
    /// reported as one — most people do not leave a daemon running.
    #[test]
    fn no_daemon_is_not_a_failure() {
        // Bound and dropped, so the port is one nothing is listening on.
        let dead = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap()
        };
        let outcome = restart_daemon(&format!("http://{dead}"), Some("0.1.3"), "t");
        assert_eq!(outcome, Restart::NoDaemon);
        assert!(
            outcome.sentence("x").contains("nothing to restart"),
            "and it should say so plainly: {}",
            outcome.sentence("x")
        );
    }

    #[test]
    fn a_daemon_that_comes_back_on_the_new_version_is_reported_as_restarted() {
        let addr = stub_daemon("0.1.2", Some("0.1.3"), 200);
        assert_eq!(
            restart_daemon(&addr, Some("0.1.3"), "t"),
            Restart::Restarted {
                version: "0.1.3".to_owned()
            }
        );
    }

    /// The failure that is easiest to miss, and the reason the caller waits and
    /// re-reads the version rather than trusting the POST. `specline update` writes
    /// beside the `specline` being run; a daemon started from another directory has
    /// a different binary at its own path and the update never touched it.
    #[test]
    fn a_daemon_that_comes_back_unchanged_says_it_is_installed_elsewhere() {
        let addr = stub_daemon("0.1.2", None, 200);
        let outcome = restart_daemon(&addr, Some("0.1.3"), "t");
        assert_eq!(
            outcome,
            Restart::Elsewhere {
                version: "0.1.2".to_owned(),
                installed: "0.1.3".to_owned(),
            }
        );
        let said = outcome.sentence(&addr);
        assert!(
            said.contains("different directory") && said.contains("pgrep"),
            "and it must say what to do about it: {said}"
        );
    }

    /// A daemon old enough to predate the endpoint. The update itself worked,
    /// so this reports a restart that did not happen rather than an update that
    /// did not — and gives the command to do it by hand.
    #[test]
    fn a_daemon_without_the_endpoint_is_a_named_failure() {
        let addr = stub_daemon("0.1.2", None, 404);
        let outcome = restart_daemon(&addr, Some("0.1.3"), "t");
        let Restart::Failed { why } = &outcome else {
            panic!("expected a failure, got {outcome:?}");
        };
        assert!(
            why.contains("too old"),
            "it should name why the daemon refused: {why}"
        );
        assert!(
            outcome.sentence(&addr).contains("pkill -f specline-daemon"),
            "and every failure must leave the person able to do it themselves"
        );
    }

    /// Rollback has no version to expect — this process is the *new* binary
    /// asking — so whatever comes back is the answer.
    #[test]
    fn with_no_expected_version_whatever_comes_back_is_the_answer() {
        let addr = stub_daemon("0.1.3", Some("0.1.2"), 200);
        assert_eq!(
            restart_daemon(&addr, None, "t"),
            Restart::Restarted {
                version: "0.1.2".to_owned()
            }
        );
    }

    #[test]
    fn same_version_is_up_to_date() {
        let m = manifest("0.1.1", 7);
        assert_eq!(
            plan("0.1.1", 7, &m, "aarch64-apple-darwin").unwrap(),
            Plan::UpToDate
        );
    }

    #[test]
    fn a_newer_release_on_the_same_schema_applies() {
        let m = manifest("0.1.2", 7);
        assert_eq!(
            plan("0.1.1", 7, &m, "aarch64-apple-darwin").unwrap(),
            Plan::Apply {
                version: "0.1.2".to_owned(),
                artifact: "specline-aarch64-apple-darwin.tar.xz".to_owned(),
            }
        );
    }

    /// The whole point of the task: a schema move never applies itself.
    #[test]
    fn a_schema_change_waits_for_a_person() {
        let m = manifest("0.2.0", 8);
        assert_eq!(
            plan("0.1.1", 7, &m, "aarch64-apple-darwin").unwrap(),
            Plan::NeedsAPerson {
                version: "0.2.0".to_owned(),
                from: 7,
                to: 8,
            }
        );
    }

    /// `releases/latest` resolves to the latest *non*-prerelease, so anybody on
    /// an rc is ahead of it. Applying that is a downgrade wearing a successful
    /// update's clothes.
    #[test]
    fn an_older_published_release_is_not_applied() {
        let m = manifest("0.1.1", 7);
        assert_eq!(
            plan("0.1.2", 7, &m, "aarch64-apple-darwin").unwrap(),
            Plan::Ahead {
                published: "0.1.1".to_owned()
            }
        );
    }

    #[test]
    fn a_prerelease_is_older_than_its_own_release() {
        // 0.2.0-rc.1 installed, 0.2.0 published: an upgrade, not a downgrade.
        let m = manifest("0.2.0", 7);
        assert_eq!(
            plan("0.2.0-rc.1", 7, &m, "aarch64-apple-darwin").unwrap(),
            Plan::Apply {
                version: "0.2.0".to_owned(),
                artifact: "specline-aarch64-apple-darwin.tar.xz".to_owned(),
            }
        );
    }

    #[test]
    fn an_unreadable_published_version_is_an_error_not_a_guess() {
        let m = manifest("latest", 7);
        let err = plan("0.1.1", 7, &m, "aarch64-apple-darwin").unwrap_err();
        assert!(
            err.to_string().contains("not MAJOR.MINOR.PATCH"),
            "unhelpful error: {err}"
        );
    }

    #[test]
    fn a_missing_checksum_refuses_rather_than_skipping() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("specline-x.tar.xz");
        std::fs::write(&file, b"whatever").unwrap();

        let err = verify(&file, "specline-x.tar.xz", &manifest("0.1.2", 7)).unwrap_err();
        assert!(
            err.to_string().contains("no checksum"),
            "unhelpful error: {err}"
        );
    }

    #[test]
    fn a_wrong_checksum_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("specline-x.tar.xz");
        std::fs::write(&file, b"whatever").unwrap();

        let mut m = manifest("0.1.2", 7);
        m.artifacts
            .insert("specline-x.tar.xz".to_owned(), "00".repeat(32));

        let err = verify(&file, "specline-x.tar.xz", &m).unwrap_err();
        assert!(
            err.to_string().contains("does not match"),
            "unhelpful error: {err}"
        );
        assert!(err.to_string().contains("Nothing has been installed"));
    }

    #[test]
    fn a_right_checksum_passes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("specline-x.tar.xz");
        std::fs::write(&file, b"whatever").unwrap();

        let mut m = manifest("0.1.2", 7);
        // sha256("whatever")
        m.artifacts.insert(
            "specline-x.tar.xz".to_owned(),
            "85738f8f9a7f1b04b5329c590ebcb9e425925c6d0984089c43a022de4f19c281".to_owned(),
        );

        verify(&file, "specline-x.tar.xz", &m).unwrap();
    }

    #[test]
    fn installing_keeps_the_previous_binaries() {
        let fresh = tempfile::tempdir().unwrap();
        let live = tempfile::tempdir().unwrap();
        for name in BINARIES {
            std::fs::write(fresh.path().join(name), b"new").unwrap();
            std::fs::write(live.path().join(name), b"old").unwrap();
        }

        install_from(fresh.path(), live.path()).unwrap();

        for name in BINARIES {
            assert_eq!(std::fs::read(live.path().join(name)).unwrap(), b"new");
            assert_eq!(
                std::fs::read(live.path().join(format!("{name}.previous"))).unwrap(),
                b"old"
            );
        }
    }

    /// Both binaries move together or neither does. A release archive missing
    /// one of them is the drift this task exists to prevent, arriving as an
    /// install rather than as a slow divergence.
    #[test]
    fn a_partial_archive_installs_nothing() {
        let fresh = tempfile::tempdir().unwrap();
        let live = tempfile::tempdir().unwrap();
        std::fs::write(fresh.path().join("specline"), b"new").unwrap();
        for name in BINARIES {
            std::fs::write(live.path().join(name), b"old").unwrap();
        }

        let err = install_from(fresh.path(), live.path()).unwrap_err();
        assert!(
            err.to_string().contains("specline-daemon"),
            "unhelpful error: {err}"
        );
        for name in BINARIES {
            assert_eq!(
                std::fs::read(live.path().join(name)).unwrap(),
                b"old",
                "{name} was touched despite the refusal"
            );
        }
    }

    #[test]
    fn rollback_puts_the_previous_binaries_back() {
        let fresh = tempfile::tempdir().unwrap();
        let live = tempfile::tempdir().unwrap();
        for name in BINARIES {
            std::fs::write(fresh.path().join(name), b"new").unwrap();
            std::fs::write(live.path().join(name), b"old").unwrap();
        }

        install_from(fresh.path(), live.path()).unwrap();
        rollback(live.path()).unwrap();

        for name in BINARIES {
            assert_eq!(std::fs::read(live.path().join(name)).unwrap(), b"old");
        }
    }

    #[test]
    fn staging_then_applying_swaps_and_keeps_the_previous() {
        let fresh = tempfile::tempdir().unwrap();
        let live = tempfile::tempdir().unwrap();
        for name in BINARIES {
            std::fs::write(fresh.path().join(name), b"new").unwrap();
            std::fs::write(live.path().join(name), b"old").unwrap();
        }

        stage(fresh.path(), live.path(), "0.1.2").unwrap();

        // Staging alone changes nothing that is running.
        for name in BINARIES {
            assert_eq!(std::fs::read(live.path().join(name)).unwrap(), b"old");
        }

        assert_eq!(apply_staged(live.path()).unwrap().as_deref(), Some("0.1.2"));
        for name in BINARIES {
            assert_eq!(std::fs::read(live.path().join(name)).unwrap(), b"new");
            assert_eq!(
                std::fs::read(live.path().join(format!("{name}.previous"))).unwrap(),
                b"old"
            );
        }
    }

    /// The daemon calls this on every start, so the ordinary answer is "nothing
    /// staged" and it has to be cheap and silent.
    #[test]
    fn applying_with_nothing_staged_is_a_no_op() {
        let live = tempfile::tempdir().unwrap();
        assert_eq!(apply_staged(live.path()).unwrap(), None);
    }

    #[test]
    fn applying_twice_does_not_apply_the_second_time() {
        let fresh = tempfile::tempdir().unwrap();
        let live = tempfile::tempdir().unwrap();
        for name in BINARIES {
            std::fs::write(fresh.path().join(name), b"new").unwrap();
            std::fs::write(live.path().join(name), b"old").unwrap();
        }

        stage(fresh.path(), live.path(), "0.1.2").unwrap();
        apply_staged(live.path()).unwrap();
        // Second start: the marker is gone, so `new` must not become `previous`.
        assert_eq!(apply_staged(live.path()).unwrap(), None);
        for name in BINARIES {
            assert_eq!(
                std::fs::read(live.path().join(format!("{name}.previous"))).unwrap(),
                b"old"
            );
        }
    }

    /// A staging run killed between the two binaries. Applying half of it would
    /// leave `specline` and `specline-daemon` at different versions, which is the exact
    /// drift this whole task exists to prevent.
    #[test]
    fn a_marker_without_its_binaries_is_discarded_not_half_applied() {
        let live = tempfile::tempdir().unwrap();
        for name in BINARIES {
            std::fs::write(live.path().join(name), b"old").unwrap();
        }
        std::fs::write(live.path().join("specline.staged"), b"new").unwrap();
        std::fs::write(live.path().join(STAGED_VERSION), "0.1.2").unwrap();

        let err = apply_staged(live.path()).unwrap_err();
        assert!(
            err.to_string()
                .contains("specline-daemon.staged is missing"),
            "unhelpful error: {err}"
        );
        for name in BINARIES {
            assert_eq!(std::fs::read(live.path().join(name)).unwrap(), b"old");
        }
        // Cleared, so the next start is not stuck on the same failure for good.
        assert_eq!(apply_staged(live.path()).unwrap(), None);
    }

    #[test]
    fn a_partial_archive_stages_nothing_applicable() {
        let fresh = tempfile::tempdir().unwrap();
        let live = tempfile::tempdir().unwrap();
        std::fs::write(fresh.path().join("specline"), b"new").unwrap();

        assert!(stage(fresh.path(), live.path(), "0.1.2").is_err());
        assert_eq!(apply_staged(live.path()).unwrap(), None);
    }

    #[test]
    fn only_zero_turns_the_check_off() {
        assert!(!enabled_from(Some("0")));
        assert!(!enabled_from(Some(" 0 ")));
        assert!(enabled_from(None));
        assert!(enabled_from(Some("1")));
        // A typo should not silently disable an update path.
        assert!(enabled_from(Some("false")));
        assert!(enabled_from(Some("")));
    }

    #[test]
    fn rollback_with_nothing_kept_says_so() {
        let live = tempfile::tempdir().unwrap();
        for name in BINARIES {
            std::fs::write(live.path().join(name), b"only").unwrap();
        }

        let err = rollback(live.path()).unwrap_err();
        assert!(
            err.to_string().contains("no earlier version"),
            "unhelpful error: {err}"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod check_stamp_tests {
    use super::*;

    /// A check that ran and found nothing still has to leave a trace, or
    /// "nothing is staged" and "nothing has been checked in a month" are the
    /// same sentence (KEEL-227).
    #[test]
    fn a_successful_check_records_when_it_happened_and_no_error() {
        let dir = tempfile::tempdir().unwrap();
        record_check(dir.path(), None).unwrap();

        let stamp = last_check(dir.path()).expect("a check that ran is readable afterwards");
        assert!(stamp.error.is_none());
        assert!(
            chrono::DateTime::parse_from_rfc3339(&stamp.at).is_ok(),
            "the stamp is a timestamp a reader can parse: {}",
            stamp.at
        );
    }

    /// The failure case, and the one the field exists for: an ordinary failure
    /// repeated for a month is not ordinary, and it used to be invisible.
    #[test]
    fn a_failed_check_records_why() {
        let dir = tempfile::tempdir().unwrap();
        record_check(dir.path(), Some("no network".to_owned())).unwrap();

        assert_eq!(
            last_check(dir.path()).unwrap().error.as_deref(),
            Some("no network")
        );
    }

    /// The latest attempt is what is reported, so a check that recovers stops
    /// reporting the failure it recovered from.
    #[test]
    fn a_later_check_replaces_an_earlier_one() {
        let dir = tempfile::tempdir().unwrap();
        record_check(dir.path(), Some("no network".to_owned())).unwrap();
        record_check(dir.path(), None).unwrap();

        assert!(last_check(dir.path()).unwrap().error.is_none());
    }

    /// No stamp and an unreadable stamp are the same answer: nothing here can
    /// vouch for the version being current.
    #[test]
    fn a_directory_with_no_stamp_or_a_corrupt_one_reports_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(last_check(dir.path()).is_none());

        std::fs::write(dir.path().join(LAST_CHECK), "{not json").unwrap();
        assert!(last_check(dir.path()).is_none());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod verify_tests {
    use super::*;

    fn manifest_with(artifact: &str, digest: &str) -> ReleaseManifest {
        let mut artifacts = BTreeMap::new();
        artifacts.insert(artifact.to_owned(), digest.to_owned());
        ReleaseManifest {
            version: "0.1.0".to_owned(),
            schema_version: specline_core::shipped_schema_version(),
            artifacts,
        }
    }

    /// The hex encoding moved when `sha2` 0.11 stopped implementing `LowerHex`
    /// on its output (KEEL-254). This is the assertion that the move did not
    /// change what a digest looks like — a manifest is written by `sha256sum`
    /// and compared as a string, so an encoding that is correct but uppercase,
    /// or one character short, rejects every release with a message that reads
    /// like a corrupted download.
    #[test]
    fn a_matching_archive_verifies() {
        use sha2::{Digest, Sha256};
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("specline.tar.xz");
        std::fs::write(&file, b"the bytes of a release").unwrap();

        let digest = specline_core::hex::encode(&Sha256::digest(b"the bytes of a release"));
        assert_eq!(digest.len(), 64, "sha256 is 32 bytes, so 64 hex characters");
        assert_eq!(digest, digest.to_lowercase());

        verify(
            &file,
            "specline.tar.xz",
            &manifest_with("specline.tar.xz", &digest),
        )
        .expect("the archive matches what the manifest says");
    }

    /// The half that matters. A verifier that accepts everything passes the
    /// test above.
    #[test]
    fn an_archive_that_does_not_match_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("specline.tar.xz");
        std::fs::write(&file, b"not the bytes anyone published").unwrap();

        let err = verify(
            &file,
            "specline.tar.xz",
            &manifest_with("specline.tar.xz", &"a".repeat(64)),
        )
        .expect_err("a mismatched archive must not install");

        let message = format!("{err}");
        assert!(
            message.contains("does not match the checksum"),
            "the refusal says what is wrong: {message}"
        );
        assert!(
            message.contains("Nothing has been installed"),
            "and what did not happen as a result: {message}"
        );
    }

    /// A release published before the manifest carried checksums. Refused
    /// rather than skipped — an installer that verifies nothing and says so
    /// quietly is the failure `scripts/patch-installer.sh` exists for.
    #[test]
    fn an_artifact_with_no_recorded_checksum_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("specline.tar.xz");
        std::fs::write(&file, b"anything").unwrap();

        let err = verify(
            &file,
            "specline.tar.xz",
            &manifest_with("something-else.tar.xz", &"b".repeat(64)),
        )
        .expect_err("no checksum is not the same as a passing checksum");
        assert!(format!("{err}").contains("nothing to verify it against"));
    }
}
