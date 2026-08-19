//! Document revisions, diffs, blobs and hybrid search, against real storage.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::Utc;
use specline_core::*;
use std::sync::Arc;

struct Fixture {
    store: Store,
    project_id: EntityId,
    _dir: tempfile::TempDir,
}

fn prov() -> Provenance {
    Provenance::anonymous(Actor::Claude).with_session("ses_docs")
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        // A hash embedder rather than the real model: the test suite must not
        // download 130 MB before it can assert anything. This exercises the
        // plumbing, not retrieval quality — see embed.rs.
        let mut store = Store::open(dir.path().join("specline.sqlite"))
            .unwrap()
            .with_embedder(Arc::new(HashEmbedder::new()));
        let project_id = store
            .create(Project::new("specline", "Specline").into(), &prov())
            .unwrap()
            .entity
            .id()
            .clone();
        Fixture {
            store,
            project_id,
            _dir: dir,
        }
    }

    fn spec(&mut self, title: &str) -> EntityId {
        self.store
            .create(Spec::new(self.project_id.clone(), title).into(), &prov())
            .unwrap()
            .entity
            .id()
            .clone()
    }

    fn write(&mut self, id: &EntityId, title: &str, body: &str) -> Document {
        let doc = Document::first(
            id.entity_type(),
            id.clone(),
            Some(self.project_id.clone()),
            title,
            body,
            Actor::Claude,
            Utc::now(),
        )
        .unwrap();
        self.store.write_revision(doc).unwrap()
    }
}

#[test]
fn a_first_revision_is_version_one_and_advances_the_header() {
    let mut f = Fixture::new();
    let id = f.spec("Storage specification");

    let doc = f.write(
        &id,
        "Storage specification",
        "DuckDB for rows, Lance for prose.",
    );
    assert_eq!(doc.version, 1);
    assert_eq!(doc.parent_version, None);
    assert_eq!(doc.status, DocStatus::Current);

    // The relational half must agree with the columnar half.
    let header = f.store.get(&id).unwrap().unwrap();
    assert_eq!(header.current_doc_version(), Some(1));
}

#[test]
fn revisions_accumulate_and_only_one_is_current() {
    let mut f = Fixture::new();
    let id = f.spec("Storage specification");

    f.write(&id, "Storage specification", "First draft.");
    f.write(&id, "Storage specification", "Second draft, with detail.");
    let third = f.write(&id, "Storage specification", "Third draft, approved.");

    assert_eq!(third.version, 3);
    assert_eq!(third.parent_version, Some(2));

    let history = f.store.revisions(&id).unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(
        history.iter().map(|d| d.version).collect::<Vec<_>>(),
        vec![1, 2, 3],
        "history must come back oldest-first"
    );

    let current: Vec<_> = history
        .iter()
        .filter(|d| d.status == DocStatus::Current)
        .collect();
    assert_eq!(current.len(), 1, "exactly one revision may be current");
    assert_eq!(current[0].version, 3);

    assert_eq!(
        f.store.get(&id).unwrap().unwrap().current_doc_version(),
        Some(3)
    );
}

#[test]
fn an_older_revision_can_be_fetched_by_version() {
    let mut f = Fixture::new();
    let id = f.spec("Storage specification");
    f.write(&id, "Storage specification", "First draft.");
    f.write(&id, "Storage specification", "Second draft.");

    let v1 = f.store.revision(&id, Some(1)).unwrap().unwrap();
    assert_eq!(v1.body, "First draft.");
    assert_eq!(v1.status, DocStatus::Superseded);

    let current = f.store.revision(&id, None).unwrap().unwrap();
    assert_eq!(current.body, "Second draft.");
}

#[test]
fn rewriting_identical_content_does_not_grow_the_history() {
    // The §8.1 mirror hook regenerates a file and re-reads it. Without this,
    // every no-op save would add a revision.
    let mut f = Fixture::new();
    let id = f.spec("Storage specification");

    let first = f.write(&id, "Storage specification", "Unchanged body.");
    let second = f.write(&id, "Storage specification", "Unchanged body.");

    assert_eq!(first.version, second.version);
    assert_eq!(f.store.revisions(&id).unwrap().len(), 1);
}

