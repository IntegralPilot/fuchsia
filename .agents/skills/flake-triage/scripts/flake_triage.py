#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import re
import subprocess
import sys
import typing
from urllib.parse import unquote, urlparse

DEFAULT_COMPONENT_ID = "1467263"  # Fuchsia > Platform > Test Flakes
DEFAULT_HOTLIST_ID = "7810007"  # systemic-flake-failure-modes


# --- URL & ResultDB Query Helpers ---


def parse_luci_url(url: str) -> dict[str, str]:
    """Parses a LUCI Milo URL to extract the invocation ID and test ID."""
    parsed_url = urlparse(url)
    path = parsed_url.path

    # Extract invocation ID
    inv_match = re.search(r"/invocations/([^/]+)", path)
    if not inv_match:
        raise ValueError("Could not find invocation ID in URL path")
    invocation_id = inv_match.group(1)

    # Extract test ID
    test_match = re.search(r"/cases/(.+)$", path)
    if not test_match:
        raise ValueError("Could not find test ID in URL path")

    test_id = unquote(test_match.group(1))

    return {"invocation_id": invocation_id, "test_id": test_id}


def query_test_results(
    invocation_id: str, test_id: str, project: str = "turquoise"
) -> list[dict[str, typing.Any]]:
    """Queries ResultDB for unexpected test results matching the invocation and test ID."""
    # Escape test ID for regex
    escaped_test_id = test_id.replace(".", r"\.")

    payload = {
        "invocations": [f"invocations/{invocation_id}"],
        "predicate": {
            "testIdRegexp": escaped_test_id,
            "expectancy": "VARIANTS_WITH_UNEXPECTED_RESULTS",
        },
        "readMask": "name,test_id,status_v2,failure_reason,test_metadata,variant,result_id",
    }

    cmd = [
        "prpc",
        "call",
        "results.api.luci.app",
        "luci.resultdb.v1.ResultDB.QueryTestResults",
    ]

    try:
        result = subprocess.run(
            cmd,
            input=json.dumps(payload),
            capture_output=True,
            text=True,
            check=True,
        )
        response = json.loads(result.stdout)

    except subprocess.CalledProcessError as e:
        raise RuntimeError(f"prpc QueryTestResults failed: {e.stderr.strip()}")
    except json.JSONDecodeError as e:
        raise RuntimeError(f"Failed to parse QueryTestResults JSON output: {e}")

    results = response.get("testResults", [])
    failures = []
    for r in results:
        status_v2 = r.get("statusV2")
        if status_v2 and status_v2 != "PASSED":
            failures.append(
                {
                    "result_id": r.get("resultId"),
                    "test_id": r.get("testId"),
                    "status_v2": status_v2,
                    "failure_reason": r.get("failureReason", {}).get(
                        "primaryErrorMessage", "No primary error message found"
                    ),
                    "variant": r.get("variant", {}).get("def", {}),
                    "bug_component": r.get("testMetadata", {})
                    .get("bugComponent", {})
                    .get("issueTracker", {})
                    .get("componentId"),
                }
            )

    return failures


def check_existing_clusters(
    project: str, test_id: str, failure_reason: str
) -> dict[str, typing.Any]:
    """Checks LUCI Analysis to determine if the failure maps to an active rule or bug."""
    # Call LUCI Analysis Cluster RPC
    payload = {
        "project": project,
        "testResults": [
            {
                "requestTag": "flake-triage-check",
                "testId": test_id,
                "failureReason": {
                    "primaryErrorMessage": failure_reason[
                        :1000
                    ]  # Cap at 1000 to be safe (<1024 bytes)
                },
            }
        ],
    }

    cmd = [
        "prpc",
        "call",
        "analysis.api.luci.app",
        "luci.analysis.v1.Clusters.Cluster",
    ]

    try:
        result = subprocess.run(
            cmd,
            input=json.dumps(payload),
            capture_output=True,
            text=True,
            check=True,
        )
        response = json.loads(result.stdout)
    except subprocess.CalledProcessError as e:
        raise RuntimeError(f"prpc Cluster failed: {e.stderr.strip()}")
    except json.JSONDecodeError as e:
        raise RuntimeError(f"Failed to parse Cluster JSON output: {e}")

    clustered_results = response.get("clusteredTestResults", [])
    if not clustered_results:
        return {"existing_bug": None, "suggested_clusters": []}

    clusters = clustered_results[0].get("clusters", [])
    existing_bug = None
    suggested_clusters = []

    for c in clusters:
        algo = c.get("clusterId", {}).get("algorithm")
        cluster_id = c.get("clusterId", {}).get("id")

        if algo == "rules":
            # Found an active rule linking this failure to a bug!
            existing_bug = c.get("bug")
            # Add rule ID
            if existing_bug:
                existing_bug["rule_id"] = cluster_id
        else:
            suggested_clusters.append({"algorithm": algo, "id": cluster_id})

    return {
        "existing_bug": existing_bug,
        "suggested_clusters": suggested_clusters,
    }


