//! `specline_context` — the digest.
//!
//! The most important tool in the surface. G2 is "an agent can orient itself on
//! a project in a single MCP call", and REQ-3 budgets that call to something an
//! agent will actually pay: roughly 3–4k tokens.
//!
//! # What is never cut, and why
//!
//! Open questions and glossary terms are declared unbounded (SPEC §6.3). Every
//! other section degrades gracefully and reports what it dropped, but those two
//! do not, because they fail differently from the rest:
//!
//! - A truncated task list makes an agent **less informed**. It will ask.
//! - A truncated question register makes an agent **confidently wrong**. It
//!   re-opens a settled decision and argues for it.
//! - A truncated glossary makes an agent **use the wrong word** for a domain
//!   concept, which then propagates into everything it writes.
//!
//! If those two alone blow the budget, the digest returns them in full and sets
//! `budget_exceeded`. That is not a failure — it is the store telling you the
//! open-question register needs pruning, which is real information.

use crate::drift::{self, Drift};
use crate::store::EventScope;
use crate::{
    Cursor, Entity, EntityId, EntityQuery, EntityStore, EntityType, QuestionStatus, Result, Store,
    TaskStatus,
};
use serde::Serialize;

/// The repository half of the digest: the join between commits and rows.
///
/// Two states and an absence. `None` on the roll-up and on a project with no
/// checkout, where there is nothing to measure. `Unreadable` when there is a
/// checkout and git could not read it — carried, and rendered, so a broken
/// git does not look like a quiet week. `Measured` is the reconciliation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DriftSection {
    /// The checkout could not be read, and this is why.
    Unreadable {
        /// What went wrong, as the caller reported it.
        reason: String,
    },
    /// The reconciliation.
    Measured(Drift),
}

/// How many of each drift list the prose shows before saying "and N more".
///
/// Five, because the section is a prompt to look rather than the place to
/// look: the JSON carries every row, and a session that wants the sixth
/// unlinked commit is reading the structured half anyway.
const DRIFT_SHOWN: usize = 5;

/// How much to include.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// A few hundred tokens: what it is, what is urgent, what is unresolved.
    Brief,
    /// The default. Budgeted to roughly 3–4k tokens.
    Standard,
    /// Most limits lifted, for when an agent is about to do something large.
    Full,
}

impl Depth {
    /// Parse the `depth` argument.
    pub fn parse(s: &str) -> std::result::Result<Self, String> {
        match s {
            "brief" => Ok(Depth::Brief),
            "standard" => Ok(Depth::Standard),
            "full" => Ok(Depth::Full),
            other => Err(format!(
                "`{other}` is not a depth. Expected: brief | standard | full"
            )),
        }
    }

    /// The soft character budget. Roughly four characters per token.
    fn budget_chars(self) -> usize {
        match self {
            Depth::Brief => 3_000,
            Depth::Standard => 14_000,
            Depth::Full => 60_000,
        }
    }

    /// How many items each bounded section may carry.
    fn section_limit(self) -> usize {
        match self {
            Depth::Brief => 3,
            Depth::Standard => 10,
            Depth::Full => 50,
        }
    }
}

/// A section that was cut, and by how much.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Truncation {
    /// Which section.
    pub section: String,
    /// How many were shown.
    pub shown: usize,
    /// How many exist.
    pub total: usize,
}

/// One line about a project in the cross-project roll-up.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProjectLine {
    /// `prj_…`
    pub id: EntityId,
    /// Display name.
    pub name: String,
    /// URL-safe short name.
    pub slug: String,
    /// The prefix of this project's readable identifiers — the `KEEL` of
    /// `KEEL-42`. Carried here so that anything holding a digest can compose a
    /// task's reference without a second lookup.
    pub key: String,
    /// active / paused / shipped / abandoned.
    pub status: String,
    /// Open tasks.
    pub open_tasks: usize,
    /// Open tasks at p0 or p1.
    pub urgent_tasks: usize,
    /// Blocked tasks.
    pub blocked_tasks: usize,
    /// Unresolved questions and risks.
    pub open_questions: usize,
    /// Untriaged signals — the Inbox. Reported separately from `open_tasks`
    /// and never folded into it: a signal is something somebody wants, not
    /// work anybody has committed to, and a count that mixes the two cannot
    /// answer "how much is left", which is the only question it is for.
    pub inbox: usize,
    /// How many days the oldest untriaged signal has been waiting, if any.
    ///
    /// The count alone does not say whether anything is wrong — forty signals
    /// filed this week is a good week, and four that have sat for two months
    /// is the pile KEEL-303 is about. Only the age tells them apart.
    pub inbox_oldest_days: Option<i64>,
    /// The milestone currently in flight, if any.
    pub active_milestone: Option<String>,
    /// What this project calls a milestone, when it has a word of its own.
    ///
    /// Carried so the digest can say it in the first paragraph a session reads.
    /// Until KEEL-121 the alias was reported back on a create and nothing stored
    /// it, so a session learned the vocabulary one rejected call at a time.
    pub milestone_noun: Option<String>,
}

/// A compact reference to an artifact.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Item {
    /// `tsk_…` and friends.
    pub id: EntityId,
    /// The artifact type.
    pub entity_type: String,
    /// Its one-line label.
    pub label: String,
    /// The readable identifier, for the types that have one — `KEEL-42`.
    ///
    /// Carried beside the label rather than folded into it, so a surface can
    /// render it as its own thing and the label stays the title. `None` for the
    /// twelve types that have no such identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Its status, for types that have one.
    pub status: Option<String>,
    /// Extra context — priority, severity, target date.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// A glossary entry.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TermEntry {
    /// The word.
    pub term: String,
    /// What it means here.
    pub definition: String,
    /// Whether it is global rather than project-scoped.
    pub global: bool,
}

