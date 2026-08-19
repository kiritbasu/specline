//! Document revisions and blobs, in the same file as the rows they describe.
//!
//! The prose half of the store, and the point of the whole move to one file: a
//! revision, the header row it belongs to and an image attached to it are three
//! writes in one transaction, so they cannot half-land. Across two engines they
//! could, and `fsck`'s `orphan_document` check was the only way to find out —
//! a Lance document pointing at a DuckDB row that was never written. That check
//! is still here, and it should now only ever fire for a revision written
//! against an id that never existed. `write_revision` guards that case itself,
//! because `documents.entity_id` is polymorphic and no foreign key can express
//! "one of five tables".
//!
//! # Why there is no `impl DocumentStore for Store` in this file
//!
//! The trait has seven methods and this file has six of them, as inherent
//! methods named exactly as the trait names them. The seventh is `search`, and
//! since a trait impl missing a method does not compile, the `impl` block has
//! to live where the last method is — in [`super::search`], which delegates
//! each of these six in one line.
//!
//! **Those delegations use the fully-qualified form**, `Store::revision(self,
//! …)` rather than `self.revision(…)`. Inside a trait impl the short form
//! resolves to the trait method being written, not to the inherent one, and the
//! result is a function that calls itself until the stack runs out — at
//! runtime, with no warning at the point it was written.

use super::Store;
use super::rows::{TIMESTAMP_FORMAT, parse_ts};
use crate::store::Blob;
use crate::{
    Actor, BlobId, DocId, DocStatus, Document, DocumentDiff, EntityId, Error, Result, Surface,
};
use chrono::{DateTime, Utc};
use rusqlite::types::Value;
use rusqlite::{Connection, Row, params, params_from_iter};

/// The columns of `documents`, in insert order.
const DOC_COLS: &str = "doc_id, entity_type, entity_id, project_id, version, parent_version, \
                        title, body, body_hash, media_ref, status, author, session_id, surface, \
                        created_at, embedding, embedding_model, embedding_version";

/// The placeholders for [`DOC_COLS`], numbered so the binding order is visible
/// in the statement rather than only in the parameter vector.
const DOC_VALUES: &str = "?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, \
                          ?16, ?17, ?18";

/// The `SELECT` list for reading revisions back.
///
/// `embedding` is deliberately absent — see [`read_document`].
const DOC_SELECT: &str = "SELECT doc_id, entity_type, entity_id, project_id, version, \
                          parent_version, title, body, body_hash, media_ref, status, author, \
                          session_id, surface, created_at, embedding_model, embedding_version \
                          FROM documents";

/// Render a timestamp for storage.
fn render_ts(v: DateTime<Utc>) -> String {
    v.format(TIMESTAMP_FORMAT).to_string()
}

/// An embedding as the raw little-endian f32 bytes the column stores.
///
/// Bytes rather than a typed column because `sqlite-vec` is 0.1.9 and its
/// author says to expect breaking changes. Owning the representation means
/// replacing the vector index is a new virtual table populated from this
/// column, not an embedding run over the whole corpus again.
fn embedding_bytes(embedding: &[f32]) -> Vec<u8> {
    embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Bind an optional string.
fn os(v: Option<String>) -> Value {
    v.map(Value::Text).unwrap_or(Value::Null)
}

/// Bind an i32 into SQLite's single 64-bit integer type.
fn i(v: i32) -> Value {
    Value::Integer(i64::from(v))
}

/// Rebuild a revision from a row of [`DOC_SELECT`].
///
/// Columns are addressed by name, never by index: an offset that drifts by one
/// produces a document where every field holds its neighbour's value, and TEXT
/// against TEXT would not complain.
fn read_document(row: &Row<'_>) -> Result<Document> {
    let e = |c: &'static str| {
        let context = format!("read column `{c}` of `documents`");
        move |source| Error::Storage { context, source }
    };
    let created_at: String = row.get("created_at").map_err(e("created_at"))?;

    Ok(Document {
        doc_id: DocId::parse(&row.get::<_, String>("doc_id").map_err(e("doc_id"))?)?,
        entity_type: crate::EntityType::parse(
            &row.get::<_, String>("entity_type")
                .map_err(e("entity_type"))?,
        )?,
        entity_id: EntityId::parse(&row.get::<_, String>("entity_id").map_err(e("entity_id"))?)?,
        project_id: match row
            .get::<_, Option<String>>("project_id")
            .map_err(e("project_id"))?
        {
            Some(p) if !p.is_empty() => Some(EntityId::parse(&p)?),
            _ => None,
        },
        version: row.get::<_, i32>("version").map_err(e("version"))?,
        parent_version: row
            .get::<_, Option<i32>>("parent_version")
            .map_err(e("parent_version"))?,
        title: row.get::<_, String>("title").map_err(e("title"))?,
        body: row.get::<_, String>("body").map_err(e("body"))?,
        body_hash: row.get::<_, String>("body_hash").map_err(e("body_hash"))?,
        media_ref: row
            .get::<_, Option<String>>("media_ref")
            .map_err(e("media_ref"))?,
        status: DocStatus::parse(&row.get::<_, String>("status").map_err(e("status"))?)?,
        author: Actor::parse(&row.get::<_, String>("author").map_err(e("author"))?)?,
        session_id: row
            .get::<_, Option<String>>("session_id")
            .map_err(e("session_id"))?,
        surface: match row
            .get::<_, Option<String>>("surface")
            .map_err(e("surface"))?
        {
            Some(s) => Some(Surface::parse(&s)?),
            None => None,
        },
        created_at: parse_ts("documents", "created_at", &created_at)?,
        // The vector is deliberately not read back. It is 384 floats per row
        // that no caller has ever needed, and reading it would make listing a
        // document's history cost more than the history is worth.
        embedding: None,
        embedding_model: row
            .get::<_, Option<String>>("embedding_model")
            .map_err(e("embedding_model"))?
            .unwrap_or_default(),
        embedding_version: row
            .get::<_, Option<i32>>("embedding_version")
            .map_err(e("embedding_version"))?
            .unwrap_or(0),
    })
}

