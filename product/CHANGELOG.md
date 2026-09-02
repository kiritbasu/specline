# Specline — Changelog

<!-- specline:generated project prj_01KZKMPVHJNCCQH3JQNAXJJ03M 2026-09-02T20:46:37Z -->
> **Generated from the task rows and the event log. Do not edit — Specline is the source of truth.**

What has finished. What is happening now is in the tracker beside this file.

---

## Closed work (316)

### 2026-09-02

- **KEEL-357** Tidy the duplication this session's two fixes left in their own code — `done`

  Both consolidations landed and nothing changed behaviour: 1,276 and 1,275 tests before and after, with no test modified to keep it passing.
  
  `injected_context` replaces five hand-rolled extractions in hooks.rs, and takes the `hookEventName` assertion in with it — that check now runs at every site that reads a payload instead of at the one test that happened to make it. The failure messages improved on the way: the worst of the three said only "additionalContext is a string" and printed nothing about what was actually there.
  
  The two http.rs calls now use imported names like everything else in that file, which stopped one call site wrapping and collapsed the `initialize` arm from three lines to one.
  
  Two candidates were rejected rather than missed, and the reasoning is in the commit: `session_start`'s nested match reads better than the flatter guard-clause version, and building `SUPPORTED_VERSIONS` by indexing `LEGACY_VERSIONS` would trade readable strings for subscripts without closing the gap the drift test already covers.

  <sub>commit:b5c1200 · test:cargo test --workspace · test:cargo test --workspace --exclude specline-embed --no-default-features</sub>

- **KEEL-356** Clear the dead code the version-negotiation change left behind — `done`

  `Era::version()` is gone — it had no call sites once `initialize_result` started taking the version rather than the era. `Era` stays, because the response envelope still branches on it.
  
  `codes::UNSUPPORTED_PROTOCOL_VERSION` stays too, and now says why nothing raises it. It is the specification's number for the condition rather than Specline's, and `http_status` should map it if a reason to send it ever returns; what it was missing was the sentence explaining the gap between a constant, two tests, and no producer.
  
  Both clippy configurations and both suites clean, with the test counts unchanged at 1,276 and 1,275 — which is what removing something nothing called should do.

  <sub>commit:61404b1 · test:cargo test --workspace · test:cargo clippy --workspace --all-targets -- -D warnings</sub>

- **KEEL-355** Codex cannot connect: the daemon refuses MCP 2025-06-18 instead of offering one it speaks — `done`

  Codex connects. The daemon no longer refuses a revision it does not recognise — anything that is not the current one is read as legacy, and negotiation moved into what gets echoed back: a served revision comes back as itself, and only an unrecognised one gets a counter-offer.
  
  Verified live rather than in tests alone. Codex 0.148.0-alpha.15 asked for 2025-06-18, was answered 2025-06-18, sent notifications/initialized, listed the thirteen tools and ran specline_projects to completion — captured through the same logging proxy that caught the original failure. Run against a scratch store on its own port so the production daemon kept its lock throughout.
  
  Three tests that asserted the refusal now assert the service, and two new ones drive the exact initialize body captured from Codex, one at the unit level and one end to end through the daemon. Both clippy configurations and both suites clean: 1,276 tests with embeddings on, 1,275 without.

  <sub>commit:d58a8c4 · test:cargo test --workspace · test:cargo test --workspace --exclude specline-embed --no-default-features</sub>

- **KEEL-354** The session hook goes quiet when the daemon is down, instead of saying so — `done`

  A session start that cannot reach the daemon now injects a message saying so, naming the address, the cause and what to do — and telling the model to pass it on rather than working as though Specline were not installed. The three causes say different things, because starting a daemon that is already running is the wrong advice.
  
  The test that covered this was asserting the bug, so it was replaced rather than added to: three integration tests for refused, listening-but-broken, and the silence that is still correct when the daemon answers with nothing, plus two unit tests for the arm a real socket cannot reach deterministically. Both clippy configurations and both suites are clean — 1,270 tests with embeddings on, 1,269 without, and the hook suite passes with an empty HOME.

  <sub>commit:0397889 · test:cargo test --workspace · test:cargo test --workspace --exclude specline-embed --no-default-features</sub>

- **KEEL-352** Verify the repository builds and tests clean from the external drive — `done`

  The repository builds and tests clean from the external drive. A full clean rebuild, `cargo fmt --all --check`, both clippy configurations and both test suites all pass from /Volumes/mydrv/development/specline, and nothing turned out to depend on where the checkout lives. The details, and the four things about the new volume worth knowing, are on the note.
  
  One thing this did not settle: the project row still records `root_path` as /Users/h8hcn/development/specline, and that checkout still exists with a daemon running against it. Which of the two copies is canonical is a decision rather than a build problem, so it is left alone.

  <sub>test:cargo test --workspace · test:cargo test --workspace --exclude specline-embed --no-default-features · test:cargo clippy --workspace --all-targets -- -D warnings · test:cargo clippy --workspace --exclude specline-embed --all-targets --no-default-features -- -D warnings</sub>

### 2026-08-19

- **KEEL-351** Setup told a downloaded binary it had embeddings on — `done`

  Setup now reads `embeddings.built_in` from the health payload it was already waiting on, and a build with no model gets told so in one bold paragraph with the reason. The daemon's startup line no longer names a flag nobody typed. Both configurations lint clean and the daemon suite passes.

  <sub>commit:502a445 · test:cargo test -p specline-daemon · test:cargo clippy --workspace --exclude specline-embed --all-targets --no-default-features -- -D warnings</sub>

- **KEEL-350** Embeddings on by default, and the backlog embedded without anyone asking — `done`

  A daemon started with no flags at all now loads the model and backfills what has no vector. Proved on a scratch store: the fixture loaded 52 documents with no vectors, the daemon started, said "embedding them in the background once the model has loaded", and thirty seconds later `specline doctor` said all 52 had one — with a search over that store returning hits sourced from `both`. `--no-embeddings` turns it off and is reported by `/api/health` as `loaded: false`; the old `--embeddings` still starts, ignored. The installer's default matches, and so do the launchd and systemd units it writes.

  <sub>commit:21bb5d4 · test:cargo test -p specline-daemon --test backfill · doc:dec_01M0DWM0GZ0AC0R0JWPKQ1DQWF</sub>

- **KEEL-348** Stop building an AV1 encoder in order to embed text — `done`

  `fastembed` is declared with default features off and the two it actually uses, so `image`, `ravif` and `rav1e` leave the graph: 310 crates to 255. The embedder still loads and embeds — a daemon built from this backfilled 52 documents from a cold start.

  <sub>commit:5fb2988 · test:cargo tree --workspace --edges normal · test:cargo test -p specline-daemon --test backfill</sub>

- **KEEL-251** specline_search promises hybrid retrieval and runs keyword-only, without saying so — `done`

  A search now returns `searched` naming the halves that ran and `not_searched` saying why the others did not, and the summary says it in prose — with a different sentence when there were no hits, because that is the one that gets read as a fact about the store. Proved against a running daemon: with no model, an unmatched query comes back "not evidence that nothing is stored about it"; with one, `searched` is `["keyword","semantic"]` and hits arrive sourced from `both`. `specline doctor` gained a `semantic_search` check for the case the embeddings check cannot see — a fully embedded store served by a process with no model.

  <sub>commit:b15861e · test:cargo test --workspace · test:cargo test --workspace --exclude specline-embed --no-default-features</sub>

- **KEEL-211** Put the embedding model somewhere sensible and ask before downloading it — `wont_do`

  Both halves are settled, neither by doing what this row asked. The model cache has lived under the Specline home since the daemon started deriving it from `home` — `~/.specline/models`, not fastembed's process-relative default — so the first half was already true. The second half, asking before the download, is reversed by B-95: KB asked for embeddings to work with no manual setup, and the six months this was open are the argument, because the download nobody was asked about is also the download nobody ever got.

- **KEEL-347** Run the daily-driver daemon with embeddings, and backfill the documents that have no vector — `done`

  Stopped the daemon, ran `specline reembed --missing` — 62 documents, 26 seconds — and restarted it with `--embeddings`. Health now reports `loaded: true`, doctor says all 200 current documents have a vector, and a search comes back with every hit sourced from `both` halves rather than keyword alone. The one part of the summary not met is the reboot: KB chose to leave the daemon hand-started rather than install a launch agent, so the flag has to be typed again after a restart.

  <sub>test:specline reembed --missing · test:specline doctor · url:http://127.0.0.1:7654/api/health</sub>

- **KEEL-346** Write the release-row-then-tag loop into the standing contract — `done`

  The standing contract now says the release row is written before the tag and why, with the concrete sequence and the lightweight-tag refusal recorded alongside the measurement that justifies it. It loads in every session, since the root CLAUDE.md imports it.

  <sub>commit:9a0b607 · doc:spc_01KZKSME2TCPVARX9M04836XD6</sub>

- **KEEL-343** Cut 0.4.1 — the rail without its shortcuts, and the signal lifecycle — `done`

  v0.4.1 is out on all three platforms with build provenance. Briefly held for the rest of Phase 14 and then released: the session that work belonged to had already finished and the remaining rows were unstarted, so holding would have parked a finished fix behind days of new work.

  <sub>commit:37808e0 · url:https://github.com/kiritbasu/specline/releases/tag/v0.4.1 · test:cargo test --workspace</sub>

- **KEEL-345** A release reads like something a person wrote, and carries five files instead of fourteen — `done`

  Proven on v0.4.1, the first release through the new path: five assets instead of fourteen, and the notes are the release row's prose rather than a list of commit subjects. The provenance caveat correctly did not appear — the repository is public now, so attestations ran instead, which is what the old caveat said would happen.

  <sub>commit:1f4e662 · url:https://github.com/kiritbasu/specline/releases/tag/v0.4.1 · url:https://github.com/kiritbasu/specline/actions/runs/32296286655</sub>

- **KEEL-328** Breaking a feature into tasks is proposed, not typed — `done`

  Nothing needed building — the whole decomposition path already works over MCP with existing tools, verified against a live daemon: a feature spec, an epic, an `implements` edge, and three children under it. What was missing is the habit rather than the mechanism, so the guidance is written as a note here and moving it into the plugin skill is KEEL-344, held until the flag flips.

  <sub>test:curl /mcp specline_create + specline_link against a live daemon · doc:spc_01M0DDZ3MKMAMFC26MRMSXX7MV · commit:131ab51</sub>

- **KEEL-327** The board shows an epic as one row that opens into its children — `done`

  An epic heads its own group with "2/4 done" read off its children, and no longer appears a second time as a loose row. Grouping by parent already existed, so this was a fraction and a de-duplication rather than a feature. Verified on screen against a seeded epic — the double-listing was invisible to every test.

  <sub>commit:131ab51 · test:npx vitest run src/lib/tasks.test.ts</sub>

- **KEEL-326** An epic is a task with children, and it appears only when we decide to build — `done`

  `feature` is a task kind, offered in the app's three kind lists while the lifecycle is on. Far smaller than the task assumed: `parent_id` was already fully implemented and guarded by `check_task_parent`, so composition needed nothing. The task's premise that it "has never been used" was true of the data and false of the code.

  <sub>commit:a1308fd · test:cargo test -p specline-core --test epics · test:cargo test -p specline-mcp --test argument_edges</sub>

- **KEEL-342** Take the shortcut keycaps out of the rail — `done`

  The rail is labels only. The keys still work and the row's title names its key on hover, so the shortcut is quiet rather than gone — removing it outright is one line further if that turns out to be what was wanted. The signal that reported it is triaged and linked.

  <sub>commit:b53121e · test:npx vitest run src/App.test.tsx</sub>

- **KEEL-341** Put the Inbox behind a flag, off by default, until the lifecycle is finished — `done`

  `SPECLINE_INBOX`, off by default. Hides the nav item, both endpoints, the digest count and section, and the two CLI verbs — and hides no data. Verified against a real daemon in both states: off gives 404 and a zero count with the rows untouched in the store; on gives 200 and eleven signals.

  <sub>commit:521c7a1 · test:cargo test -p specline-daemon --test ui_writes · test:cargo test -p specline-core --test inbox</sub>

- **KEEL-325** The triage pass: read the whole Inbox, cluster it, and check it against everything already decided — `done`

  Triage reaches MCP through a widened `specline_close` (B-94), and the digest now lists the Inbox rather than only counting it — which KEEL-321 had left uncounterable, because search needs a query and cannot enumerate "everything untriaged". A pass over the real five-signal Inbox proposed an outcome for each and named the one it could not settle; it is on the row, awaiting KB's yes. Running it found and fixed a real bug.

  <sub>commit:05e5c14 · commit:1835ad5 · commit:87f623c · test:cargo test -p specline-core --test inbox</sub>

- **KEEL-324** Setting a signal down writes the argument, so the same idea does not arrive fresh in four months — `done`

  `work::triage` enforces that a signal cannot leave the Inbox without an outcome, and `specline triage` exposes it. Two holes found and filed rather than papered over: notes are not indexed (KEEL-339), and `triaged` can still be set through the ordinary update path (KEEL-340). Triage over MCP is left to KB because it needs either a fourteenth tool or a widened `specline_close`.

  <sub>commit:f3dcc8a · test:cargo test -p specline-core --test inbox</sub>

- **KEEL-323** A signal that gets picked up becomes a feature, and a feature is a spec — `done`

  `feature` is a spec kind, and a feature spec remembers the signal it came from in both traversal directions. Smaller than expected because the `derived_from` edge and its direction test already existed — the relation table in SPEC §3.3 had spec → feedback from the start.

  <sub>commit:76bc1c0 · test:cargo test -p specline-core --test inbox</sub>

- **KEEL-322** Move the four rows KB filed in a hurry into the Inbox they should have gone to — `done`

  KEEL-303, 305 and 306 are signals in the Inbox with their verbatim preserved and their real source recorded; the task rows are closed and linked. KEEL-302 was already superseded by KEEL-325 before this ran. The Inbox on the real store now holds five, which is three more than anybody could see yesterday and two more than were filed today.

  <sub>url:http://127.0.0.1:7654/#/projects/specline/documents/fbk_01M0D5JPRSQVCSBJAGT8WYH708 · url:http://127.0.0.1:7654/#/projects/specline/documents/fbk_01M0D5K79M1A5ZHR2MY45QJX9M · url:http://127.0.0.1:7654/#/projects/specline/documents/fbk_01M0D5KE9KZKSZV9R74ZXRSEFZ</sub>

- **KEEL-306** Support openai codex — `wont_do`

  Not doing this as a task, because it was never one — and it is not even KB's own ask. Re-filed as a signal sourced to Madhu, which is the first time anybody other than KB has a request recorded as theirs: fbk_01M0D5JPRSQVCSBJAGT8WYH708. Whether "work with codex" means the MCP endpoint or the whole plugin surface is what triage has to settle.

- **KEEL-305** allow adding new Feature Requests — `wont_do`

  Not doing this as a task, because it was never one. Re-filed as a signal with the verbatim preserved: fbk_01M0D5KE9KZKSZV9R74ZXRSEFZ. The ask itself is being built — the Inbox half is done (KEEL-319, 320, 321) and decomposition is KEEL-326 and KEEL-328.

- **KEEL-303** periodic management of lots of open issues — `wont_do`

  Not doing this as a task, because it was never one — nobody had committed to it and there was nothing to claim. Re-filed as a signal in the Inbox with the verbatim preserved: fbk_01M0D5K79M1A5ZHR2MY45QJX9M. `wont_do` is the closest of the five reasons and it is not the right word; see KEEL-338.

- **KEEL-320** The Inbox screen, and filing a signal in six seconds — `done`

  The Inbox screen, the two endpoints behind it, and filing in one field. Verified in a browser against a daemon serving the built bundle, not only in tests — which is where the one real defect turned up.

  <sub>commit:f538160 · test:cargo test -p specline-daemon --test ui_writes · test:npx vitest run src/screens/Inbox.test.tsx</sub>

- **KEEL-337** Cut 0.4.0 — the Releases screen, the roadmap that says where you are, and the Inbox — `done`

  v0.4.0 is out on all three platforms, and it is the first release that exists as a row in Specline while it is happening rather than being reconstructed from git tag afterwards. Twenty-one commits went with it, including a fortnight of work that had never been pushed.

  <sub>commit:50f9ebe · url:https://github.com/kiritbasu/specline/releases/tag/v0.4.0 · url:https://github.com/kiritbasu/specline/actions/runs/32255614156 · test:cargo test --workspace</sub>

- **KEEL-336** Split the Roadmap: phases grouped by state, releases on a screen of their own — `done`

  Releases is its own screen at 6 in the rail, and the Roadmap is phases grouped by what they are doing — in flight first, then finished-but-not-declared with a line saying what to do about it. Every phase carries its full description again. Releases is a table, newest first, rather than ten cards wearing a phase's clothes.

  <sub>commit:098f3b2 · doc:dec_01M0D0KX0ZGSDTSQE1JPG5P8ZY · test:npx vitest run · url:http://127.0.0.1:7654/#/projects/specline/releases</sub>

- **KEEL-335** Fix what the review found in the roadmap progress work — `done`

  All six fixed. The roadmap now shows a fraction on all fifteen phases rather than seven; the activity query goes through `parse_ts` and takes its maximum in Rust, so no timestamp format can make a phase look untouched; the daemon derives progress only from the milestone rows a page actually contains; release table cells are escaped; and the two surfaces agree about where an uncut release goes. Every test that passed under mutation now fails under it.

  <sub>commit:4ab9f77 · test:cargo test -p specline-core --test phase_progress · test:cargo test -p specline-daemon --test milestone_progress · test:npx vitest run src/screens/Roadmap.test.tsx</sub>

- **KEEL-334** SPEC promises "overdue milestones" in the digest and nothing computes it — `done`

  Cut, not built. `needs_attention` reads the task list and the `blocks` edges and never looks at a milestone, so "overdue milestones" was not merely unimplemented — the section it was promised in is task-only. The same claim in the doc comment on `Digest::attention` went with it. The `active` line was cut back too: it still advertised a target date the digest stopped emitting an hour earlier.

  <sub>commit:pending · doc:spc_01KZKMPVNTZAZHC9HY1TSNZNGM</sub>

- **KEEL-333** Backfill the ten shipped versions as release rows, so the roadmap has a real time axis — `done`

  All ten tags from v0.1.0 to v0.3.0 are release rows now, each with its version, its shipped date and a line on what went out. The roadmap and STATUS.md give them a strand of their own rather than mixing them into the phases, because a release carries no tasks and would otherwise be ten rows of `planned 0 / 0`.

  <sub>commit:7c6ef9c · test:cargo test -p specline-daemon --test milestone_progress · url:http://127.0.0.1:7654/#/projects/specline/roadmap</sub>

- **KEEL-332** The roadmap shows how far a phase has got, instead of a target date nobody set — `done`

  The roadmap, the digest and STATUS.md all say how far a phase has got instead of a target date. `milestone_states` is now `milestone_progress` and returns the tally it was already computing and discarding, plus a last-activity time from the event log; the API sends all of it, so nothing counts tasks in the browser.

  <sub>commit:7c6ef9c · test:cargo test -p specline-core --test phase_progress · test:npx vitest run src/screens/Roadmap.test.tsx</sub>

- **KEEL-321** An untriaged signal is not work, and nothing that counts work should count it — `done`

  Signals are counted by nothing that counts work, and the digest now says the Inbox exists rather than staying silent about it. The task as written assumed signals were leaking into the task counts; they never were, and the real defect was the opposite one.

  <sub>commit:19f0580 · test:cargo test -p specline-core --test inbox</sub>

- **KEEL-319** Write a signal into the store, for the first time — `done`

  A signal can be written from MCP and from the CLI, and the feedback table holds rows for the first time. Two defects were in the way and neither was visible from reading — both were found by trying to create one.

  <sub>commit:ffbbca1 · test:cargo test -p specline-mcp --test argument_edges · test:cargo test -p specline-core --test composite · test:cargo test -p specline --test verbs</sub>