/// The digest.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Digest {
    /// The project, or `None` for a cross-project roll-up.
    pub project: Option<ProjectLine>,
    /// One line per project. Only populated for the roll-up.
    pub projects: Vec<ProjectLine>,
    /// Milestones in flight.
    pub active: Vec<Item>,
    /// Milestones whose work is finished and whose fate nobody has declared.
    ///
    /// Populated for a single project only. The roll-up leaves `attention`,
    /// `decisions` and `specs` empty for the same reason — it answers "which
    /// project needs me", and a phase-level decision is a question you ask once
    /// you are inside one.
    pub complete: Vec<Item>,
    /// Urgent and blocked work.
    ///
    /// Tasks only. This said "urgent, blocked and overdue" for the life of the
    /// project and there has never been an overdue anything: `needs_attention`
    /// reads the task list and the `blocks` edges, and never looks at a
    /// milestone. SPEC §7 promised the same thing and has been cut to match
    /// (KEEL-334).
    pub attention: Vec<Item>,
    /// Recent activity, summarised.
    pub recent: Vec<String>,
    /// The last few accepted decisions.
    pub decisions: Vec<Item>,
    /// **Never truncated.** Every unresolved question and unmitigated risk.
    pub questions: Vec<Item>,
    /// Current specs.
    pub specs: Vec<Item>,
    /// The Inbox — untriaged signals, oldest first.
    ///
    /// Listed rather than merely counted, which reverses the call KEEL-321
    /// made. That one carried the size and the age on the reasoning that
    /// `specline_search` would fetch the rows when somebody was going to
    /// triage them. It cannot: search requires a query and has no filter for
    /// "everything untriaged", so a count with no way to reach the contents
    /// left a session able to know the Inbox was long and unable to read it.
    pub inbox: Vec<Item>,
    /// **Never truncated.** The glossary.
    pub terms: Vec<TermEntry>,
    /// What is live, at what version.
    pub environments: Vec<Item>,
    /// Suggested next actions, derived from state.
    ///
    /// Kept alongside [`Digest::next_up`] rather than replaced by it: these
    /// are the observations that are not about a single task ("no milestone
    /// is active"), and they were the whole of `next` before TQ-16.
    pub next: Vec<String>,
    /// **The answer to "what should I do next".** Ranked, named, with reasons.
    ///
    /// The counts in `next` restate the problem; this names the task. See
    /// [`crate::next`] for the ranking.
    pub next_up: Option<NextUpJson>,
    /// The commits since the window started, reconciled against the rows.
    ///
    /// `None` means there was nothing to measure — the roll-up, or a project
    /// with no `root_path`. It never means "measured and found nothing"; that
    /// is a [`DriftSection::Measured`] with empty lists, and a checkout that
    /// could not be read is [`DriftSection::Unreadable`] with the reason.
    /// Three answers for three situations, because the project's own bug
    /// class is the empty result that reads as calm.
    pub drift: Option<DriftSection>,
    /// What was cut.
    pub truncated: Vec<Truncation>,
    /// Set when the unbounded sections alone exceed the budget.
    pub budget_exceeded: bool,
    /// Rough token estimate of this digest.
    pub estimated_tokens: usize,
}

impl Digest {
    /// Render the digest as the prose an agent actually reads.
    pub fn to_prose(&self) -> String {
        let mut out = String::new();

        if let Some(p) = &self.project {
            out.push_str(&format!("# {} ({})\n", p.name, p.slug));
            out.push_str(&format!(
                "status: {} · {} open task(s), {} urgent, {} blocked · {} open question(s)\n",
                p.status, p.open_tasks, p.urgent_tasks, p.blocked_tasks, p.open_questions
            ));
            // On its own line rather than appended to the one above, because
            // it is a different kind of number: everything on that line is
            // work somebody committed to, and the Inbox is what nobody has
            // decided about yet. Printed only when there is something in it —
            // "0 untriaged" is a line every session would read forever to
            // learn nothing.
            if p.inbox > 0 {
                out.push_str(&format!(
                    "inbox: {} untriaged signal(s){}\n",
                    p.inbox,
                    match p.inbox_oldest_days {
                        Some(d) if d >= 1 => format!(", oldest waiting {d} day(s)"),
                        _ => String::new(),
                    }
                ));
            }
            if let Some(m) = &p.active_milestone {
                // The project's own word, lowercased for the middle of a
                // sentence. A session that reads "active phase: Phase 8" here
                // has been told the vocabulary before it needs it, which is one
                // round trip cheaper than being corrected by a create.
                let noun = p.milestone_noun.as_deref().unwrap_or("milestone");
                out.push_str(&format!("active {}: {m}\n", noun.to_lowercase()));
            }
        } else {
            out.push_str("# All projects\n");
            for p in &self.projects {
                out.push_str(&format!(
                    "- {} ({}) — {} · {} open, {} urgent, {} blocked · {} question(s)\n",
                    p.name,
                    p.slug,
                    p.status,
                    p.open_tasks,
                    p.urgent_tasks,
                    p.blocked_tasks,
                    p.open_questions
                ));
            }
        }

        let section = |out: &mut String, title: &str, items: &[Item]| {
            if items.is_empty() {
                return;
            }
            out.push_str(&format!("\n## {title}\n"));
            for i in items {
                let status = i.status.as_deref().unwrap_or("");
                let detail = i.detail.as_deref().unwrap_or("");
                out.push_str(&format!(
                    "- {} [{}{}{}] {}\n",
                    i.label,
                    status,
                    if detail.is_empty() { "" } else { " · " },
                    detail,
                    i.id
                ));
            }
        };

        // Deliberately the first thing after the header, and above the
        // questions and glossary: it is the section an agent should act on,
        // and burying the answer under three screens of context is how the
        // old digest managed to contain everything and answer nothing.
        if let Some(n) = &self.next_up
            && !(n.ready.is_empty() && n.waiting_on_you.is_empty() && n.blocked.is_empty())
        {
            out.push_str("\n## Next\n");
            for item in &n.ready {
                out.push_str(&format!(
                    "- **{}** `{}` — {}\n",
                    item.title, item.reference, item.why
                ));
            }
            if n.ready.is_empty() && !n.blocked.is_empty() {
                out.push_str(
                    "- Nothing is ready to pick up. Everything open is blocked or waiting on a \
                     decision — unblocking one of the below is the work.\n",
                );
            }
            if !n.waiting_on_you.is_empty() {
                out.push_str("\n**Waiting on the human**\n");
                for item in &n.waiting_on_you {
                    out.push_str(&format!("- {} `{}`\n", item.title, item.reference));
                }
            }
            if !n.blocked.is_empty() {
                out.push_str("\n**Blocked**\n");
                for item in &n.blocked {
                    out.push_str(&format!("- {} — {}\n", item.title, item.why));
                }
            }
        }

        // Above `Active`, and deliberately: this is a decision somebody owes the
        // project, and the sections below it are orientation. A phase sitting
        // here costs nothing to resolve and is invisible everywhere else.
        //
        // Written as a request, because the passive version did not work. It
        // said the phase "stays here until somebody says which", every session
        // read it at start, and three phases sat in it for five weeks: a line
        // addressed to nobody is read as background (KEEL-372).
        if !self.complete.is_empty() {
            out.push_str("\n## Finished — ask the user\n");
            out.push_str(
                "Every task in these phases is closed. Whether that means shipped or cut is \
                 the user's call, not something to infer — `done` and `wont_do` both close a \
                 task. Ask them once, and record the answer: `specline_update` the milestone \
                 with `status` set to `shipped` or `cut`, and for shipped, `shipped_at` set to \
                 the date its last task closed, shown here.\n",
            );
            for i in &self.complete {
                match &i.detail {
                    Some(detail) => {
                        out.push_str(&format!("- {} {} — {detail}\n", i.label, i.id));
                    }
                    None => out.push_str(&format!("- {} {}\n", i.label, i.id)),
                }
            }
        }

        section(&mut out, "Active", &self.active);
        section(&mut out, "Needs attention", &self.attention);
        // Above the questions and below what needs attention, which is where
        // it belongs in a reader's priority: work in flight first, then the
        // things nobody has decided about, then the things nobody has answered.
        section(&mut out, "Inbox — untriaged, oldest first", &self.inbox);
        section(&mut out, "Open questions and risks", &self.questions);
        section(&mut out, "Recent decisions", &self.decisions);
        section(&mut out, "Specs", &self.specs);
        section(&mut out, "Environments", &self.environments);

        if !self.terms.is_empty() {
            out.push_str("\n## Glossary\n");
            for t in &self.terms {
                out.push_str(&format!(
                    "- **{}**{}: {}\n",
                    t.term,
                    if t.global { " (global)" } else { "" },
                    t.definition
                ));
            }
        }

        if !self.recent.is_empty() {
            out.push_str("\n## Recently\n");
            for line in &self.recent {
                out.push_str(&format!("- {line}\n"));
            }
        }

        match &self.drift {
            None => {}
            Some(DriftSection::Unreadable { reason }) => {
                out.push_str(&format!(
                    "\n## Commits\nNot measured: {reason}. The project has a checkout \
                     recorded and its history could not be read.\n"
                ));
            }
            Some(DriftSection::Measured(d)) => drift_prose(&mut out, d),
        }

        if !self.next.is_empty() {
            out.push_str("\n## Also worth noticing\n");
            for line in &self.next {
                out.push_str(&format!("- {line}\n"));
            }
        }

        if !self.truncated.is_empty() {
            out.push_str("\n---\n");
            for t in &self.truncated {
                out.push_str(&format!(
                    "{}: showing {} of {}. Use specline_search or specline_get for the rest.\n",
                    t.section, t.shown, t.total
                ));
            }
        }
        if self.budget_exceeded {
            out.push_str(
                "\nThis digest is over its size budget because the open questions and \
                 glossary are returned in full — they are never trimmed. That usually means \
                 the question register needs pruning.\n",
            );
        }

        out
    }
}

