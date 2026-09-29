# AGENTS.md

## What this is

A single-binary Rust CLI that manages long-lived Docker containers for AI agents, plus a
`worktree` subcommand that restructures a project into a container of git worktrees so
several agents can work in parallel.

## Commands

```bash
# build
cargo build
cargo build --release

# test
cargo test                        # unit + integration
cargo test --test worktree        # worktree integration suite only
cargo test --test worktree <fn>   # a single test

# gate (there is no CI, so these three stand in for it)
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## Environment requirements

Both `docker` and `git` are invoked as plain binaries through `std::process::Command`.
Neither is checked up front, so a missing tool surfaces late and opaquely.

- **`git` must be on PATH** for the `worktree` subcommand and for the integration suite.
- **`docker` must be on PATH** for `run`/`list`/`delete`/`status`. Without it, `run` prints
  `Starting new container: ...` and then fails with
  `No such file or directory (os error 2)`. That error is the missing binary, not a bug in
  the change you are testing. Records are written only after the container starts, so a
  failure here leaves no stale state.
- **Unit tests do not need git.** Only the integration suite does.

## Testing

`tests/worktree.rs` drives the real binary through `env!("CARGO_BIN_EXE_agentdock")`, and it
relocates real directories while doing so. Read it before touching that area.

**Known gap:** every test returns early instead of failing when `git --version` cannot be
executed. On a machine without git, all 16 tests report `ok` in about 0.01s having run
nothing. Do not treat a green suite as proof of anything on such a machine.

## Architecture

Single crate, binary-only (no lib target), so `pub` on items in `src/` does not mean public
API. Most are `pub` only to cross module boundaries.

- `src/main.rs` parses args and dispatches. Holds no logic.
- `src/commands/` is one file per subcommand, each exposing `execute_*`. This is the
  established pattern, so add new subcommands here.
- `src/cli/args.rs` holds every clap struct. `RunOpts` is flattened into both `RunArgs` and
  `WorktreeAddArgs` so the two share container flags.
- `src/docker/client.rs` shells out to `docker`. Note the one exception: `exec` runs
  `docker exec -it <name> sh -c <command>`, and `container.rs` passes the agent name from
  `-a {image}/{agent_name}` into that shell, so a crafted agent name is interpreted by the
  container's shell. This is pre-existing; do not introduce anything like it.
- `src/worktree/` is the `worktree` subcommand, where `git.rs` is a thin `git` CLI wrapper and
  `mod.rs` holds orchestration.
- `src/state/persistence.rs` reads and writes `~/.config/agentdock/records.json`. Both
  `find_by_path` and `find_within` use `canonicalize().ok()?`, so they **skip records whose
  path no longer exists**. That is why `worktree rm` can skip its attached-container check for
  an already-deleted worktree, leaving a dangling record.

## Behavior worth knowing before changing `worktree`

`worktree init` relocates the whole repository directory rather than the files alone, which
is what keeps the git index, uncommitted changes and untracked files untouched. The
relocation is a three-step `rename`/`create_dir`/`rename` through a hidden sibling
`.{repo}.agentdock-stage`. Every failure path after the first `rename` must roll the
repository back, or the project disappears from its original location.

All validation happens before that first `rename`, and `init` refuses rather than converting
partially, because a half-converted project is worse than none. A new check therefore belongs
in the pre-flight loop, never after the move.

One known blocker: when a repository-relative remote URL points at a path that does not
exist, `absolutize_remote_url` canonicalizes, the error propagates, and `init` aborts. It
fails before the relocation, so the project is safe, but the conversion is held up by an
unrelated remote.

A detached worktree has no branch, so `init` names its directory `{repo}-{short-commit}` and
`worktree list` shows that id in the `BRANCH` column, which is what `worktree rm` accepts.
That is why `find_worktree` takes a `Match` mode: `rm` needs the loose fallbacks, `add` must
not have them, or it rejects valid branch names that merely resemble a detached directory.

## Conventions

- Commit messages follow Conventional Commits (`feat:`, `fix:`, `refactor:`, `docs:`) with a
  body of `-` bullets. History is on `master` and pushed directly to it.
- `rustfmt` and `clippy -D warnings` are clean across the tree. Match that.
- Error text for user-facing failures belongs in `src/error.rs` as `*_error()` helpers taking
  paths and names, not inline `bail!` strings. Follow that when adding a failure mode.
