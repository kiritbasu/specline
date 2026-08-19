//! Hybrid search: FTS5 for keywords, `sqlite-vec` for meaning, fused by rank.
//!
//! This is the seventh `DocumentStore` method, and the `impl DocumentStore for
//! Store` block lives here because this file is what completes the trait.
//! The other six are inherent methods in [`super::docs`]; every one of them is
//! delegated to below in the fully-qualified form, for the reason that file's
//! module doc gives — inside the impl block `self.revision(…)` resolves to the
//! trait method being written, not to the inherent one, and the result is a
//! function that calls itself until the stack runs out.
//!
//! # The keyword half, and the stall it removes
//!
//! BM25 over `fts_entities`, which [`super::schema`] builds and triggers keep in
//! step with the rows underneath it. Nothing here refreshes, rebuilds or
//! invalidates anything, and that absence is the whole point: the DuckDB store
//! this replaced had a full-text index that did not track inserts, so the first
//! search after *any* write rebuilt the entire index — 217 ms against a 13 ms
//! mean, measured on the live store while a decision was being written.
//!
//! Two things about FTS5 will bite at runtime rather than at compile time.
//!
//! **`MATCH` takes a query language, not a string.** A caller searching for
//! `local-first` gets `no such column: first`, because the hyphen makes FTS5
//! read `first` as a column filter. The error names a word from the user's own
//! text and reads like a schema bug. [`fts_match`] turns caller input into
//! quoted terms so nothing a person types is ever parsed as syntax.
//!
//! **`bm25()` returns a negative number where lower is better.** The fusion
//! wants higher-is-better, so the score is negated on the way out. Get the sign
//! wrong and the worst match ranks first, which looks like a plausible ordering
//! and is completely wrong — hence a test that asserts an obviously-best row
//! comes first rather than one that asserts a score.
//!
//! # The vector half, and why it is brute force
//!
//! `sqlite-vec`'s `vec_distance_cosine` over the `documents.embedding` column,
//! scanned in full and sorted by distance. There is deliberately no `vec0`
//! virtual table:
//!
//! - A `vec0` table is a second copy of every vector, and something has to keep
//!   it in step with `documents`. That something would be a trigger in the
//!   schema, and until it exists the only alternative is repopulating the table
//!   at search time — which is the rebuild this task exists to delete, wearing a
//!   different hat.
//! - At this scale it buys nothing. A few thousand 384-float vectors is 1–3 ms
//!   brute force, measured, against a corpus that is one user's project memory.
//!   Scale discipline says do not add the index until a measurement asks for it.
//!
//! `sqlite-vec` is 0.1.9 and its author says to expect breaking changes. That is
//! survivable precisely because the vectors are ordinary little-endian f32 blobs
//! we own rather than rows inside a proprietary index: replacing this half is a
//! new query over the same column, not a re-embedding run over the whole corpus.
//! If `vec_distance_cosine` disappears, the same loop in Rust over the same
//! blobs is about fifty lines and needs no schema change.
//!
//! One sharp edge is guarded explicitly: `vec_distance_cosine` raises an error
//! when the two vectors differ in length, and that error fails the *whole*
//! query. A single document embedded by an older model with a different width
//! would take out search for everything. `length(embedding) = ?` in the `WHERE`
//! clause skips those rows instead, so a model change degrades recall rather
//! than breaking search.
//!
//! # The fusion
//!
//! Reciprocal rank, unchanged from the DuckDB implementation, because BM25
//! scores and cosine distances are not on comparable scales — they are not even
//! in comparable units. A row both halves found gets both contributions and is
//! reported as [`SearchSource::Both`], which is the strongest signal available
//! here: an independent keyword match and an independent semantic match
//! agreeing.
//!
//! Each half retrieves [`SearchQuery::inner_limit`] rows, not `limit`.
//! Retrieving exactly `k` and *then* filtering by project is how a search
//! returns three results when forty exist.
//!
//! # Saying which halves ran
//!
//! Everything above degrades quietly by design — a missing model, an absent
//! extension and a filter with no prose in scope all leave one half returning
//! an empty list while the other answers. That is right, and on its own it is
//! also the failure this codebase exists to refuse: the caller cannot tell
//! "there is nothing about this in the store" from "half the search did not
//! run", and the first is a much stronger claim than anyone made.
//!
//! So every half returns a [`Half`] — its hits *and* whether it looked — and
//! [`SearchResults`] carries the pair out to the caller as a
//! [`SearchReport`]. Nothing here decides what to do about it; `specline-mcp`
//! puts it in the response and `specline doctor` reports it. What this file
//! guarantees is that the information exists at all.

use super::Store;
use crate::store::{
    Blob, DocumentStore, HalfStatus, Page, SearchHit, SearchQuery, SearchReport, SearchResults,
    SearchSource,
};
use crate::{BlobId, Document, DocumentDiff, Embedder, EntityId, EntityType, Error, Result};
use chrono::{DateTime, Utc};
use rusqlite::params_from_iter;
use rusqlite::types::Value;
use std::collections::{HashMap, HashSet};

/// The reciprocal-rank-fusion constant.
///
/// 60 is the value from the original RRF paper and the value the DuckDB
/// implementation used. At this corpus size the choice barely matters, and
/// keeping the well-known number means nobody has to wonder why it is 37.
const RRF_K: f64 = 60.0;

/// How far a hit's excerpt reaches around the first matching term.
const EXCERPT_WIDTH: usize = 240;

/// `bm25()` column weights for `(label, body)`.
///
/// A match in the title of a thing is worth more than a match somewhere in its
/// prose, and doubling the label is the cheapest way to say so. It also makes
/// the ordering of a small corpus stable enough to assert on, which matters
/// more than the exact ratio.
const LABEL_WEIGHT: &str = "2.0";
/// See [`LABEL_WEIGHT`].
const BODY_WEIGHT: &str = "1.0";

/// One half's contribution: what it found, and whether it looked at all.
///
/// The status travels with the hits rather than being worked out afterwards
/// because only the half itself knows the difference between "looked and found
/// nothing" and "had nothing to look at" — and from the outside those two
/// produce the same empty vector.
struct Half {
    hits: Vec<SearchHit>,
    status: HalfStatus,
}

impl Half {
    /// It ran. The hits may still be empty.
    fn ran(hits: Vec<SearchHit>) -> Self {
        Half {
            hits,
            status: HalfStatus::Ran,
        }
    }

    /// It did not run, for the given reason.
    fn skipped(status: HalfStatus) -> Self {
        Half {
            hits: Vec::new(),
            status,
        }
    }
}

/// `?, ?, ?` for an `IN` list of `n` bound values.
fn placeholders(n: usize) -> String {
    vec!["?"; n].join(", ")
}

/// Turn caller text into an FTS5 query nobody can accidentally write syntax in.
///
/// Every term is wrapped in double quotes, with any embedded quote doubled, so
/// hyphens, apostrophes, colons, asterisks and `NOT` are all read as text rather
/// than as operators. Without this, `local-first` fails with `no such column:
/// first` — an error naming a word from the search box, which reads as a schema
/// problem and is not one.
///
/// Terms are joined with `OR` rather than the implicit `AND`, because a search
/// for "the keyword index rebuild" should rank a row matching three of those
/// four words above one matching two, not reject both. BM25 does that ranking;
/// `AND` would instead return nothing and look like an empty corpus.
///
/// `None` means the text held nothing searchable — punctuation, say. That is a
/// query with no hits rather than an error: the caller typed something, it just
/// contained no words.
///
/// Public so the fuzz target can reach it. This is the one function in the
/// crate that takes a raw human string and produces something another language
/// parses, which makes "what does it do with bytes nobody expected" a question
/// worth asking a million times rather than four.
pub fn fts_match(text: &str) -> Option<String> {
    let terms: Vec<String> = text
        .split_whitespace()
        .filter(|t| t.chars().any(char::is_alphanumeric))
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    if terms.is_empty() {
        return None;
    }
    Some(terms.join(" OR "))
}