- **KEEL-302** product feature request triage — `superseded`

  Arrived as a sentence — "set up some sort of feature where you can slice and dice features and build some sort of triage capability" — and became the whole of Phase 14. The thinking is in the spec "How feature requests should work, end to end" and settled in B-90; the triage capability itself is KEEL-325, with the eleven rows around it covering the rest of the lifecycle this turned out to be one part of.

- **KEEL-318** Make the version footer a compact version-and-icon control — `done`

  Built as agreed with KB over two rounds of layout review: version reads `Specline v0.3.0`, glyph is a 22px cloud-download in a 30px slot, state on a dot rather than on the glyph.
  
  Both objections on this row are answered structurally rather than by promise. The five-state problem: only the up-to-date state collapses to a bare icon, and every state that needs the reader to know something keeps its sentence, so failure and staleness are exactly the cases that stay expanded. The pun: the glyph is always the verb, the dot is always the state, and there is no checkmark in the control.
  
  One existing test reverses, deliberately. It asserted the check button was hidden once something was staged — correct while the control was only a button, wrong now it is also the indicator, since hiding it removes the one sign that there is something to take. Added a test that the switched-off state fires no request, since `specline doctor` promises that daemon makes none at all.
  
  Verified in the running app rather than only in tests: computed 22px glyph in a 30px slot, ink-faint, no dot and no sentence at rest, `Specline v0.3.0` rendered, legible in light and dark, no console errors. 337 desktop tests green, tsc clean.

  <sub>commit:d96450d · test:npm test --run</sub>

- **KEEL-317** Check for updates every 30 minutes, and make the footer an icon that colours when one is waiting — `done`

  The two halves KB asked for are in: the default check interval is half an hour, and the daemon now announces a staged release so the footer learns about it without waiting for an unrelated store write.
  
  Three things beyond the literal ask, each because leaving it would have made the change harmful rather than incomplete. The re-stage defect — plan() never looked at what was already staged, so a daemon left overnight with an update pending would have pulled ~176 MB to no effect at the new interval, and re-announced every half hour. The six disclosure sites that told people Specline makes one hourly request, which would have become a false statement about network behaviour. And check_interval extracted from the task it runs in, so its floor is testable.
  
  The UI half of the row — replacing the button with a version-and-icon control — is deliberately not done. It needs the three design answers on this row settled first, and KB asked to see a layout before the glyphs are chosen. Filed separately as KEEL-318.
  
  Checks: fmt clean, clippy clean in both the default and --no-default-features --exclude specline-embed configurations, full Rust suite green, desktop 336 tests green and tsc clean.

  <sub>commit:1e57620 · test:cargo test --workspace · test:npm test --run</sub>

- **KEEL-315** Three generated phase specs mirror into the repository root — `done`

  The repository root has five markdown files again — CLAUDE.md, README.md, CONTRIBUTING.md, SECURITY.md, CODE_OF_CONDUCT.md — and generate leaves it that way. The three phase specs are at .specline/specs/keel-phase-{8,9,10}.md, which git recorded as renames at 97–99% similarity, so the history follows them. CONTRIBUTING's claim that everything generated lives under product/ and .specline/ is now true. Five Rust doc comments that cited the old filenames name the spec instead; the citations inside generated decisions and SPEC.md were deliberately left, being historical records. Worth reading the note: KB chose "stop mirroring them at all" and that is not something the generator can do — clearing mirror_path moves a document to the mirror's own directory rather than suppressing it, and there is no opt-out short of archiving.

  <sub>commit:a57b3a3 · test:specline generate specline --check</sub>

- **KEEL-314** The README and ARCHITECTURE still say the app cannot change a task's fields — `done`

  The README and docs/ARCHITECTURE.md now describe the app as it is: creating, commenting, closing, archiving, and moving a status, priority, kind, phase or labels, from the task screen or by dragging a card. Both also state the two refusals, which ARCHITECTURE was missing entirely — a close owes a reason, a message and evidence, and starting work is a claim that records which session holds it. Everything else in the README was checked against the code rather than reread and was already correct: thirteen artifact types, thirteen MCP tools, six crates, twenty-four CLI commands matching docs/CLI.md exactly, every path and image present, no broken internal links, no rename leftovers. The one further edit was "All of it is in product/", which was never quite true and is less so now that three more specs sit under .specline/specs/.

  <sub>commit:a57b3a3</sub>

### 2026-08-18

- **KEEL-313** A card shows the same word twice when a label repeats the kind — `done`

  Fixed on the card rather than in the data: a label is not drawn when it repeats the kind badge beside it, so `p2 · bug · bug` is now `p2 · bug`. Cleaning the rows instead would have left the card able to do it again the next time somebody tags a bug `bug`. The label is only dropped while the kind badge is actually there — with kind `task` no kind badge is drawn, so a `task` label is the only thing saying it and survives. Both cases tested, checked by removing the filter and watching the first fail.

  <sub>commit:b03083e · test:cd apps/desktop && npx vitest run src/screens/Board.test.tsx</sub>

- **KEEL-311** remove 8a 8b labels — `done`

  The phase-section labels are off all 29 tasks that carried them — `8a 8b 8c 8e 8f 8g 9a 9b 9c` — so the picker no longer offers them and 68 labels remain. They only led the suggestion list because digits sort before letters. Nothing was lost: 27 of the 29 have a milestone, the two that do not carry the section in their title, and every one of the 29 also carries `phase8` or `phase9`. Worth knowing for later: the suggestions with nothing typed are still just the alphabetically first eight, which is why this looked broken in the first place — ordering them by use is a behaviour change and a separate row if you want it.

  <sub>commit:b03083e · url:http://127.0.0.1:7654/#/projects/specline/board</sub>

- **KEEL-312** On an alias URL the app loses every task reference and the project's own words — `done`

  The app resolves a project by id, slug, name or alias, case-insensitively, the same four spellings the daemon's `resolve_project` accepts — through one `findProject` that four callers now share: the switcher's label, the milestone noun, the project key, and the command palette's key. On an alias address a task shows `KEEL-311` again and the phase noun is "Phase". Tests cover the four spellings individually and two paths through the whole app on an alias URL, all checked by reverting the fix.

  <sub>commit:72f81e2 · test:cd apps/desktop && npx vitest run</sub>

- **KEEL-310** The What’s next heading prints ’ instead of an apostrophe — `done`

  The heading prints the apostrophe. The cause was that a JSX attribute in double quotes is not a JavaScript string — it is treated like an HTML attribute, so the escape stayed as text, while the same phrase inside `crumbs={...}` four lines below was a real string and rendered correctly. Fixed by writing the character itself in all five places rather than moving the escape into braces, so the distinction cannot bite again. Two tests, each checked by putting the bug back: one asserts the rendered heading, and one scans the .tsx sources for an escape inside an attribute. Nothing else catches this class — it is valid JSX, it type-checks, it renders, and it renders wrong.

  <sub>commit:c215f0b · test:cd apps/desktop && npx vitest run</sub>

- **KEEL-309** No mutating route carries CORS, and the comment on the layer says it does — `done`

  Resolved by the second of the two the row allowed: the comments now say plainly that mutating routes are outside the CORS layer and why, and `tests/cors.rs` asserts it — reads reachable from another local origin, writes not, lookalike origins refused. Kept rather than corrected because nothing needs cross-origin writes and covering them would let any local dev server attempt one; the reasoning is B-89. The write test was checked by moving `.merge(guarded)` above the layer and watching it fail. Also recorded, because nothing said it anywhere: cross-origin reads are open to any local origin, which is what the layer was built to do — if that is not wanted it is a separate decision and the fix is to drop the layer.

  <sub>commit:bf21f50 · doc:dec_01M0B3HDB61W4VFBWJDX2PADCR · test:cargo test -p specline-daemon --test cors</sub>

- **KEEL-308** Drag a card between board columns to move its status — `done`

  Cards drag between board columns when the columns are statuses. The three that do not simply take a card each say why while the drag is happening: `done` and `wont_do` open the Close form, and `in_progress` and `blocked` are not drop targets at all. Verified in a real browser against a real daemon — a drop on review wrote `review` attributed `human` / `ui`, a drop on in_progress wrote nothing and did not even prevent-default on dragover, a drop on done opened the Close form and cancelling left the row untouched. `blocked` turned out to need a fourth answer that was not in the plan: the card can be moved but does not appear to move, so the board says what it did. B-88.

  <sub>commit:b40adf5 · doc:dec_01M0B35ABXXTCYT8MS8TQRJ2EA · test:cd apps/desktop && npx vitest run</sub>

- **KEEL-307** A task's fields cannot be changed once it exists — `done`

  Status, priority, kind, phase and labels are editable on an existing task, through `PATCH /api/tasks/{id}` — five named fields and a version, so a document body is unreachable by construction rather than by a rule. Terminal statuses still go through the Close form and `in_progress` is refused because a claim records who; leaving `in_progress` releases the claim. Verified against a real daemon on a fixture store: each field written and read back, provenance `human` / `ui` with no invented session, the change visible in the history feed, and a closed task showing its status as a badge while the rest stayed editable. The endpoint's shape and the three refusals are B-87. Board drag is KEEL-308; the CORS finding is KEEL-309.

  <sub>commit:a9b5404 · doc:dec_01M0B18PA7WYJ57E5P5HZBD712 · test:cargo test -p specline-daemon --test ui_writes · test:cd apps/desktop && npx vitest run</sub>

- **KEEL-304** adding a new label in New Task doesn't add it — `done`

  The label box creates labels now, and folds what it creates onto the one form the existing 75 already use — so a new label is one keystroke away, and typing cannot produce a second spelling of one that exists. The autocomplete half needed no code: `available` comes from the labels the loaded tasks carry, and the dialog already reloads on create. Verified in the browser end to end — typed `Data Safety` and got the existing `data-safety` rather than a twin, created `label-ergonomics` on a new row, reloaded, and it autocompleted with no create offered. The reversal of KEEL-246's no-create rule is recorded as B-86.

  <sub>commit:8cd09da · doc:dec_01M0AYBAJEBJA2Z3Y9D0BZHBDH · test:cd apps/desktop && npx vitest run</sub>

- **KEEL-300** What changed shows field writes where a person did one thing — `done`

  What changed now shows one row per action. A close reads "closed as done" instead of four rows, two of which reported a size; a claim reads "claimed" instead of three, one of which was a session id.
  
  Done in specline-core::changes as agreed, so any surface gets the same answer, and grouped on (entity, timestamp) — exact rather than a window, because the events of one write share both. The single-writer lock is what makes that safe: two sessions cannot be mid-write on one row at the same microsecond, so a group is always one session's work.
  
  Measured against a copy of the real store: 1,682 rows, and the 144 reporting a size plus the 21 showing a bare identifier are both now zero.
  
  Two rules, kept apart on purpose. Redaction already elided prose because a body was once republished into the committed changelog by the edit that removed it (KEEL-215) — a test now closes a task with a path in its message and asserts no row carries it. Identifiers are a separate readability rule, because a ULID is short and is not prose and so passed everything the safety rule asks.
  
  Verified past the unit tests: the desktop suite is not run by cargo, so it was run (293 pass, and the screen needed no edit because it renders the summary verbatim), and the daemon was proved by serving a copy of the store on another port rather than by restarting the live one mid-session.
  
  KEEL-301 carries the part left undone: the row says "milestone id changed" where it could name the phase.

  <sub>commit:9495be5 · test:cargo test --workspace · test:npx vitest run</sub>

- **KEEL-298** Cut 0.3.0 — the tool rename, the next page, and the h2 advisory — `done`

  0.3.0 is tagged, published and installed. All three targets built, the installer and attestations went out, and `latest` resolves to v0.3.0.
  
  Verified against what was published rather than the workflow's exit code, which is how the release-notes gap turned up. The live daemon serves thirteen tools with `specline_next` present and `specline_ready` absent, `specline next` works and `specline ready` still works as an alias, and doctor reports 0 problems across 14 checks.
  
  The compatibility floor moved to 0.3.0 on both halves of the handshake. A 0.2.x plugin's skill names the removed tool, which is the case `MIN_PLUGIN_VERSION`'s own comment asks the floor to be raised for. The schema did not move, so `specline update` applied without asking and nothing rewrote a row.
  
  Two things came out of the release rather than going into it. `contracts/BREAKING.md` claims its entries reach the release notes and nothing carries them there, so v0.3.0 published with the removed tool unannounced until the notes were amended by hand — that is KEEL-299. And doctor pointed out the store had never been backed up, which it now has been.

  <sub>commit:78b449d · url:https://github.com/kiritbasu/specline/releases/tag/v0.3.0 · test:cargo test --workspace</sub>

- **KEEL-297** h2 advisory turns CI red, and it is in the daemon's own serving path — `done`

  h2 is at 0.4.16 and `cargo deny check advisories` passes locally, along with both clippy configurations and all 66 suites.
  
  The lockfile diff had one thing in it I did not ask for and it is worth writing down: `errno`, `rustls` and `winapi-util` moved from windows-sys 0.61.2 down to 0.52.0. A downgrade inside a security bump looks wrong at a glance. It is cargo re-resolving against the workspace's declared rust-version of 1.89, since 0.61.2 wants newer — and those are Windows-only crates, so nothing we build on macOS or Linux changes. Reading the lockfile diff rather than the version number is the only way that would have been noticed.
  
  Also worth keeping: nothing in the repository caused this. cargo-deny reads the live RustSec database, so a newly published advisory turns main red on a push that touched only markdown. A red advisories job is not automatically the last commit's doing.

  <sub>commit:d54c784 · test:cargo deny check advisories · test:cargo test --workspace</sub>

- **KEEL-296** The orientation docs still call the read surface a Tauri desktop app — `done`

  Both say what runs now. The root CLAUDE.md and HANDOFF.md described the read surface as a Tauri desktop app; it is a local web app compiled into the daemon and served by it, which is why `specline ui` needs no Node and no second process. The Tauri intent is left to B-39, where a reader should find it.
  
  Two files, one editable here and one not: the root CLAUDE.md is the bootstrap and is deliberately ungenerated, while HANDOFF.md went in at the source and came back through `specline generate`.

  <sub>commit:7ef731d</sub>

- **KEEL-290** Dependabot flags a vulnerability in the Tauri shell, which nothing builds any more — `done`

  Dismissed on GitHub as `not_used`, with the reasoning in the dismissal comment. Zero open alerts now, so the security tab stops being permanently red over a crate nothing builds.
  
  The row assumed the fix would be a lockfile bump, and that turned out to be impossible: `tauri 2.11.5` depends on `gtk 0.18.2`, which requires `glib ^0.18`, and gtk 0.18 is the last release of the gtk3 bindings. A full `cargo update` moved nothing. Waiting for a patched glib means waiting for tauri to move to the gtk4 stack.
  
  `not_used` is accurate rather than convenient, and I checked all three legs before claiming it: `apps/desktop/src-tauri` is excluded from the workspace by name in the root Cargo.toml, `build.rs` refuses to build without `SPECLINE_DESKTOP=1`, and `src/main.rs` names neither glib nor gtk. The unsound iterator is Linux GTK code in a crate that is never compiled on any platform we ship.
  
  B-39 stands and the shell keeps its option. If it is ever un-suspended this needs revisiting, which is why the dismissal comment says so rather than just calling it unused.

  <sub>url:https://github.com/kiritbasu/specline/security/dependabot/2</sub>

- **KEEL-295** The next strip says "in an active phase" when it could name the phase — `done`

  The reason names the phase now: "in Phase 11 · waiting 5 days" rather than "in an active phase · waiting 5 days". The board strip's three rows read differently from each other and match the Phase chips on the cards below them, which is where the information had been sitting all along.
  
  Shortening happens in specline-core, not in the screen, because the CLI, the tool and the app share one string and would otherwise word it three ways. A name with no dash is left whole — cutting at a character count would read worse than the phrase it replaced — and a row with no name to hand falls back to the old wording rather than printing a gap. Both are tested.
  
  The board strip also links through to What's next, which it never did.

  <sub>commit:16706af · test:cargo test -p specline-core --lib next</sub>

- **KEEL-294** Rename ready to next everywhere, including the MCP tool — `done`

  It is called next everywhere a person or a model meets it. The page is "What's next" at /projects/:project/next, the CLI verb is `specline next` with `ready` kept as an alias, and the tool is `specline_next`. The cap is still thirteen.
  
  The contract classifier is what made this honest. It refused the change until both differences were written down in contracts/BREAKING.md — the tool removal and the CLI lines that disappeared — each with a migration and a sentence for the user. The per-tool snapshot file moved with the name, and the unknown-tool error still lists thirteen.
  
  The part that would have bitten later: the standing instructions told every future session to call `specline_ready`. That file is generated, so the edit went in at the source and came back through `specline generate`; the same pass dropped the claim that the list is ordered by what each task unblocks, which B-83 had already retired.
  
  Task rows and notes saying "ready" are left as they are. They are prose written at the time rather than anything this code generates.

  <sub>commit:aab65cc · test:cargo test --workspace · test:CONTRACTS_BASELINE=v0.2.1 cargo test -p specline --test classify</sub>

- **KEEL-291** Ready numbers 29 tasks by a ranking that has nothing to rank on — `done`

  Ready leads with a next-up of three, then groups the rest into an active phase, bugs, and everything else oldest first. The 1-to-29 numbering is gone, and so is the subtitle claiming the list was ordered by what each task unblocks.
  
  Reasons now differ between rows: "in an active phase · today · p1", "in an active phase · waiting 5 days", "a bug, in no phase · waiting 2 days". Priority shows only when it is not p2, since printing the default on every row is how the old reason came to say nothing.
  
  Ordering uses signals that cannot decay, per B-83. `unblocks` still sorts first for stores where it means something; on this one it is 0 everywhere, so the group decides and age is the last word rather than an id pretending to be a rank.
  
  Next up is the front of the same ordering rather than a second computation, because the CLI, the MCP tool and the screen have to agree and a separate rule for the lead is how they would stop.
  
  Two MCP snapshots moved, which is the contract changing where it can be seen: the digest's Next line, and the ready payload gaining `group`. A CLI test that asserted the old "nothing is blocking it" string moved with them.

  <sub>commit:ae876f1 · test:cargo test --workspace</sub>

- **KEEL-292** The session headline counts field writes instead of saying what happened — `done`

  Headlines are built from acts now. Closes lead and are named by reference, creations are grouped and named by type, notes keep their count, and the raw change count sits on the end as a suffix.
  
  "created 21 things, 117 changes, wrote 3 notes" now reads "closed KEEL-263, KEEL-264 and 12 more · filed 18 tasks, 2 decisions, 1 milestone · 3 notes · 141 changes". Checked against the real store rather than a fixture: all seven sessions this week render differently from each other, which was the whole complaint.
  
  A close is detected by its `close_reason` event, since that is written exactly once per close. `status` was the tempting marker and would have been wrong, because a claim writes one too. That meant carrying the changed field onto `Change`, and the all-projects feed reuses it for a project chip.
  
  Reviewing it caught a regression before it shipped: a session that only edits rows has no act to name and returned "nothing", which a claim-only session would have hit. It falls back to the count now, with a test that claims a task from a second session.

  <sub>commit:37fdb0e · test:cargo test -p specline-core --test changes</sub>

### 2026-08-17

- **KEEL-285** show a snackbar and indication of a new manually created task — `done`

  Creating a task now says which row it made, with a link to it: the number came back on the create response all along and the dialog discarded it. The toast host is a new primitive, published through a module-level emitter because the caller closes itself in the same breath as announcing. The other half — finding what you wrote — is a `mine` chip on the board reading `audit.created_by`, in the address like every other facet, which turns up seven human-written rows out of 286.

  <sub>commit:9481e08 · commit:5c93944 · commit:3bfc6bf · test:npx vitest run</sub>

- **KEEL-284** Why wasn't the phase 13 automaticaly updated — `done`

  Not a bug in the derivation and it is worth saying why: `shipped` is declared and `complete` is derived on purpose, because `done` and `wont_do` both close a task, so a full tally cannot mean a phase shipped. What was broken is that `complete` had nowhere to appear — the digest filtered its phase list to `active` and `blocked`, so a finished phase dropped out of every session's first call at the moment it needed a person. The digest now has a "Finished, but not declared" section that names them and says which decision is owed, reports what it cuts, and shows phases in flight by their derived state rather than the `open` in their column.

  <sub>commit:08073b0 · commit:3bfc6bf · test:cargo test -p specline-core --test finished_phases</sub>

