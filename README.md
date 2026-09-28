# AgentDock

Docker-based AI Agent Manager. Manages persistent Docker containers for running AI agents.

## Usage

```bash
agentdock [OPTIONS]
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

Every worktree lives in its own directory named `{repo}-{branch}`, and each one
can run its own agent container:

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
`--port`, `--kvm`, proxies, `--init`) are shared with `run`.

Because the worktree directory doubles as the container mount path, each
worktree gets an independent container via the usual path-based de-duplication.

`worktree init` moves the whole repository directory rather than the files alone,
which keeps the index, uncommitted changes and untracked files untouched. It
refuses to run if the project already has agentdock containers mounted inside
it, since those records point at the pre-move paths.

## Build

```bash
cargo build --release
```