/// A short window of `body` around the first query term that appears in it.
///
/// Windowed rather than truncated from the front, because a hit whose excerpt
/// does not contain the thing that was searched for tells the reader nothing
/// about why it matched.
fn excerpt(body: &str, query: &str) -> String {
    // Case-folded on a char-by-char copy that keeps the original byte offsets.
    //
    // `to_lowercase` does not: `İ` is two bytes and lowercases to three, `İ`
    // and `ẞ` both change length, and every byte after one of them in the
    // lowercased copy sits at a different offset than in the original. The
    // offset found there was then used to slice `body`, so a document with one
    // such character upstream of the match produced an excerpt starting in the
    // wrong place — or, on a character boundary, the loops below silently
    // walked to a different one.
    //
    // ASCII folding is enough for what this does. Finding a match one case
    // rule short is a slightly worse excerpt; slicing at a shifted offset is a
    // wrong one.
    let lower: String = body
        .chars()
        .map(|c| {
            if c.is_ascii() {
                c.to_ascii_lowercase()
            } else {
                c
            }
        })
        .collect();
    debug_assert_eq!(lower.len(), body.len(), "the fold must preserve offsets");

    let start = query
        .split_whitespace()
        .filter_map(|term| {
            let term: String = term
                .chars()
                .map(|c| {
                    if c.is_ascii() {
                        c.to_ascii_lowercase()
                    } else {
                        c
                    }
                })
                .collect();
            lower.find(&term)
        })
        .min()
        .unwrap_or(0);

    let mut begin = start.saturating_sub(60);
    while begin > 0 && !body.is_char_boundary(begin) {
        begin -= 1;
    }
    let mut end = (begin + EXCERPT_WIDTH).min(body.len());
    while end < body.len() && !body.is_char_boundary(end) {
        end += 1;
    }

    let mut out = String::new();
    if begin > 0 {
        out.push('…');
    }
    out.push_str(body[begin..end].trim());
    if end < body.len() {
        out.push('…');
    }
    out
}

/// Fuse ranked lists by reciprocal rank.
///
/// Raw BM25 scores and cosine distances are not comparable, so the fusion uses
/// *rank* and never the scores themselves. A row found by both halves collects
/// both contributions, which is the behaviour wanted: two independent indexes
/// agreeing is the best evidence this store has.
fn reciprocal_rank_fusion(lists: Vec<Vec<SearchHit>>, limit: usize) -> Vec<SearchHit> {
    let mut fused: Vec<SearchHit> = Vec::new();

    for list in lists {
        for (rank, hit) in list.into_iter().enumerate() {
            let contribution = 1.0 / (RRF_K + rank as f64 + 1.0);
            match fused.iter_mut().find(|h| h.entity_id == hit.entity_id) {
                Some(existing) => {
                    existing.score += contribution;
                    if existing.source != hit.source {
                        existing.source = SearchSource::Both;
                    }
                }
                None => {
                    let mut hit = hit;
                    hit.score = contribution;
                    fused.push(hit);
                }
            }
        }
    }

    fused.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            // Ties break by id so results are stable between calls. An unstable
            // order makes a human wonder whether the data changed.
            .then_with(|| a.entity_id.cmp(&b.entity_id))
    });
    fused.truncate(limit);
    fused
}

impl Store {
    /// Hybrid search across both indexes, fused by reciprocal rank.
    ///
    /// Uses whatever embedder was attached with [`Store::with_embedder`]. If
    /// none was, this is keyword search wearing the name of a hybrid one. See
    /// [`Store::search_with`] for the caller-supplied variant.
    ///
    /// **This drops the [`SearchReport`].** Anything that shows results to a
    /// person or a model wants [`Store::search_prepared`] instead, which says
    /// which halves ran — without it there is no way to tell an empty store
    /// from an empty search.
    pub fn search(&self, query: &SearchQuery) -> Result<Page<SearchHit>> {
        // The store's own embedder, when one was attached. This was briefly a
        // hard `None`, which meant the semantic half never ran no matter what
        // the caller had set up — silently, since keyword results kept coming.
        self.search_with(query, self.embedder())
    }

    /// Search with an embedder supplied by the caller.
    ///
    /// The embedder is a parameter rather than state because `specline-core` is
    /// handed what it needs — and because a store with no embedder must still
    /// search. Passing `None` is not degraded to the point of useless: the
    /// keyword half covers every searchable artifact, prose included. Search
    /// degrades; it never fails.
    pub fn search_with(
        &self,
        query: &SearchQuery,
        embedder: Option<&dyn Embedder>,
    ) -> Result<Page<SearchHit>> {
        Ok(self.search_prepared(query, embedder, None)?.page)
    }