- **KEEL-286** Rewrite the README for someone who has never seen Specline — `done`

  Rewrote the README around problem, screenshot, install, and moved the deep material into docs/CLI.md (all 24 commands and the config table) and docs/ARCHITECTURE.md (crates, storage, graph direction, the feature flag).
  
  Fixed the three things that were wrong rather than stale: the install path is now the three plugin commands with no Rust and no settings editing; the generated-files section describes what a new project actually gets, which is four files under `.specline/`, with adopted paths explained as the opt-in they are; and `doctor` is presented as the front door with `fsck` as the deeper check beneath it.
  
  Two more turned up while writing. The README said five crates where there are six, and ten MCP tools on one page where the crate listing said thirteen a page later — it contradicted itself. It also still claimed the app is read-only, which the standing contract replaced some time ago.
  
  Screenshots come from `specline fixture`, shot against Harbour rather than the fixture's own Specline project, whose spec still argues for DuckDB and Lance and would have contradicted the architecture document on the next scroll. The script that takes them is in the repository so they can actually be retaken.

  <sub>commit:23b803a · commit:9ac0f20 · commit:ef3e3f6</sub>

- **KEEL-283** Cut 0.2.1 — the store relocation fixes from the rename's own review — `done`

  v0.2.1 published and verified from the published artifacts: three archives matching their digests, the installer carrying all three refusal branches, a clean env -i install, and the fix itself proved from the shipped binary — a Keel-shaped home relocating with 55 tasks either side, and --json status on that first run producing a payload that parses with the notice on stderr. Notes written by hand. The compatibility floor deliberately stayed at 0.2.0.

  <sub>url:https://github.com/kiritbasu/specline/releases/tag/v0.2.1 · commit:5ae000e · test:cargo test --workspace</sub>

### 2026-08-16

- **KEEL-282** 124 stored documents still say Keel, and the sweep is structurally blind to all of them — `done`

  The sweep now reads the store as well as the tree, and 34 artifacts of current prose were renamed mechanically through the write path. History is excluded by name rather than allowlisted: phase plans, dated snapshots, the journal, the frozen gate and the outside review keep the name they were written with.

  <sub>test:scripts/check-rename.sh</sub>

- **KEEL-281** Finish the rename outside the repository: the checkout, the trust settings and the runner — `done`

  Checkout moved to ~/development/specline with the project's root_path; ten references fixed in ~/.claude/settings.json, including two session hooks left pointing at scripts the rename deleted and three entries telling autoMode the repository is private when it is public; runner moved to ~/.specline-runner and re-serving, proved by a green macOS CI leg from the new root.

  <sub>commit:155803e · url:https://github.com/kiritbasu/specline/actions/runs/31962080842</sub>

- **KEEL-276** Cut the first Specline release and run the install flow end to end — `done`

  v0.2.0 published and verified from the published artifacts rather than the workflow's exit code: three archives matching their digests, the installer carrying real checksums with all three refusal branches intact, both binaries in each archive, no ONNX in any of them, and a clean install under env -i on a machine with no Rust. The store migration ran from the shipped binary on a Keel-shaped home with 55 tasks either side and fsck clean. Release notes written by hand. Two defects found by running it: KEEL-279 and the earlier KEEL-277.

  <sub>url:https://github.com/kiritbasu/specline/releases/tag/v0.2.0 · commit:9ef3b5d · test:scripts/check-rename.sh</sub>

- **KEEL-275** Prove the rename: both build configurations, fsck on the moved store, and a sweep that finds nothing left — `done`

  The sweep script exists and reports nothing unexplained. It found nine misses the earlier passes could not see, all of the same shape: the old name composed at runtime or sitting behind a path separator, never a literal worth grepping for. Both clippy configurations, 1135 Rust tests, 278 interface tests, fsck clean.

  <sub>commit:5423cd9 · test:scripts/check-rename.sh</sub>

- **KEEL-270** Rename the plugin, the skills, the hooks and the background service — `done`

  Plugin, marketplace, /specline:setup, both skills, the session hook, the launchd label and the systemd unit renamed. The adopt skill's frontmatter name was the one a bare-word sweep could not reach. Skills are installed and Claude Code lists them as specline and specline-adopt.

  <sub>commit:6c5e4ef · test:cargo test -p specline --test plugin</sub>

- **KEEL-274** Retire the old install from this Mac so two binaries cannot open one store — `done`

  Old binaries, skills and receipt removed; new ones installed and the daemon runs from ~/.cargo/bin. Nothing named keel is on PATH, so the stale binary cannot find a missing ~/.keel and create an empty store in its place.

  <sub>commit:c5acb64 · url:http://127.0.0.1:7654/api/health</sub>

- **KEEL-272** Rewrite the prose by classifying every mention, not by running sed — `done`

  Nine documents renamed and proved name-only by normalising the name out of both sides and diffing whole files — empty for all nine, SPEC.md's 70 KB included. Import matches on title, so a title change turns a revise into a create; the titles moved first.

  <sub>commit:c5acb64 · test:specline generate specline --check</sub>

- **KEEL-273** Rename the GitHub repository and put the release plumbing back together — `done`

  Repository renamed in place to kiritbasu/specline; the old URL redirects and the self-hosted runner survived — CI green on all seven jobs including the macOS leg. Remote updated so the local clone does not depend on the redirect.

  <sub>commit:8bfcc62 · url:https://github.com/kiritbasu/specline/actions/runs/31956095466</sub>

- **KEEL-269** Rename the project row to Specline, and keep KEEL as the task-id key — `done`

  Name and slug are Specline; the key stays KEEL so every existing task id resolves. Added keel as an alias. Live store relocated and regenerated: 3185 rows either side.

  <sub>commit:ff4aac9 · test:specline fsck</sub>

- **KEEL-267** Rename the thirteen MCP tools and the server they answer on — `done`

  Thirteen tools, the server key, seventeen snapshots and the tool contract. Caught a base64-encoded tool name a text sweep cannot see, and stopped the sweep from rewriting the gate scorer's deliberately-old transcript fixtures.

  <sub>commit:29b0ab8 · test:cargo test -p specline-mcp</sub>

- **KEEL-268** Rename the .keel mirror directory, and migrate a repository that has one — `done`

  The mirror is .specline/, and the nine literals are one constant — the guard on pruning was one of them, and a guard that disagrees with the writer stops pruning silently. An old .keel/ is reported, never deleted.

  <sub>commit:83f7bb9 · test:cargo test -p specline-core --test generate</sub>

- **KEEL-265** Move the store to ~/.specline, and migrate an existing one on first run — `done`

  The store relocates itself once, refusing while the advisory lock is held, and moving the write-ahead log with the database. Verified against a copy of the live 9.2 MB store with a -wal present: identical row counts and document hashes, fsck clean. Backups accept either snapshot name, because a rename that broke the recovery path would break it at the worst moment.

  <sub>commit:c51c9fd · test:cargo test -p specline-core --lib relocate</sub>

- **KEEL-271** Rename the desktop app and every string a person reads on screen — `done`

  Desktop app, its 278 tests and the three design files renamed. The daemon's token header and the page that reads it moved together.

  <sub>commit:6c5e4ef · test:npx vitest run</sub>

- **KEEL-266** Rename the 27 KEEL_ environment variables, with no fallback — `done`

  All 27 environment variables renamed with no fallback, and contracts/cli.txt re-recorded through UPDATE_CONTRACTS=1.

  <sub>commit:6c5e4ef · test:cargo test -p specline --test contracts</sub>

- **KEEL-264** Rename the six crates and the two binaries — `done`

  Six crates and both binaries renamed. Found two collisions a sweep could not see: the crate keel-update shares an underscored form with the MCP tool keel_update, and the gate scorer filtered transcripts on the product name, which would have made it blind to every session ever recorded.

  <sub>commit:6c5e4ef · test:cargo test --workspace</sub>

- **KEEL-263** Scope the Specline rename and file the phase — `done`

  Surveyed every surface the name is load-bearing on and filed Phase 13 with thirteen task rows, twelve blocking edges and the decision that shapes them. KB settled four questions: the task key stays KEEL, the store migrates itself, everything else is a clean break, and the repository is renamed in place.

  <sub>doc:dec_01M05D3X5QVJ0S6B4R9BY54MAK · url:http://127.0.0.1:7654/#/projects/keel/milestones/mst_01M05CWTRS0J8D012KC1NZQK06</sub>

- **KEEL-259** Taking an update leaves "Restarting the daemon into …" on screen for ever, and keeps the old interface running — `done`

  The message now names the version it is taking — captured at the click rather than read back after the restart, which is why it rendered as a bare ellipsis — and clears, because `applying` holds the version instead of a boolean nothing reset. The larger fix is that `onApplied` reloads in App.tsx rather than refetching: the daemon serves this interface, so the browser had been running the build the update replaced. The fixed 1500ms wait is now a poll of health, so a slow restart is not reported as a failure.

  <sub>commit:df948dc · test:npx vitest run src/components/VersionFooter.test.tsx</sub>

- **KEEL-258** There is no way to ask for an update check, so finding out means waiting up to an hour — `done`

  `POST /api/update/check` and a "Check for updates" button in the footer. Same token as apply, on `spawn_blocking`, stamping the check so `last_checked_at` has one answer whoever asked. The outcome is named — `up_to_date`, `staged`, `needs_a_person`, `ahead`, `failed` — so the interface renders each case without parsing prose, and a check that finds nothing is now visibly different from a check that never ran. Refused when `KEEL_AUTO_UPDATE=0`, because `keel doctor` prints "Keel makes no network requests at all" and a button that fired one anyway would make that false. Verified against a real daemon: 401 without the token, `up_to_date` with it, the stamp appearing in health afterwards, and a 400 with its reason when checks are off.

  <sub>commit:0550eb2 · test:cargo test -p keel-daemon --test health · test:npx vitest run src/components/VersionFooter.test.tsx</sub>

- **KEEL-256** Cut v0.1.5 — three platforms, and the first release whose binaries cannot do semantic search — `done`

  Published, verified by download, and `releases/latest` now serves it. Three archives, each matching its published checksum; the Intel binary is a Mach-O x86_64, the Linux one an x86-64 ELF with a glibc floor of 2.34, and none of the three carries a byte of ONNX. The installer served by `latest` embeds the real digests and has three refusal branches rather than skipping verification. The shipped arm64 binary runs, reports 0.1.5 and schema 4, and `keel doctor` says "not built into this binary" on a real store rather than counting 52 missing vectors as a fault. Release notes replaced: `--generate-notes` produced one Dependabot PR and a compare link, because everything else reached main by direct push — so it said nothing about the only change that affects an existing install.

  <sub>commit:5137619 · url:https://github.com/kiritbasu/keel/releases/tag/v0.1.5 · url:https://github.com/kiritbasu/keel/actions/runs/31944990156</sub>

- **KEEL-252** Restore the Intel macOS and Linux release targets, now that a build without embeddings exists — `done`

  Three archives published and verified by download. v0.1.5-rc.1 built `x86_64-apple-darwin` and `x86_64-unknown-linux-gnu` for the first time — the two targets `ort-sys` had blocked since 0.1.0 — alongside `aarch64-apple-darwin`, all through the ordinary `dist` path with no second build route. Route 1 as recommended: `[package.metadata.dist] default-features = false`, so no released binary carries ONNX, confirmed by `strings` on both new binaries. The Linux one is an x86-64 ELF with a glibc floor of 2.34, which keeps the low floor `ubuntu-22.04` was chosen for. KEEL-219's tier 2 now has a Linux binary to put in the VM.

  <sub>url:https://github.com/kiritbasu/keel/releases/tag/v0.1.5-rc.1 · url:https://github.com/kiritbasu/keel/actions/runs/31941540543 · commit:2286131</sub>

- **KEEL-255** Cut v0.1.5-rc.1 to exercise the release path before a real version depends on it — `done`

  The run published three archives and all four jobs passed, including the two targets that had never produced an artifact. Verified by downloading rather than by reading the job status: both new archives match their published checksums, the Linux one is a real x86-64 ELF with a glibc floor of 2.34, the Intel one is a Mach-O x86_64, and neither carries a byte of ONNX. Marked prerelease, and `releases/latest` still resolves to v0.1.4, so nothing offers it to anyone. The rc also found the bug it was for: the snapshot version redaction did not match a prerelease suffix.

  <sub>commit:2286131 · url:https://github.com/kiritbasu/keel/releases/tag/v0.1.5-rc.1 · url:https://github.com/kiritbasu/keel/actions/runs/31941540543</sub>

- **KEEL-254** Three dependency majors need code changes: rand, ulid and sha2 — `done`

  All three on current majors, each with the test its call site was missing. `rand::fill` replaces the renamed `fill_bytes` and draws from the same ChaCha12 generator, checked rather than assumed. `Ulid::generate` replaces `Ulid::new` in the fallback nothing exercises, which now has a test. `sha2`'s output lost `LowerHex`, so the three hand-rolled hex folds became one `keel_core::hex::encode` with tests for lowercase and leading zeros — the two properties a checksum comparison rests on and the compiler does not check. `verify` covers all three outcomes now, including refusing a mismatched archive. 65 suites green with embeddings, 63 without.

  <sub>commit:5939ae9 · test:cargo test --workspace · test:cargo test --workspace --exclude keel-embed --no-default-features</sub>

- **KEEL-249** Set the public repository up the way a public repository should be — `done`

  Landed on main and green. A fork's pull request cannot reach the self-hosted runner, and neither can Dependabot's — that second half was missed first time round and cost eleven cold builds on the laptop before it was caught. Branch protection, secret scanning, push protection, Dependabot alerts and updates, private vulnerability reporting and delete-branch-on-merge are all on and verified by reading them back. SECURITY.md, CONTRIBUTING.md, a code of conduct, CODEOWNERS, a PR template and two issue forms are in the tree; conduct reports go through GitHub rather than a personal inbox, at KB's instruction.

  <sub>commit:f4a41d9 · url:https://github.com/kiritbasu/keel/actions/runs/31936141815 · doc:tsk_01M04TS1H1WTTG5MF8K7JHNBSZ</sub>

- **KEEL-220** Put embeddings behind a feature so Intel macOS and Linux can be built at all — `done`

  `embeddings` is a cargo feature on `keel` and `keel-daemon`, on by default, and `cargo tree -p keel --no-default-features` has no `fastembed` and no `ort` — which is what makes the two targets linkable. Both configurations are green locally and CI runs the second one, with the ONNX-absence asserted directly because a passing suite would not notice it. The capability is reported by `/api/health` and `keel doctor`, and `keel reembed` refuses with the reason. Restoring the release targets is split into KEEL-252: `dist build` has no per-target feature selection, so which of three routes to take is a decision, and it cannot be proved without cutting a release.

  <sub>commit:e0f2a7d · test:cargo test --workspace --exclude keel-embed --no-default-features · test:cargo tree -p keel --no-default-features -e normal</sub>

- **KEEL-250** CI has been red on main because a test asserts the machine has a Desktop folder — `done`

  Merged to main and CI is green — four checks passing, including the Linux leg that had been failing every run. The test now asserts the refusal names the project's own checkout, which is a root on every platform, rather than `Desktop`, which only exists on a machine somebody works on. The Linux-only failure is reproducible on a Mac with `HOME=/tmp/empty`, so the next one like it does not need a Linux box to diagnose.

  <sub>commit:852f3e7 · url:https://github.com/kiritbasu/keel/actions/runs/31934838843 · test:HOME=/tmp/empty cargo test -p keel-mcp --test images</sub>

- **KEEL-248** Delete the phase-10 branch on GitHub, or say why it stays — `done`

  Deleted. `git push origin --delete phase-10` — `origin` now has `main` and nothing else. Every commit on it was already in main and main was 69 ahead, so nothing was lost. Delete-branch-on-merge is now on, which makes the cleanup structural rather than something to remember.

  <sub>url:https://github.com/kiritbasu/keel/branches</sub>

- **KEEL-204** Tell people the update check phones home, and let them turn it off — `done`

  `setup.sh` discloses the hourly release-manifest request on every install — what it fetches, that it sends nothing from the store, and both ways to turn it off — and `--no-update-check` writes `KEEL_AUTO_UPDATE=0` into the service's own environment rather than a shell profile the daemon never reads. `keel doctor` reports which it is and when the last check ran. It tells rather than asks: `/keel:setup` runs through Claude Code's Bash tool with nobody on stdin, and a prompt that only fires on a TTY would be a consent path almost no install ever reaches.

  <sub>commit:f268b5a · test:cargo test -p keel --bin keel doctor</sub>

- **KEEL-171** A question or decision can be created with no prose in it at all — `done`

  A spec, decision, question or feedback now has to arrive with prose, checked in `create_with_document` before anything is prepared or written — so a refusal cannot leave the headless row that was the second route to the same state. Design is exempt, because its content is the image. The three rows the task named have bodies, each labelled as a reconstruction with its sources. The other seven are a question rather than a chore: reconstructing an accepted decision means a machine inventing KB's reasoning, which is the thing this log exists to hold.

  <sub>commit:c7f8e0f · test:cargo test -p keel-core --test composite · doc:que_01M04PW3ZCQJ37M7EC27K58HC0</sub>

- **KEEL-227** A daemon too old to check for updates says nothing, which is when you most need telling — `done`

  The daemon now reports the state of update checking as well as its result — whether checks are on, when one last completed, why it did not — and which binary it is running. The interface reads two different absences to tell how far back it is talking to: no `staged_version` at all means a daemon with no updater, and `staged_version` without `update_check` means one that checks but cannot say when. Nothing outbound and no known-latest comparison, so the "should the interface reach the internet" question stays unanswered and unblocked. Verified against the live daemon: the footer went from silence to a sentence.

  <sub>commit:0862e71 · test:cargo test -p keel-daemon --test health · test:npx vitest run src/components/VersionFooter.test.tsx</sub>

- **KEEL-137** The daemonless fallback creates an empty store instead of saying there isn't one — `done`

  `open` refuses a home with no store, naming the path and the two commands that make one; `create_or_open` is the separate half for `bootstrap` and the doctor tests that build a store to examine. Two tests: a `keel ready` fallback that fails, names the path, and leaves no file behind, and `keel fixture` still making one.

  <sub>commit:661a9be · test:cargo test -p keel --test verbs</sub>

- **KEEL-172** keel_create cannot record a metric observation through its own schema — `done`

  `metric_id`, `value` and `observed_at` are now read from `fields` as well as from the top level, and the `fields` description names them, so the path the tool documents is the path that works. `fields` no longer re-applies what the constructor already took, which would otherwise have turned a working call into an immutable-column error. Three tests: the documented form, the top-level form that already worked, and a missing metric whose refusal says to find one with `keel_search`.

  <sub>commit:4e0a0ed · test:cargo test -p keel-mcp --test argument_edges</sub>

- **KEEL-186** A running daemon blocks writes to stores it is not serving — `duplicate`

  Already fixed. KEEL-194 was filed the next day against the same symptom and closed on 2026-08-14: `/api/health` reports the store the daemon holds, and the CLI's write guard compares stores rather than presence. Verified rather than assumed — `keel --home <scratch> fixture` with the real daemon up on 7654 seeded 99 rows and exited 0, no `--force`.

- **KEEL-136** The pre-commit check reads the wrong tree from a git worktree — `done`

  The hook now passes `--repo "$(git rev-parse --show-toplevel)"`, so the check reads the tree the commit is in rather than the checkout Keel has on file. Three tests run the script itself against a real git worktree with a stub `keel` on PATH recording its argv; the worktree one fails without the flag, which is what makes it a regression test rather than a description.

  <sub>commit:464c883 · test:cargo test -p keel --test hooks</sub>

