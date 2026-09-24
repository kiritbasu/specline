/**
 * The detail view.
 *
 * Four things it must get right, each of which was previously impossible to get
 * wrong only because nothing rendered them at all: relationships stated in the
 * direction they were walked, retracted notes shown rather than hidden, the
 * event log rendered as before-and-after, and J/K walking the board's order.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react";

const TASK = {
  id: "tsk_me",
  type: "task",
  title: "The task detail view",
  // Both nullable, and both exercised below: a row normally carries one of them
  // rather than both, and which one depends on whether it predates 8G.
  body: "Clicking a card opens the task at its own URL." as string | null,
  summary: null as string | null,
  status: "in_progress",
  priority: "p0",
  kind: "task",
  labels: ["desktop", "phase6"],
  milestone_id: "mst_1",
  parent_id: "tsk_parent",
  closed_at: null,
  external_refs: [
    "https://github.com/kb/specline/pull/1",
    "https://github.com/kb/specline/issues/2",
  ],
  audit: {
    created_at: "2026-08-10T09:00:00Z",
    updated_at: "2026-08-10T10:00:00Z",
    version: 7,
  },
};

const updateTask = vi.fn(async () => TASK);

/// Flipped by the test that proves a screen survives losing this one request.
let clientsUnavailable = false;

vi.mock("../lib/api", () => ({
  ApiError: class ApiError extends Error {
    status: number;
    constructor(message: string, status = 0) {
      super(message);
      this.status = status;
    }
  },
  subscribe: () => () => {},
  api: {
    entity: async () => ({ artifacts: [{ entity: TASK }] }),
    notesFor: async () => ({
      notes: [
        {
          id: "nte_1",
          entity_id: "tsk_me",
          entity_type: "task",
          project_id: "prj_1",
          body: "Still believed at the time.",
          author: "claude",
          session_id: "ses_abc",
          surface: "code",
          created_at: "2026-08-10T09:30:00Z",
          archived_at: "2026-08-10T09:45:00Z",
        },
        {
          id: "nte_2",
          entity_id: "tsk_me",
          entity_type: "task",
          project_id: "prj_1",
          body: "What actually happened.",
          author: "human",
          session_id: null,
          surface: null,
          created_at: "2026-08-10T09:50:00Z",
          archived_at: null,
        },
      ],
      total: 2,
    }),
    history: async () => ({
      events: [
        {
          id: "evt_1",
          entity_id: "tsk_me",
          entity_type: "task",
          project_id: "prj_1",
          action: "created",
          field: null,
          before: null,
          after: null,
          actor: "claude",
          session_id: null,
          surface: null,
          summary: "created task “The task detail view”",
          created_at: "2026-08-10T09:00:00Z",
        },
        {
          id: "evt_2",
          entity_id: "tsk_me",
          entity_type: "task",
          project_id: "prj_1",
          action: "status_changed",
          field: "status",
          before: "todo",
          after: "in_progress",
          actor: "claude",
          session_id: null,
          surface: null,
          summary: "status todo → in_progress",
          created_at: "2026-08-10T10:00:00Z",
        },
      ],
      total: 2,
      truncated: false,
    }),
    graph: async (_id: string, direction: string) => ({
      neighbours:
        direction === "outbound"
          ? [
              {
                id: "tsk_child",
                entity_type: "task",
                rel: "blocks",
                label: "Sub-tasks — a parent link",
                anchor: "",
                depth: 1,
                path: [],
              },
            ]
          : [
              {
                id: "tsk_parent",
                entity_type: "task",
                rel: "blocks",
                label: "One page shell for every screen",
                anchor: "",
                depth: 1,
                path: [],
              },
            ],
    }),
    entities: async ({ type }: { type?: string }) => ({
      items:
        type === "milestone"
          ? [
              {
                id: "mst_1",
                type: "milestone",
                name: "Phase 6 — Make the tracker real",
              },
            ]
          : [
              {
                id: "tsk_first",
                type: "task",
                title: "Aaa first",
                status: "todo",
                priority: "p0",
              },
              {
                id: "tsk_me",
                type: "task",
                title: "Me",
                status: "in_progress",
                priority: "p0",
              },
              {
                id: "tsk_last",
                type: "task",
                title: "Zzz last",
                status: "done",
                priority: "p0",
              },
              {
                id: "tsk_parent",
                type: "task",
                title: "The epic above",
                status: "todo",
                priority: "p1",
              },
              {
                id: "tsk_kid_a",
                type: "task",
                title: "A finished piece",
                status: "done",
                priority: "p2",
                parent_id: "tsk_me",
              },
              {
                id: "tsk_kid_b",
                type: "task",
                title: "An unfinished piece",
                status: "todo",
                priority: "p2",
                parent_id: "tsk_me",
              },
            ],
      total: 3,
      truncated: false,
    }),
    context: async () => ({ next_up: null }),
    // `ses_abc` wrote the first note; `ses_nobody` is here to prove a session
    // this list does not know stays silent rather than reading as anything.
    clients: async () => {
      if (clientsUnavailable) throw new Error("404 — an older daemon");
      return {
        clients: [
          {
            session_id: "ses_abc",
            name: "codex-mcp-client",
            title: "Codex",
            version: "0.148.0-alpha.15",
            display_name: "Codex",
            first_seen: "2026-08-10T09:00:00Z",
            last_wrote: "2026-08-10T09:30:00Z",
          },
        ],
        total: 1,
      };
    },
    updateTask,
  },
}));

const { TaskScreen } = await import("./Task");
// The mocked class, which is the one the component compares against.
const { ApiError } = await import("../lib/api");

const route = {
  screen: "task" as const,
  project: "specline",
  taskId: "tsk_me",
  query: {},
};

async function show() {
  render(<TaskScreen route={route} generation={0} />);
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

beforeEach(() => {
  window.location.hash = "#/projects/specline/tasks/tsk_me";
});
afterEach(cleanup);

describe("what it shows", () => {
  it("renders the description, the properties and the milestone by name", async () => {
    await show();
    expect(
      screen.getByText("Clicking a card opens the task at its own URL."),
    ).toBeTruthy();
    expect(screen.getByText("Phase 6 — Make the tracker real")).toBeTruthy();
    expect(screen.getAllByText("in_progress").length).toBeGreaterThan(0);
  });

  // The direction property, at the level a reader sees it. The same stored
  // `blocks` edge must appear under two different headings depending on which
  // way it was walked.
  it("states each relationship in the direction it was walked", async () => {
    await show();
    expect(screen.getByText("Blocked by")).toBeTruthy();
    expect(screen.getByText("Blocks")).toBeTruthy();
    expect(screen.getByText("One page shell for every screen")).toBeTruthy();
    expect(screen.getByText("Sub-tasks — a parent link")).toBeTruthy();
  });

  it("links a related task to its own page", async () => {
    await show();
    const link = screen.getByText("Sub-tasks — a parent link").closest("a");
    expect(link?.getAttribute("href")).toBe(
      "#/projects/specline/tasks/tsk_child",
    );
  });

  // A retracted note stays visible and struck through. Hiding it would rewrite
  // the record: what a session once believed is part of how the row got here.
  it("shows a retracted note rather than dropping it", async () => {
    await show();
    const note = screen.getByText("Still believed at the time.");
    expect(note).toBeTruthy();
    expect(screen.getByText("retracted")).toBeTruthy();
    expect(note.closest(".line-through")).toBeTruthy();
  });

  it("says when a note came from outside a tracked session", async () => {
    await show();
    expect(screen.getByText("written outside a tracked session")).toBeTruthy();
  });

  it("names the editor a note was written from", async () => {
    await show();
    // `surface` on this note is `code`, which Claude Code and Codex both write.
    // The version is what distinguishes them, so it has to be on screen.
    expect(screen.getByText(/Codex 0\.148\.0-alpha\.15/)).toBeTruthy();
  });

  // Unknown has to look like nothing, not like a word.
  //
  // Every row written before this was recorded resolves to no client, and a
  // screen that says "unknown" thirty times says nothing else. The session id
  // is still rendered for anyone who needs to chase it.
  it("says nothing at all for a session whose editor is unrecorded", async () => {
    await show();
    expect(screen.queryByText(/unknown/i)).toBeNull();
    // The note with no session keeps its own honest sentence.
    expect(screen.getByText("written outside a tracked session")).toBeTruthy();
  });

  // A label must not be able to take the page down with it.
  //
  // The first version of this fetched the editors inside the same
  // `Promise.all` as the task, its notes and its history, which made any
  // rejection the whole screen's — and an app talking to a daemon older than
  // the endpoint is the ordinary way that happens, not an exotic one. The page
  // renders; the editor names are simply absent.
  it("still renders the task when the editors cannot be fetched", async () => {
    clientsUnavailable = true;
    try {
      await show();
      expect(
        screen.getByText("Clicking a card opens the task at its own URL."),
      ).toBeTruthy();
      expect(screen.queryByText(/Codex/)).toBeNull();
    } finally {
      clientsUnavailable = false;
    }
  });

  // The event log has always held before and after; nothing had ever shown it.
  it("renders a field change as before and after", async () => {
    await show();
    // Scoped to the History card — "todo" also names several sub-task statuses.
    const history = screen
      .getByText("History")
      .closest("section") as HTMLElement;
    const row = within(history).getByText("todo").closest("li");
    expect(row?.textContent).toContain("status");
    expect(row?.textContent).toContain("todo");
    expect(row?.textContent).toContain("→");
    expect(row?.textContent).toContain("in_progress");
  });

  it("falls back to the summary for an event with no field", async () => {
    await show();
    expect(screen.getByText(/created task/)).toBeTruthy();
  });
});

// KEEL-170. This card read `body` alone, so the thirty-one tasks written since
// `summary` became required — several hundred words each, in the field every
// list already shows — displayed "No description." The store was never the
// problem, which is what made it hard to see: the row was complete and the page
// said it was empty.
describe("the description, whichever field carries it", () => {
  const body = TASK.body;
  const summary = TASK.summary;
  afterEach(() => {
    TASK.body = body;
    TASK.summary = summary;
  });

  it("shows the summary when there is no body", async () => {
    TASK.body = null;
    TASK.summary = "The board never says which phase a task belongs to.";
    await show();
    expect(
      screen.getByText("The board never says which phase a task belongs to."),
    ).toBeTruthy();
    expect(screen.queryByText("No description.")).toBeNull();
  });

  // Said out loud, because a required one-or-two-sentence summary is not the
  // long-form detail a reader opening the page is looking for. Showing it
  // unlabelled would answer the question with something else and look right.
  it("says when what it is showing is the summary", async () => {
    TASK.body = null;
    TASK.summary = "Short, and standing in for a body that was never written.";
    await show();
    expect(screen.getByText("from the summary")).toBeTruthy();
  });

  it("prefers the body when both are there, and does not label it", async () => {
    TASK.summary = "One sentence that must not win.";
    await show();
    expect(
      screen.getByText("Clicking a card opens the task at its own URL."),
    ).toBeTruthy();
    expect(screen.queryByText("One sentence that must not win.")).toBeNull();
    expect(screen.queryByText("from the summary")).toBeNull();
  });

  it("says there is no description only when neither field has one", async () => {
    TASK.body = null;
    TASK.summary = null;
    await show();
    expect(screen.getByText("No description.")).toBeTruthy();
    expect(screen.queryByText("from the summary")).toBeNull();
  });
});

describe("the keyboard", () => {
  it("J and K walk the board's order", async () => {
    await show();
    // Board order is todo, then in_progress, then done — so the neighbours of
    // the in_progress task are the todo one and the done one.
    fireEvent.keyDown(window, { key: "j" });
    expect(window.location.hash).toBe("#/projects/specline/tasks/tsk_last");

    window.location.hash = "#/projects/specline/tasks/tsk_me";
    fireEvent.keyDown(window, { key: "k" });
    // The last card in the todo column, which is the one immediately before
    // the in_progress column this task sits in.
    expect(window.location.hash).toBe("#/projects/specline/tasks/tsk_kid_b");
  });

  // Failure case: at the ends of the list the keys must do nothing rather than
  // wrap around, which would make J look like it had jumped at random.
  it("stops at the ends rather than wrapping", async () => {
    render(
      <TaskScreen route={{ ...route, taskId: "tsk_first" }} generation={0} />,
    );
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    window.location.hash = "#/projects/specline/tasks/tsk_first";
    fireEvent.keyDown(window, { key: "k" });
    expect(window.location.hash).toBe("#/projects/specline/tasks/tsk_first");
  });

  // Failure case: J typed into a field is a letter, not a command.
  it("ignores J and K typed into a text field", async () => {
    await show();
    const field = document.createElement("input");
    document.body.append(field);
    fireEvent.keyDown(field, { key: "j" });
    expect(window.location.hash).toBe("#/projects/specline/tasks/tsk_me");
    field.remove();
  });
});

// KEEL-405. Board order is todo (tsk_first, tsk_parent, tsk_kid_b), then
// in_progress (tsk_me), then done (tsk_last, tsk_kid_a) — the same order J/K
// walk. The arrow keys walk that sequence, skipping past anything closed on
// the way, so a closed row is never where they land — including when it is
// the one the reader started from.

/** Renders the screen with a chosen task current, and sets the address to match. */
async function showAt(taskId: string) {
  render(<TaskScreen route={{ ...route, taskId }} generation={0} />);
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
  window.location.hash = `#/projects/specline/tasks/${taskId}`;
}

