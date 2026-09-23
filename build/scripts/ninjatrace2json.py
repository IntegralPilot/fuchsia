#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Collect Ninja trace information for analysis in chrome://tracing."""

import argparse
import dataclasses
import gzip
import json
import os
import subprocess
from collections.abc import Iterable
from concurrent import futures
from pathlib import Path
from typing import Any

JsonTrace = dict[str, Any]

NINJA_BUILD_TRACE_BASENAME = "ninja_build_trace.json.gz"
COMPDB_BASENAME = "compdb.json"
GRAPH_BASENAME = "graph.dot"
NINJATRACE_BASENAME = "ninjatrace.json.gz"
NINJA_SUBBUILDS_JSON = "ninja_subbuilds.json"


def _subbuild_ninja_target(build_dir: Path) -> str:
    """Returns the name of the ninja target from the main build, based on the
    subbuild directory."""
    # LINT.IfChange
    return str(build_dir.name) + ".stamp"
    # LINT.ThenChange(//build/subbuild.gni)


def load_compressed_trace(trace_path: Path) -> list[JsonTrace]:
    with gzip.open(trace_path) as f:
        return json.load(f)


def write_compressed_trace(
    trace_path: Path, trace_data: list[JsonTrace]
) -> None:
    with gzip.open(trace_path, "wt") as f:
        json.dump(trace_data, f)


@dataclasses.dataclass
class Tracer:
    """Helper class that closes over some static configuration."""

    ninja_path: Path
    ninjatrace_path: Path
    rbe_rpl_path: Path
    rpl2trace_path: Path
    save_temps: bool

    def trace_build_dir(
        self,
        build_dir: Path,
    ) -> Path:
        """Generate the Ninja trace for a single build directory (either the
        main build or a subbuild).

        The resulting trace will be written to `build_dir / ninjatrace.json`,
        but will also be parsed and returned."""
        ninja_build_trace = build_dir / NINJA_BUILD_TRACE_BASENAME
        trace = build_dir / NINJATRACE_BASENAME

        ninjatrace_args: list[str | os.PathLike[str]] = [
            self.ninjatrace_path,
            "-ninjabuildtrace",
            ninja_build_trace,
            "-trace-json",
            trace,
            "-critical-path",
        ]

        if self.rbe_rpl_path:
            ninjatrace_args.extend(
                [
                    "-rbe-rpl-path",
                    self.rbe_rpl_path,
                    "-rpl2trace-path",
                    self.rpl2trace_path,
                ]
            )
        subprocess.run(ninjatrace_args, check=True)

        return trace

    def find_and_merge_subbuilds(
        self, main_build_dir: Path, main_build_traces: list[JsonTrace]
    ) -> bool:
        """Given the main build dir and its trace, find any subbuilds referenced
        by that trace, build them, load them, and merge them into a single
        trace file.

        Returns True if the main build trace has been modified with the merged subbuild traces.
        """

        # ninja_subbuilds.json is a build API module listing the set of
        # directories that _could_ contain subbuilds, but those subbuilds may
        # not have actually run as part of the last build.
        with (main_build_dir / NINJA_SUBBUILDS_JSON).open() as f:
            possible_subbuild_dirs = [
                Path(b["build_dir"]) for b in json.load(f)
            ]

        # If there aren't any subbuilds possible, exit early so we don't spend any more time on
        # processing the main traces to include the subbuilds.
        if not possible_subbuild_dirs:
            return False

        # Create a list of possible trace event names for the possible subbuilds
        # this is used to filter the main build trace events to find the any
        # subbuilds that were run.
        possible_subbuild_target_names = {
            _subbuild_ninja_target(b): b for b in possible_subbuild_dirs
        }

        # Get the JsonTrace from the main build for each of the subbuilds.  These are used to
        # establish the start-time for each of the subbuilds' traces in the merged trace file.
        # This filtering is so that the main_build_traces list only needs to be iterated over
        # once, comparing against a very short list of possible target names.
        main_build_traces_by_subbuild_dir: dict[Path, JsonTrace] = {
            possible_subbuild_target_names[t["name"]]: t
            for t in main_build_traces
            if t["name"] in possible_subbuild_target_names
        }

        # If there weren't any subbuilds in the last build, then exit early
        if not main_build_traces_by_subbuild_dir:
            return False

        # Load the traces for each of the subbuild dirs in parallel
        with futures.ThreadPoolExecutor() as pool:
            subbuilds_traces: Iterable[list[JsonTrace]] = pool.map(
                lambda subbuild_dir:
                # Subbuilds don't need extra targets (as of this writing).
                load_compressed_trace(
                    self.trace_build_dir(
                        build_dir=main_build_dir / subbuild_dir
                    )
                ),
                main_build_traces_by_subbuild_dir,
            )

        # For each of the subbuilds, take all the trace events and offset their ts by
        # the start time of the corresponding trace from the main build, and set the
        # pid field to the name of the subbuild, and add them to the set of main traces
        for (
            (subbuild_dir, target_in_main_build_trace),
            subbuild_traces,
        ) in zip(main_build_traces_by_subbuild_dir.items(), subbuilds_traces):
            subbuild_start = target_in_main_build_trace["ts"]
            subbuild_name = subbuild_dir.name

            main_build_traces.extend(
                [
                    {
                        **t,
                        # Rewrite the trace to set "pid" to indicate the subbuild.
                        "pid": subbuild_name,
                        # And offset the time by the start time of the subbuild
                        # action in the main build./
                        "ts": t["ts"] + subbuild_start,
                    }
                    for t in subbuild_traces
                ]
            )

        # We've modified the traces, so return True to signal that the merged output
        # needs to be written.
        return True


