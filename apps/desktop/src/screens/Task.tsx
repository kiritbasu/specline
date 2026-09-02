/**
 * The task, on a page of its own.
 *
 * The hole the whole phase was named after. A card on the board could show a
 * title, some badges and a count of notes behind a disclosure triangle, and
 * that was the end of the road — there was no way to see a task's description,
 * its full commentary, what it was waiting on, or how it got to where it is.
 *
 * Everything here already existed in the store. None of it had ever been shown.
 */

import { useEffect, useMemo, useState } from "react";
import {
  api,
  type Digest,
  type Entity,
  type EventRow,
  type Neighbour,
  type Note,
  type Page as PageOf,
  type SessionClient,
} from "../lib/api";
import { useAsync } from "../lib/useAsync";
import { ApiError } from "../lib/api";
import {
  Badge,
  Button,
  Card,
  Dialog,
  Empty,
  ErrorBox,
  Id,
  Spinner,
  Tooltip,
  cx,
  statusTone,
  when,
} from "../components/ui";
import { CloseTaskDialog } from "../components/CloseTaskDialog";
import { LabelPicker } from "../components/LabelPicker";
import { Markdown } from "../components/Markdown";
import { Page, projectCrumbs } from "../components/Page";
import { href, navigate } from "../lib/router";
import { groupRank, relationPhrase, type Direction } from "../lib/relations";
import { inBoardOrder, taskRef, type RankMap } from "../lib/tasks";
import type { ScreenProps } from "../App";

/** A neighbour, plus which way the edge was walked to reach it. */
interface Related extends Neighbour {
  direction: Direction;
}

