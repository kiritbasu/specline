//! Notes are in the keyword index (KEEL-339).
//!
//! Sessions are told to record what they found as a note on the row rather
//! than as prose, and until this nothing ever put a note into `fts_source` — a
//! search for an exact sentence from a note written an hour earlier returned
//! three unrelated decisions. These tests hold the fix to what it has to be:
//!
//! - a note's text finds the row it annotates, and says which note matched;
//! - a retracted note, or a note on an archived row, stops matching;
//! - a project-scoped search does not reach into another project's notes;
//! - notes written before the index existed are backfilled by the migration,
//!   and what the backfill writes is byte-for-byte what the trigger writes —
//!   the property the derived-index carve-out in hard constraint 3 rests on.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use specline_core::{
    Actor, EntityId, EntityStore, EntityType, HalfStatus, NewNote, Note, Project, Provenance,
    SearchQuery, Spec, Store, Task,
};

fn prov() -> Provenance {
    Provenance::anonymous(Actor::Claude)
}

fn project(store: &mut Store, slug: &str) -> EntityId {
    store
        .create(Project::new(slug, slug).into(), &prov())
        .unwrap()
        .entity
        .id()
        .clone()
}

fn task(store: &mut Store, project: &EntityId, title: &str) -> EntityId {
    store
        .create(
            Task::new(
                project.clone(),
                title,
                "A row this test needs in the store.",
            )
            .into(),
            &prov(),
        )
        .unwrap()
        .entity
        .id()
        .clone()
}

fn note(store: &mut Store, on: &EntityId, body: &str) -> Note {
    store
        .add_note(NewNote::new(on.clone(), body, Actor::Claude), &prov())
        .unwrap()
}

fn search(store: &Store, text: &str) -> Vec<specline_core::SearchHit> {
    store.search(&SearchQuery::new(text)).unwrap().items
}

fn file_store() -> (tempfile::TempDir, std::path::PathBuf, Store) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("specline.sqlite");
    let store = Store::open(&path).unwrap();
    (dir, path, store)
}

#[test]
fn a_note_is_found_by_search_and_resolves_to_the_row_it_annotates() {
    let (_d, _p, mut store) = file_store();
    let prj = project(&mut store, "specline");
    let tsk = task(&mut store, &prj, "The board is slow");
    let written = note(
        &mut store,
        &tsk,
        "STRICT rejects the wrong type but not the wrong value, so a quokka stores fine",
    );

    let hits = search(&store, "quokka");
    assert_eq!(hits.len(), 1, "exactly the annotated row: {hits:?}");
    let hit = &hits[0];
    assert_eq!(hit.entity_id, tsk, "the hit names the row, not the note");
    assert_eq!(hit.entity_type, EntityType::Task);
    assert_eq!(hit.project_id.as_ref(), Some(&prj));
    assert_eq!(
        hit.title, "The board is slow",
        "the title is the row's, since that is what a reader opens"
    );
    assert_eq!(
        hit.note_id.as_ref(),
        Some(&written.id),
        "and it says which note matched"
    );
    assert!(
        hit.excerpt.contains("quokka"),
        "the excerpt is cut from the note: {}",
        hit.excerpt
    );

    // The wire shape of a hit that did *not* come from a note is unchanged.
    let plain = search(&store, "board");
    assert_eq!(plain.len(), 1);
    assert!(plain[0].note_id.is_none());
    let json = serde_json::to_value(&plain[0]).unwrap();
    assert!(
        json.get("note_id").is_none(),
        "a row hit must not grow a null field: {json}"
    );
}

#[test]
fn a_retracted_note_stops_matching() {
    let (_d, _p, mut store) = file_store();
    let prj = project(&mut store, "specline");
    let tsk = task(&mut store, &prj, "Anything");
    let kept = note(&mut store, &tsk, "the wombat finding still stands");
    let wrong = note(&mut store, &tsk, "the platypus finding was wrong");

    assert_eq!(search(&store, "platypus").len(), 1);
    store.retract_note(&wrong.id, &prov()).unwrap();

    assert!(
        search(&store, "platypus").is_empty(),
        "a retracted note must leave the index"
    );
    let still = search(&store, "wombat");
    assert_eq!(still.len(), 1, "retracting one note leaves its siblings");
    assert_eq!(still[0].note_id.as_ref(), Some(&kept.id));
}

