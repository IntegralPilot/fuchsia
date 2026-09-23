# AI Coding Agent Tooling: Permissions & Configuration Architecture

## 1. Overview & Architecture

`tools/agents` manages configurations, permission profiles, command expansion regexes, and
tool permissions for AI coding agents operating in the Fuchsia source tree.

To let agents run commands productively without clobbering your workspace, the subsystem
implements:

- **Permission Profiles**: Declarative profiles grouped by workflow needs.
- **Permission Manifest Files**: Text files defining allowed, denied, and prompt-on-exec commands.
- **Three-Way Set Reconciliation**: Preserves custom rules in `config.json` when switching profiles.
- **Command Variant & Regex Expansion**: Expands commands into regexes that handle environment
  variables, global flags, and tool wrappers.
- **Multi-Repo Overlay Discovery**: Discovers and merges permission files and daemon configs across
  `//` and `//vendor/*`.
- **State Tracking & Backups**: Tracks managed rules in
  `~/.local/share/Fuchsia/agents/setup/state.json` with multi-step rollback and atomic writes.

---

## 2. Permission Profiles & Manifest Files

### 2.1 Tiered Profiles

`fx agents setup` provides four tiered permission profiles:

| Profile                         | Purpose                                                                                | Allowed (`allow`)                                                                                                              | Denied (`deny`)   | Prompt on Exec (`ask`)                                                                   |
| ------------------------------- | -------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ | ----------------- | ---------------------------------------------------------------------------------------- |
| **`read-only`**                 | Safe workspace inspection and build/test verification.                                 | `read_only.txt`                                                                                                                | `never_allow.txt` | `external_changes.txt`, `device_ops.txt`, `cache_destruction.txt`, `batch_execution.txt` |
| **`local-changes`** *(Default)* | Workspace editing, formatting, local commits, emulators, and device management.        | `read_only.txt`, `local_changes.txt`, `device_ops.txt`                                                                         | `never_allow.txt` | `batch_execution.txt`, `external_changes.txt`, `cache_destruction.txt`                   |
| **`external-changes`**          | Local changes plus upstream code reviews, pushes, and remote infra.                    | `read_only.txt`, `local_changes.txt`, `external_changes.txt`, `device_ops.txt`                                                 | `never_allow.txt` | `batch_execution.txt`, `cache_destruction.txt`                                           |
| **`full-access`**               | Unrestricted developer operations across all local, external, device, and cache tools. | `read_only.txt`, `local_changes.txt`, `external_changes.txt`, `device_ops.txt`, `cache_destruction.txt`, `batch_execution.txt` | `never_allow.txt` | *(none)*                                                                                 |

### 2.2 Permission Manifest Files

Rules are defined in declarative text files under `.agents/config/permissions/`:

- **`read_only.txt`**: Safe inspection (`git status`, `git diff`, `fx build`, `fx test`, `cat`,
  `grep`, `rg`, `fd`).
- **`local_changes.txt`**: In-tree editing and local VCS (`git add`, `git commit`, `git checkout`,
  `git branch`, `git stash`, `git rebase`, `fx format-code`).
- **`external_changes.txt`**: Upstream publishing and review (`git push`, `jiri upload`,
  `fx gh pr ...`).
- **`device_ops.txt`**: Target device controls (`ffx target ...`, `fx ota`, `fx reboot`).
- **`cache_destruction.txt`**: Build directory cleans and cache wipes (`fx clean`,
  `fx clean-build`).
- **`batch_execution.txt`**: Batch traversal and pipeline tools (`find`, `xargs`).
- **`never_allow.txt`**: Destructive actions that should never run automatically
  (`git reset --hard`, `git clean -fdx`, `rm -rf /`).

### 2.3 Multi-Repository Overlay Discovery

Fuchsia uses a multi-repository structure managed by `jiri`. `fx agents` discovers overlays across
public and vendor trees via [`lib/paths.py`](../lib/paths.py):

- **Permission Manifests**: `find_permission_dirs` aggregates all `permissions/` directories across
  `//.agents/config/` and `//vendor/*/.agents/config/`. Manifests with matching filenames across
  public and vendor overlays are concatenated.
- **Daemon Services**: `services.txt` files across all overlays are parsed and aggregated in order.

---