#[test]
fn a_changed_title_alone_is_a_real_revision() {
    let mut f = Fixture::new();
    let id = f.spec("Storage specification");
    f.write(&id, "Storage specification", "Same body.");
    let renamed = f.write(&id, "Storage design", "Same body.");
    assert_eq!(renamed.version, 2, "the title is part of the content hash");
}

#[test]
fn a_revision_for_a_nonexistent_entity_is_refused() {
    // The cross-engine foreign key nothing can declare.
    let mut f = Fixture::new();
    let ghost = EntityId::generate(EntityType::Spec);
    let doc = Document::first(
        EntityType::Spec,
        ghost,
        None,
        "Ghost",
        "body",
        Actor::Claude,
        Utc::now(),
    )
    .unwrap();
    let err = f.store.write_revision(doc).unwrap_err();
    assert!(
        err.to_string()
            .contains("create the entity before writing its body"),
        "{err}"
    );
}

#[test]
fn provenance_survives_onto_the_revision() {
    let mut f = Fixture::new();
    let id = f.spec("Storage specification");
    let doc = Document::first(
        EntityType::Spec,
        id.clone(),
        Some(f.project_id.clone()),
        "Storage specification",
        "body",
        Actor::Human,
        Utc::now(),
    )
    .unwrap()
    .attributed(Some("ses_abc".into()), Some(Surface::Chat));

    f.store.write_revision(doc).unwrap();
    let stored = f.store.revision(&id, None).unwrap().unwrap();
    assert_eq!(stored.author, Actor::Human);
    assert_eq!(stored.session_id.as_deref(), Some("ses_abc"));
    assert_eq!(stored.surface, Some(Surface::Chat));
}

#[test]
fn two_revisions_diff() {
    let mut f = Fixture::new();
    let id = f.spec("Storage specification");
    f.write(&id, "S", "line one\nline two\nline three\n");
    f.write(
        &id,
        "S",
        "line one\nline two changed\nline three\nline four\n",
    );

    let diff = f.store.diff(&id, 1, 2).unwrap();
    assert_eq!(diff.from_version, 1);
    assert_eq!(diff.to_version, 2);
    assert_eq!(diff.removed, 1);
    assert_eq!(diff.added, 2);
    assert!(diff.unified.contains("-line two"), "{}", diff.unified);
    assert!(
        diff.unified.contains("+line two changed"),
        "{}",
        diff.unified
    );
}

#[test]
fn diffing_a_missing_revision_says_which_one() {
    let mut f = Fixture::new();
    let id = f.spec("Storage specification");
    f.write(&id, "S", "only one revision");

    let err = f.store.diff(&id, 1, 7).unwrap_err().to_string();
    assert!(err.contains("no revision 7"), "{err}");
}

#[test]
fn embeddings_are_written_when_an_embedder_is_attached() {
    let mut f = Fixture::new();
    let id = f.spec("Storage specification");
    f.write(&id, "Storage", "DuckDB and Lance together.");

    // Passages, not a column on the document. Since B-55 a revision's vectors
    // live one per passage; `documents.embedding` is no longer written and
    // asserting on it would pass only while nothing had changed.
    let passages: i64 = f
        .store
        .connection()
        .query_row("SELECT count(*) FROM document_chunks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(passages, 1, "short prose is one passage");

    let model: String = f
        .store
        .connection()
        .query_row("SELECT embedding_model FROM document_chunks", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        model, "test-hash-embedder",
        "the model must travel with the row"
    );
}

#[test]
fn a_store_without_an_embedder_still_stores_and_searches() {
    // G8 and R-3: no embedder must degrade search, not break the store.
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("specline.sqlite")).unwrap();
    let project_id = store
        .create(Project::new("specline", "Specline").into(), &prov())
        .unwrap()
        .entity
        .id()
        .clone();
    let spec = store
        .create(
            Spec::new(project_id.clone(), "Onboarding spec").into(),
            &prov(),
        )
        .unwrap()
        .entity
        .id()
        .clone();

    store
        .write_revision(
            Document::first(
                EntityType::Spec,
                spec.clone(),
                Some(project_id),
                "Onboarding spec",
                "The onboarding flow needs to be shorter.",
                Actor::Claude,
                Utc::now(),
            )
            .unwrap(),
        )
        .unwrap();

    let hits = store.search(&SearchQuery::new("onboarding")).unwrap();
    assert!(
        !hits.items.is_empty(),
        "keyword search must work with no embedder"
    );
}