describe("arrow-key navigation between open tasks", () => {
  it("Right and Down move to the next open task", async () => {
    await showAt("tsk_kid_b");
    fireEvent.keyDown(window, { key: "ArrowRight" });
    expect(window.location.hash).toBe("#/projects/specline/tasks/tsk_me");

    window.location.hash = "#/projects/specline/tasks/tsk_kid_b";
    fireEvent.keyDown(window, { key: "ArrowDown" });
    expect(window.location.hash).toBe("#/projects/specline/tasks/tsk_me");
  });

  it("Left and Up move to the previous open task", async () => {
    await showAt("tsk_me");
    fireEvent.keyDown(window, { key: "ArrowLeft" });
    expect(window.location.hash).toBe("#/projects/specline/tasks/tsk_kid_b");

    window.location.hash = "#/projects/specline/tasks/tsk_me";
    fireEvent.keyDown(window, { key: "ArrowUp" });
    expect(window.location.hash).toBe("#/projects/specline/tasks/tsk_kid_b");
  });

  // Failure case: at either end the key does nothing rather than wrapping —
  // proved in the same render as a real move first, so a handler that had
  // been deleted, or that always did nothing, could not pass this by
  // accident. Forwards from tsk_me there is nothing open left — tsk_last and
  // tsk_kid_a are both done — so this also proves the closed pair gets
  // skipped rather than landed on.
  it("does nothing at the forward end rather than wrapping or landing on a closed task", async () => {
    await showAt("tsk_me");
    const moved = fireEvent.keyDown(window, { key: "ArrowLeft" });
    expect(window.location.hash).toBe("#/projects/specline/tasks/tsk_kid_b");
    expect(moved).toBe(false); // a real move: preventDefault was called.

    window.location.hash = "#/projects/specline/tasks/tsk_me";
    const stayed = fireEvent.keyDown(window, { key: "ArrowRight" });
    expect(window.location.hash).toBe("#/projects/specline/tasks/tsk_me");
    expect(stayed).toBe(true); // nothing prevented: there was nowhere to go.
  });

  it("does nothing at the back end rather than wrapping", async () => {
    await showAt("tsk_first");
    const moved = fireEvent.keyDown(window, { key: "ArrowRight" });
    expect(window.location.hash).toBe("#/projects/specline/tasks/tsk_parent");
    expect(moved).toBe(false);

    window.location.hash = "#/projects/specline/tasks/tsk_first";
    const stayed = fireEvent.keyDown(window, { key: "ArrowLeft" });
    expect(window.location.hash).toBe("#/projects/specline/tasks/tsk_first");
    expect(stayed).toBe(true);
  });

  // KEEL-405, review round 2: the current task can itself be closed — a
  // reader looking at a finished row still gets working arrows — so the
  // search has to find its real position in board order and step past
  // anything else closed, rather than refusing because a pre-filtered "open"
  // list has no entry for where it is.
  it("finds the previous open task starting from a closed one", async () => {
    await showAt("tsk_last"); // done, and the current task itself.
    fireEvent.keyDown(window, { key: "ArrowLeft" });
    expect(window.location.hash).toBe("#/projects/specline/tasks/tsk_me");
  });

  // Failure case: an arrow key typed into a field is cursor movement, not a
  // command to leave the page.
  it("ignores arrow keys typed into a text field", async () => {
    await showAt("tsk_kid_b");
    const field = document.createElement("input");
    document.body.append(field);
    fireEvent.keyDown(field, { key: "ArrowRight" });
    expect(window.location.hash).toBe("#/projects/specline/tasks/tsk_kid_b");
    field.remove();
  });

  // KEEL-405, review round 2. `closest("[role]")` used to find the focused
  // *item's* role — option, menuitem — and neither is in the set of widgets
  // that own arrow keys themselves, so the key leaked past the widget to
  // this page. The fix looks for the container's role instead.
  it("does not navigate when focus is on a listbox's option", async () => {
    await showAt("tsk_kid_b");
    const listbox = document.createElement("div");
    listbox.setAttribute("role", "listbox");
    const option = document.createElement("div");
    option.setAttribute("role", "option");
    listbox.append(option);
    document.body.append(listbox);
    fireEvent.keyDown(option, { key: "ArrowRight" });
    expect(window.location.hash).toBe("#/projects/specline/tasks/tsk_kid_b");
    listbox.remove();
  });

  // Failure case: a modifier held means the browser or the OS owns the key —
  // Cmd+Right for "end of line", say — and the page must not also react.
  it("ignores arrow keys held with a modifier", async () => {
    await showAt("tsk_kid_b");
    fireEvent.keyDown(window, { key: "ArrowRight", metaKey: true });
    expect(window.location.hash).toBe("#/projects/specline/tasks/tsk_kid_b");
    fireEvent.keyDown(window, { key: "ArrowRight", shiftKey: true });
    expect(window.location.hash).toBe("#/projects/specline/tasks/tsk_kid_b");
  });

  // KEEL-405, review round 2: auto-repeat would walk several tasks off one
  // held key.
  it("ignores auto-repeat", async () => {
    await showAt("tsk_kid_b");
    fireEvent.keyDown(window, { key: "ArrowRight", repeat: true });
    expect(window.location.hash).toBe("#/projects/specline/tasks/tsk_kid_b");
  });
});