    /// Search with the query text already turned into a vector.
    ///
    /// Embedding a query is the one expensive thing on the read path — model
    /// inference, tens of milliseconds — and a caller that serialises the whole
    /// store behind a lock does not want it happening inside the critical
    /// section. The daemon embeds first and hands the result in here, so the
    /// lock covers two SQL queries and nothing else.
    ///
    /// `precomputed` must be an embedding of `query.text` from the same model
    /// the corpus was embedded with. Nothing checks that, because nothing can:
    /// a vector carries no provenance. Passing one from a different model does
    /// not fail, it returns confidently irrelevant neighbours — which is why
    /// the only caller is the one that owns both ends.
    pub fn search_prepared(
        &self,
        query: &SearchQuery,
        embedder: Option<&dyn Embedder>,
        precomputed: Option<&[f32]>,
    ) -> Result<SearchResults> {
        if query.text.trim().is_empty() {
            // Refused rather than answered with nothing. An empty result
            // would read to a model as "there is
            // nothing about this in the store", which is a different and much
            // more damaging claim than "you did not say what to look for".
            return Err(Error::Invalid {
                entity_type: EntityType::Artifact,
                field: "query".to_owned(),
                problem: "the search text is empty".to_owned(),
                expected: "some words to search for; to list entities without searching, \
                           use specline_get or specline_context instead"
                    .to_owned(),
            });
        }

        // Either half can legitimately fail — a corpus with no embeddings at
        // all, a vector width that no longer matches — and one failing must not
        // take out the other. A search that returns part of the story is far
        // more useful than one that returns an error, so long as the reason is
        // in the log rather than swallowed.
        //
        // Both halves report what they did as well as what they found, and the
        // report travels out with the results. A half that failed here is
        // indistinguishable at the call site from one that matched nothing,
        // which is the whole of KEEL-251.
        let keyword = self.search_keyword(query).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "keyword search failed; returning semantic hits only");
            Half::skipped(HalfStatus::Failed)
        });
        let semantic = self
            .search_semantic(query, embedder, precomputed)
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "semantic search failed; returning keyword hits only");
                Half::skipped(HalfStatus::Failed)
            });

        let report = SearchReport {
            keyword: keyword.status,
            semantic: semantic.status,
        };
        let lists = vec![keyword.hits, semantic.hits];

        // `total` counts distinct artifacts, not raw hits: a row both halves
        // found is one result, and counting it twice would make `truncated`
        // lie about how much was left out (hard constraint 4).
        let distinct: HashSet<EntityId> = lists
            .iter()
            .flatten()
            .map(|h| h.entity_id.clone())
            .collect();
        let total = distinct.len();
        let fused = reciprocal_rank_fusion(lists, query.limit);
        Ok(SearchResults {
            page: Page {
                truncated: total > fused.len(),
                total,
                items: fused,
            },
            report,
        })
    }

    /// BM25 over every searchable artifact, prose included.
    ///
    /// The index is maintained by triggers, so there is nothing to refresh
    /// here. Archived rows are already absent from `fts_source`, which is why
    /// no query in this file carries a `WHERE archived_at IS NULL` that someone
    /// could later forget.
    fn search_keyword(&self, query: &SearchQuery) -> Result<Half> {
        let Some(expression) = fts_match(&query.text) else {
            return Ok(Half::skipped(HalfStatus::NoTerms));
        };

        let mut params: Vec<Value> = vec![Value::Text(expression)];
        let mut filters = String::new();

        if let Some(project) = &query.project_id {
            filters.push_str(" AND s.project_id = ?");
            params.push(Value::Text(project.as_str().to_owned()));
        }
        if !query.entity_types.is_empty() {
            let types: Vec<EntityType> = query
                .entity_types
                .iter()
                .copied()
                .filter(|t| t.is_searchable())
                .collect();
            // Asking only for metrics is a well-formed question with no
            // answers, not an unfiltered search.
            if types.is_empty() {
                return Ok(Half::skipped(HalfStatus::NoTypesInScope));
            }
            filters.push_str(&format!(
                " AND s.entity_type IN ({})",
                placeholders(types.len())
            ));
            params.extend(types.iter().map(|t| Value::Text(t.as_str().to_owned())));
        }

        // `bm25()` is negative and lower-is-better, so the score is negated
        // here and ordered descending. This is the sign that ranks the worst
        // match first if it is wrong, and does it plausibly.
        let sql = format!(
            "SELECT s.entity_id AS entity_id, s.entity_type AS entity_type, \
                    s.project_id AS project_id, s.label AS label, s.body AS body, \
                    -bm25(fts_entities, {LABEL_WEIGHT}, {BODY_WEIGHT}) AS score \
             FROM fts_entities \
             JOIN fts_source AS s ON s.rowid = fts_entities.rowid \
             WHERE fts_entities MATCH ?{filters} \
             ORDER BY score DESC \
             LIMIT {}",
            query.inner_limit()
        );

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(Error::storage("prepare the keyword search"))?;
        let mut rows = stmt
            .query(params_from_iter(params))
            .map_err(Error::storage(format!(
                "run the keyword search for `{}`",
                query.text
            )))?;

        let e = |c: &'static str| {
            let context = format!("read column `{c}` of a keyword hit");
            move |source| Error::Storage { context, source }
        };
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(Error::storage("read a keyword search hit"))?
        {
            let label: String = row.get("label").map_err(e("label"))?;
            let body: String = row.get("body").map_err(e("body"))?;
            out.push(SearchHit {
                entity_id: EntityId::parse(
                    &row.get::<_, String>("entity_id").map_err(e("entity_id"))?,
                )?,
                entity_type: EntityType::parse(
                    &row.get::<_, String>("entity_type")
                        .map_err(e("entity_type"))?,
                )?,
                project_id: match row
                    .get::<_, Option<String>>("project_id")
                    .map_err(e("project_id"))?
                {
                    Some(p) if !p.is_empty() => Some(EntityId::parse(&p)?),
                    _ => None,
                },
                excerpt: excerpt(if body.is_empty() { &label } else { &body }, &query.text),
                title: label,
                score: row.get::<_, f64>("score").unwrap_or_default(),
                source: SearchSource::Keyword,
            });
        }
        self.within_dates(out, query).map(Half::ran)
    }

    /// Cosine nearest neighbours over the passages of the prose types.
    ///
    /// Does not run at all when there is no embedder, which is the honest
    /// answer rather than a failure: without one there is no query vector, and
    /// the keyword half still covers the whole corpus. It says so in the
    /// [`Half`] it returns, because the caller cannot tell that from a search
    /// that ran and matched nothing.
    fn search_semantic(
        &self,
        query: &SearchQuery,
        embedder: Option<&dyn Embedder>,
        precomputed: Option<&[f32]>,
    ) -> Result<Half> {
        // If `sqlite-vec` never registered, `vec_distance_cosine` does not
        // exist and this query would fail outright — turning a search into an
        // error for a caller who only wanted results. Degrade to keyword-only
        // instead. Whoever opened the store is expected to have said so loudly
        // at startup; `Store::vector_search_available` is what they ask.
        if !self.vector_search_available() {
            return Ok(Half::skipped(HalfStatus::NoVectorExtension));
        }
        // The embedder is required even when the vector was computed
        // elsewhere, because it is what names the model — and without the name
        // there is no way to know which stored vectors this one may be compared
        // against. Returning nothing is the honest answer; guessing is how the
        // failure below happens silently.
        let Some(embedder) = embedder else {
            return Ok(Half::skipped(HalfStatus::NoModel));
        };
        let owned;
        let vector: &[f32] = match precomputed {
            Some(v) => v,
            None => {
                owned = embedder.embed_one(&query.text)?;
                &owned
            }
        };
        let probe: Vec<u8> = vector.iter().flat_map(|f| f.to_le_bytes()).collect();
        let width = probe.len() as i64;

        // The probe binds first because it appears first in the statement —
        // in the SELECT list, not the WHERE clause.
        let mut params: Vec<Value> = vec![
            Value::Blob(probe),
            Value::Integer(width),
            Value::Text(embedder.model_name().to_owned()),
        ];
        let mut filters = String::new();

        // Qualified with `c.`, because the scan now joins `document_chunks` to
        // `documents` and both carry a `project_id`. Unqualified, SQLite picks
        // one and the query still runs.
        if let Some(project) = &query.project_id {
            filters.push_str(" AND c.project_id = ?");
            params.push(Value::Text(project.as_str().to_owned()));
        }
        if !query.entity_types.is_empty() {
            let types: Vec<EntityType> = query
                .entity_types
                .iter()
                .copied()
                .filter(|t| t.has_document())
                .collect();
            // Only five types have prose, so asking for tasks alone means the
            // semantic half has nothing to contribute — not that it failed.
            if types.is_empty() {
                return Ok(Half::skipped(HalfStatus::NoTypesInScope));
            }
            filters.push_str(&format!(
                " AND c.entity_type IN ({})",
                placeholders(types.len())
            ));
            params.extend(types.iter().map(|t| Value::Text(t.as_str().to_owned())));
        }

        // `length(embedding) = ?` is the guard, not an optimisation:
        // `vec_distance_cosine` errors on a width mismatch and that error kills
        // the whole query, so one passage left behind by a model change would
        // otherwise take out semantic search entirely.
        //
        // **Best passage per document, not every passage.** A long spec has
        // sixty-nine of them and half of one page of results could otherwise be
        // the same document sixty-nine times. `ROW_NUMBER` over the distance
        // keeps the nearest and discards the rest, which is also the honest
        // ranking: a document matches as well as its best passage does. Mean
        // would punish a long document for having sections about other things,
        // which is backwards.
        //
        // No archive or status predicate here, and that is the point. Passages
        // exist only for the current revision of a live document, because
        // archiving and superseding both delete them — so there is no
        // `WHERE archived_at IS NULL` for anyone to forget, which is exactly
        // the omission KEEL-175 was.
        let sql = format!(
            "WITH scored AS (
                 SELECT c.entity_id, c.entity_type, c.project_id, d.title,
                        c.text, c.heading_path,
                        vec_distance_cosine(c.embedding, ?) AS distance
                   FROM document_chunks c
                   JOIN documents d ON d.doc_id = c.doc_id
                  WHERE length(c.embedding) = ? AND c.embedding_model = ?{filters}
             ), ranked AS (
                 SELECT *, ROW_NUMBER() OVER (
                     PARTITION BY entity_id ORDER BY distance ASC
                 ) AS rn FROM scored
             )
             SELECT entity_id, entity_type, project_id, title, text, heading_path, distance \
               FROM ranked WHERE rn = 1 \
              ORDER BY distance ASC \
              LIMIT {}",
            query.inner_limit()
        );

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(Error::storage("prepare the semantic search"))?;
        let mut rows = stmt
            .query(params_from_iter(params))
            .map_err(Error::storage(format!(
                "run the semantic search for `{}`",
                query.text
            )))?;

        let e = |c: &'static str| {
            let context = format!("read column `{c}` of a semantic hit");
            move |source| Error::Storage { context, source }
        };
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(Error::storage("read a semantic search hit"))?
        {
            // Cosine *distance*: 0 is identical, 1 is orthogonal. Reported as a
            // similarity so that `SearchHit::score`'s "higher is better" holds
            // for a caller reading it directly, and dropped at zero — a vector
            // with no overlap is not a weak hit, it is a row that happened to
            // sort somewhere. Without the floor, a query with no semantic
            // neighbours injects arbitrary rows into the fusion at rank one.
            let distance: f64 = row.get("distance").map_err(e("distance"))?;
            let similarity = 1.0 - distance;
            if similarity <= 0.0 {
                continue;
            }

            // The excerpt is cut from the passage rather than from the whole
            // document, which is the change that makes it worth reading. A
            // semantic hit often shares no words with the query, so there was
            // frequently no term to centre on and the window fell back to the
            // opening of the document — the least informative part of it, and
            // for a spec the same paragraph every time. Cut from the passage
            // there is nowhere uninformative left to fall back to.
            //
            // Still cut, not returned whole: a passage runs to 1,400
            // characters and a page of those is most of a digest's budget.
            let text: String = row.get("text").map_err(e("text"))?;
            let heading_path: String = row.get("heading_path").map_err(e("heading_path"))?;
            let body = if heading_path.is_empty() {
                text
            } else {
                format!("{heading_path} — {text}")
            };
            out.push(SearchHit {
                entity_id: EntityId::parse(
                    &row.get::<_, String>("entity_id").map_err(e("entity_id"))?,
                )?,
                entity_type: EntityType::parse(
                    &row.get::<_, String>("entity_type")
                        .map_err(e("entity_type"))?,
                )?,
                project_id: match row
                    .get::<_, Option<String>>("project_id")
                    .map_err(e("project_id"))?
                {
                    Some(p) if !p.is_empty() => Some(EntityId::parse(&p)?),
                    _ => None,
                },
                title: row.get::<_, String>("title").map_err(e("title"))?,
                excerpt: excerpt(&body, &query.text),
                score: similarity,
                source: SearchSource::Semantic,
            });
        }
        self.within_dates(out, query).map(Half::ran)
    }

    /// Keep only the hits whose entity was created inside the query's window.
    ///
    /// Done in Rust, after retrieval, because neither index carries a
    /// timestamp: `fts_source` holds text and ids, and the vector scan reads
    /// `documents`. Both halves are filtered against the *entity's* row rather
    /// than the document's, so "created since Monday" means the same thing
    /// whichever half found it — a spec revised today is not a spec created
    /// today.
    ///
    /// A hit whose row cannot be found is dropped rather than kept: it cannot
    /// be shown to be inside the window, and a date filter that quietly admits
    /// unknowns is not a filter.
    fn within_dates(&self, hits: Vec<SearchHit>, query: &SearchQuery) -> Result<Vec<SearchHit>> {
        if query.since.is_none() && query.until.is_none() {
            return Ok(hits);
        }

        let mut by_table: HashMap<&'static str, Vec<String>> = HashMap::new();
        for hit in &hits {
            by_table
                .entry(hit.entity_type.table())
                .or_default()
                .push(hit.entity_id.as_str().to_owned());
        }

        let mut created: HashMap<String, DateTime<Utc>> = HashMap::new();
        for (table, ids) in by_table {
            let sql = format!(
                "SELECT id, created_at FROM {table} WHERE id IN ({})",
                placeholders(ids.len())
            );
            let mut stmt = self.conn.prepare(&sql).map_err(Error::storage(format!(
                "prepare the date filter over `{table}`"
            )))?;
            let mut rows = stmt
                .query(params_from_iter(ids.into_iter().map(Value::Text)))
                .map_err(Error::storage(format!(
                    "read creation times from `{table}` to filter search hits"
                )))?;
            while let Some(row) = rows
                .next()
                .map_err(Error::storage("read a creation time"))?
            {
                let context = |c: &'static str| {
                    let context = format!("read column `{c}` of `{table}`");
                    move |source| Error::Storage { context, source }
                };
                let id: String = row.get("id").map_err(context("id"))?;
                let raw: String = row.get("created_at").map_err(context("created_at"))?;
                created.insert(id, super::rows::parse_ts(table, "created_at", &raw)?);
            }
        }

        Ok(hits
            .into_iter()
            .filter(|hit| match created.get(hit.entity_id.as_str()) {
                Some(at) => {
                    query.since.is_none_or(|since| *at >= since)
                        && query.until.is_none_or(|until| *at < until)
                }
                None => false,
            })
            .collect())
    }
}