export function TaskScreen({
  route,
  generation,
  milestoneNoun,
  inboxOn,
}: ScreenProps) {
  const project = route.project;
  const id = route.taskId;

  // Six requests, deliberately not folded into one. Each answers a different
  // question of a different part of the store, and the daemon is on localhost
  // — the alternative is an endpoint that exists only to serve this screen and
  // has to change every time the screen does.
  const core = useAsync(async () => {
    if (!id) return null;
    const [entity, notes, history, outbound, inbound, clients] =
      await Promise.all([
        api.entity(id),
        api.notesFor(id),
        api.history(id),
        api.graph(id, "outbound", 1),
        api.graph(id, "inbound", 1),
        // Which editor wrote each note. One request for the screen rather than
        // one per note, and a lookup by session id afterwards.
        //
        // Its failure is swallowed, and that is deliberate rather than lazy.
        // Everything else here is the page; this is a label on part of it, and
        // a `Promise.all` makes any rejection the whole screen's. A daemon
        // older than this endpoint — the ordinary state of an app updated
        // before the binary under it — would blank the task page to avoid
        // naming an editor. Falling back to none collapses into the state the
        // display already handles: unknown, and so nothing shown.
        api.clients().catch(() => ({ clients: [], total: 0 })),
      ]);
    return { entity, notes, history, outbound, inbound, clients };
  }, [id, generation]);

  // The siblings, so J and K walk the board's own order, and the milestones, so
  // a milestone reads as its name rather than as a ULID.
  const context = useAsync(async () => {
    if (!project) return null;
    const [tasks, digest, milestones] = await Promise.all([
      api.entities({ project, type: "task", limit: 2000 }),
      api.context(project),
      api.entities({ project, type: "milestone", limit: 200 }),
    ]);
    return { tasks, digest, milestones };
  }, [project, generation]);

  const task = core.data?.entity.artifacts[0]?.entity;
  const key = (context.data?.digest as Digest | undefined)?.project?.key;

  const rank = useMemo<RankMap>(() => {
    const m: RankMap = new Map();
    (context.data?.digest as Digest | undefined)?.next_up?.ready.forEach(
      (item, i) => m.set(item.id, { position: i + 1, why: item.why }),
    );
    return m;
  }, [context.data]);

  const siblings = useMemo(
    () =>
      inBoardOrder(
        (context.data?.tasks as PageOf<Entity> | undefined)?.items ?? [],
        rank,
      ),
    [context.data, rank],
  );

  const related = useMemo<Related[]>(() => {
    const out = (core.data?.outbound.neighbours ?? []).map((n) => ({
      ...n,
      direction: "outbound" as const,
    }));
    const inb = (core.data?.inbound.neighbours ?? []).map((n) => ({
      ...n,
      direction: "inbound" as const,
    }));
    return [...out, ...inb].sort(
      (a, b) =>
        groupRank(a.rel, a.direction) - groupRank(b.rel, b.direction) ||
        a.label.localeCompare(b.label),
    );
  }, [core.data]);

  // J and K move between tasks without leaving the page; Escape closes it.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (target && ["INPUT", "TEXTAREA"].includes(target.tagName)) return;
      if (e.metaKey || e.ctrlKey || e.altKey) return;

      if (e.key === "Escape") {
        e.preventDefault();
        // Back when there is somewhere to go back to — the reader almost
        // always arrived from the board and expects to land where they left.
        // Otherwise, on a link opened cold, go to the board rather than
        // nowhere.
        if (window.history.length > 1) window.history.back();
        else if (project) navigate({ screen: "board", project });
        return;
      }

      const step = e.key === "j" ? 1 : e.key === "k" ? -1 : 0;
      if (step === 0 || !id || siblings.length === 0) return;
      const at = siblings.findIndex((t) => String(t.id) === id);
      if (at === -1) return;
      const next = siblings[at + step];
      if (!next) return;
      e.preventDefault();
      navigate({ screen: "task", project, taskId: taskRef(key, next) });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [id, project, siblings, key]);

  if (!id || !project) return <Empty message="No task named." />;
  if (core.loading && !core.data) return <Spinner />;
  if (core.error) {
    return (
      <Page title="Task" crumbs={projectCrumbs(route, "Task")}>
        <ErrorBox error={core.error} retry={core.reload} />
      </Page>
    );
  }
  if (!task) {
    return (
      <Page title="Task" crumbs={projectCrumbs(route, "Task")}>
        <Empty
          message="No such task."
          hint="It may have been archived. Nothing in Specline is deleted, so it is still in the history."
        />
      </Page>
    );
  }

  const at = siblings.findIndex((t) => String(t.id) === id);
  const previous = at > 0 ? siblings[at - 1] : undefined;
  const next = at !== -1 ? siblings[at + 1] : undefined;
  const ranked = rank.get(id);

  // Every label this project uses, so the picker completes against the same
  // set the board's does. Built from the tasks already loaded for the sibling
  // walk — a label exists exactly as long as something carries it, so there is
  // no registry to ask.
  const labelsInUse = [
    ...new Set(
      ((context.data?.tasks as PageOf<Entity> | undefined)?.items ?? []).flatMap(
        (t) => (t.labels as string[] | undefined) ?? [],
      ),
    ),
  ].sort();
  // Until the project has loaded we do not know the key. Showing the ULID for
  // that moment makes the title flicker from a wall of characters to `KEEL-76`
  // on every single open, so show nothing rather than the wrong thing — and
  // fall back to the ULID only once we know the key is not coming.
  const reference = !key && context.loading ? "" : taskRef(key, task);

  return (
    <Page
      title={
        <span className="flex items-baseline gap-2.5">
          {reference && (
            <span className="font-mono text-heading text-ink-faint">
              {reference}
            </span>
          )}
          <span>{String(task.title)}</span>
        </span>
      }
      crumbs={[
        ...projectCrumbs(route, "Board").slice(0, 2),
        { label: "Board", route: { screen: "board", project } },
        { label: reference || String(task.title) },
      ]}
      width="wide"
      meta={
        <Badge tone={statusTone(String(task.status))}>
          {String(task.status)}
        </Badge>
      }
      actions={
        <span className="flex items-center gap-2 text-micro text-ink-faint">
          <a
            href={
              previous
                ? href({
                    screen: "task",
                    project,
                    taskId: taskRef(key, previous),
                  })
                : undefined
            }
            aria-disabled={!previous}
            className={cx("hover:text-ink", !previous && "opacity-30")}
          >
            ← previous
          </a>
          <kbd className="font-mono">K</kbd>
          <kbd className="font-mono">J</kbd>
          <a
            href={
              next
                ? href({ screen: "task", project, taskId: taskRef(key, next) })
                : undefined
            }
            aria-disabled={!next}
            className={cx("hover:text-ink", !next && "opacity-30")}
          >
            next →
          </a>
        </span>
      }
    >
      {/* The second fetch failing is not fatal — the task itself is on screen —
          but it silently costs the readable identifier, the milestone's name
          and J/K, and a page that quietly does less while looking complete is
          the failure this project treats as the serious one. Say so. */}
      {context.error && (
        <div className="mb-4 rounded-lg border border-warn/40 bg-warn/10 px-3 py-2 text-small text-warn">
          Showing this task on its own: the rest of the project could not be
          loaded, so it is named by its long id and J/K will not move between
          tasks.{" "}
          <button type="button" onClick={context.reload} className="underline">
            Try again
          </button>
        </div>
      )}

      <div className="grid gap-5 lg:grid-cols-[minmax(0,1fr)_18rem]">
        <div className="min-w-0 space-y-5">
          {/* Two fields can hold the description and a row usually has one of
              them, not both. `body` is optional and long-form; `summary` is
              required since 8G and is what every list shows. So the rows written
              before that rule have a body and no summary, and the rows written
              since tend to have a summary and no body — and this card used to
              read `body` alone, which meant thirty-one tasks carrying a
              seven-hundred-character description displayed "No description."
              (KEEL-170).

              Prefer the body, fall back to the summary, and say which one it is
              rather than letting a one-sentence summary pass for the long-form
              detail a reader came here to find. */}
          <Card
            title="Description"
            actions={
              !task.body && task.summary ? (
                <span className="text-small text-ink-faint">
                  from the summary
                </span>
              ) : undefined
            }
          >
            {task.body || task.summary ? (
              <Markdown>{String(task.body || task.summary)}</Markdown>
            ) : (
              <Empty message="No description." />
            )}
          </Card>

          {/* The task's own id, not the route's. The app addresses tasks by
              reference — `KEEL-240` — and the API addresses them by ULID, so
              handing the route parameter to a write endpoint produces a 400
              that looks like a broken button. */}
          <NoteStream
            notes={core.data?.notes.notes ?? []}
            clients={core.data?.clients.clients ?? []}
            entityId={task ? String(task.id) : undefined}
            onAdded={core.reload}
          />
          <History
            events={core.data?.history.events ?? []}
            truncated={core.data?.history}
          />
        </div>

        <aside className="space-y-5">
          <Card
            title="Properties"
            footer={<TaskActions task={task} onChanged={core.reload} />}
          >
            <dl className="space-y-2.5 text-small">
              <EditableFields
                task={task}
                milestones={
                  (context.data?.milestones as PageOf<Entity> | undefined)
                    ?.items ?? []
                }
                labelsInUse={labelsInUse}
                milestoneNoun={milestoneNoun}
                inboxOn={inboxOn}
                onChanged={core.reload}
              />
              {ranked && (
                <Property label="Next up">
                  <Tooltip align="right" text={ranked.why}>
                    <Badge tone="border-accent/50 bg-accent/10 text-accent">
                      #{ranked.position}
                    </Badge>
                  </Tooltip>
                </Property>
              )}
              <Property label="Created">
                {when(String(task.audit.created_at))}
              </Property>
              <Property label="Updated">
                {when(String(task.audit.updated_at))}
              </Property>
              {task.closed_at ? (
                <Property label="Closed">
                  {when(String(task.closed_at))}
                </Property>
              ) : null}
              {((task.external_refs as string[] | undefined) ?? []).length >
                0 && (
                <Property label="Links">
                  <span className="flex flex-col gap-0.5">
                    {((task.external_refs as string[] | undefined) ?? []).map(
                      (url) => (
                        <a
                          key={url}
                          href={url}
                          target="_blank"
                          rel="noreferrer"
                          className="truncate text-accent hover:underline"
                        >
                          {url.replace(/^https?:\/\//, "")}
                        </a>
                      ),
                    )}
                  </span>
                </Property>
              )}
              <Property label="Ref">
                <span className="font-mono">{reference}</span>
              </Property>
              <Property label="Internal id">
                <Id value={String(task.id)} />
              </Property>
            </dl>
          </Card>

          <Family
            task={task}
            siblings={siblings}
            project={project}
            projectKey={key}
          />
          <Relationships related={related} project={project} />
          {reference && <AskClaude reference={reference} />}
        </aside>
      </div>
    </Page>
  );
}

/**
 * What this is part of, and what is part of it.
 *
 * Composition, not blocking. The two were the same edge before a task had a
 * parent, which is why a rollup was impossible: `blocks` means "must happen
 * first", and every inbound one is read by the ranking as something in the way.
 *
 * The progress count is the whole reason a parent is worth having — "3 of 7"
 * is the question an epic exists to answer.
 */
function Family({
  task,
  siblings,
  project,
  projectKey,
}: {
  task: Entity;
  siblings: Entity[];
  project: string;
  projectKey: string | undefined;
}) {
  const parent = task.parent_id
    ? siblings.find((t) => String(t.id) === String(task.parent_id))
    : undefined;
  const children = siblings.filter(
    (t) => String(t.parent_id) === String(task.id),
  );
  if (!parent && children.length === 0) return null;

  const done = children.filter((t) =>
    ["done", "wont_do"].includes(String(t.status)),
  ).length;

  return (
    <Card title="Part of">
      <div className="space-y-3">
        {parent && (
          <div>
            <h3 className="mb-1 text-micro tracking-wide text-ink-faint uppercase">
              Parent
            </h3>
            <a
              href={href({
                screen: "task",
                project,
                taskId: taskRef(projectKey, parent),
              })}
              className="text-small hover:underline"
            >
              {String(parent.title)}
            </a>
          </div>
        )}
        {children.length > 0 && (
          <div>
            <h3 className="mb-1 flex items-baseline gap-2 text-micro tracking-wide text-ink-faint uppercase">
              Sub-tasks
              <span className="tabular-nums">
                {done} of {children.length} done
              </span>
            </h3>
            <ul className="space-y-1">
              {children.map((child) => (
                <li
                  key={String(child.id)}
                  className="flex items-baseline gap-2"
                >
                  <Badge tone={statusTone(String(child.status))}>
                    {String(child.status)}
                  </Badge>
                  <a
                    href={href({
                      screen: "task",
                      project,
                      taskId: taskRef(projectKey, child),
                    })}
                    className="min-w-0 flex-1 truncate text-small hover:underline"
                  >
                    {String(child.title)}
                  </a>
                </li>
              ))}
            </ul>
          </div>
        )}
      </div>
    </Card>
  );
}

/**
 * The five fields a person moves while looking at a row.
 *
 * Hard constraint 7 names this half as the interface's: moving a status or a
 * priority is a person's own action, distinct from authoring the reasoning,
 * which stays with Claude. Until KEEL-307 the panel rendered all five as text,
 * so a kind chosen wrongly at creation could only be fixed by opening a
 * conversation about it.
 *
 * Each control saves on change rather than collecting an Edit mode and a Save
 * button. These are single gestures — you are picking a priority, not filling
 * in a form — and a mode you can leave without saving is a way to lose work
 * that a select does not have.
 *
 * Two absences are deliberate:
 *
 * - **`done` and `wont_do` are not in the status list.** They owe a reason, a
 *   message and evidence, which the Close button below collects. A select that
 *   could reach them would be a way round the form that asks.
 * - **`in_progress` is not selectable either**, though it is shown when the
 *   task is in it. Starting work is a claim and a claim records *which
 *   session*; a person clicking a dropdown has none, and the board saying
 *   something is in flight without saying who is the state `specline_claim`
 *   exists to prevent. Moving *out* of it releases the claim, which the daemon
 *   does rather than this form.
 *
 * A closed task keeps its status fixed — reopening means deciding what becomes
 * of the reason and the evidence, and that is a question rather than a
 * control — but its priority, kind, phase and labels stay editable, because
 * recategorising something finished is ordinary.
 */
function EditableFields({
  task,
  milestones,
  labelsInUse,
  milestoneNoun,
  inboxOn,
  onChanged,
}: {
  task: Entity;
  milestones: Entity[];
  /** Every label on this project, for the picker to complete against. */
  labelsInUse: string[];
  milestoneNoun: string | undefined;
  /** Whether the feature-request lifecycle is switched on (KEEL-341). */
  inboxOn?: boolean;
  onChanged: () => void;
}) {
  const [saving, setSaving] = useState<string | null>(null);
  const [failed, setFailed] = useState<string | null>(null);

  const status = String(task.status);
  const closed = ["done", "wont_do"].includes(status);
  const labels = (task.labels as string[] | undefined) ?? [];
  const version = Number(task.audit.version);

  // Every control goes inert while any one of them is saving, rather than only
  // the one being saved. Two changes in flight would send the same version
  // twice and the second would 409 — and the alternative, dropping the second,
  // is a click that does nothing and then silently reverts.
  async function save(field: string, changes: Record<string, unknown>) {
    if (saving) return;
    setSaving(field);
    setFailed(null);
    try {
      await api.updateTask(String(task.id), { version, ...changes });
      onChanged();
    } catch (e) {
      setFailed(
        e instanceof ApiError
          ? // A 409 is the one worth wording differently: nothing is broken,
            // the row simply moved under you, and reloading is the fix.
            e.status === 409
            ? "This task changed while you were looking at it. Reload to see where it got to."
            : e.message
          : `The ${field} was not changed.`,
      );
    } finally {
      setSaving(null);
    }
  }

  return (
    <>
      <Property label="Status">
        {closed ? (
          <Badge tone={statusTone(status)}>{status}</Badge>
        ) : (
          <FieldSelect
            label="Status"
            value={status}
            busy={saving !== null}
            onChange={(next) => void save("status", { status: next })}
            options={[
              { value: "todo", label: "todo" },
              { value: "review", label: "review" },
            ]}
            // Shown so the select tells the truth about where the task is, and
            // unselectable so it cannot be reached from here.
            fixed={status === "in_progress" ? "in_progress" : undefined}
          />
        )}
      </Property>

      <Property label="Priority">
        <FieldSelect
          label="Priority"
          value={String(task.priority)}
          busy={saving !== null}
          onChange={(next) => void save("priority", { priority: next })}
          options={["p0", "p1", "p2", "p3"].map((p) => ({
            value: p,
            label: p,
          }))}
        />
      </Property>

      <Property label="Kind">
        <FieldSelect
          label="Kind"
          value={String(task.kind)}
          busy={saving !== null}
          onChange={(next) => void save("kind", { kind: next })}
          // `feature` only when the lifecycle is on. Offering it otherwise
          // would let somebody turn a task into the container half of a
          // lifecycle whose other half is hidden (KEEL-341).
          options={(inboxOn === false
            ? ["task", "bug", "chore", "spike"]
            : ["task", "bug", "chore", "spike", "feature"]
          ).map((k) => ({ value: k, label: k }))}
        />
      </Property>

      <Property label={milestoneNoun ?? "Milestone"}>
        <FieldSelect
          label={milestoneNoun ?? "Milestone"}
          value={task.milestone_id ? String(task.milestone_id) : ""}
          busy={saving !== null}
          onChange={(next) => void save("milestone", { milestone: next })}
          options={[
            { value: "", label: "none" },
            ...milestones.map((m) => ({
              value: String(m.id),
              label: String(m.name),
            })),
          ]}
        />
      </Property>

      <Property label="Labels">
        <LabelPicker
          available={labelsInUse}
          chosen={labels}
          heading={null}
          onChange={(next) => void save("labels", { labels: next })}
        />
      </Property>

      {failed && (
        <p role="alert" className="text-micro text-bad">
          {failed}
        </p>
      )}
    </>
  );
}

/**
 * One field, saved the moment it changes.
 *
 * `fixed` is the value a task is currently in but cannot be moved to — today
 * only `in_progress`. It is rendered as a disabled option rather than left out,
 * because a select that silently displays something not in its own list reads
 * as a bug, and one that displayed `todo` for a task that is in progress would
 * be lying about the row.
 */
function FieldSelect({
  label,
  value,
  options,
  busy,
  fixed,
  onChange,
}: {
  label: string;
  value: string;
  options: { value: string; label: string }[];
  busy: boolean;
  fixed?: string;
  onChange: (value: string) => void;
}) {
  return (
    <select
      aria-label={label}
      value={value}
      disabled={busy}
      onChange={(e) => onChange(e.target.value)}
      className={cx(
        "w-full rounded-md border border-border-subtle bg-surface px-2 py-1 text-small text-ink",
        busy && "opacity-60",
      )}
    >
      {fixed !== undefined && (
        <option value={fixed} disabled>
          {fixed}
        </option>
      )}
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {o.label}
        </option>
      ))}
    </select>
  );
}

