/**
 * Typed client for the daemon's local API.
 *
 * Every path is relative. In development Vite proxies `/api` to the daemon; in
 * the Tauri build the daemon runs as a sidecar on the same origin. That is
 * deliberate — SPEC §10 says the web build should be the same bundle with a
 * different base URL, and hard-coding `http://127.0.0.1:7654` here would make
 * that a rewrite rather than a config change.
 */

const BASE = import.meta.env.VITE_SPECLINE_BASE ?? "";

export class ApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

async function get<T>(
  path: string,
  params?: Record<string, string | number | undefined>,
): Promise<T> {
  const url = new URL(`${BASE}${path}`, window.location.origin);
  for (const [key, value] of Object.entries(params ?? {})) {
    if (value !== undefined && value !== "")
      url.searchParams.set(key, String(value));
  }

  let response: Response;
  try {
    response = await fetch(url.toString());
  } catch {
    // The daemon being down is the single most likely failure, and "Failed to
    // fetch" tells a human nothing about what to do. Say what is wrong.
    throw new ApiError(
      "Cannot reach the Specline daemon. Start it with `specline-daemon` and try again.",
      0,
    );
  }

  const body = await response.json().catch(() => null);
  if (!response.ok) {
    throw new ApiError(
      body?.error?.message ?? `Request failed (${response.status})`,
      response.status,
    );
  }
  return (body?.data ?? body) as T;
}

/** The token in the document the daemon served, if this page came from one. */
function currentToken(): string | null {
  return (
    document
      .querySelector('meta[name="specline-token"]')
      ?.getAttribute("content") ?? null
  );
}

function send(
  method: "POST" | "PATCH",
  url: string,
  body: unknown,
  token: string,
): Promise<Response> {
  return fetch(url, {
    method,
    headers: { "content-type": "application/json", "x-specline-token": token },
    body: JSON.stringify(body),
  });
}

/**
 * Read the token out of the page this origin serves *now*.
 *
 * Same-origin, so only a page already on this origin can do it — which is the
 * same boundary that made putting the token in the document safe in the first
 * place. It also updates the meta tag in place, so the next write does not pay
 * for this again.
 */
async function refetchToken(): Promise<string | null> {
  try {
    const html = await (await fetch(`${BASE}/`, { cache: "no-store" })).text();
    const found = html.match(/<meta name="specline-token" content="([^"]+)"/)?.[1];
    if (!found) return null;
    document
      .querySelector('meta[name="specline-token"]')
      ?.setAttribute("content", found);
    return found;
  } catch {
    return null;
  }
}

/**
 * Every write this client makes.
 *
 * Hard constraint 7 used to say the interface had none; B-78 rewrote it to draw
 * the line where the argument actually is. The interface writes what a person
 * **does** — create a task, comment on one, archive a row, close one, and ask
 * the daemon to restart into an update it already staged. It does not write
 * what a person **reasons**: there is no call here that sends a document body,
 * and if one appears, that is the constraint moving again and it needs its own
 * decision rather than a reuse of this helper.
 */
async function write<T>(
  method: "POST" | "PATCH",
  path: string,
  body: unknown,
): Promise<T> {
  const url = new URL(`${BASE}${path}`, window.location.origin);

  const token = currentToken();
  if (!token) {
    throw new ApiError(
      "This page was not served by the Specline daemon, so it cannot change anything. " +
        "Open the interface at the daemon's own address.",
      0,
    );
  }

  let response: Response;
  try {
    response = await send(method, url.toString(), body, token);
  } catch {
    throw new ApiError(
      "Cannot reach the Specline daemon. Start it with `specline-daemon` and try again.",
      0,
    );
  }

  // A token has the lifetime of one daemon, and `specline update` restarts the
  // daemon — so a page left open across an update is holding a secret that has
  // expired, and every button on it would fail with a 401 that reads like a
  // broken app.
  //
  // Rather than making the reader reload, fetch the document this origin serves
  // now and take the token out of it. That is exactly as safe as the original
  // delivery: only a page on this origin can read that response, which is the
  // property the whole scheme rests on. One retry, because a second failure is
  // a real refusal rather than a stale secret.
  if (response.status === 401) {
    const fresh = await refetchToken();
    if (fresh && fresh !== token) {
      try {
        response = await send(method, url.toString(), body, fresh);
      } catch {
        throw new ApiError(
          "Cannot reach the Specline daemon. Start it with `specline-daemon` and try again.",
          0,
        );
      }
    }
  }

  const parsed = await response.json().catch(() => null);
  if (!response.ok) {
    throw new ApiError(
      parsed?.error?.message ?? `Request failed (${response.status})`,
      response.status,
    );
  }
  return (parsed?.data ?? parsed) as T;
}