/// The commits section: one line of counts, then the two lists that are the
/// point, each cut at [`DRIFT_SHOWN`] and saying so.
fn drift_prose(out: &mut String, d: &Drift) {
    out.push_str(&format!(
        "\n## Commits since {}\n",
        d.since.format("%Y-%m-%d")
    ));
    if d.commits == 0 {
        out.push_str("None.");
    } else if d.commits < d.commits_total {
        out.push_str(&format!(
            "{} commit(s), of which the newest {} were read: {} name a task, {} name none.",
            d.commits_total,
            d.commits,
            d.linked.len(),
            d.unlinked.len()
        ));
    } else {
        out.push_str(&format!(
            "{} commit(s): {} name a task, {} name none.",
            d.commits,
            d.linked.len(),
            d.unlinked.len()
        ));
    }
    if !d.done_without_commit.is_empty() {
        out.push_str(&format!(
            " {} task(s) closed done with no commit behind them.",
            d.done_without_commit.len()
        ));
    }
    if !d.unknown.is_empty() {
        out.push_str(&format!(
            " {} commit(s) name a task that does not exist.",
            d.unknown.len()
        ));
    }
    if !d.landed_but_open.is_empty() {
        out.push_str(&format!(
            " {} open task(s) were named by a commit more than a day ago.",
            d.landed_but_open.len()
        ));
    }
    out.push('\n');

    for c in d.unlinked.iter().take(DRIFT_SHOWN) {
        out.push_str(&format!("- {} {} — no task\n", c.sha, c.subject));
    }
    if d.unlinked.len() > DRIFT_SHOWN {
        out.push_str(&format!(
            "- …and {} more commit(s) naming no task\n",
            d.unlinked.len() - DRIFT_SHOWN
        ));
    }
    for t in d.done_without_commit.iter().take(DRIFT_SHOWN) {
        out.push_str(&format!(
            "- {} {} — closed done, no commit cited\n",
            t.reference, t.title
        ));
    }
    if d.done_without_commit.len() > DRIFT_SHOWN {
        out.push_str(&format!(
            "- …and {} more task(s) closed done with no commit\n",
            d.done_without_commit.len() - DRIFT_SHOWN
        ));
    }
    // The list with an action attached: a roadmap goes stale one
    // landed-but-open row at a time (KEEL-372).
    if !d.landed_but_open.is_empty() {
        out.push_str(
            "Still open, though a commit in this window named them over a day ago and the row \
             has not moved since. If the work is done, close each with `specline_close` citing \
             the commit; if it is not, say what is left in a note:\n",
        );
    }
    for t in d.landed_but_open.iter().take(DRIFT_SHOWN) {
        out.push_str(&format!(
            "- {} {} — named by {} on {}, still open\n",
            t.reference,
            t.title,
            t.commits.join(", "),
            t.last_named_at.format("%Y-%m-%d")
        ));
    }
    if d.landed_but_open.len() > DRIFT_SHOWN {
        out.push_str(&format!(
            "- …and {} more open task(s) named by a commit\n",
            d.landed_but_open.len() - DRIFT_SHOWN
        ));
    }
    for u in d.unknown.iter().take(DRIFT_SHOWN) {
        out.push_str(&format!("- {} names {}, which has no row\n", u.sha, u.key));
    }
    if d.unknown.len() > DRIFT_SHOWN {
        out.push_str(&format!(
            "- …and {} more commit(s) naming a task that does not exist\n",
            d.unknown.len() - DRIFT_SHOWN
        ));
    }
    if d.tasks_scanned < d.tasks_total {
        out.push_str(&format!(
            "Only the newest {} of {} task rows were read, so a commit naming an older task \
             is reported above as naming one that does not exist.\n",
            d.tasks_scanned, d.tasks_total
        ));
    }
}