#[test]
fn archiving_the_row_takes_its_notes_out_of_the_index() {
    let (_d, _p, mut store) = file_store();
    let prj = project(&mut store, "specline");
    let tsk = task(&mut store, &prj, "Put away");
    note(&mut store, &tsk, "an echidna was here");
    assert_eq!(search(&store, "echidna").len(), 1);

    store.archive(&tsk, 1, &prov()).unwrap();

    assert!(
        search(&store, "echidna").is_empty(),
        "a note must not resurrect a row that was archived"
    );
}

#[test]
fn a_project_scoped_search_does_not_reach_another_projects_notes() {
    let (_d, _p, mut store) = file_store();
    let mine = project(&mut store, "mine");
    let theirs = project(&mut store, "theirs");
    let t_mine = task(&mut store, &mine, "Mine");
    let t_theirs = task(&mut store, &theirs, "Theirs");
    note(&mut store, &t_theirs, "the numbat lives over there");

    let mut scoped = SearchQuery::new("numbat");
    scoped.project_id = Some(mine.clone());
    assert!(
        store.search(&scoped).unwrap().items.is_empty(),
        "another project's note leaked into a scoped search"
    );

    scoped.project_id = Some(theirs.clone());
    let hits = store.search(&scoped).unwrap().items;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entity_id, t_theirs);
    assert_ne!(hits[0].entity_id, t_mine);
}

#[test]
fn a_type_filter_applies_to_the_row_a_note_annotates() {
    let (_d, _p, mut store) = file_store();
    let prj = project(&mut store, "specline");
    let tsk = task(&mut store, &prj, "Filtered");
    note(&mut store, &tsk, "a bilby in the notes");

    let mut q = SearchQuery::new("bilby");
    q.entity_types = vec![EntityType::Spec];
    assert!(
        store.search(&q).unwrap().items.is_empty(),
        "a note on a task is not a spec"
    );

    q.entity_types = vec![EntityType::Task];
    let hits = store.search(&q).unwrap().items;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entity_id, tsk);
}

#[test]
fn a_row_matching_in_its_title_and_several_notes_is_one_hit() {
    let (_d, _p, mut store) = file_store();
    let prj = project(&mut store, "specline");
    let tsk = task(&mut store, &prj, "The dingo problem");
    note(&mut store, &tsk, "first dingo finding");
    note(&mut store, &tsk, "second dingo finding");

    let page = store.search(&SearchQuery::new("dingo")).unwrap();
    assert_eq!(page.items.len(), 1, "one row, one hit: {:?}", page.items);
    assert_eq!(
        page.total, 1,
        "and the total counts rows, not index entries"
    );
    assert_eq!(page.items[0].entity_id, tsk);
}

#[test]
fn a_note_on_a_prose_row_resolves_to_it_with_its_title() {
    let (_d, _p, mut store) = file_store();
    let prj = project(&mut store, "specline");
    let spec = store
        .create(
            Spec::new(prj.clone(), "Storage specification").into(),
            &prov(),
        )
        .unwrap()
        .entity
        .id()
        .clone();
    note(&mut store, &spec, "the cassowary constraint");

    let hits = search(&store, "cassowary");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entity_id, spec);
    assert_eq!(hits[0].entity_type, EntityType::Spec);
    assert_eq!(hits[0].title, "Storage specification");
}

/// The failure case the design has to survive. `'note'` is a marker in
/// `fts_source`, not an entity type, and if one whose note cannot be found ever
/// reached `EntityType::parse` the whole keyword half would fail — every
/// search in the store turned into semantic-only by one stray row.
#[test]
fn an_orphaned_note_index_row_does_not_take_out_keyword_search() {
    let (_d, _p, mut store) = file_store();
    let prj = project(&mut store, "specline");
    task(&mut store, &prj, "A kookaburra task");

    store
        .connection()
        .execute(
            "INSERT INTO fts_source (entity_id, entity_type, project_id, label, body) \
             VALUES ('nte_01ORPHAN', 'note', '', '', 'a kookaburra with no note')",
            [],
        )
        .unwrap();

    let results = store
        .search_prepared(&SearchQuery::new("kookaburra"), None, None)
        .unwrap();
    assert_eq!(results.report.keyword, HalfStatus::Ran);
    assert_eq!(
        results.page.items.len(),
        1,
        "the real row is found and the orphan is not: {:?}",
        results.page.items
    );
    assert_eq!(results.page.items[0].entity_type, EntityType::Task);
}

