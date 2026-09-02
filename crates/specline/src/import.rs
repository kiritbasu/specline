//! `specline import` — put a markdown file into Specline as a versioned document.
//!
//! The one-way door from "the specs live in the repo" to "the specs live in
//! Specline". After Specline is the source of truth this is a migration tool, run once
//! per file, not part of the loop — `specline generate` runs the other way and is
//! what runs from then on.
//!
//! Importing records where the file came from, as the artifact's
//! `mirror_path`. That is what closes the round trip: the document remembers
//! which repository file it *is*, so generation puts it back in the same place
//! rather than inventing a new one and leaving the original to rot beside it.
//!
//! Importing the same file twice appends a revision rather than making a second
//! artifact, and re-importing an unchanged file does nothing at all — so it is
//! safe to re-run during the migration, when a file may still be edited by hand
//! once or twice before the switch.

use anyhow::{Context, Result};
use specline_core::{
    Actor, Decision, Document, Entity, EntityId, EntityQuery, EntityStore, EntityType, Provenance,
    Question, Spec, SpecKind, Store, Surface,
};
use std::path::{Path, PathBuf};

/// What an import did to one file.
pub struct Imported {
    /// The artifact it landed in.
    pub entity_id: EntityId,
    /// Its title.
    pub title: String,
    /// The revision now current.
    pub version: i32,
    /// Whether the artifact itself was created by this import.
    pub created: bool,
    /// Whether this import produced a new revision, or the content was
    /// already identical.
    pub revised: bool,
    /// Bytes of body stored.
    pub bytes: usize,
    /// The repository path this artifact now claims, if one could be worked
    /// out.
    pub mirror_path: Option<String>,
}

/// What an import would land on, worked out without writing anything.
///
/// Shared by [`file`] and [`preview`] so the two cannot disagree about which
/// artifact a path belongs to. A preview that resolved differently from the
/// import it is previewing would be worse than no preview: it would be
/// confidently wrong, and only about the cases where it mattered.
struct Resolved {
    /// The repository-relative path the artifact would claim.
    mirror_path: Option<String>,
    /// The body, banner stripped.
    raw: String,
    /// The title, from `--title`, the first heading, or the filename.
    title: String,
    /// The artifact this lands on, when one already answers to that title.
    existing: Option<EntityId>,
}

fn resolve(
    store: &Store,
    path: &Path,
    project_id: &EntityId,
    entity_type: EntityType,
    title_override: Option<String>,
) -> Result<Resolved> {
    let mirror_path = repo_relative(store, project_id, path)?;
    let on_disk =
        std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    // A file that has been generated carries a banner naming the artifact it
    // came from. That is generation's bookkeeping, not the document's content,
    // and storing it would make every import-then-generate cycle stack another
    // banner on the last one.
    let raw = strip_generated_banner(&on_disk);

    let title = title_override
        .or_else(|| heading_of(&raw))
        .unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Untitled".to_owned())
        });

    // Find an existing artifact with this title before creating one. The
    // create is idempotent anyway, but resolving first means a re-import lands
    // on the same artifact even if its title was edited in Specline afterwards —
    // which is the common case once the store is the source of truth.
    let existing = find_by_title(store, project_id, entity_type, &title)?;

    Ok(Resolved {
        mirror_path,
        raw,
        title,
        existing,
    })
}

