# AI Coding Agent Tooling - Developer & Architecture Guide

## Overview

`tools/agents` provides the backend implementation for `fx agents`, a CLI tool that manages
AI coding assistant configurations, permission profiles, command regexes, Git index staging
isolation, and commit hooks for Fuchsia developers.

---

## Directory Layout

```text
tools/agents/
├── BUILD.gn                         # GN build definitions with python_library("agents_lib")
├── DEVELOPMENT.md                   # Developer & architecture documentation
├── OWNERS                           # Tool ownership
├── README.md                        # User-facing guide for fx agents
├── __init__.py
├── main.py                          # CLI parser and subcommand dispatcher
├── commands/
│   ├── __init__.py
│   └── setup.py                     # CLI argument parsing and setup subcommand handler
├── docs/                            # Subsystem architecture & deep-dive guides
│   ├── git_hooks.md                 # Git staging & commit hooks subsystem architecture guide
│   └── permissions.md               # Permissions, 3-way reconciliation & regex expansion guide
├── lib/
│   ├── __init__.py
│   ├── config.py                    # Atomic JSON I/O, .tmp swap, and backup creation
│   ├── git_staging/                 # Git index staging partition & stash isolation helpers
│   │   ├── __init__.py
│   │   └── engine.py                # StashGuard, partition_staged_files, run_staged_pipeline
│   ├── githooks/                    # Fuchsia platform hook adapters and dispatcher runner
│   │   ├── __init__.py
│   │   ├── adapters.py              # fx format-code, SHAC commit_msg_checker
│   │   ├── installer.py             # Multi-repo checkout discovery & hook installation
│   │   ├── reporters.py             # ConsoleReporter (TTY/ASCII)
│   │   └── runner.py                # Hook CLI dispatcher for pre-commit & commit-msg
│   ├── paths.py                     # Root discovery, repository and git path resolution
│   ├── permissions.py               # Command regex generator, profile manifests loader
│   ├── services.py                  # Multi-repo daemon discovery and systemctl restart
│   └── state.py                     # State tracking, rolling backups, and 3-way reconciliation
├── testing/
│   ├── __init__.py
│   ├── base.py                      # Hermetic base test case with tempdir & stream capture
│   └── workspace.py                 # Fast git workspace test fixture with template cache
└── tests/
    ├── __init__.py
    ├── config_test.py               # Config I/O & atomic write unit tests
    ├── e2e_githooks_test.py         # End-to-end Git hooks test harness
    ├── git_staging_test.py          # Git staging engine unit tests
    ├── githooks_adapters_test.py    # Hook adapter unit tests
    ├── githooks_installer_test.py   # Hook installer unit tests
    ├── githooks_reporters_test.py   # Console reporter unit tests
    ├── githooks_runner_test.py      # Hook CLI dispatcher unit tests
    ├── main_test.py                 # CLI dispatcher unit tests
    ├── paths_test.py                # Path resolution unit tests
    ├── permissions_test.py          # Regex expansion & profile unit tests
    ├── services_test.py             # Daemon discovery & restart unit tests
    ├── setup_test.py                # Setup command integration unit tests
    └── state_test.py                # State journal, rollback & reconciliation unit tests
```

---

## Module Responsibilities

### 1. `main.py` (CLI Dispatcher)

- Top-level `argparse` configuration for `fx agents`.
- Dynamically registers subcommands under `commands/`.
- Routes execution to the selected subcommand handler (`args.func(args)`).

### 2. `commands/setup.py` (Setup Subcommand)

- Defines arguments for `--profile`, `--status`, `--rollback`, `--reset`, `--state-dir`, `--allow`,
  `--deny`, `--ask`, `--allow-list`, `--deny-list`, `--ask-list`, `--config`, `--git-hooks`,
  `--no-git-hooks`, and `--dry-run`.
- Collects grants from profiles and flags, writes config files, and restarts daemons.

### 3. `lib/config.py` (Atomic JSON Configuration)

