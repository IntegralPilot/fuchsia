# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import collections
import json
import os
import pathlib
import statistics
import subprocess
from typing import TYPE_CHECKING, Any, TextIO, Tuple

if TYPE_CHECKING:
    # The object returned by ArgumentParser.add_subparsers().
    # This is the most accurate type for static analysis.
    # We conditionalize on TYPE_CHECKING because _SubParsersAction is a private field
    # and we don't want to depend on implementation details at runtime.
    SubParsersAction = argparse._SubParsersAction
else:
    # Fallback for runtime, Any is sufficient.
    SubParsersAction = Any


_DEFAULT_STATE_FILE = (
    "/google/data/ro/teams/tq-performance/sample_builds_state.json"
)


def download_metrics(args, out_fh: TextIO) -> None:
    """Downloads metrics artifacts from CAS."""
    bb_tool = _find_bb_tool()
    _check_bb_auth(bb_tool)
    _download_metrics(bb_tool, args.build_id, args.out_dir, out_fh)


def download_baseline(args, out_fh: TextIO) -> None:
    """Finds the most recent successful sample build for a given builder and downloads its metrics."""
    bb_tool = _find_bb_tool()
    _check_bb_auth(bb_tool)
    _download_baseline(
        bb_tool,
        args.builder_name,
        args.count,
        args.out_dir,
        args.state_file,
        out_fh,
    )


def extract_metrics(args, out_fh: TextIO) -> None:
    """Extracts metrics from Fuchsia performance results."""
    _extract_metrics(args.suite_prefix, args.data_dir, args.out_file, out_fh)


def register_subparsers(subparsers: SubParsersAction, out_fh: TextIO) -> None:
    """Registers subparsers for workflow commands."""
    subparser = subparsers.add_parser(
        "download_metrics",
        help="Downloads metrics artifacts from CAS.",
    )
    subparser.add_argument(
        "build_id",
        help="The Buildbucket build ID for which to download metrics.",
    )
    subparser.add_argument(
        "out_dir",
        help="The directory to save the downloaded CAS contents into.",
    )
    subparser.set_defaults(func=lambda args: download_metrics(args, out_fh))

    subparser = subparsers.add_parser(
        "download_baseline",
        help="Finds the most recent successful sample build and downloads its metrics.",
    )
    subparser.add_argument(
        "-n",
        "--count",
        type=int,
        default=5,
        help="The number of builds to check (default: 5).",
    )
    subparser.add_argument(
        "builder_name",
        help="The name of the builder by which to filter.",
    )
    subparser.add_argument(
        "out_dir",
        help="The directory to save the downloaded CAS contents into.",
    )
    subparser.add_argument(
        "--state_file",
        default=_DEFAULT_STATE_FILE,
        help="Path to the sample builds state file.",
    )
    subparser.set_defaults(func=lambda args: download_baseline(args, out_fh))

    subparser = subparsers.add_parser(
        "extract_metrics",
        help="Extracts metrics from Fuchsia performance results.",
    )
    subparser.add_argument(
        "suite_prefix",
        help="The test suite prefix by which to filter metrics.",
    )
    subparser.add_argument(
        "data_dir",
        nargs="?",
        default=".",
        help="Directory to process. Defaults to the current directory.",
    )
    subparser.add_argument(
        "--out_file",
        help="A path to save the resulting CSV.",
    )
    subparser.set_defaults(func=lambda args: extract_metrics(args, out_fh))


def _download_metrics(
    bb_tool: str, build_id: str, out_dir: str, out_fh: TextIO
) -> None:
    os.makedirs(out_dir, exist_ok=True)

    cas_tool = _find_cas_tool()

    try:
        digest, instance = _get_cas_info_from_build(bb_tool, build_id, out_fh)
        _download_from_cas(cas_tool, digest, instance, out_dir, out_fh)
    except (subprocess.CalledProcessError, RuntimeError) as e:
        raise RuntimeError(f"Download failed for build {build_id}") from e


def _download_baseline(
    bb_tool: str,
    builder_name: str,
    count: int,
    out_dir: str,
    state_file: str,
    out_fh: TextIO,
) -> None:
    if not os.path.exists(state_file):
        raise FileNotFoundError(f"Error: State file not found at {state_file}")

    with open(state_file, "r") as f:
        state = json.load(f)

    filtered_dates = [
        date
        for date, entry in state.items()
        if isinstance(entry.get("launched"), dict)
    ]

    sorted_dates = sorted(filtered_dates, reverse=True)
    last_n_dates = sorted_dates[:count]

    out_fh.write(
        f"Finding most recent successful sample build for {builder_name} "
        f"(checking last {count} entries)...\n"
    )

    for date in last_n_dates:
        launched = state[date].get("launched", {})
        build_id = launched.get(builder_name, {}).get("build_id", "")
        if build_id:
            status = _get_build_status(bb_tool, build_id)

            if status == "SUCCESS":
                out_fh.write(
                    f"  {date}: {build_id} (SUCCESS) - Triggering download to {out_dir}...\n"
                )

                _download_metrics(bb_tool, build_id, out_dir, out_fh)
                break

            out_fh.write(f"  {date}: {build_id} ({status}) - Skipping...\n")
    else:
        out_fh.write(
            f"No successful sample build IDs found for {builder_name} in the last "
            f"{count} entries.\n"
        )