function Property({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-baseline gap-3">
      <dt className="w-20 shrink-0 text-micro tracking-wide text-ink-faint uppercase">
        {label}
      </dt>
      <dd className="min-w-0 flex-1">{children}</dd>
    </div>
  );
}

/**
 * The commentary, as a feed rather than a disclosure triangle.
 *
 * Full markdown, because notes are written as prose and the board was showing
 * them as one unbroken line. Retracted notes stay, struck through: what a
 * session once believed is part of how the row got here, and hiding it rewrites
 * the record.
 */
/**
 * What a person can do to this task from here.
 *
 * Both of these are *capture* in the sense B-78 means: closing records a
 * decision somebody already made, and archiving records that a row stopped
 * mattering. Neither writes prose about why the work went the way it did —
 * that is the note stream above and, for anything longer, Claude.
 */
function TaskActions({
  task,
  onChanged,
}: {
  task: Entity;
  onChanged: () => void;
}) {
  const [closing, setClosing] = useState(false);
  const [archiving, setArchiving] = useState(false);
  const open = !["done", "wont_do"].includes(String(task.status));
  const archived = Boolean(task.audit?.archived_at);

  return (
    <>
      <div className="flex flex-wrap gap-2">
        {open && (
          <Button size="sm" onClick={() => setClosing(true)}>
            Close…
          </Button>
        )}
        {!archived && (
          <Button size="sm" variant="ghost" onClick={() => setArchiving(true)}>
            Archive…
          </Button>
        )}
        {archived && (
          <span className="text-micro text-ink-faint">
            Archived. It stays readable, and stays in the history.
          </span>
        )}
      </div>

      <CloseTaskDialog
        open={closing}
        task={task}
        onClose={() => setClosing(false)}
        onDone={onChanged}
      />
      <ArchiveDialog
        open={archiving}
        task={task}
        onClose={() => setArchiving(false)}
        onDone={onChanged}
      />
    </>
  );
}