- **`load_config(path)`**: Safely loads JSON dictionaries, returning `{}` if missing or not a JSON
  object, and raising `json.JSONDecodeError` on malformed syntax.
- **`save_config_atomic(path, data)`**: Writes to a hidden temporary file `.{name}.tmp` in the
  target directory, formats JSON with 2-space indentation and trailing newline, and atomically swaps
  it using `Path.replace`.
- **`apply_grants(...)`**: Handles three-way reconciliation, backup creation, atomic config
  writes, and state tracking.

### 4. `lib/paths.py` (Path Resolution & Git Discovery)

- Resolves filesystem and Git repository paths (`find_fuchsia_dir`, `find_checkout_git_repos`,
  `find_config_dirs`, `find_permission_dirs`, `get_git_dir`, `get_hooks_dir`, `get_repo_root`).

### 5. `lib/permissions.py` (Command Expansion & Profiles)

- Defines profile specifications (`read-only`, `local-changes`, `external-changes`, `full-access`).
- Generates anchored regex patterns `command(regex:...)` matching environment variable prefixes,
  Git global flags, force-push flags, sed in-place flags, and prebuilt toolchain paths.
- For regex expansion details and manifest structure, see
  [`docs/permissions.md`](docs/permissions.md).

### 6. `lib/services.py` (Multi-Repo Daemon Management)

- Discovers daemon service lists across public and vendor directories (`services.txt`).
- Restarts running user daemons non-blockingly using `systemctl --user try-restart <service>`.

### 7. `lib/state.py` (State Tracking & Reconciliation)

- Tracks state in `~/.local/share/Fuchsia/agents/setup/state.json` (adhering to
  `$XDG_STATE_HOME` / `$XDG_DATA_HOME`).
- Implements timestamped rolling backups in `~/.local/share/Fuchsia/agents/setup/backups/` capped at
  `MAX_BACKUPS = 10`.
- Executes three-way grant reconciliation, multi-step rollback (`rollback`), configuration reset
  (`reset`), and status reporting (`format_status`).
- For the three-way reconciliation algorithm, see [`docs/permissions.md`](docs/permissions.md).

### 8. `lib/git_staging/` (Git Staging & Stash Isolation)

- **`engine.py`**: Helpers for partitioning staged files and isolating unstaged edits.
  - **`partition_staged_files(repo_root)`**: Identifies `fully_staged` and `partially_staged` files.
  - **`StashGuard(repo_root)`**: Context manager that applies the diff between `stash^2` and
    `stash` to safely isolate unstaged working tree edits during hook execution.
  - **`run_staged_pipeline(context, actions)`**: Executes a sequence of `HookAction` operations
    under `StashGuard` isolation, managing re-staging and conflict reporting.
  - For details on stash isolation and failure recovery, see
    [`docs/git_hooks.md`](docs/git_hooks.md).

### 9. `lib/githooks/` (Fuchsia Platform Hook Adapters & Dispatcher)

- **`adapters.py`**: Fuchsia-specific adapters bridging the staging engine with platform tools:
  - `fx format-code` adapter (mutating formatting on fully staged files, read-only verification on
    partially staged files).
  - `commit_msg_checker.py` adapter (validating subject length, body wrapping, and required
    footers).
- **`installer.py`**: Installs, uninstalls, and checks status of Git hooks across repositories
  (`install_git_hooks`, `uninstall_git_hooks`, `get_git_hooks_status`) targeting
  `.git/hooks/<hook_name>.d/10-fuchsia-agent.sh`.
- **`reporters.py`**: `ConsoleReporter` supporting colored UTF-8 symbols on interactive terminals
  and falling back to ASCII tags (`[FIXED]`, `[WARN]`, `[ERROR]`) on plain terminals.
- **`runner.py`**: Hook CLI entrypoint executing `pre-commit` and `commit-msg` pipelines, handling
  agent detection (`is_invoked_by_agent()`, `AGENT_ENV_VARS`) and bypasses (`FUCHSIA_SKIP_HOOKS`).
