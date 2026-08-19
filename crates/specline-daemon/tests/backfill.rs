//! The backlog pass that runs when a daemon finds documents with no vector.
//!
//! Turning embeddings on only ever covered what was written *next*: everything
//! already in the store stayed invisible to the semantic half for ever, because
//! nothing rewrites those rows. The remedy was a line in a warning telling the
//! person who installed Specline to go and run a command, which is a chore the
//! product handed over rather than did (B-95).
//!
//! Two things are worth a test here and neither is "it embeds documents".
//! The first is that it clears a backlog it did not create. The second is that
//! it *stops* — the query selects revisions with no passages, and a revision the
//! model refuses still has none, so a loop that waited for an empty backlog
//! would spin a core for ever on one bad document.

#![cfg(feature = "embeddings")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use specline_core::{Embedder, HashEmbedder, Result, Store};
use std::sync::{Arc, Mutex};

/// An embedder that refuses everything, in the way a real one does: an error
/// per batch rather than a panic.
#[derive(Debug)]
struct RefusesEverything;

impl Embedder for RefusesEverything {
    fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Err(specline_core::Error::Embedding {
            context: "embed a batch".to_owned(),
            reason: "this model refuses".to_owned(),
        })
    }

    fn model_name(&self) -> &str {
        "refuses-everything"
    }

    fn dimensions(&self) -> usize {
        specline_core::EMBEDDING_DIM
    }
}

/// A store with prose in it and not one vector, which is what every install
/// that ran without embeddings looks like.
fn store_with_an_unembedded_corpus() -> Store {
    let mut store = Store::in_memory().unwrap();
    specline_core::fixture::load(&mut store).expect("load the fixture");
    store
}

#[test]
fn the_backlog_is_cleared_without_anybody_asking() {
    let embedder = HashEmbedder::new();
    let store = store_with_an_unembedded_corpus();

    let (current, before) = store
        .documents_missing_embeddings(Some(embedder.model_name()))
        .unwrap();
    assert!(
        before > 0 && before == current,
        "the fixture should load with no vectors at all, or this proves nothing: \
         {before} of {current}"
    );

    let store = Arc::new(Mutex::new(store));
    specline_daemon::AppState::embed_the_backlog(&store, &embedder);

    let (_, after) = store
        .lock()
        .unwrap()
        .documents_missing_embeddings(Some(embedder.model_name()))
        .unwrap();
    assert_eq!(after, 0, "every document should have a vector now");
}

/// The termination case. A model that refuses every batch leaves every document
/// exactly as selectable as it was, so the loop has to stop on "no progress"
/// rather than on "backlog empty".
///
/// A failure here does not look like a failed assertion. It looks like a test
/// that never finishes, which is also what the bug looks like in production —
/// hence the thread and the deadline rather than a bare call.
#[test]
fn a_model_that_refuses_everything_stops_the_pass_rather_than_spinning() {
    let store = Arc::new(Mutex::new(store_with_an_unembedded_corpus()));
    let handle = {
        let store = Arc::clone(&store);
        std::thread::spawn(move || {
            specline_daemon::AppState::embed_the_backlog(&store, &RefusesEverything);
        })
    };

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !handle.is_finished() {
        assert!(
            std::time::Instant::now() < deadline,
            "the backfill did not stop; a refused document keeps selecting itself"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    handle.join().unwrap();

    let (current, after) = store
        .lock()
        .unwrap()
        .documents_missing_embeddings(Some("refuses-everything"))
        .unwrap();
    assert_eq!(
        after, current,
        "nothing should have been embedded by a model that refuses everything"
    );
}