/**
 * Archiving, and saying plainly what that is.
 *
 * The button a person is looking for says Delete in most tools, and hard
 * constraint 3 means nothing is ever removed. Rather than offering the word and
 * breaking the promise, this says what actually happens — and there is no undo
 * in the interface yet, which is worth admitting before the click rather than
 * after.
 */
function ArchiveDialog({
  open,
  task,
  onClose,
  onDone,
}: {
  open: boolean;
  task: Entity;
  onClose: () => void;
  onDone: () => void;
}) {
  const [saving, setSaving] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);

  async function submit() {
    if (saving) return;
    setSaving(true);
    setFailed(null);
    try {
      await api.archive(String(task.id));
      onClose();
      onDone();
    } catch (e) {
      setFailed(e instanceof ApiError ? e.message : "It was not archived.");
    } finally {
      setSaving(false);
    }
  }

  return (
    <Dialog open={open} onClose={onClose} label="Archive this task">
      <div className="max-w-md space-y-3 p-4">
        <h2 className="text-small font-semibold text-ink">
          Archive this task?
        </h2>
        <p className="text-small text-ink-muted">
          It stops appearing on the board and stays readable — nothing in
          Specline is ever deleted, so the row and its history survive. Ask
          Claude if you need it back; there is no undo here yet.
        </p>
        {failed && (
          <p role="alert" className="text-micro text-bad">
            {failed}
          </p>
        )}
        <div className="flex justify-end gap-2 pt-1">
          <Button size="sm" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            size="sm"
            variant="danger"
            onClick={() => void submit()}
            disabled={saving}
          >
            {saving ? "Archiving…" : "Archive"}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}