const post = <T>(path: string, body: unknown) => write<T>("POST", path, body);

/**
 * A change to fields that already exist, as opposed to a thing that happens.
 *
 * The distinction is the reason this is a second verb rather than another
 * `post`: everything posted here is an *action* — create this, close that,
 * apply the update you staged — and each has an endpoint named for it. Moving
 * a priority is not an action with a name, it is the field being different,
 * and it carries the version read so that two people editing one row is a
 * conflict rather than whoever clicked last.
 */
const patch = <T>(path: string, body: unknown) => write<T>("PATCH", path, body);

// --- Shapes the daemon returns ------------------------------------------

export interface Audit {
  created_at: string;
  updated_at: string;
  version: number;
  created_by: Actor;
  updated_by: Actor;
  session_id: string | null;
  surface: string | null;
  archived_at: string | null;
}

export type Actor = "human" | "claude" | "github" | "system";

export interface Entity {
  type: string;
  id: string;
  audit: Audit;
  [key: string]: unknown;
}

/**
 * A signal: something somebody wants, before anybody has decided anything.
 *
 * Stored as feedback, and it has a `summary` rather than a `title` on purpose
 * — what somebody said has no name, and inventing one is a small lie about the
 * record. `triaged` false is the whole definition of being in the Inbox.
 */
export interface Signal extends Entity {
  summary: string;
  kind: string;
  source: string | null;
  contact: string | null;
  occurred_at: string | null;
  triaged: boolean;
}

export interface ProjectLine {
  id: string;
  name: string;
  slug: string;
  /** The prefix of this project's readable identifiers — the `KEEL` of `KEEL-42`. */
  key: string;
  status: string;
  open_tasks: number;
  urgent_tasks: number;
  blocked_tasks: number;
  open_questions: number;
  /** Untriaged signals. Never folded into `open_tasks` — see B-90. */
  inbox: number;
  /** How long the oldest untriaged signal has waited, in days. */
  inbox_oldest_days: number | null;
  active_milestone: string | null;
}

export interface DigestItem {
  id: string;
  entity_type: string;
  label: string;
  /** `KEEL-42`, for the types that have one. Tasks only, today. */
  reference?: string;
  status: string | null;
  detail?: string;
}

export interface TermEntry {
  term: string;
  definition: string;
  global: boolean;
}

export interface Truncation {
  section: string;
  shown: number;
  total: number;
}

export interface Digest {
  project: ProjectLine | null;
  projects: ProjectLine[];
  active: DigestItem[];
  attention: DigestItem[];
  recent: string[];
  decisions: DigestItem[];
  questions: DigestItem[];
  specs: DigestItem[];
  terms: TermEntry[];
  environments: DigestItem[];
  next: string[];
  next_up: NextUp | null;
  truncated: Truncation[];
  budget_exceeded: boolean;
  estimated_tokens: number;
}

/** The ranked answer to "what do I do next". Same ranking the digest gives an agent. */
export interface NextUp {
  ready: NextItem[];
  waiting_on_you: NextItem[];
  blocked: NextItem[];
}

export interface NextItem {
  id: string;
  /** `KEEL-42` — what a person will type back at Claude. */
  reference: string;
  title: string;
  priority: string;
  unblocks: number;
  /** Which bucket it belongs in: an open phase, a bug, or everything else. */
  group: "active" | "bug" | "rest";
  why: string;
}

export interface SearchHit {
  entity_id: string;
  entity_type: string;
  project_id: string | null;
  title: string;
  excerpt: string;
  score: number;
  source: "keyword" | "semantic" | "both";
}

export interface EventRow {
  id: string;
  project_id: string | null;
  entity_type: string;
  entity_id: string;
  action: string;
  field: string | null;
  before: unknown;
  after: unknown;
  actor: Actor;
  session_id: string | null;
  surface: string | null;
  summary: string;
  created_at: string;
}

export interface Revision {
  version: number;
  title: string;
  author: Actor;
  session_id: string | null;
  surface: string | null;
  created_at: string;
  status: string;
}

