# AgentDock

English | [简体中文](README.zh-CN.md)

Docker-based AI Agent Manager. Manages persistent Docker containers for running AI agents.

## Usage

```bash
agentdock <COMMAND>
```

### Commands

| Command | Description |
| --- | --- |
| `apply` | Create a container, or change an existing one to match the given settings |
| `up` | Start an existing container without changing how it was built |
| `list` | List all managed containers |
| `delete` | Delete a container and its record |
| `status` | Show container status |
| `worktree` | Convert a project into git worktree form |

## Applying and starting

`apply` is where a container's configuration lives. Given a name or a directory
with no record, it creates one; given one that already exists, it changes that
container to match.

```bash
agentdock apply -a nixos/opencode -P 8080:80
agentdock apply -n box -P 9090:80      # change the published port
```

A flag you leave out is a flag that is off. `apply` replaces the configuration
rather than merging into it, which matches `docker run`: the container is built
from the arguments it was given and nothing else. So repeating an apply with
fewer flags does strip a setting, and the flags have to be repeated to keep it.

`up` starts a container and takes no configuration flags, so nothing passed to
it can change how the container was built:

```bash
agentdock up -n box        # start, or restart if it is stopped
agentdock up               # the container serving this directory
```

It does not create containers. Where there is nothing to start it says so and
points at `apply`.

### Persisting an agent's own data

The workspace mount is the only thing a container gets, so an agent's config and
history live inside the container and go when it is rebuilt. `--persist` mounts
them from the host instead:

```bash
agentdock apply -a nixos/opencode --persist              # config and data
agentdock apply -a nixos/opencode --persist config       # config only
agentdock apply -a nixos/opencode --persist data         # data only
agentdock apply -a nixos/bash     --persist              # same data, other entry point
```

You do not say where. They land under
`$XDG_DATA_HOME/agentdock/<container>/<agent>/<what>` — `~/.local/share` unless
you have set that — one directory per container, so two agents on two branches
never write the same database:

```
~/.local/share/agentdock/
  box/
    opencode/
      config/
      data/
```

Every directory agentdock knows about is mounted for every container, whichever
agent `-a` names. That flag picks the entry point rather than the owner: a
container entered with `bash` is one whose agent you have not started yet, so
mounting for `opencode` regardless is what lets you look inside it and find the
data still there. Adding an agent means one line in `src/persist.rs` per
directory it keeps.

Inside the container the paths follow from the image, since agentdock has to
name an absolute path and only the image knows which user it runs as. An image
with no `USER` runs as root, so its home is `/root`; otherwise the `HOME` the
image declares is used. A named user with no `HOME` declared is the case left,
where agentdock assumes `/home/<user>` and says so.

`agentdock list -v` shows which directories each container persists, as paths
under `~/.local/share/agentdock/` — the same directory `--purge` removes, one
level up from the container name in each row. Being off by default is
deliberate: `apply` builds from the flags it is given, so a flag that turned
itself on would be the one flag whose absence did not mean absence.

Removing a container leaves that directory alone, since it is outside the
container and `delete` says where it went:

```bash
agentdock delete box --force          # keeps the data, and says where
agentdock delete box --force --purge  # removes it too
```

`--purge` is never the default. That data is the only copy of your credentials
and conversation history, so erasing it is a separate decision from removing a
container.

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
agentdock worktree add feat/login --apply --agent nixos/opencode
agentdock worktree list
agentdock worktree rm feat/login
```

Commands that take a worktree (`add`, `rm`) name it by branch, not by
directory. A detached worktree has no branch, so `worktree list` shows its short
commit id in the `BRANCH` column, and that is what `worktree rm` accepts:

```
ROLE      BRANCH     PATH                  HEAD      CONTAINER
main      main       myproject-main        8f2a1c4   -
worktree  8f2a1c4   myproject-8f2a1c4    8f2a1c4   -
```

Because a detached worktree is named after its short commit id, that name
cannot be reused for a new branch.

`--apply` creates the worktree and then creates a container inside it, so the
container is bound to that worktree's path. Container options (`--agent`,
`--port`, `--kvm`, proxies, `--init`) are shared with `apply`; `--path` and
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