/**
 * The comment box.
 *
 * The first thing a person can write into Specline from the interface, and the
 * shape of it is the whole of hard constraint 7 as B-78 rewrote it: a note is
 * what somebody *observed*, which is capture. There is no box on this screen
 * for rewriting the task's description, because that is authoring and it stays
 * with Claude.
 *
 * The note goes in attributed `human` on the `ui` surface — the daemon decides
 * that from the token, not from anything sent here, so a page cannot claim to
 * be a person.
 */
function NoteComposer({
  entityId,
  onAdded,
}: {
  entityId: string;
  onAdded: () => void;
}) {
  const [body, setBody] = useState("");
  const [saving, setSaving] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);

  const empty = body.trim() === "";

  async function submit() {
    if (empty || saving) return;
    setSaving(true);
    setFailed(null);
    try {
      await api.addNote(entityId, body.trim());
      setBody("");
      onAdded();
    } catch (e) {
      setFailed(e instanceof ApiError ? e.message : "The note was not saved.");
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="space-y-2">
      <textarea
        value={body}
        onChange={(e) => setBody(e.target.value)}
        // Cmd-Enter, because this is a multi-line box and Enter has to mean
        // newline in something people write paragraphs into.
        onKeyDown={(e) => {
          if ((e.metaKey || e.ctrlKey) && e.key === "Enter") void submit();
        }}
        rows={3}
        placeholder="What did you find out?"
        aria-label="Add a note"
        disabled={saving}
        className="w-full resize-y rounded-md border border-border-subtle bg-surface px-3 py-2 text-small text-ink placeholder:text-ink-faint focus:border-accent focus:outline-none disabled:opacity-60"
      />
      <div className="flex items-center gap-3">
        <Button
          size="sm"
          onClick={() => void submit()}
          disabled={empty || saving}
        >
          {saving ? "Saving…" : "Add note"}
        </Button>
        <span className="text-micro text-ink-faint">⌘↵</span>
        {failed && (
          <span role="alert" className="text-micro text-bad">
            {failed}
          </span>
        )}
      </div>
    </div>
  );
}

