# Fuchsia Git Staging & Commit Hooks: Architecture & Reference Guide

## 1. Overview & Current Architecture

Fuchsia developer workflows span multiple Git repositories (`//`, `//vendor/*`, `//integration`)
coordinated by Jiri. Ensuring that code committed locally meets platform formatting, documentation,
and commit message standards is essential to preventing presubmit and CQ cycle failures.

The Git staging and hook subsystem runs pre-commit and commit-msg checks without clobbering
unstaged edits:

- **Index Partitioning**: Distinguishes fully staged files from partially staged files to prevent
  corrupting unstaged hunks during formatting.
- **Stash Isolation ($S^2 \to S$ diff apply)**: Uses the diff between the index commit ($S^2$) and
  stash worktree ($S$) to temporarily isolate staged edits without triggering false 3-way merge
  conflicts against `HEAD`.
- **Automatic Formatting**: `fx format-code` formats fully staged files with automatic re-staging
  (`git add`). Partially staged files are evaluated strictly in read-only check mode under stash
  isolation.
- **Commit Message Style Verification**: Validates subject line length ($\le 50$ chars recommended,
  $> 65$ warned), 72-character body line wrapping, and mandatory footers (`Bug:`, `Test:`,
  `Change-Id:`) via `scripts/shac/commit_msg_checker.py`.
- **Jiri Hook Delegation**: Jiri (via `//integration`) distributes a hook dispatcher
  (`hook-dispatcher.sh`) and root hook wrappers (`pre-commit`, `commit-msg`). `fx agents setup`
  drops scripts into `.git/hooks/<hook_name>.d/` without modifying the root wrappers.

---

## 2. Running Multiple Hooks & Worktree Support

### 2.1 Problem Statement

In Git worktrees (`git worktree add`), `.git` is a file pointing to the main repository, causing
relative path lookups (`.git/hooks/...`) to fail. Furthermore, developers or other tools may have
custom scripts they wish to run during `pre-commit` or `commit-msg` phases. Overwriting these
scripts leads to broken workflows.

### 2.2 Directory-Based Hooks (`<hook>.d/`)

Jiri (via `//integration`) natively distributes the universal dispatcher (`hook-dispatcher.sh`) and
root hook wrappers (`pre-commit`, `commit-msg`).

```
.git/hooks/pre-commit (Jiri Distributed Wrapper)
  │
  └──► .git/hooks/pre-commit.d/
         ├── 05-author-check.sh         (Upstream Googler email check)
         ├── 10-fuchsia-agent.sh        (Automated formatting & staging isolation)
         ├── 10-unstage-submodules.sh   (Submodule reset check)
         └── 50-team-tool.sh            (Modular team/user scripts)
```

### 2.3 How Hook Dispatch Works

The dispatcher is handled natively by Jiri executing directory-based multi-hooks (`<hook_name>.d/*`)
in lexical order. Check `//integration/git-hooks/hook-dispatcher.sh` for the dispatcher
implementation.

### 2.4 Install and Uninstall Behavior

1. **Installation (`fx agents setup`)**:
   - Resolves hooks directory via `git rev-parse --git-path hooks` (ensuring compatibility with Git
     worktrees and subdirectories).
   - `install_git_hooks` creates `.git/hooks/<hook_name>.d/` and writes or updates
     `10-fuchsia-agent.sh`.
   - It does NOT touch or overwrite `.git/hooks/<hook_name>`.
2. **Uninstallation / Reset (`fx agents setup --reset`)**:
   - Uses `uninstall_git_hooks` to delete only `.git/hooks/<hook_name>.d/10-fuchsia-agent.sh`.
   - Leaves the `.git/hooks/<hook_name>` wrappers and `<hook_name>.d/` directory intact.
   - Any user or team scripts in `<hook_name>.d/` continue executing without disruption.
   - You can inspect the status with `get_git_hooks_status`.

---

## 3. Subsystem Layers

The Git staging and hook subsystem is split into three layers:

- **Layer 1: Generic Staging Helpers (`lib/git_staging/`)**:
  - Standalone Git index partitioning, stash isolation (`StashGuard`), and pipeline execution.
  - Runs standard Git commands (`git diff`, `git apply`, `git status`) with no Fuchsia-specific
    dependencies.
- **Layer 2: Platform Hook Adapters (`lib/githooks/`)**:
  - Fuchsia-specific adapters bridging the staging engine with platform tools (`fx format-code`,
    `commit_msg_checker.py`).
  - Terminal reporters (`ConsoleReporter`) and the hook CLI dispatcher (`runner.py`).
- **Layer 3: Installer & Setup (`commands/setup.py`, `lib/githooks/installer.py`)**:
  - Discovers checkout repositories across `//` and `//vendor/*`.
  - Idempotently installs and uninstalls `10-fuchsia-agent.sh` within `.git/hooks/<hook_name>.d/`.

For the complete repository and test file layout, see
[`tools/agents/DEVELOPMENT.md#directory-layout`](../DEVELOPMENT.md#directory-layout).

### 3.1 Standalone Hook Execution

The `10-fuchsia-agent.sh` hook fragment exports `PYTHONPATH="$FUCHSIA_DIR/tools:$PYTHONPATH"` and
executes `$FUCHSIA_DIR/scripts/fuchsia-vendored-python`. This allows hooks to run during
`git commit` without depending on Ninja/GN build artifacts or an active build directory.

---

## 4. Console Output & Terminal Formatting

### 4.1 Terminal Output & Fallbacks

`ConsoleReporter` formats messages based on terminal capabilities:
- Uses UTF-8 symbols (`🧹`, `⚠️`, `❌`) on UTF-8 interactive terminals.
- Falls back to ASCII tags (`[FIXED]`, `[WARN]`, `[ERROR]`) on non-UTF-8 terminals or plain
  consoles. Clean commits run silently.

### 4.2 Example Output

#### Partial-Staging Formatting Conflict

```
❌ Formatting issues in partially staged files. Auto-fix skipped to prevent stash conflicts.
  Offending files:
    - src/sys/pkg/lib/fuchsia-pkg/src/meta.rs
  Or fix directly:
    fx format-code --files=src/sys/pkg/lib/fuchsia-pkg/src/meta.rs
```

---

## 5. Data Structures & Interfaces

### 5.1 Typed Data Models

The subsystem uses frozen dataclasses and typed callables for pipeline state:

- **[`ActionContext`](../lib/git_staging/engine.py)**: Generic execution context passed to staged
  pipelines (`repo_root`, `check_only`).
- **[`HookContext`](../lib/githooks/adapters.py)**: Specializes `ActionContext` for Fuchsia hooks
  with `is_agent` flag and `ConsoleReporter`.
- **[`PipelineResult`](../lib/git_staging/engine.py)**: Immutable result of pipeline actions,
  tracking success, `restaged_files`, `partial_conflicts`, and errors.
- **[`StagedPartition`](../lib/git_staging/engine.py)**: Immutable partitioning of staged files into
  `fully_staged` and `partially_staged` frozensets.
- **[`HookAction`](../lib/githooks/adapters.py)**: Specification of an action in the hook pipeline,
  defining name, mutate capability, formattable extensions filter, and failure remediation
  callbacks.
- **[`HookOperationResult`](../lib/githooks/installer.py) &
  [`HookStatusResult`](../lib/githooks/installer.py)**: Structured summaries of multi-repo hook
  installation and status queries.

See [`tools/agents/lib/git_staging/engine.py`](../lib/git_staging/engine.py) and
[`tools/agents/lib/githooks/adapters.py`](../lib/githooks/adapters.py) for the complete
implementations.

### 5.2 Stash Isolation (`StashGuard`)

The [`StashGuard`](../lib/git_staging/engine.py) context manager handles temporary stash isolation:

- **Isolation (`__enter__`)**: Detects unstaged working tree modifications via
  `git status --porcelain` and pushes them to a temporary stash with
  `git stash push --keep-index -q -m git-staging-isolation`.
- **Restoration (`__exit__`)**: Captures the binary stash diff between the index commit and stash
  commit ($S^2 \to S$) via `git diff --no-color --binary <stash>^2 <stash>`, reapplies it via
  `git apply --binary --allow-empty`, and safely drops the stash commit on successful restoration.
- **Unstaged Deletion Handling**: `git stash push --keep-index` omits unstaged deletions of newly
  added index files from the stash diff. `StashGuard` explicitly tracks and restores these deletions
  (`_unstaged_deleted_files`) to prevent phantom files or index corruption.
- **Exception Chaining**: All Git failures during diff or apply chain the underlying exception
  (`raise GitError(...) from exc`) to preserve tracebacks.

See [`tools/agents/lib/git_staging/engine.py`](../lib/git_staging/engine.py) for the complete
implementation.

### 5.3 Exception Handling

- `GitError(RuntimeError)`: Core exception raised on Git command failures, repository resolution
  issues, or stash isolation errors. Ensures clean error wrapping across CLI entrypoints and
  runners.

---

## 6. Bypasses & Agent Execution

### 6.1 Bypassing Hooks

Hooks can be bypassed using standard Git flags, environment variables, or `fx agents setup` (see
[`tools/agents/README.md#git-hooks-integration`](../README.md#git-hooks-integration)):

1. **Native Git Per-Commit Bypass**:
   - `git commit -n` or `git commit --no-verify` (standard Git flag, skips `pre-commit` and
     `commit-msg` natively).
2. **Environment Variable Bypass (Scripts / CI)**:
   - `FUCHSIA_SKIP_HOOKS=1 git commit ...` (immediately exits 0 from the hook runner).
3. **Configuration Options**:
   - `fx agents setup` installs `.git/hooks/<hook_name>.d/10-fuchsia-agent.sh`.
   - `fx agents setup --status`: Displays current configuration and reports Git hook configuration
     across all checkout repositories.
   - `--git-hooks` / `--no-git-hooks`: CLI flags for `fx agents setup` controlling Git hook
     installation across checkout repositories (default: `--git-hooks`). Passing `--no-git-hooks`
     configures agent permissions and profiles without installing or modifying Git hooks.
   - `fx agents setup --reset` removes `10-fuchsia-agent.sh` while leaving dispatcher and user
     scripts intact.
   - `fx agents setup --reset --no-git-hooks`: Resets Fuchsia-managed rules and configuration while
     preserving existing Git hooks intact (skips hook uninstallation).

### 6.2 Interactive Rebase

- **Automated Rebase Replay**: Git does not run `pre-commit` or `commit-msg` hooks when
  automatically applying commits during a rebase.
- **Explicit Commits & Amends**: When a developer stops during an interactive rebase
  (`git rebase -i`) to edit, amend (`git commit --amend`), or insert a commit, Git executes the
  hooks normally. The staging engine ensures newly amended or inserted commits are cleanly formatted
  and verified without false merge conflicts or unintended hook bypasses.

### 6.3 Setup Configuration vs. Runtime Agent Detection

Running `fx agents setup` configures the checkout, but hooks only apply agent-specific behavior when
an agent is actually executing:

- The presence of `~/.local/share/Fuchsia/agents/setup/state.json` indicates that `fx agents setup`
  configured the developer checkout, but does **not** mean an automated AI agent is executing a
  given commit.
- Developers routinely perform manual `git commit` commands in repositories configured with
  `fx agents setup`. These interactive human commits receive human-oriented UX (`ConsoleReporter`,
  advisory warnings, colored summaries).

#### How Agents Are Detected

Runtime AI agent execution is detected dynamically at commit time using canonical environment
variables:

- **Agent Environment Detection**: Query `is_invoked_by_agent()`, which checks the canonical agent
  environment variables defined in `AGENT_ENV_VARS` (`ANTIGRAVITY_AGENT`, `GEMINI_CLI`,
  `ANTIGRAVITY_EDITOR_APP_ROOT`).

When active runtime agent execution is detected:

- **Auto-Formatting**: Formats fully staged files in-place and re-stages them automatically.
- **Strict Validation**: Runs `commit_msg_checker.py --strict` so non-compliant messages are
  rejected before reaching CQ.

---

## 7. Failure Policies & Performance

### 7.1 Failure Policies (Fail-Open vs. Fail-Closed)

| Failure Mode                          | Policy                                           | Action                                         | Rationale                                                        |
| ------------------------------------- | ------------------------------------------------ | ---------------------------------------------- | ---------------------------------------------------------------- |
| **Missing Tool / Python Environment** | **Fail-Open**                                    | Print warning to stderr; exit 0.               | Never block commits due to environment setup issues.             |
| **Stash Restoration Failure**         | **Fail-Closed**                                  | Print recovery command; exit 1.                | Prevent silent data loss or corrupted working trees.             |
| **Code Syntax / Formatter Error**     | **Fail-Closed**                                  | Print formatter error; exit 1.                 | Catch broken code before creating commits.                       |
| **Partially Staged Formatting Error** | **Fail-Closed**                                  | Print remediation hint; exit 1.                | Avoid committing unformatted code or corrupting unstaged hunks.  |
| **Commit Message Warnings**           | **Fail-Open** (Human)<br>**Fail-Closed** (Agent) | Advisory warning for humans; error for agents. | Warn human developers without blocking commits; block commits from automated agents. |

### 7.2 Performance Optimizations

- **Fast-Path Filter**: Skip invoking `fx format-code` entirely if the staged partition contains
  zero formattable files. Supported extensions match
  [`DEFAULT_FORMATTABLE_EXTENSIONS`](../lib/githooks/adapters.py) (`.py`, `.md`, `.c`, `.cc`,
  `.cpp`, `.h`, `.hh`, `.hpp`, `.rs`, `.go`, `.fidl`, `.gn`, `.gni`, `.json`, `.json5`, `.cml`,
  `.proto`, `.ts`).
- **Binary Handling**: `git diff --binary` and `git apply --binary` preserve binary content without
  corruption, while staged path filtering targets source file extensions directly.
- **Commit Latency Target**: P50 commit overhead $< 600\text{ms}$; P90 $< 1,200\text{ms}$.

---

## 8. Troubleshooting & Manual Testing

### 8.1 Partially Staged Formatting Conflicts

When a file contains both staged and unstaged hunks, mutating formatters cannot safely rewrite the
file on disk without corrupting the unstaged hunks. If formatting issues are detected in partially
staged files, the commit is blocked with instructions on how to fix it.

Developers have three standard remediation paths:

1. **Stage remaining changes**: If the unstaged changes are ready, stage the whole file:
   ```bash
   git add <file> && git commit
   ```
2. **Discard unstaged changes**: If the unstaged changes were accidental:
   ```bash
   git checkout -- <file> && git commit
   ```
3. **Format and stage directly**:
   ```bash
   fx format-code --files=<file> && git add <file> && git commit
   ```

### 8.2 Stash Restoration Recovery

In the rare event that working tree restoration encounters an unexpected conflict or failure (e.g.
external process modification during commit), `StashGuard` aborts and leaves the temporary stash at
`stash@{0}`. The developer can inspect and recover the working tree using:

```bash
git stash list
git stash apply stash@{0}
```

### 8.3 Manual & Direct CLI Testing

Hooks can be tested directly without creating a Git commit:

```bash
# Run pre-commit hook checks on current index:
$FUCHSIA_DIR/scripts/fuchsia-vendored-python tools/agents/lib/githooks/runner.py pre-commit

# Test commit-msg hook with a sample message file:
$FUCHSIA_DIR/scripts/fuchsia-vendored-python tools/agents/lib/githooks/runner.py commit-msg path/to/COMMIT_EDITMSG

# Simulate AI agent invocation:
ANTIGRAVITY_AGENT=1 $FUCHSIA_DIR/scripts/fuchsia-vendored-python tools/agents/lib/githooks/runner.py pre-commit
```