- **KEEL-217** A task can be created directly in a terminal status, skipping the rule that guards closing one — `done`

  A create into a terminal status now runs the same check a close does — reason, message, and evidence when the reason is `done` — and stamps `closed_at` and releases any claim on the way in. Held to the rule rather than refused outright, because `keel bootstrap`, `keel fixture` and adopting a finished backlog all legitimately write rows that are already closed, and `keel import` cannot do it for them. KEEL-216, the row that found this, has had its missing `closed_at` filled in.

  <sub>commit:1ab66ab · test:cargo test --workspace</sub>

- **KEEL-246** Labels should be found by typing, not by scanning ten chips — `done`

  A combobox replaces the ten chips: type and the matching labels appear, arrow keys and Enter or click to take one, chips to remove. All sixty-four are reachable — verified in the browser by picking `security` and `tooling`, neither of which the old list showed. Enter is claimed only while a suggestion is highlighted, because it is also the dialog's submit. Still existing labels only, with an empty state that says so, so it reads as a rule rather than a field that ignores you.

  <sub>commit:0b9a9f1 · test:npx vitest run src/components/LabelPicker.test.tsx</sub>

- **KEEL-244** The new-task dialog is the wrong width and asks too little — `done`

  The dead space was a fixed-width child inside a wider panel; the child has no width of its own now and fills it. Kind, phase and labels are all there, on one row, each as a default rather than a decision — kind `task`, phase the one holding the most open work, labels offered from those already in use. Two things only visible by opening it were fixed first: the phase default picked Phase 4, open since forever with nothing happening in it, and all 64 labels rendered as 64 chips.

  <sub>commit:2dc0d59 · test:npm test --prefix apps/desktop</sub>

- **KEEL-240** The interface can create a task, comment on one, and archive or close a row — `done`

  All four of KB's use cases, as endpoints and as affordances: create a task from the board, comment on one, close it with the reason the storage layer requires, archive it. Every write goes through keel-core's write path behind the token, attributed actor human and surface ui with no session id, because there is no conversation behind a button. Driven against the real store rather than asserted about — KEEL-243 was created, closed and archived from the interface, and the close was deliberately submitted empty first so the storage layer's refusal could be seen arriving in the dialog.

  <sub>commit:331e826 · commit:983941f · commit:62c0385 · test:cargo test -p keel-daemon --test ui_writes · test:npx vitest run src/lib/api.test.ts</sub>

- **KEEL-241** Hard constraint 7 says the opposite of where the product is going — `done`

  Constraint 7 now draws the line at capture versus authoring rather than at read-only-with-exceptions. The interface writes what a person does — create, comment, archive, close, move a status — through keel-core's write path, attributed human/ui, carrying the token. Authoring stays with Claude because the reasoning is the product, and the line is checkable: an endpoint that accepts a document revision is on the wrong side of it. B-78 records the reasoning, including that this is a stage rather than a permanent boundary.

  <sub>commit:eab13ae · doc:dec_01M04DBTX99VPTD5X477XWEM9F</sub>

- **KEEL-239** Anything the daemon serves has to be safe to render in a browser — `done`

  Three of the four items were already true and are recorded on the row rather than rebuilt — the CSP, nosniff, and blobs served sandboxed so an SVG is inert. The one that was not is `image_path`, which read any image anywhere on the disk: it is now confined to Desktop, Downloads, Pictures and the project's own directory, checked on the resolved path before any byte is read, so `..` and symlinks are settled as locations rather than spellings. The refusal names the folders and offers base64.

  <sub>commit:2739f1f · test:cargo test -p keel-mcp --test images · test:cargo test -p keel-mcp --lib image_roots</sub>

### 2026-08-15

- **KEEL-226** Make every artifact Claude mentions a link into the interface — `done`

  Tool results that name one artifact now carry a `url` the daemon minted from the address it actually bound. Verified against the running daemon: `keel_ready` came back with `http://127.0.0.1:7654/#/projects/keel/tasks/KEEL-239`. Types with no screen get no field at all rather than a null, and the digest is deliberately left alone because forty rows of links would cost more than the rows. The skill now tells Claude to use the url when there is one and to say the reference plainly when there is not.

  <sub>commit:d9a4252 · test:cargo test -p keel-daemon --test token · test:cargo test -p keel-mcp</sub>

- **KEEL-238** A per-session token, so a mutating endpoint knows who is calling — `done`

  The daemon mints 256 bits at startup into a 0600 file and refuses any mutating request without it in `x-keel-token`. The interface receives it in the document the daemon serves — a page served by anything else cannot read that response, which is what makes DNS rebinding harmless against the API — and the CLI reads the file. The guard is a layer over a sub-router holding every mutating route, so a later endpoint is protected by where it is registered rather than by someone remembering. Reads stay open on purpose.

  <sub>commit:3eeb858 · test:cargo test -p keel-daemon --test token · test:cargo test -p keel-core --lib token</sub>

- **KEEL-237** Board search cannot find a task by the identifier everyone uses for it — `done`

  The board's text filter now matches the reference and the bare number as well as the title and body. Verified against the real store: `?q=KEEL-168` and `?q=168` both return KEEL-168, where both returned nothing before. Four tests cover it, including that a reference belonging to no task still comes back empty.

  <sub>commit:dfc39b4 · test:npx vitest run src/lib/filters.test.ts</sub>

- **KEEL-236** Release 0.1.4 — the update restarts the daemon, and the tracker is readable again — `done`

  Built, published and taken. `keel update` moved this machine 0.1.3 → 0.1.4, restarted the daemon itself, and reported the version that came back — the first time that path has run against a published release rather than a scratch one. Tier 1 against the downloaded artifacts is 14 passed, 0 failed, and the installer's embedded digest matches the published archive byte for byte.

  <sub>commit:de365ce · url:https://github.com/kiritbasu/keel/releases/tag/v0.1.4 · test:scripts/verify-release-tier1.sh</sub>

- **KEEL-235** The interface shows a version but gives no way to find out what is in it — `done`

  The version in the footer is now a link to that release's notes, and a staged update carries a "What's in it" link beside the restart offer. Both URLs are minted by the daemon and arrive on health, because the repository is configurable and a template in the frontend would only be right for the default. A missing URL falls back to plain text rather than a dead link, and never takes the update offer down with it.

  <sub>commit:2c6b2c4 · test:npx vitest run src/components/VersionFooter.test.tsx · test:rustup run 1.97 cargo test -p keel-update</sub>

- **KEEL-234** A dev install and a release install land in different directories, and one shadows the other — `done`

  A dev install now writes exactly where a release writes — `CARGO_HOME`, falling back to `~/.cargo/bin` — across the installer, the setup script, the session hook and both READMEs. `~/.local/bin` is off every search path because no release has ever written there. The four stray copies on this machine are gone, `keel` and `keel-daemon` both resolve to the one location, and `install.sh` now warns when the binary it installed is not the one PATH will run.

  <sub>commit:4a1b571 · test:rustup run 1.97 cargo test --workspace</sub>

- **KEEL-232** A release is a task, so cutting one shows on the board and in the changelog — `done`

  The contract now says a release is a task: created and claimed before the tag is pushed, labelled `release`, closed with the tag's commit and the published release URL as evidence. KEEL-233 is 0.1.3's row, written after the fact and saying so, so the release appears in the changelog with everything else. 0.1.2 was deliberately left alone — it was cut in an earlier session and a row for it now would be an invented record.

  <sub>doc:spc_01KZKSME2TCPVARX9M04836XD6 · doc:tsk_01M03NFEJ3XY8P0R74AH4NCSE2</sub>

- **KEEL-233** Release 0.1.3 — the installer verifies what it downloads — `done`

  Built on the self-hosted runner and published. The release job's new step reported the digest before publishing, and it holds against the published assets: the installer's `_checksum_value` is the sha256 of the archive beside it. Tier 1 against the downloaded release was 14 passed, 0 failed, including a corrupt-archive refusal that now only passes on `checksum mismatch`.

  <sub>commit:ed44e63 · url:https://github.com/kiritbasu/keel/releases/tag/v0.1.3 · test:scripts/verify-release-tier1.sh</sub>

- **KEEL-231** STATUS.md renders current state, and closed work gets its own changelog — `done`

  STATUS.md went from 488KB to 58KB and now carries open work only; CHANGELOG.md beside it carries the 207 closed rows with their reason, close message and evidence, plus the event table that used to sit at the bottom of the tracker. The tracker states the count it left out, so nothing is dropped silently. Its path is derived from status_path rather than being a fourth column, because a column means a schema migration and a migration is a version the updater will not apply without a person.

  <sub>commit:ff1ec63 · test:cargo test -p keel-core --test generate</sub>

- **KEEL-229** Work that arrives mid-session with no row never reaches the board — `done`

  `keel_create` then `keel_claim` is now the first bullet of the skill's claiming section, a rule in the standing contract's tracker discipline, and part of session-ritual step 4. All three carry the measurement, because the two earlier ones are what turned claiming from a request into a tool. The skill was reinstalled to `~/.claude/skills/keel` so the edit is not inert.

  <sub>commit:3a5efaa · doc:spc_01KZKSME2TCPVARX9M04836XD6</sub>

- **KEEL-230** keel update leaves the daemon running the old version and tells you to fix it yourself — `done`

  `POST /api/update/restart` on the daemon, called by `keel update` and `keel update --rollback`. The CLI asks, then polls health and reports the version that came back — including the case where it comes back unchanged, which means the daemon is running from a directory the update never touched. The plugin README now covers updating at all, which it did not.

  <sub>commit:7cebaf0 · commit:c85cdaf · test:cargo test -p keel-update · doc:dec_01M03KQE9V0G9VSZMPKTWHB171</sub>

- **KEEL-228** The shipped installer verifies nothing, and the check that should have caught it passed — `done`

  The build job now writes a per-target dist-manifest, which is how the archive's sha256 reaches the installer `dist` generates — its absence is why 0.1.2 shipped verifying nothing. A new release step reads the digest out of the built installer, hashes the archive about to be published and compares, so a release whose installer would print "no checksums to verify" fails before it is published. The installer itself now refuses rather than skipping when it has no checksum, and both verification tiers stop scoring that wording as a passing integrity check.

  <sub>commit:7af8c6d · test:cargo test -p keel --test installer_checksum --test installer_embedded_checksums</sub>

- **KEEL-225** Ask before restarting into an update, and check often enough to matter — `done`

  Nothing applies itself any more. The daemon checks hourly instead of daily, with `KEEL_UPDATE_INTERVAL` in seconds and a 60-second floor; it still fetches, verifies and stages, and then stops. Startup reports what is waiting rather than swapping it in. `/api/health` carries `staged_version`, which the daemon already knew and kept to itself, so the interface learned about updates without a new endpoint. The rail footer names the running version always, and offers to restart into a staged one — the single write B-75 permits, sending no body, so it can only apply what the daemon itself chose.
  
  A short interval became cheap exactly because finding an update stopped meaning taking one: it costs a request and a staged file rather than a surprise restart, which is what made a day look like the safe number in the first place.
  
  Two things found by running it rather than reasoning about it. The live daemon reported 0.1.0 — two releases behind what is installed — which is precisely the invisible state this was built to end, and it showed up in the first screenshot. And the app had no `post` helper at all, because it had never written anything; the new one carries a note saying a second caller means constraint 7 moved again.
  
  Not covered, and worth knowing: the restart itself has not been exercised end to end, because nothing newer than 0.1.2 exists to stage. The refusal path is tested (an apply with nothing staged returns 400 rather than bouncing the daemon, which is the only way an argumentless endpoint can be wrong), and the apply path is the same `apply_staged` proven against a scratch install in KEEL-203. The first real one is 0.1.3.

  <sub>commit:2703778 · commit:cbf881d · commit:bbbe8da · test:cargo test --workspace · doc:dec_01M03EVSQZBVB93NR94MYNTKWB</sub>

- **KEEL-224** The plugin skill never mentions claiming, so tasks stay in todo on every project but this one — `done`

  The skill now leads with claiming — `keel_ready` to choose, `keel_claim` before the first edit, `keel_close` with evidence, `keel_note` for what was found — ahead of everything about recording, because work being visible while it happens is the half that was broken. The tool table gained the three missing verbs and lost the count in its title. The description gained "let's build", "start on", "work on" and "implement", since the session that prompted this read as a build request and may never have tripped the skill at all.
  
  Two tests guard it, both confirmed to fail on the regression they describe before being kept: every tool in `tools::all()` must be named somewhere in the skill, and no heading may count them. Prose read by a model has no compiler, which is how "The nine tools" outlived two additions.
  
  Not fixed here, and worth being clear about: the tasks already worked on that Mac have no start times and never will. Events are immutable, so there is nothing to backfill — those tasks have to be closed by hand with commits as evidence.

  <sub>commit:5401f6d · test:cargo test -p keel --test plugin</sub>

- **KEEL-203** Update the binary without asking, but only when it cannot touch the store's shape — `done`

  A compatible release now downloads, verifies against the manifest checksum and applies itself with one log line; a release that moves the schema stops and says what it would change; and `keel update --rollback` puts back the binaries either path replaced. The daemon checks daily, stages, and swaps at its next startup — staged rather than in place, because a daemon that replaces its own executable and carries on is running code that is no longer at its own path. `KEEL_AUTO_UPDATE=0` turns the check off, which is KEEL-204's but shipped here rather than after it.
  
  One thing is deliberately unproven: no release has ever carried a manifest, because v0.1.1 predates the job that publishes one. The download, checksum and unpack chain was checked against the real v0.1.1 bytes by hand, and the staging and startup swap end to end against a scratch install on a spare port — but the release workflow's two new steps run for the first time on the next tag. Worth watching that run rather than assuming it.

  <sub>commit:4fd07d3 · commit:7385ed7 · commit:e5bae30 · test:cargo test --workspace · doc:dec_01M02ZT12E0A8RJZ050SJPKMB3</sub>

- **KEEL-223** The sidebar's keyboard shortcuts read as counts, and a new user reported them as a bug — `done`

  The rail's shortcut hints now read `·6`, `·7`, `·8`. A leading middle dot costs one character, cannot be a quantity, and matches the header's existing `Jump to… ⌘K`, so the vocabulary was already on the screen. They are also `aria-hidden` — the label names the destination and "middle dot seven" read after it is noise. Three tests in App.test.tsx: no rail hint may be a bare number, the key is still named, and they stay hidden from assistive technology.
  
  Fixed alongside the theme control's target size, which was 58×19 CSS pixels and is now 58×32. That passes WCAG 2.2's 24×24 minimum, which 19 did not — the 44px figure quoted when this was raised is Apple's touch guideline and does not apply to a pointer-only desktop surface, so the claim is narrower than first stated.

  <sub>test:npx vitest run src/App.test.tsx · commit:pending</sub>

- **KEEL-208** Build releases for three targets, and fix the installer's silent checksum skip — `done`

  The pipeline exists and has produced a release: one CI run, artifacts with embedded checksums, and the installer's silent checksum skip fixed and verified in the shipped file. Tier 1 passes 14 of 14 against the downloaded release, including refusing a corrupted archive on the checksum.
  
  Two parts of this row's criterion are deliberately not met and are tracked rather than quietly dropped. It asked for **both Macs and Linux**: 0.1.0 ships arm64 macOS only, because `ort-sys` has no Intel macOS prebuilt and the Linux one wants a newer glibc than the runner has — KEEL-220. And it asked for **build provenance**: GitHub does not offer attestations on a user-owned private repository, so the step is conditional and the release notes say provenance is absent.
  
  Closing rather than leaving open, because the pipeline is the deliverable and it works; what remains is a platform decision and a consequence of B-72, neither of which is this task.

  <sub>url:https://github.com/kiritbasu/keel/releases/tag/v0.1.0 · test:./scripts/verify-release-tier1.sh /tmp/relcheck · commit:2c80879</sub>

- **KEEL-209** Ship the plugin: a marketplace entry and a setup command that runs one tested script — `done`

  The marketplace entry, the `/keel:setup` command and the script it runs are built and covered by six tests, and 0.1.0 is published for Apple Silicon macOS. Tier 1 passes 14 of 14 against the artifacts downloaded from the release itself, including corrupting a real archive and watching the installer refuse it on the checksum. Two things were found by running it rather than reading it: the site build was resolving `@types/node` from a directory outside the repository, and artifact attestations turn out to be unavailable on a user-owned private repository.

  <sub>url:https://github.com/kiritbasu/keel/releases/tag/v0.1.0 · test:./scripts/verify-release-tier1.sh /tmp/relcheck · commit:4ff0890</sub>

- **KEEL-206** Move the hooks into the binary, leaving one shim that can report the binary is missing — `done`

  The logic is `keel hook session-start` and `keel hook stop`, with 317 lines of bash replaced by 48 lines of POSIX `sh` that shells out to nothing — so `python3` and `curl` are no longer undeclared dependencies of starting a session. Seventeen tests execute them, including one that runs the binary with nothing on PATH. Running them found two defects reading them could not: the hook exited 1 when HOME was unset, and deleting the old scripts broke every hook on a machine whose settings.json still named them, which is now handled with forwarders.

  <sub>test:cargo test -p keel --test hooks · test:cargo test -p keel --bin keel hook::</sub>

- **KEEL-207** Serve the read surface from the daemon instead of a dev server — `done`

  The daemon compiles the built site in with `rust-embed` and serves it from the port it already had, and `keel ui` opens it — finding the daemon through the address it recorded, so a non-default port still works, and refusing rather than opening a browser at a dead one. Verified against the live store in a browser. The served HTML carries the content security policy KEEL-168 asks for, with tests holding the clauses; that task's remaining item, a token on the mutating `/api/generate`, is untouched and stays open.

  <sub>test:cargo test -p keel-daemon --lib site:: · test:cargo test --workspace</sub>

- **KEEL-210** Build the two release checks that can actually be run on the hardware we have — `done`

  Both checks are built. Tier 1 has run against real locally built artifacts and passes 14 of 14, including the two claims the phase rests on — no quarantine attribute on a curl download, and a corrupted archive refused on the checksum. Running it found three defects reading it could not: the release would have failed at the build step for a missing cargo profile, tier 1 was orphaning a daemon onto the scratch port, and the sha256sum check warned about its own fix. Tier 2 is written and refuses to run anywhere but Linux; its first run needs the VM and is KEEL-219.

  <sub>commit:53ce7fc · test:./scripts/verify-release-tier1.sh target/distrib</sub>

- **KEEL-218** Two tests read the machine instead of the code, and the first real CI run found both — `done`

  Both tests now assert behaviour rather than which machine they landed on: the contract check clears the environment before reading CLI help, and the installer test builds a path holding only `awk` instead of borrowing the platform's. CI is green on both legs, with the macOS one running on the self-hosted Mac and Linux on a hosted runner — the first time the two have agreed, because the Linux leg had never executed before.

  <sub>commit:26bf398 · url:https://github.com/kiritbasu/keel/actions/runs/31857807506</sub>

- **KEEL-215** The changelog reprints old field values, so nothing can ever be redacted from the mirror — `done`

  Event summaries now quote a field's value only when it is short and not prose; longer or prose values are reported by size. Applied at write time so new events are clean, and again at render time through `Event::publishable_summary`, which rebuilds the line from the stored field and values — that second half is what covers the events already in the log, since they are immutable and could not otherwise be fixed. Verified end to end: the machine path that prompted this is gone from the whole tracked tree, and the changelog line now reads `body (1237 characters) → (1223 characters)`.

  <sub>test:cargo test -p keel-core --lib event:: · doc:dec_01M01F8R621R79SSKGCV4D4G34</sub>

- **KEEL-216** The contract gate failed every day for a calendar reason, not a code one — `done`

  The contract descriptions now redact bare calendar dates before hashing, the way they already redacted ids, so the demo corpus dating itself relative to today no longer changes six hashes a day. Three tests, including one asserting directly that the same line a day apart redacts identically.

  <sub>test:cargo test -p keel --test contracts</sub>

### 2026-08-14

- **KEEL-202** Publish a release manifest, and let the daemon say which store it is holding — `done`

  `keel release-manifest` prints version, schema version, minimum plugin version and protocol as JSON without opening a store, so a release job can publish it beside the artifacts and the updater can read it before downloading anything. `/api/health` already reports the store it holds and its minimum plugin version; the plugin manifest now carries `min_daemon_version` as the other half of the handshake. Checksums stay with the release job, since a binary cannot state the hash of a file it has never seen.

  <sub>commit:f7b9e84 · test:cargo test --workspace</sub>

