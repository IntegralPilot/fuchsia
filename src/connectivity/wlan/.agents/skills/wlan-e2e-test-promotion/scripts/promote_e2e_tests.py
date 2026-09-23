#!/usr/bin/env python3
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import os
import re
import shutil
import subprocess
import sys
import urllib.parse
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timezone

PROJECT = "turquoise"
BUILDER = "fuchsia_internal.arm64-release-fyi"
BUCKET = "global.ci"


def check_prerequisites():
    missing = []
    for cmd in ("prpc", "rdb"):
        if shutil.which(cmd) is None:
            missing.append(cmd)
    if missing:
        print(
            f"Error: Missing required commands: {', '.join(missing)}",
            file=sys.stderr,
        )
        print(
            "Please install depot_tools by following the instructions at go/depottools#_setting_up",
            file=sys.stderr,
        )
        sys.exit(1)


def run_cmd(cmd, input_data=None):
    try:
        res = subprocess.run(
            cmd, input=input_data, capture_output=True, text=True, check=True
        )
        return res.stdout
    except subprocess.CalledProcessError as e:
        print(f"Error running {' '.join(cmd)}: {e.stderr}", file=sys.stderr)
        return None


def parse_fyi_tests_from_file(filepath, workspace_root=None):
    """Simple regex-based parser to extract tests in tests_for_fyi from GN/GNI files."""
    with open(filepath, "r") as f:
        content = f.read()
    content = re.sub(r"#.*", "", content)  # Remove comments
    tests = []

    def format_dep(dep):
        dep = dep.strip().strip('"').strip("'").strip(":")
        if not dep:
            return None
        if not dep.startswith("//") and workspace_root:
            rel_dir = os.path.relpath(os.path.dirname(filepath), workspace_root)
            return f"//{rel_dir}:{dep}"
        return dep

    # Pattern 1: group("tests_for_fyi") { ... public_deps = [ ... ] }
    group_match = re.search(
        r'group\s*\(\s*"\s*tests_for_fyi\s*"\s*\)\s*\{([^}]+)\}',
        content,
        re.DOTALL,
    )
    if group_match:
        group_content = group_match.group(1)
        deps_match = re.search(
            r"public_deps\s*=\s*\[([^\]]+)\]", group_content, re.DOTALL
        )
        if deps_match:
            for dep in deps_match.group(1).split(","):
                formatted = format_dep(dep)
                if formatted:
                    tests.append(formatted)

    # Pattern 2: <prefix>_tests_for_fyi = [ ... ]
    gni_match = re.search(
        r"\w+_tests_for_fyi\s*=\s*\[([^\]]+)\]", content, re.DOTALL
    )
    if gni_match:
        for dep in gni_match.group(1).split(","):
            formatted = format_dep(dep)
            if formatted:
                tests.append(formatted)

    return tests


def find_all_fyi_tests(workspace_root):
    """Scans the WLAN tests directory in open source and vendor repos to find all tests currently in FYI."""
    search_dirs = [
        os.path.join(workspace_root, "src/connectivity/wlan/tests"),
        os.path.join(workspace_root, "vendor/google/tests/end_to_end/wlan"),
    ]
    fyi_tests = set()

    for tests_dir in search_dirs:
        if not os.path.exists(tests_dir):
            continue
        for root, dirs, files in os.walk(tests_dir):
            for file in files:
                if file in ("BUILD.gn", "test_lists.gni"):
                    filepath = os.path.join(root, file)
                    parsed = parse_fyi_tests_from_file(filepath, workspace_root)
                    for t in parsed:
                        fyi_tests.add(t)
    return sorted(list(fyi_tests))


def get_latest_build_id():
    payload = {
        "predicate": {
            "builder": {
                "project": PROJECT,
                "bucket": BUCKET,
                "builder": BUILDER,
            },
            "status": "ENDED_MASK",
        },
        "pageSize": 1,
    }
    cmd = [
        "prpc",
        "call",
        "cr-buildbucket.appspot.com",
        "buildbucket.v2.Builds.SearchBuilds",
    ]
    out = run_cmd(cmd, json.dumps(payload))
    if not out:
        return None
    data = json.loads(out)
    builds = data.get("builds", [])
    if not builds:
        return None
    return builds[0].get("id")


def get_top_level_tests_in_build(build_id):
    """Queries ResultDB for all top-level test IDs in the build."""
    cmd = [
        "rdb",
        "query",
        "-json",
        "-tr-fields",
        "testId,testMetadata",
        f"build-{build_id}",
    ]
    out = run_cmd(cmd)
    if not out:
        return []

    test_ids = set()
    for line in out.splitlines():
        if not line.strip():
            continue
        try:
            data = json.loads(line)
            tr = data.get("testResult", {})
            tid = tr.get("testId")
            if not tid:
                continue

            meta = tr.get("testMetadata", {})
            # Only keep top-level tests (which have empty testMetadata.name)
            if "name" not in meta or not meta["name"]:
                test_ids.add(tid)
        except json.JSONDecodeError:
            continue
    return list(test_ids)


