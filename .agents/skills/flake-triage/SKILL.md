---
name: flake-triage
description: >
  Workflow for the Flake Pipeline Rotation On-call to investigate, triage, and
  link flaky test failures.
---

# Flake Triage Skill (Rotation On-call)

This skill defines the workflow and procedures for the **Flake Pipeline Rotation
On-call** to investigate, triage, and manage flaky test failures in Fuchsia.

## Rotation Workflow

When given a LUCI Milo test failure URL:

### Step 1: Investigate the Failure & Check for Existing Bugs
Run the `flake_triage.py get-failure-info` command to parse the URL, query
ResultDB for the failure details, and check LUCI Analysis for any existing
active rules or bugs.

```bash
/.agents/skills/flake-triage/scripts/flake_triage.py get-failure-info "<milo_failure_url>"
```

#### Handling Multiple Failures
If the script returns `"status": "SELECTION_REQUIRED"`, it means there are
multiple failed results in that swarming task.
1.  Present the list of failures to the user.
2.  Once the user selects a `result_id`, run the script again passing the
    `--result-id` flag:
   ```bash
   /.agents/skills/flake-triage/scripts/flake_triage.py get-failure-info "<milo_failure_url>" --result-id <result_id>
   ```

#### Analyzing the Output
* **If `"existing_bug"` is populated**: An active failure association rule
  already exists for this failure pattern!
  * Report the existing bug (e.g., `b/123456`) and rule link to the user.
  * Stop here (no need to file a duplicate).
* **If `"existing_bug"` is `null`**: Proceed to Step 2 to determine the rule
  definition. Note down the following from the output:
  * `test_id`
  * `failure_reason`
  * `variant`

### Step 2: Define LUCI Analysis Rule Definition & Title

Collaborate with the user to determine the best LUCI Analysis rule definition
and a descriptive bug title.

1.  Propose an initial rule definition and bug title based on the failure
    information.
   * **Standard Default Rule**: `test = "<test_id>"`
   * **Specific Failure Rule**: If the failure reason is highly distinctive,
     suggest combining them: `test = "<test_id>" AND reason LIKE
     "%<distinctive_string>%"`.
   * **Bug Title Guidance**: Formulate a concise title reflecting the rule
     scope. If the rule is test-specific, use `Flake: <test_id>`. If the rule
     captures a specific crash reason across tests, use a title describing the
     crash (e.g., `Flake: SSH connection timeout during test execution`).
2.  Ask the user to confirm or adjust the proposed rule definition and title.
3.  Once agreed upon, proceed to Step 3.

### Step 3: File Bug & Create LUCI Analysis Rule
If no active bug exists, use the `flake_triage.py create-rule` command to file
the bug and attach the rule.

The script will automatically:
1.  Create a Buganizer issue with a placeholder (`*LUCI rule*: TODO`).
2.  Create the LUCI Analysis rule managing that bug.
3.  Update the bug description with the live rule URL.

> [!IMPORTANT]
> **Always get explicit user confirmation** before running this mutating command, as it files a real Buganizer issue and creates a live rule.

```bash
/.agents/skills/flake-triage/scripts/flake_triage.py create-rule \
  --title "<bug_title>" \
  --test-id "<test_id>" \
  --failure-reason "<failure_reason>" \
  --url "<milo_failure_url>" \
  --variant-json '<variant_json_string>' \
  --rule-definition '<custom_rule_definition>'
```

*Note: The script automatically files bugs under Component `1467263` and adds
them to Hotlist `7810007`.*

The script will output the newly created `bug_id`, `bug_url`, `rule_id`, and
`rule_url`.
