/**
 * The tasks screen: two layouts, and a view that lives in the address.
 *
 * The regression worth naming is the last one — the ranked "Next" panel used to
 * vanish the moment any filter was applied, so the best thing in the app was
 * only ever on screen when you did not need it.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { Route } from "../lib/router";

const TASKS = [
  {
    id: "tsk_1",
    type: "task",
    number: 1,
    title: "Routing and URLs",
    status: "done",
    priority: "p0",
    kind: "task",
    labels: ["desktop"],
    milestone_id: "mst_1",
    audit: { updated_at: "2026-08-01T00:00:00Z", version: 3 },
  },
  {
    id: "tsk_2",
    type: "task",
    number: 2,
    title: "Filters that compose",
    status: "todo",
    priority: "p1",
    kind: "task",
    labels: ["desktop", "phase6"],
    milestone_id: "mst_1",
    audit: { updated_at: "2026-08-09T00:00:00Z", version: 4 },
  },
  {
    id: "tsk_3",
    type: "task",
    number: 3,
    title: "Delete the file-edit hook",
    status: "todo",
    priority: "p0",
    kind: "bug",
    labels: ["plugin"],
    audit: { updated_at: "2026-08-05T00:00:00Z", version: 5 },
  },
];

/** Counted so the board's appetite is asserted rather than assumed. */
const called = { ready: 0, context: 0, notes: 0, noteCounts: 0 };

const updateTask = vi.fn(async () => TASKS[2]);

vi.mock("../lib/api", () => ({
  ApiError: class ApiError extends Error {},
  subscribe: () => () => {},
  api: {
    entities: async ({ type }: { type?: string }) => ({
      items:
        type === "milestone"
          ? [{ id: "mst_1", type: "milestone", name: "Phase 6" }]
          : TASKS,
      total: type === "milestone" ? 1 : TASKS.length,
      truncated: false,
    }),
    context: async () => {
      called.context += 1;
      return { project: null, next_up: null };
    },
    ready: async () => {
      called.ready += 1;
      return {
        ready: [
          { id: "tsk_3", reference: "KEEL-3", title: "Delete the file-edit hook", why: "p0" },
        ],
        total: 1,
        truncated: false,
        blocked: ["tsk_2"],
      };
    },
    notes: async () => {
      called.notes += 1;
      return { notes: [], total: 0 };
    },
    noteCounts: async () => {
      called.noteCounts += 1;
      return { counts: { tsk_2: 3 }, total: 3 };
    },
    updateTask,
    createTask: async (task: Record<string, unknown>) => {
      created.push(task);
      return {
        id: "tsk_42",
        type: "task",
        number: 42,
        title: task.title,
        audit: { created_by: "human" },
      };
    },
  },
}));

/** What the dialog sent, so the confirmation can be checked against it. */
const created: Array<Record<string, unknown>> = [];

const { BoardScreen } = await import("./Board");
const { Toaster } = await import("../components/ui");
const { api } = await import("../lib/api");

function at(query: Record<string, string>): Route {
  return { screen: "board", project: "specline", query };
}