## 3. Three-Way Set Reconciliation

To prevent configuration drift, lingering rule conflicts, or loss of developer-authored custom
rules, [`lib/state.py`](../lib/state.py) implements a 3-way set reconciliation model.

Let $\text{cat} \in \{\text{allow}, \text{deny}, \text{ask}\}$ represent the grant categories.

### 3.1 Finding Custom Rules

Let $E[\text{cat}]$ be the list of existing grants in `config.json`, and
$M_{\text{prev}}[\text{cat}]$ be the grants recorded as managed by the previous setup run in
`state.json`. The developer's custom rules $U[\text{cat}]$ are computed as:
$$U[\text{cat}] = E[\text{cat}] \setminus M_{\text{prev}}[\text{cat}]$$

### 3.2 Resolving Category Conflicts

When transitioning across profiles (e.g., from `local-changes` where `external_changes.txt` is
prompted, to `external-changes` where it is allowed), rules moving into $M_{\text{target}}[\text{cat}]$ must not be
blocked by lingering entries in opposing categories:
$$\forall \text{cat} \in \{\text{allow}, \text{deny}, \text{ask}\}, \forall r \in M_{\text{target}}[\text{cat}], \forall \text{other} \neq \text{cat}: \quad U[\text{other}] \leftarrow U[\text{other}] \setminus \{r\}$$

### 3.3 Removing Old Managed Rules

Rules previously managed that are no longer part of $M_{\text{target}}[\text{cat}]$ (i.e.,
$r \in M_{\text{prev}}[\text{cat}] \setminus M_{\text{target}}[\text{cat}]$) are automatically
retired and excluded from the final grants.

### 3.4 Combining Final Rules

The final grant list $\text{final}[\text{cat}]$ combines user custom rules and target managed rules,
preserving deterministic ordering:
$$\text{final}[\text{cat}] = U[\text{cat}] \cup M_{\text{target}}[\text{cat}]$$

---

## 4. Command Variant Expansion & Regex Generation

Grant entries in `config.json` need to match how agents and tools invoke commands in the shell.
[`lib/permissions.py`](../lib/permissions.py) expands human-readable command lines into regexes that
match real-world invocations:

### 4.1 Environment Variable Prefixes

Commands frequently execute with leading environment variables (e.g. `GIT_PAGER=cat git status`).
Regexes prepend `ENV_VARS_PREFIX_PATTERN` to match any number of key-value environment assignments
before the command binary.

### 4.2 Git Global Flags & Force-Push Detection

Git commands allow global flags before the subcommand (e.g. `git -C //src status`) and flags
anywhere in the argument list (e.g. `git push origin main --force` vs `git push -f origin HEAD`):

- **Global Flags**: Supports `-C <dir>`, `--git-dir <dir>`, `--work-tree <dir>`, `--namespace <name>`,
  `--bare`, `--no-pager`, `--no-color`, `--literal-pathspecs`, `--no-optional-locks`, `-p`, `--paginate`,
  and `-c <config>` (supporting both space and `=` delimiters).
- **Force-Push Detection**: Scans arguments in `git push` commands to match force flags
  (`--force`, `--force-with-lease[=<ref>]`, `-f`) regardless of argument positioning, preventing agents
  from overwriting remote history unless explicitly granted.

### 4.3 Fuchsia Tool Global Flags & Chaining (`fx`, `ffx`, `jiri`)

Fuchsia wrapper tools emit direct regexes matching any binary path (`fx`, `scripts/fx`,
`/abs/path/fx`) and supported global flags:

- **`fx`**: Handles `-t <target>`, `--dir <out_dir>`, `--enable=...`, `--disable=...`, `-x`, `-xx`,
  `-i`, `--`.
- **`ffx`**: Handles standalone `ffx` as well as chained `fx [flags] ffx [flags]`, including
  `--machine json`, `-t <target>`, `-c <config>`, `-v`, and `--isolate-dir`.
- **`jiri`**: Handles `-j <N>`, `-root <dir>`, `-color <mode>`, `-time`, `-v`, `-vv`, and
  `--show-progress`.

### 4.4 Sed In-Place Expansion

Sed in-place invocations (`-i`, `-i.bak`, `--in-place`) can execute with combined flags (e.g.,
`sed -Ei '...'`). The generator creates specialized regexes matching any `-i` flag variant.