/// Estimate tokens from characters. Deliberately crude — the budget is a
/// guardrail, not an accounting system, and a real tokeniser here would be a
/// dependency and a decision for no gain.
fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Which unfinished surfaces are switched on.
///
/// Passed in rather than looked up, because `specline-core` never reads the
/// environment — that boundary is what makes the CLI, the daemon and any
/// future surface cheap, and a config lookup buried in the digest would be the
/// first crack in it. Resolving the flag is the caller's job; obeying it is
/// this crate's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Surfaces {
    /// Whether the Inbox is shown at all.
    ///
    /// Off by default, which is the whole point: v0.4.0 shipped filing and the
    /// nav item without triage, so signals could go in and not come out.
    /// **This hides surfaces and never data** — signals already in the store
    /// are untouched and reappear intact when it goes on.
    pub inbox: bool,
}

impl Surfaces {
    /// Everything on. For tests, and for a caller that has already decided.
    pub const fn all() -> Self {
        Surfaces { inbox: true }
    }
}

/// Build the digest.
pub fn build(
    store: &Store,
    project: Option<&EntityId>,
    depth: Depth,
    since: Option<chrono::DateTime<chrono::Utc>>,
    surfaces: Surfaces,
    repository: drift::Source<'_>,
) -> Result<Digest> {
    let limit = depth.section_limit();

    let mut digest = Digest {
        project: None,
        projects: Vec::new(),
        active: Vec::new(),
        complete: Vec::new(),
        attention: Vec::new(),
        recent: Vec::new(),
        decisions: Vec::new(),
        questions: Vec::new(),
        specs: Vec::new(),
        inbox: Vec::new(),
        terms: Vec::new(),
        environments: Vec::new(),
        next: Vec::new(),
        next_up: None,
        drift: None,
        truncated: Vec::new(),
        budget_exceeded: false,
        estimated_tokens: 0,
    };

    match project {
        None => {
            // Cross-project roll-up: one line each, plus anything at risk.
            let projects = store.list(&EntityQuery::default().of_type(EntityType::Project))?;
            for entity in &projects.items {
                if let Entity::Project(p) = entity {
                    digest.projects.push(project_line(store, p, surfaces)?);
                }
            }
            digest.projects.sort_by(|a, b| {
                b.urgent_tasks
                    .cmp(&a.urgent_tasks)
                    .then_with(|| b.blocked_tasks.cmp(&a.blocked_tasks))
            });

            // Questions across every project are still never truncated: a
            // settled decision in project A is just as re-litigable from a
            // conversation that started in project B.
            digest.questions = open_questions(store, None)?;
            digest.terms = glossary(store, None)?;

            // UC-6 asks "what shipped this week" across every project, so the
            // roll-up needs activity too. Leaving it empty made the Sunday
            // review say "no activity yet" against a store with hundreds of
            // events, which is worse than saying nothing.
            digest.recent = recent_activity(store, None, since, limit * 2)?;

            // Milestones in flight, across everything — the other half of
            // "what is happening right now".
            let mut active = Vec::new();
            for line in &digest.projects {
                if line.active_milestone.is_some() {
                    active.extend(active_milestones(store, &line.id, 2)?);
                }
            }
            digest.active = active;

            digest.next = rollup_suggestions(&digest.projects);
        }
        Some(project_id) => {
            let Some(Entity::Project(p)) = store.get(project_id)? else {
                return Err(crate::Error::NotFound {
                    entity_type: EntityType::Project,
                    id: project_id.to_string(),
                });
            };
            let line = project_line(store, &p, surfaces)?;

            digest.active = active_milestones(store, project_id, limit)?;
            let complete = complete_milestones(store, project_id)?;
            let complete_total = complete.len();
            digest.complete = complete;
            if complete_total > limit {
                digest.complete.truncate(limit);
                digest.truncated.push(Truncation {
                    section: "complete".to_owned(),
                    shown: limit,
                    total: complete_total,
                });
            }
            let (attention, attention_total) = needs_attention(store, project_id, limit)?;
            digest.attention = attention;
            if attention_total > digest.attention.len() {
                digest.truncated.push(Truncation {
                    section: "attention".to_owned(),
                    shown: digest.attention.len(),
                    total: attention_total,
                });
            }

            digest.questions = open_questions(store, Some(project_id))?;
            digest.terms = glossary(store, Some(project_id))?;

            let (decisions, decisions_total) = recent_decisions(store, project_id, 5.min(limit))?;
            digest.decisions = decisions;
            if decisions_total > digest.decisions.len() {
                digest.truncated.push(Truncation {
                    section: "decisions".to_owned(),
                    shown: digest.decisions.len(),
                    total: decisions_total,
                });
            }

            let (specs, specs_total) = current_specs(store, project_id, limit)?;
            digest.specs = specs;
            if specs_total > digest.specs.len() {
                digest.truncated.push(Truncation {
                    section: "specs".to_owned(),
                    shown: digest.specs.len(),
                    total: specs_total,
                });
            }

            // Fewer than the other sections, deliberately. A signal is one line
            // and the Inbox is the section most likely to be long — that is
            // the condition it exists to report — so it takes a small slice
            // and says how much it left. Oldest first comes from the store.
            let inbox = if surfaces.inbox {
                store.inbox(project_id, INBOX_IN_DIGEST.min(limit))?
            } else {
                crate::Page::new(Vec::new(), 0)
            };
            digest.inbox = inbox
                .items
                .iter()
                .map(|e| {
                    // Who asked and how long it has waited, which together are
                    // what makes a signal triageable without opening it: one
                    // person's passing idea from yesterday and a request three
                    // people have made over two months want different answers.
                    let detail = match e {
                        Entity::Feedback(f) => {
                            let waited = (chrono::Utc::now() - f.audit.created_at).num_days();
                            Some(match (&f.source, waited) {
                                (Some(who), 0) => format!("{who} · today"),
                                (Some(who), d) => format!("{who} · waiting {d}d"),
                                (None, 0) => "today".to_owned(),
                                (None, d) => format!("waiting {d}d"),
                            })
                        }
                        _ => None,
                    };
                    item(store, e, detail)
                })
                .collect();
            if inbox.total > digest.inbox.len() {
                digest.truncated.push(Truncation {
                    section: "inbox".to_owned(),
                    shown: digest.inbox.len(),
                    total: inbox.total,
                });
            }

            digest.environments = environments(store, project_id)?;
            digest.recent = recent_activity(store, Some(project_id), since, limit)?;
            let ranked = crate::next::rank(store, project_id)?;
            let total_ready = ranked.ready.len();
            let next_up: NextUpJson = ranked.into();
            if total_ready > next_up.ready.len() {
                digest.truncated.push(Truncation {
                    section: "next_up.ready".to_owned(),
                    shown: next_up.ready.len(),
                    total: total_ready,
                });
            }
            digest.drift = match repository {
                drift::Source::NoCheckout => None,
                drift::Source::Unreadable(reason) => Some(DriftSection::Unreadable {
                    reason: reason.to_owned(),
                }),
                drift::Source::Commits {
                    since,
                    commits,
                    total,
                } => Some(DriftSection::Measured(drift::reconcile(
                    store,
                    project_id,
                    commits,
                    total,
                    since,
                    crate::now(),
                )?)),
            };
            digest.next = suggestions(&line, &digest);
            digest.next_up = Some(next_up);
            digest.project = Some(line);
        }
    }

    // Measure, then trim only the sections that may be trimmed.
    let mut rendered = digest.to_prose();
    if rendered.len() > depth.budget_chars() {
        // Trim in order of what an agent can most cheaply re-fetch. Recent
        // activity first — `specline_activity` is one call away and rarely
        // changes a decision.
        // The Inbox is trimmed late, ahead only of what needs attention: it is
        // already the smallest section, and unlike the others it is the one a
        // session is meant to *act* on rather than re-fetch — there is no
        // one-call equivalent of `specline_activity` for untriaged signals.
        for section in ["recent", "specs", "decisions", "inbox", "attention"] {
            if rendered.len() <= depth.budget_chars() {
                break;
            }
            trim_section(&mut digest, section);
            rendered = digest.to_prose();
        }
        // Questions and terms are never touched. If the digest is still over
        // budget, say so rather than trimming them.
        if rendered.len() > depth.budget_chars() {
            digest.budget_exceeded = true;
            rendered = digest.to_prose();
        }
    }

    digest.estimated_tokens = estimate_tokens(&rendered);
    Ok(digest)
}

