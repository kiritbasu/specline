# Contributing

Thanks for looking. Specline is one person's project with a public repository, so
what follows is honest about what that means rather than pretending to be a
foundation.

## Before you write code

**Open an issue first for anything beyond a bug fix.** Specline has strong opinions
and most of them are written down; a change that cuts against one is a wasted
afternoon for you and an awkward conversation for me. `product/PRD.md` says what
it is for, `product/SPEC.md` says how it works, and `product/DECISIONS.md` says
why things are the way they are — including several things that were tried and
rejected.

Small, obvious fixes need no ceremony. Send them.

## The thing that will trip you up

**Everything under `product/` and `.specline/` is generated.** Each file carries a
banner saying so. Editing one is not wrong so much as futile: the next
`specline generate` overwrites it from the store, and whatever you wrote is gone
with no trace. If a generated file is wrong, the fix is upstream of it — say so
in the issue and it will be fixed at the source.

`scripts/pre-commit` refuses a commit that hand-edits one. Install it:

```bash
ln -sf ../../scripts/pre-commit .git/hooks/pre-commit
```

The repository root's `CLAUDE.md` is the one file under those rules that is not
generated, because it is what loads them.

## Building and the gate

### Getting a working install

```bash
git clone https://github.com/kiritbasu/specline.git && cd specline
```

```bash
./plugin/install.sh
```

That builds both binaries and puts them in `~/.cargo/bin`, which is where a
release installs them too, so you only ever have one copy. It also creates the
store and copies the skill and hooks into `~/.claude/`.

After editing anything under `plugin/`, run `./plugin/install.sh --skill-only`,
which skips the build and copies the three files across. The copies under
`~/.claude` are what run, so a repository change you do not copy has no effect.

The screenshots in the README come from `specline fixture`, which loads an
invented corpus into an empty store; `scripts/shoot-screenshots.mjs` retakes
them.

### The checks

```bash
cargo build --workspace
```

Before you open a pull request, run what CI runs:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

**Run these through the pinned toolchain.** `rust-toolchain.toml` pins the
version and a Homebrew `cargo` on your `PATH` ignores it entirely, so you can be
checking a different compiler from the one CI uses and pass. Either put
`~/.cargo/bin` ahead of Homebrew on `PATH`, or run each command as
`rustup run <version> cargo …`.

**On Linux, or an Intel Mac, add `--exclude specline-embed`.** That crate pulls a
prebuilt ONNX runtime which has no Intel macOS build and wants a newer glibc
than the Linux binaries are built against, so it cannot compile there at all.
Everything else can:

```bash
cargo test --workspace --exclude specline-embed --no-default-features
```

The `embeddings` feature is on by default and that command turns it off, which
is the configuration those platforms ship. Semantic search is what it costs;
keyword search covers every artifact either way.

The desktop app has its own:

```bash
npm --prefix apps/desktop run typecheck
npm --prefix apps/desktop test
```

## What a change is expected to carry

- **A test, including a failing case.** Not only the happy path. A test that
  cannot fail is a description.
- **No `unwrap()`, `expect()` or `panic!()` in library code.** Binaries and
  tests may, with a message. This is a clippy lint, so it is a build failure
  rather than review discipline.
- **Comments that say why, not what.** The codebase is written to explain its
  own reasoning, particularly where something is surprising or was got wrong
  once. Matching that is the main thing a review will ask you for.
- **A commit message that explains the change rather than the diff**, with a
  conventional prefix: `feat:`, `fix:`, `refactor:`, `test:`, `docs:`, `chore:`.

## Pull requests

Branch off `main`, keep it short-lived, open the PR when it is green. CI runs
Linux for every pull request; the macOS leg runs on a self-hosted machine and is
deliberately skipped for pull requests from forks, so a green Linux run is what
is being asked of you.

By contributing you agree that your contribution is licensed under Apache-2.0,
the same as the rest of the repository.

## Reporting a bug

Include what you ran, what happened, and what you expected. `specline doctor` prints
a page of diagnostics and its output is usually the fastest way to a cause —
read it before you paste it, since it names paths on your machine.

Security problems go through the private route instead. See `SECURITY.md`.