### 4.5 Unified System Binary Regex Expansion

For general binaries (e.g. `grep`, `jq`, `cat`, `ls`):

- Emits anchored regexes prepending `ENV_VARS_PREFIX_PATTERN` and optional binary path prefix
  `(\S+/)?`.
- Automatically permits standard system paths (`/bin/`, `/usr/bin/`), in-tree prebuilt toolchains
  (`prebuilt/...`), and bare commands via `$PATH`.
- Supports leading environment variables (e.g. `LC_ALL=C grep`) and subcommands without generating
  redundant duplicate rules.

---

## 5. State Tracking, Backups & Rollback

### 5.1 State File Schema

`fx agents setup` tracks its state in `~/.local/share/Fuchsia/agents/setup/state.json` (respecting
`$XDG_STATE_HOME` and `$XDG_DATA_HOME`). The state file records:

- Active profile name.
- Timestamp of last modification.
- Managed rule sets for each category (`allow`, `deny`, `ask`).
- History of applied profiles and corresponding backup snapshots.

### 5.2 Atomic Writes & Backups

- **Pre-Modification Backups**: Every write creates a timestamped backup in
  `~/.local/share/Fuchsia/agents/setup/backups/`, rolling and capped at `MAX_BACKUPS = 10`.
- **Atomic File Swaps**: Configuration updates are written to `.{name}.tmp` and atomically swapped
  via `Path.replace` to prevent corrupted JSON on interruption.
- **Rollback (`--rollback [N]`)**: Reverts `config.json` to the state prior to N setup operations
  using the rolling backup snapshots.
- **Reset (`--reset`)**: Strips all Fuchsia-managed rules while strictly preserving any
  developer-authored custom rules in `config.json`.

---

## 6. Rule Evaluation Precedence & Ad-Hoc Grants

### 6.1 Runtime Evaluation Precedence

When an AI coding agent executes a shell command, the underlying agent runtime evaluates the
configured rules in priority order:

```text
                  Incoming Agent Command
                            │
                            ▼
                 ┌─────────────────────┐
                 │  Matches DENY rule? │ ───► YES ───► [BLOCKED]
                 └──────────┬──────────┘
                            │ NO
                            ▼
                 ┌─────────────────────┐
                 │  Matches ASK rule?  │ ───► YES ───► [PROMPT USER]
                 └──────────┬──────────┘
                            │ NO
                            ▼
                 ┌─────────────────────┐
                 │ Matches ALLOW rule? │ ───► YES ───► [AUTO-RUN]
                 └──────────┬──────────┘
                            │ NO
                            ▼
                    [DEFAULT PROMPT]
```

- **`DENY` takes precedence**: If a command matches any regex in `deny`, execution is
  rejected immediately, regardless of any matching rules in `allow` or `ask`.
- **`ASK` takes precedence over `ALLOW`**: If a command matches an `ask` rule and an `allow` rule
  (but not `deny`), the user is prompted for interactive confirmation before execution.

### 6.2 Ad-Hoc Grants & Conflict Behavior

Developers can pass ad-hoc flags (`--allow`, `--deny`, `--ask`) or list files (`--allow-list`,
`--deny-list`, `--ask-list`) to customize any profile.

- **Combining Rules**: Ad-hoc rules are expanded to regexes and appended to the target managed grant
  lists (`target_managed[cat]`) alongside the selected profile's rules.
- **Overlapping Rules in Configuration**:
  - `reconcile_grants` resolves conflicts between `target_managed` and `user_custom` rules (rules
    added to `config.json` manually outside of `fx agents`).
  - It does not prune contradictory rules within `target_managed` itself. If a user specifies
    `--deny "git commit"` while applying `local-changes` (which allows `git commit`), both the
    `allow` and `deny` rules are written to `config.json`.
  - Because `DENY` has highest priority in runtime evaluation, the command is blocked as intended.
- **Lifecycle & Re-runs**:
  - Ad-hoc flags apply to that specific `fx agents setup` run.
  - Running `fx agents setup` later without those flags re-applies the default profile manifests.
    If you want to maintain recurring custom restrictions, keep a custom list file (e.g.
    `my_denylist.txt`) and pass `--deny-list <path>` when running setup.