#[test]
fn search_spans_prose_and_non_prose_types_alike() {
    let mut f = Fixture::new();

    // A prose-bearing type — indexed from its current document revision.
    let spec = f.spec("Onboarding redesign");
    f.write(
        &spec,
        "Onboarding redesign",
        "Customers report that onboarding takes too many steps.",
    );

    // A non-prose type — indexed from its own row.
    let task = f
        .store
        .create(
            Task::new(
                f.project_id.clone(),
                "Shorten the onboarding flow",
                "A row this test needs in the store.",
            )
            .into(),
            &prov(),
        )
        .unwrap()
        .entity
        .id()
        .clone();

    let results = f.store.search(&SearchQuery::new("onboarding")).unwrap();
    let ids: Vec<&EntityId> = results.items.iter().map(|h| &h.entity_id).collect();

    assert!(ids.contains(&&spec), "the spec should be found: {ids:?}");
    assert!(ids.contains(&&task), "the task should be found: {ids:?}");

    // Both come back through the keyword index, which is the point: one BM25
    // index covers the whole corpus, prose included, so a spec and a task
    // compete on the same footing rather than in separate result sets.
    for id in [&spec, &task] {
        let hit = results.items.iter().find(|h| &h.entity_id == id).unwrap();
        assert!(
            matches!(hit.source, SearchSource::Keyword | SearchSource::Both),
            "{id} came from {:?}",
            hit.source
        );
    }
}

#[test]
fn the_semantic_half_finds_prose_that_shares_no_words_with_the_query() {
    // The reason the vector index is here at all. If search only ever matched
    // keywords, the embeddings would be dead weight.
    let mut f = Fixture::new();
    let spec = f.spec("Aggregation granularity");
    f.write(
        &spec,
        "Aggregation granularity",
        "hourly buckets metering aggregate storage cost sixty",
    );

    let hits = f
        .store
        .search(&SearchQuery::new("hourly buckets metering"))
        .unwrap();
    assert!(!hits.items.is_empty());
    let hit = hits.items.iter().find(|h| h.entity_id == spec).unwrap();
    assert!(
        matches!(hit.source, SearchSource::Semantic | SearchSource::Both),
        "the vector index should have contributed, got {:?}",
        hit.source
    );
}

#[test]
fn newly_created_entities_are_searchable_immediately() {
    // The stale-index trap, which the engine this store replaced walked into:
    // its full-text index was a snapshot and did not track inserts, so an
    // entity created after the last build was silently never found. FTS5's
    // triggers are what make this pass without a rebuild — assert it, because
    // a regression here fails by finding nothing rather than by erroring.
    let mut f = Fixture::new();
    f.store
        .create(
            Task::new(
                f.project_id.clone(),
                "Investigate flaky deploys",
                "A row this test needs in the store.",
            )
            .into(),
            &prov(),
        )
        .unwrap();
    let first = f.store.search(&SearchQuery::new("flaky")).unwrap();
    assert_eq!(first.items.len(), 1);

    // Create another and search again without any explicit reindex.
    f.store
        .create(
            Task::new(
                f.project_id.clone(),
                "Fix flaky integration tests",
                "A row this test needs in the store.",
            )
            .into(),
            &prov(),
        )
        .unwrap();
    let second = f.store.search(&SearchQuery::new("flaky")).unwrap();
    assert_eq!(
        second.items.len(),
        2,
        "an entity created since the last index build must still be findable"
    );
}

