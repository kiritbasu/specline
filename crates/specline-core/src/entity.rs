//! The thirteen artifact types, and the vocabulary shared across all of them.
//!
//! Thirteen is a ceiling, not a starting point (PRD R-1). The enum is closed
//! and exhaustive matching is used everywhere on purpose: adding a fourteenth
//! variant should break the build in a dozen places and force a conversation,
//! rather than slipping in behind a `_ => {}` arm.

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::fmt;

/// One of Specline's thirteen artifact types.
///
/// The serialised form is the singular snake-case name — `task`, `design`,
/// `metric_observation`. That string is what lands in `links.from_type`,
/// `events.entity_type` and `documents.entity_type`, and what an agent passes
/// as the `type` argument over MCP. It is *not* always the table name; see
/// [`EntityType::table`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    /// The root container. Everything else belongs to exactly one, except
    /// global terms.
    Project,
    /// A planning or shipping unit. Replaces "epic".
    Milestone,
    /// A unit of work: task, bug, chore or spike.
    Task,
    /// A prose document: PRD, spec, RFC, design doc or note.
    Spec,
    /// An architecture decision record.
    Decision,
    /// An open unknown: question, risk or assumption.
    Question,
    /// A glossary entry. May be global rather than project-scoped.
    Term,
    /// Raw input from the world: interview, support, sales, idea, competitor,
    /// observation.
    Feedback,
    /// A mockup, wireframe, screenshot or Figma node.
    Design,
    /// A deployment target.
    Environment,
    /// A named measure with a target.
    Metric,
    /// One timestamped value of a metric.
    MetricObservation,
    /// The escape hatch: files and links that fit nowhere else.
    Artifact,
}

/// The words a project might use for something Specline already has.
///
/// Read before the enum error is raised, never instead of the enum. Every entry
/// resolves onto one of the thirteen types — see
/// [`EntityType::parse_with_alias`] for why that is a hard rule rather than a
/// convention.
///
/// Kept deliberately short. A long list is how "close enough" creeps in: the
/// test is whether a reader would be surprised to learn the two words name the
/// same row, and if they would, it does not belong here.
const ALIASES: &[(&str, EntityType)] = &[
    // Planning units. This project says "phase" on every screen, which is what
    // made the whole thing worth fixing.
    ("phase", EntityType::Milestone),
    ("epic", EntityType::Milestone),
    ("sprint", EntityType::Milestone),
    ("cycle", EntityType::Milestone),
    ("iteration", EntityType::Milestone),
    ("release", EntityType::Milestone),
    ("version", EntityType::Milestone),
    // Units of work. `bug` and `chore` are also task *kinds*, and resolving
    // them to the type is right: a caller saying `type: "bug"` wants a task,
    // and the kind is a separate argument they can still set.
    ("issue", EntityType::Task),
    ("ticket", EntityType::Task),
    ("story", EntityType::Task),
    ("bug", EntityType::Task),
    ("chore", EntityType::Task),
    ("defect", EntityType::Task),
    // Decisions.
    ("adr", EntityType::Decision),
    ("rfc", EntityType::Decision),
    ("choice", EntityType::Decision),
    // Questions and risks.
    ("risk", EntityType::Question),
    ("unknown", EntityType::Question),
    ("open question", EntityType::Question),
    // Specs.
    ("requirement", EntityType::Spec),
    ("design doc", EntityType::Spec),
    ("prd", EntityType::Spec),
];

impl EntityType {
    /// Every type, in a stable order.
    ///
    /// Stable because it drives `fsck`'s reporting order, the fixture loader,
    /// and the unified vertex view — all of which produce diffs a human reads.
    pub const ALL: [EntityType; 13] = [
        EntityType::Project,
        EntityType::Milestone,
        EntityType::Task,
        EntityType::Spec,
        EntityType::Decision,
        EntityType::Question,
        EntityType::Term,
        EntityType::Feedback,
        EntityType::Design,
        EntityType::Environment,
        EntityType::Metric,
        EntityType::MetricObservation,
        EntityType::Artifact,
    ];

