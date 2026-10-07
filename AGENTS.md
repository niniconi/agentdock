# AGENTS.md

## What this is

A single-binary Rust CLI that manages long-lived Docker containers for AI agents, plus a
`worktree` subcommand that restructures a project into a container of git worktrees so
several agents can work in parallel.

## Project status

Pre-release: `0.1.0`, untagged. Breaking changes are cheap here, so do not add upgrade
shims or migration code for state an earlier build wrote, and do not keep a superseded field
alive out of caution. Remove the old path rather than carrying it for someone who never
shipped it.

Two files persist across builds, so a format change in either is felt by anyone who ran an
earlier version:

- **`~/.config/agentdock/records.json`.** `Record` carries no version field. Its `Option`
  fields use `#[serde(default)]` and so tolerate a format change; its required fields do
  not, so adding one makes an older file fail to parse and takes `list`, `status` and `delete`
  down together. That is an accepted outcome, not something to code around. `kvm` is a
  required field, added because `docker run` consumes it and without recording it a later
  change could never be applied. `persist` is the opposite case: it is `Option<Vec<String>>`
  even though `docker run` consumes it too, because its container-side paths are **derived
  from the image** when `apply` runs rather than stored, so there is nothing an older record
  would be missing.
- **`.agentdock.json`.** Unlike records, this one marks a project whose directory `worktree
  init` has already **renamed**. `MARKER_VERSION` is written but `read_marker` never compares
  it, so a marker with any value deserializes. A change to how worktrees are laid out or
  named would therefore be applied silently to an already-converted project, whose original
  location no longer exists.

## Commands

