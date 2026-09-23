---
name: build-triage
description: >-
  Diagnose and triage Fuchsia build failures (Ninja and Bazel). Use this when a
  user reports a build failure (e.g. via fx report-last-build) or when
  analyzing failed runs to locate root causes.
---

# General Build Triage

This skill provides a systematic workflow for triaging general build failures
in Fuchsia, distinguishing between Ninja-only build steps and Bazel-wrapped
build steps, and handing off to the rbe-diagnostic skill if remote build
execution (RBE) or caching issues are suspected.

## Overview of Build Log Sources

Before starting triage, identify the source of the build logs, as this dictates
how you locate the logs:

### Developer Builds
Developer builds occur in local development environments and produce log
artifacts in a few different ways:
* Local build workspace logs (always present):
  * Located directly in your build output folder:
    `out/_build_logs/<config>/build.<timestamp>.<id>/`.
  * `fx report-last-build` shares these build log dirs to public X20
    locations. Most tool log subdirectories are packaged into a single
    `build_logs.tgz` archive.
* ResultStore Invocations (links start with go/fxbtx):
  * Uploaded to ResultStore and identified by an invocation ID if the
    developer has `fx resultstore enable` turned on.
  * *TODO: Outline how to fetch and parse logs from ResultStore using
    invocation IDs.*

### Infra Builds
Infra builds occur in Fuchsia's CI/CQ builders:
* Identified by a Buildbucket ID (BBID). go/bbid/<bbid> navigates to the
  build web page.
* Follow a different top-level triage process but share some underlying
  diagnostic files.
* *TODO: Document the infra-specific triage workflow and log fetching
  commands.*

## Developer Build Triage Workflow

### Locate and Unpack Build Logs

When a user reports a build failure, or you are analyzing a run:
* Locate the build's log directory inside
  `out/_build_logs/<config>/build.<timestamp>.<id>/`.
  * `<config>` is the build directory basename (e.g., found via
    `.fx-build-dir` or `fx use`).
* If the user shared a build report (e.g. from `fx report-last-build`), it
  typically contains:
  * `args.gn`: GN configuration
  * `rbe_settings.json`: RBE configuration
  * `build_logs.tgz`: tool-specific logs for ninja, bazel, reproxy, etc.
  * `ninjatrace.json.gz`: ninja build trace (Chrome Trace format), viewable in
    ui.perfetto.dev.
  * `build_profile/system_profile.json`: Chronological system-wide resource
    telemetry (Chrome Trace format), including:
    * `cpu.idle` / `cpu.wait_io`: System CPU usage and wait states.
    * `pressure.io.some.avg10` / `pressure.io.full.avg10`: I/O Pressure Stall
      Information (PSI) indicating thread stalls on storage.
    * `memory.dirty` / `memory.writeback`: Volume of unwritten dirty filesystem
      cache pages waiting to be flushed to disk.
    * `memory.free` / `memory.cache`: RAM allocation states, useful for
      identifying swapping or near-OOM (Out Of Memory) conditions.
    * `processes.running` / `processes.blocked`: Counts of active running and
      blocked processes, identifying task-starvation or process/thread
      scheduling congestion.
* Unpack `build_logs.tgz` to access all logs:
  ```bash
  tar -xzf build_logs.tgz
  ```

This will produce the following main directories (and possibly more):
* `ninja_logs/` - Ninja metrics and outputs.
* `bazel_logs/` - Bazel invocation configs, profiles, and compact execution
  logs.
* `reproxy_logs/` - Logs from the RBE reproxy daemon.

---

### Triage Ninja Build Failures

Information regarding Ninja-only build steps and generic build configurations
is located in the following logs:

* Ninja Action Counts and Metrics:
  * Location: `ninja_logs/ninja_action_metrics.json`
  * Content: Counts of successful and failed actions by mnemonic category
    (e.g. CC, CXX, BAZEL, RUST).
* Standard Compilation and Link Stderr:
  * Location: Primary build log (usually `build.log` in the root build
    directory).
  * Content: Stderr output from compilers/linkers (Clang, rustc). This contains
    compiler diagnostic errors such as syntax errors, missing symbols, and
    unresolved dependency files.

---

### Triage Bazel Build Failures

When Bazel is involved in the build failure, diagnostic files and configuration
details are stored under `bazel_logs/` (and linked via `bazel_logs/recent/`):

#### Invocation Metadata and Configurations
* Bazel Command-Line Arguments:
  * Location: `bazel_invocation`
  * Content: The exact command, built targets, and configurations (e.g.
    `--config=remote`, `--jobs=960`).
* Workspace Configurations:
  * Location: `invocation.bazelrc`
  * Content: Bazel configuration options and settings applied during that
    specific run.
  * Note: `invocation.bazelrc` is ephemeral and is not intended to be reused
    when attempting to reproduce builds.

#### Execution Performance and Bottlenecks
* Chrome-Trace Profiling Log:
  * Location: `command.profile.gz`
  * Usage: A text summary of build stages and action timing is generated by
    running:
    ```bash
    ./prebuilt/third_party/bazel/linux-x64/bazel analyze-profile command.profile.gz
    ```

* Bazel Execution Log (`exec_log.pb.zstd`):
  * Location: `exec_log.pb.zstd` (Zstd-compressed length-delimited binary
    protocol buffer format of tools.protos.ExecLogEntry).
  * Note: This is Bazel's _remote execution_ log. For decompressing, parsing,
    and analyzing detailed action execution (like arguments, runners, and
    inputs), refer to the rbe-diagnostic skill.

## Infra Build Triage Workflow

* *TODO: Document the top-level infra triage process, such as navigating
  through go/bbid/<bbid>, identifying the failing step, and pulling remote logs
  or execution logs.*

## Remote Build Diagnostics

For any issues related to remote build execution (RBE), caching, or action
digests, refer to the rbe-diagnostic skill.