    /// The wire name — what appears in `links.from_type`, `events.entity_type`
    /// and MCP arguments.
    pub const fn as_str(self) -> &'static str {
        match self {
            EntityType::Project => "project",
            EntityType::Milestone => "milestone",
            EntityType::Task => "task",
            EntityType::Spec => "spec",
            EntityType::Decision => "decision",
            EntityType::Question => "question",
            EntityType::Term => "term",
            EntityType::Feedback => "feedback",
            EntityType::Design => "design",
            EntityType::Environment => "environment",
            EntityType::Metric => "metric",
            EntityType::MetricObservation => "metric_observation",
            EntityType::Artifact => "artifact",
        }
    }

    /// The table this type lives in.
    ///
    /// Separate from [`EntityType::as_str`] because two of them disagree:
    /// `design` is stored in `design_artifacts` and `feedback` is its own
    /// plural-less table. Deriving one from the other by appending an `s`
    /// would be wrong in exactly those two places, which is the kind of bug
    /// that only shows up for the artifact types you test last.
    pub const fn table(self) -> &'static str {
        match self {
            EntityType::Project => "projects",
            EntityType::Milestone => "milestones",
            EntityType::Task => "tasks",
            EntityType::Spec => "specs",
            EntityType::Decision => "decisions",
            EntityType::Question => "questions",
            EntityType::Term => "terms",
            EntityType::Feedback => "feedback",
            EntityType::Design => "design_artifacts",
            EntityType::Environment => "environments",
            EntityType::Metric => "metrics",
            EntityType::MetricObservation => "metric_observations",
            EntityType::Artifact => "artifacts",
        }
    }

    /// The three-letter ULID prefix, e.g. `tsk` for `tsk_01H8…`.
    pub const fn prefix(self) -> &'static str {
        match self {
            EntityType::Project => "prj",
            EntityType::Milestone => "mst",
            EntityType::Task => "tsk",
            EntityType::Spec => "spc",
            EntityType::Decision => "dec",
            EntityType::Question => "que",
            EntityType::Term => "trm",
            EntityType::Feedback => "fbk",
            EntityType::Design => "dsg",
            EntityType::Environment => "env",
            EntityType::Metric => "mtr",
            EntityType::MetricObservation => "obs",
            EntityType::Artifact => "art",
        }
    }

    /// Whether this type's body lives in the `documents` table.
    ///
    /// The five that do are exactly SPEC §2.1's `entity_type` domain. Any type
    /// answering `true` here must also carry `current_doc_version` on its own
    /// row — `fsck` checks that the two agree.
    pub const fn has_document(self) -> bool {
        matches!(
            self,
            EntityType::Spec
                | EntityType::Decision
                | EntityType::Question
                | EntityType::Feedback
                | EntityType::Design
        )
    }

    /// Whether the document *is* the row's content, so creating one without
    /// prose produces something empty rather than something terse.
    ///
    /// A subset of [`has_document`](Self::has_document), and the difference is
    /// the point. A task with no body still says something — its summary is
    /// required, and its status, priority and labels are content. A spec, a
    /// decision or a question has no such column: the document is the whole of
    /// what it holds, so a row with a title and nothing else records that
    /// somebody decided something and loses what it was. Three landed in this
    /// store that way (KEEL-171), and a decision log that says what was chosen
    /// and nothing about why is the one shape it exists to prevent.
    ///
    /// `Feedback` was in this list and is now out, which is worth explaining
    /// because the reasoning that put it there — "what a customer said is the
    /// artifact" — is still true. It is out because feedback's content column
    /// is `summary`, it is `NOT NULL`, and §3.2 is explicit that it is called
    /// that rather than `title` precisely so it holds what somebody said
    /// rather than a name invented for it. So feedback fails the test in the
    /// paragraph above for the same reason a task does: the row still says
    /// something without a body. The refusal also had to claim "there is no
    /// summary column to fall back on" about the one type whose only content
    /// column is exactly that.
    ///
    /// The verbatim still belongs in the body and is where an interview
    /// transcript or the whole of what was said goes. It is no longer a
    /// precondition of capture, because B-90 turns on filing a signal costing
    /// no more than typing the thought did — and a rule that demands a second,
    /// longer field for a one-line idea is answered either by writing it twice
    /// or by padding, and padding is worse than nothing.
    ///
    /// `Design` is out for the older reason: its content is the image, and a
    /// caption is a caption.
    pub const fn needs_prose(self) -> bool {
        matches!(
            self,
            EntityType::Spec | EntityType::Decision | EntityType::Question
        )
    }

    /// Whether rows of this type carry a `project_id`, and whether it may be
    /// null.
    ///
    /// Three shapes exist and the difference matters for validation:
    /// `Project` has no such column at all, `Term` has a nullable one (null
    /// means global, per Q-4), and everything else requires it.
    pub const fn project_scope(self) -> ProjectScope {
        match self {
            EntityType::Project => ProjectScope::IsTheProject,
            EntityType::Term => ProjectScope::Optional,
            _ => ProjectScope::Required,
        }
    }

    /// Whether this type participates in text search at all.
    ///
    /// Metrics and observations are excluded by design (REQ-4): they are
    /// numeric, and reaching them is a filter rather than a query.
    pub const fn is_searchable(self) -> bool {
        !matches!(self, EntityType::Metric | EntityType::MetricObservation)
    }

    /// Parse a wire name back into a type.
    pub fn parse(s: &str) -> Result<Self> {
        EntityType::ALL
            .into_iter()
            .find(|t| t.as_str() == s)
            .ok_or_else(|| Error::MalformedId {
                supplied: s.to_owned(),
                problem: format!("`{s}` is not a Specline entity type"),
                expected: Self::wire_names().join(" | "),
            })
    }

    /// Parse a wire name, accepting the words a project might actually use.
    ///
    /// `Ok((type, Some(alias)))` means the caller said something other than the
    /// canonical name and it was understood. The alias comes back so the caller
    /// can **say so in the response** — a silent success teaches the session
    /// nothing and it guesses the same way next time, where a narrated one
    /// teaches the vocabulary in a single round trip.
    ///
    /// # An alias is a spelling, not a concept
    ///
    /// This must never grow a fourteenth type. "A sprint isn't quite a
    /// milestone" is exactly how a schema acquires a type it cannot later
    /// remove, and the ceiling of thirteen is a hard constraint. Everything
    /// here maps onto something that already exists; nothing here is allowed to
    /// mean something new.
    pub fn parse_with_alias(s: &str) -> Result<(Self, Option<&'static str>)> {
        let lower = s.trim().to_lowercase();

        if let Ok(exact) = EntityType::parse(&lower) {
            return Ok((exact, None));
        }

        ALIASES
            .iter()
            .find(|(alias, _)| *alias == lower)
            .map(|(alias, ty)| (*ty, Some(*alias)))
            .ok_or_else(|| Error::MalformedId {
                supplied: s.to_owned(),
                problem: format!("`{s}` is not a Specline entity type or a word for one"),
                expected: Self::wire_names().join(" | "),
            })
    }

    /// Parse a three-letter ULID prefix back into a type.
    pub fn from_prefix(prefix: &str) -> Option<Self> {
        EntityType::ALL.into_iter().find(|t| t.prefix() == prefix)
    }

    /// Every wire name, for building error messages that tell a model what it
    /// could have said instead.
    pub fn wire_names() -> Vec<&'static str> {
        EntityType::ALL
            .into_iter()
            .map(EntityType::as_str)
            .collect()
    }
}