/// Halve a trimmable section, recording what was dropped.
fn trim_section(digest: &mut Digest, section: &str) {
    let (len, keep) = match section {
        "recent" => (digest.recent.len(), digest.recent.len() / 2),
        "specs" => (digest.specs.len(), digest.specs.len() / 2),
        "decisions" => (digest.decisions.len(), digest.decisions.len() / 2),
        "attention" => (digest.attention.len(), digest.attention.len() / 2),
        // Trimmable, unlike the questions and the glossary. It is already
        // capped at eight and always reports its true total, so halving it
        // under pressure loses a few lines and no information — whereas the
        // question register exists precisely so nothing gets re-litigated,
        // which a shortened list would let happen.
        "inbox" => (digest.inbox.len(), digest.inbox.len() / 2),
        _ => return,
    };
    if len == 0 {
        return;
    }
    match section {
        "recent" => digest.recent.truncate(keep),
        "specs" => digest.specs.truncate(keep),
        "decisions" => digest.decisions.truncate(keep),
        "attention" => digest.attention.truncate(keep),
        "inbox" => digest.inbox.truncate(keep),
        _ => {}
    }
    match digest.truncated.iter_mut().find(|t| t.section == section) {
        Some(existing) => existing.shown = keep,
        None => digest.truncated.push(Truncation {
            section: section.to_owned(),
            shown: keep,
            total: len,
        }),
    }
}