/// Write a blob through whatever connection is handed in.
///
/// Separate from [`Store::put_blob`] so that a caller holding a
/// transaction — the daemon writing a design and its screenshot together — can
/// use the same statement. A `rusqlite::Transaction` derefs to `Connection`, so
/// `&tx` is accepted here and the blob commits or vanishes with everything else
/// in that transaction.
pub(super) fn insert_blob_in(conn: &Connection, blob: &Blob) -> Result<()> {
    conn.execute(
        "INSERT INTO blobs \
           (blob_id, entity_id, project_id, media_type, byte_length, sha256, bytes, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            blob.blob_id.as_str(),
            blob.entity_id.as_ref().map(EntityId::as_str),
            blob.project_id.as_ref().map(EntityId::as_str),
            blob.media_type,
            blob.bytes.len() as i64,
            blob.sha256,
            // Bound by reference, not cloned. A 5 MB screenshot copied on the
            // way to the binding is 5 MB of memcpy that buys nothing.
            blob.bytes,
            render_ts(blob.created_at),
        ],
    )
    .map_err(Error::storage(format!("store the blob {}", blob.blob_id)))?;
    Ok(())
}

/// Write one revision through whatever connection is handed in.
///
/// The whole of `write_revision`'s work, minus the transaction — because the
/// composite create needs the same steps inside a transaction it already holds,
/// and a second copy of "demote, insert, advance the header, record the event"
/// is exactly the sort of duplicate that drifts a column at a time.
///
/// Everything that changes state is here: demoting the old current revision,
/// inserting the new one, advancing the header's `current_doc_version` and
/// appending the `Revised` event. *History:* under DuckDB-and-Lance those were
/// writes across two engines with nothing that could bracket them, so a crash
/// between two of them left a document whose header disagreed with it.
pub(super) fn write_revision_in(
    conn: &Connection,
    embedder: Option<&dyn crate::Embedder>,
    mut document: Document,
) -> Result<Document> {
    let table = document.entity_type.table();
    // Only five types carry prose, and only their tables have a
    // `current_doc_version` column. A hand-built `Document` for a task
    // would otherwise fail on the header update with "no such column",
    // after the revision had already been inserted.
    if !document.entity_type.has_document() {
        return Err(Error::Invariant {
            operation: format!("write a revision of {}", document.entity_id),
            problem: format!(
                "{} has no prose body; only spec, decision, question, feedback and \
                 design have documents",
                document.entity_type
            ),
        });
    }

    // `documents.entity_id` names a row in one of five tables, so no
    // foreign key can enforce it and this check is the only thing standing
    // between a typo and a document nothing can ever reach.
    let exists: i64 = conn
        .query_row(
            &format!("SELECT count(*) FROM {table} WHERE id = ?1"),
            [document.entity_id.as_str()],
            |r| r.get(0),
        )
        .map_err(Error::storage(format!(
            "check that {} exists before writing its document",
            document.entity_id
        )))?;
    if exists == 0 {
        return Err(Error::Invariant {
            operation: format!("write a revision of {}", document.entity_id),
            problem: format!(
                "no {} exists with that id; create the entity before writing its body",
                document.entity_type
            ),
        });
    }

    if let Some(current) = current_revision_in(conn, &document.entity_id)?
        && current.body_hash == document.body_hash
    {
        return Ok(current);
    }

    let previous = max_version_in(conn, &document.entity_id)?;
    document.version = previous + 1;
    document.parent_version = if previous == 0 { None } else { Some(previous) };
    document.status = DocStatus::Current;

    // `documents.embedding` is deliberately not filled in any more (B-55).
    // Passages carry the vectors now, and a whole-document vector beside them
    // is a second copy of the same claim with nothing keeping the two in step.
    // The column stays until a later migration takes it.

    let params: Vec<Value> = vec![
        Value::Text(document.doc_id.as_str().to_owned()),
        Value::Text(document.entity_type.as_str().to_owned()),
        Value::Text(document.entity_id.as_str().to_owned()),
        os(document.project_id.as_ref().map(|p| p.as_str().to_owned())),
        i(document.version),
        document.parent_version.map(i).unwrap_or(Value::Null),
        Value::Text(document.title.clone()),
        Value::Text(document.body.clone()),
        Value::Text(document.body_hash.clone()),
        os(document.media_ref.clone()),
        Value::Text(document.status.as_str().to_owned()),
        Value::Text(document.author.as_str().to_owned()),
        os(document.session_id.clone()),
        os(document.surface.map(|s| s.as_str().to_owned())),
        Value::Text(render_ts(document.created_at)),
        document
            .embedding
            .as_ref()
            .map(|v| Value::Blob(embedding_bytes(v)))
            .unwrap_or(Value::Null),
        Value::Text(document.embedding_model.clone()),
        i(document.embedding_version),
    ];

    // Demote first, so there is never a moment with two current revisions
    // — not even one another connection could observe, since the demotion
    // and the insert commit together.
    conn.execute(
        "UPDATE documents SET status = 'superseded' \
         WHERE entity_id = ?1 AND status = 'current'",
        [document.entity_id.as_str()],
    )
    .map_err(Error::storage(format!(
        "supersede the previous revision of {}",
        document.entity_id
    )))?;

    conn.execute(
        &format!("INSERT INTO documents ({DOC_COLS}) VALUES ({DOC_VALUES})"),
        params_from_iter(params),
    )
    .map_err(Error::storage(format!(
        "write revision {} of {}",
        document.version, document.entity_id
    )))?;

    conn.execute(
        &format!("UPDATE {table} SET current_doc_version = ?1 WHERE id = ?2"),
        params![document.version, document.entity_id.as_str()],
    )
    .map_err(Error::storage(format!(
        "advance current_doc_version on {}",
        document.entity_id
    )))?;

    // The event, inside the same transaction as the revision it describes.
    //
    // `Action::Revised` was declared from the beginning and never once
    // constructed, so a session that only wrote prose left no trace at all:
    // not in the changelog, which is derived from the event log, and not in
    // the app's live feed. Whole sessions of work were invisible, and
    // nothing looked broken — the feed simply had less in it.
    //
    // Provenance comes off the document rather than from a parameter. The
    // author, session and surface were already decided at the boundary and
    // recorded on the revision; taking them from anywhere else would let
    // the row and its event disagree about who wrote it.
    let provenance = crate::Provenance {
        actor: document.author,
        session_id: document.session_id.clone(),
        surface: document.surface,
    };
    let summary = format!(
        "revised {} “{}” to v{}",
        document.entity_type, document.title, document.version
    );
    crate::store::entity::append_event_inner(
        conn,
        crate::NewEvent::new(document.entity_id.clone(), crate::Action::Revised, summary)
            .in_project(document.project_id.clone())
            .with_meta(serde_json::json!({
                "version": document.version,
                "doc_id": document.doc_id.as_str(),
            })),
        &provenance,
        document.created_at,
    )?;

    write_chunks_in(
        conn,
        embedder,
        ChunkSource {
            doc_id: document.doc_id.as_str(),
            entity_id: document.entity_id.as_str(),
            entity_type: document.entity_type.as_str(),
            project_id: document
                .project_id
                .as_ref()
                .map(|p| p.as_str())
                .unwrap_or(""),
            title: &document.title,
            body: &document.body,
            body_hash: &document.body_hash,
        },
    )?;

    Ok(document)
}