- **KEEL-205** Fail loudly on a taken port, and refuse to bind anywhere but loopback — `done`

  The daemon refuses a non-loopback bind unless `--allow-network-access` says otherwise, fails on a taken port with a message naming `--bind` and `KEEL_BIND` rather than wandering to another one, and records the address it bound in `~/.keel/daemon.json` — which the CLI reads and then probes, because a SIGKILL leaves the file behind. Three tests on the bind decision; the port and endpoint behaviour verified end to end.

  <sub>commit:9a64293 · test:cargo test -p keel-daemon --bin keel-daemon</sub>

- **KEEL-199** Refuse a release whose breaking changes nobody wrote down — `done`

  A breaking difference now has to appear in `contracts/BREAKING.md` with its migration and the sentence the user reads, or CI fails. It fails in both directions — an unacknowledged break, a stale entry describing nothing that changed, and a blank field each get their own complaint. The Breaking section of the release notes is generated from the entries. Verified four ways against real git history: unacknowledged fails, acknowledged passes, reverting the code while leaving the entry fails as stale, clean passes.

  <sub>commit:dc9f129 · test:CONTRACTS_BASELINE=HEAD cargo test -p keel-cli --test classify</sub>

- **KEEL-198** Sort every contract difference into additive or breaking, and fail closed on the rest — `done`

  A release diff now comes back sorted against the §5.2 table, with anything unplaceable treated as breaking. Fourteen tests, all on synthetic before/after pairs so the asymmetric cases get both directions — narrowed against widened enum, added against removed argument, new NOT NULL with and without a default. Verified against real git history too: simulating a removed tool and a newly required argument produced exactly those two breaking findings, and a clean tree produced none.

  <sub>commit:a228ead · test:CONTRACTS_BASELINE=HEAD cargo test -p keel-cli --test classify</sub>

- **KEEL-213** The fuzz smoke job fails, and nobody has ever read why — `done`

  It was the harness, not a finding. `install-action` has no recipe for cargo-fuzz so it falls back to cargo-binstall, which fetches a musl-linked build; cargo-fuzz then took its default `--target` from its own binary's triple and tried to build for musl on a gnu runner, where ASan refuses statically linked libc and the musl std is not installed. Naming the host triple from `rustc -vV` fixes it. The job now runs all three targets for 61 seconds each — 19.7 million executions in total, no crashes — and every CI job is green.

  <sub>commit:e51f2cc · url:https://github.com/kiritbasu/keel/actions/runs/31832754944</sub>

- **KEEL-195** Create the remote and make CI run for the first time — `done`

  CI runs, and `fmt · clippy · test` is green on both ubuntu-latest and macos-latest — along with snapshots, coverage and licences. The toolchain is pinned in `rust-toolchain.toml` so the local gate and CI now ask the same question. The one job still red is `fuzz smoke`, which was failing before any of this and is filed separately as KEEL-213; it runs only on a schedule or a manual dispatch, so it gates nothing day to day.

  <sub>commit:d5ba558 · commit:aef9b39 · url:https://github.com/kiritbasu/keel/actions/runs/31830331548</sub>

- **KEEL-197** Make every surface describe itself into contracts/, and fail CI when it drifts — `done`

  The schema, the tool surface, every subcommand's help and the generated markdown all emit descriptions into one checked-in `contracts/` directory, and the test fails when the code and the recording disagree. `UPDATE_CONTRACTS=1` re-records; `settle()` refuses to write under `stores/` at all. Verified with a control: a simulated new required argument on `keel_context` fails the gate, and restoring goes green.

  <sub>commit:2e039a6 · test:cargo test -p keel-cli --test contracts</sub>

- **KEEL-201** Assert the two cross-version behaviours that exist but are not tested — `done`

  The forward guard now has three assertions naming it directly — read path, migrate path, and the daemon's exclusive entry point — and the backup round trip has a sibling that crosses a schema boundary: back up looking like an older release, restore and migrate forward with the current binary, assert the manifest matches, fsck is clean and no rows were lost.

  <sub>commit:3a6d844 · test:cargo test -p keel-core --test migration --test fixture_backup</sub>

- **KEEL-194** A write to one store is refused because a daemon is serving a different one — `done`

  `/api/health` now reports the store it is holding, canonicalised, and the CLI's write guard compares stores rather than presence. A daemon serving `~/.keel` no longer refuses a write to `/tmp/scratch`. Only positive evidence of a different store permits the write — a missing field, a timeout or unparseable JSON all still refuse, because refusing wrongly costs one `--force` and permitting wrongly costs a second writer.

  <sub>commit:79168c5 · test:cargo test -p keel-cli</sub>

- **KEEL-196** Adopt Apache-2.0 and clear the things that assume Keel is unpublished — `done`

  Apache-2.0 is in the tree and in the metadata, and `deny.toml`'s `private.ignore` exemption is gone. `cargo deny check` passes all four sections.

  <sub>test:cargo deny check · commit:pending-phase-10</sub>

- **KEEL-193** Prove the contract surfaces emit deterministically, before anything is built on them — `done`

  Nine of ten contract surfaces emit identically across 100 runs against a fixed fixture store. The tenth, `keel generate`, differs on every run for one reason — two wall-clock timestamps, the per-file `keel:generated` banner and the manifest's `generated_at` — and is stable at 1 hash across 100 runs once normalised the way `keel generate --check` already normalises it. Phase 10 §5 proceeds, with banner normalisation as a stated requirement on the emitter rather than an assumption.

  <sub>test:N=100 bash scratchpad/determinism.sh · commit:deeee69</sub>

- **KEEL-192** The Stop hook nags in projects Keel has never heard of — `done`

  The Stop hook now resolves `cwd` through `/api/context` before it asks whether the session wrote anything, and exits silently when no project matches. A session in a repository Keel has never heard of is no longer told it failed to record work about it.

  <sub>commit:fd25d47 · test:printf '{"session_id":"x","stop_hook_active":false,"cwd":"/Users/h8hcn"}' | ./plugin/hooks/stop.sh</sub>

- **KEEL-191** Nothing watches the write-ahead log, and doctor alone would not fix that — `wont_do`

  KB's call: stay on SQLite's defaults and add nothing. No WAL monitor, no `journal_size_limit`, no background checkpoint loop.
  
  The reasoning holds up. `wal_autocheckpoint = 1000` handles the ordinary case, `await_holding_lock = "deny"` already forbids the coding mistake most likely to pin a snapshot, and the daemon checkpoints with TRUNCATE on clean shutdown. The failure this task was about needs a reader that never releases, which nothing has been observed doing — the live store's log was 3 MB against a 7.2 MB database, which is the mechanism working. Adding a background timer to watch for something that has not happened is what the scale-discipline rule exists to stop.
  
  Nothing is lost by closing it. The four notes carry the measurements — PASSIVE costs 7 ms against 1.32 µs for a stat, a checkpoint never truncates the file but the next write after one does, and a pinned snapshot shows as `checkpointed == 0` while `busy` stays 0. If a `-wal` is ever found larger than the store beside it, the diagnosis is already written down and the reopening is cheap.

### 2026-08-13

- **KEEL-190** Close the raw-SQL leak in doctor and the duplicated row readers in entity.rs — `done`

  `clock_sanity` now calls `latest_event_id()` instead of running its own `SELECT max(id) FROM events`, so keel-cli writes no SQL and the `Store::connection()` exception is back to the two in-crate users its doc comment defends. `entity.rs` dropped its private copies of `col_err`, `get_ts` and `get_ots` in favour of the ones in `rows.rs`, leaving one timestamp reader rather than two.
  
  916 tests pass, clippy clean with `-D warnings`, behaviour-preserving throughout.
  
  The file-size half of the second finding is deliberately not done. `entity.rs` is 3,118 lines, of which 1,185 are its test module, and splitting the remaining ~1,900 into two files would relocate the concepts rather than reduce them — which is the thing the review standard warns against. What was reducible was the duplication, and that is gone. The original finding stands as written: decompose before adding, rather than as a refactor to do now.

  <sub>commit:40ed562 · test:cargo test --workspace</sub>

- **KEEL-189** A passing test run leaks temp stores too, which is not what B-58 was decided on — `done`

  Fixed at the source. `generate.rs`'s `fixture()` was `Box::leak`ing its `TempDir`, and nineteen tests call it — which is the whole of the 19 stores a passing workspace run left behind. It now returns the `TempDir` and each caller binds it as `_home`, so it drops with the test.
  
  A full `cargo test --workspace` now leaks zero, measured either side. 916 tests pass, clippy clean with `-D warnings`.
  
  B-58 does not need re-deciding after all. It rejected a repo-local TMPDIR on the grounds that only killed processes leak; that was false when written, and this makes it true. The task asked for either a fix or a re-decision, and fixing the leak is what makes the existing decision correct rather than merely lucky.
  
  Left alone deliberately: the `Box::leak` calls in `store/schema.rs` and `writes.rs`. Those leak a `&'static str` of a few bytes once per process and touch no disk.

  <sub>commit:7f667db · test:cargo test --workspace</sub>

- **KEEL-188** Sweep the tree for dead code and files that outlived their phase — `done`

  Removed seven functions nothing called, three files and one crate. In keel-core: `MirrorReport::is_noop`, `mirror_root` and `Document::same_content_as`. In keel-mcp: `open_task_statuses` and `urgent_priorities`, both thin wrappers over enum methods. In the app: `inListOrder` and `useNavigate`. Files: `START-PHASE-8.md` and the one-off `scripts/decompose-logs.py`; `RESET-PLAN.md` moved into Keel as a document first, then deleted. The `keel-github` stub crate went with KB's agreement, reasoned in B-63.
  
  Also corrected two comments that had gone stale: `tools.rs` said the surface is ten tools when thirteen are registered, and the `CLOCK` mutex in `doctor.rs` still carried the old doc paragraph saying only clock-related tests need the lock — the exact belief that caused KEEL-179, sitting directly above the correction that KEEL-179 added.
  
  Clippy clean with `-D warnings`, `cargo fmt --check` clean, 915 Rust tests and 225 app tests passing, `tsc --noEmit` clean.

  <sub>test:cargo test --workspace · test:npx vitest run · doc:dec_01KZYFS0PJY5RPXCMN15GC2AS7</sub>

- **KEEL-185** The daemonless work verbs are fixed but have no tests, so nothing would catch the regression — `done`

  Narrower than written, because the premise was wrong: `crates/keel-cli/tests/verbs.rs` already covered the daemonless path with nine tests driving the real binary at a closed port, and I had only checked `work.rs` for inline tests. Reintroducing the original regression showed where the real gap was — removing the payload unwrap from the read path fails two existing tests, and removing it from the write path left all nine green. That is the quieter half of the original bug: `claim` and `close` fall back to echoing their argument, so the line still reads like success while naming the ULID that was typed. Two tests added, one per verb, passing the raw ULID so a readable `HARB-n` can only have come from the response. Both verified to fail with the unwrap removed. 912 tests, clippy and fmt clean.

  <sub>commit:0940f39 · test:cargo test -p keel-cli --test verbs · test:cargo test --workspace --no-fail-fast</sub>

- **KEEL-179** A doctor test fails only in a full suite run and passes on its own — `done`

  Diagnosed and fixed, and the diagnostic added this morning is what found it — the panic named the check instead of asserting a bare boolean, and it was the clock one all along. `an_event_id_from_the_future_is_a_problem` writes a ULID an hour ahead; opening that store primes the process-global monotonic id generator to match, which is right in production and leaks between tests. It resets afterwards, and that is not sufficient: a test running inside that window mints future ids into its own store, and the next open of that store re-primes the generator after the reset. Three of the nine tests calling `examine` held the serialising lock and six did not, which is why it failed about one run in three and always passed alone or under a `doctor::tests` filter narrow enough to exclude the culprit. All nine hold it now. Twelve consecutive clean runs of the suite that previously failed two in seven.

  <sub>commit:87c51c5 · test:cargo test -p keel-cli --bin keel · test:cargo test --workspace --no-fail-fast</sub>

- **KEEL-183** Write the adoption workflow: how a session backfills an existing project without flooding it — `done`

  A `keel-adopt` skill, installed alongside the everyday one rather than folded into it — that one loads in every project conversation and this runs once per project. It leads with the failure mode being enthusiasm rather than laziness, argues it through the digest budget that every future session pays, and asks to be judged on what it left out. Run against `~/development/data-coworker`: 63 markdown files, 129 commits, not a Keel project. Stopped at the proposal because that is what the workflow says to do, so nothing was created there. The survey found what a parser would miss — nine tidy ADRs in `decisions/`, plus more decisions buried in `BACKLOG.md` task notes with their reasoning attached — and that went back into the skill as its own instruction. Also recorded what the test did not show: the repository had one `TODO` comment in total, so the rule about stale ones was never exercised.

  <sub>commit:9af83d7 · doc:tsk_01KZYATHMWX2P8QFT8J70KEK4X · url:file:///Users/h8hcn/.claude/skills/keel-adopt/SKILL.md</sub>

- **KEEL-182** keel import cannot be previewed, and a bad import cannot be taken back — `done`

  `keel import --dry-run` reports per file whether the import would create, revise or do nothing, what the artifact would be called, and the adopted path when it would change — the last only when it changes, because that is the surprise worth flagging. The resolution is shared with the real import rather than reimplemented, and the "would anything change" answer comes from building the same `Document` and comparing its `body_hash`, so the preview and the import cannot disagree. `preview` takes `&Store`, so it cannot write by accident, needs no lock, and runs against a store a daemon is already serving. Also closed a hole found on the way: `run_import` opened the store directly, making it the one write command that went round both the probe and the lock, in the command whose own docs say to stop the daemon first — it now uses `open_for_write`, so import refuses while a daemon runs unless forced. The first real run found a one-byte round-trip difference on `product/SPEC.md`, filed as KEEL-184.

  <sub>commit:9bb5c47 · test:cargo test -p keel-cli --bin keel preview_tests · test:cargo test --workspace --no-fail-fast</sub>

- **KEEL-180** Take an advisory lock when opening the store for writing — `done`

  Opening the store for writing takes an advisory lock now — the daemon for its lifetime, a CLI command for its duration, and `keel migrate` while it changes the tables. Reading takes nothing, so `doctor`, `fsck` and the app still work against a store the daemon is writing to. `--force` skips the lock as well as the probe, because that flag exists for a wedged daemon and one the lock could veto would fail when it is needed. The SIGKILL claim TQ-36 turned on is proved through `Store::open_exclusive` rather than a lookalike: a child takes the claim, is killed, and the store is claimable again with nothing to clean up. Verified live by repeating the original mistake — `keel-daemon --bind 127.0.0.1:7699 --embeddings` with `--home` forgotten now exits saying another process has the store and suggesting `--home`, instead of migrating it under the daemon already serving. 905 tests, clippy and fmt clean.

  <sub>commit:0145e48 · test:cargo test -p keel-core --test single_writer · test:cargo clippy --workspace --all-targets -- -D warnings</sub>

- **KEEL-181** The app never refetches when the live feed reconnects, so a daemon restart leaves it stale — `done`

  The stream's `open` event now triggers a refetch, so a reconnect catches up on everything written while the connection was gone instead of waiting for an unrelated write to arrive. The shell also shows a line when the feed is down — until now a stale page and a current one rendered identically, which is the failure this project spends most of its effort avoiding. Both were checked against a negative control: disabling the `open` handler makes the reconnect tests fail on the assertion they are about. The loose end about notes is closed too, and it was nothing: an unbuffered probe shows `{"kind":"note","summary":"keel_note completed"}` arriving on the stream, so the first probe's silence was curl buffering rather than a gap in TQ-29. Not verified in a browser — port 1420 is held by another session's dev server, and taking it or rewriting the shared launch config was worse than letting the tests be the evidence.

  <sub>commit:dda327b · test:npx vitest run --prefix apps/desktop · test:npx tsc --noEmit</sub>

- **KEEL-176** fsck and doctor should prove the passage index matches the store — `done`

  Three checks now answer whether the passage index still describes the store, using the `body_hash` carried on each passage row. `document_without_passages` is a warning — search is degraded, not wrong. `stale_passage` is an error, because a passage whose revision was superseded or edited is returned as current and the ranking gives no sign. `passages_from_mixed_models` is neither: it is the ordinary state during a model change, worth reporting only because vectors of another width are skipped by search rather than failing it, so those rows stop being findable quietly. `doctor` gained a `passage_index` check that reads all three out of the fsck report rather than querying again — the duplicate-query mistake it made with embedding coverage earlier today. Every check has a corruption test that trips it and asserts the store was quiet beforehand. Against the live store: 40 checks, only the two known warnings, and every passage matches the revision it was built from.

  <sub>commit:b754662 · test:cargo test --workspace --no-fail-fast · test:keel fsck · test:keel doctor</sub>

- **KEEL-131** Semantic search has never run — there are 227 documents and no embeddings — `done`

  Semantic search is running on the live store for the first time. 128 documents became 566 passages, 849 KB of vectors, embedded in 29 seconds once the model was cached; `doctor` now says "all 128 current document(s) have a vector". The daemon runs with `--embeddings` so new revisions get passages on the way in, and its log says "semantic search is live" rather than nothing at all. The exit criterion holds against real data: "why is the first startup slow" returns a decision about DuckDB and Lance coming out, sourced `semantic`, sharing no words with the query, and "how do we stop two writers corrupting things" puts TQ-36 on the single write path at rank one.

  <sub>commit:9948ddf · test:keel doctor · test:curl 'http://127.0.0.1:7654/api/search?query=why+is+the+first+startup+slow'</sub>

- **KEEL-174** Long documents are embedded on their first 1,700 characters and nothing says so — `done`

  Documents are split into passages and each one is embedded, so nothing is truncated away any more. Markdown sections first, then a 1,400-character wrap with 15% overlap, with the heading path carried into the embedded text so a passage from deep inside a spec still says what it is a section of. Vectors moved to `document_chunks` and `documents.embedding` stopped being written; two triggers delete passages when a revision is superseded or its entity archived. Search takes the best passage per document, and the excerpt is now cut from the passage that matched rather than from the top of the document. Measured on the live corpus: 128 documents become 566 passages, none over the model's window, for 849 KB — the technical specification goes from one truncated passage covering 2.5% of it to 69 covering all of it.

  <sub>commit:9948ddf · test:cargo test --workspace --no-fail-fast · test:cargo clippy --workspace --all-targets -- -D warnings</sub>

- **KEEL-178** The B-57 commit is red: 28 tests still set a milestone status that is now derived — `done`

  Already fixed when I picked it up — `3218c12` landed after the task was written and updated every fixture that still declared a milestone status. I verified rather than changed anything: 868 tests across 55 suites, clippy and fmt clean, and all three previously-failing groups now pass. The concern about stored rows was unfounded; migration 3 rewrites `planned`, `active` and `blocked` alike, and applying migrations 2 and 3 to a copy of the live store took unparseable statuses from 3 to 0 and archived rows in the keyword index from 13 to 0, leaving fsck with only its two known warnings.

  <sub>commit:3218c12 · test:cargo test --workspace --no-fail-fast · test:cargo clippy --workspace --all-targets -- -D warnings</sub>

- **KEEL-177** Derive whether a phase is planned, active or complete instead of storing it — `done`

  `MilestoneStatus` holds only declarations now — open, paused, shipped, cut — and `MilestoneState::derive` works out planned, active, blocked or complete from a task tally and the blocks edges. Migration 3 rewrites the old stored values and backfills `shipped_at` from the last task to close. The digest, the tracker and `/api/entities` all read the derived state, and the first two report every phase in flight rather than the first one found, which is what named a finished phase for a week. `apply_changes` refuses to write a derived value and writes `shipped_at` with `shipped`. Two fsck checks cover what neither the migration nor the guard can reach.
  
  Writing it turned up a bug that had nothing to do with milestones: `ensure_above` primes a process-wide id generator, so the doctor test that deliberately writes a future event poisoned every test that ran after it, and the failure surfaced on an innocent one. Fixed with `id::reset_for_tests` and a lock. Latent until this week's new tests changed the ordering.
  
  The live store still needs `keel migrate` with the daemon stopped — it is on schema 1 and the code ships three.

  <sub>commit:3218c12 · doc:dec_01KZX9ZJWEGGFSPXK1MH750G94 · test:cargo test --workspace</sub>

