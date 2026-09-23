---
name: ffx-log
description: >
  Retrieve and search target device logs, with automatic system-level noise
  filtering and line limit controls to optimize token space.
---

# Fuchsia Device Diagnostics Log Skill (`ffx-log`)

This skill provides the ability to view, filter, and search diagnostic logs from
a running Fuchsia target device (including physical hardware and emulators). It
uses standard `ffx log` capabilities with token-aware limits and automatic
background connection/repository retry noise-reduction filters to let you focus
on active components and code bugs.

## Workflow

### 1. Retrieve logsa

To view device logs, use the `ffx log` command with the `--exclude-regex-file`
flag:

```posix-terminal
ffx log --exclude-regex-file src/diagnostics/skills/ffx-log/references/noise_patterns.txt dump --tail 100
```

By default, this command returns up to the **last 100 log lines** (`--tail 100`
after `dump`) and **excludes repetitive background system-level noise**
(`--exclude-regex-file src/diagnostics/skills/ffx-log/references/noise_patterns.txt`).

### 2. Available flags for advanced filtering

Customize the log query using these arguments:

- `--severity <level>`: Minimum log severity level to display. Case-insensitive.
  Accepted values: `trace`, `debug`, `info`, `warn` (or `warning`), `error`,
  `fatal`.
- `--component <moniker_or_url>`: Filter logs for specific component monikers
  or URLs. Specify multiple times for multiple components (fuzzy matching).
- `--filter <text>`: Keep only log lines where the moniker, the
  tag, or the log message contains the specified text (case-insensitive).
  Can be repeated.
- `--exclude <text>`: Exclude log lines matching the specified text string. Can
  be repeated.
- `--exclude-regex <pattern>`: Exclude log lines matching the specified regular
  expression.
- `--since <duration_or_time>`: Filter logs to only include events since a
  duration (for example, `"5m ago"`, `"1h ago"`, or `"now"`).

Under the `dump` subcommand, you can specify:

- `--tail <count>`: Return only the last `N` log lines. Omit or set to `none` to
  retrieve all matching lines without truncation.

### 3. Deep dive without noise suppression

If you want to diagnose repository connection or virtual network packet loss
issues, omit the `--exclude-regex-file` flag to show all background system
noise.

## Scenario examples

* **Scenario A: View the latest 50 logs of warning/error severity:**

    ```posix-terminal
    ffx log --exclude-regex-file src/diagnostics/skills/ffx-log/references/noise_patterns.txt --severity warn dump --tail 50
    ```

* **Scenario B: Diagnose a specific component by moniker (for example,
  `archivist`):**

    ```posix-terminal
    ffx log --exclude-regex-file src/diagnostics/skills/ffx-log/references/noise_patterns.txt --component archivist dump --tail 100
    ```

* **Scenario C: Check errors from the last 10 minutes (with no system noise):**

    ```posix-terminal
    ffx log --exclude-regex-file src/diagnostics/skills/ffx-log/references/noise_patterns.txt --severity error --since "10m ago" dump
    ```

* **Scenario D: Deep-dive including package server and network auto-client
  retries (with system noise):**

    ```posix-terminal
    ffx log --severity info dump --tail 200
    ```