/// The columns a revision's passages are built from.
///
/// Borrowed rather than a `&Document`, because the re-embed pass has a row and
/// not a document, and inventing a half-populated `Document` to satisfy a
/// signature is how a field that was never read becomes a field that is. Seven
/// borrowed strs is the honest description of what chunking needs.
pub(super) struct ChunkSource<'a> {
    pub doc_id: &'a str,
    pub entity_id: &'a str,
    pub entity_type: &'a str,
    pub project_id: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub body_hash: &'a str,
}

/// Split the revision into passages and embed each one.
///
/// Runs inside the caller's transaction, so a revision and its passages become
/// visible together or not at all. There is no moment when a document is
/// current and its passages describe the version before it.
///
/// Does nothing without an embedder, and that is the ordinary case for the CLI
/// and the whole test suite. A revision written with no model attached is not
/// broken — it is keyword-searchable and invisible to the semantic half, which
/// is what `specline reembed --missing` exists to fix and what `doctor` reports.
///
/// A model that fails is logged and skipped rather than failing the write. The
/// document is the thing worth keeping; the passages can be rebuilt from it at
/// any time, which is the invariant the whole delete-rather-than-archive
/// carve-out rests on.
pub(super) fn write_chunks_in(
    conn: &Connection,
    embedder: Option<&dyn crate::Embedder>,
    document: ChunkSource<'_>,
) -> Result<usize> {
    let Some(embedder) = embedder else {
        return Ok(0);
    };
    // Belt and braces against the superseded trigger: a rewritten revision
    // reuses neither doc_id nor ordinal, but an interrupted re-embed could
    // leave passages behind and this is cheaper than reasoning about it.
    conn.execute(
        "DELETE FROM document_chunks WHERE doc_id = ?1",
        [document.doc_id],
    )
    .map_err(Error::storage(format!(
        "clear the old passages of {}",
        document.entity_id
    )))?;

    let chunks = crate::chunk::split(document.body);
    if chunks.is_empty() {
        return Ok(0);
    }
    let texts: Vec<String> = chunks
        .iter()
        .map(|c| c.embed_text(document.title))
        .collect();
    let vectors = match embedder.embed(&texts) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                entity_id = %document.entity_id,
                error = %e,
                "embedding failed; the revision is stored without passages and \
                 `specline reembed --missing` will pick it up"
            );
            return Ok(0);
        }
    };

    for (chunk, vector) in chunks.iter().zip(vectors.iter()) {
        conn.execute(
            "INSERT INTO document_chunks (
                 chunk_id, doc_id, entity_id, entity_type, project_id, ordinal,
                 heading_path, char_start, char_end, text, body_hash,
                 embedding, embedding_model, embedding_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            rusqlite::params![
                crate::ChunkId::generate().as_str(),
                document.doc_id,
                document.entity_id,
                document.entity_type,
                document.project_id,
                chunk.ordinal as i64,
                chunk.heading_path.as_str(),
                chunk.start as i64,
                chunk.end as i64,
                chunk.text.as_str(),
                document.body_hash,
                embedding_bytes(vector),
                embedder.model_name(),
                1i64,
            ],
        )
        .map_err(Error::storage(format!(
            "write passage {} of {}",
            chunk.ordinal, document.entity_id
        )))?;
    }
    Ok(chunks.len())
}

/// The highest revision number recorded for an entity, or zero.
fn max_version_in(conn: &Connection, entity_id: &EntityId) -> Result<i32> {
    let n: Option<i32> = conn
        .query_row(
            "SELECT max(version) FROM documents WHERE entity_id = ?1",
            [entity_id.as_str()],
            |r| r.get(0),
        )
        .map_err(Error::storage(format!(
            "find the latest revision of {entity_id}"
        )))?;
    Ok(n.unwrap_or(0))
}

