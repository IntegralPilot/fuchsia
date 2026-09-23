---
name: wlan-e2e-test-luci-triage
description: >
  Workflow to query Fuchsia WLAN E2E test failures in infra, find patterns in recurring flakes, analyze network failure modes, check hardware correlations, and parse syslogs, test logs, and AP logs for triaging issues.
---

# Fuchsia WLAN E2E Test Triage

This skill provides a standard workflow tailored for the Fuchsia WLAN team to analyze CI/CQ E2E test failures, identify recurring network flakes, and find hardware/bot correlations for Buildbucket and ResultDB.

**Note on Usage**: This workflow is meant to be executed against a specific target. The developer invoking the agent MUST provide the specific test case, test suite, or builder they want to investigate in their prompt.

## Prerequisites

This skill requires `depot_tools` to be installed and available in your `PATH`. Specifically, the workflow relies on:
*   `bb`: To query Buildbucket builds and logs.
*   `rdb`: To query ResultDB test results and artifacts.
*   `prpc`: Optional fallback for raw RPC calls when advanced queries or unsupported fields are required.

If these tools are not present, the automation script will fail and direct you to install them. You can install `depot_tools` by following the instructions at [go/depottools#_setting_up](http://go/depottools#_setting_up).

## Workflow

### Step 1: Query Buildbucket for Relevant WLAN Builds
To get a list of builds for a specific builder, use the `bb ls` command with the builder target formatted as `turquoise/<bucket>/<builder>`:
*   `turquoise`: The project name.
*   `<bucket>`: The LUCI bucket (typically `global.ci` for internal FYI builders, or `smart.ci` for smart-display builders).
*   `<builder>`: The builder name (e.g. `fuchsia_internal.arm64-release-fyi`).

Always include `-nopage` when running `bb ls` in terminal/agent environments to prevent the CLI from invoking an interactive pager and hanging.

*   **List recent builds**:
    ```bash
    bb ls turquoise/global.ci/fuchsia_internal.arm64-release-fyi -n 50 -nopage
    ```
*   **List build IDs only** (convenient for piping into ResultDB queries):
    ```bash
    bb ls turquoise/global.ci/fuchsia_internal.arm64-release-fyi -n 50 -id -nopage
    ```
*   **Filter by status** (e.g. only failed builds):
    ```bash
    bb ls turquoise/global.ci/fuchsia_internal.arm64-release-fyi -status failure -n 50 -id -nopage
    ```
*   **JSON output** (to inspect timestamps or summary details):
    ```bash
    bb ls turquoise/global.ci/fuchsia_internal.arm64-release-fyi -n 50 -json -nopage
    ```

*(Note: If complex filtering predicates are required, `bb ls` also supports the `-predicate '<JSON>'` flag (preferred), or you can fall back to `prpc call cr-buildbucket.appspot.com buildbucket.v2.Builds.SearchBuilds`.)*

### Step 2: Query ResultDB for WLAN Test Results & Bot IDs
Find specific WLAN tests that failed within those builds using `rdb query`. ResultDB handles pagination automatically and provides a simpler interface than raw PRPC.

**Scope & Filtering Logic**:
*   **Builder**: If the user provides a builder, restrict your search to that builder.
*   **Test Suite**: If the user provides a test suite name, you may be looking across multiple builders (unless a builder is also specified).
*   **Regex**: If the user provides a regex, apply that alongside whatever builder and test suite name was supplied (via the `-test` flag).
*   **Fallback**: Otherwise, assume we want all test variants in the supplied builder.

**Query Modes**:
*   **Full Volume (Total Runs)**: Omit the `-u` flag to return all executions (both passed and failed). This is required in Step 3 to compute accurate pass/fail rates per bot.
*   **Failures Only (`-u`)**: Pass the `-u` flag to filter only unexpected failures (`status: "FAIL"`) when looking for root causes.

**Examples**:
*   **Query all test runs in a build (for bot pass/fail rate calculation)**:
    ```bash
    rdb query -json -tr-fields testId,status,name,failureReason,summaryHtml,tags "build-<BUILD_ID>"
    ```
*   **Filter by test regex (unexpected failures only)**:
    ```bash
    rdb query -json -u -tr-fields testId,status,name,failureReason,summaryHtml,tags -test ".*<TEST_SUITE_OR_REGEX>.*" "build-<BUILD_ID>"
    ```
*   **Extract all failed sub-test cases and failure reasons directly**:
    All sub-test results and error messages are already structured in ResultDB. You do not need to download or parse `test_summary.yaml` to discover which test cases failed or why:
    ```bash
    rdb query -json -u -tr-fields testId,status,failureReason "build-<BUILD_ID>" \
      | jq -r '.testResult | select(.failureReason != null) | "\(.testId) => \(.failureReason.primaryErrorMessage)"'
    ```
*   **Pipe failed build IDs directly from `bb ls`**:
    ```bash
    bb ls turquoise/global.ci/fuchsia_internal.arm64-release-fyi -status failure -n 10 -id -nopage | sed 's/^/build-/' | rdb query -json -u -tr-fields testId,status,name,failureReason,summaryHtml,tags
    ```

### Step 3: Analyze Hardware Correlations
WLAN E2E flakes can sometimes be caused by hardware issues.
1. Iterate through the `tags` array on each `FAIL` test result.
2. Find the tag object where `key: "swarming_bot_id"`.
3. Tally the failures by `swarming_bot_id`.
4. **Validate against total runs (CRITICAL)**:
   Never rely solely on raw failure counts; a high number of failures on a bot may simply mean more runs were scheduled on it. Always validate:
   *   **Failure Rate**: Calculate `(bot_failures / bot_total_runs) * 100` using runs fetched without `-u`.
   *   **Fleet Baseline Comparison**: Compare the suspicious bot's failure rate against the fleet average (e.g. 70% vs. 1% baseline).
   *   **Symptom Concentration**: Check if a specific symptom (e.g. `Network not found` or AP de-authentications) is localized to 1 or 2 specific bots. If so, it is almost certainly a hardware or AP broadcast issue on that testbed.

### Step 4: Fetch & Parse Artifacts
To root cause WLAN test failures, understand where artifacts are located in ResultDB's hierarchy:

| Level | Resource Name Format | What It Contains | How to Query |
| :--- | :--- | :--- | :--- |
| **Suite-Level Test Result** | `invocations/<SWARMING_INV>/tests/<SUITE>/results/<ID>` | `Snapshot_*.zip`, `test_log.INFO`, AP logs, `ffx.log` | `luci test-result artifact list` |
| **Sub-Test Case Level** | `.../tests/<SUITE>/<Class>:<test_case>/results/<ID>` | Pass/fail status & error messages only | *(No file artifacts attached)* |
| **Swarming Invocation** | `invocations/<SWARMING_INV>` | `syslog.txt`, `serial_log.txt`, infra logs | `bb log` |

#### 1. Discover Artifacts with `luci`

*   **List all artifacts for the test suite (`luci test-result artifact list`)**:
    Use the IDs of the **top-level test suite** result from Step 2:
    ```bash
    /google/bin/releases/luci-cli/luci test-result artifact list -legacy -invocationid <SWARMING_INV> -testid <TEST_ID> -resultid <RESULT_ID>
    ```
    *(Tip: Run `/google/bin/releases/luci-cli/luci ids "<suite_test_result_name>"` to extract `-invocationid`, `-testid`, and `-resultid` automatically from the test result name).*

*   **Search artifacts across the build (`QueryArtifacts`)**:
    When searching across all runs in a build without knowing the individual test result names, you can query ResultDB with a `predicate` to find specific artifacts:
    ```bash
    echo '{"invocations": ["invocations/build-<BUILD_ID>"], "pageSize": 100, "predicate": {"testResultPredicate": {"testIdRegexp": ".*<TEST_SUITE_OR_NAME>.*"}, "artifactIdRegexp": ".*(Snapshot|test_log).*"}}' | rdb rpc luci.resultdb.v1.ResultDB QueryArtifacts
    ```
    *For root invocation-level logs (e.g., `serial_log.txt` or `syslog.txt` across shards)*:
    ```bash
    echo '{"invocations": ["invocations/build-<BUILD_ID>"], "pageSize": 100, "predicate": {"followEdges": {"includedInvocations": true}, "artifactIdRegexp": ".*(serial_log|syslog).*"}}' | rdb rpc luci.resultdb.v1.ResultDB QueryArtifacts
    ```

#### 2. Fetch and Read Artifact Logs

Use `/google/bin/releases/luci-cli/luci test-result artifact get` for all test artifacts (prefer this over `curl`, which is not usually in the agent allowlist):

*   **Stream text logs to stdout (e.g. `test_log.INFO`, `test_summary.yaml`, `stdout-and-stderr.txt`)**:
    Print or pipe artifact content directly without downloading files or dealing with base64 decoding:
    ```bash
    /google/bin/releases/luci-cli/luci test-result artifact get -legacy -invocationid <SWARMING_INV> -testid <TEST_ID> -resultid <RESULT_ID> -artifactid "<ARTIFACT_ID>"
    ```
    *(Can be piped directly to `grep`, `head`, `tail`, etc.)*

*   **Download binary files (e.g. `Snapshot_*.zip`)**:
    Save to a local file using `-o`:
    ```bash
    /google/bin/releases/luci-cli/luci test-result artifact get -legacy -invocationid <SWARMING_INV> -testid <TEST_ID> -resultid <RESULT_ID> -artifactid "<SNAPSHOT_ARTIFACT_ID>" -o /tmp/snapshot.zip

    # Extract the device syslog from the snapshot:
    unzip -p /tmp/snapshot.zip log.system.txt > /tmp/log.system.txt
    ```

*   **View build step and infra logs via `bb log`**:
    For harness-level crashes or build infrastructure logs:
    ```bash
    bb log <BUILD_ID> "<STEP_NAME>" "<LOG_NAME>"
    ```
    *(Tip: Run `bb get <BUILD_ID> -steps` to discover the exact step name for a failed test, e.g. `failures|<SHARD>|attempt 0 (fail)|failed: <TEST_ID>`, which contains logs like `stdout-and-stderr.txt`, `infra_and_test_std_and_klog.txt`, and `syslog.txt`).*

#### 3. Key Artifacts Reference
Most issues can be triaged using a combination of the test logs and Fuchsia device logs (syslog/snapshots), but AP logs and metadata are sometimes helpful to confirm root causes.

*   **Fuchsia Device Logs**:
    *   `InfraTestbed/.../Snapshot_*.zip`: The primary Fuchsia snapshot. It contains the `syslog`, `inspect` data, traces, and other Fuchsia-specific debugging information. **Always prefer the syslog inside this snapshot over the parent invocation syslogs. When reading the syslog, search for log lines tagged with `lacewing` that contain your test case name to find the exact start and end of the test.**
*   **Test Framework Logs**:
    *   `InfraTestbed/.../test_log.DEBUG` and `test_log.INFO`: Shows the test execution from the test framework's perspective.
    *   `stdout-and-stderr.txt` or `infra_and_test_std_and_klog.txt`: Combined test framework and infra output. Useful for crashes in the test harness itself.
*   **AP Logs (for confirming network rejections/issues)**:
    *   `InfraTestbed/.../ap_dhcp_*.log`: DHCP logs from the Access Point.
    *   `InfraTestbed/.../ap_hostapd_*.log`: hostapd logs from the Access Point. Useful for verifying if the AP actually sent frames.
    *   `InfraTestbed/.../ap_systemd_*.log`: Systemd logs from the AP.
*   **Ancillary Logs**:
    *   `InfraTestbed/.../wifi_log.txt`: Wi-Fi logs retrieved via ADB (only present if the test uses ADB).
    *   `InfraTestbed/.../ffx/ffx.log`: Logs from the `ffx` tool, which are helpful if the test failed due to a host-device communication issue.
    *   `InfraTestbed/.../test_summary.yaml`: Test framework execution summary (failures are directly readable at the bottom of the file with `tail` or `grep`, no parsing needed).
    *   `InfraTestbed/.../triage_output`: Test metadata.
*   **Parent Invocation Logs**:
    *   You may see `syslog.txt` or `serial_log.txt` at the root of the parent invocation. These logs contain output from your test along with other tests, so they can be noisy. Rely on the snapshot syslogs unless debugging a hardware lockup.


### Step 5: Triage Strategy
When debugging a failure, follow this specific order of operations:

1. **Check Test Logs First:**
   Before diving into device syslogs, ALWAYS start with `test_log.INFO` or `stdout-and-stderr.txt`. Look for the exact assertion that failed and what test case was running.
2. **Consult Test Source Code:**
   Use the local Fuchsia source code or `code_search` to find the E2E test code itself. Understand what the test was trying to do when the assertion failed (e.g. was it waiting for a connection? expecting a disconnect?).
3. **Find the Test Boundaries in Syslog:**
   Open `log.system.txt` from the device snapshot. Search for the exact test case name (e.g., `test_wlan_connection_with_suspend_resume`). The test framework logs the start and end of each test case. **Specifically, look for log lines tagged with `lacewing` that contain the test case name to ensure you are looking at the exact boundaries of the test run.** You MUST restrict your analysis to the logs between these markers to avoid cross-pollution.
4. **Filter by WLAN Tags:**
   Within those boundaries, look for evidence of what occurred on the device. WLAN logs are typically tagged with:
   *   `wlan` or `wlanif` (Generic core WLAN stack)
   *   `wlancfg` or `wlanix` (WLAN policy layer)
   *   `wlansoftmac` (SoftMAC layer)
   *   `brcmfmac` or `iwlwifi` or `synadhd` (Specific driver logs)
   *   `wlan-hw-sim` (Simulated environment)
5. **Consult Fuchsia Source Code:**
   When you find suspicious logs (e.g. `AP Rejection status: 2` or `wlanif: interface destroyed`), use local Fuchsia code or `code_search` to find the relevant code in the Fuchsia platform to understand exactly what triggers that log.

## Helper Scripts
A Python script (`scripts/luci_triage.py`) automates extracting failure distributions by `swarming_bot_id`. It fetches both total runs and failures to accurately compute the failure rate.
Run it as follows (use `--test-pattern ".*"` if you want all variants; default bucket is `global.ci`, or specify `--bucket smart.ci` for smart-display builders):
```bash
python3 src/connectivity/wlan/.agents/skills/wlan-e2e-test-luci-triage/scripts/luci_triage.py --builder fuchsia_internal.arm64-release-fyi --bucket global.ci --cutoff-date 2026-06-01 --test-pattern ".*"
```

## Example Case Study: Triaging `e2e_connection_test_using_adb`
When the user asks you to triage a test suite like `e2e_connection_test_using_adb`, follow these examples:

**Scenario 1: Hardware Flake Analysis ("Network not found")**
*   **Observation**: The test failed with "Network not found".
*   **Action**: Query ResultDB for all recent failures of this test and parse the `swarming_bot_id` from the `tags`.
*   **Result**: We discovered that out of 40 recent failures, 39 occurred on a single testbed (`fuchsia-meadowpoint-8-4-02`).
*   **Conclusion**: We proved this was a hardware/AP broadcast issue on that specific bot, allowing the lab team to fix the testbed without us chasing ghosts in the driver code.

**Scenario 2: Root Causing AP Rejections on Suspend/Resume**
*   **Observation**: The `with_suspend_resume` variants of the test were occasionally failing because the connection dropped.
*   **Action**: We checked `test_log.INFO` and saw the test failing at an assertion waiting for the connection. We downloaded the `Snapshot_*.zip` using `luci test-result artifact get`, extracted `log.system.txt`, and isolated the logs to the exact test boundaries.
*   **Result**: By filtering for `wlan` tags, we found an `AP Rejection status: 2` followed immediately by `wlanif: interface destroyed`, occurring the exact moment the device logged a `resume` from sleep.
*   **Conclusion**: We proved the test wasn't flaky; the AP was intentionally de-authenticating the device while it slept, requiring the test code to be refactored to expect this rejection and wait longer for a reconnection.
