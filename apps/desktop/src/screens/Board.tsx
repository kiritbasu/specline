/**
 * Screen 4 — the tasks. Two layouts, one address.
 *
 * Board or list, grouped and sorted how you choose, filtered by anything the
 * rows carry — and all of it in the URL, so the view you are looking at is a
 * link you can send. That is also what gives saved views for free: bookmark it.
 *
 * The ranked "Next" panel stays visible whatever is filtered. It used to
 * disappear the moment you touched a filter, which meant the best thing in the
 * app was only ever there when you did not need it.
 */

import { useEffect, useMemo, useState } from "react";
import {
  api,
  type Entity,
  type NextItem,
  type Page as PageOf,
} from "../lib/api";
import { useAsync } from "../lib/useAsync";
import { ApiError } from "../lib/api";
import { Button, Dialog, Empty, ErrorBox, Spinner, toast } from "../components/ui";
import { CloseTaskDialog } from "../components/CloseTaskDialog";
import { LabelPicker } from "../components/LabelPicker";
import { Page, projectCrumbs } from "../components/Page";
import { FilterBar, type Facets, type View } from "../components/FilterBar";
import { TaskBoard } from "../components/TaskBoard";
import { TaskList } from "../components/TaskList";
import { href, setQuery } from "../lib/router";
import {
  GROUP_BY,
  SORT_BY,
  dropOnStatus,
  groupTasks,
  sortTasks,
  taskRef,
  type GroupBy,
  type RankMap,
  type SortBy,
  type SortDir,
} from "../lib/tasks";
import {
  applyFilter,
  filterToQuery,
  isFiltering,
  parseFilter,
} from "../lib/filters";
import type { ScreenProps } from "../App";