/**
 * Which editor wrote something, when that is known.
 *
 * Renders nothing at all when it is not. The alternative is a word like
 * "unknown" beside every row written before this was recorded, which is
 * accurate and reads as breakage — a screen that says it does not know
 * thirty times says nothing else. The absence is the honest display, and the
 * session id is still there for anyone who needs to chase it.
 *
 * Never a live indicator. What is known is that this conversation wrote from
 * this editor, not that the editor is running now.
 */
function Editor({ client }: { client: SessionClient | undefined }) {
  if (!client) return null;
  return (
    <Tooltip align="left" text="The editor this conversation was driven from">
      <span className="text-ink-muted">
        {client.display_name}
        {client.version ? ` ${client.version}` : ""}
      </span>
    </Tooltip>
  );
}

function NoteStream({
  notes,
  clients,
  entityId,
  onAdded,
}: {
  notes: Note[];
  clients: SessionClient[];
  entityId: string | undefined;
  onAdded: () => void;
}) {
  // Session id to editor, built once for the stream rather than searched per
  // note. A session missing from here is unknown rather than absent-and-so-
  // Claude Code: nothing wrote a client for it, and guessing which editor that
  // was is the one thing this display must not do.
  const editors = new Map(clients.map((c) => [c.session_id, c]));
  const composer = entityId ? (
    <NoteComposer entityId={entityId} onAdded={onAdded} />
  ) : null;

  if (notes.length === 0) {
    return (
      <Card title="Notes">
        <Empty
          message="Nothing recorded yet."
          hint="Notes are what a session learned while doing the work — the part a status cannot carry."
        />
        {composer}
      </Card>
    );
  }
  return (
    <Card title={`Notes (${notes.length})`} footer={composer}>
      <ol className="space-y-4">
        {notes.map((note) => {
          const retracted = Boolean(note.archived_at);
          return (
            <li
              key={note.id}
              className={cx(
                "border-l-2 pl-3",
                retracted
                  ? "border-border-subtle opacity-60"
                  : "border-accent/40",
              )}
            >
              <div className="mb-1 flex flex-wrap items-center gap-2 text-micro text-ink-faint">
                <span className="font-medium text-ink-muted">
                  {note.author}
                </span>
                <span>{when(note.created_at)}</span>
                {note.session_id ? (
                  <>
                    <Tooltip
                      align="left"
                      text="The conversation that wrote this"
                    >
                      <span className="font-mono">{note.session_id}</span>
                    </Tooltip>
                    <Editor client={editors.get(note.session_id)} />
                  </>
                ) : (
                  <span>written outside a tracked session</span>
                )}
                {retracted && (
                  <Badge tone="border-bad/40 text-bad">retracted</Badge>
                )}
              </div>
              <div className={cx("min-w-0", retracted && "line-through")}>
                <Markdown>{note.body}</Markdown>
              </div>
            </li>
          );
        })}
      </ol>
    </Card>
  );
}

