# AI Coding Agent Tooling (`fx agents`)

## Overview

`fx agents` is a Fuchsia developer tool that manages tool configurations, permissions, and
local state for AI coding assistants (such as Gemini CLI, Claude, and IDE plugins) operating
within the Fuchsia source tree.

Its primary command is `fx agents setup`, which handles:

- **Permission Profiles**: Presets for different workflows (e.g. read-only inspection vs.
  local editing).
- **Command Expansion**: Translates command names into regex patterns that match flags and
  environment variables.
- **Atomic Updates & Backups**: Writes configuration atomically and creates a backup before
  making changes.
- **State Tracking**: Tracks managed rules in `~/.local/share/Fuchsia/agents/setup/state.json`
  (respecting `$XDG_STATE_HOME` / `$XDG_DATA_HOME`) so custom rules you manually add aren't lost
  when switching profiles.
- **Status & Rollback**: Inspect active rules (`--status`), roll back previous changes
  (`--rollback [N]`), or reset to a clean baseline (`--reset`).
- **Daemon Restarts**: Automatically restarts background daemons across `//` and `//vendor/*`.
- **Git Hooks**: Installs and manages modular `.git/hooks/<hook_name>.d/` hooks across all
  checkout repositories (`//`, `//vendor/*`) to format staged code and check commit message
  formatting.

---

## Quickstart

### Apply Default Permission Profile

Run `fx agents setup` to configure the default `local-changes` profile:

```bash
fx agents setup
```

### Apply a Specific Profile

```bash
fx agents setup --profile read-only
```

### Dry Run Mode

Preview calculated grant modifications, Git hook actions, and daemon restarts without touching
`config.json` or installing hooks:

```bash
fx agents setup --profile full-access --dry-run
```

---

## Permission Profiles

`fx agents setup` provides four tiered permission profiles:

| Profile                         | Purpose                                                                                | Allowed (`allow`)                                                                                                              | Denied (`deny`)   | Prompt on Exec (`ask`)                                                                   |
| ------------------------------- | -------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ | ----------------- | ---------------------------------------------------------------------------------------- |
| **`read-only`**                 | Safe workspace inspection and build/test verification.                                 | `read_only.txt`                                                                                                                | `never_allow.txt` | `external_changes.txt`, `device_ops.txt`, `cache_destruction.txt`, `batch_execution.txt` |
| **`local-changes`** *(Default)* | Workspace editing, formatting, local commits, emulators, and device management.        | `read_only.txt`, `local_changes.txt`, `device_ops.txt`                                                                         | `never_allow.txt` | `batch_execution.txt`, `external_changes.txt`, `cache_destruction.txt`                   |
| **`external-changes`**          | Local changes plus upstream code reviews, pushes, and remote infra.                    | `read_only.txt`, `local_changes.txt`, `external_changes.txt`, `device_ops.txt`                                                 | `never_allow.txt` | `batch_execution.txt`, `cache_destruction.txt`                                           |
| **`full-access`**               | Unrestricted developer operations across all local, external, device, and cache tools. | `read_only.txt`, `local_changes.txt`, `external_changes.txt`, `device_ops.txt`, `cache_destruction.txt`, `batch_execution.txt` | `never_allow.txt` | *(none)*                                                                                 |

### Permission Rule Files

Rules are stored in plain text files under `.agents/config/permissions/`:

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
- **`never_allow.txt`**: Prohibited dangerous commands (`git reset --hard`, `git clean -fdx`,
  `rm -rf /`).

---

## Safety Guarantees

1. **`never_allow.txt` Enforcement**: Dangerous commands (like `rm -rf /` or `git reset --hard`)
   are denied across every profile.
2. **Force-Push Detection**: Detects force-push flags (`--force`, `-f`, `+<ref>`) anywhere in `git
   push` arguments, preventing accidental remote history overwrites.
3. **Preserves Custom Rules**: Custom rules you manually added to `config.json` outside of
   `fx agents` are preserved when switching profiles.
4. **Atomic JSON Writes**: Modifications are written to `.config.json.tmp` and renamed into place
   atomically to prevent corrupted files if interrupted.
5. **Pre-Modification Backups**: Every write creates a timestamped backup in
   `~/.local/share/Fuchsia/agents/setup/backups/` before modifying existing configuration.
6. **Safe Staged Formatting**: The `pre-commit` hook isolates staged changes before formatting so
   unstaged edits are never modified or accidentally committed.

---

## Git Hooks Integration

`fx agents setup` automatically manages Git hooks across all checkout repositories discovered in the
Fuchsia source tree (including `//` and `//vendor/*`).

Hooks are installed into `.git/hooks/<hook_name>.d/10-fuchsia-agent.sh`, integrating cleanly with
the Jiri universal hook dispatcher without overwriting custom developer or team hooks.

### What the Hooks Do

- **`pre-commit` (Safe Formatting & Staging Isolation)**:
  - **Fully Staged Files**: Automatically formatted using `fx format-code` and re-staged
    (`git add`).
  - **Partially Staged Files**: Checked in read-only mode under stash isolation. If formatting
    issues exist, the commit is blocked with instructions on how to fix them so unstaged edits
    aren't overwritten.
  - **Fast Path**: Skips invoking `fx format-code` entirely if no formattable files are staged.
- **`commit-msg` (Commit Message Standards)**:
  - Validates subject line length ($\le 50$ chars recommended, $> 65$ warned), 72-character body
    wrapping, and mandatory footers (`Bug:`, `Test:`, `Change-Id:`) via
    `scripts/shac/commit_msg_checker.py`.