impl DocumentStore for Store {
    // Every one of these is the inherent method, named in full. `self.method(…)`
    // would resolve to the trait method being defined here and recurse until
    // the stack ran out — at runtime, with nothing at compile time to say so.
    fn write_revision(&mut self, document: Document) -> Result<Document> {
        Store::write_revision(self, document)
    }

    fn revision(&self, entity_id: &EntityId, version: Option<i32>) -> Result<Option<Document>> {
        Store::revision(self, entity_id, version)
    }

    fn revisions(&self, entity_id: &EntityId) -> Result<Vec<Document>> {
        Store::revisions(self, entity_id)
    }

    fn diff(&self, entity_id: &EntityId, from: i32, to: i32) -> Result<DocumentDiff> {
        Store::diff(self, entity_id, from, to)
    }

    fn search(&self, query: &SearchQuery) -> Result<Page<SearchHit>> {
        Store::search(self, query)
    }

    fn put_blob(&mut self, blob: Blob) -> Result<BlobId> {
        Store::put_blob(self, blob)
    }

    fn get_blob(&self, blob_id: &BlobId) -> Result<Option<Blob>> {
        Store::get_blob(self, blob_id)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::store::rows::spec_for;
    use crate::store::rows::{insert_params, insert_stmt};
    use crate::{Actor, Entity, Project, Spec, Task};

    /// The excerpt slices the original string with an offset found in a
    /// case-folded copy, so the two must have identical byte lengths.
    ///
    /// `to_lowercase` does not guarantee that. `İ` is two bytes and lowercases
    /// to three, which shifts every byte after it — and the shifted offset was
    /// then used to slice the original. Latin text never noticed; one Turkish
    /// capital İ in a document, ahead of the match, was enough.
    #[test]
    fn an_excerpt_offset_survives_a_character_that_changes_length_when_lowercased() {
        let body = "İstanbul is where the meeting happened, and the decision was taken there.";
        let out = excerpt(body, "decision");
        assert!(
            out.contains("decision"),
            "the excerpt should contain the term it was centred on: {out}"
        );

        // The same document without the awkward character, to show the term is
        // found either way and it is the offset that was at stake.
        let plain = "Istanbul is where the meeting happened, and the decision was taken there.";
        assert!(excerpt(plain, "decision").contains("decision"));
    }

    /// And the ordinary case still works: the excerpt is centred on the match
    /// rather than starting at the top of the document.
    #[test]
    fn an_excerpt_is_centred_on_the_match() {
        let body = format!("{}needle{}", "a".repeat(400), "b".repeat(400));
        let out = excerpt(&body, "needle");
        assert!(out.contains("needle"), "{out}");
        assert!(out.starts_with('…'), "a cut front should say so: {out}");
        assert!(out.ends_with('…'), "a cut tail should say so: {out}");
    }

    /// An embedder that maps text to a topic, not to its words.
    ///
    /// The point of a semantic half is finding something that shares no
    /// vocabulary with the query, and a hash-of-words embedder cannot
    /// demonstrate that — anything it scores highly, the keyword half found
    /// too. This one puts "penguin" and "flightless" in the same bucket and
    /// nowhere near "billing", so a test can assert a hit that BM25 provably
    /// could not have produced.
    #[derive(Debug)]
    struct TopicEmbedder;

    impl TopicEmbedder {
        const TOPICS: [&'static str; 4] = ["birds", "billing", "storage", "other"];

        fn topic_of(text: &str) -> usize {
            let lower = text.to_lowercase();
            for word in ["penguin", "flightless", "albatross"] {
                if lower.contains(word) {
                    return 0;
                }
            }
            for word in ["invoice", "billing", "ledger"] {
                if lower.contains(word) {
                    return 1;
                }
            }
            for word in ["sqlite", "index", "storage"] {
                if lower.contains(word) {
                    return 2;
                }
            }
            3
        }
    }

    impl Embedder for TopicEmbedder {
        fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            Ok(texts
                .iter()
                .map(|t| {
                    let mut v = vec![0.0f32; Self::TOPICS.len()];
                    v[Self::topic_of(t)] = 1.0;
                    v
                })
                .collect())
        }

        fn model_name(&self) -> &str {
            "test-topic-embedder"
        }

        fn dimensions(&self) -> usize {
            Self::TOPICS.len()
        }
    }

    /// A timestamp the TEXT column can hold exactly — the format is six
    /// fractional digits and `Utc::now()` is nanosecond-precise.
    fn stored_now() -> DateTime<Utc> {
        DateTime::from_timestamp_micros(Utc::now().timestamp_micros()).unwrap_or_default()
    }

    fn insert_entity(store: &Store, entity: &Entity) {
        let spec = spec_for(entity.entity_type());
        store
            .conn
            .execute(&insert_stmt(&spec), params_from_iter(insert_params(entity)))
            .unwrap();
    }

    /// A store with one project in it, and that project's id.
    fn store_with_a_project() -> (Store, EntityId) {
        let store = Store::in_memory().unwrap();
        let mut project = Project::new("specline", "Specline");
        project.description = Some("a local-first store for everything but the code".to_owned());
        let id = project.id.clone();
        insert_entity(&store, &project.into());
        (store, id)
    }

    fn add_project(store: &Store, slug: &str, name: &str) -> EntityId {
        let project = Project::new(slug, name);
        let id = project.id.clone();
        insert_entity(store, &project.into());
        id
    }

    fn add_task(store: &Store, project: &EntityId, title: &str, body: &str) -> EntityId {
        add_task_created_at(store, project, title, body, stored_now())
    }

    /// A task with its creation time chosen, for the date-filter test.
    ///
    /// `number` is assigned here because `(project_id, number)` is unique and
    /// `Task::new` leaves it at zero — the entity half fills it in, and these
    /// tests insert rows directly.
    fn add_task_created_at(
        store: &Store,
        project: &EntityId,
        title: &str,
        body: &str,
        created_at: DateTime<Utc>,
    ) -> EntityId {
        let taken: i64 = store
            .conn
            .query_row(
                "SELECT count(*) FROM tasks WHERE project_id = ?1",
                [project.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        let mut task = Task::new(project.clone(), title, "a summary");
        task.number = (taken + 1) as i32;
        task.body = Some(body.to_owned());
        task.audit.created_at = created_at;
        let id = task.id.clone();
        insert_entity(store, &task.into());
        id
    }

    /// A spec row plus the document that carries its prose, embedded if an
    /// embedder is supplied. Prose types are indexed from `documents`, so a
    /// spec with no revision is a spec with no searchable text.
    fn add_spec(
        store: &mut Store,
        project: &EntityId,
        title: &str,
        body: &str,
        embedder: Option<std::sync::Arc<dyn Embedder>>,
    ) -> EntityId {
        let spec = Spec::new(project.clone(), title);
        let id = spec.id.clone();
        insert_entity(store, &spec.into());

        let document = Document::first(
            EntityType::Spec,
            id.clone(),
            Some(project.clone()),
            title,
            body,
            Actor::Claude,
            stored_now(),
        )
        .unwrap();
        // The embedder is attached to the *store* and the passages are built by
        // the write path, rather than a vector being set on the document here.
        //
        // Setting it by hand was how this helper worked until B-55, and it was
        // always a small lie: production embeds inside `write_revision_in` and
        // the tests embedded outside it, so the two could diverge and the suite
        // would agree with itself either way. Now the only way a test gets a
        // vector is the way a user does.
        if let Some(embedder) = embedder {
            store.set_embedder(embedder);
        }
        store.write_revision(document).unwrap();
        id
    }

    fn ids(page: &Page<SearchHit>) -> Vec<String> {
        page.items
            .iter()
            .map(|h| h.entity_id.as_str().to_owned())
            .collect()
    }

    #[test]
    fn a_task_a_project_and_a_spec_are_all_findable_by_keyword() {
        let (mut store, project) = store_with_a_project();
        let task = add_task(
            &store,
            &project,
            "The board is slow",
            "the keyword index is rebuilt on every write",
        );
        let spec = add_spec(
            &mut store,
            &project,
            "Storage",
            "documents and rows live in one SQLite file",
            None,
        );

        let hit = |text: &str| store.search(&SearchQuery::new(text)).unwrap();

        assert_eq!(ids(&hit("keyword")), vec![task.as_str()]);
        assert_eq!(ids(&hit("SQLite")), vec![spec.as_str()]);
        assert_eq!(ids(&hit("local-first")), vec![project.as_str()]);

        // The type comes back with the hit, and getting it wrong would send a
        // caller to the wrong table.
        let spec_hit = hit("SQLite");
        assert_eq!(spec_hit.items[0].entity_type, EntityType::Spec);
        assert_eq!(spec_hit.items[0].title, "Storage");
    }

    /// Nothing a person types may reach the FTS5 parser as syntax.
    ///
    /// Unquoted, `local-first` fails with `no such column: first` — an error
    /// naming a word from the search box, which reads like a schema bug.
    #[test]
    fn punctuation_in_a_query_is_text_and_not_syntax() {
        let (mut store, project) = store_with_a_project();
        add_task(
            &store,
            &project,
            "Don't rebuild the index",
            "the \"board\" stalls behind a write",
        );
        add_spec(
            &mut store,
            &project,
            "Storage",
            "one SQLite file, no second engine",
            None,
        );

        for text in [
            "local-first",
            "the \"board\" is slow",
            "don't rebuild",
            "NOT storage",
            "index:label",
            "store*",
            "-- ; DROP",
            "…",
            "?!",
        ] {
            let found = store.search(&SearchQuery::new(text));
            assert!(
                found.is_ok(),
                "searching for `{text}` failed: {:?}",
                found.err()
            );
        }

        // Not merely un-erroring: the hyphenated phrase still finds the row
        // whose description contains it.
        assert_eq!(
            ids(&store.search(&SearchQuery::new("local-first")).unwrap()),
            vec![project.as_str()]
        );
        // A query of pure punctuation has no terms, which is no hits and not a
        // failure.
        let nothing = store.search(&SearchQuery::new("…?!")).unwrap();
        assert!(nothing.items.is_empty());
        assert_eq!(nothing.total, 0);
    }

    /// The failure case: an empty query is refused, with something a model can
    /// act on rather than an empty result that reads as "nothing exists".
    #[test]
    fn an_empty_query_is_refused_with_an_actionable_message() {
        let (store, _) = store_with_a_project();
        let refused = store.search(&SearchQuery::new("   "));
        match refused {
            Err(Error::Invalid {
                field,
                problem,
                expected,
                ..
            }) => {
                assert_eq!(field, "query");
                assert!(problem.contains("empty"));
                assert!(
                    expected.contains("specline_get"),
                    "the error should say what to do instead, not only what was wrong"
                );
            }
            other => panic!("an empty query should be refused, got {other:?}"),
        }
    }

    /// `bm25()` is negative and lower-is-better. Asserted by identity, because
    /// the wrong sign produces an ordering that looks entirely plausible.
    #[test]
    fn the_best_match_comes_first() {
        let (store, project) = store_with_a_project();
        let best = add_task(
            &store,
            &project,
            "The keyword index is rebuilt on every search",
            "rebuilding the keyword index costs 217ms on the first search after a write",
        );
        let weaker = add_task(
            &store,
            &project,
            "Roadmap",
            "an index of everything planned for the autumn",
        );

        let found = store.search(&SearchQuery::new("keyword index")).unwrap();
        let order = ids(&found);
        assert_eq!(
            order.first().map(String::as_str),
            Some(best.as_str()),
            "the row matching both terms should rank first, got {order:?}"
        );
        assert!(
            order.contains(&weaker.as_str().to_owned()),
            "the weaker match should still be returned, got {order:?}"
        );
    }

    #[test]
    fn a_project_filter_and_a_type_filter_both_bite() {
        let (store, specline) = store_with_a_project();
        let other = add_project(&store, "widgets", "Widgets");
        let mine = add_task(
            &store,
            &specline,
            "Ship the widget",
            "a widget for the board",
        );
        let theirs = add_task(&store, &other, "Widget parity", "another widget entirely");

        let all = store.search(&SearchQuery::new("widget")).unwrap();
        assert_eq!(all.items.len(), 3, "the other project's name matches too");

        let scoped = store
            .search(&SearchQuery {
                project_id: Some(specline.clone()),
                ..SearchQuery::new("widget")
            })
            .unwrap();
        assert_eq!(ids(&scoped), vec![mine.as_str()]);

        let typed = store
            .search(&SearchQuery {
                entity_types: vec![EntityType::Task],
                ..SearchQuery::new("widget")
            })
            .unwrap();
        let mut found = ids(&typed);
        found.sort();
        let mut expected = vec![mine.as_str().to_owned(), theirs.as_str().to_owned()];
        expected.sort();
        assert_eq!(found, expected, "the project row should have been excluded");

        // A search restricted to a type nothing in the store carries is an
        // empty answer, not an unfiltered one.
        let none = store
            .search(&SearchQuery {
                entity_types: vec![EntityType::Metric],
                ..SearchQuery::new("widget")
            })
            .unwrap();
        assert!(none.items.is_empty());
    }

    /// Search degrades without an embedder; it does not fail.
    #[test]
    fn a_store_with_no_embedder_still_returns_keyword_hits() {
        let (mut store, project) = store_with_a_project();
        add_spec(
            &mut store,
            &project,
            "Storage",
            "one SQLite file holds the rows and the documents",
            // Deliberately unembedded: this is a store that has never had an
            // embedder attached at all.
            None,
        );

        let found = store.search(&SearchQuery::new("documents")).unwrap();
        assert_eq!(found.items.len(), 1);
        assert!(
            found
                .items
                .iter()
                .all(|h| h.source == SearchSource::Keyword),
            "without an embedder every hit must be a keyword hit"
        );
    }

    /// The exit criterion: written in one statement, findable in the next, with
    /// nothing having asked the index to catch up.
    #[test]
    fn a_row_is_findable_immediately_and_stays_in_step() {
        let (store, project) = store_with_a_project();
        let task = add_task(
            &store,
            &project,
            "Something",
            "the daemon stalls behind a rebuild",
        );

        assert_eq!(
            ids(&store.search(&SearchQuery::new("stalls")).unwrap()),
            vec![task.as_str()],
            "a row written should be searchable at once, with no rebuild"
        );

        store
            .conn
            .execute(
                "UPDATE tasks SET body = 'now it says something else entirely' WHERE id = ?1",
                [task.as_str()],
            )
            .unwrap();

        assert!(
            store
                .search(&SearchQuery::new("stalls"))
                .unwrap()
                .items
                .is_empty(),
            "the old text should have left the index with the edit"
        );
        assert_eq!(
            ids(&store.search(&SearchQuery::new("entirely")).unwrap()),
            vec![task.as_str()]
        );

        store
            .conn
            .execute(
                "UPDATE tasks SET archived_at = '2026-08-11T01:00:00.000000Z' WHERE id = ?1",
                [task.as_str()],
            )
            .unwrap();
        assert!(
            store
                .search(&SearchQuery::new("entirely"))
                .unwrap()
                .items
                .is_empty(),
            "an archived row must not be offered by search"
        );
    }

    /// The same assertion as above, on a type that actually exercises it.
    ///
    /// The test above archives a **task**, and a task has no row in `documents`
    /// and therefore no vector — so it proves the keyword half and cannot reach
    /// the semantic one. For two years that was the only coverage archiving had,
    /// and underneath it the five prose types had no archive trigger at all:
    /// archiving a spec removed it from nothing. On the live store ten archived
    /// specs, two decisions and a question were still being returned.
    ///
    /// So this one archives a spec, and asserts against both halves separately —
    /// a single `search` assertion would pass if either half went quiet for the
    /// wrong reason.
    #[test]
    fn an_archived_spec_leaves_both_halves_of_the_index() {
        let (mut store, project) = store_with_a_project();
        let spec = add_spec(
            &mut store,
            &project,
            "Penguins",
            "the emperor penguin broods in the antarctic winter",
            Some(std::sync::Arc::new(TopicEmbedder)),
        );

        // "flightless" is nowhere in the text, so a hit for it can only have
        // come from the vector half. Establishing that first is what makes the
        // disappearance below mean something.
        let before = store
            .search_prepared(&SearchQuery::new("flightless"), Some(&TopicEmbedder), None)
            .unwrap();
        assert_eq!(ids(&before.page), vec![spec.as_str()]);
        assert_eq!(
            before.page.items[0].source,
            SearchSource::Semantic,
            "the setup is wrong if BM25 could have found this"
        );

        store
            .conn
            .execute(
                "UPDATE specs SET archived_at = '2026-08-13T01:00:00.000000Z' WHERE id = ?1",
                [spec.as_str()],
            )
            .unwrap();

        assert!(
            store
                .search(&SearchQuery::new("penguin"))
                .unwrap()
                .items
                .is_empty(),
            "an archived spec must leave the keyword index"
        );
        assert!(
            store
                .search_prepared(&SearchQuery::new("flightless"), Some(&TopicEmbedder), None)
                .unwrap()
                .page
                .items
                .is_empty(),
            "an archived spec must leave the semantic index too"
        );

        // And the mechanism, not just the outcome: the vector is gone rather
        // than merely filtered out of one query somewhere.
        let vectors: i64 = store
            .conn
            .query_row(
                "SELECT count(*) FROM documents WHERE entity_id = ?1 AND embedding IS NOT NULL",
                [spec.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(vectors, 0, "archiving must clear the vector, not hide it");
    }

    /// Writing to an archived entity must not quietly un-archive it.
    ///
    /// Nothing in `docs.rs` refuses a revision on an archived entity, and the
    /// indexing triggers fire on the write — so without the guard in
    /// [`super::super::schema`] the next edit puts the row back in front of
    /// people with no event, no error and nothing to notice. Archiving is
    /// one-way, so there is no legitimate reading of this as a restore.
    #[test]
    fn a_revision_on_an_archived_spec_does_not_put_it_back() {
        let (mut store, project) = store_with_a_project();
        let spec = add_spec(
            &mut store,
            &project,
            "Penguins",
            "the emperor penguin broods in the antarctic winter",
            Some(std::sync::Arc::new(TopicEmbedder)),
        );
        store
            .conn
            .execute(
                "UPDATE specs SET archived_at = '2026-08-13T01:00:00.000000Z' WHERE id = ?1",
                [spec.as_str()],
            )
            .unwrap();

        let next = Document::first(
            EntityType::Spec,
            spec.clone(),
            Some(project.clone()),
            "Penguins",
            "the adelie penguin nests on bare rock",
            Actor::Claude,
            stored_now(),
        )
        .unwrap();
        store.write_revision(next).unwrap();

        assert!(
            store
                .search(&SearchQuery::new("adelie"))
                .unwrap()
                .items
                .is_empty(),
            "a revision written to an archived spec must not resurrect it"
        );
    }

    /// Hard constraint 4: a cut list says it was cut, and by how much.
    #[test]
    fn truncation_is_reported_with_a_total() {
        let (store, project) = store_with_a_project();
        for n in 0..5 {
            add_task(
                &store,
                &project,
                &format!("Widget {n}"),
                "a widget among widgets",
            );
        }

        let page = store
            .search(&SearchQuery {
                limit: 2,
                ..SearchQuery::new("widget")
            })
            .unwrap();
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.total, 5);
        assert!(page.truncated);

        let whole = store
            .search(&SearchQuery {
                limit: 20,
                ..SearchQuery::new("widget")
            })
            .unwrap();
        assert_eq!(whole.items.len(), 5);
        assert!(!whole.truncated);
    }

    /// The semantic half earns its place by finding something the keyword half
    /// provably cannot: a row sharing no words with the query.
    #[test]
    fn the_semantic_half_finds_a_row_with_no_words_in_common() {
        let (mut store, project) = store_with_a_project();
        let birds = add_spec(
            &mut store,
            &project,
            "Husbandry",
            "penguin colonies in the southern ocean",
            Some(std::sync::Arc::new(TopicEmbedder)),
        );
        add_spec(
            &mut store,
            &project,
            "Money",
            "the invoice ledger and how it settles",
            Some(std::sync::Arc::new(TopicEmbedder)),
        );

        let query = SearchQuery::new("flightless");
        // Explicitly without an embedder, which is what makes this the keyword
        // half alone. `search` would no longer do: the store carries an
        // embedder now that passages are built by the write path, so the plain
        // call runs both halves and the control would be asserting nothing.
        assert!(
            store.search_with(&query, None).unwrap().items.is_empty(),
            "the keyword half must not find this, or the test proves nothing"
        );

        let found = store.search_with(&query, Some(&TopicEmbedder)).unwrap();
        assert_eq!(ids(&found), vec![birds.as_str()]);
        assert_eq!(found.items[0].source, SearchSource::Semantic);
    }

    /// Vectors from two different models must never be compared.
    ///
    /// The width guard catches a model that changed *dimension*. It cannot
    /// catch one that did not: two 384-wide models produce vectors in
    /// unrelated spaces, and the cosine between them is a number that sorts
    /// perfectly well and means nothing. That is this codebase's signature
    /// failure — a plausible ranking with nothing behind it — and it is the
    /// reason TQ-3 could not be answered with a re-embedding strategy alone.
    #[test]
    fn a_passage_from_another_model_is_not_compared_against_this_one() {
        let (mut store, project) = store_with_a_project();
        let ours = add_spec(
            &mut store,
            &project,
            "Husbandry",
            "penguin colonies in the southern ocean",
            Some(std::sync::Arc::new(TopicEmbedder)),
        );
        let theirs = add_spec(
            &mut store,
            &project,
            "Older",
            "albatross nesting sites",
            Some(std::sync::Arc::new(TopicEmbedder)),
        );

        // Same width, different model: exactly what swapping bge-small for
        // another 384-dimension model leaves behind.
        let touched = store
            .conn
            .execute(
                "UPDATE document_chunks SET embedding_model = 'some-other-model' \
                 WHERE entity_id = ?1",
                [theirs.as_str()],
            )
            .unwrap();
        assert!(touched > 0, "nothing was relabelled, so nothing is proven");

        let found = store
            .search_prepared(&SearchQuery::new("flightless"), Some(&TopicEmbedder), None)
            .unwrap();
        assert_eq!(
            ids(&found.page),
            vec![ours.as_str()],
            "a passage from another model was ranked against this one's query vector"
        );
    }

    /// A store told about an embedder must actually use it when `search` is
    /// called through the trait, rather than only when a caller remembers to
    /// reach for `search_with`.
    ///
    /// This is a regression test for a real gap that shipped for about an hour:
    /// `search` passed a hard `None`, so the semantic half never ran however
    /// the store had been built. Nothing failed. Keyword results kept arriving
    /// and were merely worse, which is the shape of degradation nobody reports
    /// as a bug because everything still looks like it is working.
    #[test]
    fn a_store_given_an_embedder_searches_semantically_through_the_trait() {
        let (store, project) = store_with_a_project();
        let mut store = store.with_embedder(std::sync::Arc::new(TopicEmbedder));

        let birds = add_spec(
            &mut store,
            &project,
            "Husbandry",
            "penguin colonies in the southern ocean",
            Some(std::sync::Arc::new(TopicEmbedder)),
        );

        let query = SearchQuery::new("flightless");
        assert!(
            store.search_with(&query, None).unwrap().items.is_empty(),
            "the keyword half must not find this, or the test proves nothing"
        );

        // No embedder passed at the call site — the store's own is what has to
        // be reached.
        let found = store.search(&query).unwrap();
        assert_eq!(
            ids(&found),
            vec![birds.as_str()],
            "the store's attached embedder was not used by `search`"
        );
    }

    /// Both halves agreeing is the strongest signal available, and it is
    /// reported as such.
    #[test]
    fn a_row_both_halves_found_is_marked_both() {
        let embedder = TopicEmbedder;
        let (mut store, project) = store_with_a_project();
        let spec = add_spec(
            &mut store,
            &project,
            "Storage",
            "the SQLite index is maintained by triggers",
            Some(std::sync::Arc::new(TopicEmbedder)),
        );

        let found = store
            .search_with(&SearchQuery::new("SQLite index"), Some(&embedder))
            .unwrap();
        assert_eq!(ids(&found), vec![spec.as_str()]);
        assert_eq!(found.items[0].source, SearchSource::Both);
    }

    /// A vector of the wrong width must cost recall of one row, not the whole
    /// search. `vec_distance_cosine` errors on a length mismatch, and that
    /// error would otherwise fail the entire query.
    #[test]
    fn an_embedding_of_the_wrong_width_is_skipped_rather_than_failing_the_search() {
        let (mut store, project) = store_with_a_project();
        let good = add_spec(
            &mut store,
            &project,
            "Husbandry",
            "penguin colonies in the southern ocean",
            Some(std::sync::Arc::new(TopicEmbedder)),
        );
        let stale = add_spec(
            &mut store,
            &project,
            "Older",
            "albatross nesting sites",
            Some(std::sync::Arc::new(TopicEmbedder)),
        );

        // As if an older model had written it: two floats where there should
        // be four. On the passage, because that is where vectors live since
        // B-55 — the same corruption on `documents.embedding` now proves
        // nothing, since nothing reads that column.
        let narrow: Vec<u8> = [1.0f32, 0.0].iter().flat_map(|f| f.to_le_bytes()).collect();
        let touched = store
            .conn
            .execute(
                "UPDATE document_chunks SET embedding = ?1 WHERE entity_id = ?2",
                rusqlite::params![narrow, stale.as_str()],
            )
            .unwrap();
        assert!(
            touched > 0,
            "nothing was corrupted, so the guard is not being exercised"
        );

        let found = store
            .search_with(&SearchQuery::new("flightless"), Some(&TopicEmbedder))
            .unwrap();
        assert_eq!(
            ids(&found),
            vec![good.as_str()],
            "the well-formed row should still be found"
        );
    }

    #[test]
    fn since_and_until_filter_by_when_the_row_was_created() {
        let (store, project) = store_with_a_project();

        let old = add_task_created_at(
            &store,
            &project,
            "An old widget",
            "a widget from last year",
            DateTime::from_timestamp(1_600_000_000, 0).unwrap_or_default(),
        );
        let recent = add_task(&store, &project, "A new widget", "a widget from today");

        let cutoff = DateTime::from_timestamp(1_700_000_000, 0).unwrap_or_default();

        let since = store
            .search(&SearchQuery {
                since: Some(cutoff),
                ..SearchQuery::new("widget")
            })
            .unwrap();
        assert!(ids(&since).contains(&recent.as_str().to_owned()));
        assert!(!ids(&since).contains(&old.as_str().to_owned()));

        let until = store
            .search(&SearchQuery {
                until: Some(cutoff),
                ..SearchQuery::new("widget")
            })
            .unwrap();
        assert_eq!(ids(&until), vec![old.as_str()]);
    }

    /// The trait and the inherent method must be the same search. The delegation
    /// is the place a `self.search(…)` typo would recurse forever, so it is
    /// worth calling through the trait once.
    #[test]
    fn the_trait_method_reaches_the_same_search() {
        let (store, project) = store_with_a_project();
        let task = add_task(&store, &project, "The board", "a widget for the board");
        let via_trait = DocumentStore::search(&store, &SearchQuery::new("widget")).unwrap();
        assert_eq!(ids(&via_trait), vec![task.as_str()]);
    }

    /// Both halves ran, so an empty answer would have been a fact about the
    /// store. This is the case every other report has to be distinguishable
    /// from, which is why it is asserted rather than assumed.
    #[test]
    fn a_search_with_a_model_reports_both_halves() {
        let (mut store, project) = store_with_a_project();
        add_spec(
            &mut store,
            &project,
            "Penguins",
            "the emperor penguin broods in the antarctic winter",
            Some(std::sync::Arc::new(TopicEmbedder)),
        );

        let found = store
            .search_prepared(&SearchQuery::new("flightless"), Some(&TopicEmbedder), None)
            .unwrap();
        assert!(found.report.complete());
        assert_eq!(found.report.ran(), vec!["keyword", "semantic"]);
        assert_eq!(found.report.keyword.why(), None);
        assert_eq!(found.report.semantic.why(), None);
    }

    /// The failure case, and the one this whole report exists for: no model, so
    /// the semantic half never touched the database, and the results say so.
    ///
    /// The hits are identical to a healthy search's — that is the point. There
    /// is nothing in `items` for a caller to notice, which is how a store with
    /// a fully populated vector index served by a daemon with no model went
    /// months looking exactly like a store that had been asked and had little
    /// to say.
    #[test]
    fn a_search_with_no_model_says_the_semantic_half_did_not_run() {
        let (mut store, project) = store_with_a_project();
        add_spec(
            &mut store,
            &project,
            "Penguins",
            "the emperor penguin broods in the antarctic winter",
            Some(std::sync::Arc::new(TopicEmbedder)),
        );

        let found = store
            .search_prepared(&SearchQuery::new("penguin"), None, None)
            .unwrap();
        assert!(
            !found.page.items.is_empty(),
            "the keyword half still answers, which is what makes this quiet"
        );
        assert!(!found.report.complete());
        assert_eq!(found.report.ran(), vec!["keyword"]);
        assert_eq!(found.report.semantic, HalfStatus::NoModel);
        assert!(
            found
                .report
                .semantic
                .why()
                .unwrap_or_default()
                .contains("embedding model"),
            "the reason has to name what is missing, not only that something is"
        );
    }

    /// A filter naming only types that have no prose is a narrowing, not a
    /// degradation — but it still silences a half, and a caller comparing two
    /// searches deserves to know which of them asked both indexes.
    #[test]
    fn a_type_filter_with_no_prose_in_it_reports_the_semantic_half_out_of_scope() {
        let (store, project) = store_with_a_project();
        let task = add_task(&store, &project, "The board", "a widget for the board");

        let found = store
            .search_prepared(
                &SearchQuery {
                    entity_types: vec![EntityType::Task],
                    ..SearchQuery::new("widget")
                },
                Some(&TopicEmbedder),
                None,
            )
            .unwrap();
        assert_eq!(
            ids(&found.page),
            vec![task.as_str()],
            "the keyword half still answers for a type it covers"
        );
        assert_eq!(found.report.semantic, HalfStatus::NoTypesInScope);
        assert_eq!(found.report.keyword, HalfStatus::Ran);
    }

    /// And the mirror image: text with no words in it leaves the keyword half
    /// with nothing to match, while the semantic half is perfectly able to
    /// answer. Empty text is refused outright, so this is the narrowest input
    /// that still reaches the search.
    #[test]
    fn text_with_no_words_reports_the_keyword_half_had_no_terms() {
        let (store, _project) = store_with_a_project();
        let found = store
            .search_prepared(&SearchQuery::new("!!! ???"), Some(&TopicEmbedder), None)
            .unwrap();
        assert_eq!(found.report.keyword, HalfStatus::NoTerms);
        assert_eq!(found.report.semantic, HalfStatus::Ran);
    }
}