fn project_line(store: &Store, p: &crate::Project, surfaces: Surfaces) -> Result<ProjectLine> {
    // Counted by the database. This used to load up to two thousand full task
    // rows — every column of every one — and loop over them to produce two
    // integers, once per project, on the most-called tool in the surface. It
    // also silently capped at two thousand, so a large enough project would
    // have reported a number that was merely plausible.
    let (open, urgent) = store.task_counts(&p.id)?;

    // The one derivation, shared with the ranking and the generated tracker.
    // Counting a `blocked` status here is what let the digest and the file
    // state different numbers about the same project (TQ-25).
    let blocked = crate::next::blocked_tasks(store, &p.id)?.len();

    let questions = store.list(
        &EntityQuery::in_project(p.id.clone())
            .of_type(EntityType::Question)
            .with_status([QuestionStatus::Open.as_str()])
            .limited(2000),
    )?;

    // Counted, not listed. The Inbox can be long by design — that is the
    // condition KEEL-303 describes — so the digest carries its size and its
    // age rather than its contents, and `specline_search` fetches the rows
    // when somebody is actually going to triage them.
    let (signals, oldest_signal) = if surfaces.inbox {
        store.untriaged_signals(&p.id)?
    } else {
        (0, None)
    };

    // Which phases are in flight is derived, not filtered on a column. The
    // column used to say, and for a week it said Phase 9 — a phase that had
    // finished — to every session that opened this project (B-57).
    let states = store.milestone_progress(&p.id)?;
    let milestones = store.list(
        &EntityQuery::in_project(p.id.clone())
            .of_type(EntityType::Milestone)
            .limited(200),
    )?;
    let mut in_flight: Vec<&Entity> = milestones
        .items
        .iter()
        .filter(|m| {
            matches!(
                states.get(m.id()).map(|p| p.state),
                Some(crate::MilestoneState::Active | crate::MilestoneState::Blocked)
            )
        })
        .collect();
    in_flight.sort_by_key(|m| m.label().to_owned());

    Ok(ProjectLine {
        id: p.id.clone(),
        name: p.name.clone(),
        slug: p.slug.clone(),
        key: p.key.clone(),
        status: p.status.as_str().to_owned(),
        open_tasks: open,
        urgent_tasks: urgent,
        blocked_tasks: blocked,
        open_questions: questions.total,
        inbox: signals,
        inbox_oldest_days: oldest_signal.map(|t| (chrono::Utc::now() - t).num_days()),
        // Every phase in flight, not the first one found. Two are normal —
        // Phase 11 and 12 are both live as this is written — and a singular
        // field over plural data picks one arbitrarily and hides the rest.
        active_milestone: match in_flight.as_slice() {
            [] => None,
            [one] => Some(one.label().to_owned()),
            many => Some(
                many.iter()
                    .map(|m| m.label().to_owned())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        },
        milestone_noun: p.milestone_noun.clone(),
    })
}

/// The project's phases whose derived state is one of `wanted`.
///
/// Each item carries the **derived** state as its status rather than the
/// declared one. They are different facts and only one of them is worth reading
/// here: a phase in flight is `open` in the column and always was, so printing
/// the column told a session nothing the section heading had not (B-57).
fn milestones_in_state(
    store: &Store,
    project: &EntityId,
    wanted: &[crate::MilestoneState],
) -> Result<Vec<Item>> {
    let states = store.milestone_progress(project)?;
    let page = store.list(
        &EntityQuery::in_project(project.clone())
            .of_type(EntityType::Milestone)
            .limited(200),
    )?;
    Ok(page
        .items
        .iter()
        .filter_map(|m| states.get(m.id()).map(|p| (m, *p)))
        .filter(|(_, progress)| wanted.contains(&progress.state))
        .map(|(e, progress)| {
            // How far through, not when it is due. The date was the only detail
            // here and four of fifteen phases had one, all of them the same day
            // the store was seeded — so this line said nothing for eleven
            // phases and something misleading for four (KEEL-332).
            let detail = match progress.tally.total {
                0 => None,
                total => Some(format!("{} of {total} done", progress.tally.closed)),
            };
            Item {
                status: Some(progress.state.as_str().to_owned()),
                ..item(store, e, detail)
            }
        })
        .collect())
}

/// Phases in flight.
fn active_milestones(store: &Store, project: &EntityId, limit: usize) -> Result<Vec<Item>> {
    let mut items = milestones_in_state(
        store,
        project,
        &[
            crate::MilestoneState::Active,
            crate::MilestoneState::Blocked,
        ],
    )?;
    items.truncate(limit);
    Ok(items)
}

/// Phases whose every task is closed and which nobody has declared shipped or
/// cut.
///
/// This section exists because the state had nowhere to appear. `complete` is
/// not `active`, so a finished phase dropped out of the digest at the exact
/// moment it needed a person — and three of this project's own phases sat that
/// way unnoticed, because closing the last task told nobody (KEEL-284).
///
/// Returned in full, for the caller to cut and report. A phase silently dropped
/// from this list is the very failure the list exists to end, so it is one of
/// the places hard constraint 4 has to be honoured rather than assumed.
///
/// Each carries when its last task closed, because that is the date the
/// answer needs: a phase declared shipped today that finished five weeks ago
/// would put the roadmap's history wrong by five weeks.
fn complete_milestones(store: &Store, project: &EntityId) -> Result<Vec<Item>> {
    let mut items = milestones_in_state(store, project, &[crate::MilestoneState::Complete])?;
    if items.is_empty() {
        return Ok(items);
    }
    let tasks = store.list(
        &EntityQuery::in_project(project.clone())
            .of_type(EntityType::Task)
            .limited(crate::drift::TASK_SCAN_CAP),
    )?;
    for item in &mut items {
        // Done rows only: a late `wont_do` clean-up is not when the phase
        // shipped, and an archived row is one somebody set aside.
        let last = tasks
            .items
            .iter()
            .filter_map(|e| match e {
                Entity::Task(t)
                    if t.milestone_id.as_ref() == Some(&item.id)
                        && t.audit.archived_at.is_none()
                        && t.close_reason == Some(crate::CloseReason::Done) =>
                {
                    t.closed_at
                }
                _ => None,
            })
            .max();
        if let Some(last) = last {
            let tally = item.detail.take().unwrap_or_default();
            item.detail = Some(format!(
                "{tally}, the last closed {}",
                last.format("%Y-%m-%d")
            ));
        }
    }
    Ok(items)
}

fn needs_attention(store: &Store, project: &EntityId, limit: usize) -> Result<(Vec<Item>, usize)> {
    let page = store.list(
        &EntityQuery::in_project(project.clone())
            .of_type(EntityType::Task)
            .with_status([
                TaskStatus::Todo.as_str(),
                TaskStatus::InProgress.as_str(),
                TaskStatus::Review.as_str(),
            ])
            .limited(2000),
    )?;

    // Blocked is derived from the edges, so this section asks the graph rather
    // than reading a column that used to claim the same thing.
    let blocked_ids = crate::next::blocked_tasks(store, project)?;

    let mut urgent: Vec<(u8, Item)> = page
        .items
        .iter()
        .filter_map(|e| match e {
            Entity::Task(t) if t.priority.is_urgent() || blocked_ids.contains(&t.id) => {
                // Blocked first, then by priority — a blocked p1 needs a human
                // more than an unblocked p0 does.
                let rank = if blocked_ids.contains(&t.id) {
                    0
                } else if t.priority == crate::TaskPriority::P0 {
                    1
                } else {
                    2
                };
                Some((rank, item(store, e, Some(format!("{}", t.priority)))))
            }
            _ => None,
        })
        .collect();
    urgent.sort_by_key(|(rank, _)| *rank);

    let total = urgent.len();
    Ok((
        urgent.into_iter().take(limit).map(|(_, i)| i).collect(),
        total,
    ))
}

/// Every unresolved question and risk. **Never truncated.**
fn open_questions(store: &Store, project: Option<&EntityId>) -> Result<Vec<Item>> {
    let mut query = EntityQuery::default()
        .of_type(EntityType::Question)
        .with_status([QuestionStatus::Open.as_str()]);
    query.project_id = project.cloned();
    // A high explicit limit rather than the default: this list must not be cut
    // by the store's paging either. If a project ever has 5,000 open
    // questions, the digest saying so is the useful outcome.
    query.limit = Some(5_000);

    let page = store.list(&query)?;
    Ok(page
        .items
        .iter()
        .map(|e| {
            let detail = match e {
                Entity::Question(q) => {
                    let mut parts = vec![q.kind.to_string()];
                    if let Some(s) = q.severity {
                        parts.push(s.to_string());
                    }
                    Some(parts.join(" · "))
                }
                _ => None,
            };
            item(store, e, detail)
        })
        .collect())
}

/// The glossary. **Never truncated.**
fn glossary(store: &Store, project: Option<&EntityId>) -> Result<Vec<TermEntry>> {
    let mut query = EntityQuery::default().of_type(EntityType::Term);
    query.project_id = project.cloned();
    query.limit = Some(5_000);

    let page = store.list(&query)?;
    let mut terms: Vec<TermEntry> = page
        .items
        .iter()
        .filter_map(|e| match e {
            Entity::Term(t) => Some(TermEntry {
                term: t.term.clone(),
                definition: t.definition.clone(),
                global: t.project_id.is_none(),
            }),
            _ => None,
        })
        .collect();

    // Project-first resolution (Q-4): a project-scoped term overrides a global
    // one of the same name, so drop the global when both are present.
    let scoped: std::collections::HashSet<String> = terms
        .iter()
        .filter(|t| !t.global)
        .map(|t| t.term.to_lowercase())
        .collect();
    terms.retain(|t| !t.global || !scoped.contains(&t.term.to_lowercase()));
    terms.sort_by_key(|t| t.term.to_lowercase());
    Ok(terms)
}

fn recent_decisions(store: &Store, project: &EntityId, limit: usize) -> Result<(Vec<Item>, usize)> {
    let page = store.list(
        &EntityQuery::in_project(project.clone())
            .of_type(EntityType::Decision)
            .with_status([crate::DecisionStatus::Accepted.as_str()])
            .limited(2000),
    )?;
    let total = page.items.len();
    Ok((
        page.items
            .iter()
            .take(limit)
            .map(|e| item(store, e, None))
            .collect(),
        total,
    ))
}

fn current_specs(store: &Store, project: &EntityId, limit: usize) -> Result<(Vec<Item>, usize)> {
    let page = store.list(
        &EntityQuery::in_project(project.clone())
            .of_type(EntityType::Spec)
            .with_status([
                crate::SpecStatus::Draft.as_str(),
                crate::SpecStatus::Review.as_str(),
                crate::SpecStatus::Approved.as_str(),
            ])
            .limited(2000),
    )?;
    let total = page.items.len();
    Ok((
        page.items
            .iter()
            .take(limit)
            .map(|e| {
                let detail = match e {
                    Entity::Spec(s) => Some(s.kind.to_string()),
                    _ => None,
                };
                item(store, e, detail)
            })
            .collect(),
        total,
    ))
}

fn environments(store: &Store, project: &EntityId) -> Result<Vec<Item>> {
    let page = store.list(
        &EntityQuery::in_project(project.clone())
            .of_type(EntityType::Environment)
            .limited(20),
    )?;
    Ok(page
        .items
        .iter()
        .map(|e| {
            let detail = match e {
                Entity::Environment(env) => env.deployed_version.clone(),
                _ => None,
            };
            item(store, e, detail)
        })
        .collect())
}

/// Recent activity. `project` of `None` spans every project.
fn recent_activity(
    store: &Store,
    project: Option<&EntityId>,
    since: Option<chrono::DateTime<chrono::Utc>>,
    limit: usize,
) -> Result<Vec<String>> {
    // `since` still goes through the feed: it is a bounded window, so reading
    // it forwards cannot lose the newest rows. Everything else asks for the
    // newest directly — reading 2,000 oldest-first and keeping the tail was
    // right only while the log was under 2,000, and wrong quietly after that.
    let page = match since {
        Some(t) => {
            let mut page = store.events(&Cursor::Since(t), project, 2_000)?;
            page.items.reverse();
            page
        }
        None => store.recent_events(
            project.map_or(EventScope::Everything, EventScope::Project),
            limit,
        )?,
    };
    Ok(page
        .items
        .iter()
        .take(limit)
        .map(|e| {
            format!(
                "{} — {} ({})",
                e.created_at.format("%Y-%m-%d"),
                e.summary,
                e.actor
            )
        })
        .collect())
}

/// The ranked answer, in the shape the wire and the app want.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct NextUpJson {
    /// Pick these up, best first.
    pub ready: Vec<NextItem>,
    /// Decisions a human owes the project.
    pub waiting_on_you: Vec<NextItem>,
    /// Stuck, each with its blocker named.
    pub blocked: Vec<NextItem>,
}