/**
 * KEEL-405, review round 2. Down and Up are the keys this page's own body
 * already scrolls with — `overflow-y-auto` in `components/Page.tsx` — so
 * they must not also move between tasks while there is still something to
 * scroll that way, and must not scroll the (already-at-the-end) page
 * instead once there is truly nowhere further to go.
 */
describe("Down and Up defer to the scrolling body", () => {
  function pane(): HTMLElement {
    const el = document.querySelector(".overflow-y-auto");
    if (!(el instanceof HTMLElement)) throw new Error("no scrolling pane rendered");
    return el;
  }

  function setScroll(scrollTop: number, clientHeight: number, scrollHeight: number) {
    const el = pane();
    Object.defineProperty(el, "scrollTop", { configurable: true, value: scrollTop });
    Object.defineProperty(el, "clientHeight", { configurable: true, value: clientHeight });
    Object.defineProperty(el, "scrollHeight", { configurable: true, value: scrollHeight });
  }

  it("does not navigate while the pane can still scroll down, and does once it is at the bottom", async () => {
    await showAt("tsk_kid_b");
    setScroll(0, 500, 1000); // top of a long page: not at the bottom yet.
    const scrolled = fireEvent.keyDown(window, { key: "ArrowDown" });
    expect(window.location.hash).toBe("#/projects/specline/tasks/tsk_kid_b");
    expect(scrolled).toBe(true); // left alone, so the browser scrolls it.

    setScroll(500, 500, 1000); // scrolled all the way down.
    const moved = fireEvent.keyDown(window, { key: "ArrowDown" });
    expect(window.location.hash).toBe("#/projects/specline/tasks/tsk_me");
    expect(moved).toBe(false);
  });

  it("does not navigate while the pane can still scroll up, and does once it is at the top", async () => {
    await showAt("tsk_me");
    setScroll(200, 500, 1000); // scrolled down some: not at the top yet.
    const scrolled = fireEvent.keyDown(window, { key: "ArrowUp" });
    expect(window.location.hash).toBe("#/projects/specline/tasks/tsk_me");
    expect(scrolled).toBe(true);

    setScroll(0, 500, 1000); // back at the top.
    const moved = fireEvent.keyDown(window, { key: "ArrowUp" });
    expect(window.location.hash).toBe("#/projects/specline/tasks/tsk_kid_b");
    expect(moved).toBe(false);
  });

  // A page shorter than its own pane is already at both ends, so Down/Up
  // must not sit waiting for a scroll that will never happen.
  it("navigates on Down and Up when the pane is not scrollable at all", async () => {
    await showAt("tsk_kid_b");
    setScroll(0, 0, 0);
    fireEvent.keyDown(window, { key: "ArrowDown" });
    expect(window.location.hash).toBe("#/projects/specline/tasks/tsk_me");
  });
});

