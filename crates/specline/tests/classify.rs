//! Sorting contract differences into "additive" and "breaking".
//!
//! The emitter next door records what every surface looks like. This decides
//! whether a change to one hurts anybody — which is a different question, and
//! the one a release actually turns on.
//!
//! # Why a table rather than a judgement
//!
//! A judgement call at release time is made by whoever is tired. So the rules
//! are fixed, written down in the Phase 10 spec §5.2, and applied the same way
//! every time.
//!
//! # Failing closed is the whole design
//!
//! **Anything this cannot place is breaking.** A classifier that guesses
//! "additive" when it is unsure is worse than no classifier, because it
//! produces confidence rather than information — and confidence is what stops
//! somebody looking. Being wrong in the safe direction costs a sentence in the
//! release notes; being wrong in the other direction costs somebody's store.
//!
//! # Where the baseline comes from
//!
//! `git show <ref>:contracts/…`. There are no stored per-release copies —
//! version control already has them — so the release diff is
//! `git diff <last-tag>..HEAD -- contracts/` and this reads the two sides of it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt;

/// What a single difference does to somebody depending on this surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Verdict {
    /// Nothing that worked before stops working.
    Additive,
    /// Something that worked before may not. Needs an entry in the release
    /// notes naming its migration and what the user is told.
    Breaking,
}

/// One difference, and what it means.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Difference {
    verdict: Verdict,
    surface: &'static str,
    what: String,
}

impl fmt::Display for Difference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tag = match self.verdict {
            Verdict::Additive => "additive",
            Verdict::Breaking => "BREAKING",
        };
        write!(f, "{tag:9} {:10} {}", self.surface, self.what)
    }
}

fn breaking(surface: &'static str, what: impl Into<String>) -> Difference {
    Difference {
        verdict: Verdict::Breaking,
        surface,
        what: what.into(),
    }
}

fn additive(surface: &'static str, what: impl Into<String>) -> Difference {
    Difference {
        verdict: Verdict::Additive,
        surface,
        what: what.into(),
    }
}

// --- the MCP tool surface -------------------------------------------------

/// Compare two `tools.json` documents.
///
/// Unparseable input on either side is breaking rather than an error to shrug
/// at: a contract file nobody can read is a contract nobody can check.
fn classify_tools(before: &str, after: &str) -> Vec<Difference> {
    let (old, new) = match (
        serde_json::from_str::<Value>(before),
        serde_json::from_str::<Value>(after),
    ) {
        (Ok(o), Ok(n)) => (o, n),
        _ => return vec![breaking("tools", "one side is not valid JSON")],
    };

    let mut out = Vec::new();
    let old_tools = tools_by_name(&old);
    let new_tools = tools_by_name(&new);

    for name in old_tools.keys() {
        if !new_tools.contains_key(name) {
            out.push(breaking("tools", format!("tool `{name}` was removed")));
        }
    }
    for name in new_tools.keys() {
        if !old_tools.contains_key(name) {
            out.push(additive("tools", format!("tool `{name}` was added")));
        }
    }

    for (name, before_tool) in &old_tools {
        let Some(after_tool) = new_tools.get(name) else {
            continue;
        };

        // Descriptions break no caller, so they are additive — and they are
        // also the only documentation a model gets, which makes a silent
        // rewrite of the thing that decides tool selection worth a human
        // reading. So it is reported rather than ignored.
        if before_tool.get("description") != after_tool.get("description") {
            out.push(additive(
                "tools",
                format!("`{name}` description changed — worth reading, it steers tool choice"),
            ));
        }
        out.extend(classify_schema_object(
            name,
            before_tool.get("input_schema"),
            after_tool.get("input_schema"),
        ));
    }

    out
}

