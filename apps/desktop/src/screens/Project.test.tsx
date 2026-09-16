/**
 * The Commits card on the project dashboard: three states, and the one
 * collapse that must never happen.
 *
 * The digest carries the join between the repository and the rows in one of
 * three shapes — absent, unreadable, measured — and they mean different
 * things. What is worth guarding is that an unreadable checkout produces a
 * card that *says so*, because the alternative (no card, or an empty one) is
 * indistinguishable from a quiet week, and that is the bug class this project
 * has already been bitten by in its graph queries.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import type { Digest } from "../lib/api";

const BASE: Digest = {
  project: {
    id: "prj_1",
    name: "Tideline",
    slug: "tideline",
    key: "TIDE",
    status: "active",
    open_tasks: 3,
    urgent_tasks: 0,
    blocked_tasks: 0,
    open_questions: 0,
    inbox: 0,
    inbox_oldest_days: null,
    active_milestone: null,
  },
  projects: [],
  active: [],
  attention: [],
  recent: [],
  decisions: [],
  questions: [],
  specs: [],
  terms: [],
  environments: [],
  next: [],
  next_up: null,
  truncated: [],
  budget_exceeded: false,
  estimated_tokens: 0,
};

const state = { digest: BASE as Digest };

vi.mock("../lib/api", () => ({
  ApiError: class ApiError extends Error {},
  subscribe: () => () => {},
  api: {
    context: async () => state.digest,
  },
}));

const { ProjectScreen } = await import("./Project");

async function show() {
  render(
    <ProjectScreen route={{ screen: "project", project: "tideline", query: {} }} generation={0} />,
  );
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

beforeEach(() => {
  window.location.hash = "#/projects/tideline";
  state.digest = BASE;
});
afterEach(cleanup);

describe("the Commits card", () => {
  it("is absent when there is nothing to measure", async () => {
    state.digest = { ...BASE, drift: null };
    await show();
    expect(screen.queryByText(/Commits since/)).toBeNull();
    expect(screen.queryByText(/Not measured/)).toBeNull();
  });

  it("says why when the checkout could not be read, rather than looking quiet", async () => {
    state.digest = {
      ...BASE,
      drift: { state: "unreadable", reason: "git exited 128: not a git repository" },
    };
    await show();
    expect(screen.getByText(/Not measured: git exited 128/)).toBeTruthy();
    expect(screen.queryByText(/Nothing landed/)).toBeNull();
  });

  it("shows the counts, the unrowed commits and the uncommitted closes", async () => {
    state.digest = {
      ...BASE,
      drift: {
        state: "measured",
        since: "2026-09-09T18:00:00Z",
        commits: 3,
        commits_total: 3,
        linked: [{ sha: "aaaaaaa", subject: "feat: rowed (TIDE-1)", tasks: ["TIDE-1"] }],
        unlinked: [
          { sha: "bbbbbbb", subject: "chore: unrowed", committed_at: "2026-09-15T10:00:00Z" },
          { sha: "ccccccc", subject: "fix: also unrowed", committed_at: "2026-09-15T11:00:00Z" },
        ],
        unknown: [{ sha: "ccccccc", key: "TIDE-4242" }],
        done_without_commit: [
          { reference: "TIDE-7", id: "tsk_7", title: "Closed on a test", closed_at: "2026-09-14T09:00:00Z" },
        ],
        tasks_scanned: 12,
        tasks_total: 12,
      },
    };
    await show();
    expect(screen.getByText("Commits since 2026-09-09")).toBeTruthy();
    expect(screen.getByText("chore: unrowed")).toBeTruthy();
    expect(screen.getByText("fix: also unrowed")).toBeTruthy();
    expect(screen.getByText("Closed on a test").closest("a")?.getAttribute("href")).toBe(
      "#/projects/tideline/tasks/TIDE-7",
    );
    expect(screen.getByText(/1 commit\(s\) name a task that does not exist: TIDE-4242/)).toBeTruthy();
    expect(screen.queryByText(/lists are incomplete/)).toBeNull();
  });

  it("says how many it cut and when the row scan was capped", async () => {
    state.digest = {
      ...BASE,
      drift: {
        state: "measured",
        since: "2026-09-09T18:00:00Z",
        commits: 8,
        commits_total: 8,
        linked: [],
        unlinked: Array.from({ length: 8 }, (_, i) => ({
          sha: `${i}${i}${i}${i}${i}${i}${i}`,
          subject: `chore: ${i}`,
          committed_at: "2026-09-15T10:00:00Z",
        })),
        unknown: [],
        done_without_commit: [],
        tasks_scanned: 5000,
        tasks_total: 5100,
      },
    };
    await show();
    expect(screen.getByText("chore: 4")).toBeTruthy();
    expect(screen.queryByText("chore: 5")).toBeNull();
    expect(screen.getByText(/…and 3 more commit\(s\)/)).toBeTruthy();
    expect(screen.getByText(/Only the newest 5000 of 5100 task rows were read/)).toBeTruthy();
  });

  it("is measured-and-empty when nothing landed, which is not the same as absent", async () => {
    state.digest = {
      ...BASE,
      drift: {
        state: "measured",
        since: "2026-09-09T18:00:00Z",
        commits: 0,
        commits_total: 0,
        linked: [],
        unlinked: [],
        unknown: [],
        done_without_commit: [],
        tasks_scanned: 12,
        tasks_total: 12,
      },
    };
    await show();
    expect(screen.getByText("Commits since 2026-09-09")).toBeTruthy();
    expect(screen.getByText("Nothing landed and nothing closed.")).toBeTruthy();
  });
});