```bash
# build
cargo build
cargo build --release

# test
cargo test                        # unit + integration
cargo test --test worktree        # worktree integration suite only
cargo test --test worktree <fn>   # a single test

# the gate, which ci.yml runs on every push
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## Environment requirements

Both `docker` and `git` are invoked as plain binaries through `std::process::Command`.
Neither is checked up front, so a missing tool surfaces late, but both now report the tool by
name (`docker is not installed or not on PATH`) rather than a bare OS error.

- **`git` must be on PATH** for the `worktree` subcommand and for the integration suite.
- **`docker` must be on PATH** for `apply`/`up`/`list`/`delete`/`status`. Without it, `apply`
  prints `Creating container: ...` and then fails with
  `docker is not installed or not on PATH`. Records are written only after the container
  starts, so a failure here leaves no stale state.
- **Unit tests do not need git.** Only the integration suite does.

## Testing

`tests/worktree.rs` drives the real binary through `env!("CARGO_BIN_EXE_agentdock")`, and it
relocates real directories while doing so. Read it before touching that area.

**Known gap:** every test returns early instead of failing when `git --version` cannot be
executed. On a machine without git, the suite reports `ok` in about 0.01s having run
nothing. Do not treat a green suite as proof of anything on such a machine.

`tests/docker_real.rs` is the same shape, pointed at the daemon instead of git: it drives
the real binary through real containers (local: skips cleanly when no daemon). `tests/record_image.rs` keeps the stub `docker` and remains the place to
assert exact command sequences. The stub's own assumptions about the daemon (inspect key
spellings, config keys) are only checkable through `docker_real.rs`, so a change to
`src/docker/` that touches the daemon interface belongs in both.

## Architecture

Single crate, binary-only (no lib target), so `pub` on items in `src/` does not mean public
API. Most are `pub` only to cross module boundaries.

- `src/main.rs` parses args and dispatches. Holds no logic.
- `src/commands/` is one file per subcommand, each exposing `execute_*`. This is the
  established pattern, so add new subcommands here.
- `src/cli/args.rs` holds every clap struct. `ApplyOpts` is flattened into both `ApplyArgs` and
  `WorktreeAddArgs` so the two share container flags. Its `agent` is a plain `String` carrying a
  clap `default_value`, which applies to every call rather than only the one creating a container,
  and its `port` is a `Vec` so that an omitted flag and a cleared one are the same empty list —
  both mean "none", since `apply` replaces rather than merges.
- `Config` in `src/config.rs` is what a container should be built like, with every field
  carrying a value. `Config::new` builds one from the command line alone, `Config::of` reads one
  back from a record and `Config::to_record` writes it. There is deliberately **no** `PartialEq`
  on it and nothing compares a config against a record: the flags are the only source of the
  configuration, so a comparison would have nothing to decide. `apply` always rebuilds for that
  reason, and `up` carries no configuration flags, which is what keeps a bare `up` from
  rebuilding anything.
- `src/docker/client.rs` shells out to `docker`. Note the one exception: `exec` runs
  `docker exec -it <name> sh -c <command>`, and `container.rs` passes the agent name from
  `-a {image}/{agent_name}` into that shell, so a crafted agent name is interpreted by the
  container's shell. This is pre-existing; do not introduce anything like it.
- `src/persist.rs` decides where `--persist` mounts from. Two halves that must not drift: the
  host side is `<XDG_DATA_HOME|~/.local/share>/agentdock/<container>/<agent>/<what>`. Three
  readers reach it through `container_data_dir` rather than rebuilding it — `plan_mounts`,
  `delete` for both the report and `--purge`, and `worktree rm` naming what it kept —
  while `container_data_dirs` lists the same layout one entry per (agent, kind), relative to
  the data home rather than to a container's own directory, for `list -v`, which prints one
  column and would be stretched by a repeated absolute path on every row. `container_data_dir`
  rejects a name that is not a single path segment: `PathBuf::join` resolves `..` and lets an
  absolute component replace the base, which would turn `--purge` into a recursive delete of a
  directory agentdock did not create. The name comes from `delete`'s positional argument, from
  `-n`, from a UUID, or from `records.json` — and that last is a plain user-owned file.
  The container side comes from `DockerClient::image_config`, which is `docker image inspect`
  with a pull retry — deliberately **not** `exec`, since `exec` is `docker exec -it` and needs a
  TTY. Its `#[serde(rename_all = "PascalCase")]` is load-bearing: `{{json .Config}}` prints a Go
  struct, so the keys are `User` and `Env`, and serde matches field names exactly while ignoring
  keys it does not recognise. Without the rename every field deserializes as `None` for every
  real image, and a `USER=node` image is then indistinguishable from one declaring nothing.
  `SUPPORTED` is a list of agents and their directories with **no lookup** on the image or
  on the agent named in `-a`: every entry is mounted for every container. Keying on the image
  put `nixos`, `nixos:latest` and `ghcr.io/owner/nixos` in separate arms, so pinning a tag meant
  editing the table; and keying on `-a` would refuse persistence to a container entered with
  `bash`, which is one whose agent has not been started yet.
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
- Failures a user can act on belong in `src/error.rs` as a `thiserror` variant
  (`WorktreeError`, `ContainerError`, `RecordError`, `GitError`) raised with `bail!`. The
  variant carries the path or name it interpolates and owns its entire message, suggested
  commands included, so no user-facing sentence is assembled at a call site.
- Failures that are pure plumbing (IO, serde) stay as `.context("...")` on the underlying
  error. That is what preserves the `Caused by:` chain, so prefer it over a variant that
  drops the source. The two invariants in `git.rs` (`Failed to determine the worktree root`,
  `Failed to determine the current branch`) are the deliberate exception and stay plain
  `bail!` strings.
- `.github/workflows/release.yml` is written by `dist generate` and **must not be hand
  edited**; a change there is lost the next time the config is regenerated. Change
  `dist-workspace.toml` and re-run `dist generate` instead, then read the diff: dist does
  not check that a combination of settings is coherent, so a wrong pairing is generated
  confidently and only the CI run reports it. That is how `github-attestations-phase` once
  granted `id-token: write` to one job while putting the attest step in another. The
  release workflow also runs its plan step on pull requests, so a broken release
  configuration should surface there rather than at the next tag.
- Dependabot covers the `cargo` ecosystem only. `release.yml` is regenerated, so an
  actions bump proposed there would be undone; `ci.yml` is hand written and could be
  covered separately if its two actions ever need it.