/**
 * How the task got here.
 *
 * Every field change is already in the event log with its before and after, and
 * until now nothing in the app had ever rendered it.
 */
function History({
  events,
  truncated,
}: {
  events: EventRow[];
  truncated?: { total: number; truncated: boolean };
}) {
  if (events.length === 0) {
    return (
      <Card title="History">
        <Empty message="No recorded changes." />
      </Card>
    );
  }
  return (
    <Card title="History">
      <ol className="space-y-1.5 text-small">
        {[...events].reverse().map((e) => (
          <li key={e.id} className="flex items-baseline gap-3">
            <span className="w-16 shrink-0 text-right text-micro tabular-nums text-ink-faint">
              {when(e.created_at)}
            </span>
            <span className="w-14 shrink-0 text-micro text-ink-faint">
              {e.actor}
            </span>
            <span className="min-w-0 flex-1">
              {e.field ? (
                <>
                  <span className="text-ink-muted">{e.field}</span>{" "}
                  <span className="text-ink-faint">{short(e.before)}</span>
                  <span className="mx-1 text-ink-faint">→</span>
                  <span>{short(e.after)}</span>
                </>
              ) : (
                <span className="text-ink-muted">{e.summary}</span>
              )}
            </span>
          </li>
        ))}
      </ol>
      {truncated?.truncated && (
        <p className="mt-3 text-small text-ink-faint">
          Showing {events.length} of {truncated.total} changes.
        </p>
      )}
    </Card>
  );
}

