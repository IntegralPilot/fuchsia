---
name: rbe-diagnostic
description: >
  Diagnose remote build (RBE) issues, such as remote-only failures, remote vs.
  local artifact differences.
---

# RBE Diagnostic

## When to use this skill

Use this skill when a user reports a build failure related to Remote Build
Execution (RBE). This includes:
* "File not found" errors during remote compilation.
* Unexpected remote cache hits or misses.
* Suspected non-determinism (e.g., identical commands producing different
  results).
* State leaks across parallel or sequential actions in the same reproxy session.
* Local vs. remote build yield different artifacts.

## Persona

Assume the role of a specialized build engineer. You are methodical and
data-driven, relying on reproxy logs, action digests, and remote input root
inspections to pinpoint the exact failure mechanism in the RBE infrastructure or
wrapper scripts.

## Diagnostics Workflow

### Locate and Parse Remote Build Logs

Remote build logs are essential for understanding remote execution behavior,
caching, and inputs.

#### A. Reproxy Logs (Ninja-Launched Remote Actions)
Reproxy logs track remote compilation and linking steps launched by Ninja via
reclient.
* **Location**: Check
  `out/_build_logs/<config>/build.<timestamp>.<id>/reproxy_logs/`.
  * `<config>` is your build directory's basename, which can be found by
    `fx use` or looking at `.fx-build-dir`.
  * `<id>` is a random suffix.
  * Look for `reproxy.INFO`, `scandeps_server.INFO`, and `.rrpl` (reclient
    reproxy proto log) files.
* **Note**: Setting `FX_BUILD_RBE_STATS=1` prints the exact log directory in
  the build summary. It is highly recommended to set this environment variable
  while re-building and debugging.

#### B. Bazel Remote Execution Logs (Bazel-Launched Remote Actions)
When Bazel is used with RBE, it logs every action execution inside
`exec_log.pb.zstd` (located under `bazel_logs/recent/` or
`bazel_logs/invocation-<timestamp>--<uuid>/`).

* **Decoding the Compact Execution Log**:
  1. Decompress the log:
     ```bash
     zstd -d exec_log.pb.zstd -o exec_log.pb
     ```
  2. Decode to Text Proto (using `gqui` on gLinux, see `go/gqui` for
     installation instructions):
     ```bash
     gqui --printproto_readable length-delimited:exec_log.pb proto tools.protos.ExecLogEntry --outfile=exec_log.textproto
     ```
* **Structure of `exec_log.textproto`**:
  The resulting `exec_log.textproto` contains `ExecLogEntry` records:
  * **`spawn`**: Represents a single action execution.
    * `target_label`: The Bazel target label (e.g., `//products/zedboot:zedboot`).
    * `mnemonic`: The action mnemonic (e.g., `FuchsiaProductConfig`).
    * `runner`: The execution mechanism (e.g., `"remote"`, `"linux-sandbox"`). If
      built remotely via RBE, this is typically `"remote"`.
    * `args`: Command-line arguments of the action.
    * `input_set_id`: The ID of the input set used.
    * `tool_set_id`: The ID of the toolchain tool set used.
    * `digest`: The remote RBE action digest for the execution, containing
      `hash` and `size_bytes`. This can be formatted as `<hash>/<size_bytes>`
      (e.g., `32fdb693b5.../328`) and used directly as the `<ActionDigest>` in
      `remotetool.sh` queries (as documented under `Inspect Remote Input
      Roots`).
    * `platform`: Requested execution environment properties (e.g., `OSFamily`,
      `container-image`, and the requested worker `Pool` name). To find the
      *actually* resolved backend worker or pool execution metadata, run a
      `remotetool.sh` query (under `Inspect Remote Input Roots`) on the
      action's `digest`.
    * `remotable` / `cacheable` / `remote_cacheable`: Boolean flags indicating
      eligibility to run remotely or read/write to local/remote caches (governed
      by tags like `no-cache`, `no-remote-cache`, or `no-remote`).
    * `env_vars`: Key-value environment variables passed to the action.
      Extremely useful for identifying leaked absolute paths or environmental
      differences causing cache misses between environments.
    * `metrics`: Sub-stage timing breakdowns for remote execution (e.g.,
      `queue_time` spent in RBE queues, `setup_time` setting up remote workers,
      `upload_time` / `download_time` for transferring inputs/outputs).
      Essential for diagnosing slow remote builds.
  * **`input_set`**: Represents a group of input files and nested input sets.
    * Contains `input_ids` linking to individual files, and can reference nested
      input sets.
    * Note: In the `exec_log` format, input sets of files/directories are often
      expressed as depset-like graphs of groups of files, requiring graph
      workspace traversal to completely accumulate the full set of action input
      files and directories.
  * **`file` / `directory`**: Maps an `id` to a workspace path (e.g.,
    `bazel-out/...`) and a `digest` (hash and size) of the input/output
    artifact, allowing you to trace exactly what files were sent or produced by
    the action.

### Fetching Infra Build RBE Logs