#[test]
fn archived_entities_drop_out_of_search() {
    let mut f = Fixture::new();
    let task = f
        .store
        .create(
            Task::new(
                f.project_id.clone(),
                "Retire the legacy importer",
                "A row this test needs in the store.",
            )
            .into(),
            &prov(),
        )
        .unwrap()
        .entity
        .id()
        .clone();
    assert_eq!(
        f.store
            .search(&SearchQuery::new("importer"))
            .unwrap()
            .items
            .len(),
        1
    );

    f.store.archive(&task, 1, &prov()).unwrap();
    assert_eq!(
        f.store
            .search(&SearchQuery::new("importer"))
            .unwrap()
            .items
            .len(),
        0,
        "archived work should not surface in search"
    );
}

#[test]
fn search_can_be_filtered_by_project_and_type() {
    let mut f = Fixture::new();
    let other = f
        .store
        .create(Project::new("other", "Other").into(), &prov())
        .unwrap()
        .entity
        .id()
        .clone();

    f.store
        .create(
            Task::new(
                f.project_id.clone(),
                "Shared word alpha",
                "A row this test needs in the store.",
            )
            .into(),
            &prov(),
        )
        .unwrap();
    f.store
        .create(
            Task::new(
                other.clone(),
                "Shared word alpha",
                "A row this test needs in the store.",
            )
            .into(),
            &prov(),
        )
        .unwrap();

    let all = f.store.search(&SearchQuery::new("alpha")).unwrap();
    assert_eq!(all.items.len(), 2);

    let scoped = f
        .store
        .search(&SearchQuery {
            project_id: Some(other.clone()),
            ..SearchQuery::new("alpha")
        })
        .unwrap();
    assert_eq!(scoped.items.len(), 1);
    assert_eq!(scoped.items[0].project_id.as_ref(), Some(&other));

    let typed = f
        .store
        .search(&SearchQuery {
            entity_types: vec![EntityType::Spec],
            ..SearchQuery::new("alpha")
        })
        .unwrap();
    assert!(
        typed.items.is_empty(),
        "tasks must not match a spec-only filter"
    );
}

#[test]
fn an_empty_search_says_what_to_use_instead() {
    let f = Fixture::new();
    let err = f
        .store
        .search(&SearchQuery::new("   "))
        .unwrap_err()
        .to_string();
    assert!(err.contains("specline_context"), "{err}");
}

#[test]
fn blobs_round_trip() {
    let mut f = Fixture::new();
    let bytes = b"\x89PNG\r\n\x1a\n fake image bytes".to_vec();
    let blob = Blob {
        blob_id: BlobId::generate(),
        entity_id: None,
        project_id: Some(f.project_id.clone()),
        media_type: "image/png".into(),
        sha256: "deadbeef".into(),
        bytes: bytes.clone(),
        created_at: Utc::now(),
    };
    let id = f.store.put_blob(blob).unwrap();

    let fetched = f.store.get_blob(&id).unwrap().unwrap();
    assert_eq!(
        fetched.bytes, bytes,
        "blob bytes must survive the round trip intact"
    );
    assert_eq!(fetched.media_type, "image/png");
    assert!(f.store.get_blob(&BlobId::generate()).unwrap().is_none());
}

