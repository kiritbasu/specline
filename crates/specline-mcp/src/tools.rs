//! The thirteen tool definitions.
//!
//! Thirteen, not forty. Models choose correctly among a dozen and badly among
//! forty, and `product/CLAUDE.md` names expanding this surface as an
//! anti-pattern explicitly: more tools means worse selection, not more
//! capability. The argument for the last three is on [`all`].
//!
//! # These descriptions are the product
//!
//! The MCP surface is what an agent actually experiences, and the tool
//! description is the only documentation it gets. So each one says *when to
//! reach for this tool*, not merely what it does — a description that reads
//! like a function signature produces an agent that calls the wrong tool
//! confidently.

use serde_json::{Value, json};
use specline_core::{CloseReason, EntityType, Relation};

/// A tool as advertised over `tools/list`.
#[derive(Debug, Clone)]
pub struct Tool {
    /// The name an agent calls.
    pub name: &'static str,
    /// Short label for a UI.
    pub title: &'static str,
    /// When to use it and what it returns.
    pub description: String,
    /// JSON Schema for the arguments.
    pub input_schema: Value,
    /// Whether the tool mutates anything. Advertised so a host can gate
    /// writes without inferring intent from the name.
    pub read_only: bool,
    /// Whether the tool can change or hide something that already exists.
    ///
    /// Not the same as "deletes". Nothing in Specline is ever `DELETE`d, and this
    /// used to be hardcoded `false` for every tool on that basis — but the
    /// annotation a host gates on means "may perform non-additive updates",
    /// and `specline_update` overwrites fields and can archive a row while
    /// `specline_link` can remove an edge. Both are non-additive whether or not
    /// they are recoverable.
    pub destructive: bool,
    /// Whether calling twice with the same arguments is the same as calling
    /// once.
    ///
    /// True for everything that carries an idempotency key. False for
    /// `specline_note`, which appends: two identical calls make two notes, and a
    /// client that retries on timeout would otherwise duplicate silently.
    /// Deduplicating by body was the alternative and is worse — "retested,
    /// still flaky" is a legitimate thing to note twice a week apart.
    pub idempotent: bool,
}

impl Tool {
    /// The `tools/list` representation.
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "title": self.title,
            "description": self.description,
            "inputSchema": self.input_schema,
            "annotations": {
                "readOnlyHint": self.read_only,
                "destructiveHint": self.destructive,
                "idempotentHint": self.idempotent,
            }
        })
    }
}

/// The three ambient arguments every tool accepts.
///
/// Documented once in SPEC §6.2 rather than repeated per tool, and injected
/// once here for the same reason. `session_id` is the one that matters: it is
/// the provenance unit G3 and REQ-2 rest on, and the daemon never invents one.
fn with_ambient(mut schema: Value, write: bool) -> Value {
    let Some(props) = schema.get_mut("properties").and_then(Value::as_object_mut) else {
        return schema;
    };

    props.insert(
        "session_id".to_owned(),
        json!({
            "type": "string",
            "description": "A stable identifier for this conversation, minted once at first \
                            use and passed on every call. Specline never invents one: a stateless \
                            transport has no session to borrow, so provenance is cooperative. \
                            Omitting it still works, but the write is attributed only to \
                            'some Claude session'."
        }),
    );
    props.insert(
        "surface".to_owned(),
        json!({
            "type": "string",
            "enum": ["chat", "cowork", "code", "ui", "cli"],
            "description": "Where this call came from."
        }),
    );
    if write {
        props.insert(
            "idempotency_key".to_owned(),
            json!({
                "type": "string",
                "description": "Makes a retry a no-op instead of a duplicate. Derived from the \
                                project, type and normalised title when omitted, which is \
                                usually what you want — supply one only when two genuinely \
                                different things share a title."
            }),
        );
    }
    schema
}

/// The house style, stated once and attached to every prose field.
///
/// This is the layer that actually changes what gets written. A validator can
/// refuse an empty field or a body that restates its title, but it cannot tell
/// a limp sentence from a sharp one — so the register has to reach the model at
/// the moment of writing, which is here. Short on purpose: this string is paid
/// for on every `tools/list`, and a style guide nobody finishes reading teaches
/// less than three lines that land. B-46.
const HOUSE_STYLE: &str = "Write it the way you would say it to a colleague. Plain words, \
     no padding, no sentence that exists to sound considered. Avoid \"leverage\", \"utilize\", \
     \"robust\", \"seamless\", \"delve\", \"in order to\", \"it's worth noting\" — these are \
     refused. Quoting an error message or what someone actually said is fine: put it in a code \
     fence or a block quote and it is exempt.";

/// Enumerate a closed set for a schema.
fn enum_of<T: AsRef<str>>(values: impl IntoIterator<Item = T>) -> Value {
    Value::Array(
        values
            .into_iter()
            .map(|v| Value::String(v.as_ref().to_owned()))
            .collect(),
    )
}

/// Every entity type name, for `type` arguments.
fn type_enum() -> Value {
    enum_of(EntityType::wire_names())
}

/// Every relation name.
fn relation_enum() -> Value {
    enum_of(Relation::ALL.iter().map(|r| r.as_str()))
}