- **KEEL-175** Semantic search has no archived filter, and the test that should catch it can't — `done`

  Archived documents now leave both halves of search, and the bug turned out to be live rather than latent — the keyword half was leaking too, which is the opposite of what this task was written believing. Three holes closed: the five prose tables got the `_fts_archived` trigger they were never given, the `documents` triggers got a guard so a revision on an archived entity cannot resurrect it, and `reembed_missing` skips archived so the backfill cannot undo an archive. Archiving clears the vector rather than adding a `WHERE` clause to `search_semantic`, so there is no new predicate to forget. Migration 2 took the live store's leaked rows from 13 to 0 on a copy, with live rows untouched.

  <sub>commit:217e612 · commit:1299bdb · test:cargo test -p keel-core --lib store::search · test:cargo test -p keel-core --test documents reembed · test:cargo test -p keel-cli --bin keel doctor::tests</sub>

- **KEEL-119** A killed test run leaks its temp stores into TMPDIR — `done`

  The task was half stale and half real. Stale: it described a 4.8 MB DuckDB store plus a `lance` directory, and Phase 9 made that a 388 KB `keel.sqlite` — thirteen times smaller — so the "multi-megabyte store" half of its own done-criterion was already met by something else. Real: the leak still happens, and the sweeper meant to catch it had never worked outside this laptop.
  
  Measured first. A test binary that finishes leaks nothing; a killed one leaks every store it had open. The 2,318 stores on disk came from one `cargo mutants` run whose 24 timeouts each killed a binary mid-suite.
  
  Fixed the sweeper: its glob only matched where TMPDIR ends in a slash, and it stayed quiet when it swept nothing, so it read as "nothing to do" rather than "I did not look". It now also matches the 47 remaining pre-Phase-9 DuckDB stores.
  
  One thing not done, and it needs a human: the 1.1 GB currently on disk is still there. Running the sweep was refused by this session's permission classifier, so `./scripts/sweep-build-artifacts.sh` has to be run by hand. `DRY_RUN=1` shows what it would take first.

  <sub>commit:e15dcc7 · doc:dec_01KZXA7K5NXDGTVTEG9G26JPBB</sub>

### 2026-08-12

- **KEEL-164** Strengthen the QA process: Linux CI, snapshot gate, coverage and mutation as discovery — `done`

  The check job runs on ubuntu-latest and macos-latest with fail-fast off, and `cargo insta test --unreferenced=reject` is a gate of its own — verified by hand that it would pass today, since cargo-insta is not installed locally. Coverage, mutation testing and fuzzing are scheduled weekly and block nothing, per the panel's own recommendation: coverage publishes lcov and a summary as artifacts, mutants is scoped to graph.rs, next.rs, safe_path.rs and id.rs, and three 60-second fuzz targets live in their own workspace so nightly does not leak into ordinary builds. Each fuzz target asserts a property rather than merely not crashing. `fts_match` became public so one of them could reach it.

  <sub>commit:e8cb8a9 · test:cargo check --manifest-path fuzz/Cargo.toml</sub>

- **KEEL-166** Reduce the digest's N+1 traversal and full-table scans before it matters — `done`

  `blocked_tasks` and `next::rank` now take one `links_in_project` query each instead of a traversal per open task, with blocker liveness resolved from the task page already loaded and non-task blockers fetched once and remembered. The fail-closed rule is unchanged — an unreadable blocker still means blocked. Per-project open and urgent counts come from `Store::task_counts`, a single GROUP BY whose `IN` lists are built from the enum predicates so the definitions stay in one place; that also removed a silent 2,000-row cap on the count. The digest snapshots assert the output is unchanged.

  <sub>commit:5a900ec · test:cargo test -p keel-core --test next · test:cargo test -p keel-mcp --test snapshots</sub>

- **KEEL-165** Add the end-to-end binary and MCP-protocol conformance test tiers — `done`

  Both tiers added. `end_to_end.rs` spawns the real binary over a real port and covers health, a tool call, the SSE stream, generate, a malformed body and a 429 with its retry-after. `protocol_conformance.rs` pins the envelope independent of tool semantics: both handshake revisions, a request with no version anywhere, an unsupported version, the mirrored header rules both ways, the error codes with their HTTP mappings, and the id surviving every path. Writing the first tier found that the startup line logged the requested bind address rather than the bound one, so `--bind 127.0.0.1:0` announced port 0 — it reports `local_addr` now.

  <sub>commit:66f42ae · test:cargo test -p keel-daemon --test end_to_end · test:cargo test -p keel-daemon --test protocol_conformance</sub>

- **KEEL-169** Warn on the environmental conditions that quietly corrupt a local SQLite store — `done`

  `keel_core::environment` detects a store under a sync root or a network-shaped path from the path alone, and both the daemon (at startup, before opening the store) and `keel doctor` (as a `location` check) report it with a remedy that names the service and says to `keel backup` first. It warns rather than refuses and matches whole path components rather than substrings, because a warning that has cried wolf once is one nobody reads the second time. The launchd restart storm is fixed by exiting zero on an unopenable store — `KeepAlive` with `SuccessfulExit: false` restarts on non-zero, and a store that will not open will not open on the retry either.

  <sub>commit:7d5c05d · test:cargo test -p keel-daemon --test wont_restart_loop · test:cargo test -p keel-core --lib environment</sub>

- **KEEL-167** Guard against unbounded WAL growth under the long-lived SSE reader — `done`

  `PRAGMA wal_autocheckpoint = 1000` is set explicitly — SQLite's own default, so no behaviour change, but now a decision with a number tests can assert against. `await_holding_lock` is denied workspace-wide and passes, which is the verification that nothing pins a read snapshot across a suspended future; the SSE handler only touches the broadcast channel and never the store. `Store::wal_pages` plus three tests: bounded under ordinary writing, bounded with a second connection reading throughout, and emptied by the shutdown checkpoint.

  <sub>commit:f0b4043 · test:cargo test -p keel-core --test wal_growth</sub>

- **KEEL-162** Tidy the modularity seams the review flagged — `done`

  Three of the four folded. `read_link` and the seventeen-column link SELECT moved into `rows.rs`, so adding a link column is one edit rather than four. The digest moved to `keel_core::digest` — a thousand lines of pure store logic that could only be reached by speaking JSON-RPC — with the JSON shaping still in keel-mcp and the snapshots unchanged. The two daemon-read helpers folded into `writes::read` with the timeout passed in; `urlencode` turned out to exist only once already. The fourth, gating the fixture corpus behind a feature, is deliberately not done: B-54 records why, and the short version is that cargo unifies features across a workspace build so it would save nothing, while breaking plain `cargo test -p keel-core` and putting back the feature machinery B-11 removed.

  <sub>commit:2f44e4c · doc:dec_01KZTFE58YF4AZATMPAHDQ8R87 · test:cargo test --workspace</sub>

- **KEEL-160** Cover the untested surfaces: SSE stream, embedder fallback, CLI verbs, malformed MCP — `done`

  All four surfaces covered. `live_stream.rs` opens the SSE stream and asserts the pre-write comment arrives, a create reaches an open stream, and a lagged subscriber is told rather than left stale. `degraded_search.rs` boots the daemon against a model cache that cannot load and asserts it binds without waiting, serves, answers a keyword search and still accepts prose. `http_edge.rs` gained ten malformed-body shapes that all have to come back as JSON-RPC. `crates/keel-cli/tests/verbs.rs` drives ready, claim, close and generate through the real binary, plus clap's `debug_assert` as a unit test. Used `CARGO_BIN_EXE_keel` rather than adding assert_cmd, since cargo already provides the path.

  <sub>commit:295036c · test:cargo test -p keel-cli --test verbs · test:cargo test -p keel-daemon --test live_stream · test:cargo test -p keel-daemon --test degraded_search</sub>

- **KEEL-163** Stop doc/code drift with guard meta-tests, and fix the stale docs found — `done`

  Both stale docs corrected — "nine tool definitions" is thirteen, and the daemon's write-path list no longer claims mirror generation is one of its steps. Two guards added in the shape of `graph_direction.rs`: `event_coverage.rs` performs every kind of write and asserts `Action::ALL` matches the actions the log contains, and `fsck_coverage.rs` corrupts the store nineteen ways, one per check, asserting each fires and that a healthy store fires none. `fsck::CHECKS` is the declared list with a test inside `fsck.rs` asserting it matches what the code emits; `page_integrity` is the one waiver, with the reason recorded. Thirteen of the nineteen fsck checks had never been seen to fire before this.

  <sub>commit:6faadde · test:cargo test -p keel-core --test fsck_coverage · test:cargo test -p keel-core --test event_coverage</sub>

- **KEEL-161** Clear the small correctness papercuts found in review — `done`

  All seven fixed with tests. Id deserialisation now parses (both `EntityId` and the connective ids); `version` is range-checked instead of wrapped through `as i32`; cross-type paging offsets the merged list rather than each table; the search excerpt folds ASCII-only so byte offsets survive; `retract_note` records an attributed event; a rank between misordered anchors and a task closed as a duplicate of a non-task are both refused; and the `unreachable!` became the arm its guard was preventing.

  <sub>commit:77ad8f1 · test:cargo test -p keel-core --test papercuts · test:cargo test -p keel-mcp --test argument_edges</sub>

- **KEEL-156** Write the property tests the contract has always required — `done`

  Four properties in `crates/keel-core/tests/properties.rs`. Outbound traversal against a separately-computed breadth-first walk, inbound against the reversed adjacency — the only check that fails when the directions are swapped. Two on the revision chain: contiguous from 1 with exactly one current, asserted after every write, and every version still fetchable by number with its parent named. The graph generator allows self-edges and back-edges so cycles arrive without being asked for, and the revision generator draws bodies from a four-value alphabet so the body-hash collapse is exercised rather than assumed away.

  <sub>commit:ac76adb · test:cargo test -p keel-core --test properties</sub>

- **KEEL-157** Complete the MCP response snapshots and guard the surface against drift — `done`

  All thirteen tools have a seeded-store response snapshot now — the nine that did not are get, update, write_doc, link, note, activity, ready, claim and close. The meta-test reads the snapshot directory rather than a list, so tool fourteen cannot arrive without one. Two side effects: the fixture returns its seeded ids, since six of the nine tools are addressed by an id, and the redaction filter was missing `nte` and `blb` — nothing had ever snapshotted a response carrying a note id, so the first one that did churned on every run.

  <sub>commit:45d55ee · test:cargo test -p keel-mcp --test snapshots</sub>

- **KEEL-159** Shrink the daemon's store-lock scope so one slow call can't freeze it — `done`

  Generating is two phases now: `generate::plan` reads the store and decides every file, `GeneratePlan::apply` writes them and touches nothing else, and the daemon drops the lock in between. The mirror split the same way, which also collapsed its copy of write-if-changed into `generate`'s — the only real difference between them was whether a missing banner counts as a change, and that is a field on the planned file now. Search query embedding moved out of the lock too, via `dispatch_prepared`, falling back to the old path when the store is busy.

  <sub>commit:a5910cc · test:cargo test -p keel-daemon --test lock_scope · test:cargo test -p keel-core --test generate</sub>

- **KEEL-154** Make schema migration deliberate, not a side effect of opening the store — `done`

  Two doors instead of one. `Store::open` creates-and-migrates a store that does not exist yet and refuses an existing one with migrations outstanding, naming them and naming the command; `Store::open_and_migrate` is the owner's door with three callers — the daemon at startup, `keel migrate`, and a restore. `keel migrate` reads the ledger without opening a Store, refuses while a daemon listens using the same probe as every other direct write, and has a --dry-run. `/api/health` reports `schema` and the CLI refuses to write through a daemon behind it. `keel doctor` checks the schema first and reports pending migrations as a problem with the fix, rather than failing to open the store at all.

  <sub>commit:b0db107 · test:cargo test -p keel-core --test migration · test:cargo test -p keel-daemon --test health</sub>

- **KEEL-158** Harden the daemon's HTTP edge: body limit, origin ordering, drop Origin: null — `done`

  All three landed. `Origin: null` is refused rather than treated as local; the origin check now runs before the rate limiter, so a page that gets refused no longer spends the user's budget; and an 8 MB `DefaultBodyLimit` sits behind a middleware that turns axum's bare 413 into a JSON-RPC error naming the limit and pointing at `image_path`. Five new integration tests in `crates/keel-daemon/tests/http_edge.rs` drive real HTTP against a real daemon, including the two failure cases and the under-the-limit case that would have broken silently if the cap were set too low. The unit test that asserted `null` was allowed moved to the rejected list.

  <sub>commit:61e8649 · test:cargo test -p keel-daemon --test http_edge</sub>

- **KEEL-155** Write generated files atomically (temp file + rename) — `done`

  New keel_core::atomic::write — sibling temp file, sync_all, rename over the target. Same directory because a cross-filesystem rename is a copy and a delete; flush before the rename because otherwise a power cut leaves the right name and no content; the file handle is closed before the rename because Windows refuses to rename over an open one. Every generated-file writer goes through it: the adopted prose files, the .keel mirror, the mirror manifest, and render-status --out. Six tests, including one asserting a failed write leaves the old content rather than a prefix of the new.

  <sub>commit:8d1157b · test:cargo test -p keel-core atomic</sub>

- **KEEL-153** Keep the event feed alive across a clock step (sleep/wake, NTP) — `done`

  Store::open now calls seed_id_generator, which reads max(events.id) and max(v_entities.id) and calls the new keel_core::id::ensure_above on each — priming the process-wide ULID generator with one throwaway id at highest+1ms when the stored id is at or ahead of the clock, and logging when it had to. The generator increments from there until the clock genuinely passes it. Test writes an event an hour in the future, reopens the store as a fresh process would, appends a normal write and asserts the feed advances; verified failing without the fix.

  <sub>commit:0289993 · test:cargo test -p keel-core --test recent_events</sub>

- **KEEL-151** Fix the embedding operational story: backfill, don't block boot, signal degradation — `done`

  Four parts. `keel reembed --missing` walks current revisions with no vector in batches of 32, one transaction per batch, sharing Document::searchable_text_of with the write path so backfilled and freshly written vectors are comparable. The daemon logs at startup how many current documents lack vectors and whether sqlite-vec registered at all; search consults Store::vector_search_available and degrades to keyword-only rather than failing the query. The model now loads on a background thread after the socket binds, via the new Store::set_embedder — it used to load inline, so a first run left the daemon unreachable for a 130MB download. Two tests on the pass, including that a second run has nothing to do.

  <sub>commit:b26805f · test:cargo test -p keel-core --test documents</sub>

- **KEEL-152** Extract keel-embed so keel-core stops opening a network socket — `done`

  New keel-embed crate holds FastEmbedder and the fastembed dependency, depending on keel-core only for the Embedder trait and the error type. The daemon depends on it and injects the embedder, which it was already the only constructor of. Embedder and HashEmbedder stay in core. Verified with cargo tree: keel-cli's dependency graph no longer contains fastembed or ONNX Runtime, and cargo deny still passes.

  <sub>commit:e690d68 · test:cargo test --workspace</sub>

- **KEEL-150** Add `keel doctor` — one command that surfaces silent divergence — `done`

  `keel doctor` composes seven read-only checks — daemon reachable, PRAGMA quick_check, fsck, embedding coverage, mirror drift per project with a checkout, backup age, and whether the newest event id was minted ahead of the wall clock. Three levels: ok, degraded (says so, does not fail) and problem (exits 1), because a check that cannot tell "worse than it should be" from "broken" is one nobody can put in a hook. --json emits the same report. Added keel_core::id::minted_at for the clock check. Run against the live store it immediately reported the real conditions: 133 current documents, none with a vector.

  <sub>commit:cc2027a · test:cargo test -p keel-cli doctor</sub>

- **KEEL-149** Fix the backup manifest race and verify the snapshot after creating it — `done`

  The snapshot is taken first and the manifest is counted through the snapshot's own read-only connection, so there is no window between the count and the copy for a write to land in. PRAGMA integrity_check runs on the snapshot while it is open; a damaged one is refused with the previous backup left as the most recent good copy. read_manifest takes a &Connection now rather than a &Store. Two tests: one restores after a write at the seam and asserts verify_restore accepts it, one asserts the manifest's counts match the snapshot file.

  <sub>commit:74a347d · test:cargo test -p keel-core --test fixture_backup</sub>

- **KEEL-148** Add integrity checks: PRAGMA integrity_check plus the missing fsck orphan checks — `done`

  fsck gained four checks: page_integrity (PRAGMA integrity_check), row_without_creation_event, live_link_to_archived and orphan_blob. The daemon runs the cheaper quick_check at startup and logs at error level with the sync-folder diagnosis rather than refusing to boot — a daemon that will not start leaves no way to back up what survives. fsck::page_integrity(store, which) is public so keel doctor can reuse it. Four tests corrupt the store behind the API and assert each finding, plus one asserting a healthy store reports None rather than an empty list.

  <sub>commit:55e4d99 · test:cargo test -p keel-core --test fixture_backup</sub>

- **KEEL-147** Make readiness ranking fail closed when a blocker row is unreadable — `done`

  Both blocker-liveness sites in next.rs — blocked_tasks and rank — now fail closed: a storage error while reading a blocker logs at warn and treats the task as blocked rather than ready, and the blocked row's reason names the unreadable blocker with "run keel fsck" so it is not blocked by nothing visible. Two tests corrupt a question's status column behind the API and assert the task never reaches the ready list; both verified failing against the previous commit.

  <sub>commit:9b57567 · test:cargo test -p keel-core --test next</sub>

- **KEEL-143** Close the two-writer footgun: lock-free health, honest fallback, probe before direct write — `done`

  /api/health now uses AppState::try_store and never waits — it reports the last observed project count with store_busy: true when the lock is held. New keel-cli module `writes`: probe() is a TCP connect (connection-refused is the only answer meaning nothing is listening; a timeout means alive and busy), open_for_write() refuses a direct write while a daemon holds the port with --force as the escape, and may_read_directly() replaces generate's fall-back-on-any-error. note, archive, task add and fixture all route through open_for_write. Six unit tests on the probe and funnel, two daemon tests asserting health answers under a held lock in under 400ms.

  <sub>commit:f4bafc7 · test:cargo test -p keel-daemon --test health · test:cargo test -p keel-cli writes</sub>

- **KEEL-144** Fix the oldest-first event reads that already truncate history today — `done`

  Added EntityStore::recent_events(EventScope, limit) — ORDER BY id DESC in SQL, so the engine picks which rows survive the cap. All four oldest-first-then-reverse sites moved to it: the 409 payload (now scoped to the row by index instead of filtering 500 store-wide events in Rust), the digest's Recently, the rendered changelog, and changes::by_session. Five tests including the cap-plus-one boundary, and one asserting the cursor feed still returns oldest-first so paging cannot skip rows.

  <sub>commit:f34bea1 · test:cargo test -p keel-core --test recent_events</sub>

- **KEEL-142** Confine entity-controlled file paths to the project, in keel-core — `done`

  New keel-core module safe_path with two layers: validate_repo_relative runs inside validate_entity, so create and update both refuse an absolute path, any `..` component, a leading `~`, a NUL byte or an empty string; confine runs at every repo_root.join in generate.rs and mirror.rs and additionally resolves both sides, which catches a directory inside the repo that is a symlink out of it. root_path gets the opposite rule — it must be absolute, since a relative one names a different repository depending on where the daemon started. Seven end-to-end tests plus nine unit tests, including one asserting generate writes nothing outside a temp root when the bad path was planted directly in the table.

  <sub>commit:110d779 · test:cargo test -p keel-core --test path_confinement</sub>

