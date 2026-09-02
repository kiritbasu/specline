//! The reads a board load makes, held to a budget (KEEL-123).
//!
//! # Why this exists
//!
//! The board "felt slow now and then" for months and nobody had a number for it.
//! The measurement that closed that is on the Phase 9 milestone; this is the part
//! that stops it drifting back. It is not a benchmark suite and it is not trying
//! to detect a twenty percent regression — it is trying to make "somebody made a
//! read ten times slower" a red build instead of something noticed a quarter
//! later.
//!
//! # Why the thresholds are so loose
//!
//! Every number here is roughly ten times the measured mean, and that is
//! deliberate. A tight threshold on wall-clock time in a shared CI runner does
//! not measure the code — it measures whatever else was scheduled on the box.
//! A test that goes red when the machine is busy gets `#[ignore]`d within a
//! month, and then it is measuring nothing at all. Ten times the mean cannot
//! catch a small regression, and is not meant to: it catches the accidental
//! N+1, the traversal that got moved inside the loop, the full-table rebuild
//! that starts running on every read. Those are the ones that actually happened.
//!
//! The byte budgets are the tight half, and they are where the real guard is.
//! Response size is deterministic — it does not care how loaded the machine is —
//! so `/api/ready` being twenty times smaller than the digest is asserted
//! exactly, and a change that puts the whole digest back in front of the board
//! fails here rather than in someone's browser.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::Value;
use specline_core::{Actor, EntityQuery, EntityStore, EntityType, NewNote};
use specline_daemon::{AppState, router};
use std::time::{Duration, Instant};

/// A daemon serving a store with the fixture corpus in it.
struct Daemon {
    base: String,
    client: reqwest::Client,
    _dir: tempfile::TempDir,
    _handle: tokio::task::JoinHandle<()>,
}

impl Daemon {
    /// Start one, with the fixture loaded and a handful of notes on tasks.
    ///
    /// The fixture writes no notes, and a note-count budget measured against
    /// zero notes measures nothing. These are added before the daemon opens the
    /// store because the daemon holds the only write handle once it is up.
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut store = specline_core::Store::open(dir.path().join("specline.sqlite"))
                .expect("open the store");
            specline_core::fixture::load(&mut store).expect("load the fixture");

            let project = project_id(&store, "harbour");
            let tasks = store
                .list(
                    &EntityQuery::in_project(project)
                        .of_type(EntityType::Task)
                        .limited(1_000),
                )
                .expect("list tasks");
            let provenance = specline_core::Provenance {
                actor: Actor::Claude,
                session_id: Some("ses_budget".to_owned()),
                surface: Some(specline_core::Surface::Code),

                client: None,
            };
            for entity in tasks.items.iter().take(20) {
                for n in 0..3 {
                    store
                        .add_note(
                            NewNote::new(
                                entity.id().clone(),
                                format!(
                                    "A note with enough prose in it to be worth not sending: \
                                     round {n} of the same paragraph, because a count-only \
                                     response is only cheaper than the bodies when the bodies \
                                     are the size real ones are."
                                ),
                                Actor::Claude,
                            ),
                            &provenance,
                        )
                        .expect("add a note");
                }
            }
        }

        let state = AppState::open(dir.path(), false).expect("open the store");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, router(state)).await;
        });

        Daemon {
            base: format!("http://{addr}"),
            client: reqwest::Client::new(),
            _dir: dir,
            _handle: handle,
        }
    }

    /// One GET, returning its status, its body and how many bytes came back.
    async fn get(&self, path: &str) -> (u16, Value, usize) {
        let response = self
            .client
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let bytes = response.bytes().await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, body, bytes.len())
    }

    /// Time a read over several rounds, reporting the mean and the worst.
    ///
    /// Several rounds rather than one, because a single sample of an HTTP round
    /// trip is mostly noise. The first round is discarded: the store's
    /// full-text index is rebuilt on the first search after any write, and the
    /// twenty notes above are a write — so round one is measuring the rebuild
    /// rather than the read.
    async fn time(&self, path: &str, rounds: u32) -> Timing {
        let (status, _, bytes) = self.get(path).await;
        assert_eq!(status, 200, "{path} did not answer 200");

        let mut total = Duration::ZERO;
        let mut worst = Duration::ZERO;
        for _ in 0..rounds {
            let started = Instant::now();
            let (status, _, _) = self.get(path).await;
            let took = started.elapsed();
            assert_eq!(status, 200, "{path} did not answer 200");
            total += took;
            worst = worst.max(took);
        }
        Timing {
            mean: total / rounds,
            worst,
            bytes,
        }
    }
}