export function BoardScreen({
  route,
  generation,
  milestoneNoun,
  projectKey,
  inboxOn,
}: ScreenProps) {
  const project = route.project;

  // The whole view comes out of the address. Anything unrecognised falls back
  // to the default rather than erroring — a hand-edited URL should degrade, not
  // break.
  const filter = parseFilter(route.query);
  const layout = route.query.view === "list" ? "list" : "board";
  const group: GroupBy = GROUP_BY.includes(route.query.group as GroupBy)
    ? (route.query.group as GroupBy)
    : "status";
  const sort: SortBy = SORT_BY.includes(route.query.sort as SortBy)
    ? (route.query.sort as SortBy)
    : "next";
  const dir: SortDir = route.query.dir === "desc" ? "desc" : "asc";

  const { data, error, loading, reload } = useAsync<PageOf<Entity>>(
    () => api.entities({ project, type: "task", limit: 2000 }),
    [project, generation],
  );

  // The same ranking the digest gives an agent, rather than a second opinion
  // computed in the browser. A board that disagrees with what Claude was told
  // is worse than a board with no ordering at all.
  //
  // This was `api.context(project)` — the whole digest, 27 KB and the slowest
  // read the board waited on, for the ranking and the blocked set and nothing
  // else. `/api/ready` is the same computation with the briefing left off. The
  // limit matches the digest's own cap of three, so the ranking a card shows is
  // the ranking a session was given rather than a longer list the app invented.
  const next = useAsync<{ ready: NextItem[]; blocked?: string[] }>(
    () => api.ready({ project: project ?? "", blocked: "true", limit: 3 }),
    [project, generation],
  );

  const milestones = useAsync<PageOf<Entity>>(
    () => api.entities({ project, type: "milestone", limit: 200 }),
    [project, generation],
  );

  // Every count in one request. Seventy cards asking individually is seventy
  // round trips to render a number — and asking for the bodies was 150 KB of
  // prose to run `length` on, which is what `counts` leaves behind.
  const notes = useAsync<{ counts: Record<string, number>; total: number }>(
    () => api.noteCounts(project),
    [project, generation],
  );
  const noteCounts = useMemo(
    () => new Map(Object.entries(notes.data?.counts ?? {})),
    [notes.data],
  );

  const ready = next.data?.ready ?? null;

  const rank = useMemo<RankMap>(() => {
    const m: RankMap = new Map();
    ready?.forEach((item, i) =>
      m.set(item.id, { position: i + 1, why: item.why }),
    );
    return m;
  }, [ready]);

  // What "blocked" means here is what it means to the ranking: something is
  // linked to it as a blocker. The app must not grow a second opinion, so these
  // are the ids `specline_core::next::blocked_tasks` returns — the same function the
  // digest and the generated tracker count from.
  const blockedIds = useMemo(
    () => new Set(next.data?.blocked ?? []),
    [next.data],
  );

  // Milestones and tasks share one lookup, because grouping needs a name for
  // whatever it grouped by and both kinds of key land in the same place.
  const groupNames = useMemo(() => {
    const names = new Map<string, string>();
    for (const m of milestones.data?.items ?? [])
      names.set(String(m.id), String(m.name));
    for (const t of data?.items ?? []) names.set(String(t.id), String(t.title));
    return names;
  }, [milestones.data, data]);

  // Milestone names only. `groupNames` above also carries task titles, because
  // grouping by parent needs them — looking a card's milestone up in that map
  // would find a task title for any id that happened to match.
  const milestoneNames = useMemo(() => {
    const names = new Map<string, string>();
    for (const m of milestones.data?.items ?? [])
      names.set(String(m.id), String(m.name));
    return names;
  }, [milestones.data]);

  const facets = useMemo<Facets>(() => {
    const labels = new Set<string>();
    for (const task of data?.items ?? []) {
      for (const label of (task.labels as string[] | undefined) ?? [])
        labels.add(label);
    }
    return {
      labels: [...labels].sort(),
      milestones: (milestones.data?.items ?? []).map((m) => ({
        id: String(m.id),
        name: String(m.name),
      })),
    };
  }, [data, milestones.data]);

  // The phase in flight, for the new-task dialog to default to: the one holding
  // the most *open* work.
  //
  // This was "the earliest phase still open", which sounds equivalent and is
  // not — it picked Phase 4, open since forever and where nothing is happening.
  // Where the open tasks are is what "in flight" actually means, and the board
  // is already holding every task, so it costs nothing to ask.
  //
  // A default rather than a decision, since the select is sitting right there.
  // What it avoids is every task created here belonging to no phase and
  // appearing in none of the phase-scoped views (KEEL-244).
  const activeMilestone = useMemo(() => {
    const openTasks = new Map<string, number>();
    for (const task of data?.items ?? []) {
      const status = String(task.status);
      if (status === "done" || status === "wont_do") continue;
      const on = task.milestone_id ? String(task.milestone_id) : null;
      if (on) openTasks.set(on, (openTasks.get(on) ?? 0) + 1);
    }
    let best: string | undefined;
    let most = 0;
    for (const [id, count] of openTasks) {
      if (count > most) {
        most = count;
        best = id;
      }
    }
    return best;
  }, [data]);

  const [creating, setCreating] = useState(false);
  /** The task a drop on a terminal column is asking to close, if any. */
  const [closing, setClosing] = useState<Entity | null>(null);

  /**
   * A card dropped on a status column.
   *
   * `dropOnStatus` has already been consulted by the board to decide whether
   * the column would take the card at all, and is consulted again here rather
   * than trusted — the board is deciding what to *show*, and this is deciding
   * what to *write*. A refusal reaching this point would be a bug, and the
   * toast says so instead of writing something nobody asked for.
   */
  async function move(task: Entity, columnKey: string) {
    const drop = dropOnStatus(columnKey);
    if (drop.kind === "refused") {
      toast({ text: drop.why });
      return;
    }
    if (drop.kind === "close") {
      setClosing(task);
      return;
    }
    if (String(task.status) === drop.status) return;

    try {
      await api.updateTask(String(task.id), {
        version: Number(task.audit.version),
        status: drop.status,
      });
      reload();
      // A blocked card keeps its place, because the blocked column is derived
      // from the graph and comes first — so the write lands and the card does
      // not move, which reads as nothing having happened. Say what did.
      if (blockedIds.has(String(task.id))) {
        toast({
          text: `Moved to ${drop.status}. It stays under Blocked while something blocks it.`,
        });
      }
    } catch (e) {
      // The card snaps back on its own, because nothing was written and the
      // board still holds the old row — so the toast is the only thing that
      // says why, and a silent revert is the worst version of this.
      toast({
        text:
          e instanceof ApiError
            ? e.status === 409
              ? "That task changed while you were dragging it. Reloading."
              : e.message
            : "It was not moved.",
      });
      if (e instanceof ApiError && e.status === 409) reload();
    }
  }

  const filterKey = JSON.stringify(filter);
  const groups = useMemo(() => {
    const matching = applyFilter(
      data?.items ?? [],
      parseFilter(route.query),
      blockedIds,
      projectKey,
    );
    // A board with one column is not a board, so `none` degrades to status
    // there rather than being offered and then quietly ignored.
    const by = layout === "board" && group === "none" ? "status" : group;
    return groupTasks(matching, by, groupNames, blockedIds).map((g) => ({
      ...g,
      tasks: sortTasks(g.tasks, sort, dir, rank),
    }));
    // `filter` is rebuilt from the query on every render, so the memo compares
    // its serialised form rather than its identity.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    data,
    filterKey,
    blockedIds,
    group,
    layout,
    groupNames,
    sort,
    dir,
    rank,
    projectKey,
  ]);

  // Counted as distinct tasks, not as the sum of the groups: grouping by label
  // puts a task with three labels in three groups, and adding the columns up
  // would report more tasks than exist.
  const shown = useMemo(
    () => new Set(groups.flatMap((g) => g.tasks.map((t) => String(t.id)))).size,
    [groups],
  );

  const view: View = { filter, group, sort, dir, layout };

  if (loading && !data) return <Spinner />;
  if (error) {
    return (
      <Page title="Tasks" crumbs={projectCrumbs(route, "Board")}>
        <ErrorBox error={error} retry={reload} />
      </Page>
    );
  }

  return (
    <Page
      title="Tasks"
      crumbs={projectCrumbs(route, "Board")}
      width="full"
      meta={
        <span className="text-small text-ink-faint">
          {shown} of {data?.total ?? 0}
        </span>
      }
      actions={
        route.project ? (
          <Button size="sm" variant="primary" onClick={() => setCreating(true)}>
            New task
          </Button>
        ) : undefined
      }
      toolbar={
        <FilterBar
          view={view}
          facets={facets}
          milestoneNoun={milestoneNoun}
          inboxOn={inboxOn}
          total={(data?.items ?? []).length}
          onFilter={(next) =>
            setQuery(route, filterToQuery(next), { replace: true })
          }
          onView={(next) =>
            setQuery(
              route,
              {
                // A default is written as the absence of a parameter, so the
                // unfiltered board has a clean address rather than one trailing
                // four parameters that say "as usual".
                ...(next.group
                  ? { group: next.group === "status" ? undefined : next.group }
                  : {}),
                ...(next.sort
                  ? { sort: next.sort === "next" ? undefined : next.sort }
                  : {}),
                ...(next.dir
                  ? { dir: next.dir === "asc" ? undefined : next.dir }
                  : {}),
                ...(next.layout
                  ? { view: next.layout === "board" ? undefined : next.layout }
                  : {}),
              },
              { replace: true },
            )
          }
        />
      }
    >
      <div className="flex h-full flex-col p-6">
        {/* Shown whatever is filtered. The ranking answers "what should I do
            next", and that does not stop being the question because you
            narrowed the board to look at something else. */}
        {ready && ready.length > 0 && (
          <section className="mb-4 shrink-0 rounded-lg border border-accent/30 bg-accent/5 px-3 py-2.5">
            <h2 className="mb-1.5 flex items-baseline justify-between text-micro font-semibold tracking-wide text-accent uppercase">
              Next
              {/* The strip is the top three of the same ranking the full page
                  shows grouped, with counts and the rest of the queue. Until
                  now there was no way through to it from here. */}
              <a
                href={href({ screen: "next", project })}
                className="normal-case tracking-normal hover:underline"
              >
                What&rsquo;s next &rarr;
              </a>
            </h2>
            <ol className="space-y-1">
              {ready.slice(0, 3).map((item, i) => (
                <li key={item.id}>
                  {/* A real link, not a row that looked like one — KEEL-260.
                      Opens the same task the same way a card or the full
                      Ready list does, and is reachable by keyboard the way
                      any `<a>` is: Tab lands on it, Enter opens it. */}
                  <a
                    href={href({
                      screen: "task",
                      project,
                      taskId: item.reference || item.id,
                    })}
                    className="flex gap-2 rounded-control text-small hover:underline"
                  >
                    <span className="w-3 shrink-0 text-right tabular-nums text-ink-faint">
                      {i + 1}
                    </span>
                    <span className="min-w-0">
                      <span className="mr-1.5 font-mono text-micro text-ink-faint">
                        {item.reference}
                      </span>
                      {item.title}{" "}
                      <span className="text-ink-faint">— {item.why}</span>
                    </span>
                  </a>
                </li>
              ))}
            </ol>
          </section>
        )}

        {shown === 0 ? (
          <Empty
            message="No tasks match."
            hint={isFiltering(filter) ? "Clear a filter above." : undefined}
          />
        ) : layout === "list" ? (
          <TaskList
            groups={groups}
            project={project ?? ""}
            projectKey={projectKey}
            rank={rank}
            sort={sort}
            dir={dir}
            milestoneNames={milestoneNames}
            onFilterMilestone={(id) =>
              setQuery(route, filterToQuery({ ...filter, milestone: id }), {
                replace: true,
              })
            }
            onSort={(by) =>
              setQuery(
                route,
                {
                  sort: by === "next" ? undefined : by,
                  // Clicking the column already sorted by reverses it.
                  dir: sort === by && dir === "asc" ? "desc" : undefined,
                },
                { replace: true },
              )
            }
            showGroupHeadings={group !== "none"}
          />
        ) : (
          <TaskBoard
            groups={groups}
            project={project ?? ""}
            projectKey={projectKey}
            rank={rank}
            noteCounts={noteCounts}
            milestoneNames={milestoneNames}
            onFilterMilestone={(id) =>
              setQuery(route, filterToQuery({ ...filter, milestone: id }), {
                replace: true,
              })
            }
            // Only when the columns are statuses. Grouped by label a card
            // legitimately sits in three of them at once, and there would be
            // nothing sensible for a drop to mean.
            onDropOnStatus={
              group === "status" ? (task, key) => void move(task, key) : undefined
            }
          />
        )}
      </div>
      {/* Dropping on `done` or `wont_do` lands here rather than writing: the
          storage layer wants a reason, a message and evidence, so the drop
          opens the same form the task screen's Close button does. Cancelling
          leaves the card where it was, because nothing was written. */}
      {closing && (
        <CloseTaskDialog
          open
          task={closing}
          onClose={() => setClosing(null)}
          onDone={reload}
        />
      )}
      {route.project && (
        <NewTaskDialog
          open={creating}
          project={route.project}
          projectKey={projectKey}
          facets={facets}
          milestoneNoun={milestoneNoun}
          inboxOn={inboxOn}
          activeMilestone={activeMilestone}
          onClose={() => setCreating(false)}
          onCreated={reload}
        />
      )}
    </Page>
  );
}