#[test]
fn a_full_size_specification_round_trips_byte_for_byte() {
    // Specline's own SPEC.md is 51 KB. If a document that size could not go in,
    // come back unchanged, stay searchable and still diff, then "read your
    // specs in the app" would not be a real offer.
    let mut f = Fixture::new();
    let id = f.spec("Specline — Technical Specification");

    // Built rather than read from disk, so the test does not depend on a path
    // outside the crate — but shaped like the real thing: headings, prose,
    // tables, fenced code.
    let mut body = String::from("# Technical Specification\n\n");
    for section in 1..=150 {
        body.push_str(&format!(
            "## {section}. Section {section}\n\n             Prose explaining why section {section} is the way it is, at enough length \
             that the whole document reaches the size of a real specification rather \
             than a toy one.\n\n             | Column | Meaning |\n|---|---|\n| a | the first |\n| b | the second |\n\n             ```sql\nSELECT {section} AS n FROM generate_series(1, 10);\n```\n\n"
        ));
    }
    body.push_str("## Buried\n\nThe phrase reciprocal rank fusion appears only here.\n");
    assert!(body.len() > 20_000, "not a realistic size: {}", body.len());

    let doc = Document::first(
        EntityType::Spec,
        id.clone(),
        Some(f.project_id.clone()),
        "Specline — Technical Specification",
        &body,
        Actor::Claude,
        Utc::now(),
    )
    .unwrap();
    f.store.write_revision(doc).unwrap();

    let back = f.store.revision(&id, None).unwrap().unwrap();
    assert_eq!(back.body, body, "the body must come back byte-identical");

    // A phrase near the end must still be findable — a document that is stored
    // but not indexed is a document nobody will find.
    let hits = f
        .store
        .search(&SearchQuery::new("reciprocal rank fusion"))
        .unwrap();
    assert!(
        hits.items.iter().any(|h| h.entity_id == id),
        "a phrase from deep inside a large document must be searchable"
    );

    // And a one-line edit produces a one-line diff, not a whole-file rewrite.
    let edited = body.replace("## 1. Section 1", "## 1. Section 1 (revised)");
    f.store
        .write_revision(
            Document::first(
                EntityType::Spec,
                id.clone(),
                Some(f.project_id.clone()),
                "Specline — Technical Specification",
                &edited,
                Actor::Human,
                Utc::now(),
            )
            .unwrap(),
        )
        .unwrap();
    let diff = f.store.diff(&id, 1, 2).unwrap();
    assert_eq!((diff.added, diff.removed), (1, 1), "{}", diff.unified);
}

#[test]
fn a_document_survives_reopening_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let (project_id, spec_id) = {
        let mut store = Store::open(dir.path().join("specline.sqlite")).unwrap();
        let p = store
            .create(Project::new("specline", "Specline").into(), &prov())
            .unwrap()
            .entity
            .id()
            .clone();
        let s = store
            .create(Spec::new(p.clone(), "Persisted spec").into(), &prov())
            .unwrap()
            .entity
            .id()
            .clone();
        store
            .write_revision(
                Document::first(
                    EntityType::Spec,
                    s.clone(),
                    Some(p.clone()),
                    "Persisted spec",
                    "This must survive a restart.",
                    Actor::Claude,
                    Utc::now(),
                )
                .unwrap(),
            )
            .unwrap();
        (p, s)
    };

    let store = Store::open(dir.path().join("specline.sqlite")).unwrap();
    let doc = store.revision(&spec_id, None).unwrap().unwrap();
    assert_eq!(doc.body, "This must survive a restart.");
    assert_eq!(doc.project_id, Some(project_id));
}

// --- Backfilling the vectors that were never written ---------------------