/// Import one markdown file.
pub fn file(
    store: &mut Store,
    path: &Path,
    project_id: &EntityId,
    entity_type: EntityType,
    kind: Option<SpecKind>,
    title_override: Option<String>,
) -> Result<Imported> {
    let Resolved {
        mirror_path,
        raw,
        title,
        existing,
    } = resolve(store, path, project_id, entity_type, title_override)?;

    let prov = Provenance {
        actor: Actor::Human,
        session_id: Some("ses_import".to_owned()),
        surface: Some(Surface::Cli),

        client: None,
    };

    let (entity_id, created) = match existing {
        Some(id) => (id, false),
        None => {
            let entity: Entity = match entity_type {
                EntityType::Spec => {
                    let mut s = Spec::new(project_id.clone(), &title);
                    s.kind = kind.unwrap_or_else(|| infer_kind(path, &title));
                    s.mirror_path = mirror_path.clone();
                    s.into()
                }
                EntityType::Decision => {
                    let mut d = Decision::new(project_id.clone(), &title);
                    d.mirror_path = mirror_path.clone();
                    d.into()
                }
                EntityType::Question => {
                    let mut q = Question::new(project_id.clone(), &title);
                    q.mirror_path = mirror_path.clone();
                    q.into()
                }
                other => anyhow::bail!(
                    "cannot import a file as a {other}. Prose-bearing types are spec, \
                     decision and question"
                ),
            };
            let created = store.create(entity, &prov)?;
            (created.entity.id().clone(), created.created)
        }
    };

    // An artifact imported before this behaviour existed has no recorded
    // path, and would otherwise be generated into `.specline/` at a slugged name
    // while the file it came from sat beside it going stale.
    adopt_path(store, &entity_id, mirror_path.as_deref(), &prov)?;

    let before = store.revision(&entity_id, None)?.map(|d| d.version);
    let doc = Document::first(
        entity_type,
        entity_id.clone(),
        Some(project_id.clone()),
        &title,
        &raw,
        prov.actor,
        chrono::Utc::now(),
    )?
    .attributed(prov.session_id.clone(), prov.surface);
    let written = store.write_revision(doc)?;

    Ok(Imported {
        entity_id,
        title,
        version: written.version,
        created,
        // `write_revision` is content-addressed: an unchanged body returns the
        // existing revision rather than appending a duplicate.
        revised: before != Some(written.version),
        bytes: raw.len(),
        mirror_path,
    })
}