/// The thirteen tools, in a stable order.
///
/// Order is deliberate and must not change casually: the specification asks
/// for a deterministic `tools/list` so clients can cache it and so the list
/// lands identically in every prompt, which is worth real money in cache hits.
/// The order is also pedagogical — `specline_context` first because it is the
/// entry point, writes after reads, and the three work verbs last because they
/// are what you reach for once you know what the project is.
///
/// # Why ten and not nine
///
/// Nine was the cap, and the reasoning behind it stands: more tools makes a
/// model choose worse, not do more. `specline_note` earns the tenth slot on the
/// one argument that outranks it. Every measurement this project has taken is
/// of a single behaviour — whether a session records what it learned without
/// being asked — and the mechanism that decides it is whether the recording
/// action is *findable* at the moment the finding happens. A `note` parameter
/// on `specline_update` is not findable; a tool whose name and description are
/// about findings is.
///
/// The modelling agrees. Notes are append-only and carry no version, while
/// `specline_update` is optimistic-concurrency and takes one. Folding them
/// together would give `specline_update` a mode that ignores its own contract.
///
/// # Why thirteen and not ten
///
/// TQ-31, KB's call. The same argument, applied to the same evidence: what
/// decides whether an action happens is whether it is findable at the moment
/// of use. Claiming and closing were both already possible through
/// `specline_update`, and both were simply not done — across sixty-six tasks the
/// number of transitions into `in_progress` before work began was zero.
///
/// I recommended twelve, on the grounds that two ways to close a task is how
/// the two come to disagree. That reasoning does not survive: the storage layer
/// enforces the reason-message-evidence rule on every path into a terminal
/// status regardless, so drift is impossible under any option. With that
/// removed, twelve was the least principled of the three — it gives claiming a
/// front door and closing none, when they are the same shape of thing, purely
/// to match a number in a spec.
///
/// The cost, accepted: roughly 200 tokens per request and marginally more
/// chance of a wrong selection on an unrelated call. `specline_update`'s
/// description points at `specline_close` so the overlap is signposted rather than
/// left to chance.
///
/// **Thirteen is the new ceiling**, and it should be defended the way ten was.
/// A fourteenth needs an argument at least as good as this one.
pub fn all() -> Vec<Tool> {
    vec![
        Tool {
            name: "specline_context",
            title: "Orient on a project",
            description:
                "START HERE in any new conversation about a project. Returns a compact digest: \
                 what the project is, the active milestone, urgent and blocked work, recent \
                 decisions, every unresolved question, the glossary, what is deployed, and a \
                 suggested next action.\n\n\
                 Call this before reading files or asking the human what is going on — it is \
                 one call and roughly 3–4k tokens. With no `project`, returns a one-line \
                 roll-up of every project plus anything at risk.\n\n\
                 Pass `cwd` when you are in a repository. It scopes the digest to whichever \
                 project owns that checkout, and if none does it tells you so directly — \
                 rather than returning other projects and leaving you to conclude that yours \
                 is missing.\n\n\
                 Open questions and glossary terms are never truncated. A missing open question \
                 makes you re-litigate something already settled; a missing glossary term makes \
                 you use the wrong word for a domain concept. Everything else degrades and the \
                 response reports what it dropped."
                    .to_owned(),
            read_only: true,
            destructive: false,
            idempotent: true,
            input_schema: with_ambient(
                json!({
                    "type": "object",
                    "properties": {
                        "project": {
                            "type": "string",
                            "description": "Project id, slug or name. Omit for a cross-project roll-up."
                        },
                        "cwd": {
                            "type": "string",
                            "description": "The absolute directory you are working in. Resolves to \
                                            the project whose checkout contains it, and — when \
                                            nothing matches — says so plainly instead of leaving \
                                            you to infer it from a list of other projects. Always \
                                            pass this when you are working in a repository."
                        },
                        "depth": {
                            "type": "string",
                            "enum": ["brief", "standard", "full"],
                            "default": "standard",
                            "description": "How much to include. `brief` is a few hundred tokens; \
                                            `full` drops most limits."
                        },
                        "since": {
                            "type": "string",
                            "format": "date-time",
                            "description": "Only summarise activity after this instant. Useful when \
                                            resuming a conversation you already have context for."
                        }
                    }
                }),
                false,
            ),
        },
        Tool {
            name: "specline_search",
            title: "Search everything",
            description:
                "Hybrid keyword and semantic search across every artifact that carries text, in \
                 every project. Use it to answer 'what do we know about X', 'has anyone raised \
                 this before', or 'what did customers say about onboarding'.\n\n\
                 Searches specs, decisions, questions, feedback and design captions by meaning \
                 and by keyword together, and tasks, milestones, terms, environments, artifacts \
                 and projects by keyword. Metrics are deliberately excluded — they are numbers, \
                 and reaching them is a filter rather than a search.\n\n\
                 Prefer a natural question over keywords; the semantic half is what makes \
                 'why is billing slow' find a decision titled 'Aggregate hourly, not per-minute'.\n\n\
                 Semantic search needs a daemon started with embeddings, and not every build \
                 carries one — so `searched` in the response names the halves that actually ran, \
                 and `not_searched` says why the others did not. When only the keyword half ran, \
                 no matches means no words matched; it is not evidence that the store holds \
                 nothing on the subject."
                    .to_owned(),
            read_only: true,
            destructive: false,
            idempotent: true,
            input_schema: with_ambient(
                json!({
                    "type": "object",
                    "required": ["query"],
                    "properties": {
                        "query": { "type": "string", "description": "What you are looking for." },
                        "project": { "type": "string", "description": "Restrict to one project." },
                        "types": {
                            "type": "array",
                            "items": { "type": "string", "enum": type_enum() },
                            "description": "Restrict to these artifact types."
                        },
                        "since": { "type": "string", "format": "date-time" },
                        "until": { "type": "string", "format": "date-time" },
                        "limit": { "type": "integer", "default": 20, "minimum": 1, "maximum": 100 }
                    }
                }),
                false,
            ),
        },
        Tool {
            name: "specline_get",
            title: "Fetch by id",
            description:
                "Fetch one or more artifacts by id, optionally with their prose body, their \
                 linked neighbours, or a diff between two revisions.\n\n\
                 Use `depth` to pull in the graph around something — `specline_get(ids: [spec_id], \
                 depth: 2)` answers 'what implements this spec, and what do those things \
                 depend on' in one call. Use `version` to read an older revision and \
                 `diff_against` to see what changed between two."
                    .to_owned(),
            read_only: true,
            destructive: false,
            idempotent: true,
            input_schema: with_ambient(
                json!({
                    "type": "object",
                    "required": ["ids"],
                    "properties": {
                        "ids": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Prefixed ULIDs, e.g. tsk_01H8… The prefix says what \
                                            the artifact is, so you never need to say."
                        },
                        "include_body": {
                            "type": "boolean",
                            "default": true,
                            "description": "Include the prose body for artifacts that have one."
                        },
                        "version": {
                            "type": "integer",
                            "description": "Read this document revision instead of the current one."
                        },
                        "diff_against": {
                            "type": "integer",
                            "description": "Also return a unified diff between `version` (or the \
                                            current revision) and this one."
                        },
                        "depth": {
                            "type": "integer",
                            "default": 0,
                            "minimum": 0,
                            "maximum": 16,
                            "description": "Also return linked neighbours to this depth. 0 means \
                                            no traversal; 6 is the usual maximum worth asking for."
                        },
                        "direction": {
                            "type": "string",
                            "enum": ["outbound", "inbound", "both"],
                            "default": "both",
                            "description": "Which way to walk. Outbound follows edges away from \
                                            the artifact ('what does this implement'); inbound \
                                            follows edges into it ('what implements this')."
                        },
                        "rels": {
                            "type": "array",
                            "items": { "type": "string", "enum": relation_enum() },
                            "description": "Restrict the traversal to these relations."
                        }
                    }
                }),
                false,
            ),
        },
        Tool {
            name: "specline_projects",
            title: "List and resolve projects",
            description:
                "List projects, or resolve a name to one. **Call this before creating a project, \
                 every time.** It fuzzy-matches on name, slug, aliases and repository URL, and \
                 when anything plausible matches it returns `requires_confirmation: true` with \
                 the candidates.\n\n\
                 When that happens, ask the human before creating anything. Nine near-duplicate \
                 projects is the failure mode that quietly ruins the cross-project view, and it \
                 is much cheaper to ask than to merge later."
                    .to_owned(),
            read_only: true,
            destructive: false,
            idempotent: true,
            input_schema: with_ambient(
                json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "A name, slug, alias or repo URL to match against. \
                                            Omit to list everything."
                        },
                        "include_archived": { "type": "boolean", "default": false }
                    }
                }),
                false,
            ),
        },
        Tool {
            name: "specline_activity",
            title: "What changed",
            description:
                "Every mutation since a timestamp or an event cursor, oldest first. Use it to \
                 catch up: 'what happened since I last looked', or to see what another session \
                 did while you were working.\n\n\
                 Pass the `cursor` from a previous response to continue exactly where you left \
                 off, with no gaps and no repeats.\n\n\
                 For one row's own story, read its notes with `specline_get` — a note says what was \
                 learned, where an event says only which field moved."
                    .to_owned(),
            read_only: true,
            destructive: false,
            idempotent: true,
            input_schema: with_ambient(
                json!({
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" },
                        "since": { "type": "string", "format": "date-time" },
                        "cursor": {
                            "type": "string",
                            "description": "An event id from a previous response. Takes precedence \
                                            over `since`."
                        },
                        "limit": { "type": "integer", "default": 50, "minimum": 1, "maximum": 500 }
                    }
                }),
                false,
            ),
        },
        Tool {
            name: "specline_create",
            title: "Create an artifact",
            description:
                "Create any of the thirteen artifact types. Returns the created artifact, so you \
                 never need to read it back.\n\n\
                 Creates are idempotent: calling twice with the same project, type and title \
                 returns the existing artifact with `created: false` rather than making a \
                 duplicate. Whitespace and capitalisation are normalised, so 'Add login page' \
                 and 'add  Login  Page' are one task.\n\n\
                 Before creating a **project**, call `specline_projects` first and confirm with the \
                 human (see that tool). Prefer consolidating into fewer, larger artifacts: a \
                 project with forty trivial tasks that should be eight is worse than useless."
                    .to_owned(),
            read_only: false,
            destructive: false,
            idempotent: true,
            input_schema: with_ambient(
                json!({
                    "type": "object",
                    "required": ["type"],
                    "properties": {
                        "type": {
                            "type": "string",
                            "enum": type_enum(),
                            "description": "Which artifact type to create."
                        },
                        "project": {
                            "type": "string",
                            "description": "Project id or slug. Required for everything except \
                                            `project` itself, and optional for `term` — omitting \
                                            it there defines the term globally."
                        },
                        "title": {
                            "type": "string",
                            "description": "The name. Called `name` on some types and `term` on \
                                            glossary entries; `title` is accepted for all of them."
                        },
                        // Declared, not merely mentioned in the prose above.
                        // These four were accepted and undeclared, so a model
                        // reading the schema — which is most of them — could
                        // not see that `slug` exists at all, and a project
                        // cannot be created without one.
                        "name": {
                            "type": "string",
                            "description": "An alias for `title`, for the types whose column is \
                                            called `name`."
                        },
                        "term": {
                            "type": "string",
                            "description": "An alias for `title`, for glossary entries."
                        },
                        "slug": {
                            "type": "string",
                            "description": "Projects only, and required for them: the URL-safe \
                                            short name, unique across the store."
                        },
                        // Required for milestones, and the description carries
                        // the register because a length cap can be enforced
                        // and a voice cannot. A model reads this at the moment
                        // of writing, which is the only moment that works —
                        // thirty gate sessions invoked the skill zero times.
                        "summary": {
                            "type": "string",
                            "description": "Tasks, milestones and feedback: REQUIRED. One or two \
                                            plain sentences \
                                            a colleague could read cold six weeks from now, \
                                            without having been in this conversation. For a \
                                            task: what is wrong or wanted, what it affects, and \
                                            what done looks like. For a milestone: what the \
                                            phase covers. For feedback this is the whole row — \
                                            what somebody said, in their words, because \
                                            feedback has no title to invent. It is what lists \
                                            show, so a row \
                                            without one means something only to whoever wrote \
                                            it.\n\n\
                                            Write it like a person, not like a release note. No \
                                            section references, no internal names, no \
                                            rule-of-three lists, no sentence that exists to \
                                            sound considered.\n\n\
                                            Good: \"The board shows a task's priority but never \
                                            which phase it belongs to, so you have to open each \
                                            one to find out. Done when every row shows its \
                                            milestone and you can group by it.\"\n\n\
                                            Bad: \"Implement milestone surfacing on the board \
                                            view per TQ-31 to improve organisational \
                                            legibility.\""
                        },
                        "definition": {
                            "type": "string",
                            "description": "Glossary entries only: what the word means in this \
                                            project. `body` is accepted for the same thing."
                        },
                        // The stated ceiling used to be 1 MB, which no session
                        // can reach: base64 inflates by a third and the *model*
                        // emits every character, so 1 MB is 350,000 to 450,000
                        // output tokens. A description promising ten times what
                        // is usable is a trap, so it now says the reachable
                        // number and points at the path that has no such cost.
                        "image": {
                            "type": "string",
                            "description": "Design and artifact only: a small image, base64 \
                                            encoded, or a `data:image/png;base64,…` URL.\n\n\
                                            **Keep this under about 100 KB.** You have to emit \
                                            every base64 character, so 100 KB costs you roughly \
                                            35,000–45,000 output tokens and 1 MB costs 350,000 \
                                            or more. The hard limit is 1 MB decoded and it is \
                                            not a target. If the file is on the same machine, \
                                            use `image_path` instead and none of this applies.\n\n\
                                            Put a Figma or web link in `body` when the image \
                                            already lives somewhere."
                        },
                        "image_path": {
                            "type": "string",
                            "description": "Design and artifact only: an absolute path to an \
                                            image on the machine Specline is running on. The daemon \
                                            reads the file itself, so the bytes never enter your \
                                            context and a real screenshot costs you nothing.\n\n\
                                            This is the right way to attach anything bigger than \
                                            a small mockup — up to 10 MB. Not a URL: the daemon \
                                            makes no outbound requests on a model's instruction.\n\n\
                                            Readable folders only: Desktop, Downloads, Pictures, \
                                            and the project's own directory. Anywhere else is \
                                            refused and the refusal lists them. That is not about \
                                            you — a path can be suggested by text you are reading \
                                            rather than by the person you are talking to."
                        },
                        "body": {
                            "type": "string",
                            "description": format!(
                                "For prose-bearing types (spec, decision, question, feedback, \
                                 design) this is written as the first document revision. For a \
                                 task it is the short detail field — anything long-form belongs \
                                 in a spec.\n\n{HOUSE_STYLE}"
                            )
                        },
                        "fields": {
                            "type": "object",
                            "description": "Any other column on the type: status, kind, priority, \
                                            labels, target_date, severity, sentiment, url, and so \
                                            on. Invalid values are rejected with the list of \
                                            valid ones.\n\n\
                                            A `metric_observation` is recorded here too: \
                                            `metric_id` (the `mtr_…` being measured), `value`, and \
                                            an optional `observed_at`.",
                            "additionalProperties": true
                        }
                    }
                }),
                true,
            ),
        },
        Tool {
            name: "specline_update",
            title: "Update an artifact",
            description:
                "Change fields on an existing artifact, including status transitions. Returns the \
                 updated artifact.\n\n\
                 **Not for prose.** A document body belongs to `specline_write_doc`, which versions \
                 it; this tool is for the fields around it — title, status, kind. Sending a body \
                 here would overwrite without a revision, and the previous author's text would \
                 be gone.\n\n\
                 Pass the `version` you read. If someone else changed it since, the call is \
                 rejected with the current state and the events that happened in between, so you \
                 can usually merge and retry without asking anyone.\n\n\
                 An accepted decision's content is immutable — supersede it with a new decision \
                 linked by `supersedes` rather than editing it. Use `archive: true` to soft-delete; \
                 nothing in Specline is ever really deleted.\n\n\
                 To reorder a task, put `rank_after` or `rank_before` in `changes` naming another \
                 task — `{\"rank_after\": \"KEEL-12\"}` — rather than choosing a number. To make one \
                 task part of another, set `parent_id`; that is composition, and it is a different \
                 thing from `blocks`, which means \"must happen first\".\n\n\
                 **Two task transitions belong to their own tools.** Starting work is \
                 `specline_claim`, which records who is on it; finishing is `specline_close`, which asks \
                 for the reason, the message and the evidence together. A `status` of `done` or \
                 `wont_do` sent here is refused without all three, so reaching for this tool to \
                 close something only costs you a round trip.\n\n\
                 To attach an image to a design or artifact that already exists, put \
                 `image_path` in `changes` with an absolute path — the daemon reads the file, so \
                 the bytes never enter your context."
                    .to_owned(),
            read_only: false,
            destructive: true,
            idempotent: true,
            input_schema: with_ambient(
                json!({
                    "type": "object",
                    "required": ["id", "version"],
                    "properties": {
                        "id": { "type": "string" },
                        "version": {
                            "type": "integer",
                            "description": "The `version` from when you read it. Not the document \
                                            revision — that is `current_doc_version`."
                        },
                        "changes": {
                            "type": "object",
                            "description": "Fields to set. Unknown fields are rejected with the \
                                            list of real ones.\n\n\
                                            Two are not fields but placements, for tasks: \
                                            `rank_after` and `rank_before` each name another task \
                                            and put this one next to it in the deliberate order. \
                                            They resolve to `rank`, which you should not set \
                                            directly.\n\n\
                                            One more is an instruction rather than a value: \
                                            `image_path`, on a design or artifact, is an absolute \
                                            path the daemon reads the image from. Do not set \
                                            `blob_id` yourself.",
                            "additionalProperties": true
                        },
                        "archive": {
                            "type": "boolean",
                            "default": false,
                            "description": "Soft-delete instead of updating."
                        }
                    }
                }),
                true,
            ),
        },
        Tool {
            name: "specline_write_doc",
            title: "Write a document revision",
            description:
                "Append a new revision of an artifact's prose body — for specs, decisions, \
                 questions, feedback and design captions.\n\n\
                 Use this whenever the *content* of a document changes. Use `specline_update` \
                 instead for the fields around it: title, status, kind. The two are separate \
                 because a body is versioned and a status is not, and conflating them would \
                 either version every status flip or lose the history of every edit.\n\n\
                 Always send the **full** body, not a patch — the revision is a snapshot. The \
                 previous one is kept and stays readable by version, and `specline_get` will diff \
                 any two. Writing content identical to the current revision is a no-op rather \
                 than a new version, so regenerating a document you have not changed is safe."
                    .to_owned(),
            read_only: false,
            destructive: false,
            idempotent: true,
            input_schema: with_ambient(
                json!({
                    "type": "object",
                    "required": ["id", "body"],
                    "properties": {
                        "id": { "type": "string" },
                        "title": {
                            "type": "string",
                            "description": "Update the title alongside the body. Omit to keep it."
                        },
                        "body": {
                            "type": "string",
                            "description": format!("The full markdown body.\n\n{HOUSE_STYLE}")
                        }
                    }
                }),
                true,
            ),
        },
        Tool {
            name: "specline_note",
            title: "Record what you learned about something",
            description:
                "Append a note to any artifact — a finding, a gotcha, a measurement, a reason \
                 something was harder than expected. Use it the moment you learn something \
                 worth the next session knowing, not at the end.\n\n\
                 This is the tool for the sentence that starts 'turns out…'. A task's status \
                 says a colour; its notes say what actually happened. Prefer a note over \
                 rewriting the artifact's body: notes accumulate and stay attributed to the \
                 conversation that wrote them, whereas a body is overwritten and the previous \
                 author's finding is gone.\n\n\
                 Do not ask permission first — record it, then say in one line that you did. \
                 Notes are append-only: to withdraw one, pass `retract` with its id.\n\n\
                 `body` is required unless you are passing `list` or `retract`."
                    .to_owned(),
            read_only: false,
            destructive: false,
            idempotent: false,
            input_schema: with_ambient(
                json!({
                    "type": "object",
                    // `id` and nothing else: `body` is required for the common
                    // case but not when listing or retracting, and a schema
                    // that demands it would make those two impossible to call.
                    "required": ["id"],
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "The artifact to annotate. Any Specline id."
                        },
                        "body": {
                            "type": "string",
                            "description": format!(
                                "The note. One or two sentences beats a heading — say what you \
                                 found and why it matters.\n\n{HOUSE_STYLE}"
                            )
                        },
                        "list": {
                            "type": "boolean",
                            "default": false,
                            "description": "Return the artifact's existing notes instead of \
                                            adding one."
                        },
                        "retract": {
                            "type": "string",
                            "description": "A note id (`nte_…`) to withdraw. Soft — it stays \
                                            readable as a record of what was once believed."
                        }
                    }
                }),
                // No idempotency key: appending is not idempotent and
                // advertising one the handler ignores is worse than none.
                false,
            ),
        },
        Tool {
            name: "specline_link",
            title: "Link two artifacts",
            description:
                "Create or remove a typed edge. Direction matters and reads left to right: \
                 `from` does the verb to `to`.\n\n\
                 A task **implements** a spec. A blocker **blocks** the thing waiting on it. A \
                 newer decision **supersedes** an older one. A decision **resolves** a question. \
                 Feedback **informs** a spec. If you find yourself wanting to say 'A depends on \
                 B', use `depends_on` and Specline will store it the right way round.\n\n\
                 Use `anchor` to link to one requirement inside a spec (`REQ-4`) rather than the \
                 whole document — that is what makes traceability answerable per requirement."
                    .to_owned(),
            read_only: false,
            destructive: true,
            idempotent: true,
            input_schema: with_ambient(
                json!({
                    "type": "object",
                    "required": ["from", "rel", "to"],
                    "properties": {
                        "from": { "type": "string", "description": "The artifact doing the verb." },
                        "rel": { "type": "string", "enum": relation_enum() },
                        "to": { "type": "string", "description": "The artifact it is done to." },
                        "anchor": {
                            "type": "string",
                            "description": "A block inside the target, e.g. `REQ-4`. Omit for a \
                                            whole-artifact link."
                        },
                        "note": { "type": "string", "description": "Why this link exists." },
                        "remove": {
                            "type": "boolean",
                            "default": false,
                            "description": "Archive the edge instead of creating it."
                        }
                    }
                }),
                true,
            ),
        },
        Tool {
            name: "specline_next",
            title: "What can be worked on right now",
            description:
                "The answer to 'what should I do next', ranked. Open work with nothing live in \
                 its way, best first.\n\n\
                 Call this rather than `specline_context` when the project is already familiar and \
                 the question is only what to pick up — the digest costs roughly 3,500 tokens \
                 and this costs a fraction of it.\n\n\
                 The order is deliberate: **what a task unblocks comes before its priority**, \
                 so a p1 that releases three other tasks ranks above a p0 that releases \
                 nothing. Each row carries the reason it is where it is.\n\n\
                 Parents are excluded, because their children are the actual work. Decisions \
                 waiting on a human are excluded too — they are in `specline_context`'s open \
                 questions, and nothing can start on them until someone answers.\n\n\
                 Then `specline_claim` the one you pick, so the row says who is on it."
                    .to_owned(),
            read_only: true,
            destructive: false,
            idempotent: true,
            input_schema: with_ambient(
                json!({
                    "type": "object",
                    "required": ["project"],
                    "properties": {
                        "project": { "type": "string", "description": "Project id, slug or name." },
                        "unclaimed": {
                            "type": "boolean",
                            "default": false,
                            "description": "Only work nobody is holding. Pass this when another \
                                            session may be running alongside you."
                        },
                        "labels": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Only tasks carrying **all** of these labels."
                        },
                        "without_labels": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Skip tasks carrying any of these labels."
                        },
                        "milestone": {
                            "type": "string",
                            "description": "Only work under this milestone — 'what is next in \
                                            Phase 8' in one argument. An id or its name."
                        },
                        "limit": {
                            "type": "integer",
                            "default": 10,
                            "minimum": 1,
                            "maximum": 100,
                            "description": "How many to return. The response says how many were \
                                            ready in total, so a cut list is never mistaken for \
                                            the whole one."
                        }
                    }
                }),
                false,
            ),
        },
        Tool {
            name: "specline_claim",
            title: "Take a task, so the row says who is on it",
            description:
                "Move a task to `in_progress` and record that this session is doing it. Call this \
                 **before** the work, not after — the point is that someone looking at the \
                 board can see what is happening now rather than only what has finished.\n\n\
                 Refused if another session is already holding it, and the refusal names that \
                 session. A claim releases itself after three days, so a conversation that died \
                 mid-task does not hold work hostage, and `force` takes a live claim \
                 deliberately. Closing releases it immediately.\n\n\
                 Re-claiming your own task is a no-op, so a retry costs nothing.\n\n\
                 This needs your `session_id`. It is the one call that is refused without one: \
                 everywhere else an anonymous write is merely less traceable, but a claim \
                 naming nobody says the task is taken and not by whom."
                    .to_owned(),
            read_only: false,
            destructive: false,
            idempotent: true,
            input_schema: with_ambient(
                json!({
                    "type": "object",
                    "required": ["id"],
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "The task — `KEEL-42` or a `tsk_…` id."
                        },
                        "force": {
                            "type": "boolean",
                            "default": false,
                            "description": "Take a claim another session still holds. Use it when \
                                            you know that session is gone."
                        }
                    }
                }),
                true,
            ),
        },
        Tool {
            name: "specline_close",
            title: "Finish a task or triage a signal, saying why and showing the work",
            description:
                "Close a task with a reason, a message, and — for `done` — evidence. This is the \
                 tool for finishing work; `specline_update` can move a status but will refuse a \
                 terminal one without all of this, so use this instead of working around it.\n\n\
                 The five reasons:\n\
                 - `done` — finished. Needs a message and at least one piece of evidence.\n\
                 - `wont_do` — deliberately not doing it. Needs a message.\n\
                 - `duplicate` — the same work as another task. Names it, and draws a \
                 `duplicates` edge.\n\
                 - `superseded` — replaced by another task. Names it, and draws a `supersedes` \
                 edge.\n\
                 - `no_change` — looked at it, nothing needed doing. Needs a message.\n\n\
                 Evidence is typed so that 'what shipped this week, with the commits' is a \
                 query rather than prose: `commit:<sha>`, `pr:<url>`, `test:<command>`, \
                 `doc:<entity-id>`, `url:<url>`, `image:<blob-id>`. A bare sha is refused.\n\n\
                 The message is the one sentence that belongs to the transition. Anything else \
                 you learned along the way belongs in `specline_note`, which keeps accumulating.\n\n\
                 **This also triages a signal.** Pass a `fbk_…` and the same three reasons mean \
                 what happens to a want rather than to work:\n\
                 - `done` — picked up, or otherwise answered. Name what became of it as \
                 evidence; a `doc:spc_…` feature spec among it is linked to the signal. Not \
                 every want becomes a feature — some become a commit.\n\
                 - `wont_do` — set down. The message is the argument, and it is appended to the \
                 signal so the same idea arriving in four months finds the reasoning instead of \
                 silence. Nothing is destroyed and it can be picked up again.\n\
                 - `duplicate` — somebody had already asked. Names the other signal.\n\n\
                 `superseded` and `no_change` are refused for a signal: neither describes what \
                 happens to a want. A signal cannot leave the Inbox without one of the three."
                    .to_owned(),
            read_only: false,
            destructive: true,
            idempotent: true,
            input_schema: with_ambient(
                json!({
                    "type": "object",
                    "required": ["id", "reason", "message"],
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "The task — `KEEL-42` or a `tsk_…` id."
                        },
                        "reason": {
                            "type": "string",
                            "enum": enum_of(CloseReason::ALL.iter().map(|r| r.as_str())),
                            "description": "Which of the five."
                        },
                        "message": {
                            "type": "string",
                            "description": format!(
                                "What actually happened, in a sentence or two. What was built, \
                                 or why it is not being done — it is what the next session \
                                 reads instead of guessing from a status.\n\n{HOUSE_STYLE}"
                            )
                        },
                        "evidence": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Typed proof, at least one for `done`: \
                                            `commit:<sha>`, `pr:<url>`, `test:<command>`, \
                                            `doc:<entity-id>`, `url:<url>`, `image:<blob-id>`."
                        },
                        "other": {
                            "type": "string",
                            "description": "For `duplicate` and `superseded` only: the task this \
                                            one duplicates, or the one that replaced it."
                        }
                    }
                }),
                true,
            ),
        },
    ]
}