# --- Bug Description Helpers ---


def generate_bug_comment(
    test_id: str,
    failure_reason: str,
    variant: typing.Optional[dict[str, typing.Any]],
    url: str,
) -> str:
    """Generates the markdown description template for the Buganizer flake issue."""
    variant_str = json.dumps(variant, indent=2) if variant else "None"
    comment = f"""The test `{test_id}` is flaking.

* **LUCI rule**: TODO
* **First Observed Failure**: [Milo Failure Link]({url})

### Failure Reason
```
{failure_reason}
```

### Variant Details
```json
{variant_str}
```

---
*This bug was filed automatically by the Flake Triage workflow.*
"""
    return comment


# --- Subcommand handlers ---


def handle_get_failure_info(args: argparse.Namespace) -> None:
    """Handler for the 'get-failure-info' subcommand."""
    # Step 1: Parse URL
    parsed = parse_luci_url(args.url)
    invocation_id = parsed["invocation_id"]
    test_id = parsed["test_id"]

    # Step 2: Query failures
    failures = query_test_results(invocation_id, test_id, project=args.project)

    if not failures:
        print(
            json.dumps(
                {
                    "status": "PASS",
                    "message": "No unexpected test failures found in this invocation.",
                },
                indent=2,
            )
        )
        sys.exit(0)

    # Handle multiple failures
    selected_failure = None
    if len(failures) > 1:
        if not args.result_id:
            print(
                json.dumps(
                    {
                        "status": "SELECTION_REQUIRED",
                        "message": f"Multiple failures ({len(failures)}) found. Please specify --result-id.",
                        "failures": [
                            {
                                "result_id": f["result_id"],
                                "test_id": f["test_id"],
                                "variant": f["variant"],
                                "failure_reason_summary": f["failure_reason"][
                                    :200
                                ]
                                + "...",
                            }
                            for f in failures
                        ],
                    },
                    indent=2,
                )
            )
            sys.exit(0)
        else:
            # Find the chosen one
            for f in failures:
                if f["result_id"] == args.result_id:
                    selected_failure = f
                    break
            if not selected_failure:
                raise ValueError(
                    f"Result ID '{args.result_id}' not found in failures."
                )
    else:
        # Only one failure, select it automatically
        selected_failure = failures[0]

    # Step 3: Check for existing bugs & clusters in LUCI Analysis
    cluster_info = check_existing_clusters(
        args.project,
        selected_failure["test_id"],
        selected_failure["failure_reason"],
    )

    output = {
        "status": "SUCCESS",
        "url_info": parsed,
        "selected_failure": selected_failure,
        "existing_bug": cluster_info["existing_bug"],
        "suggested_clusters": cluster_info["suggested_clusters"],
    }

    print(json.dumps(output, indent=2))