def _extract_metrics(
    suite_prefix: str, data_dir: str, out_file: str, out_fh: TextIO
) -> None:
    target_dir = pathlib.Path(data_dir)
    perfcompare_dirs = (target_dir / "with_cl", target_dir / "without_cl")
    if all(p.is_dir() for p in perfcompare_dirs):
        if not out_file:
            raise ValueError(
                "Error: --out_file is required when data_dir contains results from multiple builds."
            )
        with open(perfcompare_dirs[0] / out_file, "w") as f:
            _process_files(suite_prefix, perfcompare_dirs[0], f, out_fh)
        with open(perfcompare_dirs[1] / out_file, "w") as f:
            _process_files(suite_prefix, perfcompare_dirs[1], f, out_fh)

    elif out_file:
        with open(out_file, "w") as f:
            _process_files(suite_prefix, target_dir, f, out_fh)

    else:
        _process_files(suite_prefix, target_dir, out_fh, out_fh)


def _process_files(
    suite_prefix: str,
    search_dir: pathlib.Path,
    out_file: TextIO,
    out_fh: TextIO,
) -> None:
    """Finds and processes all *.fuchsiaperf.json files in a single pass."""
    fuchsia_perf_files = sorted(search_dir.rglob("*.fuchsiaperf.json"))

    aggregated_results = collections.defaultdict(
        lambda: collections.defaultdict(list)
    )

    for file_path in fuchsia_perf_files:
        try:
            with open(file_path) as f:
                data = json.load(f)
                for record in data:
                    suite = record.get("test_suite")
                    label = record.get("label")

                    if suite and suite.startswith(suite_prefix):
                        # We just take the mean if there are multiple values.
                        # This is lossy, but it's the most sensical thing we can
                        # do when aggregating multiple runs for output in CSV.
                        value = statistics.mean(record["values"])
                        aggregated_results[suite][label].append(value)
        except json.JSONDecodeError as e:
            raise RuntimeError(
                f"Malformed JSON in file {file_path}: {e}"
            ) from e

    for suite, metrics in sorted(aggregated_results.items()):
        for metric, values in sorted(metrics.items()):
            values_str = ",".join(map(str, values))
            out_file.write(f"{suite},{metric},{values_str}\n")


def _get_cas_info_from_build(
    bb_tool: pathlib.Path, build_id: str, out_fh: TextIO
) -> Tuple[str, str]:
    """Uses the `bb` tool to get the CAS digest and instance for a build."""
    cmd = [bb_tool, "get", "-p", "-json", build_id]
    out_fh.write(f"Fetching build info for {build_id} using bb...\n")
    result = subprocess.run(cmd, capture_output=True, text=True, check=True)

    try:
        build_info = json.loads(result.stdout)
    except json.JSONDecodeError as e:
        raise RuntimeError(f"Failed to parse bb output as JSON: {e}") from e

    properties = build_info["output"]["properties"]
    try:
        instance = properties["cas_instance"]
        digest = properties.get("perf_dataset_digest") or properties.get(
            "perfcompare_dataset_digest"
        )
        if not digest:
            raise KeyError("perf_dataset_digest or perfcompare_dataset_digest")
    except KeyError as e:
        raise KeyError(
            f"Missing required entry {e} in build output properties: {properties}"
        ) from e

    return digest, instance


def _download_from_cas(
    cas_tool: pathlib.Path,
    digest: str,
    instance: str,
    out_dir: str,
    out_fh: TextIO,
) -> None:
    """Uses the hermetic `cas` binary to download the specified digest."""
    cmd = [
        cas_tool,
        "download",
        "-digest",
        digest,
        "-cas-instance",
        instance,
        "-dir",
        out_dir,
    ]
    out_fh.write(f"Downloading from cas to {out_dir}...\n")
    subprocess.run(cmd, check=True)


def _get_build_status(bb_tool: pathlib.Path, build_id: str) -> str:
    """Uses the `bb` tool to get the status of a build."""
    cmd = [bb_tool, "get", "-json", build_id]
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, check=True)
        build_info = json.loads(result.stdout)
        return build_info.get("status", "UNKNOWN")
    except (subprocess.CalledProcessError, json.JSONDecodeError):
        return "ERROR"


def _check_bb_auth(bb_tool: pathlib.Path) -> None:
    """Checks if the user is authenticated with buildbucket."""
    result = subprocess.run(
        [bb_tool, "auth-info"], capture_output=True, text=True
    )
    if result.returncode != 0:
        raise RuntimeError(
            'Error: "bb auth-info" returned an error, indicating that you are '
            "not authenticated with Buildbucket.\n"
            f"Please run: {bb_tool} auth-login"
        )


def _find_bb_tool() -> pathlib.Path:
    """Finds the path to the hermetic `bb` binary in the Fuchsia checkout."""
    fuchsia_dir = _ensure_fuchsia_dir()
    bb_path = fuchsia_dir / "prebuilt" / "tools" / "buildbucket" / "bb"

    if not bb_path.exists():
        raise RuntimeError(
            f"Could not find `bb` tool at expected location: {bb_path}"
        )

    return bb_path


def _find_cas_tool() -> pathlib.Path:
    """Finds the path to the hermetic `cas` binary in the Fuchsia checkout."""
    fuchsia_dir = _ensure_fuchsia_dir()
    cas_path = fuchsia_dir / "prebuilt" / "tools" / "cas" / "cas"

    if not cas_path.exists():
        raise RuntimeError(
            f"Could not find `cas` tool at expected location: {cas_path}"
        )

    return cas_path


def _ensure_fuchsia_dir() -> pathlib.Path:
    fuchsia_dir = os.environ.get("FUCHSIA_DIR")
    if not fuchsia_dir:
        raise RuntimeError(
            "FUCHSIA_DIR environment variable is not set. Are you running via `fx`?"
        )
    return pathlib.Path(fuchsia_dir)
