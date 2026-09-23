---
name: fx-test-remote
description: >
  Run Fuchsia tests on infrastructure builders using locally built artifacts
  "Infra At Desk". Use this skill when you need to reproduce test failures
  that only occur in infra, or when you need to test on specific hardware
  (e.g., VIM3, NUC) that is not available locally.
compatibility: Test availability of this tool by running `fx test-remote --help`.
---

# `fx test-remote`

Reference: [fx test-remote documentation](http://go/fx-test-remote-docs)

## Capabilities

This skill allows you to:

1.  **List available infra builders** to find the right environment.
2.  **Run specific tests** on those builders using your local code changes.
3.  **Validate fixes** for infra-only failures without waiting for full CQ runs.

## Prerequisites

- You must test availability of this tool by running `fx test-remote --help`. If
  it succeeds, you can use it.

## Workflow

### 1. Find a Builder

Before running tests, identify the builder that matches the target environment.
You should determine this from the user's prompt, Buganizer issue details, or
provided CQ failure logs.

If it is not explicitly clear, run `fx status` to determine the local Board and
Product configuration (e.g., `core` and `x64`) and use those to pick the closest
matching builder from the list. Do NOT guess randomly.

```bash
fx test-remote --list-builders
```

_Tip: Filter the output to find specific architecture/product combos, e.g.,
`grep arm64`._

### 2. Run Tests

Execute the tests on the remote builder.

**Basic Syntax:**

```bash
fx test-remote --builder <BUILDER_NAME> --test <TEST_NAME>
```

**Common Flags:**

- `--builder`: (Required) Name of the infra builder (e.g.,
  `bringup.arm64-debug`).
- `--test`: (Optional) Specific test target to run. Can be repeated. If omitted,
  runs _all_ tests in the builder's set (use with caution!).
- `--device`: (Optional) Filter by device type (e.g., `vim3`, `nuc11`, `emu`).
  Must be a case-insensitive substring of a device type in
  `//build/testing/platforms.gni` (`emu64` will not match, use `emu` for
  QEMU/AEMU).
- `--build-dir`: (Optional) Specify build directory (default:
  `out/test_remote`).
- `--skip-set`: (Optional) Skip `fx set` if you have already configured the
  remote build dir.

### 3. Example Scenarios

**Scenario A: Reproducing an infra failure** You have a failure in
`my-component-test` on the `core.x64-debug` builder.

```bash
fx test-remote --builder core.x64-debug --test my-component-test
```

**Scenario B: Testing driver changes on VIM3 hardware** You modified a driver
and want to test it on real hardware, but you don't have a VIM3 connected to the
local workstation.

```bash
fx test-remote --builder core.vim3-debug --device vim3 --test my-driver-test
```

## Important Notes

- **Build Directory**: By default, this tool creates/uses `out/test_remote`. It
  **will** run `fx set` and `fx build` in that directory.
- **Capacity**: These tests run on the same pool as CQ. Limit execution to
  serial runs (do not run this tool in parallel) to avoid swamping the queue.
- **Logs**: `fx test-remote` **DOES NOT** stream test output. It only outputs
  the command it ran and the Buildbucket link, then exits. You must extract the
  Buildbucket URL/build ID from standard output and retrieve the test
  logs/results using infra tools (e.g. `bb get <build_id>`, Buildbucket UI, or
  CQ dashboards).

  **Using bb**: The `bb`  (Buildbucket) tool is found in
  `prebuilt/tools/buildbucket/`. If authentication is needed, ask the user to
  run `bb auth-login` as agents cannot perform interactive authentication.

  **Using Logdog**: If you have the `logdog` CLI tool installed, you can fetch
  specific logs (like `serial_log.txt`) directly from the command line.

  *Note: `logdog` is no longer part of `depot_tools` and must be installed from CIPD:*
  ```bash
  # Create the directory if it doesn't exist
  mkdir -p ~/.local/luci/logdog

  # Define the package and version
  PACKAGE="infra/tools/luci/logdog/logdog/\${platform} latest"

  # Install using cipd ensure
  echo $PACKAGE | cipd ensure -root ~/.local/luci/logdog -ensure-file -
  ```

  1. Find the Logdog URL by running:
     `bb get -A -json <build_id> | jq | grep <log_file_name>`
  2. Stream or filter the log using:
     `logdog cat logdog://...`