/// The path of `file` relative to the project's checkout.
///
/// `None` when the project has no recorded checkout or the file sits outside
/// it — in which case the artifact adopts no file and generation sends it to
/// the `.specline/` mirror instead. Guessing would be worse: a wrong path means
/// generation writes over something it does not own.
fn repo_relative(store: &Store, project_id: &EntityId, file: &Path) -> Result<Option<String>> {
    let Some(Entity::Project(project)) = store.get(project_id)? else {
        return Ok(None);
    };
    let Some(root) = project.root_path.as_deref() else {
        return Ok(None);
    };

    let root = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    let absolute = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    Ok(absolute
        .strip_prefix(&root)
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/")))
}

/// Record the repository file this artifact is, if it does not already say so.
fn adopt_path(
    store: &mut Store,
    entity_id: &EntityId,
    path: Option<&str>,
    prov: &Provenance,
) -> Result<()> {
    let Some(path) = path else { return Ok(()) };
    let Some(entity) = store.get(entity_id)? else {
        return Ok(());
    };
    if entity.mirror_path() == Some(path) {
        return Ok(());
    }

    let mut changes = serde_json::Map::new();
    changes.insert("mirror_path".to_owned(), serde_json::json!(path));
    store.update(entity_id, entity.audit().version, &changes, prov)?;
    Ok(())
}

/// Drop a leading `<!-- specline:generated … -->` banner, if there is one.
fn strip_generated_banner(content: &str) -> String {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("<!-- specline:generated") {
        return content.to_owned();
    }
    match trimmed.find("-->") {
        Some(end) => trimmed[end + 3..].trim_start_matches('\n').to_owned(),
        // An unterminated banner is a damaged file, not a generated one.
        // Storing it whole is the conservative choice: nothing is lost, and
        // the next generate rewrites it anyway.
        None => content.to_owned(),
    }
}

/// The first level-one heading, which is what these files call themselves.
fn heading_of(markdown: &str) -> Option<String> {
    markdown.lines().find_map(|line| {
        line.strip_prefix("# ")
            .map(|h| h.trim().trim_end_matches(" —").trim().to_owned())
            .filter(|h| !h.is_empty())
    })
}

/// Guess the kind from the filename and title.
///
/// A guess, and overridable with `--kind`. Getting it wrong costs a wrong
/// badge in the UI, not data.
fn infer_kind(path: &Path, title: &str) -> SpecKind {
    let hay = format!(
        "{} {}",
        path.file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default(),
        title.to_lowercase()
    );
    if hay.contains("prd") || hay.contains("requirement") || hay.contains("product") {
        SpecKind::Prd
    } else if hay.contains("rfc") {
        SpecKind::Rfc
    } else if hay.contains("spec") {
        SpecKind::Spec
    } else if hay.contains("design") {
        SpecKind::DesignDoc
    } else {
        SpecKind::Note
    }
}

/// Find a live artifact of this type with this title.
fn find_by_title(
    store: &Store,
    project_id: &EntityId,
    entity_type: EntityType,
    title: &str,
) -> Result<Option<EntityId>> {
    let page = store.list(
        &EntityQuery::in_project(project_id.clone())
            .of_type(entity_type)
            .limited(5_000),
    )?;
    Ok(page
        .items
        .iter()
        .find(|e| e.label().eq_ignore_ascii_case(title))
        .map(|e| e.id().clone()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn the_title_comes_from_the_first_heading() {
        assert_eq!(
            heading_of("# Specline — Technical Specification\n\nBody\n").as_deref(),
            Some("Specline — Technical Specification")
        );
        // A heading further down still counts; frontmatter and blank lines
        // above it are common.
        assert_eq!(
            heading_of("<!-- generated -->\n\n# Real Title\n").as_deref(),
            Some("Real Title")
        );
        assert_eq!(heading_of("no heading here\n"), None);
        assert_eq!(heading_of("#not a heading\n"), None);
    }

    #[test]
    fn a_generated_banner_is_not_stored_as_content() {
        let generated =
            "<!-- specline:generated spec spc_1 v3\n     do not edit -->\n\n# Title\n\nBody\n";
        assert_eq!(strip_generated_banner(generated), "# Title\n\nBody\n");

        // Idempotent: re-importing a file Specline generated must not slowly eat
        // the top of the document, nor stack banner on banner.
        assert_eq!(
            strip_generated_banner(&strip_generated_banner(generated)),
            "# Title\n\nBody\n"
        );

        // A hand-written file is untouched, comments and all.
        let plain = "<!-- a normal comment -->\n\n# Title\n";
        assert_eq!(strip_generated_banner(plain), plain);
        assert_eq!(strip_generated_banner("# Title\n"), "# Title\n");

        // A truncated banner is damage, not generation: keep everything.
        let broken = "<!-- specline:generated spec spc_1 v3\n# Title\n";
        assert_eq!(strip_generated_banner(broken), broken);
    }

    #[test]
    fn the_kind_is_guessed_from_the_name() {
        assert_eq!(
            infer_kind(Path::new("PRD.md"), "Product Requirements"),
            SpecKind::Prd
        );
        assert_eq!(
            infer_kind(Path::new("SPEC.md"), "Technical Specification"),
            SpecKind::Spec
        );
        assert_eq!(
            infer_kind(Path::new("0001-rfc.md"), "Some RFC"),
            SpecKind::Rfc
        );
        assert_eq!(
            infer_kind(Path::new("HANDOFF.md"), "Handoff"),
            SpecKind::Note
        );
    }

    #[test]
    fn importing_the_same_file_twice_does_not_duplicate_or_re_version() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("specline.sqlite")).unwrap();
        let project = store
            .create(
                specline_core::Project::new("specline", "Specline").into(),
                &Provenance::anonymous(Actor::Human),
            )
            .unwrap()
            .entity
            .id()
            .clone();

        let path = dir.path().join("SPEC.md");
        std::fs::write(&path, "# Storage\n\nOne file, one engine.\n").unwrap();

        let first = file(&mut store, &path, &project, EntityType::Spec, None, None).unwrap();
        assert!(first.created);
        assert!(first.revised);
        assert_eq!(first.version, 1);

        let again = file(&mut store, &path, &project, EntityType::Spec, None, None).unwrap();
        assert!(
            !again.created,
            "a re-import must not create a second artifact"
        );
        assert!(
            !again.revised,
            "unchanged content must not append a revision"
        );
        assert_eq!(again.version, 1);
        assert_eq!(again.entity_id, first.entity_id);

        // A real edit does append.
        std::fs::write(&path, "# Storage\n\nOne file, one engine, and a WAL.\n").unwrap();
        let edited = file(&mut store, &path, &project, EntityType::Spec, None, None).unwrap();
        assert!(edited.revised);
        assert_eq!(edited.version, 2);
        assert_eq!(edited.entity_id, first.entity_id);
    }

    #[test]
    fn the_whole_body_is_stored_not_a_summary() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("specline.sqlite")).unwrap();
        let project = store
            .create(
                specline_core::Project::new("specline", "Specline").into(),
                &Provenance::anonymous(Actor::Human),
            )
            .unwrap()
            .entity
            .id()
            .clone();

        let body = format!("# Big\n\n{}", "Paragraph of real prose.\n\n".repeat(2_000));
        let path = dir.path().join("BIG.md");
        std::fs::write(&path, &body).unwrap();

        let imported = file(&mut store, &path, &project, EntityType::Spec, None, None).unwrap();
        assert_eq!(imported.bytes, body.len());

        let stored = store.revision(&imported.entity_id, None).unwrap().unwrap();
        assert_eq!(stored.body, body, "the file must be stored whole");
    }
}

