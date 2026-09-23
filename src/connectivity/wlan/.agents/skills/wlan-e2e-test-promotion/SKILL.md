---
name: wlan-e2e-test-promotion
description: >
  Workflow to identify FYI WLAN tests with 300+ consecutive passes, calculate
  their average runtime, and promote them by creating a CL.
---

# Promoting FYI WLAN Tests to Postsubmit CI

This skill defines the workflow to identify WLAN tests currently in the
`tests_for_fyi` group that are stable enough to be promoted to
`tests_for_postsubmit`.

A test is considered stable enough if it has at least 300 runs in the last 90
days (retention limit) on all of its board variants (e.g. astro, nelson,
sherlock) across all builders, and 100% of them passed.

## Prerequisites

This skill requires `depot_tools` to be installed and available in your `PATH`. Specifically, the workflow relies on:
*   `prpc`: To query Buildbucket and LUCI Analysis.
*   `rdb`: To query ResultDB.

If these tools are not present, the automation script will fail and direct you to install them. You can install `depot_tools` by following the instructions at [go/depottools#_setting_up](http://go/depottools#_setting_up).

## Workflow

### Step 1: Run the Stability Analysis Script

To identify FYI tests and check their stability across all board variants, run the provided helper script from the workspace root:

```bash
python3 src/connectivity/wlan/.agents/skills/wlan-e2e-test-promotion/scripts/promote_e2e_tests.py
```

The script will:
1. Scan the workspace to identify WLAN tests currently in `tests_for_fyi` (under `src/connectivity/wlan/tests/` and `vendor/google/tests/end_to_end/wlan/`).
2. Query LUCI Analysis to check the stability of each test across all board variants.
3. Output a **Stability Report** showing which tests are "Ready for Promotion" and which are "Not Ready" (with reasons, consecutive passes up to the present, and how long ago the latest failure occurred if any).

### Step 2: Promote the Test and Create a CL

If a test meets the promotion criteria (stable on ALL board variants), create a CL to promote it. Perform this for **only one test at a time** to keep CLs small, focused, and easy to review.

#### 1. Modify the Code
Move the test from `tests_for_fyi` to `tests_for_postsubmit` in the appropriate GN/GNI file.
*   **Example (wlanix/test_lists.gni or BUILD.gn)**:
    *   Remove `:sched_scan_test` from `tests_for_fyi` `public_deps`.
    *   Add `:sched_scan_test` to `tests_for_postsubmit` `public_deps`.
*   **Example (vendor/google/tests/end_to_end/wlan/BUILD.gn)**:
    *   Remove `:base_functional_test` from `tests_for_fyi` `public_deps`.
    *   Add `:base_functional_test` to `tests_for_postsubmit` `public_deps`.
*   After modifying the files, run `fx format-code` to ensure correct formatting (run from the corresponding repository root, e.g. `vendor/google` if modifying vendor files).

#### 2. Commit Message Guidelines
The commit message must be descriptive and include the empirical data gathered during your analysis (which can be found in the script's output):

*   **Subject Line**: Use the imperative mood, keep it under 50 characters, and prefix with the appropriate area.
    *   *Example*: `[wlan] Promote sched_scan_test to CI`
*   **Body**:
    *   Explain that the test has achieved 300 consecutive passes in FYI.
    *   Include the average runtime over the last 20 runs.
    *   Include a link to the test history view in LUCI Milo.
        *   Format: `https://luci-milo.appspot.com/ui/test/<project>/<url_encoded_test_id>`
        *   *Note*: URL-encode the `test_id` (specifically, `/` must be encoded as `%2F`).
*   **Footer**:
    *   Include a `Test:` line explaining how this change was verified (since it is a promotion based on history, state that).
        *   *Example*: `Test: None, promotion based on history.`

**Example Commit Message:**
```
[wlan] Promote sched_scan_test to CI

Promoting sched_scan_test from FYI to postsubmit CI. The test has
achieved 300 consecutive passing runs in the last 90 days across all
builders.

Test History: https://luci-milo.appspot.com/ui/test/turquoise/host_x64%2Fobj%2Fsrc%2Fconnectivity%2Fwlan%2Ftests%2Fwlanix%2Fsched_scan_test.sh-for-testing-wlan-wlanix.sorrel
Passing Runs: 300/300
Average Runtime (last 20 runs): 18.2s

Test: None, promotion based on history.
```

## Step 4: Post-Execution Reporting

Once the workflow has processed all candidate tests, print a consolidated
summary to stdout for the user. The summary must contain two lists:

1.  **Promoted Tests (Created Commits)**: A list of FYI tests that successfully
    met the 300-pass bar on ALL board variants and had a CL created.
    * For each test, print:
        * The FYI test name.
        * All matched board variants and their individual status including test runtime.
        * The local commit hash and subject line.
        * The Gerrit CL link (returned after successful push).
2.  **Tests that Did Not Meet the Bar**: A list of FYI tests that failed the
    stability check on at least one board variant (either due to failures, or
    running fewer than 300 times in 90 days).
    * For each test, print:
        * The FYI test name.
        * A list of all its board variants with their individual status (Stable,
          Unstable, or API Error), consecutive passes up to the present (including
          how long ago the latest failure occurred if any), and direct links to
          their test history in LUCI Milo. Include test runtime if available.