- **KEEL-146** Make the composite create (entity + body + image) one transaction in core — `done`

  keel-core now has Store::create_with_document in a new store/composite.rs — entity, first revision, blob and the row's blob_id in one transaction. The keel_create dispatch is parse-and-call, and the second update() that existed only to record the blob pointer is deleted: minting the blob id before the row insert removes the cycle that made it two writes. Seven tests in tests/composite.rs, including denied-documents and denied-blobs faults that assert the entity does not survive alone.

  <sub>commit:cc285c5 · test:cargo test -p keel-core --test composite</sub>

- **KEEL-141** Make entity writes atomic, and emit the missing revision event — `done`

  create, update, archive, link and unlink are each one transaction now, and write_revision appends the Action::Revised event inside the transaction it already had. Nine regression tests in tests/atomicity.rs, every one verified to fail against the previous commit by stashing the source and re-running.

  <sub>commit:043360e · test:cargo test -p keel-core --test atomicity</sub>

- **KEEL-140** Push write primitives down to &Connection (the transaction substrate) — `done`

  Every write primitive in store/entity.rs now takes &Connection: insert_entity_row, insert_link_row, set_link_archived, archive_links_touching and insert_note_row are new names for statements that were inline in the trait methods, and append_event_inner, write_back, find_by_key, find_link, require_live, resolve_vertex plus the four query helpers changed signature. docs.rs already had insert_blob on &Connection and write_revision in a transaction. Pure refactor — 666 tests green, clippy clean.

  <sub>commit:b7c8c5e · test:cargo test --workspace</sub>

- **KEEL-145** Build the fault-injection testkit for the resilience regression tests — `done`

  Built crates/keel-core/tests/support/faults.rs with the four primitives — deny the nth INSERT (and a deny_write_after sibling for update paths), a page cap that produces SQLITE_FULL, a progress-handler interrupt, and an atomic-write asserter that rejects a prefix. Each one is asserted against plain SQL in tests/faults_testkit.rs before anything relies on it, and one test arms a fault on a real Store to prove the &Connection signature allows it. rusqlite's `hooks` feature is a dev-dependency, so the shipped binaries never get the authorizer API.

  <sub>commit:03236a2 · test:cargo test -p keel-core --test faults_testkit</sub>

- **KEEL-170** A task with a summary but no body shows "No description." in the app — `done`

  The card prefers the body, falls back to the summary, and says "from the summary" when that is what it is showing. Verified against the live store rather than a fixture: KEEL-143 read "No description." before and now shows its 737-character summary, and KEEL-96 — a pre-8G row with a body and no summary — still shows its body with no label. Four tests, and the two new behaviours both fail without the change.

  <sub>commit:8c27101 · test:npx vitest run src/screens/Task.test.tsx · test:npx tsc --noEmit</sub>

- **KEEL-139** Run the deep engineering review and expert panel; land the prioritized hardening backlog — `done`

  Ran the deep engineering review as six parallel dimension reviewers (architecture, code quality, security, performance, resilience, test/QA), then a five-member expert panel (storage reliability, security architecture, Rust architecture, SRE, QA) over the consolidated findings. Landed the result as Phase 11 (mst_01KZSNZS9H4E4TA1J1SD72DB53): 30 prioritized tasks KEEL-140..169 (7 p1, 12 p2, 11 p3), the forced-sequencing blocks edges, decision dec_01KZSQJ05N4TSXDETPAZKD685F recording the panel-agreed write-path atomicity design, and two open questions for KB (single-writer lock file; amending hard constraint 7 before a web-UI write endpoint). Top findings: the entity write path is non-transactional (silent history loss, flagged HIGH by three reviewers independently); an entity-controlled mirror_path lets a prompt-injected write plant a file anywhere via keel generate (security HIGH); the 409-conflict event payload already reads the wrong rows at 804 events; proptest is a declared-but-unused dependency. Full per-dimension and per-panelist reports in the session scratchpad.

  <sub>commit:4f3a14d</sub>

- **KEEL-134** The live store is still DuckDB — Phase 9 shipped in the tree but never on this machine — `done`

  The machine is on SQLite. Backup taken, daemon stopped clean, `keel migrate` from a throwaway ab66018 build reported the two stores identical — 1599 rows across 18 tables, 234 document hashes, 1 blob hash — and that was checked again against an inventory read from the backup's Parquet, which does not go through the DuckDB bindings. New binaries installed, new daemon up, and this closure is itself a write landing in keel.sqlite. `keel.duckdb` and `lance/` are deleted and the store's own git repo records their removal. 40 MB plus a Lance directory is now a 4.6 MB file; `keel` went 44 MB to 6.8 MB.

  <sub>test:keel migrate · test:sqlite3 ~/.keel/keel.sqlite "pragma integrity_check" · commit:0fcb0fc · test:keel fsck</sub>

### 2026-08-11

- **KEEL-138** generate and the pre-commit check both ignore which worktree you are in — `duplicate`

  Same bug, filed twelve minutes apart by two sessions in two worktrees, which is a fair demonstration of the bug. KEEL-136 covers the check side better than this row did — it has the false-pass case, where a hand-edited file in a worktree is compared against a current main checkout and waved through. The half this row added, `generate` writing into the main checkout from a worktree, is now a note on KEEL-136.

- **KEEL-133** keel ready says "nothing ready" when no daemon is listening — `done`

  `keel ready`, `claim` and `close` all took the store fallback and got the MCP envelope where the daemon path gave them the payload inside it, so every field they asked for read as absent — which for `ready`'s list meant "nothing ready". There is now one fallback rather than two copies, the unwrap sits inside it, and `keel_mcp::structured` is the single implementation shared with the daemon's `as_api`. Six tests drive the daemonless branch through a dead port; four of them fail without the fix.

  <sub>commit:38dece4 · test:cargo test -p keel-cli work:: · test:cargo clippy --workspace --all-targets -- -D warnings</sub>

- **KEEL-132** SPEC.md sections 1 to 7 still describe the two-engine store — `done`

  SPEC §1 to §12 and §14 now describe the SQLite store. The architecture diagram is one box, §2's storage split is a split between tables rather than engines, §3's DDL is the real schema down to the STRICT and the JSON-in-TEXT columns, §4's traversals are SQLite with a delimited path string for the cycle guard, §5 is FTS5 plus sqlite-vec, §7 says the single write path is a convention now, and §11's backup is VACUUM INTO. Six claims reversed rather than merely aged and each is quoted and marked where it stood, in D-1's style, rather than deleted. §13 was left alone on purpose: six of its rows argue from the old engine, and spec decisions are KB's — that is TQ-37.

  <sub>commit:5b165f4 · doc:spc_01KZKMPVNTZAZHC9HY1TSNZNGM · test:keel generate keel --check</sub>

- **KEEL-130** 9C — Take DuckDB and Lance out of the repository entirely — `done`

  DuckDB and Lance are gone from the tree: the dependencies, the five source files, the CLI subcommand, the dev-profile workarounds, CI, the install script and the prose. 657 workspace tests pass, both snapshot suites have zero diffs, and a cold release build is 1m 21s against 22m 11s. SqliteStore became Store and store/sqlite was promoted, since naming a type after its engine only earns its keep while there are two. What is left is deliberate history — comments saying what something used to be and why it changed, and a restore that still recognises an old two-part backup so its owner gets a sentence rather than a parse error.

  <sub>commit:8150c26 · test:cargo test --workspace</sub>

- **KEEL-129** 9C — Every surface still works, checked rather than assumed — `done`

  Every surface runs on SQLite and was exercised against a migrated copy of the real store: daemon routes, MCP, the SSE stream, all eleven CLI commands, generate and its --check, and every screen in the browser. 696 workspace tests and 220 desktop tests pass, and both snapshot suites have zero diffs. Two silent regressions fell out of repointing the existing tests rather than rewriting them: the SQLite store was not computing embeddings on write, and the older-binary guard from KEEL-95 was missing.

  <sub>test:cargo test --workspace · commit:37a2341</sub>

- **KEEL-127** 9B — keel migrate: read the old store, write a new one, prove they match — `done`

  keel migrate runs on the real store and verifies clean: 1,514 rows across 18 tables, 227 document hashes and 1 blob hash all matching, in 1.9 seconds, with the old store never written to. 40 MB of DuckDB and Lance become one 4.3 MB SQLite file. Task numbers, event summaries and link direction all spot-checked afterwards rather than trusted to the report.

  <sub>test:cargo test -p keel-core --lib migrate · commit:cb464db</sub>

- **KEEL-126** 9B — Search on FTS5 and sqlite-vec, with an index that does not need rebuilding — `done`

  Search on the SQLite store now runs BM25 over the trigger-maintained `fts_entities` index and cosine nearest neighbours over `documents.embedding` with `sqlite-vec`, fused by reciprocal rank as before. Nothing rebuilds an index: a row written in one statement is findable in the next, and an archived row leaves the index on its own. Caller text is quoted before it reaches `MATCH`, so a hyphen or a quote is text rather than syntax, and `bm25()`'s negative score is negated so the best match ranks first. All in `crates/keel-core/src/store/sqlite/search.rs`, which also carries the `impl DocumentStore for SqliteStore` block that completes the trait.

  <sub>test:cargo test -p keel-core --lib store::sqlite · doc:tsk_01KZS80VSXJC2NVK2D5WJYE5HV</sub>

- **KEEL-124** 9B — The SQLite store: schema, EntityStore and GraphStore — `done`

  EntityStore and GraphStore both run on SQLite. 43 tests on the entity half and 18 on the graph, and the existing round-trip, idempotency, optimistic-concurrency and both-direction traversal coverage passes without a test being changed to accommodate the engine. Graph direction was written from SPEC 3.3 rather than translated, and the tests were proved to bite by inverting the mapping and watching 14 of 18 fail.

  <sub>commit:f9d1f7f · commit:4d3069e · test:cargo test -p keel-core --lib store::sqlite</sub>

- **KEEL-123** 9A — A performance budget with numbers, and the stall that has never been measured — `done`

  Three things landed. `scripts/measure-performance.sh` times every read a board load makes plus search over N rounds and prints mean, max and size, with the two calls the board dropped kept in the table so the saving is measured rather than remembered. `crates/keel-daemon/tests/read_budget.rs` holds those reads to a budget roughly ten times the measured mean, and asserts the byte side exactly — a change that puts the digest back in front of the board fails there rather than in someone's browser. And the board now reads `/api/ready?blocked=true` instead of the whole digest and `/api/notes?counts=true` instead of every note body: 384 KB down to 198 KB, and the slowest read it waits on halved. Verified in a browser against a rebuilt daemon — 130 cards, the ranked Next panel, the blocked column and the blocked filter all still right, no console errors.

  <sub>test:cargo test -p keel-daemon · test:npm test --prefix apps/desktop · test:bash scripts/measure-performance.sh --base http://127.0.0.1:7655 --rounds 20</sub>

- **KEEL-125** 9B — DocumentStore on SQLite: revisions, blobs in the same transaction — `done`

  Revisions and blobs are ordinary SQLite tables. `write_revision` demotes the old current revision, inserts the new one and advances the header's `current_doc_version` in one transaction, so the three can no longer disagree; identical content still short-circuits to the existing revision, and the returned version is the one the store assigned. Blobs are written through a helper that takes any connection, so a caller holding a transaction writes an image and the row that owns it together — there is a test that a rollback takes both. Seventeen tests pass, including a 5 MB blob round-tripping byte-identically. Search is not here: the six methods are inherent on `SqliteStore` rather than a trait impl, because the trait cannot be satisfied without it.

  <sub>test:cargo test -p keel-core --lib store::sqlite::docs</sub>

- **KEEL-128** 9B — Backup becomes one file, and restore stops needing two halves — `done`

  A backup is now one SQLite file plus a manifest, written by `VACUUM INTO`, and a restore is a file copy. `crates/keel-core/src/sqlite_backup.rs`, six tests, including a 5 MB blob surviving byte-identically and two refusals that matter more than the happy path.

  <sub>test:cargo test -p keel-core --lib sqlite_backup · doc:tsk_01KZS81S8DJQAEAV0RPPT9JV8K</sub>

- **KEEL-121** 8F — Store the project's own noun, and read the glossary before the built-in list — `done`

  A project stores its own word for a milestone and the interface says it, and a glossary term can declare which type it is a spelling of — so a project's vocabulary no longer has to be anticipated in the source.

  <sub>test:cargo test -p keel-core --test vocabulary · doc:dec_01KZS6ZARDDED3P4GF3X8QF9E7</sub>

- **KEEL-98** 8C — Make the app legible — `done`

  All eight pieces of section 8C are done — the design pass, the rail, the Library, dates, the phase chip, search chips, the filter box, and What changed.

  <sub>doc:tsk_01KZR4W6SDNM90Y5BT44A9FAGN · commit:52f789e</sub>

- **KEEL-107** C4 — Activity: make the rows go somewhere, then decide about grouping — `done`

  Activity is rebuilt as What changed: sessions newest first, each with its own account, every row a link, and notes in the union because a note leaves no event.

  <sub>test:cargo test -p keel-core --test changes · test:npx vitest run src/screens/Changed.test.tsx</sub>

- **KEEL-115** 8B — keel_attach: let the daemon read the file, and tell the truth about the base64 cap — `done`

  The daemon reads an image off the disk now, as image_path on keel_create and keel_update rather than a fourteenth tool. The base64 description states a ceiling a session can actually reach.

  <sub>test:cargo test -p keel-mcp --test images · doc:dec_01KZS2VXYGZ35YVV56QZ4AYNC0</sub>

- **KEEL-113** 8G — keel lint: report the rows that are already unreadable — `done`

  keel lint reports three kinds of unreadable row and rewrites none of them. The unexpanded-identifier count is zero, which was the Phase 8 exit criterion; the nine it found were glossed by hand.

  <sub>test:cargo test -p keel-core --test lint · url:keel lint keel --check unexpanded_identifier</sub>

- **KEEL-114** 8B — File an issue from the app, with a pasted screenshot — `wont_do`

  KB's call on 2026-08-11: skip filing issues from the app for now. Hard constraint 7 stands unamended, so nothing has to be reversed if it returns. TQ-30 carries the two costs: Phase 8 cannot claim its 30-second stopwatch criterion, and large images have no route in from a chat surface.

- **KEEL-111** 8A — A triage status, ahead of todo — `wont_do`

  KB's call on 2026-08-11, following from TQ-30: with app filing declined nothing files in a hurry, so a holding pen has nothing to hold. Not now rather than never — the argument comes back with app filing if that does. The dropped correction it carried was split out as KEEL-122 and is done.

- **KEEL-122** CLAUDE.md tells every session to use a task status that does not exist — `done`

  The contract now says wont_do, and points at keel_close for the five reasons. The dropped line had been failing silently for as long as it existed.

  <sub>doc:spc_01KZKSME2TCPVARX9M04836XD6</sub>

- **KEEL-110** 8A — Closing a task needs a reason, and done needs evidence — `done`

  Closing states one of five reasons with a message, and done needs evidence. Enforced in the storage layer, so keel_update cannot get round it.

  <sub>commit:ca30147 · commit:775aa96 · test:cargo test -p keel-daemon --test use_cases</sub>

- **KEEL-109** 8A — keel claim: taking a task, atomically — `done`

  keel claim and keel_claim record who is working on a task and refuse a second session by name. Atomic through optimistic concurrency rather than a lock.

  <sub>commit:ca30147 · test:cargo test -p keel-core --test work</sub>

- **KEEL-108** 8A — keel ready: what can be worked on right now — `done`

  keel ready is a CLI command, an MCP tool and a screen, all reading one computation in keel-core. A daemon test asserts the MCP and REST answers match reference for reference, in order.

  <sub>commit:775aa96 · commit:52f789e · test:cargo test -p keel-daemon --test use_cases</sub>

- **KEEL-112** 8G — A task cannot be created without a summary — `done`
- **KEEL-116** 8F — Let a project use its own word for a milestone — `done`
- **KEEL-117** 8E — Rate limiting on /mcp — `done`
- **KEEL-118** 8E — Copy-ready prompts on the task page — `done`
- **KEEL-105** C5 — Search that suggests questions about this project — `done`
- **KEEL-106** C6 — A filter box in the Library, and words that say what filtering does — `done`
- **KEEL-102** C2 — The Library: one layout per kind of thing — `done`
- **KEEL-104** C7 — Show which phase a task belongs to, on the row — `done`
- **KEEL-103** C3 — One way to show a date, used on every screen — `done`
- **KEEL-120** Plain English on every prose field, not just milestone summaries — `done`
- **KEEL-99** A milestone cannot be created without a plain-English explainer — `done`
- **KEEL-101** C1 — Navigation is inside out: put the project first — `done`
- **KEEL-100** C0 — The design pass: Geist, a token layer, and a theme the user chooses — `done`
- **KEEL-97** SSE never opens in the in-app browser, cause unknown — `done`
- **KEEL-47** Metric observations charted against target — `done`
- **KEEL-46** Design artifacts with stored images — `done`
- **KEEL-48** Deployable daemon with auth — `wont_do`
- **KEEL-45** GitHub App and webhook receiver — `wont_do`
- **KEEL-57** Route the remaining read commands through the daemon API — `done`
- **KEEL-95** keel_create fails on decisions written during the number migration window — `done`
### 2026-08-10

- **KEEL-94** One decision register: fold the B-n prose table into the decision rows — `done`
- **KEEL-92** Document reset and the question prune — `done`
- **KEEL-91** Freeze the gate — one retrospective in place of six documents — `done`
- **KEEL-90** Protocol honesty, and one name for the daemon address — `done`
- **KEEL-89** Make the local API and the MCP tools genuinely one thing — `done`
- **KEEL-88** Tidy the tool surface — `done`
- **KEEL-87** Delete the file-edit hook, and replace it with a check that fails loudly — `done`
- **KEEL-86** One authority per instruction — `done`
- **KEEL-85** Speak English in the interface — `done`
- **KEEL-84** Make STATUS.md safe to render, and worth reading — `done`
- **KEEL-83** Write closed_at, and backfill it from the event log — `done`
- **KEEL-82** One definition of blocked, and the same numbers everywhere — `done`
- **KEEL-81** Sub-tasks — a parent link between tasks — `done`
- **KEEL-80** Task rank — a deliberate order, not reverse-creation-order — `done`
- **KEEL-79** Search results that go somewhere — `done`
- **KEEL-78** Filters that compose and survive, encoded in the URL — `done`
- **KEEL-77** A list view beside the board, with grouping and sorting you choose — `done`
- **KEEL-76** Readable identifiers — KEEL-42, not tsk_01KZKW28CS… — `done`
- **KEEL-75** The task detail view — `done`
- **KEEL-93** fsck died on its own staleness check, and the test suite was red — `done`
- **KEEL-74** Cmd-K command palette — `done`
- **KEEL-73** A real design system — named type scale and the missing primitives — `done`
- **KEEL-72** One page shell for every screen — `done`
- **KEEL-71** Routing and URLs — every screen, project, task and search has an address — `done`
- **KEEL-56** Render the tracker from the task rows, not stored prose — `done`
- **KEEL-70** Keyboard navigation — `done`
- **KEEL-69** Typed API client and live refresh — `done`
- **KEEL-68** Session-ID threading — `done`
- **KEEL-67** POST /api/generate — `done`
- **KEEL-66** Check that ID references in artifact bodies resolve — `done`
- **KEEL-65** Near-duplicate titles defeat the idempotency key — `done`
- **KEEL-64** Step 10 — hand-judge 20 writes for keep-rate — `done`
- **KEEL-58** Daemon wedge: SIGKILL mid-write corrupted a DuckDB ART index — `done`
- **KEEL-63** Step 6 — deterministic Stop hook for sessions that recorded nothing — `done`
- **KEEL-62** Step 3 — Run A: baseline against the repaired instrument — `done`
- **KEEL-61** Step 2 — repair the instrument before any further run — `done`
- **KEEL-60** Step 1 — validity audit of the gate runs — `done`
- **KEEL-51** Decide TQ-11: how long to carry the 2025-11-25 handshake — `done`
- **KEEL-50** Decide TQ-10: BM25 in DuckDB rather than Lance — `done`
- **KEEL-49** Decide TQ-9: idempotency_key on all thirteen tables — `done`
### 2026-08-09

