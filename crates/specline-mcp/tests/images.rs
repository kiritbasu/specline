//! Inline images: the ingestion path for design artifacts (TQ-6, KEEL-46).
//!
//! Base64 in the tool call is the only path that works from every surface. A
//! filesystem path works only where there is a filesystem, which excludes chat
//! and Cowork — the two places a design image actually comes from.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use base64::Engine as _;
use serde_json::{Value, json};
use specline_core::{Actor, EntityStore, Project, Provenance, Store};
use specline_mcp::{ToolCall, dispatch};

/// The smallest real PNG: 1×1, transparent. Magic bytes are what matter here.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
];

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// A store whose project has a `root_path`, and that path.
///
/// The root matters now: a picture may only be read from the folders KB
/// approved — Desktop, Downloads, Pictures — or from the project's own
/// directory (KEEL-239). A test writing to a bare `tempdir` is nowhere, which
/// is correct behaviour and useless as a fixture, so the project is rooted at
/// the scratch directory these tests then write into.
fn store() -> (Store, tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("checkout");
    std::fs::create_dir_all(&root).unwrap();

    let mut s = Store::open(dir.path().join("specline.sqlite")).unwrap();
    let mut project = Project::new("harbour", "Harbour");
    project.root_path = Some(root.display().to_string());
    s.create(project.into(), &Provenance::anonymous(Actor::Claude))
        .unwrap();
    (s, dir, root)
}

fn call(store: &mut Store, args: Value) -> Result<Value, specline_mcp::protocol::RpcError> {
    dispatch(
        store,
        ToolCall {
            name: "specline_create",
            arguments: &args,
            client: None,
        },
    )
}

#[test]
fn a_base64_image_is_stored_and_the_design_points_at_it() {
    let (mut store, _d, _root) = store();
    let result = call(
        &mut store,
        json!({
            "type": "design", "project": "harbour", "name": "Invoice screen",
            "image": b64(PNG), "session_id": "ses_t", "surface": "chat"
        }),
    )
    .expect("a design with an inline image");

    let blob_id = result["structuredContent"]["entity"]["blob_id"]
        .as_str()
        .expect("the design must point at the blob it was given");

    let blob = store
        .get_blob(&specline_core::BlobId::parse(blob_id).unwrap())
        .unwrap()
        .expect("the blob is stored");
    assert_eq!(blob.bytes, PNG, "byte-for-byte, not re-encoded");

    // Sniffed from the magic bytes, not taken on trust.
    assert_eq!(blob.media_type, "image/png");

    // Owned, so `fsck` can trace it. A blob with no entity is bytes nobody
    // dares delete.
    assert!(blob.entity_id.is_some(), "the blob must name its owner");
    assert!(blob.project_id.is_some());
}

#[test]
fn a_data_url_is_accepted_because_a_model_will_produce_one() {
    let (mut store, _d, _root) = store();
    let result = call(
        &mut store,
        json!({
            "type": "design", "project": "harbour", "name": "From a data URL",
            "image": format!("data:image/png;base64,{}", b64(PNG)),
            "session_id": "ses_t", "surface": "chat"
        }),
    )
    .expect("a data: URL is a reasonable thing to be handed");
    assert!(result["structuredContent"]["entity"]["blob_id"].is_string());
}