def extract_trace_events(raw_trace: Any) -> list[JsonTrace]:
    """Extracts a list of Chrome trace events from parsed raw trace data.

    Supports both official Chrome Trace formats:
    - JSON Array: raw_trace is a flat list of event dictionaries.
    - JSON Object: raw_trace is a dictionary containing a 'traceEvents' key.
    """
    if isinstance(raw_trace, list):
        return raw_trace
    if isinstance(raw_trace, dict):
        return raw_trace.get("traceEvents", [])
    return []


def merge_profile(profile: Path, main_build_traces: list[JsonTrace]) -> bool:
    with open(profile) as f:
        raw_trace = json.load(f)

    main_build_traces.extend(extract_trace_events(raw_trace))
    return True


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--extra-ninja-targets",
        nargs="*",
        help="""\
If you ran a full `fx build`, ignore this flag. If you built a specific set of
ninja targets (e.g. `fx build my_target other_target`), some of which aren't
depended on `//:default`, list those targets here. Otherwise they might not show
up in the resulting traces.""",
    )
    parser.add_argument(
        "--save-temps",
        action="store_true",
        help="""\
if set, keep the intermediate compdb.json and graph.dot files
in each build directory.  The are only needed temporarily to produce the
final ninjatrace.json, and can be large at O(100)s of MBs.""",
    )
    parser.add_argument(
        "--fuchsia-build-dir",
        type=Path,
        required=True,
        help="Path to the Fuchsia build directory.",
    )
    parser.add_argument(
        "--ninja-path",
        type=Path,
        required=True,
        help="Path to the prebuilt ninja binary.",
    )
    parser.add_argument(
        "--ninjatrace-path",
        type=Path,
        required=True,
        help="Path to the prebuilt ninjatrace binary.",
    )
    parser.add_argument(
        "--rbe-rpl-path",
        help="when provided, interleave remote execution stats from RBE into the main trace",
    )
    parser.add_argument(
        "--rpl2trace-path",
        type=Path,
        help="Path to the prebuilt rpl2trace tool.",
    )
    parser.add_argument(
        "--subbuilds-in-place",
        action="store_true",
        help="""\
If set, merge traces from subbuilds with traces from the main build and
include these traces in the main build's ninjatrace.json file. Must not be
specified if --subbuilds-output-path is set.""",
    )
    parser.add_argument(
        "--system-profile",
        type=Path,
        help="Path to a consolidated system profiling log to incorporate into the merged build.",
    )
    args = parser.parse_args()

    tracer = Tracer(
        ninja_path=args.ninja_path,
        ninjatrace_path=args.ninjatrace_path,
        rbe_rpl_path=args.rbe_rpl_path,
        rpl2trace_path=args.rpl2trace_path,
        save_temps=args.save_temps,
    )

    fuchsia_build_dir = args.fuchsia_build_dir

    # Convert the trace for the main build
    outpath: Path = tracer.trace_build_dir(fuchsia_build_dir)

    if args.subbuilds_in_place or args.system_profile:
        # We are merging other trace files, so read in the converted trace
        # for the main build.
        main_build_traces = load_compressed_trace(
            fuchsia_build_dir / NINJATRACE_BASENAME
        )

        traces_merged = False
        if args.subbuilds_in_place:
            traces_merged = (
                tracer.find_and_merge_subbuilds(
                    fuchsia_build_dir, main_build_traces
                )
                or traces_merged
            )

        if args.system_profile:
            traces_merged = (
                merge_profile(args.system_profile, main_build_traces)
                or traces_merged
            )

        if traces_merged:
            write_compressed_trace(
                fuchsia_build_dir / NINJATRACE_BASENAME, main_build_traces
            )

    print(f"Now visit chrome://tracing and load {str(outpath)}")


if __name__ == "__main__":
    main()