/// Every note index row, in a stable order, as the bytes that were stored.
fn note_index_rows(store: &Store) -> Vec<(String, String, String, String, String)> {
    let mut stmt = store
        .connection()
        .prepare(
            "SELECT entity_id, entity_type, project_id, label, body FROM fts_source \
             WHERE entity_type = 'note' ORDER BY entity_id",
        )
        .unwrap();
    stmt.query_map([], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    })
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

/// Make a store look like one written before migration 6: the notes are there,
/// the index has none of them, no trigger would add one, and the ledger says
/// the migration is outstanding.
fn forget_the_notes_index(path: &std::path::Path) {
    let conn = rusqlite::Connection::open(path).unwrap();
    let triggers: Vec<String> = conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'trigger' \
             AND (name LIKE 'notes_fts_%' OR name LIKE '%_notes_fts_archived')",
        )
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        triggers.len() > 2,
        "the fixture expected the migration's triggers: {triggers:?}"
    );
    for t in triggers {
        conn.execute_batch(&format!("DROP TRIGGER {t};")).unwrap();
    }
    conn.execute("DELETE FROM fts_source WHERE entity_type = 'note'", [])
        .unwrap();
    let n = conn
        .execute("DELETE FROM _keel_migrations WHERE id = 6", [])
        .unwrap();
    assert_eq!(n, 1, "the fixture did not have migration 6 to forget");
}

#[test]
fn notes_written_before_the_index_existed_are_backfilled_by_the_migration() {
    let (_d, path, mut store) = file_store();
    let prj = project(&mut store, "specline");
    let live = task(&mut store, &prj, "Live row");
    let gone = task(&mut store, &prj, "Archived row");
    let kept = note(&mut store, &live, "a pademelon was measured");
    let retracted = note(&mut store, &live, "a quoll was misread");
    note(&mut store, &gone, "a bandicoot on a row put away");
    store.retract_note(&retracted.id, &prov()).unwrap();
    store.archive(&gone, 1, &prov()).unwrap();
    drop(store);

    forget_the_notes_index(&path);
    // The gap has to be real before the migration fills it, or this test
    // passes against a migration that does nothing.
    let conn = rusqlite::Connection::open(&path).unwrap();
    let indexed: i64 = conn
        .query_row(
            "SELECT count(*) FROM fts_source WHERE entity_type = 'note'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(indexed, 0, "the fixture should start with no notes indexed");
    drop(conn);

    let store = Store::open_and_migrate(&path).unwrap();
    let hits = search(&store, "pademelon");
    assert_eq!(
        hits.len(),
        1,
        "a pre-existing note is findable after migrating"
    );
    assert_eq!(hits[0].entity_id, live);
    assert_eq!(hits[0].note_id.as_ref(), Some(&kept.id));
    assert!(
        search(&store, "quoll").is_empty(),
        "a retracted note is not backfilled"
    );
    assert!(
        search(&store, "bandicoot").is_empty(),
        "nor is a note on an archived row"
    );
}

/// Hard constraint 3 lets `fts_source` rows be deleted because each one can be
/// recomputed from the record it came from. For notes that means the rows the
/// triggers wrote as the store was used and the rows the backfill writes from
/// the `notes` table alone have to be the same bytes — including across a
/// retraction, an archived parent and text that is not ASCII.
#[test]
fn a_note_index_row_can_always_be_rebuilt_from_its_note() {
    let (_d, path, mut store) = file_store();
    let prj = project(&mut store, "specline");
    let other = project(&mut store, "other");
    let a = task(&mut store, &prj, "A");
    let b = task(&mut store, &other, "B");
    let c = task(&mut store, &prj, "C");
    note(&mut store, &a, "plain ascii finding");
    note(
        &mut store,
        &a,
        "İstanbul — ẞ, naïve, 日本語, and a \"quoted\" word",
    );
    let r = note(&mut store, &b, "to be retracted");
    note(&mut store, &b, "stays, in another project");
    note(&mut store, &c, "on a row that gets archived");
    store.retract_note(&r.id, &prov()).unwrap();
    store.archive(&c, 1, &prov()).unwrap();

    let by_trigger = note_index_rows(&store);
    assert_eq!(
        by_trigger.len(),
        3,
        "three live notes on live rows: {by_trigger:?}"
    );
    drop(store);

    forget_the_notes_index(&path);
    let store = Store::open_and_migrate(&path).unwrap();
    let by_backfill = note_index_rows(&store);

    assert_eq!(
        by_trigger, by_backfill,
        "the index row a trigger writes must be exactly what the backfill recomputes"
    );
}

/// The review finding behind the in-SQL dedupe. With the `LIMIT` applied
/// before collapsing notes onto their row, one task with more matching notes
/// than the inner limit filled every slot, collapsed to a single hit, and every
/// other match in the store vanished behind `total: 1, truncated: false` — a
/// silent truncation (hard constraint 4).
#[test]
fn a_row_with_more_matching_notes_than_the_limit_does_not_crowd_out_the_rest() {
    let (_d, _p, mut store) = file_store();
    let prj = project(&mut store, "specline");
    let noisy = task(&mut store, &prj, "The gecko problem");
    for i in 0..25 {
        note(
            &mut store,
            &noisy,
            &format!("gecko gecko finding number {i}"),
        );
    }
    let mut quiet = Vec::new();
    // Weaker matches: the word once, in a longer title. (The third argument
    // is the summary, which is not indexed — the title is.)
    for which in ["first", "second", "third"] {
        let id = store
            .create(
                Task::new(
                    prj.clone(),
                    format!("The {which} row, which mentions a gecko once among many words"),
                    "A row this test needs in the store.",
                )
                .into(),
                &prov(),
            )
            .unwrap()
            .entity
            .id()
            .clone();
        quiet.push(id);
    }

    // A limit of 5 makes the inner retrieval depth 20 — fewer than the notes.
    let mut q = SearchQuery::new("gecko");
    q.limit = 5;
    assert!(q.inner_limit() < 25, "the fixture must exceed the depth");
    let page = store.search(&q).unwrap();
    assert_eq!(page.total, 4, "four rows match: {:?}", page.items);
    assert!(!page.truncated, "all four fit in a limit of five");
    let ids: Vec<&EntityId> = page.items.iter().map(|h| &h.entity_id).collect();
    assert!(ids.contains(&&noisy), "{ids:?}");
    for id in &quiet {
        assert!(ids.contains(&id), "a weaker row was crowded out: {ids:?}");
    }

    // And when the page really is cut, it says so, with the right total.
    q.limit = 2;
    let page = store.search(&q).unwrap();
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.total, 4);
    assert!(page.truncated);
}