export interface DocumentBody {
  version: number;
  title: string;
  body: string;
  created_at: string;
  author: Actor;
}

export interface Diff {
  from_version: number;
  to_version: number;
  unified: string;
  added: number;
  removed: number;
}

export interface Neighbour {
  id: string;
  entity_type: string;
  rel: string;
  /** What it is called. Carried by the traversal so a caller need not re-fetch. */
  label: string;
  anchor: string;
  depth: number;
  path: string[];
}

/** Every list the daemon returns says whether it was cut, and by how much. */
/** One entry in a row's running commentary. */
export interface Note {
  id: string;
  project_id: string | null;
  entity_type: string;
  entity_id: string;
  body: string;
  author: Actor;
  session_id: string | null;
  surface: string | null;
  created_at: string;
  archived_at: string | null;
}

/**
 * Which editor drove a conversation.
 *
 * `surface` on a row says what *kind* of place wrote it and there are five;
 * this says which product, which is the difference between "code" and "Codex
 * 0.148". Resolved through `session_id`, which every task, note and revision
 * already carries.
 *
 * **`last_wrote`, and never "connected".** MCP over HTTP is stateless, so there
 * is nothing connected to report — a dot rendered from this would be wrong for
 * an editor quit an hour ago and wrong again for one sitting idle mid-thought.
 * A session absent from this list is unknown, not offline.
 */
export interface SessionClient {
  session_id: string;
  name: string;
  title: string | null;
  version: string | null;
  display_name: string;
  first_seen: string;
  last_wrote: string;
}

export interface Page<T> {
  items: T[];
  total: number;
  truncated: boolean;
}

// --- Calls ---------------------------------------------------------------