/** A before/after value, short enough to sit on one line. */
function short(value: unknown): string {
  if (value === null || value === undefined) return "—";
  const text = typeof value === "string" ? value : JSON.stringify(value);
  return text.length > 60 ? `${text.slice(0, 57)}…` : text;
}

/**
 * What this is connected to, stated as sentences.
 *
 * Grouped by the phrase rather than by the stored verb, because `blocks` means
 * two different things depending on which way the edge was walked, and printing
 * the verb states half of them backwards.
 */
function Relationships({
  related,
  project,
}: {
  related: Related[];
  project: string;
}) {
  if (related.length === 0) {
    return (
      <Card title="Connected">
        <Empty message="Nothing linked." />
      </Card>
    );
  }

  const groups = new Map<string, Related[]>();
  for (const r of related) {
    const phrase = relationPhrase(r.rel, r.direction);
    const list = groups.get(phrase);
    if (list) list.push(r);
    else groups.set(phrase, [r]);
  }

  return (
    <Card title="Connected">
      <div className="space-y-3">
        {[...groups].map(([phrase, items]) => (
          <div key={phrase}>
            <h3 className="mb-1 text-micro tracking-wide text-ink-faint uppercase">
              {phrase}
            </h3>
            <ul className="space-y-1">
              {items.map((r) => (
                <li key={`${r.id}-${r.rel}-${r.direction}`}>
                  <a
                    href={href(
                      r.entity_type === "task"
                        ? { screen: "task", project, taskId: r.id }
                        : { screen: "documents", project, documentId: r.id },
                    )}
                    className="flex items-baseline gap-1.5 text-small hover:underline"
                  >
                    <Badge>{r.entity_type}</Badge>
                    <span className="min-w-0 flex-1">
                      {r.label || <Id value={r.id} />}
                    </span>
                    {r.anchor && (
                      <Badge tone="border-accent/40 text-accent">
                        {r.anchor}
                      </Badge>
                    )}
                  </a>
                </li>
              ))}
            </ul>
          </div>
        ))}
      </div>
    </Card>
  );
}

/**
 * Prompts you can paste into Claude Code, with this task's identifier in them.
 *
 * The app cannot write — Claude and Specline are the only writers — and a
 * read-only surface can read as either deliberate or inert. The difference is
 * whether it hands you the next move. These are the four things you most often
 * want to do to a task you are looking at, already addressed to the right one.
 *
 * Copying rather than deep-linking: there is no URL that puts text into a
 * Claude Code session, and a button that pretended otherwise would be worse
 * than one that is honest about the clipboard.
 */
function AskClaude({ reference }: { reference: string }) {
  const [copied, setCopied] = useState<string | null>(null);

  const prompts = [
    `close ${reference} as done with the commit`,
    `what is blocking ${reference}`,
    `split ${reference} into sub-tasks`,
    `add a note to ${reference}`,
  ];

  async function copy(prompt: string) {
    try {
      await navigator.clipboard.writeText(prompt);
      setCopied(prompt);
      window.setTimeout(
        () => setCopied((c: string | null) => (c === prompt ? null : c)),
        1500,
      );
    } catch {
      // A denied clipboard is not worth an error state on a convenience: the
      // text is on screen and can be selected by hand.
    }
  }

  return (
    <Card title="Ask Claude">
      <ul className="space-y-1">
        {prompts.map((prompt) => (
          <li key={prompt}>
            <button
              type="button"
              onClick={() => copy(prompt)}
              title="Copy, then paste into Claude Code"
              className="w-full rounded-control px-2 py-1.5 text-left text-small text-ink-muted hover:bg-surface-hover hover:text-ink"
            >
              <span className="font-mono text-micro">
                {copied === prompt ? "copied" : "copy"}
              </span>{" "}
              {prompt}
            </button>
          </li>
        ))}
      </ul>
    </Card>
  );
}
