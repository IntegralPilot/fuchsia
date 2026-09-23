#!/usr/bin/env fuchsia-vendored-python
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Functions for working with reproxy logs.

This requires python protobufs for reproxy LogDump.
"""

import argparse
import dataclasses
import hashlib
import json
import math
import multiprocessing
import os
import shutil
import subprocess
import sys
from collections.abc import Callable, Iterable, Sequence
from pathlib import Path

import fuchsia
from api.log import log_pb2
from go.api.command import command_pb2
from pb_message_util import proto_message_to_bq_dict

_SCRIPT_BASENAME = Path(__file__).name


def msg(text: str) -> None:
    print(f"[{_SCRIPT_BASENAME}] {text}")


@dataclasses.dataclass
class DownloadEvent(object):
    bytes_downloaded: int
    start_time: float
    end_time: float

    def distribute_over_intervals(
        self, interval_seconds: float
    ) -> Iterable[tuple[int, float, float]]:
        """Distribute bytes_downloaded into indexed time intervals.

        Args:
          interval_seconds: scaling factor from time to interval index,
            assuming indices start at 0.

        Returns:
          Series of (index, bytes_downloaded, duty_cycle) intervals:
          'bytes_downloaded' distribution of bytes_downloaded per time interval.
          'duty_cycle' represents the fraction of the interval spent trying
            to download, i.e. counts towards number of concurrent downloads.
            Values range in [0.0, 1.0].
        """
        lower_bound = self.start_time / interval_seconds
        upper_bound = self.end_time / interval_seconds
        duration_intervals = upper_bound - lower_bound
        avg_rate = self.bytes_downloaded / duration_intervals
        cursor = lower_bound
        while cursor < upper_bound:
            index = int(math.floor(cursor))
            next_cursor = min(index + 1, upper_bound)
            duty_cycle = next_cursor - cursor
            bytes_downloaded = duty_cycle * avg_rate
            yield index, bytes_downloaded, duty_cycle
            cursor = next_cursor


def distribute_bytes_downloaded_over_time(
    total_time_seconds: float,
    interval_seconds: float,
    events: Iterable[DownloadEvent],
) -> Sequence[tuple[float, float]]:
    """Distribute a series of download intervals over discrete time-partitions.

    Returns:
      Tuples: (indexed by time interval)
        bytes_downloaded: total number of bytes downloaded per interval.
        concurrent_downloads: number of concurrent downloads per interval.
    """
    num_intervals = math.ceil(total_time_seconds / interval_seconds) + 1
    bytes_per_interval = [0.0] * num_intervals
    concurrent_downloads_per_interval = [0.0] * num_intervals
    for event in events:
        for index, bytes, duty_cycle in event.distribute_over_intervals(
            interval_seconds
        ):
            bytes_per_interval[index] += bytes
            concurrent_downloads_per_interval[index] += duty_cycle
    return list(zip(bytes_per_interval, concurrent_downloads_per_interval))


def timestamp_pb_to_float(timestamp) -> float:
    return timestamp.seconds + (timestamp.nanos / 1e9)


class ReproxyLog(object):
    """Contains a LogDump proto and indexes information internally.

    The output_file paths used are relative to the working dir.
    """

    def __init__(self, log_dump: log_pb2.LogDump):
        self._proto: log_pb2.LogDump = log_dump

        # Index in a few ways for easy lookup.
        self._records_by_output_file = self._index_records_by_output_file()
        self._records_by_action_digest = self._index_records_by_action_digest()

    def _index_records_by_output_file(self) -> dict[Path, log_pb2.LogRecord]:
        """Map files to the records of the actions that created them."""
        records: dict[Path, log_pb2.LogRecord] = dict()
        for record in self.proto.records:
            for output_file in record.command.output.output_files:
                records[Path(output_file)] = record
        return records

    def _index_records_by_action_digest(self) -> dict[str, log_pb2.LogRecord]:
        """Map action records by their action digest (hash/size)."""
        return {
            record.remote_metadata.action_digest: record
            for record in self.proto.records
            # Not all actions have remote_metadata, due to local/racing.
            # If there happens to be duplicates (due to retries),
            # just keep the last one.
            if record.HasField("remote_metadata")
            and record.remote_metadata.action_digest
        }

    @property
    def proto(self) -> log_pb2.LogDump:
        return self._proto

    @property
    def records_by_output_file(self) -> dict[Path, log_pb2.LogRecord]:
        return self._records_by_output_file

    @property
    def records_by_action_digest(self) -> dict[str, log_pb2.LogRecord]:
        return self._records_by_action_digest

    def diff_by_outputs(
        self,
        other: "ReproxyLog",
        eq_comparator: Callable[[log_pb2.LogRecord, log_pb2.LogRecord], bool],
    ) -> Iterable[tuple[Path, log_pb2.LogRecord, log_pb2.LogRecord]]:
        """Compare actions output-by-output.

        Args:
          other: the other log to compare.
          eq_comparator: compare function for LogRecord.
              When this evaluates to False, report a difference.

        Yields:
          pairs of (left, right) log records with key differences.
        """
        # Find output files common to both logs.
        # Non-common files are ignored.
        left_keys = set(self.records_by_output_file.keys())
        right_keys = set(other.records_by_output_file.keys())
        common_keys = left_keys & right_keys
        visited_outputs = set()
        for k in common_keys:
            if k in visited_outputs:
                continue
            left_record = self.records_by_output_file[k]
            right_record = other.records_by_output_file[k]
            if not eq_comparator(left_record, right_record):
                yield (k, left_record, right_record)
            left_outputs = set(
                Path(p) for p in left_record.command.output.output_files
            )
            right_outputs = set(
                Path(p) for p in right_record.command.output.output_files
            )
            common_outputs = left_outputs & right_outputs
            for p in common_outputs:
                visited_outputs.add(p)

    def bandwidth_summary(self) -> tuple[int, int]:
        total_upload_bytes = 0
        total_download_bytes = 0
        for record in self.proto.records:
            if record.HasField("remote_metadata"):
                total_upload_bytes += record.remote_metadata.real_bytes_uploaded
                total_download_bytes += (
                    record.remote_metadata.real_bytes_downloaded
                )
        return total_download_bytes, total_upload_bytes

    def _min_max_event_times(self) -> tuple[float, float]:
        """Find the minimum and maximum event times in the reproxy log."""
        min_time = None
        max_time = None
        for record in self.proto.records:
            if record.HasField("remote_metadata"):
                rmd = record.remote_metadata
                for event, times in rmd.event_times.items():
                    start_time = timestamp_pb_to_float(getattr(times, "from"))
                    end_time = timestamp_pb_to_float(getattr(times, "to"))
                    min_time = (
                        start_time
                        if min_time is None
                        else min(min_time, start_time)
                    )
                    max_time = (
                        end_time
                        if max_time is None
                        else max(max_time, end_time)
                    )
        return min_time, max_time

    def _download_events(
        self, min_time: float = 0.0
    ) -> Iterable[DownloadEvent]:
        """Extract series of download events from reproxy log.

        Args:
          min_time: subtract this absolute time so resulting times
            are relative to the start (or close) of a build.

        Yields:
          DownloadEvents
        """
        for record in self.proto.records:
            if record.HasField("remote_metadata"):
                rmd = record.remote_metadata
                download_times = rmd.event_times.get("DownloadResults", None)
                if download_times is not None:
                    yield DownloadEvent(
                        bytes_downloaded=rmd.real_bytes_downloaded,
                        # 'from' is a Python keyword, so use getattr()
                        start_time=timestamp_pb_to_float(
                            getattr(download_times, "from")
                        )
                        - min_time,
                        end_time=timestamp_pb_to_float(
                            getattr(download_times, "to")
                        )
                        - min_time,
                    )

    def download_profile(
        self, interval_seconds: float
    ) -> Sequence[tuple[float, float]]:
        """Plots download bandwidth at each point in time.

        Assumes that for each DownloadResult event time entry in the
        reproxy log, that downloading used a constant average rate.

        Args:
          interval_seconds: the time granularity at which to accumulate
            download rates.

        Returns:
          time series of download bandwidth (time, bytes, duty_cycle)
        """
        min_time, max_time = self._min_max_event_times()
        if min_time is None or max_time is None:
            # no events were found
            return []

        return distribute_bytes_downloaded_over_time(
            total_time_seconds=max_time - min_time,
            interval_seconds=interval_seconds,
            # Use min_time as a proxy for the actual build start time,
            # which cannot be determined from the reproxy log alone.
            events=self._download_events(min_time=min_time),
        )


def setup_logdir_for_logdump(path: Path, verbose: bool = False) -> Path:
    """Automatically setup a log dir for logdump, if needed.

    The `logdump` tool expects a log directory, but sometimes we
    are only given a log file.  When we are given a log file, copy it
    to a directory.  The created directory is cached by the hash
    of the log file, so that it can be re-used for multiple queries.

    Args:
      path: path to either a .rrpl/.rpl log, or an reproxy log dir
        containing a .rrpl/.rpl log.

    Returns:
      The original path, or path to a newly created cached directory
      containing a copy of the log.
    """
    if path.is_dir():
        # This is a directory containing a .rrpl or .rpl file.
        return path

    # Otherwise, this is assumed to be a .rrpl or .rpl file.
    # Copy it to a cached location.
    tmpdir = Path(os.environ.get("TMPDIR", "/tmp"))

    stdin_path = Path("-")

    if path == stdin_path:
        log_contents = sys.stdin.read().encode()
    else:
        log_contents = path.read_bytes()

    readable_hash = hashlib.sha256(log_contents).hexdigest()
    cached_log_dir = tmpdir / "reproxy_logs_py" / "cache" / readable_hash
    if verbose:
        msg(f"Copying log to {cached_log_dir} for logdump processing.")
    cached_log_dir.mkdir(parents=True, exist_ok=True)

    # The 'logdump' tool expects there to be a file named 'reproxy_*.rrpl'.
    if path == stdin_path:
        (cached_log_dir / "reproxy_from_stdin.rrpl").write_bytes(log_contents)
    else:
        dest_file = str(path.name)
        if not dest_file.startswith("reproxy_"):
            dest_file = "reproxy_" + dest_file
        shutil.copy2(path, cached_log_dir / dest_file)

    return cached_log_dir


def _log_dump_from_pb(log_pb_file: Path) -> log_pb2.LogDump:
    log_dump = log_pb2.LogDump()
    with open(log_pb_file, mode="rb") as logf:
        log_dump.ParseFromString(logf.read())
    return log_dump


def convert_reproxy_actions_log(
    reproxy_logdir: Path, reclient_bindir: Path, verbose: bool = False
) -> log_pb2.LogDump:
    log_pb_file = reproxy_logdir / "reproxy_log.pb"

    if not log_pb_file.exists():
        # Convert reproxy text logs to binary proto.
        logdump_cmd = [
            str(reclient_bindir / "logdump"),
            "--proxy_log_dir",
            str(reproxy_logdir),
            "--output_dir",
            str(reproxy_logdir),  # Use same log dir, must be writeable.
        ]
        if verbose:
            msg(f"Ingesting logs from {reproxy_logdir}.")
        # logdump could take a few minutes on large reproxy logs.
        subprocess.check_call(logdump_cmd)
        # Produces "reproxy_log.pb" in args.reproxy_logdir.

    if verbose:
        msg(f"Loading log records from {log_pb_file}.")

    return _log_dump_from_pb(log_pb_file)


def parse_log(
    log_path: Path, reclient_bindir: Path, verbose: bool = False
) -> ReproxyLog:
    """Prepare and parse reproxy logs."""
    return ReproxyLog(
        convert_reproxy_actions_log(
            reproxy_logdir=setup_logdir_for_logdump(log_path, verbose=verbose),
            reclient_bindir=reclient_bindir,
            verbose=verbose,
        )
    )


def _action_digest_eq(
    left: log_pb2.LogRecord, right: log_pb2.LogRecord
) -> bool:
    return (
        left.remote_metadata.action_digest
        == right.remote_metadata.action_digest
    )


def _diff_printer(log: log_pb2.LogRecord) -> str:
    lines = [f"action_digest: {log.remote_metadata.action_digest}"]
    for file, digest in log.remote_metadata.output_file_digests.items():
        lines.extend(
            [
                f"file: {file}",
                f"  digest: {digest}",
            ]
        )
    return "\n".join(lines)


# Defined at the module level for multiprocessing to be able to serialize.
def _process_log_mp(log_path: Path) -> ReproxyLog:
    """Prepare and parse reproxy logs (for parallel processing)."""
    return parse_log(
        log_path=log_path,
        reclient_bindir=fuchsia.RECLIENT_BINDIR,
        verbose=True,
    )


def parse_logs(reproxy_logs: Sequence[Path]) -> Sequence[ReproxyLog]:
    """Parse reproxy logs, maybe in parallel."""
    try:
        with multiprocessing.Pool() as pool:
            return pool.map(_process_log_mp, reproxy_logs)
    except OSError:  # in case /dev/shm is not write-able (required)
        if len(reproxy_logs) > 1:
            msg("Warning: downloading sequentially instead of in parallel.")
        return list(map(_process_log_mp, reproxy_logs))


def diff_logs(args: argparse.Namespace) -> int:
    logs = parse_logs(args.reproxy_logs)

    # len(logs) == 2, enforced by _MAIN_ARG_PARSER.

    # TODO: argparse options to customize:
    #   different comparators
    #   different result printers
    # TODO: use a structured proto difference representation
    #   to avoid showing fields that match.
    left, right = logs
    diffs = list(left.diff_by_outputs(right, _action_digest_eq))

    if diffs:
        print("Action differences found.")
        for path, d_left, d_right in diffs:
            left_text = _diff_printer(d_left)
            right_text = _diff_printer(d_right)
            print("---------------------------------")
            print(f"{path}: (left)\n{left_text}\n")
            print(f"{path}: (right)\n{right_text}\n")

    return 0


def warmth_logs(args: argparse.Namespace) -> int:
    ref_log, test_log = parse_logs([args.reference_log, args.test_log])

    ref_actions = ref_log.records_by_action_digest
    test_actions = test_log.records_by_action_digest
    ref_digests = set(ref_actions.keys())
    print(f"The first build had {len(ref_digests)} actions.")
    test_digests = set(test_actions.keys())
    print(f"The second build had {len(test_digests)} actions.")
    # Focus on common action digests.
    common_digests = ref_digests & test_digests
    print(
        f"There are {len(common_digests)} actions in common with the first build."
    )
    newly_warmed = set()
    already_warmed = set()
    for digest in common_digests:
        ref_action = ref_actions[digest]
        test_action = test_actions[digest]
        if (
            test_action.remote_metadata.result.status
            == command_pb2.CommandResultStatus.CACHE_HIT
        ):
            if (
                ref_action.remote_metadata.result.status
                == command_pb2.CommandResultStatus.SUCCESS
            ):
                newly_warmed.add(digest)
            else:
                already_warmed.add(digest)

    total_cache_hits = len(newly_warmed) + len(already_warmed)
    print(f"Among those common actions, {total_cache_hits} were cache hits.")
    print(
        f"Cache hits warmed up directly by the first build: {len(newly_warmed)}"
    )
    print(f"Cache hits warmed up before the first build: {len(already_warmed)}")
    return 0


def lookup_output_file_digest(log: Path, path: Path) -> str | None:
    """Lookup the digest of an output file in the reproxy log.

    Args:
      log: reproxy log location
      path: path under the build output directory of a remote action output.

    Returns:
      digest: hash/size of the named output, or None if not found.
    """
    reproxy_log = parse_log(
        log_path=log,
        reclient_bindir=fuchsia.RECLIENT_BINDIR,
        verbose=False,
    )

    try:
        action_record = reproxy_log.records_by_output_file[path]
    except KeyError:
        msg(f"No record with output {path} found.")
        return None

    digest = action_record.remote_metadata.output_file_digests[str(path)]
    # lookup must succeed by construction, else would have failed above
    return digest


def lookup_output_file_digest_command(args: argparse.Namespace) -> int:
    digest = lookup_output_file_digest(log=args.log, path=args.path)

    if digest is None:
        return 1

    print(digest)
    return 0


def bandwidth_command(args: argparse.Namespace) -> int:
    log = parse_log(
        log_path=args.log,
        reclient_bindir=fuchsia.RECLIENT_BINDIR,
        verbose=False,
    )
    download_bytes, upload_bytes = log.bandwidth_summary()
    print(f"total_download_bytes = {download_bytes}")
    print(f"total_upload_bytes   = {upload_bytes}")
    return 0


def plot_download_command(args: argparse.Namespace) -> int:
    log = parse_log(
        log_path=args.log,
        reclient_bindir=fuchsia.RECLIENT_BINDIR,
        verbose=False,
    )
    profile = log.download_profile(interval_seconds=args.interval)
    # print csv to stdout
    print("time,bytes,concurrent")
    time = 0.0
    for downloaded_bytes, concurrent_downloads in profile:
        print(f"{time},{downloaded_bytes},{concurrent_downloads}")
        time += args.interval
    return 0


def filter_and_apply_records(
    records: Iterable[log_pb2.LogRecord],
    predicate: Callable[[log_pb2.LogRecord], bool],
    action: Callable[[log_pb2.LogRecord], None],
) -> None:
    for record in records:
        if predicate(record):
            action(record)


def _action_produces_rlib(record: log_pb2.LogRecord) -> bool:
    return any(f.endswith(".rlib") for f in record.command.output.output_files)


def _print_record(record: log_pb2.LogRecord) -> None:
    print(str(record) + "\n")


def filter_rlibs_command(args: argparse.Namespace) -> int:
    log = parse_log(
        log_path=args.log,
        reclient_bindir=fuchsia.RECLIENT_BINDIR,
        verbose=False,
    )
    filter_and_apply_records(
        records=log.proto.records,
        predicate=_action_produces_rlib,
        action=_print_record,
    )
    return 0


def json_export_command(args: argparse.Namespace) -> int:
    log = parse_log(
        log_path=args.log,
        reclient_bindir=fuchsia.RECLIENT_BINDIR,
        verbose=True,
    )
    records = [proto_message_to_bq_dict(record) for record in log.proto.records]
    with open(args.output, "w") as output:
        json.dump(records, output, indent=2)
    return 0


def _main_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()

    # universal options
    parser.add_argument(
        "--verbose",
        action="store_true",
        default=False,
        help="Print additional debug information while running.",
    )

    subparsers = parser.add_subparsers(required=True)

    # command: diff
    diff_parser = subparsers.add_parser("diff", help="diff --help")
    diff_parser.set_defaults(func=diff_logs)
    diff_parser.add_argument(
        "reproxy_logs",
        type=Path,
        help="reproxy logs (.rrpl or .rpl) or the directories containing them",
        metavar="PATH",
        nargs=2,
    )

    # command: warmth
    warmth_parser = subparsers.add_parser(
        "warmth",
        help="warmth reports the cache-warming effect between two builds",
    )
    warmth_parser.set_defaults(func=warmth_logs)
    warmth_parser.add_argument(
        "reference_log",
        type=Path,
        help="reproxy log from an earlier build expected to warm the cache",
        metavar="PATH",
    )
    warmth_parser.add_argument(
        "test_log",
        type=Path,
        help="reproxy log from a later build, for measure warming effect",
        metavar="PATH",
    )

    # command: output_file_digest
    output_file_digest_parser = subparsers.add_parser(
        "output_file_digest",
        help="prints the digest of a remote action output file",
    )
    output_file_digest_parser.set_defaults(
        func=lookup_output_file_digest_command
    )
    output_file_digest_parser.add_argument(
        "--log",
        type=Path,
        help="reproxy log (.rpl or .rrpl)",
        metavar="REPROXY_LOG",
        required=True,
    )
    output_file_digest_parser.add_argument(
        "--path",
        type=Path,
        help="Path under the build output directory to lookup.",
        metavar="OUTPUT_PATH",
        required=True,
    )

    # command: bandwidth
    bandwidth_parser = subparsers.add_parser(
        "bandwidth",
        help="prints total download and upload",
    )
    bandwidth_parser.set_defaults(func=bandwidth_command)
    bandwidth_parser.add_argument(
        "log",
        type=Path,
        help="reproxy log",
        metavar="PATH",
    )

    # command: plot_download
    plot_download_parser = subparsers.add_parser(
        "plot_download",
        help="plot download usage over time, prints csv to stdout",
    )
    plot_download_parser.set_defaults(func=plot_download_command)
    plot_download_parser.add_argument(
        "log",
        type=Path,
        help="reproxy log (.rpl or .rrpl)",
        metavar="PATH",
    )
    plot_download_parser.add_argument(
        "--interval",
        help="time graularity to accumulate download bandwidth",
        type=float,  # seconds
        default=5.0,
        metavar="SECONDS",
    )

    # command: filter_rlibs
    filter_rlibs_parser = subparsers.add_parser(
        "filter_rlibs",
        help="Keep log records for actions that produce rlibs, print to stdout.",
    )
    filter_rlibs_parser.set_defaults(func=filter_rlibs_command)
    filter_rlibs_parser.add_argument(
        "log",
        type=Path,
        help="reproxy log (.rpl or .rrpl)",
        metavar="PATH",
    )

    # command: json
    json_export_parser = subparsers.add_parser(
        "json_export",
        help="Export log records as json for use with tools like jq.",
    )
    json_export_parser.set_defaults(func=json_export_command)
    json_export_parser.add_argument(
        "log",
        type=Path,
        help="reproxy log (.rpl or .rrpl)",
        metavar="PATH",
    )
    json_export_parser.add_argument(
        "output", type=Path, help="json output file", metavar="OUTPUT"
    )

    return parser


_MAIN_ARG_PARSER = _main_arg_parser()


def main(argv: Sequence[str]) -> int:
    args = _MAIN_ARG_PARSER.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