export const api = {
  /**
   * Liveness. Never blocks, which is why `projects` can be stale:
   * `store_busy` means the daemon was mid-write and answered from the last
   * count it saw rather than waiting for the lock.
   */
  health: () =>
    get<{
      status: string;
      protocol: string;
      version: string;
      projects: number;
      store_busy: boolean;
      /**
       * Where the store is, and what shape it is in. Both are returned by the
       * daemon and were simply missing from this type.
       *
       * `home` earns its place on the first run: an empty read-only screen
       * cannot otherwise distinguish a working install from a broken one, and
       * a path the daemon reports is a fact no broken install could produce.
       */
      home?: string;
      schema?: number;
      /**
       * The version downloaded, verified and waiting, or null.
       *
       * The daemon checks on its own schedule and stages what is safe, but
       * applies nothing — a restart is agreed to, not arranged (KEEL-225). This
       * field is how the interface knows there is something to offer, and it
       * costs no extra request because health is already fetched.
       */
      staged_version?: string | null;
      /**
       * Where to read what the running version contains — the release page for
       * its tag, which the release job builds with `--generate-notes`.
       *
       * The daemon mints it rather than the interface composing one, because
       * the repository is configurable (`SPECLINE_REPO`) and a template here would
       * be right only for the default.
       */
      release_notes?: string;
      /** The same for the staged version, or null when nothing is staged. */
      staged_release_notes?: string | null;
      /**
       * Whether update checking is happening, and when it last did.
       *
       * `staged_version: null` says only that nothing is waiting, and that
       * reads as "you are current" whether the daemon checked an hour ago, has
       * been failing since March, or has checks switched off. This separates
       * the three.
       *
       * **Absent, not null, on a daemon older than the updater** — which is
       * every install before 0.1.2, the population that most needs telling it
       * is behind. The interface reads the absence as the answer, so no
       * outbound request and no known-latest comparison is involved.
       */
      update_check?: {
        enabled?: boolean;
        last_checked_at?: string | null;
        last_error?: string | null;
      };
      /** The path of the running binary, for a machine with more than one. */
      executable?: string | null;
      /**
       * Which unfinished surfaces this daemon has switched on.
       *
       * Absent on a daemon older than the flag, which is every 0.4.0 install —
       * and those *do* serve the Inbox, so an absent field reads as on. That
       * is the honest default: hiding a screen whose endpoints still answer
       * would be the interface lying about what the daemon can do.
       */
      surfaces?: { inbox?: boolean };
    }>("/api/health"),

  /**
   * Apply the staged update and restart the daemon into it.
   *
   * The one write the interface is allowed (B-75, amending hard constraint 7).
   * It sends no body: the daemon applies what it already staged, so there is no
   * version to choose and nothing for a caller to point elsewhere.
   */
  applyUpdate: () =>
    post<{ applied: string; restarting: boolean }>("/api/update/apply", {}),

  /**
   * Ask the daemon to look for a new release now.
   *
   * The same call it makes hourly, on a person's say-so instead of a timer —
   * because there was no way to ask, and "no update showing" and "it has not
   * looked since the release existed" were the same picture (KEEL-258).
   *
   * Strictly less than `applyUpdate`: this can download and stage, and cannot
   * promote anything into place or restart anything.
   *
   * `outcome` is named rather than described so each case can be rendered
   * without reading prose. `ahead` is not exotic — anybody on an `-rc` is ahead
   * of what `releases/latest` resolves to.
   */
  checkForUpdate: () =>
    post<{
      outcome:
        | "up_to_date"
        | "staged"
        | "already_staged"
        | "needs_a_person"
        | "ahead"
        | "failed";
      version?: string;
      published?: string;
      release_notes?: string;
      schema_from?: number;
      schema_to?: number;
      error?: string;
    }>("/api/update/check", {}),

  /** Create a task. What a person types when they think of something. */
  createTask: (task: {
    project: string;
    title: string;
    summary?: string;
    priority?: string;
    kind?: string;
    /** A milestone id. Absent means the task belongs to no phase. */
    milestone?: string;
    labels?: string[];
  }) => post<Entity>("/api/tasks", task),

  /**
   * The Inbox — untriaged signals, oldest first.
   *
   * Oldest first is the opposite of every other list here and is deliberate: a
   * newest-first Inbox buries the thing that has been ignored longest under
   * whatever was filed this morning, which is the failure the screen exists to
   * prevent.
   */
  inbox: (params: { project: string; limit?: number }) =>
    get<Page<Signal>>("/api/inbox", { ...params, limit: params.limit ?? 200 }),

  /**
   * File a signal. What a person types when somebody wants something.
   *
   * `summary` is the only required field beyond the project, and that is the
   * requirement rather than an omission — an Inbox that costs more to file
   * into than the thought cost to have is one nobody uses.
   *
   * There is no `body` and the daemon refuses one. A signal's verbatim is a
   * document revision, and hard constraint 7's own test is that an endpoint
   * accepting one is on the wrong side of the line; a longer verbatim is
   * written from the session the conversation happened in.
   */
  createSignal: (signal: {
    project: string;
    summary: string;
    kind?: string;
    source?: string;
    contact?: string;
  }) => post<Signal>("/api/signals", signal),

  /**
   * Change the fields on a task that a person moves while looking at it.
   *
   * `version` is the one you read. Send it, and a concurrent edit comes back a
   * 409 naming what is actually there; leave it out and the daemon refuses,
   * because the alternative is quietly overwriting somebody.
   *
   * Three statuses are not settable here and the daemon says why: `done` and
   * `wont_do` go through `closeTask`, which collects the reason, the message
   * and the evidence the storage layer wants, and `in_progress` is a claim,
   * which records which session is on the work.
   */
  updateTask: (
    id: string,
    changes: {
      version: number;
      status?: "todo" | "review";
      priority?: string;
      kind?: string;
      /** A milestone id, or `""` to clear the phase. */
      milestone?: string;
      labels?: string[];
    },
  ) => patch<Entity>(`/api/tasks/${encodeURIComponent(id)}`, changes),

  /** Add a note to a row — a comment, in the words a person would use. */
  addNote: (id: string, body: string) =>
    post<Note>(`/api/entity/${encodeURIComponent(id)}/notes`, { body }),

  /**
   * Archive a row.
   *
   * Named for what it does, not for what the button says. Hard constraint 3 is
   * soft delete only: the row stays readable and stays in the history, so a
   * method called `deleteEntity` would be a promise this cannot keep.
   */
  archive: (id: string) =>
    post<Entity>(`/api/entity/${encodeURIComponent(id)}/archive`, {}),

  /**
   * Close a task, with the reason it needs.
   *
   * The reason, the message and — for `done` — the evidence are required by the
   * storage layer, not by this form. A close without them is refused wherever it
   * comes from, which is why the form asks for them rather than sending a
   * status change and hoping.
   */
  closeTask: (
    id: string,
    close: { reason: string; message: string; evidence?: string[] },
  ) => post<Entity>(`/api/tasks/${encodeURIComponent(id)}/close`, close),

  /** The digest. No `project` gives the cross-project roll-up. */
  context: (project?: string) =>
    get<Digest>("/api/context", { project, depth: "full" }),

  projects: () => get<{ projects: Entity[] }>("/api/projects"),

  /**
   * What can be worked on right now, ranked.
   *
   * The same `specline_next` a session calls, not a second ranking computed here.
   * That is the point of the endpoint existing at all — an app that ordered the
   * work differently from the tool would make "what next" a question with two
   * answers.
   *
   * `blocked: "true"` asks for the ids of what is stuck as well. The board wants
   * both and used to get them from the digest, which meant fetching a whole
   * project briefing — every open question, every recent decision, the glossary
   * — to draw one column and rank three cards.
   */
  ready: (params: {
    project: string;
    unclaimed?: string;
    milestone?: string;
    blocked?: string;
    limit?: number;
  }) =>
    get<{
      ready: NextItem[];
      total: number;
      truncated: boolean;
      blocked?: string[];
    }>("/api/ready", params),

  /**
   * A project's notes, in one call.
   *
   * Fetched for the whole project rather than per card: a board showing
   * seventy tasks would otherwise open seventy requests to render a count.
   */
  notes: (project?: string) =>
    get<{ notes: Note[]; total: number }>("/api/notes", { project }),

  /**
   * Which editors have written, most recently first.
   *
   * Fetched whole rather than per row, for the same reason `notes` is: a screen
   * showing thirty notes from six conversations wants one request and a lookup,
   * not thirty. The table holds one row per conversation, so it stays small
   * enough for that to be the cheaper shape.
   */
  clients: () =>
    get<{ clients: SessionClient[]; total: number }>("/api/clients"),

  /**
   * How many notes each row in a project carries, and nothing else.
   *
   * The board puts a number on every card and never shows a body, so it was
   * pulling a hundred and fifty kilobytes of prose across the wire to run
   * `length` on it. This is the same read against the store with the bodies left
   * behind — which is the part the browser was waiting for.
   */
  noteCounts: (project?: string) =>
    get<{ counts: Record<string, number>; total: number }>("/api/notes", {
      project,
      counts: "true",
    }),

  /**
   * One row's notes, retracted ones included.
   *
   * A detail view shows a retracted note struck through rather than hiding it:
   * what a session once believed is part of how the row got here, and silently
   * dropping it rewrites the record.
   */
  notesFor: (entity: string) =>
    get<{ notes: Note[]; total: number }>("/api/notes", {
      entity,
      all: "true",
    }),

  /**
   * One row's history — every status and field change, with before and after.
   *
   * The event log has always held this and nothing has ever shown it.
   *
   * Its own endpoint rather than `/api/activity?entity=`, because that route is
   * the `specline_activity` tool and the tool no longer takes an entity (TQ-24).
   * B-15 is the rule this follows: the local API has more endpoints than the
   * tool surface has tools, since a UI knows what it wants and a model chooses
   * worse among more options.
   */
  history: (entity: string, limit = 500) =>
    get<{ events: EventRow[]; total: number; truncated: boolean }>(
      `/api/entity/${encodeURIComponent(entity)}/history`,
      { limit },
    ),

  entities: (params: {
    project?: string;
    type?: string;
    status?: string;
    limit?: number;
  }) =>
    get<Page<Entity>>("/api/entities", {
      ...params,
      limit: params.limit ?? 500,
    }),

  entity: (id: string, depth = 0) =>
    get<{
      artifacts: Array<{
        entity: Entity;
        document?: DocumentBody;
        neighbours?: Neighbour[];
      }>;
    }>(`/api/entity/${id}`, { depth }),

  document: (id: string, version?: number, diffAgainst?: number) =>
    get<{
      revisions: Revision[];
      document: DocumentBody | null;
      diff: Diff | null;
    }>(`/api/document/${id}`, { version, diff_against: diffAgainst }),

  graph: (
    id: string,
    direction: "outbound" | "inbound" | "both" = "both",
    depth = 2,
  ) =>
    get<{ neighbours: Neighbour[] }>(`/api/graph/${id}`, { direction, depth }),

  search: (
    query: string,
    params?: { project?: string; types?: string; limit?: number },
  ) =>
    get<Page<SearchHit> & { hits: SearchHit[] }>("/api/search", {
      query,
      ...params,
    }),

  activity: (params?: { project?: string; limit?: number; cursor?: string }) =>
    get<{
      events: EventRow[];
      total: number;
      truncated: boolean;
      cursor: string | null;
    }>("/api/activity", params),

  /**
   * What changed, grouped by the session that changed it.
   *
   * Its own endpoint rather than a shape on `/api/activity`, because that URL is
   * the `specline_activity` tool and this is a different question: the tool pages
   * every mutation from a cursor for a model catching up, and this answers "what
   * did each session do" for a person who left Claude working.
   *
   * The union with notes is the part that cannot be done here: a note leaves no
   * row in `events` (TQ-29), so a per-session count built from the feed alone
   * silently misses the part most worth reading.
   */
  changed: (params: {
    project?: string;
    actor?: string;
    since?: string;
    limit?: number;
  }) =>
    get<{
      sessions: Array<{
        session_id: string | null;
        actor: string;
        started_at: string;
        ended_at: string;
        headline: string;
        /** Short project keys this session touched, e.g. ["KEEL"]. */
        projects: string[];
        changes: Array<{
          id: string;
          kind: "field" | "created" | "note";
          entity_id: string;
          entity_type: string;
          reference: string;
          summary: string;
          at: string;
        }>;
      }>;
      changes: number;
      truncated: boolean;
    }>("/api/changes", params),
};