impl fmt::Display for EntityType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a type relates to the project that owns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectScope {
    /// The type *is* the project — there is no `project_id` column.
    IsTheProject,
    /// `project_id` is `NOT NULL`.
    Required,
    /// `project_id` may be null, meaning global. Only `terms`.
    Optional,
}

/// Who performed an act.
///
/// SPEC §3.1 calls this "provenance vocabulary": one concept in two shapes.
/// Entity rows record state (`created_by`, `updated_by`), the event log
/// records the act (`actor`), and both draw from this set. An entity's
/// `updated_by` always equals the `actor` of the event that produced it —
/// `fsck` asserts exactly that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    /// KB, typing directly.
    Human,
    /// A Claude session, on any surface.
    Claude,
    /// The GitHub App, acting on a webhook.
    Github,
    /// Specline itself — migrations, fixtures, scheduled jobs.
    System,
}

impl Actor {
    /// The stored string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Actor::Human => "human",
            Actor::Claude => "claude",
            Actor::Github => "github",
            Actor::System => "system",
        }
    }

    /// Parse a stored string.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "human" => Ok(Actor::Human),
            "claude" => Ok(Actor::Claude),
            "github" => Ok(Actor::Github),
            "system" => Ok(Actor::System),
            other => Err(Error::MalformedId {
                supplied: other.to_owned(),
                problem: format!("`{other}` is not a known actor"),
                expected: "human | claude | github | system".to_owned(),
            }),
        }
    }
}