/// The current revision of an entity, read through a caller's connection.
///
/// By version rather than by `status = 'current'`, for the reason
/// [`Store::revision`] gives: the two agree, and asking by version means a store
/// whose statuses have drifted still hands back its newest revision rather than
/// nothing — and "nothing" reads as "this document does not exist".
fn current_revision_in(conn: &Connection, entity_id: &EntityId) -> Result<Option<Document>> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {DOC_COLS} FROM documents WHERE entity_id = ?1 ORDER BY version DESC LIMIT 1"
        ))
        .map_err(Error::storage("prepare a revision lookup"))?;
    let mut rows = stmt
        .query([entity_id.as_str()])
        .map_err(Error::storage("run a revision lookup"))?;
    match rows.next().map_err(Error::storage("read a revision row"))? {
        Some(row) => Ok(Some(read_document(row)?)),
        None => Ok(None),
    }
}

impl Store {
    /// Append a revision, and return it with the version the store assigned.
    ///
    /// The returned version is the one that was written, not the one the
    /// caller asked for: `specline_write_doc` reports it back, the mirror names it
    /// in the generated file's banner, and a caller that trusted its own guess
    /// would be describing a revision that does not exist.
    ///
    /// Writing content identical to the current revision is a no-op that
    /// returns the existing revision. That is what makes `specline generate` safe
    /// to run repeatedly — it regenerates every file whether or not anything
    /// changed, and without this the history would grow by one per run per
    /// document.
    ///
    /// Everything that changes state happens in one transaction: demoting the
    /// old current revision, inserting the new one, and advancing the header's
    /// `current_doc_version`. *History:* under DuckDB-and-Lance those were three
    /// writes across two engines with nothing that could bracket them, so a
    /// crash between the second and the third left a document whose header
    /// disagreed with it. That is what the transaction is here to make
    /// impossible.
    pub fn write_revision(&mut self, document: Document) -> Result<Document> {
        let embedder = self.embedder.clone();
        let tx = self
            .conn
            .transaction()
            .map_err(Error::storage("begin a revision write"))?;
        let written = write_revision_in(&tx, embedder.as_deref(), document)?;
        tx.commit().map_err(Error::storage(format!(
            "commit revision {} of {}",
            written.version, written.entity_id
        )))?;
        Ok(written)
    }

    /// Fetch a revision — the current one if `version` is `None`.
    ///
    /// The current one is found by version rather than by `status = 'current'`.
    /// The two agree, and the invariant that they agree is asserted in the
    /// tests; asking by version means a store whose statuses have somehow
    /// drifted still hands back its newest revision rather than nothing, and
    /// "nothing" is the answer that reads as "this document does not exist".
    pub fn revision(&self, entity_id: &EntityId, version: Option<i32>) -> Result<Option<Document>> {
        let (clause, params): (&str, Vec<Value>) = match version {
            Some(v) => (
                "WHERE entity_id = ?1 AND version = ?2",
                vec![Value::Text(entity_id.as_str().to_owned()), i(v)],
            ),
            None => (
                "WHERE entity_id = ?1 ORDER BY version DESC LIMIT 1",
                vec![Value::Text(entity_id.as_str().to_owned())],
            ),
        };

        let mut stmt = self
            .conn
            .prepare(&format!("{DOC_SELECT} {clause}"))
            .map_err(Error::storage(format!(
                "prepare a revision read for {entity_id}"
            )))?;
        let mut rows = stmt
            .query(params_from_iter(params))
            .map_err(Error::storage(format!("read a revision of {entity_id}")))?;

        match rows.next().map_err(Error::storage("read a document row"))? {
            Some(row) => Ok(Some(read_document(row)?)),
            None => Ok(None),
        }
    }

