# AgentDock

Docker-based AI Agent Manager. Manages persistent Docker containers for running AI agents.

## Usage

```bash
agentdock <COMMAND>
```

### Commands

| Command | Description |
| --- | --- |
| `run` | Start or manage an AI agent container |
| `list` | List all managed containers |
| `delete` | Delete a container and its record |
| `status` | Show container status |
| `worktree` | Convert a project into git worktree form |

## Worktrees

`agentdock worktree` restructures a single checkout into a container of independent
git worktrees, so several agents can work on the same project at the same time
without sharing a working directory.

```bash
# inside an existing project
agentdock worktree init
```

```
myproject/
  .agentdock.json
  myproject-main/     # was the original checkout
  myproject-dev/
  myproject-feat-login/
```

A worktree on a branch lives in a directory named `{repo}-{branch}`, with
slashes flattened, and each one can run its own agent container:

```bash
agentdock worktree add dev
agentdock worktree add feat/login --run --agent nixos/opencode
agentdock worktree list
agentdock worktree rm feat/login
```

Commands that take a worktree (`add`, `rm`) name it by branch, not by
directory. A detached worktree has no branch, so `worktree list` shows its short
commit id in the `BRANCH` column, and that is what `worktree rm` accepts:

```
ROLE      BRANCH  PATH                 HEAD
main      main    myproject-main       8f2a1c4
worktree  8f2a1c4  myproject-8f2a1c4   8f2a1c4
```

Because a detached worktree is named after its short commit id, that name
cannot be reused for a new branch.

`--run` creates the worktree and then starts a container inside it, so the
container is bound to that worktree's path. Container options (`--agent`,
`--port`, `--kvm`, proxies, `--init`) are shared with `run`; `--path` and
`--name` are not, because the worktree determines both.

`--start-point <branch-or-commit>` bases a newly created branch on something
other than the current `HEAD`. It only applies to branches agentdock creates,
so it cannot be combined with a branch that already exists.

Because the worktree directory doubles as the container mount path, each
worktree gets an independent container via the usual path-based de-duplication.

`worktree init` moves the whole repository directory rather than the files alone,
which keeps the index, uncommitted changes and untracked files untouched. Any
linked worktrees that already exist are moved into the container as well, and
their `.git` pointers are repaired afterwards, since those hold absolute paths
to their old locations.

`worktree init` either converts the project completely or leaves it untouched.
It refuses to run when the layout could not be resolved up front:

- the project already has agentdock containers mounted inside it, because
  those records point at the pre-move paths
- a linked worktree lives inside the repository, which would relocate it out
  from under git. Move it out first with `git worktree move <path> <outside>`
- two branches flatten to the same directory name, e.g. `a/b` and `a-b`
- a leftover `.myproject.agentdock-stage` directory is present, which means a
  previous conversion was interrupted

`worktree list` shows a worktree as `stale` when its directory has been deleted
while git still tracks it. Remove it with `worktree rm <branch>` to clear the
registration.

`worktree rm` refuses while a container is attached to the worktree. Delete the
container first with `agentdock delete <name>`, or pass `--force` to drop both.
`--force` also discards any uncommitted changes in that worktree, so without it
git rejects the removal instead.

## Build

```bash
cargo build --release
```