fn tools_by_name(doc: &Value) -> std::collections::BTreeMap<String, Value> {
    doc.get("tools")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|t| {
                    t.get("name")
                        .and_then(Value::as_str)
                        .map(|n| (n.to_owned(), t.clone()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The rules for one tool's JSON Schema.
fn classify_schema_object(
    tool: &str,
    before: Option<&Value>,
    after: Option<&Value>,
) -> Vec<Difference> {
    let (Some(before), Some(after)) = (before, after) else {
        // A schema appearing or vanishing changes what a caller must send, and
        // this cannot tell which way without one of them. Fail closed.
        return vec![breaking(
            "tools",
            format!("`{tool}` gained or lost its argument schema"),
        )];
    };

    let mut out = Vec::new();
    let old_props = props(before);
    let new_props = props(after);

    for (arg, old_spec) in &old_props {
        match new_props.get(arg) {
            None => out.push(breaking(
                "tools",
                format!("`{tool}` argument `{arg}` was removed"),
            )),
            Some(new_spec) => {
                if old_spec.get("type") != new_spec.get("type") {
                    out.push(breaking(
                        "tools",
                        format!("`{tool}` argument `{arg}` changed type"),
                    ));
                }
                // A narrowed enum rejects values that used to be accepted.
                // Widening one does not.
                let (old_enum, new_enum) = (enum_values(old_spec), enum_values(new_spec));
                if !old_enum.is_empty() || !new_enum.is_empty() {
                    let removed: Vec<_> = old_enum.difference(&new_enum).cloned().collect();
                    if !removed.is_empty() {
                        out.push(breaking(
                            "tools",
                            format!(
                                "`{tool}` argument `{arg}` no longer accepts {}",
                                removed.join(", ")
                            ),
                        ));
                    }
                    let added: Vec<_> = new_enum.difference(&old_enum).cloned().collect();
                    if !added.is_empty() {
                        out.push(additive(
                            "tools",
                            format!(
                                "`{tool}` argument `{arg}` also accepts {}",
                                added.join(", ")
                            ),
                        ));
                    }
                }
            }
        }
    }

    for arg in new_props.keys() {
        if !old_props.contains_key(arg) {
            out.push(additive(
                "tools",
                format!("`{tool}` gained optional argument `{arg}`"),
            ));
        }
    }

    // Required is the one that bites: a caller that was correct yesterday is
    // wrong today, with no code change of their own.
    let newly_required: Vec<_> = required(after)
        .difference(&required(before))
        .cloned()
        .collect();
    for arg in newly_required {
        out.push(breaking(
            "tools",
            format!("`{tool}` argument `{arg}` is now required"),
        ));
    }

    out
}

fn props(schema: &Value) -> std::collections::BTreeMap<String, Value> {
    schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

fn required(schema: &Value) -> BTreeSet<String> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(std::borrow::ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn enum_values(spec: &Value) -> BTreeSet<String> {
    spec.get("enum")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(std::string::ToString::to_string).collect())
        .unwrap_or_default()
}

// --- the store's shape ----------------------------------------------------

/// Compare two `schema.json` documents.
fn classify_store_schema(before: &str, after: &str) -> Vec<Difference> {
    let (old, new) = match (
        serde_json::from_str::<Value>(before),
        serde_json::from_str::<Value>(after),
    ) {
        (Ok(o), Ok(n)) => (o, n),
        _ => return vec![breaking("schema", "one side is not valid JSON")],
    };

    let mut out = Vec::new();
    let empty = serde_json::Map::new();
    let old_tables = old.as_object().unwrap_or(&empty);
    let new_tables = new.as_object().unwrap_or(&empty);

    for table in old_tables.keys() {
        if !new_tables.contains_key(table) {
            out.push(breaking("schema", format!("table `{table}` was dropped")));
        }
    }
    for table in new_tables.keys() {
        if !old_tables.contains_key(table) {
            out.push(additive("schema", format!("table `{table}` was added")));
        }
    }

    for (table, old_def) in old_tables {
        let Some(new_def) = new_tables.get(table) else {
            continue;
        };
        let old_cols = columns(old_def);
        let new_cols = columns(new_def);

        for (name, old_col) in &old_cols {
            match new_cols.get(name) {
                None => out.push(breaking("schema", format!("`{table}.{name}` was dropped"))),
                Some(new_col) => {
                    if old_col.declared_type != new_col.declared_type {
                        out.push(breaking(
                            "schema",
                            format!(
                                "`{table}.{name}` changed type: {} -> {}",
                                old_col.declared_type, new_col.declared_type
                            ),
                        ));
                    }
                    if !old_col.not_null && new_col.not_null {
                        out.push(breaking(
                            "schema",
                            format!("`{table}.{name}` became NOT NULL"),
                        ));
                    }
                    if old_col.primary_key != new_col.primary_key {
                        out.push(breaking(
                            "schema",
                            format!("`{table}.{name}` changed its primary-key role"),
                        ));
                    }
                }
            }
        }

        for (name, col) in &new_cols {
            if old_cols.contains_key(name) {
                continue;
            }
            // A new column that must be populated breaks every existing row,
            // unless a default fills them. A nullable one does not.
            if col.not_null && col.default.is_none() {
                out.push(breaking(
                    "schema",
                    format!("`{table}.{name}` is new, NOT NULL, and has no default"),
                ));
            } else {
                out.push(additive("schema", format!("`{table}.{name}` was added")));
            }
        }
    }

    out
}

struct Column {
    declared_type: String,
    not_null: bool,
    default: Option<String>,
    primary_key: i64,
}

/// Read `PRAGMA table_info` rows, which are positional:
/// `cid, name, type, notnull, dflt_value, pk`.
fn columns(table: &Value) -> std::collections::BTreeMap<String, Column> {
    table
        .get("columns")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    let r = r.as_array()?;
                    let name = r.get(1)?.as_str()?.to_owned();
                    Some((
                        name,
                        Column {
                            declared_type: r
                                .get(2)
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            not_null: r.get(3).and_then(Value::as_i64).unwrap_or(0) != 0,
                            default: r
                                .get(4)
                                .filter(|v| !v.is_null())
                                .map(std::string::ToString::to_string),
                            primary_key: r.get(5).and_then(Value::as_i64).unwrap_or(0),
                        },
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

// --- the text surfaces ----------------------------------------------------

/// Compare a line-oriented surface — `cli.txt`, `generated.txt`.
///
/// `generated.txt` has no additive case at all, and that is deliberate rather
/// than laziness. Users commit those files, so any layout change is a diff in
/// every repository that has ever run `specline generate`. It breaks nothing in a
/// technical sense and is exactly as annoying as something that does, so it
/// gets announced.
///
/// `cli.txt` is judged by removal: a line that disappeared is a subcommand,
/// flag or default somebody may have scripted against. Lines only added are
/// additive.
fn classify_text(surface: &'static str, before: &str, after: &str) -> Vec<Difference> {
    if before == after {
        return Vec::new();
    }
    if surface == "generated" {
        return vec![breaking(
            surface,
            "the generated layout changed — every repository that runs `specline generate` \
             will see a diff",
        )];
    }

    // Runs of spaces are collapsed before comparing, because clap lays its help
    // out in columns and widens them to fit the longest subcommand name. Adding
    // one long name therefore re-indents every other line, and a naive
    // comparison reads all of them as removed and re-added.
    //
    // Found the first time a subcommand was added after this gate existed:
    // `release-manifest` reported 21 lines gone and 27 arrived, and called an
    // unambiguously additive change breaking. It fails in the safe direction,
    // which is why it was only annoying — but a gate that cries wolf on every
    // new subcommand is one people learn to wave through, and then it is not
    // failing safe any more.
    let flatten = |s: &str| -> BTreeSet<String> {
        s.lines()
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|l| !l.is_empty())
            .collect()
    };
    let old = flatten(before);
    let new = flatten(after);
    let gone: Vec<&String> = old.difference(&new).collect();
    let arrived = new.difference(&old).count();

    let mut out = Vec::new();
    if !gone.is_empty() {
        let sample: Vec<String> = gone
            .iter()
            .take(3)
            .map(|l| format!("`{}`", l.trim()))
            .collect();
        out.push(breaking(
            surface,
            format!(
                "{} line(s) disappeared, e.g. {}",
                gone.len(),
                sample.join(", ")
            ),
        ));
    }
    if arrived > 0 {
        out.push(additive(surface, format!("{arrived} line(s) added")));
    }
    out
}

/// Sort a whole set of differences so a reader meets the worst first.
fn worst_first(mut all: Vec<Difference>) -> Vec<Difference> {
    all.sort_by(|a, b| b.verdict.cmp(&a.verdict).then_with(|| a.what.cmp(&b.what)));
    all
}

// --- the rules, one test each ---------------------------------------------

fn verdicts(d: &[Difference]) -> Vec<(Verdict, &str)> {
    d.iter().map(|x| (x.verdict, x.what.as_str())).collect()
}

fn tools(json: &str) -> String {
    format!(r#"{{"count":1,"protocol":"x","tools":{json}}}"#)
}

#[test]
fn an_added_tool_is_additive_and_a_removed_one_is_not() {
    let before = tools(r#"[{"name":"a","description":"d","input_schema":{"properties":{}}}]"#);
    let after = tools(
        r#"[{"name":"a","description":"d","input_schema":{"properties":{}}},
            {"name":"b","description":"d","input_schema":{"properties":{}}}]"#,
    );

    let added = classify_tools(&before, &after);
    assert_eq!(added.len(), 1, "{added:?}");
    assert_eq!(added[0].verdict, Verdict::Additive);

    let removed = classify_tools(&after, &before);
    assert_eq!(removed.len(), 1, "{removed:?}");
    assert_eq!(removed[0].verdict, Verdict::Breaking);
    assert!(removed[0].what.contains("removed"), "{removed:?}");
}

/// The one that bites hardest: a caller correct yesterday is wrong today,
/// having changed nothing.
#[test]
fn a_newly_required_argument_is_breaking() {
    let before = tools(
        r#"[{"name":"a","description":"d","input_schema":{"properties":{"x":{"type":"string"}}}}]"#,
    );
    let after = tools(
        r#"[{"name":"a","description":"d","input_schema":{"properties":{"x":{"type":"string"}},"required":["x"]}}]"#,
    );

    let d = classify_tools(&before, &after);
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].verdict, Verdict::Breaking);
    assert!(d[0].what.contains("now required"), "{d:?}");
}

#[test]
fn an_optional_argument_is_additive_but_a_removed_one_is_not() {
    let one = tools(
        r#"[{"name":"a","description":"d","input_schema":{"properties":{"x":{"type":"string"}}}}]"#,
    );
    let two = tools(
        r#"[{"name":"a","description":"d","input_schema":{"properties":{"x":{"type":"string"},"y":{"type":"string"}}}}]"#,
    );

    assert_eq!(
        verdicts(&classify_tools(&one, &two))[0].0,
        Verdict::Additive
    );
    assert_eq!(
        verdicts(&classify_tools(&two, &one))[0].0,
        Verdict::Breaking
    );
}

/// Narrowing rejects values that used to work. Widening does not, and the two
/// directions must not be confused — this is the asymmetry the whole table is
/// made of.
#[test]
fn a_narrowed_enum_breaks_and_a_widened_one_does_not() {
    let narrow = tools(
        r#"[{"name":"a","description":"d","input_schema":{"properties":{"k":{"enum":["x"]}}}}]"#,
    );
    let wide = tools(
        r#"[{"name":"a","description":"d","input_schema":{"properties":{"k":{"enum":["x","y"]}}}}]"#,
    );

    let widened = classify_tools(&narrow, &wide);
    assert_eq!(widened.len(), 1, "{widened:?}");
    assert_eq!(widened[0].verdict, Verdict::Additive);

    let narrowed = classify_tools(&wide, &narrow);
    assert_eq!(narrowed.len(), 1, "{narrowed:?}");
    assert_eq!(narrowed[0].verdict, Verdict::Breaking);
    assert!(
        narrowed[0].what.contains("no longer accepts"),
        "{narrowed:?}"
    );
}

/// A description change breaks nobody and still gets printed, because it is
/// the only documentation a model gets.
#[test]
fn a_description_change_is_additive_and_still_reported() {
    let before = tools(r#"[{"name":"a","description":"old","input_schema":{"properties":{}}}]"#);
    let after = tools(r#"[{"name":"a","description":"new","input_schema":{"properties":{}}}]"#);

    let d = classify_tools(&before, &after);
    assert_eq!(d.len(), 1, "a description change must not be silent: {d:?}");
    assert_eq!(d[0].verdict, Verdict::Additive);
}

/// Fail closed. Unreadable input is not "no differences".
#[test]
fn unparseable_input_is_breaking_rather_than_empty() {
    let d = classify_tools("{not json", &tools("[]"));
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].verdict, Verdict::Breaking);
}

// PRAGMA table_info rows: cid, name, type, notnull, dflt_value, pk
fn table(cols: &str) -> String {
    format!(r#"{{"t":{{"columns":[{cols}],"indexes":[],"foreign_keys":[]}}}}"#)
}

#[test]
fn a_dropped_column_breaks_and_a_nullable_one_does_not() {
    let one = table(r#"[0,"a","TEXT",0,null,0]"#);
    let two = table(r#"[0,"a","TEXT",0,null,0],[1,"b","TEXT",0,null,0]"#);

    let added = classify_store_schema(&one, &two);
    assert_eq!(added.len(), 1, "{added:?}");
    assert_eq!(added[0].verdict, Verdict::Additive);

    let dropped = classify_store_schema(&two, &one);
    assert_eq!(dropped.len(), 1, "{dropped:?}");
    assert_eq!(dropped[0].verdict, Verdict::Breaking);
}

/// The distinction that decides whether existing rows survive.
#[test]
fn a_new_not_null_column_breaks_unless_it_has_a_default() {
    let base = table(r#"[0,"a","TEXT",0,null,0]"#);
    let no_default = table(r#"[0,"a","TEXT",0,null,0],[1,"b","TEXT",1,null,0]"#);
    let with_default = table(r#"[0,"a","TEXT",0,null,0],[1,"b","TEXT",1,"''",0]"#);

    assert_eq!(
        classify_store_schema(&base, &no_default)[0].verdict,
        Verdict::Breaking
    );
    assert_eq!(
        classify_store_schema(&base, &with_default)[0].verdict,
        Verdict::Additive
    );
}

#[test]
fn a_changed_column_type_breaks() {
    let text = table(r#"[0,"a","TEXT",0,null,0]"#);
    let int = table(r#"[0,"a","INTEGER",0,null,0]"#);

    let d = classify_store_schema(&text, &int);
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].verdict, Verdict::Breaking);
    assert!(d[0].what.contains("TEXT -> INTEGER"), "{d:?}");
}

#[test]
fn a_dropped_table_breaks_and_a_new_one_does_not() {
    let one = r#"{"t":{"columns":[],"indexes":[],"foreign_keys":[]}}"#;
    let two = r#"{"t":{"columns":[],"indexes":[],"foreign_keys":[]},
                  "u":{"columns":[],"indexes":[],"foreign_keys":[]}}"#;

    assert_eq!(
        classify_store_schema(one, two)[0].verdict,
        Verdict::Additive
    );
    assert_eq!(
        classify_store_schema(two, one)[0].verdict,
        Verdict::Breaking
    );
}

/// Generated markdown has no additive case, deliberately: users commit those
/// files, so any layout change is a diff in every repository that ever ran
/// `specline generate`.
#[test]
fn any_change_to_the_generated_layout_is_announced() {
    assert!(classify_text("generated", "a\n", "a\n").is_empty());
    let d = classify_text("generated", "a\n", "a\nb\n");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].verdict, Verdict::Breaking);
}

#[test]
fn a_disappearing_cli_line_breaks_and_a_new_one_does_not() {
    let before = "--home <HOME>\n--json\n";
    let after = "--home <HOME>\n--json\n--force\n";

    assert_eq!(
        classify_text("cli", before, after)[0].verdict,
        Verdict::Additive
    );

    let removed = classify_text("cli", after, before);
    assert_eq!(removed[0].verdict, Verdict::Breaking);
    assert!(removed[0].what.contains("--force"), "{removed:?}");
}

#[test]
fn the_worst_is_reported_first() {
    let sorted = worst_first(vec![
        additive("tools", "b"),
        breaking("schema", "a"),
        additive("cli", "a"),
    ]);
    assert_eq!(sorted[0].verdict, Verdict::Breaking);
}

/// The real contracts, against a real baseline, when one is named.
///
/// Skipped rather than failed when `CONTRACTS_BASELINE` is unset, because the
/// baseline is a git ref and there are no release tags yet. At release time CI
/// runs this with the previous tag, which is the whole point of the file.
#[test]
fn the_recorded_contracts_classify_against_a_baseline() {
    let Ok(baseline) = std::env::var("CONTRACTS_BASELINE") else {
        eprintln!("CONTRACTS_BASELINE unset — skipping. Set it to a git ref to compare.");
        return;
    };

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let at_baseline = |file: &str| -> Option<String> {
        let out = std::process::Command::new("git")
            .current_dir(&root)
            .args(["show", &format!("{baseline}:contracts/{file}")])
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let now = |file: &str| std::fs::read_to_string(root.join("contracts").join(file)).unwrap();

    let mut all = Vec::new();
    if let Some(old) = at_baseline("tools.json") {
        all.extend(classify_tools(&old, &now("tools.json")));
    }
    if let Some(old) = at_baseline("schema.json") {
        all.extend(classify_store_schema(&old, &now("schema.json")));
    }
    if let Some(old) = at_baseline("cli.txt") {
        all.extend(classify_text("cli", &old, &now("cli.txt")));
    }
    if let Some(old) = at_baseline("generated.txt") {
        all.extend(classify_text("generated", &old, &now("generated.txt")));
    }

    let all = worst_first(all);
    println!("contract differences since {baseline}:");
    for d in &all {
        println!("  {d}");
    }
    let breaking = all
        .iter()
        .filter(|d| d.verdict == Verdict::Breaking)
        .count();
    println!("  {} difference(s), {breaking} breaking", all.len());
}

// --- the acknowledgement gate ---------------------------------------------
//
// A release carrying a breaking difference must not merge unless somebody has
// written down what breaks, which migration handles it, and what the user is
// told. That is the mechanism; the version number is decoration.
//
// It replaces a gate that could not work. An earlier draft refused a release
// unless the version bump matched the highest severity found — and on 0.x both
// additive and breaking mean a minor bump, so the condition was satisfied by
// every release forever while looking like a guard.
//
// # Acknowledgement is by the exact sentence, and that is deliberate
//
// An entry names a difference by the same words the classifier produces. So
// rewording a rule in this file invalidates the acknowledgements that quote it,
// and the gate fails until somebody updates them.
//
// That is a feature rather than an oversight. If the description of what breaks
// has changed, the person who signed it off should read it again — the
// alternative is a stable key that lets prose drift away from what was actually
// agreed to. The cost is real and bounded: rewording a rule is a change to this
// file, which is reviewed anyway.

// `ENTRY_MARKER`, `Acknowledgement`, `parse_acknowledgements` and
// `breaking_section` used to be defined here a second time, with a subtly
// different marker rule than `render-breaking-notes.sh`'s: this copy found the
// marker anywhere in the text (`str::split_once`), the script only on a line
// by itself. `contracts/BREAKING.md` mentions the marker inline in prose above
// the real one, so this copy silently read the real entries as instructions.
// See `crates/specline/tests/common/acknowledgements.rs` for the shared,
// corrected version and why it is a shared module rather than two disagreeing
// copies (KEEL-299 review).
#[path = "common/acknowledgements.rs"]
mod acknowledgements;
use acknowledgements::{Acknowledgement, breaking_section, parse_acknowledgements};

/// What the gate decided, so the caller can print all of it at once rather than
/// stopping at the first problem.
#[derive(Debug, Default, PartialEq, Eq)]
struct GateResult {
    /// Breaking differences nobody wrote an entry for. Any of these fails.
    unacknowledged: Vec<String>,
    /// Entries whose `what` matches no current difference — a stale sign-off,
    /// or a reworded rule. Also fails: an acknowledgement that describes
    /// nothing is how a file of them stops meaning anything.
    stale: Vec<String>,
    /// Entries that exist but say nothing useful in one of the two fields.
    incomplete: Vec<String>,
}

impl GateResult {
    fn passes(&self) -> bool {
        self.unacknowledged.is_empty() && self.stale.is_empty() && self.incomplete.is_empty()
    }
}

/// Hold every breaking difference against the acknowledgements.
fn gate(differences: &[Difference], acknowledgements: &[Acknowledgement]) -> GateResult {
    let mut result = GateResult::default();

    for d in differences
        .iter()
        .filter(|d| d.verdict == Verdict::Breaking)
    {
        if !acknowledgements.iter().any(|a| a.what == d.what) {
            result.unacknowledged.push(d.what.clone());
        }
    }

    for a in acknowledgements {
        if !differences.iter().any(|d| d.what == a.what) {
            result.stale.push(a.what.clone());
        // An entry with an empty field is not an entry. "none" is a real
        // answer for a migration and is accepted; blank is not.
        } else if a.migration.is_empty() || a.tells_the_user.is_empty() {
            result.incomplete.push(a.what.clone());
        }
    }

    result
}

#[test]
fn a_breaking_change_nobody_wrote_down_fails_the_gate() {
    let differences = vec![breaking("tools", "tool `specline_note` was removed")];
    let result = gate(&differences, &[]);

    assert!(!result.passes());
    assert_eq!(
        result.unacknowledged,
        vec!["tool `specline_note` was removed"]
    );
}

#[test]
fn an_acknowledged_one_passes() {
    let differences = vec![breaking("tools", "tool `specline_note` was removed")];
    let acknowledgements = parse_acknowledgements(
        "<!-- acknowledgements -->\n## tool `specline_note` was removed\n\
         - migration: none — callers move to specline_update\n\
         - tells the user: specline_note is gone; notes are a field on specline_update now.\n",
    );

    assert!(gate(&differences, &acknowledgements).passes());
}

/// Additive differences need no entry. A gate that demanded one for every
/// change would be a gate people learn to satisfy without reading.
#[test]
fn additive_differences_need_no_entry() {
    let differences = vec![additive("tools", "tool `specline_x` was added")];
    assert!(gate(&differences, &[]).passes());
}

/// An entry that describes nothing is how a file of them stops meaning
/// anything — usually a sign-off left behind after the change was reverted.
#[test]
fn a_stale_entry_fails_too() {
    let acknowledgements = parse_acknowledgements(
        "<!-- acknowledgements -->\n## tool `long_gone` was removed\n\
         - migration: none\n\
         - tells the user: it is gone.\n",
    );

    let result = gate(&[], &acknowledgements);
    assert!(!result.passes());
    assert_eq!(result.stale, vec!["tool `long_gone` was removed"]);
}

/// A heading with nothing under it is somebody acknowledging the existence of
/// a problem rather than the problem.
#[test]
fn an_entry_missing_its_prose_fails() {
    let differences = vec![breaking("tools", "tool `specline_note` was removed")];
    let acknowledgements =
        parse_acknowledgements("<!-- acknowledgements -->\n## tool `specline_note` was removed\n");

    let result = gate(&differences, &acknowledgements);
    assert!(!result.passes());
    assert_eq!(result.incomplete, vec!["tool `specline_note` was removed"]);
    assert!(
        result.unacknowledged.is_empty(),
        "an incomplete entry is a different complaint from a missing one"
    );
}

#[test]
fn the_release_notes_are_built_from_the_entries() {
    let acknowledgements = parse_acknowledgements(
        "<!-- acknowledgements -->\n## tool `specline_note` was removed\n\
         - migration: none — callers move to specline_update\n\
         - tells the user: specline_note is gone; notes are a field on specline_update now.\n",
    );

    let notes = breaking_section(&acknowledgements);
    assert!(notes.contains("## Breaking"));
    assert!(notes.contains("specline_note is gone"), "{notes}");
    assert!(notes.contains("Migration: none"), "{notes}");
    assert!(
        breaking_section(&[]).is_empty(),
        "a release with nothing breaking gets no section at all"
    );
}

/// The real gate, when a baseline is named. Same skip rule as the classifier
/// above: there are no release tags yet.
#[test]
fn the_real_release_is_gated_when_a_baseline_is_named() {
    let Ok(baseline) = std::env::var("CONTRACTS_BASELINE") else {
        eprintln!("CONTRACTS_BASELINE unset — skipping.");
        return;
    };

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let acknowledgements = std::fs::read_to_string(root.join("contracts/BREAKING.md"))
        .map(|t| parse_acknowledgements(&t))
        .unwrap_or_default();

    let at_baseline = |file: &str| -> Option<String> {
        let out = std::process::Command::new("git")
            .current_dir(&root)
            .args(["show", &format!("{baseline}:contracts/{file}")])
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let now = |f: &str| std::fs::read_to_string(root.join("contracts").join(f)).unwrap();

    let mut all = Vec::new();
    if let Some(o) = at_baseline("tools.json") {
        all.extend(classify_tools(&o, &now("tools.json")));
    }
    if let Some(o) = at_baseline("schema.json") {
        all.extend(classify_store_schema(&o, &now("schema.json")));
    }
    if let Some(o) = at_baseline("cli.txt") {
        all.extend(classify_text("cli", &o, &now("cli.txt")));
    }
    if let Some(o) = at_baseline("generated.txt") {
        all.extend(classify_text("generated", &o, &now("generated.txt")));
    }

    let result = gate(&all, &acknowledgements);
    assert!(
        result.passes(),
        "the release is not ready.\n\
         Breaking changes with no entry in contracts/BREAKING.md:\n  {}\n\
         Entries describing nothing that changed:\n  {}\n\
         Entries missing a migration or a user-facing sentence:\n  {}",
        result.unacknowledged.join("\n  "),
        result.stale.join("\n  "),
        result.incomplete.join("\n  "),
    );
}

/// The bug the control run for this gate found: a documentation heading read
/// as an acknowledgement.
///
/// `## How to add one` became an entry for a difference of that name, which
/// then failed as stale and blocked a release that was otherwise fine. The
/// instructions and the data live in one file on purpose — the instructions
/// are what somebody needs at the moment they are writing an entry — so the
/// boundary has to be explicit rather than inferred from heading levels.
#[test]
fn prose_headings_before_the_marker_are_not_entries() {
    let text = "# Breaking changes\n\n\
                ## How to add one\n\
                Copy the line exactly.\n\n\
                <!-- acknowledgements -->\n\n\
                ## tool `x` was removed\n\
                - migration: none\n\
                - tells the user: it is gone.\n";

    let parsed = parse_acknowledgements(text);
    assert_eq!(
        parsed.len(),
        1,
        "only the entry after the marker: {parsed:?}"
    );
    assert_eq!(parsed[0].what, "tool `x` was removed");
}

/// A file whose marker is missing acknowledges nothing, which is the failing
/// side. Renaming the marker must not quietly wave every breaking change
/// through.
#[test]
fn a_file_with_no_marker_acknowledges_nothing() {
    let text = "## tool `x` was removed\n- migration: none\n- tells the user: gone.\n";
    assert!(parse_acknowledgements(text).is_empty());

    let differences = vec![breaking("tools", "tool `x` was removed")];
    assert!(!gate(&differences, &parse_acknowledgements(text)).passes());
}

/// clap widens its help columns to fit the longest subcommand name, so adding
/// one re-indents every other line. That must not read as 21 removals.
#[test]
fn re_indenting_the_cli_help_is_not_a_removal() {
    let before = "Commands:\n  archive   Archive a row\n  backup    Back up the store\n";
    let after = "Commands:\n  archive           Archive a row\n  \
                 backup            Back up the store\n  \
                 release-manifest  Print what a release promises\n";

    let d = classify_text("cli", before, after);
    assert!(
        d.iter().all(|x| x.verdict == Verdict::Additive),
        "a wider help column is not a breaking change: {d:?}"
    );
}

/// And the real removal still has to be caught, or the fix above would have
/// bought nothing.
#[test]
fn a_genuinely_removed_cli_line_is_still_caught() {
    let before = "  archive   Archive a row\n  backup    Back up the store\n";
    let after = "  archive           Archive a row\n";

    let d = classify_text("cli", before, after);
    assert!(
        d.iter().any(|x| x.verdict == Verdict::Breaking),
        "losing `backup` is breaking however the rest is spaced: {d:?}"
    );
}