/// Find a tool by name.
pub fn find(name: &str) -> Option<Tool> {
    all().into_iter().find(|t| t.name == name)
}

/// The `tools/list` result, with the cache hints this revision requires.
pub fn list_result() -> Value {
    json!({
        "tools": all().iter().map(Tool::to_json).collect::<Vec<_>>(),
        // Specline's tool list is static — it changes when the binary changes and
        // never at runtime — so a long TTL is honest and stops clients
        // polling. `public` because there is nothing caller-specific in it.
        "ttlMs": 86_400_000u64,
        "cacheScope": "public",
    })
}

/// The `server/discover` result.
///
/// Required in this revision: a client may call it before anything else to
/// pick a protocol version, and on stdio it doubles as the backward-
/// compatibility probe.
pub fn discover_result() -> Value {
    json!({
        // Both, because both are accepted. Advertising only the current one
        // while the handshake happily answers 2025-11-25 is a server telling a
        // client less than it does — and `server/discover` exists precisely so
        // a client can pick a version without guessing. Claude Code opens with
        // the legacy handshake today (TQ-11), so the one that was omitted is
        // the one in daily use.
        "protocolVersions": crate::protocol::SUPPORTED_VERSIONS,
        "serverInfo": crate::protocol::server_info(),
        "capabilities": {
            "tools": { "listChanged": false },
            // Resources, prompts, sampling, roots and logging are all
            // deliberately absent. Specline's surface is thirteen tools; advertising
            // capabilities it does not implement would invite calls it would
            // then have to refuse.
        },
        "instructions": crate::protocol::INSTRUCTIONS,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn there_are_exactly_thirteen_tools() {
        // Nine was the ceiling from SPEC §6.1, ten after `specline_note`, thirteen
        // after the three work verbs (TQ-31). Each rise needed KB's agreement
        // rather than a passing test suite, and the reasoning for the last one
        // is in the doc comment on `all()`. A fourteenth needs an argument at
        // least as good.
        assert_eq!(
            all().len(),
            13,
            "thirteen tools is the ceiling — more tools means worse model selection"
        );
    }

    #[test]
    fn tool_names_match_the_spec() {
        let names: Vec<&str> = all().iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            vec![
                "specline_context",
                "specline_search",
                "specline_get",
                "specline_projects",
                "specline_activity",
                "specline_create",
                "specline_update",
                "specline_write_doc",
                "specline_note",
                "specline_link",
                "specline_next",
                "specline_claim",
                "specline_close",
            ]
        );
    }

    #[test]
    fn the_order_is_deterministic() {
        // Required for client-side caching and prompt-cache hits.
        let first: Vec<&str> = all().iter().map(|t| t.name).collect();
        let second: Vec<&str> = all().iter().map(|t| t.name).collect();
        assert_eq!(first, second);
    }

    #[test]
    fn every_tool_accepts_the_ambient_arguments() {
        for tool in all() {
            let props = tool.input_schema["properties"].as_object().unwrap();
            assert!(
                props.contains_key("session_id"),
                "{} must accept session_id — it is the provenance unit",
                tool.name
            );
            assert!(props.contains_key("surface"), "{}", tool.name);
            // The rule is keyed on what the tool *claims*, not on whether it
            // writes. A tool that advertises `idempotentHint` and then has no
            // way to be told which call is the retry is lying to its client;
            // one that accepts a key it ignores is lying the other way.
            if !tool.read_only && tool.idempotent {
                assert!(
                    props.contains_key("idempotency_key"),
                    "{} advertises idempotency and must accept a key to honour it",
                    tool.name
                );
            }
            if !tool.idempotent {
                assert!(
                    !props.contains_key("idempotency_key"),
                    "{} is not idempotent and must not advertise a key it ignores",
                    tool.name
                );
            }
        }
    }

    #[test]
    fn read_tools_are_marked_read_only() {
        let reads = [
            "specline_context",
            "specline_search",
            "specline_get",
            "specline_projects",
            "specline_activity",
            "specline_next",
        ];
        for tool in all() {
            assert_eq!(
                tool.read_only,
                reads.contains(&tool.name),
                "{} has the wrong read_only flag",
                tool.name
            );
        }
    }

    #[test]
    fn every_description_explains_when_to_use_it() {
        // A description that reads like a function signature produces an agent
        // that calls the wrong tool confidently.
        for tool in all() {
            assert!(
                tool.description.len() > 200,
                "{} has a thin description ({} chars)",
                tool.name,
                tool.description.len()
            );
            let d = tool.description.to_lowercase();
            assert!(
                d.contains("use it")
                    || d.contains("use this")
                    || d.contains("start here")
                    || d.contains("call this")
                    || d.contains("use `")
                    || d.contains("prefer"),
                "{} does not say when to reach for it",
                tool.name
            );
        }
    }

    #[test]
    fn the_link_tool_teaches_direction_by_example() {
        // Direction is the most dangerous thing to get wrong, and the tool
        // description is the only documentation an agent gets.
        let link = find("specline_link").unwrap();
        assert!(
            link.description.contains("implements"),
            "{}",
            link.description
        );
        assert!(link.description.contains("supersedes"));
        assert!(link.description.contains("depends_on"));
        assert!(link.description.contains("anchor") || link.description.contains("REQ-4"));
    }

    #[test]
    fn the_context_tool_says_it_is_the_entry_point() {
        let ctx = find("specline_context").unwrap();
        assert!(ctx.description.starts_with("START HERE"));
    }

    #[test]
    fn the_projects_tool_carries_the_disambiguation_instruction() {
        // REQ-8: safety lives in the skill and in this description, not in the
        // API, because creating a project is a legitimate thing to do.
        let p = find("specline_projects").unwrap();
        assert!(p.description.contains("before creating a project"));
        assert!(p.description.contains("ask the human"));
    }

    #[test]
    fn tools_list_carries_the_required_cache_hints() {
        let list = list_result();
        assert!(list["ttlMs"].as_u64().unwrap() > 0);
        assert_eq!(list["cacheScope"], "public");
        assert_eq!(list["tools"].as_array().unwrap().len(), all().len());
    }

    #[test]
    fn discover_advertises_only_what_is_implemented() {
        let d = discover_result();
        assert_eq!(d["protocolVersions"][0], crate::protocol::PROTOCOL_VERSION);
        let caps = d["capabilities"].as_object().unwrap();
        assert!(caps.contains_key("tools"));
        for absent in ["resources", "prompts", "sampling", "roots", "logging"] {
            assert!(
                !caps.contains_key(absent),
                "advertising `{absent}` invites calls Specline would have to refuse"
            );
        }
    }

    #[test]
    fn unknown_tools_are_not_found() {
        assert!(find("specline_delete").is_none());
        assert!(find("specline_context").is_some());
    }

    #[test]
    fn schemas_are_valid_json_objects_with_a_type() {
        for tool in all() {
            assert_eq!(tool.input_schema["type"], "object", "{}", tool.name);
            assert!(tool.input_schema["properties"].is_object(), "{}", tool.name);
        }
    }
}