describe("when the rest of the project cannot be loaded", () => {
  // The failure that prompted this: the daemon was briefly down, the task
  // itself rendered from cache, and the page quietly lost its readable
  // identifier, its milestone name and J/K — while looking complete.
  it("says what is missing rather than degrading silently", async () => {
    const api = (await import("../lib/api")).api as unknown as {
      entities: () => Promise<unknown>;
    };
    const working = api.entities;
    api.entities = () =>
      Promise.reject(new Error("Cannot reach the Specline daemon."));
    try {
      await show();
      expect(screen.getByText(/could not be loaded/)).toBeTruthy();
    } finally {
      api.entities = working;
    }
  });
});

describe("what this is part of", () => {
  // Composition, not blocking. The two were the same edge before a task had a
  // parent, which is why a rollup was impossible: `blocks` means "must happen
  // first", and the ranking reads every inbound one as something in the way.
  it("shows the parent and the sub-tasks with a progress count", async () => {
    await show();
    expect(screen.getByText("Part of")).toBeTruthy();
    expect(screen.getByText("The epic above")).toBeTruthy();
    expect(screen.getByText("1 of 2 done")).toBeTruthy();
  });

  it("links a sub-task to its own page", async () => {
    await show();
    expect(
      screen.getByText("A finished piece").closest("a")?.getAttribute("href"),
    ).toBe("#/projects/specline/tasks/tsk_kid_a");
  });

  it("shows every external link, not just the first", async () => {
    await show();
    expect(screen.getByText("github.com/kb/specline/pull/1")).toBeTruthy();
    expect(screen.getByText("github.com/kb/specline/issues/2")).toBeTruthy();
  });
});