- **KEEL-59** SessionStart hook: inject keel_context instead of hoping the skill loads — `done`
- **KEEL-36** Run the ten unprompted sessions — `done`
- **KEEL-55** Route reads through the daemon: POST /api/generate — `done`
- **KEEL-54** keel generate — the repo files become outputs — `done`
- **KEEL-53** Rendered markdown in the document reader — `done`
- **KEEL-52** keel import — whole markdown files into Keel — `done`
- **KEEL-44** Screen 9 — Activity feed — `done`
- **KEEL-43** Screen 6 — Search — `done`
- **KEEL-42** Screen 5 — Documents with revision diff — `done`
- **KEEL-41** Screen 4 — Board — `done`
- **KEEL-40** Screen 3 — Roadmap — `done`
- **KEEL-39** Screen 2 — Project dashboard — `done`
- **KEEL-38** Screen 1 — Home, all projects at a glance — `done`
- **KEEL-37** Tauri v2 shell with the daemon as a sidecar — `done`
- **KEEL-35** MCP config and install script — `done`
- **KEEL-34** Project-confirmation behaviour — `done`
- **KEEL-33** PostToolUse hook for mirror edits — `done`
- **KEEL-32** The skill that teaches Claude when to write — `done`
- **KEEL-31** Markdown mirror generator — `done`
- **KEEL-30** Serve MCP 2025-11-25 as well as 2026-07-28 — `done`
- **KEEL-29** Scripted UC-1 to UC-4 harness — `done`
- **KEEL-28** keel render-status — `done`
- **KEEL-27** Snapshot tests for every tool response — `done`
- **KEEL-26** Concurrency gate: zero duplicates, zero lost updates — `done`
- **KEEL-25** Local REST and SSE for the desktop app — `done`
- **KEEL-24** Shared single write path — `done`
- **KEEL-23** Write tools: create, update, write_doc, link — `done`
- **KEEL-22** Read tools: search, get, activity, projects — `done`
- **KEEL-21** keel_context — the digest — `done`
- **KEEL-20** The nine tool schemas — `done`
- **KEEL-19** server/discover and tools/list — `done`
- **KEEL-18** JSON-RPC and the stateless Streamable HTTP transport — `done`
- **KEEL-17** Monotonic ULID generation — `done`
- **KEEL-16** Implement idempotency keys and optimistic concurrency — `done`
- **KEEL-15** Test suite: concurrency, idempotency, OCC, round-trip — `done`
- **KEEL-14** 200-entity fixture across all types and relations — `done`
- **KEEL-13** keel fsck — cross-engine referential integrity — `done`
- **KEEL-12** Backup: DuckDB and Lance to Parquet, restore — `done`
- **KEEL-11** Hybrid search — BM25 plus vectors, RRF fusion — `done`
- **KEEL-10** Embeddings via fastembed — `done`
- **KEEL-9** Event log — append, query since cursor — `done`
- **KEEL-8** Links, GraphStore trait, recursive CTE traversal — `done`
- **KEEL-7** Document revisions — append, fetch by version, diff — `done`
- **KEEL-6** Entity storage layer — CRUD for all 13 types — `done`
- **KEEL-5** Lance documents and blobs datasets, ATTACH wiring — `done`
- **KEEL-4** DuckDB schema and forward-only migrations — `done`
- **KEEL-3** Domain types, ULID prefixes, the audit block — `done`
- **KEEL-2** Verify fast-moving dependencies — `done`
- **KEEL-1** Cargo workspace scaffold, CI, lint/fmt/deny gates — `done`
---

## Every change

| Date | Actor | Change |
|---|---|---|
| 2026-09-02 | claude | status in_progress → done |
| 2026-09-02 | claude | evidence [] → (125 characters) |
| 2026-09-02 | claude | close_reason none → done |
| 2026-09-02 | claude | close_message none → (1039 characters) |
| 2026-09-02 | claude | status todo → in_progress |
| 2026-09-02 | claude | claimed_by none → ses_864b193a-5582-4c58-bad7-f91398b0d3f8 |
| 2026-09-02 | claude | claimed_at none → 2026-09-02T20:46:11.175930Z |
| 2026-09-02 | claude | created task “Tidy the duplication this session's two fixes left in their own code” |
| 2026-09-02 | claude | status in_progress → done |
| 2026-09-02 | claude | evidence [] → (109 characters) |
| 2026-09-02 | claude | close_reason none → done |
| 2026-09-02 | claude | close_message none → (681 characters) |
| 2026-09-02 | claude | status todo → in_progress |
| 2026-09-02 | claude | claimed_by none → ses_864b193a-5582-4c58-bad7-f91398b0d3f8 |
| 2026-09-02 | claude | claimed_at none → 2026-09-02T20:35:07.351924Z |
| 2026-09-02 | claude | created task “Clear the dead code the version-negotiation change left behind” |
| 2026-09-02 | claude | status in_progress → done |
| 2026-09-02 | claude | evidence [] → (125 characters) |
| 2026-09-02 | claude | close_reason none → done |
| 2026-09-02 | claude | close_message none → (949 characters) |
| 2026-09-02 | claude | status todo → in_progress |
| 2026-09-02 | claude | claimed_by none → ses_864b193a-5582-4c58-bad7-f91398b0d3f8 |
| 2026-09-02 | claude | claimed_at none → 2026-09-02T20:08:02.130930Z |
| 2026-09-02 | claude | created task “Codex cannot connect: the daemon refuses MCP 2025-06-18 instead of offering one it speaks” |
| 2026-09-02 | claude | status in_progress → done |
| 2026-09-02 | claude | evidence [] → (125 characters) |
| 2026-09-02 | claude | close_reason none → done |
| 2026-09-02 | claude | close_message none → (782 characters) |
| 2026-09-02 | claude | status todo → in_progress |
| 2026-09-02 | claude | claimed_by none → ses_864b193a-5582-4c58-bad7-f91398b0d3f8 |
| 2026-09-02 | claude | claimed_at none → 2026-09-02T18:59:33.022418Z |
| 2026-09-02 | claude | revised question “Does a Mac app become the front door for other editors, and does it own the daemon?” to v1 |
| 2026-09-02 | claude | created question “Does a Mac app become the front door for other editors, and does it own the daemon?” |
| 2026-09-02 | claude | created task “The session hook goes quiet when the daemon is down, instead of saying so” |
| 2026-09-02 | claude | created feedback “A user asked for a heads-up when Specline is not connected, because it currently fails silently. In their words: "One small suggestion: a heads-up when Specline isn't connected would help, since it currently fails silently."” |
| 2026-09-02 | claude | created task “Install-script testing leaves dead lines in the tester's ~/.zshrc” |
| 2026-09-02 | claude | revised decision “The working copy lives on the external drive, and the project row points at it” to v1 |
| 2026-09-02 | claude | created decision “The working copy lives on the external drive, and the project row points at it” |
| 2026-09-02 | claude | root_path /Users/h8hcn/development/specline → /Volumes/mydrv/development/specline |
| 2026-09-02 | claude | status in_progress → done |
| 2026-09-02 | claude | evidence [] → (277 characters) |
| 2026-09-02 | claude | close_reason none → done |
| 2026-09-02 | claude | close_message none → (636 characters) |
| 2026-09-02 | claude | status todo → in_progress |
| 2026-09-02 | claude | claimed_by none → ses_864b193a-5582-4c58-bad7-f91398b0d3f8 |
| 2026-09-02 | claude | claimed_at none → 2026-09-02T18:32:14.974175Z |
| 2026-09-02 | claude | created task “Verify the repository builds and tests clean from the external drive” |
| 2026-08-19 | claude | status todo → done |
| 2026-08-19 | claude | evidence [] → (163 characters) |
| 2026-08-19 | claude | close_reason none → done |
| 2026-08-19 | claude | close_message none → (289 characters) |
| 2026-08-19 | claude | created task “Setup told a downloaded binary it had embeddings on” |
| 2026-08-19 | claude | status in_progress → done |
| 2026-08-19 | claude | evidence [] → (108 characters) |
| 2026-08-19 | claude | close_reason none → done |
| 2026-08-19 | claude | close_message none → (599 characters) |
| 2026-08-19 | claude | status in_progress → done |
| 2026-08-19 | claude | evidence [] → (116 characters) |
| 2026-08-19 | claude | close_reason none → done |
| 2026-08-19 | claude | close_message none → (253 characters) |
| 2026-08-19 | claude | status in_progress → done |
| 2026-08-19 | claude | evidence [] → (125 characters) |
| 2026-08-19 | claude | close_reason none → done |
| 2026-08-19 | claude | close_message none → (628 characters) |
| 2026-08-19 | claude | status todo → in_progress |
| 2026-08-19 | claude | claimed_by none → ses_0c88585b-90df-4dd7-aca0-f92db84044d9 |
| 2026-08-19 | claude | claimed_at none → 2026-08-19T21:00:41.082790Z |
| 2026-08-19 | claude | status todo → in_progress |
| 2026-08-19 | claude | claimed_by none → ses_0c88585b-90df-4dd7-aca0-f92db84044d9 |
| 2026-08-19 | claude | claimed_at none → 2026-08-19T21:00:39.131485Z |
| 2026-08-19 | claude | created task “Embeddings on by default, and the backlog embedded without anyone asking” |
| 2026-08-19 | claude | status todo → wont_do |
| 2026-08-19 | claude | close_reason none → wont_do |
| 2026-08-19 | claude | close_message none → (517 characters) |
| 2026-08-19 | claude | created task “Put semantic search in a released binary, by loading the ONNX runtime instead of linking it” |
| 2026-08-19 | claude | revised decision “Semantic search is on unless you turn it off, and the model arrives without being asked for” to v1 |
| 2026-08-19 | claude | created decision “Semantic search is on unless you turn it off, and the model arrives without being asked for” |
| 2026-08-19 | claude | created task “Stop building an AV1 encoder in order to embed text” |
| 2026-08-19 | claude | status todo → in_progress |
| 2026-08-19 | claude | claimed_by none → ses_0c88585b-90df-4dd7-aca0-f92db84044d9 |
| 2026-08-19 | claude | claimed_at none → 2026-08-19T20:33:42.838781Z |
| 2026-08-19 | claude | status in_progress → done |
| 2026-08-19 | claude | evidence [] → (97 characters) |
| 2026-08-19 | claude | close_reason none → done |
| 2026-08-19 | claude | close_message none → (480 characters) |
| 2026-08-19 | claude | status todo → in_progress |
| 2026-08-19 | claude | claimed_by none → ses_0c88585b-90df-4dd7-aca0-f92db84044d9 |
| 2026-08-19 | claude | claimed_at none → 2026-08-19T20:28:21.763078Z |
| 2026-08-19 | claude | created task “Run the daily-driver daemon with embeddings, and backfill the documents that have no vector” |
| 2026-08-19 | claude | status in_progress → done |
| 2026-08-19 | claude | evidence [] → ["commit:9a0b607","doc:spc_01KZKSME2TCPVARX9M04836XD6"] |
| 2026-08-19 | claude | close_reason none → done |
| 2026-08-19 | claude | close_message none → (258 characters) |
| 2026-08-19 | human | revised spec “Specline — standing instructions” to v17 |
| 2026-08-19 | claude | status todo → in_progress |
| 2026-08-19 | claude | claimed_by none → ses_e1aa5ce8-c196-4c0e-8882-d4422009c9f4 |
| 2026-08-19 | claude | claimed_at none → 2026-08-19T20:21:17.551450Z |
| 2026-08-19 | claude | unlinked “Phase 14 — Feature requests: the Inbox and the lifecycle” blocks “Cut 0.4.1 — the rail without its shortcuts, and the signal…” |
| 2026-08-19 | claude | created task “Write the release-row-then-tag loop into the standing contract” |
| 2026-08-19 | claude | status todo → done |
| 2026-08-19 | claude | evidence [] → (112 characters) |
| 2026-08-19 | claude | close_reason none → done |
| 2026-08-19 | claude | close_message none → (279 characters) |
| 2026-08-19 | claude | status in_progress → done |
| 2026-08-19 | claude | evidence [] → (151 characters) |
| 2026-08-19 | claude | close_reason none → done |
| 2026-08-19 | claude | close_message none → (327 characters) |
| 2026-08-19 | claude | status open → shipped |
| 2026-08-19 | claude | shipped_at none → 2026-08-19T20:18:09Z |
| 2026-08-19 | claude | created milestone “0.4.1 — a quieter rail, and the start of the signal lifecycle” |
| 2026-08-19 | claude | status todo → in_progress |
| 2026-08-19 | claude | claimed_by none → ses_e1aa5ce8-c196-4c0e-8882-d4422009c9f4 |
| 2026-08-19 | claude | claimed_at none → 2026-08-19T19:52:19.191523Z |
| 2026-08-19 | claude | created task “A release reads like something a person wrote, and carries five files instead of fourteen” |
| 2026-08-19 | claude | summary (385 characters) → (535 characters) |
| 2026-08-19 | claude | status in_progress → done |
| 2026-08-19 | claude | evidence [] → (126 characters) |
| 2026-08-19 | claude | close_reason none → done |
| 2026-08-19 | claude | close_message none → (381 characters) |
| 2026-08-19 | claude | created task “Teach the skill the feature-request lifecycle, once the flag flips” |
| 2026-08-19 | claude | status todo → in_progress |
| 2026-08-19 | claude | claimed_by none → ses_964b1889-ae01-4634-9dad-c0ca98e1546c |
| 2026-08-19 | claude | claimed_at none → 2026-08-19T16:33:41.853794Z |
| 2026-08-19 | claude | status in_progress → done |
| 2026-08-19 | claude | evidence [] → ["commit:131ab51","test:npx vitest run src/lib/tasks.test.ts"] |
| 2026-08-19 | claude | close_reason none → done |
| 2026-08-19 | claude | close_message none → (312 characters) |
| 2026-08-19 | claude | status todo → in_progress |
| 2026-08-19 | claude | claimed_by none → ses_964b1889-ae01-4634-9dad-c0ca98e1546c |
| 2026-08-19 | claude | claimed_at none → 2026-08-19T16:25:05.417738Z |
| 2026-08-19 | claude | status in_progress → done |
| 2026-08-19 | claude | evidence [] → (122 characters) |
| 2026-08-19 | claude | close_reason none → done |
| 2026-08-19 | claude | close_message none → (327 characters) |
| 2026-08-19 | claude | status todo → in_progress |
| 2026-08-19 | claude | claimed_by none → ses_964b1889-ae01-4634-9dad-c0ca98e1546c |
| 2026-08-19 | claude | claimed_at none → 2026-08-19T16:06:54.828842Z |
| 2026-08-19 | claude | “Phase 14 — Feature requests: the Inbox and the lifecycle” blocks “Cut 0.4.1 — the rail without its shortcuts, and the signal…” |
| 2026-08-19 | claude | created task “Cut 0.4.1 — the rail without its shortcuts, and the signal lifecycle” |
| 2026-08-19 | claude | retracted a note on tsk_01M0DB5TTNDFT9DR6DNXY3GBZ4 |
| 2026-08-19 | claude | status todo → done |
| 2026-08-19 | claude | evidence [] → ["commit:b53121e","test:npx vitest run src/App.test.tsx"] |
| 2026-08-19 | claude | close_reason none → done |
| 2026-08-19 | claude | close_message none → (265 characters) |
| 2026-08-19 | claude | triaged false → true |
| 2026-08-19 | claude | “The rail's `·1` markers read as unclear, and ⌘ was the…” informs “Take the shortcut keycaps out of the rail” |
| 2026-08-19 | claude | created task “Take the shortcut keycaps out of the rail” |
| 2026-08-19 | claude | status in_progress → done |
| 2026-08-19 | claude | evidence [] → (120 characters) |
| 2026-08-19 | claude | close_reason none → done |
| 2026-08-19 | claude | close_message none → (291 characters) |
| 2026-08-19 | claude | status todo → in_progress |
| 2026-08-19 | claude | claimed_by none → ses_964b1889-ae01-4634-9dad-c0ca98e1546c |
| 2026-08-19 | claude | claimed_at none → 2026-08-19T14:58:39.995682Z |
| 2026-08-19 | claude | created task “Put the Inbox behind a flag, off by default, until the lifecycle is finished” |
| 2026-08-19 | claude | status todo → done |
| 2026-08-19 | claude | evidence [] → (100 characters) |
| 2026-08-19 | claude | close_reason none → done |
| 2026-08-19 | claude | close_message none → (423 characters) |
| 2026-08-19 | claude | “Closing is what you do to anything that is dealt with, not…” resolves “How does triage reach MCP without a fourteenth tool?” |
| 2026-08-19 | claude | status proposed → accepted |
| 2026-08-19 | claude | revised decision “Closing is what you do to anything that is dealt with, not only to a task” to v1 |
| 2026-08-19 | claude | created decision “Closing is what you do to anything that is dealt with, not only to a task” |
| 2026-08-19 | claude | status in_progress → done |
| 2026-08-19 | claude | evidence [] → ["commit:f3dcc8a","test:cargo test -p specline-core --test inbox"] |
| 2026-08-19 | claude | close_reason none → done |
| 2026-08-19 | claude | close_message none → (379 characters) |
| 2026-08-19 | claude | created task “A signal can be cleared out of the Inbox with no outcome, going round work::triage” |
| 2026-08-19 | claude | revised question “How does triage reach MCP without a fourteenth tool?” to v1 |
| 2026-08-19 | claude | created question “How does triage reach MCP without a fourteenth tool?” |
| 2026-08-19 | claude | created task “Notes are not in the search index, so every finding recorded on a task is unfindable” |
| 2026-08-19 | claude | status todo → in_progress |
| 2026-08-19 | claude | claimed_by none → ses_964b1889-ae01-4634-9dad-c0ca98e1546c |
| 2026-08-19 | claude | claimed_at none → 2026-08-19T14:16:58.472793Z |
| 2026-08-19 | claude | status in_progress → done |
| 2026-08-19 | claude | evidence [] → ["commit:76bc1c0","test:cargo test -p specline-core --test inbox"] |
| 2026-08-19 | claude | close_reason none → done |
| 2026-08-19 | claude | close_message none → (271 characters) |
| 2026-08-19 | claude | status todo → in_progress |
| 2026-08-19 | claude | claimed_by none → ses_964b1889-ae01-4634-9dad-c0ca98e1546c |
| 2026-08-19 | claude | claimed_at none → 2026-08-19T14:10:27.785045Z |
| 2026-08-19 | claude | status in_progress → done |
| 2026-08-19 | claude | evidence [] → (268 characters) |
| 2026-08-19 | claude | close_reason none → done |
| 2026-08-19 | claude | close_message none → (339 characters) |
| 2026-08-19 | claude | created task “A task that turns out to be a signal has no honest way to close” |
| 2026-08-19 | claude | “Support openai codex” references “Specline should work with OpenAI Codex, not only Claude Code” |
| 2026-08-19 | claude | “allow adding new Feature Requests” references “A feature request needs somewhere to live that is not a…” |
| 2026-08-19 | claude | “periodic management of lots of open issues” references “Open work piles up until it is too expensive to read, and…” |
| 2026-08-19 | claude | status todo → wont_do |
| 2026-08-19 | claude | close_reason none → wont_do |
| 2026-08-19 | claude | close_message none → (343 characters) |
| 2026-08-19 | claude | status todo → wont_do |
| 2026-08-19 | claude | close_reason none → wont_do |
| 2026-08-19 | claude | close_message none → (253 characters) |
| 2026-08-19 | claude | status todo → wont_do |
| 2026-08-19 | claude | close_reason none → wont_do |
| 2026-08-19 | claude | close_message none → (295 characters) |
| 2026-08-19 | claude | revised feedback “A feature request needs somewhere to live that is not a task, and it should break into subtasks” to v1 |
| 2026-08-19 | claude | created feedback “A feature request needs somewhere to live that is not a task, and it should break into subtasks” |

*Showing the 200 most recent of 2674 changes. Use `specline_activity` for the rest.*