Infra builds in buildbucket are identified by a number, e.g. go/bbid/NUMBER. If
the failure occurred in an infrastructure build, you can fetch the RBE logs
(specifically the `.rrpl` files) to a temporary directory using:

```bash
./build/rbe/bb_fetch_rbe_cas.sh --verbose --bbid <BBID>
```

* `<BBID>` is the Buildbucket ID of the failed infra build ('b' prefix is ok).
* **Prerequisites**: Requires the `bb` (Buildbucket) and `cas` tools (found
  under `prebuilt/tools/buildbucket/` and `prebuilt/tools/cas/`). If
  authentication is needed, ask the user to run `bb auth-login` and `cas login`
  as agents cannot perform interactive authentication.

### Analyze Action and Command Digests

The `.rrpl` logs contain remote action details.
* **CommandDigest**: Represents the command line and environment. If two actions
  have the same CommandDigest, they are considered "the same command".
* **ActionDigest**: Represents the full execution context, including the Input
  Root (all files uploaded).
* **Comparison**: If two actions have the same CommandDigest but unexpectedly
  different ActionDigests, it means their input roots differ (e.g., different
  headers, different toolchain versions, or different environment variables).
  Investigate the Input Root differences to find the cause of cache misses or
  collisions.

### Reproxy Event Timings

The `.rrpl` (reproxy proto log) contains structured event timing intervals under
`local_metadata` and `remote_metadata`. These intervals can help identify
bottlenecks.

Some useful event timing keys include:
* `ProxyExecution` (under `local_metadata`):
  * Represents the end-to-end duration reproxy spent processing the action
    request.
* `ProcessInputs` (under `local_metadata`) / `ComputeMerkleTree` (under
  `remote_metadata`):
  * Measures the time spent on local dependency processing and hashing inputs to
    build the Merkle tree.
* `CheckActionCache` (under `remote_metadata`):
  * Time spent checking RBE's Action Cache (AC) for a match.
* `DownloadResults` (under `remote_metadata`):
  * Time spent downloading output file blobs from the cloud CAS over the
    network.
* `ServerQueue` (under `remote_metadata.event_times`):
  * Measures server-side queuing duration (in ms) before a backend worker
    became available. Also summarized as `ServerQueuedMillis` in
    `rbe_metrics.txt`.

Should any of these event intervals stand out as unexpectedly slow, look for
possible explanations in `reproxy.INFO` and the system-wide profile from build
logs. The system profile can reveal when local resources (CPU, memory, I/O,
network) become limiting factors.

### Inspect Remote Input Roots

Use `remotetool.sh` to see exactly what files were sent to the remote worker.

```bash
./build/rbe/remotetool.sh --operation show_action --digest <ActionDigest>
```

* **Prerequisites**: If authentication is needed, ask the user to run `gcloud
  auth application-default login`.
* **Canonicalization**: Pay close attention to paths under `set_by_reclient/a/`
  (if canonicalization is enabled, which is the default for C++). This maps
  absolute paths to a generic working directory.
* Compare the input roots of near-identical actions to find differences. These
  differences are crucial to investigation.

### Reproduction Techniques

* **Build action repro**: `fx build -- -v -n <target>` is one way to get the
  command for a given ninja target, which may include the remote action
  wrappers.
  * For ninja actions that write long command-line arguments to response files
    (`.rsp`), also pass `-d keeprsp` to prevent ninja from deleting them. You
    can find the `.rsp` file paths in the ninja command output.
* **Reproxy management**: `./build/rbe/fuchsia-reproxy-wrap.sh --
  <your_command>` starts and stops the `reproxy` tool around any command, which
  is useful for reproducing single build actions. Note that manual invocations
  of this script will produce logs in `out/_unknown/.reproxy_logs/`.
* **Sequential Repro**: If you suspect order-dependent behavior or
  race-conditions, try to reduce the scenario to a small number of sequential
  build actions and permute their order. One way to experiment with action
  ordering is to write them to a temporary script, and run them in a single
  `reproxy` session using `./build/rbe/fuchsia-reproxy-wrap.sh -- bash
  <your_script>.sh`.
* **Fresh State**: Attempt reproduction with a clean disk cache (e.g., by
  changing the log directory or temporary RBE environment).

### Common Root Causes to Check

* **Missing Remote Inputs**: Calculation of remote inputs is different for each
  language/tool.
  * C++: Uses a lightweight `build/rbe/reclient_cxx.sh` (mostly forward directly
    to `rewrapper`, using its built-in InputProcessor), unless additional
    features are needed from `cxx_remote_wrapper.py`.
  * linking: Uses `build/rbe/cxx_link_remote_wrapper.py`.
  * Rust: Uses `build/rbe/rustc_remote_wrapper.py`.
* **Workaround**: Use `--remote-inputs` in GN to force-upload specific files as
  remote inputs if the scanner fails to detect them.

## Reporting

When diagnosing an issue, always provide:
1.  The specific **Action Digest** and **Command Digest**.
2.  The relevant snippets from `remotetool` showing the path mismatch or missing
    file.
3.  A deterministic reproduction script if possible.