/// One ranked task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NextItem {
    /// `tsk_…`, so the caller can act on it without another lookup.
    pub id: String,
    /// The readable identifier — `KEEL-42`. What a person will type back.
    pub reference: String,
    /// Its title.
    pub title: String,
    /// `p0`…`p3`.
    pub priority: String,
    /// How many open tasks finishing this would release.
    pub unblocks: usize,
    /// Why it is here, or what is in the way.
    pub why: String,
}

impl From<crate::Candidate> for NextItem {
    fn from(c: crate::Candidate) -> Self {
        NextItem {
            id: c.id.to_string(),
            reference: c.reference,
            title: c.title,
            priority: c.priority,
            unblocks: c.unblocks,
            why: c.why,
        }
    }
}

impl From<crate::NextUp> for NextUpJson {
    fn from(n: crate::NextUp) -> Self {
        NextUpJson {
            // Three is a deliberate cap on `ready`: a ranked list of thirty is
            // the same "you work it out" the counts were. The rest is one
            // specline_search away, and the truncation is reported like every
            // other cut list.
            ready: n.ready.into_iter().take(3).map(Into::into).collect(),
            waiting_on_you: n.waiting_on_you.into_iter().map(Into::into).collect(),
            blocked: n.blocked.into_iter().map(Into::into).collect(),
        }
    }
}

/// Suggested next actions, derived from state rather than invented.
fn suggestions(line: &ProjectLine, digest: &Digest) -> Vec<String> {
    let mut out = Vec::new();

    // Blocked tasks used to be reported here as a count plus a query to run.
    // The query returned nothing, because nothing was linked. `next_up.blocked`
    // now names the blocker instead, so this said less than nothing.
    if line.open_questions > 0 {
        out.push(format!(
            "{} question(s) are unresolved. Resolving one usually unblocks more than it costs.",
            line.open_questions
        ));
    }
    let review = digest
        .attention
        .iter()
        .filter(|i| i.status.as_deref() == Some("review"))
        .count();
    if review > 0 {
        out.push(format!(
            "{review} task(s) are in review — confirm with the human whether they are done."
        ));
    }
    if digest.active.is_empty() && line.open_tasks > 0 {
        out.push(
            "No milestone is active, but there is open work. Consider grouping it under one."
                .to_owned(),
        );
    }
    // The drift number, where the agent reads. The section itself sits below
    // the glossary; this line is what makes it noticed, and it is the one
    // number the contract measured by hand and could not keep measuring.
    if let Some(DriftSection::Measured(d)) = &digest.drift {
        if !d.unlinked.is_empty() {
            out.push(format!(
                "{} of {} commit(s) since {} name no task. Every commit should have a row \
                 behind it — see the Commits section.",
                d.unlinked.len(),
                d.commits,
                d.since.format("%Y-%m-%d")
            ));
        }
        if !d.done_without_commit.is_empty() {
            out.push(format!(
                "{} task(s) closed done since {} with no commit or PR cited. Closing with \
                 `commit:` evidence is what makes the changelog answer \"what shipped\".",
                d.done_without_commit.len(),
                d.since.format("%Y-%m-%d")
            ));
        }
        if !d.landed_but_open.is_empty() {
            out.push(format!(
                "{} open task(s) were named by a commit over a day ago and not touched since. \
                 If the work is done, close them — see the Commits section.",
                d.landed_but_open.len()
            ));
        }
    }
    if line.urgent_tasks == 0 && line.open_tasks > 0 {
        out.push(
            "Nothing is marked p0 or p1. If something is actually urgent, say so on the task."
                .to_owned(),
        );
    }
    // KEEL-303, and the reason the surface is called an Inbox: a pile that
    // grows until it is too expensive to read is the failure, and nothing
    // pointing it out is what lets it happen. Suggested on age rather than
    // size, because a big Inbox somebody is working through is fine and a
    // small one nobody has looked at in a month is not.
    if let (n, Some(days)) = (line.inbox, line.inbox_oldest_days)
        && n > 0
        && days >= INBOX_STALE_DAYS
    {
        out.push(format!(
            "{n} signal(s) are untriaged and the oldest has waited {days} day(s). Triaging is \
             cheaper than deciding the same thing twice."
        ));
    }
    out
}