def clean_test_name(name):
    # Remove leading //
    name = name.lstrip("/")
    # If it has a colon, take the part after the colon (the target name)
    if ":" in name:
        name = name.split(":")[-1]
    return name


def match_test_id(fyi_name, tid):
    """Matches an FYI test target against a ResultDB test ID."""
    if ":" in fyi_name and fyi_name.startswith("//"):
        gn_path = fyi_name.lstrip("/").replace(":", "/")
        if f"{gn_path}.sh-" in tid:
            return True
    cleaned = clean_test_name(fyi_name)
    return f"/{cleaned}.sh-" in tid


def format_time_ago(partition_time_str, now):
    if not partition_time_str:
        return "unknown time ago", ""
    try:
        dt = datetime.fromisoformat(partition_time_str)
        diff = now - dt
        days = diff.days
        hours = diff.seconds // 3600
        minutes = (diff.seconds % 3600) // 60
        if days > 0:
            time_ago = f"{days}d {hours}h ago"
        elif hours > 0:
            time_ago = f"{hours}h {minutes}m ago"
        else:
            time_ago = f"{minutes}m ago"
        date_str = dt.strftime("%Y-%m-%d %H:%M UTC")
        return time_ago, date_str
    except Exception:
        return "unknown time ago", ""


def check_stability(test_id):
    payload = {
        "project": PROJECT,
        "testId": test_id,
        "predicate": {
            # Filter by sub_realm to only count CI results and exclude tryjob builders (i.e. global.try).
            "sub_realm": BUCKET,
        },
        "pageSize": 300,
    }
    cmd = [
        "prpc",
        "call",
        "analysis.api.luci.app",
        "luci.analysis.v1.TestHistory.Query",
    ]
    out = run_cmd(cmd, json.dumps(payload))
    if not out:
        return {"error": "Failed to query LUCI Analysis"}

    data = json.loads(out)
    verdicts = data.get("verdicts", [])

    # Always calculate average runtime over the last 20 runs if verdicts are available
    runtimes = []
    for v in verdicts[:20]:
        dur_str = v.get("passedAvgDuration", "0s")
        if dur_str.endswith("s"):
            runtimes.append(float(dur_str[:-1]))
    avg_runtime = sum(runtimes) / len(runtimes) if runtimes else 0

    encoded_id = urllib.parse.quote(test_id, safe="")
    history_url = (
        f"https://luci-milo.appspot.com/ui/test/{PROJECT}/{encoded_id}"
    )

    failed = 0
    flaky = 0
    error = 0
    passed = 0

    consecutive_passes = 0
    latest_failure = None
    now = datetime.now(timezone.utc)

    for i, v in enumerate(verdicts):
        status = v.get("statusV2")
        if status == "PASSED":
            passed += 1
            if latest_failure is None:
                consecutive_passes += 1
        else:
            if status == "FAILED":
                failed += 1
            elif status == "FLAKY":
                flaky += 1
            elif status in ("EXECUTION_ERRORED", "PRECLUDED"):
                error += 1

            if latest_failure is None:
                part_time = v.get("partitionTime")
                time_ago_str, date_str = format_time_ago(part_time, now)
                latest_failure = {
                    "status": status,
                    "runs_ago": i,
                    "partition_time": part_time,
                    "time_ago": time_ago_str,
                    "date": date_str,
                }

    if len(verdicts) < 300:
        return {
            "stable": False,
            "reason": f"Only ran {len(verdicts)} times in 90 days (needs 300+).",
            "total_runs": len(verdicts),
            "consecutive_passes": consecutive_passes,
            "latest_failure": latest_failure,
            "avg_runtime_20": avg_runtime,
            "history_url": history_url,
        }

    if failed > 0 or flaky > 0 or error > 0:
        reason = f"Failed {failed} times, Flaky {flaky} times, Errored {error} times out of {len(verdicts)} runs."
        return {
            "stable": False,
            "reason": reason,
            "total_runs": len(verdicts),
            "consecutive_passes": consecutive_passes,
            "latest_failure": latest_failure,
            "avg_runtime_20": avg_runtime,
            "history_url": history_url,
        }

    return {
        "stable": True,
        "total_runs": len(verdicts),
        "consecutive_passes": consecutive_passes,
        "latest_failure": None,
        "avg_runtime_20": avg_runtime,
        "history_url": history_url,
    }


def process_test(test_id):
    res = check_stability(test_id)
    return test_id, res