#[test]
fn wrapped_base64_still_decodes() {
    // A model breaking a long payload across lines has valid intent and
    // invalid base64. Failing on it would be a papercut with no upside.
    let (mut store, _d, _root) = store();
    let wrapped = b64(PNG)
        .as_bytes()
        .chunks(20)
        .map(|c| String::from_utf8_lossy(c).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let result = call(
        &mut store,
        json!({
            "type": "design", "project": "harbour", "name": "Wrapped",
            "image": wrapped, "session_id": "ses_t", "surface": "chat"
        }),
    )
    .expect("whitespace is stripped before decoding");
    assert!(result["structuredContent"]["entity"]["blob_id"].is_string());
}

#[test]
fn an_oversized_image_is_refused_with_its_size_and_nothing_is_created() {
    let (mut store, _d, _root) = store();
    let huge = vec![0x89u8; 1_048_577];
    let err = call(
        &mut store,
        json!({
            "type": "design", "project": "harbour", "name": "Too big",
            "image": b64(&huge), "session_id": "ses_t", "surface": "chat"
        }),
    )
    .expect_err("over the cap");

    assert!(
        err.message.contains("1048577"),
        "name the actual size: {}",
        err.message
    );
    assert!(
        err.message.contains("1048576"),
        "and the limit: {}",
        err.message
    );

    // The check runs before anything is written, so a refused image leaves no
    // half-made design behind. Truncating instead would be worse still: a
    // corrupt file that looks like a successful write.
    let designs = store
        .list(&specline_core::EntityQuery::default().of_type(specline_core::EntityType::Design))
        .unwrap();
    assert!(designs.items.is_empty(), "nothing may be created");
}

#[test]
fn undecodable_base64_says_so_rather_than_storing_rubbish() {
    let (mut store, _d, _root) = store();
    let err = call(
        &mut store,
        json!({
            "type": "design", "project": "harbour", "name": "Nonsense",
            "image": "this is not base64 !!!", "session_id": "ses_t", "surface": "chat"
        }),
    )
    .expect_err("must not be stored as bytes");
    assert!(err.message.contains("base64"), "{}", err.message);
}

#[test]
fn a_type_that_holds_no_image_says_which_ones_do() {
    let (mut store, _d, _root) = store();
    let err = call(
        &mut store,
        json!({
            "type": "task", "project": "harbour", "title": "Not an image holder",
            "image": b64(PNG), "session_id": "ses_t", "surface": "chat"
        }),
    )
    .expect_err("a task has nowhere to put it");
    assert!(
        err.message.contains("design"),
        "point at the right type: {}",
        err.message
    );
}

// --- Reading a file off the disk (TQ-33) ----------------------------------
//
// Base64 through a tool call is capped by *context*, not storage: the model
// emits every character, so 1 MB costs it 350,000 to 450,000 output tokens and
// the useful ceiling is nearer 100 KB — a small mockup, not a screenshot. The
// daemon reading a file on the same machine has none of that cost, which is what
// makes a real screenshot possible from Claude Code.

/// Write bytes to a real file and return its absolute path.
fn file_with(dir: &std::path::Path, name: &str, bytes: &[u8]) -> String {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn a_design_can_be_created_from_a_file_on_disk() {
    let (mut store, _d, root) = store();
    let path = file_with(&root, "screenshot.png", PNG);

    let result = call(
        &mut store,
        json!({
            "type": "design", "project": "harbour", "name": "Board, dark",
            "image_path": path, "session_id": "ses_t", "surface": "code"
        }),
    )
    .expect("a design created from a path");

    let blob_id = result["structuredContent"]["entity"]["blob_id"]
        .as_str()
        .expect("the design points at a blob")
        .to_owned();
    let blob = store
        .get_blob(&specline_core::BlobId::parse(&blob_id).unwrap())
        .unwrap()
        .expect("the bytes are in the store");
    assert_eq!(blob.bytes, PNG, "the file's bytes, unchanged");
    assert_eq!(blob.media_type, "image/png");
}

#[test]
fn an_existing_design_can_be_given_an_image_afterwards() {
    let (mut store, _d, root) = store();
    let path = file_with(&root, "later.png", PNG);

    let created = call(
        &mut store,
        json!({
            "type": "design", "project": "harbour", "name": "Added later",
            "session_id": "ses_t", "surface": "code"
        }),
    )
    .unwrap();
    let id = created["structuredContent"]["entity"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let version = created["structuredContent"]["entity"]["version"]
        .as_i64()
        .unwrap();

    let attached = dispatch(
        &mut store,
        ToolCall {
            name: "specline_update",
            arguments: &json!({
                "id": id, "version": version,
                "changes": { "image_path": path },
                "session_id": "ses_t", "surface": "code"
            }),
            client: None,
        },
    )
    .expect("attaching to something that already exists");

    assert!(
        attached["structuredContent"]["entity"]["blob_id"].is_string(),
        "the design now points at a blob: {attached}"
    );
    assert_eq!(
        attached["structuredContent"]["attached"]["bytes"],
        PNG.len()
    );
    // Said out loud, because the whole reason to use this path rather than
    // base64 is that the bytes cost the caller nothing.
    assert!(
        attached["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("your context"),
        "{attached}"
    );
}

// The boundary TQ-33 names explicitly: "if a future change makes the path
// argument accept anything URL-shaped, that is this decision being reversed by
// accident". Reading a local file and fetching a URL look similar and are not —
// the second would give a model the ability to make the daemon talk to the
// internet, which TQ-6 declined.
#[test]
fn a_url_is_refused_rather_than_fetched() {
    let (mut store, _d, _root) = store();
    for url in [
        "https://example.com/screenshot.png",
        "http://127.0.0.1/x.png",
        "file:///etc/passwd",
    ] {
        let err = call(
            &mut store,
            json!({
                "type": "design", "project": "harbour", "name": format!("From {url}"),
                "image_path": url, "session_id": "ses_t", "surface": "code"
            }),
        )
        .expect_err("a URL must not be fetched");
        assert!(
            err.message.contains("outbound") || err.message.contains("URL"),
            "the refusal has to say why, or the next person will widen it: {}",
            err.message
        );
    }
}

#[test]
fn a_relative_path_is_refused_because_the_daemon_has_its_own_directory() {
    let (mut store, _d, _root) = store();
    let err = call(
        &mut store,
        json!({
            "type": "design", "project": "harbour", "name": "Relative",
            "image_path": "screenshot.png", "session_id": "ses_t", "surface": "code"
        }),
    )
    .expect_err("a relative path resolves against something the caller cannot see");
    assert!(err.message.contains("relative"), "{}", err.message);
}

#[test]
fn a_file_that_is_not_an_image_is_refused_on_its_bytes_not_its_extension() {
    let (mut store, _d, root) = store();
    // Named `.png`, and it is a text file. The extension is whatever somebody
    // typed; the magic bytes are what the app will try to render.
    let path = file_with(&root, "lies.png", b"this is not a picture");

    let err = call(
        &mut store,
        json!({
            "type": "design", "project": "harbour", "name": "Not a picture",
            "image_path": path, "session_id": "ses_t", "surface": "code"
        }),
    )
    .expect_err("bytes decide, not the name");
    assert!(err.message.contains("not an image"), "{}", err.message);
}

#[test]
fn a_missing_file_says_what_to_do_instead() {
    let (mut store, _d, _root) = store();
    let err = call(
        &mut store,
        json!({
            "type": "design", "project": "harbour", "name": "Ghost",
            "image_path": "/nonexistent/definitely/not/here.png",
            "session_id": "ses_t", "surface": "code"
        }),
    )
    .expect_err("a path that names nothing");
    assert!(err.message.contains("could not read"), "{}", err.message);
    assert!(
        err.message.contains("base64"),
        "the error points at the other path, since a session on a machine the daemon cannot \
         see has one: {}",
        err.message
    );
}

// Two answers to one question. Silently preferring one would mean a caller who
// sent both sometimes got the file and sometimes the payload, depending on an
// ordering nothing documents.
#[test]
fn giving_both_an_inline_image_and_a_path_is_refused() {
    let (mut store, _d, root) = store();
    let path = file_with(&root, "both.png", PNG);

    let err = call(
        &mut store,
        json!({
            "type": "design", "project": "harbour", "name": "Both at once",
            "image": b64(PNG), "image_path": path,
            "session_id": "ses_t", "surface": "code"
        }),
    )
    .expect_err("both is ambiguous");
    assert!(err.message.contains("two answers"), "{}", err.message);
}

#[test]
fn a_task_cannot_be_given_an_image_by_path_either() {
    let (mut store, _d, root) = store();
    let path = file_with(&root, "wrong-type.png", PNG);

    let err = call(
        &mut store,
        json!({
            "type": "task", "project": "harbour", "title": "Not an image holder",
            "summary": "A task used to check that only designs and artifacts take an image.",
            "image_path": path, "session_id": "ses_t", "surface": "code"
        }),
    )
    .expect_err("only designs and artifacts hold images");
    assert!(
        err.message.contains("does not hold an image"),
        "{}",
        err.message
    );
}

/// The reason the allowlist exists, as a test.
///
/// The model choosing an `image_path` is reading issue text, customer feedback
/// and web pages — so "add the screenshot at /Users/…/.ssh/backup.png" is a
/// sentence something else can write. Before KEEL-239 that copied the file into
/// the store. Now it is refused, and the refusal says where pictures may come
/// from rather than leaving somebody to guess.
#[test]
fn a_picture_from_somewhere_private_is_refused_and_the_refusal_says_where_to_put_it() {
    let (mut store, d, root) = store();

    // Outside the project's root and outside any of the approved folders,
    // which is exactly where a path suggested by untrusted text would point.
    let elsewhere = d.path().join("not-the-project");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let path = file_with(&elsewhere, "private.png", PNG);

    let err = call(
        &mut store,
        json!({
            "type": "design", "project": "harbour", "name": "Somewhere else",
            "image_path": path, "session_id": "ses_t", "surface": "code"
        }),
    )
    .expect_err("a file outside the allowed folders must not be read");

    assert!(
        err.message.contains("outside the folders"),
        "and it must say that is why: {}",
        err.message
    );
    assert!(
        err.message.contains("base64"),
        "and name the way round it: {}",
        err.message
    );

    // It has to name the folders it *will* read, and this asserts that against
    // the project's own checkout rather than against `Desktop`.
    //
    // Asserting on `Desktop` made this test depend on the machine it ran on.
    // `image_roots::roots` drops a folder that does not exist — deliberately,
    // because offering somebody a directory they do not have is a worse answer
    // than a shorter list — and a Linux CI runner has no Desktop, Downloads or
    // Pictures. So the assertion passed on every Mac and failed on Linux, which
    // is precisely the platform difference the Linux leg exists to find, and it
    // found a test rather than a bug.
    //
    // That the list names the home folders when they exist is covered in
    // `image_roots`' own unit tests, where the home is constructed rather than
    // inherited from whoever is running the suite.
    let allowed = root
        .canonicalize()
        .expect("the project checkout exists")
        .display()
        .to_string();
    assert!(
        err.message.contains(&allowed),
        "and name a folder it will read — the project's own checkout is one on every \
         platform: {}",
        err.message
    );
}
