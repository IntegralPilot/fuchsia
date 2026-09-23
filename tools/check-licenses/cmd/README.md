# `tools/check-licenses/cmd`

The `cmd` package is the primary execution entry point for the `check-licenses`
CLI tool. It handles command-line arguments, environment variable expansion,
configuration merging, logging setup, and orchestrates the high-level
execution pipeline across all other sub-packages.

## Core Architecture

### `main.go` (The Entry Point)
This file is the `package main` executable root. It is responsible for parsing
flags and initializing the environment safely.
*   **Pure Configuration:** It instantiates a local `configVars` map based on
    user-provided flags (like `--fuchsia_dir` and `--out_dir`). This map is
    passed down into the configuration parser to safely expand templated
    strings (like `{FUCHSIA_DIR}`) without relying on mutable global state.
*   **Path Normalization:** It aggressively normalizes all provided filesystem
    paths into absolute paths before passing them down the stack, ensuring
    downstream caches behave deterministically.
*   **Logging:** It initializes the logging engine based on the `--log_level`
    flag, securely routing `stdout` and `stderr` to either the terminal, a
    persistent log file in the `--out_dir`, or both.

### `config.go` (The Configuration Engine)
This file defines the master `CheckLicensesConfig` struct, which embeds all of
the sub-configs from the other packages (e.g., `*file.FileConfig`,
`*project.ProjectConfig`).
*   **Inclusion Engine:** Fuchsia spreads compliance configurations across both
    open-source and proprietary vendor repositories. `config.go` provides an
    inclusion engine (`ProcessIncludes`) that recursively scans `vendor`
    directories, safely ignores non-JSON files, and deep-merges any discovered
    `_config.json` files into the master `CheckLicensesConfig` object. It
    gracefully skips missing repositories if they are not marked `Required`.

### `driver.go` (The Orchestrator)
This contains the `Execute()` function. It acts as the master loop for the
tool, chronologically invoking the top-level functions of every sub-package in
the correct dependency order:
1.  `initialize()` (Sets up package-level configurations)
2.  `directory.NewDirectory()` (Crawls the filesystem)
3.  `project.FilterProjects()` (Prunes the GN graph to find compiled projects)
4.  `project.AnalyzeLicenses()` (Executes the Google License Classifier)
5.  `result.SaveResults()` (Generates the SPDX and NOTICE files)

## Testing Strategy
Because the `cmd` package acts primarily as a thin orchestrator mapping OS
arguments to underlying library calls, it contains minimal business logic of
its own.
*   The inclusion engine and configuration merging logic (`config.go`) are
    rigorously covered by targeted unit tests in `config_test.go`.
*   The end-to-end execution pipeline (`driver.go` and `main.go`) is validated
    by the individual unit tests defined in the downstream sub-packages.