def handle_create_rule(args: argparse.Namespace) -> None:
    """Handler for the 'create-rule' subcommand."""
    variant = json.loads(args.variant_json) if args.variant_json else None

    # Step 1: Generate bug description with TODO
    full_comment = generate_bug_comment(
        args.test_id, args.failure_reason, variant, args.url
    )

    # Step 2: Create bug via issues CLI
    title = args.title
    if len(title) > 250:
        title = title[:247] + "..."

    issues_bin = "/google/bin/releases/issues-cli/issues"
    create_cmd = [
        issues_bin,
        "create",
        "--title",
        title,
        "--description",
        full_comment,
        "--component_id",
        DEFAULT_COMPONENT_ID,
        "--priority",
        "P2",
        "--hotlists",
        DEFAULT_HOTLIST_ID,
    ]

    result = subprocess.run(
        create_cmd, capture_output=True, text=True, check=True
    )
    stdout = result.stdout

    # Extract Bug ID
    bug_id = None
    url_match = re.search(r"issues/(\d+)", stdout)
    if url_match:
        bug_id = url_match.group(1)
    else:
        numbers = re.findall(r"\b\d{7,9}\b", stdout)
        if numbers:
            bug_id = numbers[0]

    if not bug_id:
        raise RuntimeError(
            f"Failed to extract Bug ID from issues create output:\n{stdout}"
        )

    # Step 3: Create LUCI Analysis rule
    rule_definition = args.rule_definition
    if not rule_definition:
        rule_definition = f'test = "{args.test_id}"'

    rule_payload = {
        "parent": f"projects/{args.project}",
        "rule": {
            "project": args.project,
            "ruleDefinition": rule_definition,
            "bug": {"system": "buganizer", "id": str(bug_id)},
            "isActive": True,
            "isManagingBug": False,
            "isManagingBugPriority": False,
        },
    }

    prpc_cmd = [
        "prpc",
        "call",
        "analysis.api.luci.app",
        "luci.analysis.v1.Rules.Create",
    ]
    prpc_result = subprocess.run(
        prpc_cmd,
        input=json.dumps(rule_payload),
        capture_output=True,
        text=True,
        check=True,
    )
    rule_response = json.loads(prpc_result.stdout)
    rule_id = rule_response.get("ruleId")
    rule_name = rule_response.get("name")

    rule_url = (
        f"https://luci-analysis.appspot.com/p/{args.project}/rules/{rule_id}"
    )

    # Step 4: Update bug description with Rule URL
    updated_comment = full_comment.replace(
        "* **LUCI rule**: TODO", f"* **LUCI rule**: {rule_url}"
    )
    update_cmd = [
        issues_bin,
        "update",
        "description",
        "--issue_id",
        str(bug_id),
        "--text",
        updated_comment,
    ]
    subprocess.run(update_cmd, capture_output=True, text=True, check=True)

    hotlist_status = "SUCCESS"

    bug_url = f"https://issuetracker.google.com/issues/{bug_id}"

    print(
        json.dumps(
            {
                "status": "SUCCESS",
                "rule_id": rule_id,
                "rule_url": rule_url,
                "bug_url": bug_url,
                "bug_id": bug_id,
                "rule_name": rule_name,
                "hotlist_status": hotlist_status,
            },
            indent=2,
        )
    )


def main() -> None:
    """Main entrypoint setting up CLI argument parsing and routing."""
    parser = argparse.ArgumentParser(description="Flake Triage Utility")
    subparsers = parser.add_subparsers(
        dest="command", required=True, help="Subcommands"
    )

    # get-failure-info subcommand
    parser_parse = subparsers.add_parser(
        "get-failure-info", help="Triage a LUCI Milo failure URL"
    )
    parser_parse.add_argument("url", help="The LUCI Milo URL to triage")
    parser_parse.add_argument(
        "--result-id",
        help="The specific Result ID to choose if there are multiple failures",
    )
    parser_parse.add_argument(
        "--project",
        default="turquoise",
        help="The LUCI project (fuchsia or turquoise)",
    )
    parser_parse.set_defaults(func=handle_get_failure_info)

    # create-rule subcommand
    parser_create = subparsers.add_parser(
        "create-rule", help="File a bug and create a LUCI Analysis rule"
    )
    parser_create.add_argument(
        "--project",
        default="turquoise",
        help="LUCI Project (fuchsia or turquoise)",
    )
    parser_create.add_argument("--title", required=True, help="The bug title")
    parser_create.add_argument("--test-id", required=True, help="The test ID")
    parser_create.add_argument(
        "--failure-reason", required=True, help="The failure reason"
    )
    parser_create.add_argument(
        "--url", required=True, help="The LUCI Milo failure URL"
    )
    parser_create.add_argument(
        "--variant-json", help="JSON string of the variant def"
    )
    parser_create.add_argument(
        "--rule-definition",
        help="Custom rule definition (default: test = '<test_id>')",
    )
    parser_create.set_defaults(func=handle_create_rule)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