impl fmt::Display for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What *kind* of place an act happened in — not which product.
///
/// SPEC §3.1's audit block lists four values; §6.5 additionally names `cli` as
/// a fixed sentinel for the command line. The two passages disagree, and this
/// enum reconciles them by carrying all five — see DECISIONS B-8. The column
/// is a bare `VARCHAR` with no check constraint, so this costs nothing at the
/// storage layer.
///
/// **Five values, and it stays five.** Naming a product here is the mistake
/// waiting to be made: `Code` meant Claude Code for as long as Claude Code was
/// the only agent that spoke MCP, and the moment Codex connected the two became
/// indistinguishable. The fix for that is not a sixth variant, because the sixth
/// implies a seventh for Cursor and an eighth for Windsurf, and an enum that
/// grows with the market has to be migrated every time the market moves. Which
/// editor wrote something is a *name and a version*, it is self-reported by the
/// client on every request, and it belongs in open text rather than in a closed
/// set (KEEL-360).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    /// A chat client — Claude's, or another with no coding surface.
    Chat,
    /// Cowork.
    Cowork,
    /// A coding agent in a terminal or editor. Claude Code and Codex both land
    /// here, and the client's own name distinguishes them.
    Code,
    /// The desktop app or the browser interface the daemon serves.
    Ui,
    /// The `specline` command line.
    Cli,
}

impl Surface {
    /// The stored string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Surface::Chat => "chat",
            Surface::Cowork => "cowork",
            Surface::Code => "code",
            Surface::Ui => "ui",
            Surface::Cli => "cli",
        }
    }

    /// Parse a stored string.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "chat" => Ok(Surface::Chat),
            "cowork" => Ok(Surface::Cowork),
            "code" => Ok(Surface::Code),
            "ui" => Ok(Surface::Ui),
            "cli" => Ok(Surface::Cli),
            other => Err(Error::MalformedId {
                supplied: other.to_owned(),
                problem: format!("`{other}` is not a known surface"),
                expected: "chat | cowork | code | ui | cli".to_owned(),
            }),
        }
    }
}