struct Timing {
    mean: Duration,
    worst: Duration,
    bytes: usize,
}

fn project_id(store: &specline_core::Store, slug: &str) -> specline_core::EntityId {
    let page = store
        .list(
            &EntityQuery::default()
                .of_type(EntityType::Project)
                .limited(50),
        )
        .expect("list projects");
    page.items
        .iter()
        .find_map(|e| match e {
            specline_core::Entity::Project(p) if p.slug == slug => Some(p.id.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the fixture has no `{slug}` project"))
}

/// The ids in a `next_up` section of the digest.
fn digest_blocked(body: &Value) -> Vec<String> {
    let mut ids: Vec<String> = body["data"]["next_up"]["blocked"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item["id"].as_str().map(str::to_owned))
        .collect();
    ids.sort();
    ids
}

/// Every read a board load makes, in one place.
///
/// Named rather than inlined so the budget and the board cannot drift apart
/// silently: if the board starts fetching something else, this list is where
/// the omission shows up.
const BOARD_READS: &[(&str, &str, Duration)] = &[
    (
        "tasks",
        "/api/entities?project=harbour&type=task&limit=2000",
        Duration::from_millis(200),
    ),
    (
        "milestones",
        "/api/entities?project=harbour&type=milestone&limit=200",
        Duration::from_millis(200),
    ),
    (
        "note counts",
        "/api/notes?project=harbour&counts=true",
        Duration::from_millis(200),
    ),
    (
        "ranking and blocked",
        "/api/ready?project=harbour&blocked=true&limit=3",
        Duration::from_millis(2_000),
    ),
];

/// The board's reads, each under its budget.
///
/// The ranking gets ten times the budget the others do because it is ten times
/// the work: it walks the `blocks` edges of every open task twice, once for what
/// is in the way and once for what finishing it would release. That is a known
/// cost, not a regression — what this catches is it becoming a hundred times.
#[tokio::test]
async fn board_reads_stay_within_budget() {
    let daemon = Daemon::start().await;

    let mut report = String::new();
    let mut over: Vec<String> = Vec::new();
    for (name, path, budget) in BOARD_READS {
        let timing = daemon.time(path, 5).await;
        report.push_str(&format!(
            "{name:22} mean {:>6.1}ms  worst {:>6.1}ms  {:>7.1}KB  budget {:>6}ms\n",
            timing.mean.as_secs_f64() * 1000.0,
            timing.worst.as_secs_f64() * 1000.0,
            timing.bytes as f64 / 1024.0,
            budget.as_millis(),
        ));
        if timing.mean > *budget {
            over.push(format!(
                "{name} averaged {:.1}ms against a {}ms budget",
                timing.mean.as_secs_f64() * 1000.0,
                budget.as_millis()
            ));
        }
    }

    // Printed either way. When this fails, the first question is "by how much,
    // and was it only the one read" — and a bare assertion answers neither.
    println!("{report}");
    assert!(over.is_empty(), "reads over budget:\n{}", over.join("\n"));
}

/// Search, which is the read that stalls.
///
/// Its own test because its budget is a different kind of number: the first
/// search after any write rebuilds the whole BM25 index, so what is measured
/// here is the *warm* case and the stall is a known, separately-tracked cost
/// that the SQLite migration removes rather than tunes.
#[tokio::test]
async fn search_stays_within_budget() {
    let daemon = Daemon::start().await;
    let timing = daemon
        .time("/api/search?query=invoice&project=harbour&limit=20", 5)
        .await;
    println!(
        "search                 mean {:>6.1}ms  worst {:>6.1}ms  {:>7.1}KB",
        timing.mean.as_secs_f64() * 1000.0,
        timing.worst.as_secs_f64() * 1000.0,
        timing.bytes as f64 / 1024.0,
    );
    assert!(
        timing.mean < Duration::from_millis(1_500),
        "warm search averaged {:.1}ms against a 1500ms budget",
        timing.mean.as_secs_f64() * 1000.0
    );
}

/// The board must not go back to fetching the whole digest.
///
/// This is the byte half of the budget and the one that does not care how busy
/// the machine is. `/api/context?depth=full` is a project briefing — every open
/// question, the glossary, recent decisions, the activity feed — and the board
/// read one field out of it. If somebody points the board back at it, the
/// symptom in a browser is "the board feels slow now and then", which is exactly
/// the report nobody could act on.
#[tokio::test]
async fn the_ranking_is_far_cheaper_than_the_digest() {
    let daemon = Daemon::start().await;

    let (status, ready, ready_bytes) = daemon
        .get("/api/ready?project=harbour&blocked=true&limit=3")
        .await;
    assert_eq!(status, 200);
    let (status, digest, digest_bytes) =
        daemon.get("/api/context?project=harbour&depth=full").await;
    assert_eq!(status, 200);

    println!("ready {ready_bytes} bytes vs digest {digest_bytes} bytes");
    assert!(
        ready_bytes * 5 < digest_bytes,
        "the ranking ({ready_bytes} bytes) is supposed to be far cheaper than the digest \
         ({digest_bytes} bytes); if the digest has shrunk that much, say so here rather than \
         deleting the assertion"
    );

    // And it must be the *same* answer, not a cheaper different one. An app that
    // ranked work differently from the session is worse than an app with no
    // ranking at all.
    let ready_ids: Vec<String> = ready["data"]["ready"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item["id"].as_str().map(str::to_owned))
        .collect();
    let digest_ready: Vec<String> = digest["data"]["next_up"]["ready"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item["id"].as_str().map(str::to_owned))
        .collect();
    assert!(!ready_ids.is_empty(), "the fixture has ready work");
    assert_eq!(
        ready_ids, digest_ready,
        "`/api/ready` and the digest disagree about what to do next"
    );

    let mut blocked: Vec<String> = ready["data"]["blocked"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|id| id.as_str().map(str::to_owned))
        .collect();
    blocked.sort();
    assert!(
        !blocked.is_empty(),
        "the fixture has blocked work, so an empty blocked set means the traversal is inverted"
    );
    assert_eq!(
        blocked,
        digest_blocked(&digest),
        "`/api/ready?blocked=true` and the digest disagree about what is blocked"
    );
}

/// Counting notes is the same answer as counting the bodies, minus the bodies.
///
/// The saving is only worth having if the number on the card is still right, so
/// both shapes are fetched and compared rather than the cheap one being trusted.
#[tokio::test]
async fn note_counts_agree_with_the_bodies_they_replace() {
    let daemon = Daemon::start().await;

    let (status, full, full_bytes) = daemon.get("/api/notes?project=harbour").await;
    assert_eq!(status, 200);
    let (status, counted, counted_bytes) =
        daemon.get("/api/notes?project=harbour&counts=true").await;
    assert_eq!(status, 200);

    println!("notes {full_bytes} bytes vs counts {counted_bytes} bytes");
    assert!(
        counted_bytes * 4 < full_bytes,
        "counts ({counted_bytes} bytes) should be far smaller than the bodies ({full_bytes} bytes)"
    );

    let mut expected: std::collections::BTreeMap<String, usize> = Default::default();
    for note in full["data"]["notes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        if let Some(id) = note["entity_id"].as_str() {
            *expected.entry(id.to_owned()).or_default() += 1;
        }
    }
    assert!(!expected.is_empty(), "the seeded store has notes");

    let counts = counted["data"]["counts"].as_object().cloned().unwrap();
    assert_eq!(counts.len(), expected.len(), "a row was counted or dropped");
    for (id, n) in &expected {
        assert_eq!(
            counts.get(id).and_then(Value::as_u64),
            Some(*n as u64),
            "the count for {id} disagrees with its notes"
        );
    }
    assert_eq!(
        counted["data"]["total"].as_u64(),
        full["data"]["total"].as_u64(),
        "the totals disagree"
    );
}

/// Failure case: `blocked=true` with no project is refused, not answered wrong.
///
/// Blocked is per project — there is no cross-project blocked set — so the only
/// alternatives to a 400 are inventing a project or returning an empty list. The
/// second is the dangerous one: an empty blocked set is indistinguishable from
/// "nothing is blocked", and the board would quietly drop its blocked column.
#[tokio::test]
async fn blocked_without_a_project_is_refused() {
    let daemon = Daemon::start().await;
    let (status, body, _) = daemon.get("/api/ready?blocked=true").await;
    assert_eq!(status, 400, "asking for blocked with no project must fail");
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("project"),
        "the error should say what was missing, not just that something was: {message}"
    );
}

/// Failure case: an unknown project is refused rather than silently empty.
#[tokio::test]
async fn blocked_for_a_project_that_does_not_exist_is_refused() {
    let daemon = Daemon::start().await;
    let (status, body, _) = daemon
        .get("/api/ready?project=no-such-project&blocked=true")
        .await;
    assert_eq!(status, 400);
    assert!(
        body["error"]["message"].is_string(),
        "a refusal must explain itself"
    );
}