/// The pass that makes semantic search real on a corpus that predates it.
///
/// Embedding happens on the way into a new revision and nowhere else, so a
/// store that had the feature turned on late kept every existing document
/// invisible to the vector half — permanently, because nothing would ever
/// rewrite those rows.
#[test]
fn reembed_gives_every_vectorless_revision_a_vector() {
    use specline_core::Embedder;

    // Written with no embedder attached, which is exactly how the live store
    // came to hold 227 documents with null embeddings.
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("specline.sqlite")).unwrap();
    let project = store
        .create(Project::new("demo", "Demo").into(), &prov())
        .unwrap()
        .entity
        .id()
        .clone();

    for title in ["First spec", "Second spec", "Third spec"] {
        store
            .create_with_document(
                Spec::new(project.clone(), title).into(),
                Some(format!("The body of {title}, with words in it.")),
                None,
                &prov(),
            )
            .unwrap();
    }

    let (current, missing) = store.documents_missing_embeddings(None).unwrap();
    assert_eq!(current, 3);
    assert_eq!(missing, 3, "no embedder was attached, so none has a vector");

    let embedder = HashEmbedder::new();
    let mut steps = Vec::new();
    let report = store
        .reembed_missing(&embedder, None, |done, total| steps.push((done, total)))
        .unwrap();

    assert_eq!(report.missing, 3);
    assert_eq!(report.embedded, 3);
    assert_eq!(report.failed, 0);
    assert!(!steps.is_empty(), "a slow pass has to report progress");

    let (_, still_missing) = store.documents_missing_embeddings(None).unwrap();
    assert_eq!(still_missing, 0);

    // The stored width has to match what the schema and the query expect, or
    // `vec_distance_cosine` errors on every comparison.
    let width: i64 = store
        .connection()
        .query_row(
            "SELECT length(embedding) FROM document_chunks LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(width as usize, embedder.dimensions() * 4);

    // And the model is recorded, which is what a later model change reads to
    // find stale rows.
    let model: String = store
        .connection()
        .query_row(
            "SELECT embedding_model FROM document_chunks LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(model, embedder.model_name());
}

/// Running it twice must not rewrite what it already did.
#[test]
fn a_second_reembed_pass_has_nothing_to_do() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("specline.sqlite")).unwrap();
    let project = store
        .create(Project::new("demo", "Demo").into(), &prov())
        .unwrap()
        .entity
        .id()
        .clone();
    store
        .create_with_document(
            Spec::new(project, "A spec").into(),
            Some("Some prose.".to_owned()),
            None,
            &prov(),
        )
        .unwrap();

    let embedder = HashEmbedder::new();
    store.reembed_missing(&embedder, None, |_, _| {}).unwrap();
    let again = store.reembed_missing(&embedder, None, |_, _| {}).unwrap();

    assert_eq!(again.missing, 0);
    assert_eq!(again.embedded, 0);
}