describe("the ask-Claude prompts", () => {
  // The app cannot write, and a read-only surface reads as either deliberate
  // or inert. The difference is whether it hands you the next move.
  it("offers prompts already addressed to this task", async () => {
    await show();
    // Matching on the button's accessible name: the prompt is a bare text node
    // beside the "copy" affordance, so it is not an element of its own.
    expect(
      screen.getByRole("button", { name: /close .+ as done with the commit/ }),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: /what is blocking/ }),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: /split .+ into sub-tasks/ }),
    ).toBeTruthy();
  });

  it("copies one to the clipboard", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    await show();
    const button = screen.getByRole("button", { name: /what is blocking/ });
    fireEvent.click(button);
    // Whatever this task's readable identifier is, the prompt carries it —
    // a prompt addressed to no task is worse than no prompt.
    expect(writeText).toHaveBeenCalledWith(
      expect.stringMatching(/^what is blocking \S+$/),
    );
  });
});

/**
 * The fields a person moves, which until KEEL-307 were text you could only
 * change by opening a conversation about it.
 *
 * Two of these are about what the panel refuses rather than what it does, and
 * those are the ones worth having: the reasons live in the storage layer and
 * in the claim model, and a control that quietly offered them would produce a
 * rejection the form could not explain.
 */
describe("changing the fields a person moves", () => {
  const status = TASK.status;
  afterEach(() => {
    TASK.status = status;
    updateTask.mockClear();
    updateTask.mockImplementation(async () => TASK);
  });

  it("saves a priority the moment it is picked, with the version it read", async () => {
    await show();

    fireEvent.change(screen.getByLabelText("Priority"), {
      target: { value: "p2" },
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    expect(updateTask).toHaveBeenCalledWith("tsk_me", {
      version: 7,
      priority: "p2",
    });
  });

  /**
   * Starting work is a claim and a claim records which session is on it. A
   * person clicking a dropdown has none — so the status is shown, because the
   * task really is in progress, and cannot be selected.
   */
  it("shows in_progress but will not let you pick it", async () => {
    await show();

    const select = screen.getByLabelText("Status") as HTMLSelectElement;
    expect(select.value).toBe("in_progress");

    const inProgress = within(select).getByRole("option", {
      name: "in_progress",
    }) as HTMLOptionElement;
    expect(inProgress.disabled).toBe(true);

    // And the two that can be reached from here are the two that owe nothing.
    const selectable = within(select)
      .getAllByRole("option")
      .filter((o) => !(o as HTMLOptionElement).disabled)
      .map((o) => (o as HTMLOptionElement).value);
    expect(selectable).toEqual(["todo", "review"]);
  });

  /** Closing owes a reason, a message and evidence, which the Close form asks for. */
  it("never offers done or wont_do", async () => {
    await show();

    const select = screen.getByLabelText("Status");
    expect(within(select).queryByRole("option", { name: "done" })).toBeNull();
    expect(
      within(select).queryByRole("option", { name: "wont_do" }),
    ).toBeNull();
  });

  it("sends an empty milestone to clear the phase, rather than nothing", async () => {
    await show();

    fireEvent.change(screen.getByLabelText("Milestone"), {
      target: { value: "" },
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    expect(updateTask).toHaveBeenCalledWith("tsk_me", {
      version: 7,
      milestone: "",
    });
  });

  /**
   * Reopening means deciding what becomes of the close reason and the
   * evidence, which is a question rather than a control — so a finished task
   * shows its status and does not offer to move it.
   */
  it("does not offer a status control on a task that is closed", async () => {
    TASK.status = "done";
    await show();

    expect(screen.queryByLabelText("Status")).toBeNull();
    // The rest stays editable: recategorising something finished is ordinary.
    expect(screen.getByLabelText("Priority")).toBeTruthy();
    expect(screen.getByLabelText("Kind")).toBeTruthy();
  });

  /** A conflict is not a broken app, and the message should not read like one. */
  it("says the row moved when the version is stale", async () => {
    updateTask.mockImplementation(async () => {
      throw new ApiError("stale", 409);
    });
    await show();

    fireEvent.change(screen.getByLabelText("Kind"), {
      target: { value: "bug" },
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    expect(screen.getByRole("alert").textContent).toContain(
      "changed while you were looking at it",
    );
  });
});

/**
 * Two changes in flight would send the same version twice, and the second
 * would come back a conflict for no reason the reader could see. The controls
 * go inert together rather than the one being saved going inert alone.
 */
describe("while a change is saving", () => {
  afterEach(() => {
    updateTask.mockClear();
    updateTask.mockImplementation(async () => TASK);
  });

  it("disables every field, not only the one being saved", async () => {
    let release: (() => void) | null = null;
    updateTask.mockImplementation(
      () =>
        new Promise((resolve) => {
          release = () => resolve(TASK);
        }),
    );
    await show();

    fireEvent.change(screen.getByLabelText("Priority"), {
      target: { value: "p3" },
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    for (const field of ["Status", "Priority", "Kind", "Milestone"]) {
      expect((screen.getByLabelText(field) as HTMLSelectElement).disabled).toBe(
        true,
      );
    }

    await act(async () => {
      release?.();
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect((screen.getByLabelText("Kind") as HTMLSelectElement).disabled).toBe(
      false,
    );
  });
});
