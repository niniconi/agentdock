# AgentDock

[English](README.md) | 简体中文

基于 Docker 的 AI Agent 管理器。管理用于运行 AI agent 的常驻容器。

## 用法

```bash
agentdock <COMMAND>
```

### 子命令

| 命令 | 说明 |
| --- | --- |
| `apply` | 创建容器，或把已有容器改成给定的配置 |
| `up` | 启动已有容器，不改变它当初的构建方式 |
| `list` | 列出所有受管理的容器 |
| `delete` | 删除容器及其记录 |
| `status` | 显示容器状态 |
| `worktree` | 把一个项目转换成 git worktree 形式 |

## 应用与启动

容器的配置由 `apply` 负责。给一个名字或一个没有记录的目录，它会创建容器；
如果那个容器已经存在，它会把该容器改成给定的配置。

```bash
agentdock apply -a nixos/opencode -P 8080:80
agentdock apply -n box -P 9090:80      # 改一个已发布的端口
```

**没传的 flag 就是关掉的 flag。** `apply` 是替换配置而不是合并进去，这与 `docker run`
一致：容器只按给它的参数构建，除此之外什么都不看。所以少传几个 flag 再执行一次
`apply`，确实会丢掉某项设置 —— 想保留它就得把 flag 再传一遍。

`up` 启动容器，不接受任何配置类 flag，因此传给它的东西无法改变容器当初是怎么被构建的：

```bash
agentdock up -n box        # 启动；若已停止则重启
agentdock up               # 服务当前目录的那个容器
```

`up` 不创建容器。没有东西可启动时它会说明情况并指向 `apply`。

### 持久化 agent 自己的数据

容器唯一得到的挂载是工作目录，所以 agent 的配置和对话历史都写在容器内部，
容器一重建就没了。`--persist` 改为从宿主机挂载它们：

```bash
agentdock apply -a nixos/opencode --persist              # config 与 data
agentdock apply -a nixos/opencode --persist config       # 只 config
agentdock apply -a nixos/opencode --persist data         # 只 data
agentdock apply -a nixos/bash     --persist              # 同一份数据，换个入口程序
```

你不需要指定放在哪里。它们会落在
`$XDG_DATA_HOME/agentdock/<容器名>/<agent>/<哪一项>` —— 没设 `XDG_DATA_HOME`
时就是 `~/.local/share` —— 并且每个容器一个目录，这样两个分支上的 agent 不会写同一个数据库：

```
~/.local/share/agentdock/
  box/
    opencode/
      config/
      data/
```

agentdock 知道的每一个目录都会挂进每一个容器，与 `-a` 写的是什么 agent 无关。
那个 flag 决定的是入口程序而不是数据归属：一个用 `bash` 进去的容器，往往是它的
agent 还没启动过，所以无论如何都挂 `opencode` 的目录，你才能进去看到那份数据。
新增一个 agent 是在 `src/persist.rs` 里每个目录写一行。

容器内的路径由镜像决定，因为 agentdock 必须给出一个绝对路径，而只有镜像知道自己
以哪个用户运行。镜像没有 `USER` 声明时以 root 运行，home 就是 `/root`；否则用镜像
声明的 `HOME`。剩下一种情况是有用户名但没声明 `HOME`，这时 agentdock 假定
`/home/<用户名>` 并会说明它这么假定了。

`agentdock list -v` 会显示每个容器持久化了哪些目录，路径相对于
`~/.local/share/agentdock/` —— 也就是 `--purge` 会删掉的那个目录，比每行里的容器名
高一层。默认关闭是刻意的：`apply` 只按给它的 flag 构建容器，一个会自己打开的 flag
会是唯一一个「不传就意味着存在」的 flag。

删除容器不会动那个目录，因为它在容器之外，而 `delete` 会告诉你它在哪：

```bash
agentdock delete box --force          # 保留数据，并说明位置
agentdock delete box --force --purge  # 一并删除
```

`--purge` 永远不会成为默认值。那份数据是你的凭证和对话历史的唯一副本，
所以「删掉它」应该是与「删掉容器」分开的两个决定。