/// The backfill must not undo an archive.
///
/// Archiving clears the vector, which leaves the row indistinguishable from a
/// document that was never embedded at all — so a pass that looked only for
/// `embedding IS NULL` would put back, every single time it ran, exactly what
/// archiving had just taken away. The two mechanisms would have fought each
/// other quietly and the archive would have lost.
#[test]
fn reembed_leaves_archived_documents_alone() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("specline.sqlite")).unwrap();
    let project = store
        .create(Project::new("demo", "Demo").into(), &prov())
        .unwrap()
        .entity
        .id()
        .clone();

    let live = store
        .create_with_document(
            Spec::new(project.clone(), "A live spec").into(),
            Some("Prose that should be findable.".to_owned()),
            None,
            &prov(),
        )
        .unwrap();
    let put_away = store
        .create_with_document(
            Spec::new(project, "A spec someone put away").into(),
            Some("Prose that should not be.".to_owned()),
            None,
            &prov(),
        )
        .unwrap();

    let doomed = put_away.entity.id().clone();
    store
        .archive(&doomed, put_away.entity.audit().version, &prov())
        .unwrap();

    // Both counts see one document, not two: an archived row that can never be
    // embedded must not sit in the denominator making the check permanently red.
    let (current, missing) = store.documents_missing_embeddings(None).unwrap();
    assert_eq!(current, 1, "the archived spec must not be counted");
    assert_eq!(missing, 1);

    let embedder = HashEmbedder::new();
    let report = store.reembed_missing(&embedder, None, |_, _| {}).unwrap();
    assert_eq!(report.missing, 1, "only the live spec was pending");
    assert_eq!(report.embedded, 1);

    let vectors = |id: &EntityId| -> i64 {
        store
            .connection()
            .query_row(
                "SELECT count(*) FROM document_chunks WHERE entity_id = ?1",
                [id.as_str()],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert_eq!(
        vectors(&doomed),
        0,
        "the backfill must not give an archived document a vector back"
    );
    assert_eq!(vectors(live.entity.id()), 1, "the live spec still gets one");
}

/// The exit criterion for KEEL-174: something written in the middle of a long
/// document is findable.
///
/// Before B-55 a document went to the model whole and the model stopped at 512
/// tokens, so on the live store the technical specification had its first 2.5%
/// embedded and the rest was invisible to the semantic half — silently, because
/// the keyword half kept answering.
///
/// The needle here sits about 30,000 characters in, far past any truncation
/// point, and shares no words with the surrounding filler.
#[test]
fn a_phrase_buried_deep_in_a_long_document_is_findable() {
    let mut f = Fixture::new();
    let id = f.spec("A very long specification");

    let filler = "Routine prose about scheduling and throughput. ".repeat(650);
    let body = format!(
        "# Opening\n\n{filler}\n\n## Buried\n\nThe wombat marsupial burrows nocturnally.\n\n\
         ## Closing\n\n{filler}"
    );
    assert!(
        body.len() > 30_000,
        "the needle must be past any truncation"
    );
    f.write(&id, "A very long specification", &body);

    let hits = f
        .store
        .search(&SearchQuery::new("wombat marsupial burrows"))
        .unwrap();
    let found: Vec<&str> = hits.items.iter().map(|h| h.entity_id.as_str()).collect();
    assert!(
        found.contains(&id.as_str()),
        "the buried phrase did not surface: {found:?}"
    );

    // Being *found* proves little on its own: the needle's words are in the
    // body, so BM25 would have found it before any of this existed. What the
    // chunking has to earn is the semantic half finding it too — before B-55
    // that text was past the truncation point and had no vector at all, so the
    // only possible source was `Keyword`.
    let hit = hits
        .items
        .iter()
        .find(|h| h.entity_id == id)
        .expect("the hit is there");
    assert_eq!(
        hit.source,
        SearchSource::Both,
        "the semantic half did not reach the buried passage, so nothing was gained"
    );

    // And the passage that matched is the buried one, not the opening — which
    // is what makes the excerpt worth reading.
    assert!(
        hit.excerpt.contains("wombat") || hit.excerpt.contains("Buried"),
        "the excerpt should come from the passage that matched: {}",
        hit.excerpt
    );

    // The mechanism behind it, asserted directly: some passage begins past the
    // point the whole-document embed used to stop at.
    let deepest: i64 = f
        .store
        .connection()
        .query_row(
            "SELECT max(char_start) FROM document_chunks WHERE entity_id = ?1",
            [id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        deepest > 1_700,
        "every passage started inside the old truncation window ({deepest})"
    );
}

/// The invariant the delete carve-out to hard constraint 3 rests on.
///
/// Passages are `DELETE`d rather than archived, which is legal only because
/// nothing is lost: the revision in `documents` is immutable and a passage can
/// always be rebuilt from it. If this ever fails, the carve-out is unsound and
/// passages have to start being soft-deleted like everything else.
#[test]
fn a_passage_can_always_be_rebuilt_from_its_revision() {
    let mut f = Fixture::new();
    let id = f.spec("Rebuildable");
    let body = "# One\n\nAlpha beta gamma.\n\n## Two\n\nDelta epsilon zeta.\n";
    f.write(&id, "Rebuildable", body);

    let before: Vec<(i64, String, Vec<u8>)> = passages(&f.store, &id);
    assert!(before.len() >= 2, "expected several passages");

    // Wipe the derived table entirely, the way a corrupted index or a model
    // change would.
    f.store
        .connection()
        .execute("DELETE FROM document_chunks", [])
        .unwrap();
    assert!(passages(&f.store, &id).is_empty());

    let embedder = HashEmbedder::new();
    let report = f.store.reembed_missing(&embedder, None, |_, _| {}).unwrap();
    assert_eq!(report.embedded, 1, "the revision should have been rebuilt");

    let after = passages(&f.store, &id);
    assert_eq!(
        before, after,
        "a rebuilt passage must be byte-identical to the original"
    );
}

/// One document must not fill a page of results with its own passages.
#[test]
fn a_long_document_contributes_one_hit_not_one_per_passage() {
    let mut f = Fixture::new();
    let id = f.spec("Long");
    let body = "Storage and retrieval and indexing. ".repeat(400);
    f.write(&id, "Long", &body);

    let chunk_count: i64 = f
        .store
        .connection()
        .query_row("SELECT count(*) FROM document_chunks", [], |r| r.get(0))
        .unwrap();
    assert!(
        chunk_count > 3,
        "expected several passages, got {chunk_count}"
    );

    let hits = f
        .store
        .search(&SearchQuery::new("storage indexing"))
        .unwrap();
    let mine = hits.items.iter().filter(|h| h.entity_id == id).count();
    assert_eq!(mine, 1, "one document, one hit");
}

/// Editing a document must take its old passages with it.
#[test]
fn rewriting_a_document_replaces_its_passages() {
    let mut f = Fixture::new();
    let id = f.spec("Changing");
    f.write(&id, "Changing", "The original text mentions penguins.");
    let first: Vec<String> = passages(&f.store, &id).into_iter().map(|p| p.1).collect();
    assert!(first.iter().any(|t| t.contains("penguins")));

    f.write(
        &id,
        "Changing",
        "The replacement text mentions albatrosses.",
    );
    let second: Vec<String> = passages(&f.store, &id).into_iter().map(|p| p.1).collect();
    assert!(
        second.iter().all(|t| !t.contains("penguins")),
        "the old passages outlived the revision they described: {second:?}"
    );
    assert!(second.iter().any(|t| t.contains("albatrosses")));
}

/// Passages of an entity, ordered, as (ordinal, text, vector).
fn passages(store: &Store, id: &EntityId) -> Vec<(i64, String, Vec<u8>)> {
    let conn = store.connection();
    let mut stmt = conn
        .prepare(
            "SELECT ordinal, text, embedding FROM document_chunks \
             WHERE entity_id = ?1 ORDER BY ordinal",
        )
        .unwrap();
    let rows = stmt
        .query_map([id.as_str()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap();
    rows.map(|r| r.unwrap()).collect()
}

/// TQ-3, answered: changing the model makes every document missing again, and
/// the ordinary `--missing` pass rebuilds them.
///
/// Before B-59 "missing" meant "has no passages at all", so a model change left
/// a corpus that looked complete and was half in one vector space and half in
/// another. Nothing would ever rebuild it, and nothing said so.
#[test]
fn changing_the_model_makes_every_document_missing_again() {
    let mut f = Fixture::new();
    let id = f.spec("Storage");
    f.write(
        &id,
        "Storage",
        "One SQLite file holds the rows and the prose.\n",
    );

    let first = HashEmbedder::new();
    let (current, missing) = f
        .store
        .documents_missing_embeddings(Some(first.model_name()))
        .unwrap();
    assert_eq!(current, 1);
    assert_eq!(missing, 0, "the write path already embedded it");

    // A different model, same dimensions — the case the width guard cannot see.
    let renamed = HashEmbedder::new().named("a-different-model");
    let (_, missing_now) = f
        .store
        .documents_missing_embeddings(Some(renamed.model_name()))
        .unwrap();
    assert_eq!(
        missing_now, 1,
        "under the new model the document has no passages and must be rebuilt"
    );

    let report = f.store.reembed_missing(&renamed, None, |_, _| {}).unwrap();
    assert_eq!(report.embedded, 1);

    // Rebuilt, not accumulated: the old model's passages are gone rather than
    // sitting alongside the new ones in a different vector space.
    let models: Vec<String> = {
        let conn = f.store.connection();
        let mut stmt = conn
            .prepare("SELECT DISTINCT embedding_model FROM document_chunks")
            .unwrap();
        let rows = stmt.query_map([], |r| r.get(0)).unwrap();
        rows.map(|r| r.unwrap()).collect()
    };
    assert_eq!(models, vec!["a-different-model".to_owned()]);

    let (_, still) = f
        .store
        .documents_missing_embeddings(Some(renamed.model_name()))
        .unwrap();
    assert_eq!(still, 0);
}