def main():
    check_prerequisites()
    # Determine workspace root (assumes run from the workspace root)
    workspace_root = os.getcwd()

    print("1. Scanning workspace for FYI tests...")
    fyi_test_names = find_all_fyi_tests(workspace_root)
    print(
        f"Found {len(fyi_test_names)} FYI tests in GN/GNI files: {', '.join(fyi_test_names)}"
    )

    if not fyi_test_names:
        print("No FYI tests found to promote.")
        return

    print("\n2. Finding latest completed build ID...")
    build_id = get_latest_build_id()
    if not build_id:
        print("Failed to find latest build.")
        sys.exit(1)
    print(f"Found latest build: {build_id}")

    print("\n3. Fetching all top-level tests from build...")
    build_tids = get_top_level_tests_in_build(build_id)
    print(f"Found {len(build_tids)} top-level tests in build.")

    # Match FYI tests against build test IDs (grouping by FYI test)
    matched_tests = {}  # Map: fyi_name -> list of ResultDB tids
    for fyi_name in fyi_test_names:
        cleaned_fyi_name = clean_test_name(fyi_name)
        matched_tests[fyi_name] = []
        for tid in build_tids:
            if match_test_id(fyi_name, tid):
                matched_tests[fyi_name].append(tid)

        if not matched_tests[fyi_name]:
            print(
                f"Warning: Could not match FYI test '{fyi_name}' (cleaned: '{cleaned_fyi_name}') to any ResultDB ID in build."
            )

    print("\nMatched FYI tests to ResultDB IDs:")
    for fyi_name, tids in matched_tests.items():
        print(f"  * {fyi_name}:")
        for tid in tids:
            print(f"    - {tid}")

    # Flatten to process in parallel
    all_tids_to_query = []
    for fyi_name, tids in matched_tests.items():
        for tid in tids:
            all_tids_to_query.append(tid)

    if not all_tids_to_query:
        print(
            "No matched tests found in the latest build to query history for."
        )
        return

    print(
        "\n4. Querying LUCI Analysis for stability (last 300 runs) in parallel..."
    )
    results = {}
    with ThreadPoolExecutor(max_workers=5) as executor:
        futures = {
            executor.submit(process_test, tid): tid for tid in all_tids_to_query
        }
        for future in as_completed(futures):
            tid, res = future.result()
            results[tid] = res

    # Aggregate stability by FYI test
    fyi_stability = (
        {}
    )  # Map: fyi_name -> { "stable": bool, "variants": { tid: res } }
    for fyi_name, tids in matched_tests.items():
        if not tids:
            continue

        all_stable = True
        variants_data = {}
        for tid in tids:
            res = results.get(tid, {"error": "No result"})
            variants_data[tid] = res
            if "error" in res or not res.get("stable", False):
                all_stable = False

        fyi_stability[fyi_name] = {
            "stable": all_stable,
            "variants": variants_data,
        }

    # Print Report
    promoted = []
    failed_bar = []

    for fyi_name, data in fyi_stability.items():
        if data["stable"]:
            promoted.append((fyi_name, data))
        else:
            failed_bar.append((fyi_name, data))

    print("\n==========================================")
    print("            STABILITY REPORT              ")
    print("==========================================")

    print("\n=== Ready for Promotion (Stable on ALL boards) ===")
    if not promoted:
        print("None")
    else:
        for fyi_name, data in promoted:
            print(f"* FYI Test: {fyi_name}")
            for tid, res in data["variants"].items():
                board = tid.split(".")[-1] if "." in tid else "unknown"
                print(f"  - Board [{board}]: {tid}")
                print(
                    f"    Consecutive Passes (up to present): {res.get('consecutive_passes', 0)} (no failures in last {res.get('total_runs', 0)} runs)"
                )
                print(
                    f"    Average Runtime (last 20 runs): {res['avg_runtime_20']:.2f}s"
                )
                print(f"    History URL: {res['history_url']}")
            print()

    print("=== Not Ready for Promotion ===")
    if not failed_bar:
        print("None")
    else:
        for fyi_name, data in failed_bar:
            print(f"* FYI Test: {fyi_name}")
            for tid, res in data["variants"].items():
                board = tid.split(".")[-1] if "." in tid else "unknown"
                if "error" in res:
                    status = f"API Error: {res['error']}"
                elif res["stable"]:
                    status = f"Stable (Runs: {res['total_runs']}, Avg: {res['avg_runtime_20']:.2f}s)"
                else:
                    status = f"Unstable - {res['reason']}"

                print(f"  - Board [{board}]: {tid}")
                print(f"    Status: {status}")
                if "consecutive_passes" in res:
                    lf = res.get("latest_failure")
                    if lf:
                        if lf["runs_ago"] == 0:
                            lf_info = f"most recent run was {lf['status']} ({lf['time_ago']}, {lf['date']})"
                        else:
                            lf_info = f"latest {lf['status']} was {lf['runs_ago']} runs ago ({lf['time_ago']}, {lf['date']})"
                        print(
                            f"    Consecutive Passes (up to present): {res['consecutive_passes']} ({lf_info})"
                        )
                    else:
                        print(
                            f"    Consecutive Passes (up to present): {res['consecutive_passes']} (no failures in {res['total_runs']} runs)"
                        )
                if "avg_runtime_20" in res:
                    print(
                        f"    Average Runtime (last 20 runs): {res['avg_runtime_20']:.2f}s"
                    )
                history_url = res.get("history_url")
                if not history_url:
                    encoded_id = urllib.parse.quote(tid, safe="")
                    history_url = f"https://luci-milo.appspot.com/ui/test/{PROJECT}/{encoded_id}"
                print(f"    History URL: {history_url}")
            print()


if __name__ == "__main__":
    main()
