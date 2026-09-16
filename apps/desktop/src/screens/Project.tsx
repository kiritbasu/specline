/**
 * Screen 2 — Project dashboard.
 *
 * The digest, rendered. Deliberately the same data `specline_context` gives an
 * agent: if a human and a model are looking at different summaries of the same
 * project, one of them is wrong and nobody knows which.
 */

import { api, type Digest, type DriftSection } from "../lib/api";
import { useAsync } from "../lib/useAsync";
import { Badge, Card, Empty, ErrorBox, Id, Spinner, Stat, statusTone } from "../components/ui";
import { Page, projectCrumbs } from "../components/Page";
import { href } from "../lib/router";
import type { ScreenProps } from "../App";

export function ProjectScreen({ route, generation }: ScreenProps) {
  const project = route.project;
  const { data, error, loading, reload } = useAsync<Digest>(
    () => api.context(project),
    [project, generation],
  );

  if (!project) return <Empty message="Pick a project." />;
  if (loading && !data) return <Spinner />;
  if (error) {
    return (
      <Page title={project} crumbs={projectCrumbs(route)}>
        <ErrorBox error={error} retry={reload} />
      </Page>
    );
  }
  if (!data?.project) return <Empty message="Project not found." />;

  const p = data.project;

  return (
    <Page
      title={p.name}
      crumbs={projectCrumbs(route)}
      width="wide"
      meta={<Badge tone={statusTone(p.status)}>{p.status}</Badge>}
      actions={
        p.active_milestone ? (
          <span className="text-small text-ink-muted">Active: {p.active_milestone}</span>
        ) : undefined
      }
    >
      <div className="space-y-5">
        <div className="flex gap-8 rounded-lg border border-border-subtle bg-surface-raised px-5 py-4">
          <Stat value={p.open_tasks} label="open" />
          <Stat value={p.urgent_tasks} label="urgent" tone={p.urgent_tasks ? "text-warn" : undefined} />
          <Stat value={p.blocked_tasks} label="blocked" tone={p.blocked_tasks ? "text-bad" : undefined} />
          <Stat value={p.open_questions} label="questions" />
          {/* The digest's size and its budget used to sit here. Both measure
              what an agent reads, in a unit only an agent has — a token count
              on a human's dashboard is a number nobody can act on. */}
        </div>

        {data.drift && <DriftCard drift={data.drift} project={project} />}

        <div className="grid gap-5 lg:grid-cols-2">
          <Card
            title="Needs attention"
            actions={
              <a
                href={href({ screen: "board", project })}
                className="text-small text-accent hover:underline"
              >
                board →
              </a>
            }
          >
            {data.attention.length === 0 ? (
              <Empty message="Nothing urgent or blocked." />
            ) : (
              <ul className="space-y-2">
                {data.attention.map((t) => (
                  <li key={t.id} className="flex items-center gap-2 text-small">
                    <Badge tone={statusTone(t.status)}>{t.status}</Badge>
                    <span className="selectable truncate">{t.label}</span>
                    {t.detail && <span className="ml-auto text-micro text-ink-faint">{t.detail}</span>}
                  </li>
                ))}
              </ul>
            )}
            {data.truncated
              .filter((t) => t.section === "attention")
              .map((t) => (
                <p key={t.section} className="mt-3 text-small text-ink-faint">
                  Showing {t.shown} of {t.total}.
                </p>
              ))}
          </Card>

          <Card title="Open questions and risks">
            {data.questions.length === 0 ? (
              <Empty message="Nothing unresolved." />
            ) : (
              <ul className="space-y-2">
                {data.questions.map((q) => (
                  <li key={q.id} className="flex items-start gap-2 text-small">
                    <span className="mt-1.5 h-1.5 w-1.5 shrink-0 rounded-full bg-warn" />
                    <span className="selectable">{q.label}</span>
                    {q.detail && (
                      <span className="ml-auto shrink-0 text-micro text-ink-faint">{q.detail}</span>
                    )}
                  </li>
                ))}
              </ul>
            )}
          </Card>

          <Card title="Recent decisions">
            {data.decisions.length === 0 ? (
              <Empty message="None accepted yet." />
            ) : (
              <ul className="space-y-2 text-small">
                {data.decisions.map((d) => (
                  <li key={d.id} className="selectable">
                    {d.label}
                  </li>
                ))}
              </ul>
            )}
          </Card>

          <Card
            title="Specs"
            actions={
              <a
                href={href({ screen: "documents", project })}
                className="text-small text-accent hover:underline"
              >
                read →
              </a>
            }
          >
            {data.specs.length === 0 ? (
              <Empty message="No specs yet." />
            ) : (
              <ul className="space-y-2 text-small">
                {data.specs.map((s) => (
                  <li key={s.id}>
                    <a
                      href={href({ screen: "documents", project, documentId: s.id })}
                      className="flex items-center gap-2 hover:underline"
                    >
                      <Badge tone={statusTone(s.status)}>{s.status}</Badge>
                      <span className="truncate">{s.label}</span>
                    </a>
                  </li>
                ))}
              </ul>
            )}
          </Card>

          <Card title="What is live">
            {data.environments.length === 0 ? (
              <Empty message="No environments recorded." />
            ) : (
              <ul className="space-y-2 text-small">
                {data.environments.map((e) => (
                  <li key={e.id} className="flex items-center gap-2">
                    <Badge tone={statusTone(e.status)}>{e.status}</Badge>
                    <span>{e.label}</span>
                    {e.detail && <Id value={e.detail} />}
                  </li>
                ))}
              </ul>
            )}
          </Card>

          <Card title="Glossary">
            {data.terms.length === 0 ? (
              <Empty
                message="No terms yet."
                hint="Terms are cheap to add and stop the next session guessing."
              />
            ) : (
              <dl className="space-y-2 text-small">
                {data.terms.map((t) => (
                  <div key={t.term} className="selectable">
                    <dt className="inline font-medium">{t.term}</dt>
                    {t.global && <span className="ml-1 text-micro text-ink-faint">(global)</span>}
                    <dd className="inline text-ink-muted"> — {t.definition}</dd>
                  </div>
                ))}
              </dl>
            )}
          </Card>
        </div>

        {data.next_up && (
          <Card title="Next">
            {data.next_up.ready.length > 0 ? (
              <ol className="space-y-2">
                {data.next_up.ready.map((item, i) => (
                  <li key={item.id} className="flex gap-2.5">
                    <span className="mt-0.5 w-4 shrink-0 text-right text-small tabular-nums text-ink-faint">
                      {i + 1}
                    </span>
                    <div className="min-w-0">
                      <div className="text-small">
                        <span className="mr-1.5 font-mono text-micro text-ink-faint">
                          {item.reference}
                        </span>
                        {item.title}
                      </div>
                      <div className="mt-0.5 text-small text-ink-faint">{item.why}</div>
                    </div>
                  </li>
                ))}
              </ol>
            ) : (
              <Empty
                message="Nothing is ready to pick up."
                hint="Everything open is blocked or waiting on a decision — unblocking one is the work."
              />
            )}

            {data.next_up.waiting_on_you.length > 0 && (
              <div className="mt-4 border-t border-border-subtle pt-3">
                <h3 className="mb-1.5 text-small font-semibold tracking-wide text-ink-muted uppercase">
                  Waiting on you
                </h3>
                <ul className="space-y-1 text-small text-ink-muted">
                  {data.next_up.waiting_on_you.map((item) => (
                    <li key={item.id}>
                      <span className="mr-1.5 font-mono text-micro text-ink-faint">
                        {item.reference}
                      </span>
                      {item.title}
                    </li>
                  ))}
                </ul>
              </div>
            )}

            {data.next_up.blocked.length > 0 && (
              <div className="mt-4 border-t border-border-subtle pt-3">
                <h3 className="mb-1.5 text-small font-semibold tracking-wide text-ink-muted uppercase">
                  Blocked
                </h3>
                <ul className="space-y-1.5 text-small">
                  {data.next_up.blocked.map((item) => (
                    <li key={item.id}>
                      <div className="text-ink-muted">{item.title}</div>
                      <div className="text-small text-ink-faint">{item.why}</div>
                    </li>
                  ))}
                </ul>
              </div>
            )}
          </Card>
        )}

        {data.next.length > 0 && (
          <Card title="Also worth noticing">
            <ul className="space-y-1.5 text-small text-ink-muted">
              {data.next.map((line, i) => (
                <li key={i}>{line}</li>
              ))}
            </ul>
          </Card>
        )}
      </div>
    </Page>
  );
}