impl fmt::Display for Surface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn wire_names_round_trip() {
        for t in EntityType::ALL {
            assert_eq!(EntityType::parse(t.as_str()).unwrap(), t);
        }
    }

    #[test]
    fn prefixes_round_trip_and_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for t in EntityType::ALL {
            assert!(seen.insert(t.prefix()), "duplicate prefix {}", t.prefix());
            assert_eq!(EntityType::from_prefix(t.prefix()), Some(t));
            assert_eq!(t.prefix().len(), 3, "{t} prefix must be three characters");
        }
    }

    #[test]
    fn table_names_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for t in EntityType::ALL {
            assert!(seen.insert(t.table()), "duplicate table {}", t.table());
        }
    }

    #[test]
    fn design_and_feedback_tables_are_not_the_wire_name_plus_s() {
        // The two cases that would break a naive pluralisation. Asserted so a
        // future refactor to `format!("{}s", self.as_str())` fails loudly.
        assert_eq!(EntityType::Design.table(), "design_artifacts");
        assert_eq!(EntityType::Feedback.table(), "feedback");
    }

    #[test]
    fn exactly_five_types_carry_documents() {
        let with_docs: Vec<_> = EntityType::ALL
            .into_iter()
            .filter(|t| t.has_document())
            .collect();
        assert_eq!(
            with_docs,
            vec![
                EntityType::Spec,
                EntityType::Decision,
                EntityType::Question,
                EntityType::Feedback,
                EntityType::Design,
            ],
            "SPEC §2.1 fixes the documents table's entity_type domain at these five"
        );
    }

    #[test]
    fn metrics_are_excluded_from_search() {
        assert!(!EntityType::Metric.is_searchable());
        assert!(!EntityType::MetricObservation.is_searchable());
        assert_eq!(
            EntityType::ALL
                .into_iter()
                .filter(|t| t.is_searchable())
                .count(),
            11
        );
    }

    #[test]
    fn a_project_can_say_phase_and_mean_milestone() {
        let (ty, alias) = EntityType::parse_with_alias("phase").unwrap();
        assert_eq!(ty, EntityType::Milestone);
        assert_eq!(
            alias,
            Some("phase"),
            "the caller's word comes back so it can be reported"
        );
    }

    #[test]
    fn a_canonical_name_reports_no_alias() {
        // Nothing to narrate when the caller already used Specline's word.
        let (ty, alias) = EntityType::parse_with_alias("milestone").unwrap();
        assert_eq!(ty, EntityType::Milestone);
        assert_eq!(alias, None);
    }

    #[test]
    fn aliases_are_case_and_whitespace_insensitive() {
        assert_eq!(
            EntityType::parse_with_alias("  Sprint ").unwrap().0,
            EntityType::Milestone
        );
    }

    // Failure case: an alias is a spelling, not an escape hatch. A word nobody
    // taught it still fails, with the thirteen names to choose from.
    #[test]
    fn an_unknown_word_still_fails_with_the_valid_names() {
        let err = EntityType::parse_with_alias("widget").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("widget"), "{msg}");
        assert!(msg.contains("milestone"), "{msg}");
    }

    /// The constraint that keeps this safe, asserted rather than trusted.
    ///
    /// "A sprint isn't quite a milestone" is exactly how a schema acquires a
    /// fourteenth type it can never remove. Every alias must land on one of the
    /// thirteen that already exist, and this is the test that notices if one
    /// ever does not.
    #[test]
    fn every_alias_resolves_to_a_type_that_already_exists() {
        for (alias, ty) in ALIASES {
            assert!(
                EntityType::ALL.contains(ty),
                "`{alias}` resolves outside the thirteen"
            );
            assert!(
                EntityType::parse(alias).is_err(),
                "`{alias}` is a canonical name, so it does not belong in the alias table"
            );
        }
    }

    #[test]
    fn no_alias_is_listed_twice() {
        // Two entries for one word means whichever comes first silently wins,
        // and the loser is invisible.
        let mut seen: Vec<&str> = ALIASES.iter().map(|(a, _)| *a).collect();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(before, seen.len(), "an alias is listed more than once");
    }

    #[test]
    fn unknown_type_names_say_what_was_valid() {
        let err = EntityType::parse("epic").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("epic"),
            "should quote what was supplied: {msg}"
        );
        assert!(
            msg.contains("milestone"),
            "should list valid options: {msg}"
        );
    }

    #[test]
    fn actor_and_surface_reject_nonsense() {
        assert!(Actor::parse("robot").is_err());
        assert!(Surface::parse("fax").is_err());
        for a in [Actor::Human, Actor::Claude, Actor::Github, Actor::System] {
            assert_eq!(Actor::parse(a.as_str()).unwrap(), a);
        }
        for s in [
            Surface::Chat,
            Surface::Cowork,
            Surface::Code,
            Surface::Ui,
            Surface::Cli,
        ] {
            assert_eq!(Surface::parse(s.as_str()).unwrap(), s);
        }
    }
}