async function show(query: Record<string, string> = {}) {
  render(<BoardScreen route={at(query)} generation={0} projectKey="KEEL" />);
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

beforeEach(() => {
  window.location.hash = "#/projects/specline/board";
  called.ready = 0;
  called.context = 0;
  called.notes = 0;
  called.noteCounts = 0;
  created.length = 0;
  updateTask.mockClear();
});
afterEach(cleanup);

describe("what the board asks the daemon for", () => {
  // KEEL-123. The board used to wait on the whole digest — 27 KB of project
  // briefing — for the ranking and the blocked set, and pull every note body in
  // the project to put a number on a card. Both are cheap calls now, and this is
  // what stops either creeping back: the shapes still work, so nothing else
  // would fail if they did.
  it("reads the ranking from /api/ready and never the digest", async () => {
    await show();
    expect(called.ready).toBe(1);
    expect(called.context).toBe(0);
  });

  it("asks for note counts, not note bodies", async () => {
    await show();
    expect(called.noteCounts).toBe(1);
    expect(called.notes).toBe(0);
  });

  // The blocked column is the reason `/api/ready` had to grow a `blocked`
  // parameter at all. If the ids stopped arriving the column would quietly
  // vanish and every card would look fine.
  it("still draws the blocked column from the ids /api/ready returns", async () => {
    await show();
    // The column heading, not the filter menu that shares the word.
    const headings = screen
      .getAllByText("blocked")
      .filter((el) => el.tagName === "SPAN" && el.className.includes("uppercase"));
    expect(headings).toHaveLength(1);
  });
});

describe("the filter, read from the address", () => {
  it("shows everything when the address carries no filter", async () => {
    await show();
    expect(screen.getByText("Routing and URLs")).toBeTruthy();
    expect(screen.getByText("Filters that compose")).toBeTruthy();
  });

  it("narrows to what the query names", async () => {
    await show({ status: "todo" });
    expect(screen.queryByText("Routing and URLs")).toBeNull();
    expect(screen.getByText("Filters that compose")).toBeTruthy();
  });

  it("combines facets with AND", async () => {
    await show({ status: "todo", priority: "p0" });
    expect(screen.queryByText("Filters that compose")).toBeNull();
    // Twice on screen — once in the Next panel, once as the one matching card.
    expect(screen.getAllByText("Delete the file-edit hook").length).toBeGreaterThan(0);
    expect(screen.getByText("1 of 3")).toBeTruthy();
  });

  it("counts distinct matches against the total", async () => {
    await show({ priority: "p0" });
    expect(screen.getByText("2 of 3")).toBeTruthy();
  });

  // Failure case: a filter matching nothing must say so rather than showing
  // everything, which is how a silently-dropped filter looks from the outside.
  it("says nothing matches rather than falling back to everything", async () => {
    await show({ status: "review" });
    expect(screen.getByText("No tasks match.")).toBeTruthy();
    expect(screen.getByText("Clear a filter above.")).toBeTruthy();
  });
});

describe("the ranked Next panel", () => {
  it("is there unfiltered", async () => {
    await show();
    expect(screen.getByText("Next")).toBeTruthy();
  });

  // The regression. Narrowing the board to look at something else does not
  // stop "what should I do next" being the question.
  it("is still there when a filter is applied", async () => {
    await show({ status: "todo", priority: "p1" });
    expect(screen.getByText("Next")).toBeTruthy();
    expect(screen.getByText("KEEL-3")).toBeTruthy();
  });

  // KEEL-260. Each row looked like every other clickable row on the screen and
  // was not one — a `<li>` with no `<a>` inside it, so nothing happened on
  // click and nothing was reachable by Tab.
  //
  // The card for the same task is also on the board underneath, so the row
  // is found inside the "Next" section specifically rather than by name
  // alone, which would match both.
  function nextRow(name: string | RegExp): HTMLElement {
    const heading = screen.getByText("Next");
    const section = heading.closest("section") as HTMLElement;
    return within(section).getByRole("link", { name });
  }

  it("opens the task when a row is clicked, the same as everywhere else", async () => {
    await show();
    const link = nextRow(/Delete the file-edit hook/);
    expect(link.getAttribute("href")).toBe("#/projects/specline/tasks/KEEL-3");
  });

  // Not a press of Enter — jsdom's focus is enough to prove the row is a real
  // `<a>` reachable by Tab, and a real `<a>` is what the browser opens on
  // Enter without this page doing anything more.
  it("is keyboard-reachable: a real link, not just something that looks clickable", async () => {
    await show();
    const link = nextRow(/Delete the file-edit hook/);
    link.focus();
    expect(document.activeElement).toBe(link);
  });
});

describe("the two layouts", () => {
  it("is a board by default", async () => {
    await show();
    expect(document.querySelector("table")).toBeNull();
  });

  it("is a table when the address says list", async () => {
    await show({ view: "list" });
    expect(document.querySelector("table")).toBeTruthy();
    expect(screen.getByRole("columnheader", { name: /Ref/ })).toBeTruthy();
  });

  it("puts a sort into the address when a column header is clicked", async () => {
    await show({ view: "list" });
    fireEvent.click(sortHeader("Priority"));
    expect(window.location.hash).toContain("sort=priority");
  });

  it("reverses when the column already sorted by is clicked again", async () => {
    await show({ view: "list", sort: "priority" });
    fireEvent.click(sortHeader("Priority"));
    expect(window.location.hash).toContain("dir=desc");
  });
});

/** The column header's own button — "Priority" also names a filter menu. */
function sortHeader(name: string): HTMLElement {
  return within(screen.getByRole("columnheader", { name: new RegExp(name) })).getByRole("button");
}

describe("grouping", () => {
  it("groups by status by default, keeping the empty columns", async () => {
    await show();
    expect(screen.getByText("in progress")).toBeTruthy();
    expect(screen.getByText("review")).toBeTruthy();
  });

  it("groups by milestone, and names it", async () => {
    await show({ group: "milestone" });
    // The column heading specifically. Since C7 every card also carries a
    // milestone chip, so a bare text match now finds the heading *and* the
    // chips under it — which is the feature working, not a collision.
    const headings = screen
      .getAllByText("Phase 6")
      .filter((el) => el.tagName === "SPAN" && el.className.includes("uppercase"));
    expect(headings).toHaveLength(1);
    expect(screen.getByText("no milestone")).toBeTruthy();
  });

  // C7. The single most-asked question about this project is "what is left in
  // Phase 8", and it used to mean opening every card.
  it("shows what each task is part of, and marks the ones nobody placed", async () => {
    await show({});
    const chips = screen.getAllByRole("button", { name: /Phase 6|unplaced/ });
    expect(chips.length).toBeGreaterThan(0);
    // An unassigned task is visibly unassigned rather than silently blank —
    // that gap usually means something was filed and never placed.
    expect(screen.getAllByRole("button", { name: "unplaced" }).length).toBeGreaterThan(0);
  });

  // Failure case: a board with one column is not a board. `none` is a list
  // option, and offering it on the board and then ignoring it silently would be
  // worse than degrading visibly to the default.
  it("falls back to status when the board is asked for no grouping", async () => {
    await show({ group: "none" });
    expect(screen.getByText("todo")).toBeTruthy();
    expect(screen.queryByText("All")).toBeNull();
  });

  it("honours no grouping in the list", async () => {
    await show({ view: "list", group: "none" });
    expect(screen.queryByText("todo", { selector: "th" })).toBeNull();
  });
});

describe("a hand-edited address", () => {
  // Degrade, do not break. Someone will type these by hand — that is the point
  // of a readable URL — and a typo must not produce an error screen.
  it("falls back to the defaults for values it does not recognise", async () => {
    await show({ group: "byMood", sort: "vibes", dir: "sideways", view: "hologram" });
    expect(document.querySelector("table")).toBeNull();
    expect(screen.getByText("todo")).toBeTruthy();
    expect(screen.getByText("Routing and URLs")).toBeTruthy();
  });
});

describe("making a task from the board", () => {
  /** The board, plus the shell's toast host, which is where a confirmation goes. */
  async function withToasts(query: Record<string, string> = {}) {
    render(
      <>
        <BoardScreen route={at(query)} generation={0} projectKey="KEEL" />
        <Toaster />
      </>,
    );
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
  }

  async function createOne(title: string) {
    fireEvent.click(screen.getByText("New task"));
    fireEvent.change(screen.getByPlaceholderText("What needs doing"), {
      target: { value: title },
    });
    await act(async () => {
      fireEvent.click(screen.getByText("Create task"));
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
  }

  // The whole of KEEL-285. The number came back on the create response all
  // along and the dialog dropped it, so the one thing you would type back into
  // a conversation was the one thing the app never told you.
  it("says which row was created", async () => {
    await withToasts();
    await createOne("Something I thought of");

    expect(created).toHaveLength(1);
    expect(screen.getByText("Created KEEL-42")).toBeTruthy();
  });

  it("links to the row it just named", async () => {
    await withToasts();
    await createOne("Something I thought of");

    expect(screen.getByText("Open").getAttribute("href")).toBe(
      "#/projects/specline/tasks/KEEL-42",
    );
  });

  // Failure case: nothing was created, so nothing may claim it was. A
  // confirmation that fires on a failed write is worse than none — it is the
  // reason somebody stops checking.
  it("says nothing when the create fails", async () => {
    const failing = vi
      .spyOn(api, "createTask")
      .mockRejectedValueOnce(new Error("The daemon is not reachable."));

    await withToasts();
    await createOne("Something I thought of");

    expect(screen.queryByText(/^Created /)).toBeNull();
    failing.mockRestore();
  });
});

/**
 * Dragging a card between columns — the gesture everyone tries first, and the
 * three columns where trying it should not quietly do nothing.
 */
describe("dragging a card to another column", () => {
  /** jsdom has no DataTransfer, and the handler writes to it. */
  const dataTransfer = () => ({ setData: vi.fn(), effectAllowed: "" });

  function cardFor(title: string): HTMLElement {
    // The draggable element is the card wrapper, not the title's anchor.
    const link = screen.getByRole("link", { name: title });
    const card = link.closest("[draggable]");
    if (!card) throw new Error(`no draggable card for ${title}`);
    return card as HTMLElement;
  }

  function columnFor(label: string): HTMLElement {
    const heading = screen.getByText(label, { selector: "span" });
    const column = heading.closest("div")?.parentElement;
    if (!column) throw new Error(`no column for ${label}`);
    return column;
  }

  it("moves a card into a column that owes nothing", async () => {
    await show();

    const card = cardFor("Delete the file-edit hook");
    fireEvent.dragStart(card, { dataTransfer: dataTransfer() });
    const review = columnFor("review");
    fireEvent.dragOver(review, { dataTransfer: dataTransfer() });
    fireEvent.drop(review, { dataTransfer: dataTransfer() });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    expect(updateTask).toHaveBeenCalledWith("tsk_3", {
      version: 5,
      status: "review",
    });
  });

  /**
   * A claim records which session is on the work, and a person dragging has
   * none. The refusal is shown while the card is still in the air, because a
   * drop target that silently does nothing reads as a broken app.
   */
  it("refuses in_progress, says why while dragging, and writes nothing", async () => {
    await show();

    expect(screen.queryByText(/Starting work is a claim/)).toBeNull();

    const card = cardFor("Delete the file-edit hook");
    fireEvent.dragStart(card, { dataTransfer: dataTransfer() });
    expect(screen.getByText(/Starting work is a claim/)).toBeTruthy();

    const inProgress = columnFor("in progress");
    fireEvent.dragOver(inProgress, { dataTransfer: dataTransfer() });
    fireEvent.drop(inProgress, { dataTransfer: dataTransfer() });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    expect(updateTask).not.toHaveBeenCalled();

    // And the explanation goes away when the drag does.
    fireEvent.dragEnd(card);
    expect(screen.queryByText(/Starting work is a claim/)).toBeNull();
  });

  /** Blocked is derived from the graph, so there is nothing a drop could set. */
  it("refuses the blocked column, and says why", async () => {
    await show();

    const card = cardFor("Delete the file-edit hook");
    fireEvent.dragStart(card, { dataTransfer: dataTransfer() });

    expect(screen.getByText(/Blocked is not a status/)).toBeTruthy();
    const blocked = columnFor("blocked");
    fireEvent.drop(blocked, { dataTransfer: dataTransfer() });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    expect(updateTask).not.toHaveBeenCalled();
  });

  /**
   * Closing owes a reason, a message and evidence. The drop opens the form
   * that collects them rather than writing a status and failing behind it.
   */
  it("opens the close form when a card is dropped on done", async () => {
    await show();

    const card = cardFor("Delete the file-edit hook");
    fireEvent.dragStart(card, { dataTransfer: dataTransfer() });
    fireEvent.drop(columnFor("done"), { dataTransfer: dataTransfer() });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    expect(updateTask).not.toHaveBeenCalled();
    expect(screen.getByText("Close this task")).toBeTruthy();
  });

  /**
   * Grouped by label a card legitimately sits in three columns at once, so
   * there is nothing a drop could sensibly mean.
   */
  it("does not make cards draggable when the columns are not statuses", async () => {
    await show({ group: "label" });

    const link = screen.getAllByRole("link", { name: "Delete the file-edit hook" })[0];
    expect(link?.closest("[draggable='true']")).toBeNull();
  });
});

/**
 * The blocked column is derived from the graph and comes first, so a blocked
 * card that is moved keeps its place. The write lands and nothing appears to
 * happen, which is the one case where the board has to say what it did.
 */
describe("moving a card that is blocked", () => {
  const dataTransfer = () => ({ setData: vi.fn(), effectAllowed: "" });

  it("says the card stays under Blocked, rather than looking inert", async () => {
    render(<Toaster />);
    await show();

    // tsk_2 is the one `ready` reports as blocked.
    const link = screen.getByRole("link", { name: "Filters that compose" });
    const card = link.closest("[draggable]") as HTMLElement;
    fireEvent.dragStart(card, { dataTransfer: dataTransfer() });

    const heading = screen.getByText("review", { selector: "span" });
    const column = heading.closest("div")?.parentElement as HTMLElement;
    fireEvent.drop(column, { dataTransfer: dataTransfer() });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    expect(updateTask).toHaveBeenCalledWith("tsk_2", {
      version: 4,
      status: "review",
    });
    expect(screen.getByText(/stays under Blocked/)).toBeTruthy();
  });
});

/**
 * A card must not say the same word twice.
 *
 * The kind is a badge and so is every label, and nothing noticed when they
 * agreed — a bug carrying a `bug` label rendered "p2 · bug · bug", which reads
 * as broken rather than as two fields saying the same thing (KEEL-313).
 */
describe("a label that repeats the kind", () => {
  const target = TASKS[2]!;
  const kind = target.kind;
  const labels = target.labels;
  afterEach(() => {
    target.kind = kind;
    target.labels = labels;
  });

  it("is not drawn a second time beside the kind badge", async () => {
    target.kind = "bug";
    target.labels = ["bug", "plugin"];
    await show();

    const card = screen
      .getByRole("link", { name: "Delete the file-edit hook" })
      .closest("[draggable]") as HTMLElement;
    const badges = [...card.querySelectorAll("span")]
      .map((s) => s.textContent?.trim())
      .filter((t) => t === "bug");
    expect(badges).toHaveLength(1);
    // And the label that says something the kind does not still shows.
    expect(card.textContent).toContain("plugin");
  });

  /**
   * With kind `task` no kind badge is drawn, so a `task` label is the only
   * thing saying it and must survive.
   */
  it("still shows a label that duplicates nothing on screen", async () => {
    target.kind = "task";
    target.labels = ["task"];
    await show();

    const card = screen
      .getByRole("link", { name: "Delete the file-edit hook" })
      .closest("[draggable]") as HTMLElement;
    expect(card.textContent).toContain("task");
  });
});