- For hook architecture, coexistence, and recovery, see [`docs/git_hooks.md`](docs/git_hooks.md).

---

## Subsystem Architecture Guides

For deep-dive architecture and design details, see the guides in `docs/`:

- **[Permissions & Configuration Architecture (`docs/permissions.md`)](docs/permissions.md)**:
  - Profile definitions (`read-only`, `local-changes`, `external-changes`, `full-access`).
  - Three-way reconciliation algorithm (preserving user custom rules across profile changes).
  - Regex expansion mechanics (environment variables, Git global flags, force-push detection, sed
    in-place, tool chaining).
  - Multi-repository overlay discovery across `//` and `//vendor/*`.
- **[Git Staging & Commit Hooks Architecture (`docs/git_hooks.md`)](docs/git_hooks.md)**:
  - Staged partition isolation (mutating formatting on fully staged files vs read-only checks on
    partially staged files).
  - Stash isolation (applying the diff between `stash^2` and `stash` to protect unstaged edits).
  - Modular `.git/hooks/<hook_name>.d/` coexistence with Jiri's universal dispatcher.
  - Failure policies and stash recovery.

---

## Developer Workflows

### 1. Adding or Modifying Permission Rules

- **Manifest Files**: Permission lists reside under `.agents/config/permissions/` (e.g.
  `read_only.txt`, `local_changes.txt`). Add commands as simple strings (e.g. `fx format-code`).
- **Tool Global Flags**: To support global flags for a new tool (e.g. `jj`, `cipd`), update
  `TOOL_SPECS` in `lib/permissions.py` and verify regex expansion in `tests/permissions_test.py`.
- **Profile Definitions**: Update `PROFILE_DEFINITIONS` in `lib/permissions.py` to change which
  manifest categories belong to `allow`, `deny`, or `ask`.

### 2. Adding or Modifying Hook Actions

- Hook actions are defined in `lib/githooks/adapters.py` using `HookAction`.
- Register new actions in `DEFAULT_PRE_COMMIT_ACTIONS`.
- Set `extensions` on `HookAction` (or reuse `DEFAULT_FORMATTABLE_EXTENSIONS`) to ensure actions
  only run when relevant file types are staged.
- Ensure all mutating actions support a `check_only: bool` flag for safe execution on partially
  staged files.

### 3. Writing Tests with `GitWorkspaceTestCase`

When writing tests that interact with Git:

- Inherit from `GitWorkspaceTestCase` (`from agents_testing.workspace import GitWorkspaceTestCase`).
- Use `self.commit_file("path/to/file", "content")` to create commits.
- Use `self.stage_partial("path/to/file", "staged content", "unstaged content")` to test partial
  staging scenarios.
- Verify state with `self.assertStaged("path/to/file")`, `self.assertStashEmpty()`, or
  `self.git("status", "--porcelain")`.

---

## GN Build Packaging & Testing Guidelines

### GN Build Rules (`tools/agents/BUILD.gn`)

The package is structured as a host Python library and individual host unit tests prefixed with
`agents_` to prevent target name collisions across the Fuchsia build graph:

```gn
import("//build/python/python_host_test.gni")
import("//build/python/python_library.gni")

if (is_host) {
  python_library("agents_lib") {
    library_name = "agents"
    source_root = "."
    sources = [ ... ]
  }

  python_library("agents_testing") {
    testonly = true
    library_name = "agents_testing"
    source_root = "testing"
    sources = [ "base.py", "workspace.py" ]
  }

  python_host_test("agents_setup_test") {
    main_source = "tests/setup_test.py"
    libraries = [ ":agents_lib", ":agents_testing" ]
  }
}
```

See [`tools/agents/BUILD.gn`](BUILD.gn) for the full list of library sources and test targets.

### Test Infrastructure & Common Fixtures (`agents_testing`)

Most tests in `tools/agents/tests` inherit from common hermetic test classes provided by
`agents_testing`:

- **`BaseTestCase`** (`from agents_testing.base import BaseTestCase`):
  - Automatically isolates temporary directories (`self.test_dir`).
  - Redirects `sys.stdout` and `sys.stderr` to `io.StringIO` buffers accessible via `self.stdout`
    and `self.stderr`.
  - Helpers for creating files (`self.write_file("rel/path", "content")`) and registering mock
    cleanups (`self.patch_object(...)`, `self.patch_environ(...)`).
- **`GitWorkspaceTestCase`** (`from agents_testing.workspace import GitWorkspaceTestCase`):
  - Inherits from `BaseTestCase` and clones a cached template repository (via `shutil.copytree`)
    instead of running `git init` for each test.
  - Configures safe hermetic git environments (`GIT_CONFIG_GLOBAL=/dev/null`,
    `GIT_CONFIG_NOSYSTEM=1`, `commit.gpgsign=false`, `.fx-root` marker).
  - Helpers for running git commands (`self.git(...)`), staging and committing files
    (`self.commit_file(...)`), and creating linked worktrees (`self.create_worktree(...)`).

Pure unit tests (such as `githooks_reporters_test.py`) or specialized suites (such as
`e2e_githooks_test.py`) may use standard `unittest.TestCase` or dedicated fixtures.

### Running Tests (Golden Path)

The standard, golden path workflow for configuring and running the test suite:

1. **Configure tests in build** (one-time setup if not already in `args.gn`):
   ```bash
   fx add-host-test //tools/agents:tests
   ```
2. **Build and run all tool tests**:
   ```bash
   fx test //tools/agents
   ```

> **Note**: `//tools/agents:tests` is the GN aggregator target (`group("tests")`) used when
> configuring build targets (`fx add-host-test` or in `args.gn`). For running tests with `fx test`,
> provide the directory path `//tools/agents` (or `tools/agents`), which matches all test target
> labels declared under that directory.

### Running Individual Sub-Tests

- **Run a single test target via `fx test`**:
  ```bash
  fx test agents_setup_test
  ```
- **Filter specific test cases within a suite**:
  ```bash
  # Pass test case names after '--' to forward to python unittest:
  fx test agents_setup_test -- SetupCommandTest.test_run_dry_run

  # Or filter by substring with '-k':
  fx test agents_setup_test -- -k dry_run
  ```

### Code Formatting & Linting

Before committing code changes, run the formatting and linter checks:

- **Format code**:
  ```bash
  fx format-code
  ```
- **Run shac linters** (Python formatting, GN formatting, markdown lint, doc checks):
  ```bash
  fx host-tool shac check --only python_format,gn_format,mdlint,doc_checker,commit_msg
  ```
- **Verify commit message format**:
  ```bash
  python3 scripts/shac/commit_msg_checker.py --strict --message-file <file>
  ```

---

## Adding New Subcommands

To add a new subcommand `fx agents <subcommand>`:

1. **Create Subcommand Module**: Create `tools/agents/commands/<subcommand>.py`.
2. **Implement Handlers**:
   ```python
   import argparse

   def register_subcommand(
       subparsers: argparse._SubParsersAction[argparse.ArgumentParser],
   ) -> argparse.ArgumentParser:
       parser = subparsers.add_parser("<subcommand>", help="...")
       parser.set_defaults(func=run)
       return parser

   def run(args: argparse.Namespace) -> int:
       # Execution logic
       return 0
   ```
3. **Register in `main.py`**:
   ```python
   from agents.commands import <subcommand>, setup

   def create_parser() -> argparse.ArgumentParser:
       # ...
       setup.register_subcommand(subparsers)
       <subcommand>.register_subcommand(subparsers)
       return parser
   ```
4. **Update `BUILD.gn`**: Add `"commands/<subcommand>.py"` to `_agents_sources` in
   `tools/agents/BUILD.gn`.
5. **Add Tests**: Create `tools/agents/tests/<subcommand>_test.py` and declare
   `python_host_test("agents_<subcommand>_test")` in `BUILD.gn`.