/// How many signals the digest lists before it starts saying "and N more".
///
/// Small on purpose. The Inbox being long is the thing the digest reports; it
/// is not the thing the digest is for, and REQ-3's budget belongs to the work.
/// Enough to triage the oldest handful in one pass, and the total is always
/// there for anyone who wants the rest.
const INBOX_IN_DIGEST: usize = 8;

/// How long an untriaged signal may sit before the digest mentions it.
///
/// A fortnight, chosen so that a normal week of capture never trips it and a
/// pile that has genuinely been abandoned always does. It is a suggestion and
/// not a warning, so being slightly wrong in either direction is cheap.
const INBOX_STALE_DAYS: i64 = 14;

/// Suggestions for the cross-project view.
fn rollup_suggestions(projects: &[ProjectLine]) -> Vec<String> {
    let mut out = Vec::new();
    let at_risk: Vec<&ProjectLine> = projects
        .iter()
        .filter(|p| p.blocked_tasks > 0 || p.urgent_tasks > 0)
        .collect();
    if !at_risk.is_empty() {
        out.push(format!(
            "At risk: {}. Open one with specline_context(project: …).",
            at_risk
                .iter()
                .map(|p| p.slug.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let stale: Vec<&ProjectLine> = projects
        .iter()
        .filter(|p| p.status == "active" && p.open_tasks == 0)
        .collect();
    if !stale.is_empty() {
        out.push(format!(
            "Active but with no open work: {}. Either they shipped or the tracker is stale.",
            stale
                .iter()
                .map(|p| p.slug.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    out
}

/// Compact an entity into a digest item.
///
/// Takes the store only to resolve a task's project key, which is the one part
/// of a readable identifier the task row does not carry. That is a point lookup
/// per task in a list capped at ten — the alternative is threading the key
/// through six call sites so that the digest can say `KEEL-42`.
fn item(store: &Store, entity: &Entity, detail: Option<String>) -> Item {
    let reference = match entity {
        Entity::Task(t) => match store.get(&t.project_id) {
            Ok(Some(Entity::Project(p))) => Some(format!("{}-{}", p.key, t.number)),
            _ => None,
        },
        _ => None,
    };
    Item {
        id: entity.id().clone(),
        entity_type: entity.entity_type().as_str().to_owned(),
        label: entity.label().to_owned(),
        reference,
        status: entity.status().map(str::to_owned),
        detail,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn depth_parses_and_rejects_nonsense() {
        assert_eq!(Depth::parse("brief").unwrap(), Depth::Brief);
        assert_eq!(Depth::parse("standard").unwrap(), Depth::Standard);
        assert_eq!(Depth::parse("full").unwrap(), Depth::Full);
        let err = Depth::parse("verbose").unwrap_err();
        assert!(err.contains("brief | standard | full"), "{err}");
    }

    #[test]
    fn budgets_increase_with_depth() {
        assert!(Depth::Brief.budget_chars() < Depth::Standard.budget_chars());
        assert!(Depth::Standard.budget_chars() < Depth::Full.budget_chars());
    }

    #[test]
    fn the_standard_budget_is_about_four_thousand_tokens() {
        // REQ-3. Four characters per token is crude but it is the guardrail,
        // not an accounting system.
        assert_eq!(estimate_tokens(&"x".repeat(14_000)), 3_500);
    }

    #[test]
    fn trimming_never_touches_questions_or_terms() {
        let mut d = Digest {
            project: None,
            projects: vec![],
            active: vec![],
            complete: vec![],
            attention: vec![],
            recent: vec!["a".into(), "b".into(), "c".into(), "d".into()],
            decisions: vec![],
            questions: vec![Item {
                id: EntityId::generate(EntityType::Question),
                entity_type: "question".into(),
                label: "Where does the store live?".into(),
                reference: None,
                status: Some("open".into()),
                detail: None,
            }],
            specs: vec![],
            inbox: vec![],
            terms: vec![TermEntry {
                term: "Digest".into(),
                definition: "…".into(),
                global: false,
            }],
            environments: vec![],
            next: vec![],
            next_up: None,
            truncated: vec![],
            budget_exceeded: false,
            estimated_tokens: 0,
            drift: None,
        };

        for section in ["recent", "questions", "terms"] {
            trim_section(&mut d, section);
        }
        assert_eq!(d.recent.len(), 2, "recent is trimmable");
        assert_eq!(d.questions.len(), 1, "questions must never be trimmed");
        assert_eq!(d.terms.len(), 1, "terms must never be trimmed");
    }

    #[test]
    fn trimming_records_what_it_dropped() {
        let mut d = Digest {
            project: None,
            projects: vec![],
            active: vec![],
            complete: vec![],
            attention: vec![],
            recent: (0..10).map(|i| format!("event {i}")).collect(),
            decisions: vec![],
            questions: vec![],
            specs: vec![],
            inbox: vec![],
            terms: vec![],
            environments: vec![],
            next: vec![],
            next_up: None,
            truncated: vec![],
            budget_exceeded: false,
            estimated_tokens: 0,
            drift: None,
        };
        trim_section(&mut d, "recent");
        assert_eq!(d.truncated.len(), 1);
        assert_eq!(d.truncated[0].section, "recent");
        assert_eq!(d.truncated[0].shown, 5);
        assert_eq!(d.truncated[0].total, 10);
        assert!(d.to_prose().contains("showing 5 of 10"));
    }
}