/** How many of each list the card shows before saying how many it cut. */
const DRIFT_SHOWN = 5;

/**
 * The join between the repository and the rows: what landed this week with no
 * task behind it, and what closed with nothing landing.
 *
 * Rendered only when the digest carries the section, so a project with no
 * checkout has no card rather than an empty one. An unreadable checkout is a
 * card that says so — the one state this must never collapse into is "no
 * commits", because a broken git and a quiet week would then look the same.
 */
function DriftCard({ drift, project }: { drift: DriftSection; project: string }) {
  if (drift.state === "unreadable") {
    return (
      <Card title="Commits">
        <p className="text-small text-warn">Not measured: {drift.reason}</p>
        <p className="mt-1.5 text-small text-ink-faint">
          The project has a checkout recorded and its history could not be read.
        </p>
      </Card>
    );
  }

  const since = drift.since.slice(0, 10);
  const cut = (shown: number, total: number, what: string) =>
    total > shown ? (
      <li className="text-small text-ink-faint">
        …and {total - shown} more {what}
      </li>
    ) : null;

  return (
    <Card title={`Commits since ${since}`}>
      <div className="flex gap-8">
        <Stat
          value={drift.commits_total}
          label={drift.commits < drift.commits_total ? `commits, newest ${drift.commits} read` : "commits"}
        />
        <Stat value={drift.linked.length} label="name a task" />
        <Stat
          value={drift.unlinked.length}
          label="name none"
          tone={drift.unlinked.length ? "text-warn" : undefined}
        />
        <Stat
          value={drift.done_without_commit.length}
          label="done, no commit"
          tone={drift.done_without_commit.length ? "text-warn" : undefined}
        />
      </div>

      {drift.commits === 0 && drift.done_without_commit.length === 0 ? (
        <p className="mt-4 text-small text-ink-faint">Nothing landed and nothing closed.</p>
      ) : (
        <div className="mt-4 grid gap-5 lg:grid-cols-2">
          <div>
            <h3 className="mb-1.5 text-small font-semibold tracking-wide text-ink-muted uppercase">
              Commits naming no task
            </h3>
            {drift.unlinked.length === 0 ? (
              <p className="text-small text-ink-faint">Every commit reached a row.</p>
            ) : (
              <ul className="space-y-1.5 text-small">
                {drift.unlinked.slice(0, DRIFT_SHOWN).map((c) => (
                  <li key={c.sha} className="flex min-w-0 items-baseline gap-2">
                    {/* Not `Id`: that breaks anywhere to fit, and beside a
                        truncating subject it split a seven-character sha over
                        two lines on the live dashboard. */}
                    <code className="selectable shrink-0 font-mono text-micro text-ink-faint">
                      {c.sha}
                    </code>
                    <span className="selectable truncate">{c.subject}</span>
                  </li>
                ))}
                {cut(DRIFT_SHOWN, drift.unlinked.length, "commit(s)")}
              </ul>
            )}
          </div>
          <div>
            <h3 className="mb-1.5 text-small font-semibold tracking-wide text-ink-muted uppercase">
              Closed done with no commit
            </h3>
            {drift.done_without_commit.length === 0 ? (
              <p className="text-small text-ink-faint">Every closed task cites one.</p>
            ) : (
              <ul className="space-y-1.5 text-small">
                {drift.done_without_commit.slice(0, DRIFT_SHOWN).map((t) => (
                  <li key={t.id}>
                    <a
                      href={href({ screen: "task", project, taskId: t.reference })}
                      className="flex min-w-0 items-baseline gap-2 hover:underline"
                    >
                      <span className="shrink-0 font-mono text-micro text-ink-faint">
                        {t.reference}
                      </span>
                      <span className="truncate">{t.title}</span>
                    </a>
                  </li>
                ))}
                {cut(DRIFT_SHOWN, drift.done_without_commit.length, "task(s)")}
              </ul>
            )}
          </div>
        </div>
      )}

      {drift.unknown.length > 0 && (
        <p className="mt-3 text-small text-ink-faint">
          {drift.unknown.length} commit(s) name a task that does not exist:{" "}
          {drift.unknown
            .slice(0, DRIFT_SHOWN)
            .map((u) => `${u.key} (${u.sha})`)
            .join(", ")}
          {drift.unknown.length > DRIFT_SHOWN && ", …"}
        </p>
      )}
      {drift.tasks_scanned < drift.tasks_total && (
        <p className="mt-3 text-small text-warn">
          Only the newest {drift.tasks_scanned} of {drift.tasks_total} task rows were read, so a
          commit naming an older task is reported above as naming one that does not exist.
        </p>
      )}
    </Card>
  );
}