/** Whether the live feed is currently attached. */
export type FeedStatus = "connecting" | "live" | "down";

/**
 * Subscribe to change notifications.
 *
 * The daemon emits a `lagged` event when a subscriber has fallen behind and
 * lost messages. That is surfaced rather than swallowed: a UI that missed
 * changes should refetch, and quietly continuing would leave it showing stale
 * state indefinitely.
 *
 * **`open` refetches too, and that is the whole point of it.** `EventSource`
 * reconnects on its own, but a reconnect announces nothing about what happened
 * while it was away — so before this, a daemon restart left the app showing
 * whatever it had before the drop, indefinitely, until some unrelated write
 * arrived and everything appeared at once. That is exactly the shape this
 * project keeps trying to eliminate: a screen that is wrong and looks settled.
 * It cost a real half hour, hunting a task that existed everywhere except on
 * the board.
 *
 * The first `open` duplicates the initial load, which costs one wasted read on
 * startup. That is the correct direction to be wrong in.
 *
 * `onStatus` exists so the shell can say when the feed is down. Without it,
 * "nothing has changed" and "nothing can reach me" render identically.
 */
export function subscribe(
  onChange: (change: ChangeEvent) => void,
  onStatus: (status: FeedStatus) => void = () => {},
): () => void {
  const source = new EventSource(`${BASE}/api/events`);
  onStatus("connecting");

  const forward = (raw: MessageEvent | Event) => {
    const data =
      "data" in raw && typeof raw.data === "string" ? raw.data : null;
    let change: ChangeEvent = { kind: "entity", summary: "" };
    if (data) {
      try {
        change = { ...change, ...(JSON.parse(data) as ChangeEvent) };
      } catch {
        // A change we cannot parse is still a change. Refetching on it is the
        // safe direction: the cost is one wasted read, and the alternative is
        // showing stale state because a payload shape moved.
      }
    }
    onChange(change);
  };

  source.addEventListener("change", forward);
  source.addEventListener("lagged", forward);
  source.addEventListener("open", () => {
    onStatus("live");
    // Catch up on whatever was written while this was not listening. On the
    // first connect there is nothing to catch up on and this is redundant.
    forward(new Event("open"));
  });
  // `EventSource` reports a dropped connection as an error and then retries by
  // itself, so this is a status change rather than a failure to handle.
  source.addEventListener("error", () => onStatus("down"));

  return () => source.close();
}

/** One announced write. */
export interface ChangeEvent {
  /**
   * `entity` for anything that wrote an event; `note` for a note; `update`
   * for a release staged and waiting for a restart.
   *
   * Notes are announced separately because they are not events, and the daemon
   * cannot see them by watching the event log (TQ-29). `update` is the same
   * shape of problem one step further out: nothing is written to the store at
   * all, so before it existed a staged release reached the footer only when
   * some unrelated write happened to refresh health (KEEL-317).
   */
  kind: "entity" | "note" | "update";
  /** The row it is about, when known. */
  entity_id?: string;
  /** One line describing it. */
  summary: string;
}