/// Metrics and their observations are not searchable — a search restricted to
/// them answers "no types in scope" — so a note on one must not make them
/// appear in an unfiltered search either. They are kept out of the index
/// itself, so there is no row for any query to forget to filter.
#[test]
fn a_note_on_a_row_that_is_not_searchable_is_not_indexed() {
    let (_d, _p, mut store) = file_store();
    let prj = project(&mut store, "specline");
    let metric = store
        .create(
            specline_core::Metric::new(prj.clone(), "Latency").into(),
            &prov(),
        )
        .unwrap()
        .entity
        .id()
        .clone();
    note(&mut store, &metric, "the tuatara measurement drifted");

    assert!(
        search(&store, "tuatara").is_empty(),
        "a metric is not searchable, and neither are its notes"
    );
    let indexed: i64 = store
        .connection()
        .query_row(
            "SELECT count(*) FROM fts_source WHERE entity_type = 'note'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(indexed, 0);
}

/// A note hit is the row's hit, so the date window applies to the row's
/// creation time — documented on `SearchHit::note_id`, pinned here. The row is
/// backdated so the two readings give different answers.
#[test]
fn a_date_window_applies_to_the_row_a_note_annotates_not_the_note() {
    let (_d, _p, mut store) = file_store();
    let prj = project(&mut store, "specline");
    let old = task(&mut store, &prj, "An old row");
    note(&mut store, &old, "a fresh axolotl finding");
    store
        .connection()
        .execute(
            "UPDATE tasks SET created_at = '2020-01-01T00:00:00.000000Z' WHERE id = ?1",
            [old.as_str()],
        )
        .unwrap();

    let mut q = SearchQuery::new("axolotl");
    q.since = Some(chrono::Utc::now() - chrono::Duration::days(1));
    assert!(
        store.search(&q).unwrap().items.is_empty(),
        "the row was created in 2020, whatever the note's date"
    );

    q.since = Some("2019-12-31T00:00:00Z".parse().unwrap());
    let hits = store.search(&q).unwrap().items;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entity_id, old);
}