/**
 * Making a task from the board.
 *
 * The one place a person is already looking at the work when they think of
 * something else that needs doing, and until now the answer was "go and tell
 * Claude". Capture, in the sense B-78 draws the line: a title and a sentence
 * about what is wanted, not the reasoning behind it.
 *
 * The summary is asked for rather than optional-by-omission, because a row that
 * is only a title is the kind that nobody can pick up later — and `specline_next`
 * ranks on what a task says about itself.
 */
function NewTaskDialog({
  open,
  project,
  projectKey,
  facets,
  milestoneNoun,
  inboxOn,
  activeMilestone,
  onClose,
  onCreated,
}: {
  open: boolean;
  project: string;
  /** The `KEEL` of `KEEL-42`, so the confirmation can name the new row. */
  projectKey: string | undefined;
  facets: Facets;
  /** The project's own word for a milestone — "Phase" here. */
  milestoneNoun: string | undefined;
  /** The phase in flight, which is what a new task almost always belongs to. */
  activeMilestone: string | undefined;
  /** Whether the feature-request lifecycle is switched on (KEEL-341). */
  inboxOn?: boolean;
  onClose: () => void;
  onCreated: () => void;
}) {
  const [title, setTitle] = useState("");
  const [summary, setSummary] = useState("");
  const [priority, setPriority] = useState("p2");
  const [kind, setKind] = useState("task");
  const [milestone, setMilestone] = useState("");
  const [labels, setLabels] = useState<string[]>([]);
  const [saving, setSaving] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);

  // Default to the phase in flight, and re-default each time it opens. A task
  // typed here almost always belongs to the work in front of you, and the
  // alternative is a row that appears in no phase-scoped view at all.
  useEffect(() => {
    if (open) setMilestone(activeMilestone ?? "");
  }, [open, activeMilestone]);

  async function submit() {
    if (saving || title.trim() === "") return;
    setSaving(true);
    setFailed(null);
    try {
      const created = await api.createTask({
        project,
        title: title.trim(),
        summary: summary.trim(),
        priority,
        kind,
        milestone,
        labels,
      });
      setTitle("");
      setSummary("");
      setLabels([]);
      onClose();
      onCreated();
      // The number the row was just given, which the create response has
      // carried all along and this dialog used to discard. It is the thing you
      // type back into a conversation, so saying it is most of the point.
      const reference = taskRef(projectKey, created);
      toast({
        text: `Created ${reference}`,
        href: href({ screen: "task", project, taskId: reference }),
        linkLabel: "Open",
      });
    } catch (e) {
      setFailed(
        e instanceof ApiError ? e.message : "The task was not created.",
      );
    } finally {
      setSaving(false);
    }
  }

  return (
    <Dialog open={open} onClose={onClose} label="New task">
      {/* No width of its own: `Dialog`'s panel is already `max-w-xl`, and a
          narrower child inside it left a strip of dead space down the right
          (KEEL-244). */}
      <div className="space-y-3 p-4">
        <h2 className="text-small font-semibold text-ink">New task</h2>

        <label className="block space-y-1">
          <span className="text-micro text-ink-muted">Title</span>
          <input
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            autoFocus
            placeholder="What needs doing"
            className="w-full rounded-md border border-border-subtle bg-surface px-3 py-2 text-small text-ink placeholder:text-ink-faint"
          />
        </label>

        <label className="block space-y-1">
          <span className="text-micro text-ink-muted">
            Summary{" "}
            <span className="text-ink-faint">
              — what it is and when it is done
            </span>
          </span>
          <textarea
            value={summary}
            onChange={(e) => setSummary(e.target.value)}
            rows={3}
            placeholder="One or two sentences somebody could pick this up from cold."
            className="w-full resize-y rounded-md border border-border-subtle bg-surface px-3 py-2 text-small text-ink placeholder:text-ink-faint"
          />
        </label>

        {/* Three selects on one row, which is what the row is for. Each is a
            default rather than a decision: the phase in flight, an ordinary
            task, middling priority. Somebody who types a title and hits Create
            gets all three without reading them. */}
        <div className="grid grid-cols-3 gap-3">
          <label className="block space-y-1">
            <span className="text-micro text-ink-muted">Priority</span>
            <select
              value={priority}
              onChange={(e) => setPriority(e.target.value)}
              className="w-full rounded-md border border-border-subtle bg-surface px-2 py-1.5 text-small text-ink"
            >
              <option value="p0">p0</option>
              <option value="p1">p1</option>
              <option value="p2">p2</option>
              <option value="p3">p3</option>
            </select>
          </label>

          <label className="block space-y-1">
            <span className="text-micro text-ink-muted">Kind</span>
            <select
              value={kind}
              onChange={(e) => setKind(e.target.value)}
              className="w-full rounded-md border border-border-subtle bg-surface px-2 py-1.5 text-small text-ink"
            >
              <option value="task">task</option>
              <option value="bug">bug</option>
              <option value="chore">chore</option>
              <option value="spike">spike</option>
              {/* An epic is created when somebody decides to build, so it
                  belongs in the same box as everything else — but only while
                  the lifecycle it belongs to is switched on (KEEL-341). */}
              {inboxOn !== false && <option value="feature">feature</option>}
            </select>
          </label>

          <label className="block space-y-1">
            <span className="text-micro text-ink-muted">
              {milestoneNoun ?? "Milestone"}
            </span>
            <select
              value={milestone}
              onChange={(e) => setMilestone(e.target.value)}
              className="w-full rounded-md border border-border-subtle bg-surface px-2 py-1.5 text-small text-ink"
            >
              <option value="">none</option>
              {facets.milestones.map((m) => (
                <option key={m.id} value={m.id}>
                  {m.name}
                </option>
              ))}
            </select>
          </label>
        </div>

        {/* Every label on this project, found by typing. The list is built
            from the tasks the board loaded, and that query is scoped by
            project — so these cannot be another project's labels. */}
        <LabelPicker
          available={facets.labels}
          chosen={labels}
          onChange={setLabels}
        />

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
            variant="primary"
            onClick={() => void submit()}
            disabled={saving || title.trim() === ""}
          >
            {saving ? "Creating…" : "Create task"}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}