### 用你自己的配置给容器做模板

`--template` 会把宿主机的 `~/.config/opencode` 当作起点，新建的容器就自带你的
MCP 和 skills，不必再装一遍：

```bash
agentdock apply -a nixos/opencode --persist --template
```

当 config 被持久化时，每个容器的 config 目录在第一次创建时会从宿主机目录种入。
已经存在的文件不动，所以只种一次，之后 agent 自己写的内容会保留。当 config 没被
持久化时，容器看不到宿主机目录，于是每次 apply 都把宿主机配置直接复制进容器 ——
反正它随容器一起丢弃。两种情况里，如果你没有 `~/.config/opencode`，这个 flag
什么都不做。

## Worktree

`agentdock worktree` 把一个检出目录改造成一组独立的 git worktree，
这样多个 agent 就能同时在同一个项目上工作而不共用工作目录。

```bash
# 在已有项目内执行
agentdock worktree init
```

```
myproject/
  .agentdock.json
  myproject-main/     # 原来的检出目录
  myproject-dev/
  myproject-feat-login/
```

分支对应的 worktree 放在名为 `{仓库名}-{分支名}` 的目录里，分支名里的斜杠会被
拍平，每一个 worktree 都可以跑自己的 agent 容器：

```bash
agentdock worktree add dev
agentdock worktree add feat/login --apply --agent nixos/opencode
agentdock worktree list
agentdock worktree rm feat/login
```

需要指定 worktree 的命令（`add`、`rm`）按**分支名**指定，而不是目录名。
游离（detached）的 worktree 没有分支，所以 `worktree list` 会在 `BRANCH` 列里
显示它的短 commit id，而 `worktree rm` 接受的就是这个值：

```
ROLE      BRANCH     PATH                  HEAD      CONTAINER
main      main       myproject-main        8f2a1c4   -
worktree  8f2a1c4   myproject-8f2a1c4    8f2a1c4   -
```

由于游离 worktree 是以它的短 commit id 命名的，这个名字不能再被新分支复用。

`--apply` 会创建 worktree，然后在其中创建一个容器，因此该容器与这个 worktree 的路径
绑定。容器选项（`--agent`、`--port`、`--kvm`、代理、`--init`）与 `apply` 共用；
`--path` 和 `--name` 不共用，因为这两者由 worktree 决定。

`--start-point <分支或提交>` 让新建的分支基于 `HEAD` 之外的东西。它只对 agentdock
自己创建的分支有效，所以不能与一个已存在的分支组合使用。

由于 worktree 目录同时充当容器的挂载路径，每个 worktree 会通过通常的按路径去重
得到一个独立的容器。

`worktree init` 移动的是整个仓库目录而不只是文件本身，这样索引、未提交的改动和
未跟踪的文件都不会被碰到。已经存在的链接 worktree 也会一并移进容器，之后修复
它们的 `.git` 指针，因为那些指针里存的是指向旧位置的绝对路径。

`worktree init` 要么完全转换这个项目，要么原样不动。遇到无法事先解析的布局时
它会拒绝执行：

- 项目里已经挂有 agentdock 容器，因为那些记录指向移动前的路径
- 有链接 worktree 位于仓库内部，那会把它从 git 底下挪走。先用
  `git worktree move <路径> <外部路径>` 把它移出去
- 两个分支拍平后得到同一个目录名，例如 `a/b` 和 `a-b`
- 存在残留的 `.myproject.agentdock-stage` 目录，说明上一次转换被中断了

当 worktree 的目录已被删除而 git 仍在跟踪它时，`worktree list` 会把它显示为
`stale`。用 `worktree rm <分支>` 清除该注册记录。

worktree 上还挂着容器时，`worktree rm` 会拒绝执行。先用
`agentdock delete <容器名>` 删掉容器，或者传 `--force` 把两者一起删掉。
`--force` 还会丢弃该 worktree 里未提交的改动，所以不加它时 git 会拒绝这次删除。

## 构建

```bash
cargo build --release
```