### Human vs. AI Agent Commit Behavior

The hooks check whether a commit is run by a human or an AI agent (via environment variables):

| Commit Invocation                        | Commit Message Style                                                            | Formatting Behavior                              |
| ---------------------------------------- | ------------------------------------------------------------------------------- | ------------------------------------------------ |
| **Human Developer** (Interactive)        | **Advisory Warnings**: Non-blocking warnings for style/length; commits succeed. | Fully staged files auto-formatted and re-staged. |
| **AI Coding Agent** (`GEMINI_CLI`, etc.) | **Strict Rejection**: Non-compliant commit messages fail (`--strict`).          | Auto-formatting strictly enforced.               |

### Opt-Outs & Bypasses

1. **Skip Hooks During Setup**:
   ```bash
   # Configure permissions without installing Git hooks:
   fx agents setup --no-git-hooks

   # Reset permissions while leaving Git hooks installed:
   fx agents setup --reset --no-git-hooks
   ```
2. **Per-Commit Bypass**:
   ```bash
   # Standard Git flag (skips pre-commit and commit-msg natively):
   git commit -n
   # or
   git commit --no-verify
   ```
3. **Environment Variable Bypass (Scripts / CI)**:
   ```bash
   FUCHSIA_SKIP_HOOKS=1 git commit ...
   ```

---

## Status, Rollback, and Reset

### Status Inspection (`--status`)

Inspect active configuration, rule counts, Git hook status, and history:

```bash
fx agents setup --status
```

Example output:

```text
=== AI Coding Agent Configuration Status ===
Active Profile : local-changes
Last Updated   : 2026-08-26 21:00:00
Fuchsia Root   : /usr/local/google/home/username/fuchsia
Config File    : /home/username/.gemini/config/config.json [exists]
State File     : /home/username/.local/share/Fuchsia/agents/setup/state.json [exists]

Rule Breakdown:
  [ALLOW] :  58 total ( 55 managed,   3 custom)
  [DENY ] :   8 total (  8 managed,   0 custom)
  [ASK  ] :  12 total ( 12 managed,   0 custom)

Recent History (2 transactions):
  1. [2026-08-26 21:00:00] Profile: local-changes | Backup: config.20260826_210000.bak
  2. [2026-08-26 20:30:00] Profile: read-only | Backup: config.20260826_203000.bak
Git hooks: Configured across 12/12 repositories.
```

### Multi-Step Rollback (`--rollback [N]`)

Revert `config.json` to the state prior to N setup operations:

```bash
# Roll back to previous configuration (N=1)
fx agents setup --rollback

# Roll back 3 setups prior
fx agents setup --rollback 3
```

### Reset Configuration (`--reset`)

Remove all permissions configured by `fx agents setup`, reverting to a clean baseline while
preserving any custom developer-authored rules in `config.json`:

```bash
fx agents setup --reset
```

> **Note:** Unlike `--rollback` (which steps backward through previous profile changes using backup
> snapshots), `--reset` removes all Fuchsia-managed grants and clears the local state file
> (`state.json`) without modifying custom permissions or non-permission IDE settings you manually
> configured. To reset permissions while preserving Git hooks, pass `--no-git-hooks`
> (`fx agents setup --reset --no-git-hooks`).

---

## Ad-Hoc Grants and Custom Lists

You can customize any profile by passing ad-hoc commands (`--allow`, `--deny`, `--ask`) or custom
list files (`--allow-list`, `--deny-list`, `--ask-list`):

```bash
fx agents setup \
  --profile local-changes \
  --allow "fx test //custom:target" \
  --deny "git push upstream main" \
  --deny-list ~/my_denylist.txt \
  --allow-list path/to/extra_allowed.txt
```

### Rule Evaluation Precedence & Conflicts

When an AI coding assistant attempts to execute a command, rules are evaluated in strict priority
order:

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

- **`DENY` Always Wins**: If a command matches both an `allow` and a `deny` rule, the command is
  blocked immediately.
- **Overlapping Rules in Configuration**: If you specify an ad-hoc `--deny` or `--deny-list` for a
  command that is already in the selected profile's `allow` list (e.g. denying `git commit` under
  `local-changes`), both rules will appear in `config.json`. Because `DENY` has highest priority,
  the command is safely blocked at runtime.
- **Persistence Across Runs**: Ad-hoc flags only apply to that specific `setup` run. Running
  `fx agents setup` later without those flags reverts to the profile defaults. Keep your custom
  list files (e.g. `my_denylist.txt`) so you can pass them when re-running setup.

---

## Multi-Repository Overlays

In multi-repo setups (e.g. `//` with `//vendor/*`), `fx agents` automatically discovers and merges
configuration:

1. `.agents/config/permissions/` (Public Fuchsia tree)
2. `vendor/*/.agents/config/permissions/` (Internal & vendor overlays)
3. `services.txt` daemon declarations across all repository roots.

---

## Architecture & Further Reading

For in-depth documentation on the architecture and implementation of `fx agents`:

- **[Permissions & Configuration Architecture](docs/permissions.md)**: Deep dive into tiered
  profiles, partitioned manifests, regex expansion mechanics, and three-way set reconciliation.
- **[Git Staging & Commit Hooks Guide](docs/git_hooks.md)**: Deep dive into staging partition
  isolation, the `StashGuard` ($S^2 \to S$) algorithm, modular `.d/` hook dispatchers, and
  troubleshooting.
- **[Developer & Contributor Guide](DEVELOPMENT.md)**: Guide for contributing to `tools/agents`,
  including directory structure, building, testing, and adding new subcommands.