    /// Every revision of a document, oldest first.
    ///
    /// Oldest first because that is the order a history reads in, and because
    /// the app's revision list and `specline_get`'s diff both index from it.
    pub fn revisions(&self, entity_id: &EntityId) -> Result<Vec<Document>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "{DOC_SELECT} WHERE entity_id = ?1 ORDER BY version ASC"
            ))
            .map_err(Error::storage(format!(
                "prepare a history read for {entity_id}"
            )))?;
        let mut rows = stmt
            .query([entity_id.as_str()])
            .map_err(Error::storage(format!("read the history of {entity_id}")))?;

        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(Error::storage("read a document row"))? {
            out.push(read_document(row)?);
        }
        Ok(out)
    }

    /// A unified diff between two revisions.
    ///
    /// A store operation rather than a rendering one: REQ-2 is "what changed
    /// between these two versions", and an MCP caller asking that should not
    /// have to fetch both bodies and diff them itself — nor should two surfaces
    /// each grow their own idea of what a diff looks like.
    pub fn diff(&self, entity_id: &EntityId, from: i32, to: i32) -> Result<DocumentDiff> {
        let fetch = |v: i32| -> Result<Document> {
            self.revision(entity_id, Some(v))?
                .ok_or_else(|| Error::Invalid {
                    entity_type: entity_id.entity_type(),
                    field: "version".to_owned(),
                    problem: format!("{entity_id} has no revision {v}"),
                    expected:
                        "a revision number returned by specline_get, or omit it for the current one"
                            .to_owned(),
                })
        };
        let a = fetch(from)?;
        let b = fetch(to)?;

        let diff = similar::TextDiff::from_lines(&a.body, &b.body);
        let mut unified = String::new();
        let (mut added, mut removed) = (0usize, 0usize);
        for change in diff.iter_all_changes() {
            let sign = match change.tag() {
                similar::ChangeTag::Delete => {
                    removed += 1;
                    '-'
                }
                similar::ChangeTag::Insert => {
                    added += 1;
                    '+'
                }
                similar::ChangeTag::Equal => ' ',
            };
            unified.push(sign);
            unified.push_str(change.value());
            if !change.value().ends_with('\n') {
                unified.push('\n');
            }
        }

        Ok(DocumentDiff {
            entity_id: entity_id.clone(),
            from_version: from,
            to_version: to,
            unified,
            added,
            removed,
        })
    }

    /// Store bytes, and return the id they are addressed by.
    ///
    /// The hash is whatever [`Blob::new`] computed from the bytes — it is never
    /// taken from a caller, because a hash nobody has checked is not a content
    /// address, it is a claim.
    pub fn put_blob(&mut self, blob: Blob) -> Result<BlobId> {
        insert_blob_in(&self.conn, &blob)?;
        Ok(blob.blob_id)
    }

    /// Fetch bytes. `None` when nothing is stored under that id.
    pub fn get_blob(&self, blob_id: &BlobId) -> Result<Option<Blob>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT blob_id, entity_id, project_id, media_type, sha256, bytes, created_at \
                 FROM blobs WHERE blob_id = ?1",
            )
            .map_err(Error::storage("prepare a blob read"))?;
        let mut rows = stmt
            .query([blob_id.as_str()])
            .map_err(Error::storage(format!("read the blob {blob_id}")))?;

        let e = |c: &'static str| {
            let context = format!("read column `{c}` of `blobs`");
            move |source| Error::Storage { context, source }
        };
        match rows.next().map_err(Error::storage("read a blob row"))? {
            Some(row) => {
                let created_at: String = row.get("created_at").map_err(e("created_at"))?;
                Ok(Some(Blob {
                    blob_id: BlobId::parse(
                        &row.get::<_, String>("blob_id").map_err(e("blob_id"))?,
                    )?,
                    entity_id: match row
                        .get::<_, Option<String>>("entity_id")
                        .map_err(e("entity_id"))?
                    {
                        Some(x) => Some(EntityId::parse(&x)?),
                        None => None,
                    },
                    project_id: match row
                        .get::<_, Option<String>>("project_id")
                        .map_err(e("project_id"))?
                    {
                        Some(x) => Some(EntityId::parse(&x)?),
                        None => None,
                    },
                    media_type: row
                        .get::<_, String>("media_type")
                        .map_err(e("media_type"))?,
                    sha256: row.get::<_, String>("sha256").map_err(e("sha256"))?,
                    bytes: row.get::<_, Vec<u8>>("bytes").map_err(e("bytes"))?,
                    created_at: parse_ts("blobs", "created_at", &created_at)?,
                }))
            }
            None => Ok(None),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::store::rows::spec_for;
    use crate::store::rows::{insert_params, insert_stmt};
    use crate::{Entity, EntityType, Project, Spec};

    /// Timestamps whose sub-second digits the column can hold exactly.
    ///
    /// `Utc::now()` is nanosecond-precise and the format is six digits, so an
    /// un-truncated timestamp fails a round trip for a reason that has nothing
    /// to do with what is under test.
    fn stored_now() -> DateTime<Utc> {
        DateTime::from_timestamp_micros(Utc::now().timestamp_micros()).unwrap_or_default()
    }

    /// Put an entity row in the store, the way the entity half will.
    fn insert_entity(store: &Store, entity: &Entity) {
        let spec = spec_for(entity.entity_type());
        store
            .conn
            .execute(&insert_stmt(&spec), params_from_iter(insert_params(entity)))
            .unwrap();
    }

    /// A store holding a project and a spec, and the spec's id.
    ///
    /// A revision cannot be written without its header row — that check is the
    /// one thing `documents.entity_id` cannot get from a foreign key — so
    /// every test here needs one.
    fn store_with_a_spec() -> (Store, EntityId) {
        let store = Store::in_memory().unwrap();
        let project = Project::new("specline", "Specline");
        let project_id = project.id.clone();
        insert_entity(&store, &project.into());

        let spec = Spec::new(project_id, "Storage");
        let spec_id = spec.id.clone();
        insert_entity(&store, &spec.into());
        (store, spec_id)
    }

    fn draft(entity_id: &EntityId, body: &str) -> Document {
        Document::first(
            EntityType::Spec,
            entity_id.clone(),
            None,
            "Storage",
            body,
            Actor::Claude,
            stored_now(),
        )
        .unwrap()
    }

    /// How many revisions of this entity claim to be current.
    fn current_count(store: &Store, entity_id: &EntityId) -> i64 {
        store
            .conn
            .query_row(
                "SELECT count(*) FROM documents WHERE entity_id = ?1 AND status = 'current'",
                [entity_id.as_str()],
                |r| r.get(0),
            )
            .unwrap()
    }

    #[test]
    fn three_revisions_round_trip_and_only_the_last_is_current() {
        let (mut store, id) = store_with_a_spec();
        for body in ["one", "two", "three"] {
            store.write_revision(draft(&id, body)).unwrap();
        }

        for (version, body) in [(1, "one"), (2, "two"), (3, "three")] {
            let back = store.revision(&id, Some(version)).unwrap().unwrap();
            assert_eq!(back.body, body);
            assert_eq!(back.version, version);
            assert_eq!(back.title, "Storage");
            assert_eq!(back.author, Actor::Claude);
        }

        assert_eq!(
            store.revision(&id, Some(1)).unwrap().unwrap().status,
            DocStatus::Superseded
        );
        assert_eq!(
            store.revision(&id, Some(2)).unwrap().unwrap().status,
            DocStatus::Superseded
        );

        let current = store.revision(&id, None).unwrap().unwrap();
        assert_eq!(current.version, 3);
        assert_eq!(current.status, DocStatus::Current);
        assert_eq!(current.parent_version, Some(2));
    }

    /// The version the caller hoped for is not the version it gets. Callers
    /// report the returned one back to a human, so a store that quietly
    /// honoured the request would have them describing a revision that is not
    /// there.
    #[test]
    fn the_returned_version_is_the_one_the_store_assigned() {
        let (mut store, id) = store_with_a_spec();
        store.write_revision(draft(&id, "one")).unwrap();

        let mut hopeful = draft(&id, "two");
        hopeful.version = 7;
        hopeful.parent_version = Some(6);

        let written = store.write_revision(hopeful).unwrap();
        assert_eq!(written.version, 2);
        assert_eq!(written.parent_version, Some(1));
    }

    /// `specline generate` regenerates every file whether or not anything changed.
    /// Without this the history would grow by one per run per document.
    #[test]
    fn identical_content_is_not_a_new_revision() {
        let (mut store, id) = store_with_a_spec();
        let first = store.write_revision(draft(&id, "unchanged")).unwrap();
        let again = store.write_revision(draft(&id, "unchanged")).unwrap();

        assert_eq!(again.version, first.version);
        assert_eq!(again.doc_id, first.doc_id, "a no-op must not mint a doc id");
        assert_eq!(store.revisions(&id).unwrap().len(), 1);

        // And a real change still lands, so the short-circuit is not simply
        // refusing every second write.
        let changed = store.write_revision(draft(&id, "changed")).unwrap();
        assert_eq!(changed.version, 2);
    }

    /// Two current revisions is a state every reader gets wrong: the app shows
    /// one, search indexes the other, and nothing reports a problem.
    #[test]
    fn exactly_one_revision_is_current_after_several_writes() {
        let (mut store, id) = store_with_a_spec();
        for body in ["a", "b", "c", "d"] {
            store.write_revision(draft(&id, body)).unwrap();
        }
        assert_eq!(current_count(&store, &id), 1);

        // A no-op write must not disturb it either.
        store.write_revision(draft(&id, "d")).unwrap();
        assert_eq!(current_count(&store, &id), 1);
    }

    #[test]
    fn revisions_come_back_oldest_first() {
        let (mut store, id) = store_with_a_spec();
        for body in ["first", "second", "third"] {
            store.write_revision(draft(&id, body)).unwrap();
        }

        let history = store.revisions(&id).unwrap();
        assert_eq!(
            history.iter().map(|d| d.version).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(
            history.iter().map(|d| d.body.as_str()).collect::<Vec<_>>(),
            vec!["first", "second", "third"]
        );
    }

    #[test]
    fn the_header_row_points_at_the_newest_revision() {
        let (mut store, id) = store_with_a_spec();
        store.write_revision(draft(&id, "one")).unwrap();
        store.write_revision(draft(&id, "two")).unwrap();

        let pointer: i64 = store
            .conn
            .query_row(
                "SELECT current_doc_version FROM specs WHERE id = ?1",
                [id.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pointer, 2, "the header and the revision disagree");
    }

    #[test]
    fn a_diff_shows_the_changed_line() {
        let (mut store, id) = store_with_a_spec();
        store
            .write_revision(draft(&id, "DuckDB and Lance.\nTwo engines.\n"))
            .unwrap();
        store
            .write_revision(draft(&id, "One SQLite file.\nTwo engines.\n"))
            .unwrap();

        let diff = store.diff(&id, 1, 2).unwrap();
        assert_eq!(diff.from_version, 1);
        assert_eq!(diff.to_version, 2);
        assert_eq!(diff.added, 1);
        assert_eq!(diff.removed, 1);
        assert!(
            diff.unified.contains("-DuckDB and Lance."),
            "the removed line is missing: {}",
            diff.unified
        );
        assert!(
            diff.unified.contains("+One SQLite file."),
            "the added line is missing: {}",
            diff.unified
        );
        assert!(
            diff.unified.contains(" Two engines."),
            "the unchanged line should carry a space: {}",
            diff.unified
        );
    }

    /// A stated Phase 9 exit criterion. The bytes are generated rather than
    /// committed, because a 5 MB fixture in git is 5 MB in every clone forever.
    #[test]
    fn a_five_megabyte_blob_round_trips_byte_identically() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("specline.sqlite")).unwrap();

        // Not all one byte: a compressible run would measure something SQLite
        // is not being asked to do. This is cheap and varied.
        let bytes: Vec<u8> = (0..5 * 1024 * 1024)
            .map(|n: usize| (n.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        let blob = Blob::new(bytes.clone(), "image/png", stored_now());
        let sha = blob.sha256.clone();

        let started = std::time::Instant::now();
        let id = store.put_blob(blob).unwrap();
        let wrote = started.elapsed();

        let started = std::time::Instant::now();
        let back = store.get_blob(&id).unwrap().unwrap();
        let read = started.elapsed();

        assert_eq!(back.bytes.len(), 5 * 1024 * 1024);
        assert_eq!(back.bytes, bytes, "the bytes came back changed");
        assert_eq!(back.sha256, sha);
        assert_eq!(back.media_type, "image/png");
        println!("5 MB blob: wrote in {wrote:?}, read in {read:?}");
    }

    /// The point of the whole task. An image and the row that owns it commit
    /// together or not at all, which across two engines was not expressible at
    /// all.
    #[test]
    fn a_blob_and_its_entity_row_are_written_in_one_transaction() {
        let store = Store::in_memory().unwrap();
        let project = Project::new("specline", "Specline");
        let project_id = project.id.clone();
        insert_entity(&store, &project.into());

        let design = crate::Design::new(project_id.clone(), "The board");
        let design_id = design.id.clone();
        let blob = Blob::new(vec![1, 2, 3, 4], "image/png", stored_now())
            .owned_by(design_id.clone(), project_id.clone());
        let blob_id = blob.blob_id.clone();

        let spec = spec_for(EntityType::Design);
        let entity: Entity = design.into();

        {
            let tx = store.conn.unchecked_transaction().unwrap();
            tx.execute(
                &insert_stmt(&spec),
                params_from_iter(insert_params(&entity)),
            )
            .unwrap();
            insert_blob_in(&tx, &blob).unwrap();
            tx.commit().unwrap();
        }

        assert!(store.get_blob(&blob_id).unwrap().is_some());
        let rows: i64 = store
            .conn
            .query_row(
                "SELECT count(*) FROM design_artifacts WHERE id = ?1",
                [design_id.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 1);

        // And the half that matters: a rollback takes both, so there is no
        // state where the image exists and the design does not.
        let orphan = Blob::new(vec![9, 9, 9], "image/png", stored_now());
        let orphan_id = orphan.blob_id.clone();
        let doomed = crate::Design::new(project_id, "Never committed");
        let doomed_id = doomed.id.clone();
        let doomed: Entity = doomed.into();
        {
            let tx = store.conn.unchecked_transaction().unwrap();
            tx.execute(
                &insert_stmt(&spec),
                params_from_iter(insert_params(&doomed)),
            )
            .unwrap();
            insert_blob_in(&tx, &orphan).unwrap();
            tx.rollback().unwrap();
        }

        assert!(
            store.get_blob(&orphan_id).unwrap().is_none(),
            "the blob outlived the transaction that wrote it"
        );
        let rows: i64 = store
            .conn
            .query_row(
                "SELECT count(*) FROM design_artifacts WHERE id = ?1",
                [doomed_id.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 0, "the design outlived the transaction that wrote it");
    }

    /// A supplied vector is stored as raw little-endian f32 and nothing else —
    /// that is the promise that lets the vector index be rebuilt from this
    /// column instead of from the embedder.
    #[test]
    fn an_embedding_is_stored_as_little_endian_floats() {
        let (mut store, id) = store_with_a_spec();
        let mut document = draft(&id, "with a vector");
        document.embedding = Some(vec![1.0f32, -0.5]);
        store.write_revision(document).unwrap();

        let stored: Vec<u8> = store
            .conn
            .query_row(
                "SELECT embedding FROM documents WHERE entity_id = ?1",
                [id.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        let mut expected = 1.0f32.to_le_bytes().to_vec();
        expected.extend_from_slice(&(-0.5f32).to_le_bytes());
        assert_eq!(stored, expected);

        // Reading a document does not carry it back — 384 floats no caller
        // has ever wanted.
        assert!(
            store
                .revision(&id, None)
                .unwrap()
                .unwrap()
                .embedding
                .is_none()
        );
    }

    /// The reason [`TIMESTAMP_FORMAT`] pins six fractional digits: with a
    /// variable-width fraction, `…36.5Z` sorts after `…36.524Z` and `ORDER BY
    /// created_at` — a string comparison now — hands back the wrong order.
    #[test]
    fn stored_timestamps_are_fixed_width_and_sort_correctly() {
        let earlier = DateTime::from_timestamp_micros(1_775_000_000_500_000).unwrap();
        let later = DateTime::from_timestamp_micros(1_775_000_000_500_001).unwrap();
        let a = render_ts(earlier);
        let b = render_ts(later);

        assert_eq!(a.len(), b.len(), "the format must be fixed width");
        assert!(a < b, "{a} should sort before {b}");
        assert_eq!(parse_ts("documents", "created_at", &a).unwrap(), earlier);
    }

    /// The migration off DuckDB wrote rows in DuckDB's rendering and they are
    /// still on disk, so the reader has to accept more shapes than the writer
    /// produces.
    #[test]
    fn a_migrated_timestamp_is_accepted_and_a_nonsense_one_names_its_column() {
        for raw in [
            "2026-08-11T09:14:36Z",
            "2026-08-11T09:14:36.524Z",
            "2026-08-11 09:14:36.524",
        ] {
            assert!(parse_ts("documents", "created_at", raw).is_ok(), "{raw}");
        }

        let err = parse_ts("blobs", "created_at", "last thursday").unwrap_err();
        let message = err.to_string();
        assert!(message.contains("created_at"), "unhelpful error: {message}");
        assert!(message.contains("blobs"), "unhelpful error: {message}");
    }

    #[test]
    fn an_entity_with_no_document_has_no_revisions() {
        let (store, id) = store_with_a_spec();
        assert!(store.revision(&id, None).unwrap().is_none());
        assert!(store.revision(&id, Some(1)).unwrap().is_none());
        assert!(store.revisions(&id).unwrap().is_empty());
    }

    #[test]
    fn asking_for_a_version_that_does_not_exist_is_none_not_an_error() {
        let (mut store, id) = store_with_a_spec();
        store.write_revision(draft(&id, "one")).unwrap();
        assert!(store.revision(&id, Some(4)).unwrap().is_none());
    }

    #[test]
    fn an_unknown_blob_is_none() {
        let store = Store::in_memory().unwrap();
        assert!(store.get_blob(&BlobId::generate()).unwrap().is_none());
    }

    /// The error has to say which version was missing. "No such revision" for
    /// a request naming two of them is a message that sends the reader back to
    /// the store to find out which.
    #[test]
    fn a_diff_against_a_missing_version_says_which_one() {
        let (mut store, id) = store_with_a_spec();
        store.write_revision(draft(&id, "one")).unwrap();

        let err = store.diff(&id, 1, 9).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("revision 9"), "unhelpful error: {message}");
        assert!(
            message.contains(id.as_str()),
            "the error should name the document: {message}"
        );

        let err = store.diff(&id, 8, 1).unwrap_err();
        assert!(
            err.to_string().contains("revision 8"),
            "the `from` side must be checked too: {err}"
        );
    }

    /// A revision for an id no table holds is a document nothing can reach.
    /// Across two engines it could only be found by `fsck`, after the fact;
    /// here the write itself refuses.
    #[test]
    fn a_revision_for_an_entity_that_does_not_exist_is_refused() {
        let store = Store::in_memory().unwrap();
        let mut store = store;
        let ghost = EntityId::generate(EntityType::Spec);

        let err = store.write_revision(draft(&ghost, "orphan")).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("create the entity"),
            "unhelpful error: {message}"
        );

        let rows: i64 = store
            .conn
            .query_row("SELECT count(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0, "the refused revision was written anyway");
    }
}

/// How many revisions to embed per batch.
///
/// Inference is batched because per-item calls dominate a pass over the whole
/// corpus, and 32 is small enough that a failure loses a fraction of a second
/// of work rather than the whole run.
pub const REEMBED_BATCH: usize = 32;

/// What a re-embedding pass did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReembedReport {
    /// Revisions that had no vector when the pass started.
    pub missing: usize,
    /// Revisions that have one now.
    pub embedded: usize,
    /// Revisions the model refused, which stay readable and keyword-searchable.
    pub failed: usize,
}

impl Store {
    /// Give every current revision that has no vector one.
    ///
    /// Embedding happens on the way into a new revision and nowhere else, so
    /// turning the feature on left the entire existing corpus invisible to the
    /// vector half of hybrid search — permanently, because nothing would ever
    /// rewrite those rows. This is the pass that fixes that, and it is the
    /// reason `embedding_version` exists.
    ///
    /// `progress` is called after each batch with (done, total), because the
    /// first run is slow enough that a silent wait reads as a hang.
    ///
    /// `limit` caps how many revisions one call will take on. `None` is the
    /// whole backlog and is what a person at a terminal wants. The daemon
    /// passes a small number and calls this repeatedly, because it holds the
    /// store behind a mutex for the length of the call — 62 documents took 26
    /// seconds on the machine this was written on, and a request arriving in
    /// the middle of that would wait for all of it.
    ///
    /// A batch the model refuses is logged and skipped rather than aborting the
    /// pass: one unembeddable document should not stop the other two hundred.
    ///
    /// Archived entities are skipped, and this is the one place the exclusion
    /// has to be written out rather than inherited. Archiving deletes the
    /// passages through a trigger, which leaves the revision looking exactly
    /// like one that was never embedded — so without the predicate this pass
    /// would put back, every time it ran, precisely what archiving had just
    /// taken away.
    ///
    /// Since B-55 the unit of work is a *revision*, and what it builds is that
    /// revision's passages. "Missing" therefore means "current, live, and has
    /// no passages" rather than "has a null `embedding`", and the column it
    /// used to fill is no longer written at all.
    pub fn reembed_missing(
        &mut self,
        embedder: &dyn crate::Embedder,
        limit: Option<usize>,
        mut progress: impl FnMut(usize, usize),
    ) -> Result<ReembedReport> {
        let mut report = ReembedReport::default();

        let mut pending: Vec<(String, String, String)> = {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT d.doc_id, d.title, d.body FROM documents d \
                     WHERE d.status = 'current' \
                       AND NOT EXISTS (SELECT 1 FROM document_chunks c \
                                        WHERE c.doc_id = d.doc_id \
                                          AND c.embedding_model = ?1) \
                       AND NOT EXISTS (SELECT 1 FROM v_entities v \
                                        WHERE v.id = d.entity_id AND v.archived_at IS NOT NULL) \
                     ORDER BY d.doc_id",
                )
                .map_err(Error::storage("list the revisions with no passages"))?;
            let rows = stmt
                .query_map([embedder.model_name()], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })
                .map_err(Error::storage("list the revisions with no passages"))?;
            rows.collect::<std::result::Result<_, _>>()
                .map_err(Error::storage("read a revision with no passages"))?
        };

        if let Some(limit) = limit {
            pending.truncate(limit);
        }
        report.missing = pending.len();
        if pending.is_empty() {
            return Ok(report);
        }

        for batch in pending.chunks(REEMBED_BATCH) {
            // One transaction per batch. A killed pass then leaves whole
            // revisions done and the rest untouched, which is exactly what
            // re-running it expects to find. A revision's passages all land
            // together or not at all — a half-chunked document would look
            // complete to the "has no passages" query above and never be
            // finished.
            let tx = self
                .conn
                .transaction()
                .map_err(Error::storage("begin a re-embedding batch"))?;
            let mut done = 0usize;
            let mut failed = 0usize;
            for (doc_id, title, body) in batch {
                // Rebuilt through the same function the write path calls, on
                // purpose. Two routes to the same passages is two chances for
                // a backfilled one and a freshly written one to differ, and
                // then which vector a document has depends on when it was last
                // touched rather than on what it says.
                match chunks_for(&tx, embedder, doc_id, title, body) {
                    // Passages written, so the revision is embedded. Zero of
                    // them is not: `write_chunks_in` turns a refusal from the
                    // model into `Ok(0)` — deliberately, so one bad document
                    // cannot fail a write — and counting that as a success made
                    // this report say "62 document(s) embedded" about a pass
                    // that had embedded none of them. It also made a caller
                    // looping until no progress loop for ever.
                    Ok(n) if n > 0 => done += 1,
                    Ok(_) => failed += 1,
                    Err(e) => {
                        tracing::warn!(
                            doc_id = %doc_id,
                            error = %e,
                            "a revision could not be embedded; skipping it and continuing"
                        );
                        failed += 1;
                    }
                }
            }
            tx.commit()
                .map_err(Error::storage("commit a re-embedding batch"))?;

            report.embedded += done;
            report.failed += failed;
            progress(report.embedded + report.failed, report.missing);
        }

        Ok(report)
    }
}

/// Build one revision's passages from its stored title and body.
///
/// A thin seam over [`write_chunks_in`] so the re-embed pass does not need a
/// whole `Document` in hand — it reads three columns, which is the point of
/// reading three columns.
fn chunks_for(
    conn: &Connection,
    embedder: &dyn crate::Embedder,
    doc_id: &str,
    title: &str,
    body: &str,
) -> Result<usize> {
    let (entity_id, entity_type, project_id, body_hash): (String, String, String, String) = conn
        .query_row(
            "SELECT entity_id, entity_type, COALESCE(project_id, ''), body_hash \
             FROM documents WHERE doc_id = ?1",
            [doc_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(Error::storage(format!("read the revision {doc_id}")))?;

    write_chunks_in(
        conn,
        Some(embedder),
        ChunkSource {
            doc_id,
            entity_id: &entity_id,
            entity_type: &entity_type,
            project_id: &project_id,
            title,
            body,
            body_hash: &body_hash,
        },
    )
}
