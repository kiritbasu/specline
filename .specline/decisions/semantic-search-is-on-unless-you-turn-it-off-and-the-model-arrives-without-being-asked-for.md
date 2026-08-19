<!-- specline:generated decision dec_01M0DWM0GZ0AC0R0JWPKQ1DQWF v1 2026-08-19T21:04:12Z
     source of truth is Specline — edits here are not saved -->
# B-95 — Semantic search is on unless you turn it off, and the model arrives without being asked for

**Status:** `accepted`  
**Id:** `dec_01M0DWM0GZ0AC0R0JWPKQ1DQWF`

KB decided, 2026-08-19: "even in production environments for anyone downloading the app embeddings are created automatically, and they don't have to set anything up manually."

This reverses the position recorded on KEEL-211 — "opt-in behind a visible prompt in setup, never a silent pull" — and the reversal is worth stating plainly rather than letting the newer instruction quietly win.

**What the old position was protecting.** Turning semantic search on downloads a 127 MB model. The argument was that keyword search works without it, so the offer is "better search later" rather than "search is broken until you agree", which makes consent honest instead of coerced.

**Why it loses anyway.** The default was off, so the thing that actually happened is that nobody ever turned it on. This machine ran with a working model on disk, a binary that could load it, and every search answering from the keyword half alone — and nothing in any response said so. A consent prompt protects somebody from a download they did not expect. It does not protect them from a product whose headline capability is off, which is the worse outcome and the one that occurred.

**What is decided:**

- The daemon loads a model when its build has one, unless started with `--no-embeddings`. The flag survives, inverted, because "I do not want a 127 MB download on this machine" is a real thing to want.
- The first start fetches the model, says so in the log while it happens, and degrades to keyword-only if it cannot — no network, no disk, no model is a warning and never a failure to start.
- A daemon that finds documents with no vector backfills them rather than printing an instruction to run `specline reembed --missing`. An install that upgraded into this should not inherit a chore.
- `specline reembed` stays, for the case where somebody deliberately declined and later changed their mind.

**What this does not decide.** Whether a released binary can do any of it. Every published archive is built with the feature off, because `ort-sys` has no prebuilt ONNX Runtime for Intel macOS — checked again on 2026-08-19 against `ort-sys` 2.0.0-rc.13, whose prebuilt list still names nine targets and not that one. So "anyone downloading the app" is not yet true of anyone, and the route to making it true is its own piece of work.