/// What importing a file would do, worked out without doing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// No artifact answers to this title yet.
    Create,
    /// One does, and the file's content differs from its current revision.
    Revise { from_version: i32 },
    /// One does, and the content is identical. Importing would write nothing.
    Unchanged { version: i32 },
}

impl Outcome {
    /// One word for a column.
    pub fn word(&self) -> &'static str {
        match self {
            Outcome::Create => "create",
            Outcome::Revise { .. } => "revise",
            Outcome::Unchanged { .. } => "unchanged",
        }
    }
}

/// What an import of one file would do.
pub struct Preview {
    /// The artifact it would land on, when one already exists.
    pub entity_id: Option<EntityId>,
    /// The title it would use.
    pub title: String,
    /// Create, revise, or nothing at all.
    pub outcome: Outcome,
    /// The repository path the artifact would claim after the import.
    pub mirror_path: Option<String>,
    /// The path it claims today, when that differs from the one above.
    ///
    /// Separate from `mirror_path` because a *changed* adopted path is the
    /// surprise worth flagging: it is what `specline generate` writes back over,
    /// and it is invisible afterwards.
    pub mirror_path_now: Option<String>,
    /// Bytes of body that would be stored.
    pub bytes: usize,
}

/// Work out what [`file`] would do, without writing anything.
///
/// Takes `&Store` rather than `&mut Store`, which is the part that matters:
/// a preview cannot write even by accident, it needs no advisory lock (B-60),
/// and it runs against a store a daemon is already serving — which is the
/// state an adopter's machine is in when they are deciding what to import.
///
/// The "would this change anything" answer comes from building the same
/// `Document` the import would build and comparing its `body_hash`, rather
/// than from a second opinion about what counts as a change. The title is part
/// of that hash, so renaming a file's heading correctly reads as a revision.
pub fn preview(
    store: &Store,
    path: &Path,
    project_id: &EntityId,
    entity_type: EntityType,
    title_override: Option<String>,
) -> Result<Preview> {
    let Resolved {
        mirror_path,
        raw,
        title,
        existing,
    } = resolve(store, path, project_id, entity_type, title_override)?;

    let (outcome, mirror_path_now) = match &existing {
        None => (Outcome::Create, None),
        Some(id) => {
            let current = store.revision(id, None)?;
            let outcome = match current {
                None => Outcome::Create,
                Some(doc) => {
                    let candidate = Document::first(
                        entity_type,
                        id.clone(),
                        Some(project_id.clone()),
                        &title,
                        &raw,
                        Actor::Human,
                        chrono::Utc::now(),
                    )?;
                    if candidate.body_hash == doc.body_hash {
                        Outcome::Unchanged {
                            version: doc.version,
                        }
                    } else {
                        Outcome::Revise {
                            from_version: doc.version,
                        }
                    }
                }
            };
            let now = store
                .get(id)?
                .and_then(|e| e.mirror_path().map(str::to_owned));
            let changed = if now.as_deref() == mirror_path.as_deref() {
                None
            } else {
                now
            };
            (outcome, changed)
        }
    };

    Ok(Preview {
        entity_id: existing,
        title,
        outcome,
        mirror_path,
        mirror_path_now,
        bytes: raw.len(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod preview_tests {
    use super::*;
    use specline_core::{EntityStore, Project};

    /// A store with one project, and a directory to put files in.
    fn fixture() -> (Store, EntityId, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("specline.sqlite")).unwrap();
        let project = store
            .create(
                Project::new("demo", "Demo").into(),
                &Provenance::anonymous(Actor::Human),
            )
            .unwrap()
            .entity
            .id()
            .clone();
        (store, project, dir)
    }

    fn write(dir: &tempfile::TempDir, name: &str, body: &str) -> PathBuf {
        let p = dir.path().join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn an_unseen_file_would_be_created() {
        let (store, project, dir) = fixture();
        let f = write(&dir, "spec.md", "# Storage\n\nOne file.\n");

        let p = preview(&store, &f, &project, EntityType::Spec, None).unwrap();
        assert_eq!(p.outcome, Outcome::Create);
        assert_eq!(p.title, "Storage");
        assert!(p.entity_id.is_none());
    }

    /// The distinction the task was written for: re-running a preview during a
    /// migration has to tell you whether anything would actually change.
    #[test]
    fn an_imported_file_reads_as_unchanged_and_an_edited_one_as_a_revision() {
        let (mut store, project, dir) = fixture();
        let f = write(&dir, "spec.md", "# Storage\n\nOne file.\n");
        file(&mut store, &f, &project, EntityType::Spec, None, None).unwrap();

        let same = preview(&store, &f, &project, EntityType::Spec, None).unwrap();
        assert!(
            matches!(same.outcome, Outcome::Unchanged { version: 1 }),
            "{:?}",
            same.outcome
        );
        assert!(
            same.entity_id.is_some(),
            "it should have found the artifact"
        );

        std::fs::write(&f, "# Storage\n\nOne file, and a second sentence.\n").unwrap();
        let edited = preview(&store, &f, &project, EntityType::Spec, None).unwrap();
        assert!(
            matches!(edited.outcome, Outcome::Revise { from_version: 1 }),
            "{:?}",
            edited.outcome
        );
    }

    /// The title is part of the body hash, so a renamed heading is a revision.
    /// It is also a *different artifact* if nothing answers to the new title,
    /// which is the sharper surprise and the reason to look before importing.
    #[test]
    fn renaming_the_heading_lands_somewhere_else_entirely() {
        let (mut store, project, dir) = fixture();
        let f = write(&dir, "spec.md", "# Storage\n\nOne file.\n");
        file(&mut store, &f, &project, EntityType::Spec, None, None).unwrap();

        std::fs::write(&f, "# Storage and retrieval\n\nOne file.\n").unwrap();
        let p = preview(&store, &f, &project, EntityType::Spec, None).unwrap();
        assert_eq!(
            p.outcome,
            Outcome::Create,
            "a new title matches no artifact, so this would make a second one"
        );
        assert_eq!(p.title, "Storage and retrieval");
    }

    /// The whole point: previewing must not write.
    #[test]
    fn a_preview_leaves_the_store_exactly_as_it_found_it() {
        let (store, project, dir) = fixture();
        let f = write(&dir, "spec.md", "# Storage\n\nOne file.\n");

        let before = store.list(&Default::default()).unwrap().total;
        for _ in 0..3 {
            preview(&store, &f, &project, EntityType::Spec, None).unwrap();
        }
        let after = store.list(&Default::default()).unwrap().total;
        assert_eq!(before, after, "a preview created something");
    }

    /// It takes `&Store`, so it can run against a store something else holds
    /// open for writing — which is the state an adopter's machine is in.
    #[test]
    fn a_preview_runs_while_another_process_holds_the_write_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("specline.sqlite");
        let mut writer = Store::open_exclusive(&path).unwrap();
        let project = writer
            .create(
                Project::new("demo", "Demo").into(),
                &Provenance::anonymous(Actor::Human),
            )
            .unwrap()
            .entity
            .id()
            .clone();

        let f = dir.path().join("spec.md");
        std::fs::write(&f, "# Storage\n\nOne file.\n").unwrap();

        let reader = Store::open(&path).expect("a preview must not need the lock");
        let p = preview(&reader, &f, &project, EntityType::Spec, None).unwrap();
        assert_eq!(p.outcome, Outcome::Create);
        drop(writer);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod adopted_path_tests {
    //! The one column of the preview that exists to prevent a silent loss.
    //!
    //! An artifact remembers which repository file it *is*, and `specline generate`
    //! writes it back there. Importing the same document from a different path
    //! moves that claim — so the next generate writes somewhere new and the old
    //! file stops being updated while still looking maintained. Nothing errors.
    //!
    //! The preview says so when it would happen. These are the tests that the
    //! saying works, because a warning nobody verified is the same as no
    //! warning, and this one guards a file getting quietly abandoned.

    use super::*;
    use specline_core::{EntityStore, Project, Provenance};

    /// A project whose `root_path` is the temporary directory, which is what
    /// makes `repo_relative` produce a path at all.
    fn rooted() -> (Store, EntityId, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("specline.sqlite")).unwrap();
        let mut project = Project::new("demo", "Demo");
        project.root_path = Some(dir.path().to_string_lossy().into_owned());
        let id = store
            .create(project.into(), &Provenance::anonymous(Actor::Human))
            .unwrap()
            .entity
            .id()
            .clone();
        (store, id, dir)
    }

    fn write_at(dir: &tempfile::TempDir, rel: &str, body: &str) -> PathBuf {
        let p = dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn moving_a_document_to_another_path_is_reported_before_it_happens() {
        let (mut store, project, dir) = rooted();
        let first = write_at(&dir, "docs/spec.md", "# Storage\n\nOne file.\n");
        file(&mut store, &first, &project, EntityType::Spec, None, None).unwrap();

        // Same title, so it resolves to the same artifact — and a different
        // path, so importing would move where `specline generate` writes it.
        let second = write_at(&dir, "product/SPEC.md", "# Storage\n\nOne file, revised.\n");
        let p = preview(&store, &second, &project, EntityType::Spec, None).unwrap();

        assert_eq!(p.mirror_path.as_deref(), Some("product/SPEC.md"));
        assert_eq!(
            p.mirror_path_now.as_deref(),
            Some("docs/spec.md"),
            "the preview has to name the path being given up, or the old file \
             goes stale with nothing said"
        );
    }

    #[test]
    fn re_importing_from_the_same_path_reports_no_move() {
        let (mut store, project, dir) = rooted();
        let f = write_at(&dir, "docs/spec.md", "# Storage\n\nOne file.\n");
        file(&mut store, &f, &project, EntityType::Spec, None, None).unwrap();

        std::fs::write(&f, "# Storage\n\nOne file, edited in place.\n").unwrap();
        let p = preview(&store, &f, &project, EntityType::Spec, None).unwrap();

        assert_eq!(p.mirror_path.as_deref(), Some("docs/spec.md"));
        assert_eq!(
            p.mirror_path_now, None,
            "an unchanged path must stay quiet — a warning that fires every \
             time is one people stop reading"
        );
    }

    #[test]
    fn a_document_that_has_never_claimed_a_path_reports_no_move() {
        let (store, project, dir) = rooted();
        let f = write_at(
            &dir,
            "docs/new.md",
            "# Brand new\n\nNothing owns this yet.\n",
        );

        let p = preview(&store, &f, &project, EntityType::Spec, None).unwrap();
        assert_eq!(p.outcome, Outcome::Create);
        assert_eq!(p.mirror_path.as_deref(), Some("docs/new.md"));
        assert_eq!(p.mirror_path_now, None, "nothing is being given up");
    }
}
