#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import contextlib
import io
import json
import shutil
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path
from typing import Any
from unittest import mock

import reproxy_logs
from api.log import log_pb2
from go.api.command import command_pb2
from google.protobuf import timestamp_pb2


def _write_file_contents(path: Path, contents: str) -> None:
    with open(path, "w") as f:
        f.write(contents)


def _digests_equal(left: log_pb2.LogRecord, right: log_pb2.LogRecord) -> bool:
    return (
        left.remote_metadata.action_digest
        == right.remote_metadata.action_digest
    )


class DownloadEventTests(unittest.TestCase):
    def test_distribute_fits_in_first_interval(self) -> None:
        event = reproxy_logs.DownloadEvent(
            bytes_downloaded=1000,
            start_time=5.0,
            end_time=6.0,
        )
        intervals = list(event.distribute_over_intervals(interval_seconds=10.0))
        for (actual_index, actual_bytes, actual_concurrent), (
            expected_index,
            expected_bytes,
            expected_concurrent,
        ) in zip(intervals, [(0, 1000.0, 0.1)]):
            self.assertEqual(actual_index, expected_index)
            self.assertAlmostEqual(actual_bytes, expected_bytes)
            self.assertAlmostEqual(actual_concurrent, expected_concurrent)

    def test_distribute_fits_in_one_interval(self) -> None:
        event = reproxy_logs.DownloadEvent(
            bytes_downloaded=1000,
            start_time=34.0,
            end_time=37.0,
        )
        intervals = list(event.distribute_over_intervals(interval_seconds=10.0))
        for (actual_index, actual_bytes, actual_concurrent), (
            expected_index,
            expected_bytes,
            expected_concurrent,
        ) in zip(intervals, [(3, 1000.0, 0.3)]):
            self.assertEqual(actual_index, expected_index)
            self.assertAlmostEqual(actual_bytes, expected_bytes)
            self.assertAlmostEqual(actual_concurrent, expected_concurrent)

    def test_distribute_spans_two_intervals(self) -> None:
        event = reproxy_logs.DownloadEvent(
            bytes_downloaded=1000,
            start_time=24.0,
            end_time=34.0,
        )
        intervals = list(event.distribute_over_intervals(interval_seconds=10.0))
        for (actual_index, actual_bytes, actual_concurrent), (
            expected_index,
            expected_bytes,
            expected_concurrent,
        ) in zip(
            intervals,
            [
                (2, 600.0, 0.6),
                (3, 400.0, 0.4),
            ],
        ):
            self.assertEqual(actual_index, expected_index)
            self.assertAlmostEqual(actual_bytes, expected_bytes)
            self.assertAlmostEqual(actual_concurrent, expected_concurrent)

    def test_distribute_spans_one_exact_interval(self) -> None:
        event = reproxy_logs.DownloadEvent(
            bytes_downloaded=500,
            start_time=20.0,
            end_time=40.0,
        )
        intervals = list(event.distribute_over_intervals(interval_seconds=20.0))
        for (actual_index, actual_bytes, actual_concurrent), (
            expected_index,
            expected_bytes,
            expected_concurrent,
        ) in zip(
            intervals,
            [
                (1, 500.0, 1.0),
            ],
        ):
            self.assertEqual(actual_index, expected_index)
            self.assertAlmostEqual(actual_bytes, expected_bytes)
            self.assertAlmostEqual(actual_concurrent, expected_concurrent)

    def test_distribute_spans_three_intervals(self) -> None:
        event = reproxy_logs.DownloadEvent(
            bytes_downloaded=1600,
            start_time=28.0,
            end_time=44.0,
        )
        intervals = list(event.distribute_over_intervals(interval_seconds=10.0))
        for (actual_index, actual_bytes, actual_concurrent), (
            expected_index,
            expected_bytes,
            expected_concurrent,
        ) in zip(
            intervals,
            [
                (2, 200.0, 0.2),
                (3, 1000.0, 1.0),
                (4, 400.0, 0.4),
            ],
        ):
            self.assertEqual(actual_index, expected_index)
            self.assertAlmostEqual(actual_bytes, expected_bytes)
            self.assertAlmostEqual(actual_concurrent, expected_concurrent)


class DistributeDownloadEventBytesOverTimeTests(unittest.TestCase):
    def test_empty(self) -> None:
        intervals = reproxy_logs.distribute_bytes_downloaded_over_time(
            total_time_seconds=3.0,
            interval_seconds=1.0,
            events=[],
        )
        for (
            (actual_bytes, actual_concurrent),
            expected_bytes,
            expected_concurrent,
        ) in zip(intervals, [0.0] * 4, [0.0] * 4):
            self.assertAlmostEqual(actual_bytes, expected_bytes)
            self.assertAlmostEqual(actual_concurrent, expected_concurrent)

    def test_two_events(self) -> None:
        events = [
            reproxy_logs.DownloadEvent(
                bytes_downloaded=100,
                start_time=2.0,
                end_time=12.0,
            ),
            reproxy_logs.DownloadEvent(
                bytes_downloaded=300,
                start_time=12.0,
                end_time=18.0,
            ),
        ]
        intervals = reproxy_logs.distribute_bytes_downloaded_over_time(
            total_time_seconds=20.0,
            interval_seconds=5.0,
            events=events,
        )
        # bytes[0]: (0, 30), (1, 50), (2, 20)
        # bytes[1]:                   (2, 150), (3, 150)
        # concurrent[0]: (0, 0.6), (1, 1.0), (2, 0.4)
        # concurrent[1]:                     (2, 0.6), (3, 0.6)
        for (
            (actual_bytes, actual_concurrent),
            expected_bytes,
            expected_concurrent,
        ) in zip(
            intervals,
            [30.0, 50.0, 170.0, 150.0, 0.0],
            [0.6, 1.0, 1.0, 0.6, 0.0],
        ):
            self.assertAlmostEqual(actual_bytes, expected_bytes)
            self.assertAlmostEqual(actual_concurrent, expected_concurrent)


class ReproxyLogTests(unittest.TestCase):
    def test_construction(self) -> None:
        record1 = log_pb2.LogRecord()
        record1.command.output.output_files.extend(["obj/foo.o", "obj/foo2.o"])
        record1.remote_metadata.action_digest = "ffff0000/13"
        record2 = log_pb2.LogRecord()
        record2.command.output.output_files.extend(["obj/bar.o"])
        record2.remote_metadata.action_digest = "bbbbaaaa/9"
        log_dump = log_pb2.LogDump(records=[record1, record2])
        log = reproxy_logs.ReproxyLog(log_dump)
        self.assertEqual(log.proto, log_dump)
        self.assertEqual(
            log.records_by_output_file,
            {
                Path("obj/foo.o"): record1,
                Path("obj/foo2.o"): record1,
                Path("obj/bar.o"): record2,
            },
        )
        self.assertEqual(
            log.records_by_action_digest,
            {
                "ffff0000/13": record1,
                "bbbbaaaa/9": record2,
            },
        )

    def test_diff_by_outputs_matching(self) -> None:
        record1 = log_pb2.LogRecord()
        record1.command.output.output_files.extend(["obj/foo.o"])
        record1.remote_metadata.action_digest = "ffeeff001100/313"
        log_dump = log_pb2.LogDump(records=[record1])
        log = reproxy_logs.ReproxyLog(log_dump)
        diffs = list(log.diff_by_outputs(log, _digests_equal))
        self.assertEqual(diffs, [])

    def test_diff_by_outputs_different(self) -> None:
        output = Path("obj/foo")
        record1 = log_pb2.LogRecord()
        record1.command.output.output_files.extend([str(output)])
        record1.remote_metadata.action_digest = "ffeeff001100/313"
        record2 = log_pb2.LogRecord()
        record2.command.output.output_files.extend([str(output)])
        record2.remote_metadata.action_digest = "98231772bbbd/313"
        log_dump1 = log_pb2.LogDump(records=[record1])
        log_dump2 = log_pb2.LogDump(records=[record2])
        log1 = reproxy_logs.ReproxyLog(log_dump1)
        log2 = reproxy_logs.ReproxyLog(log_dump2)
        diffs = list(log1.diff_by_outputs(log2, _digests_equal))
        self.assertEqual(diffs, [(output, record1, record2)])

    def test_bandwidth_no_remote_metadata(self) -> None:
        record = log_pb2.LogRecord()
        # no remote metadata
        log_dump = log_pb2.LogDump(records=[record])
        log = reproxy_logs.ReproxyLog(log_dump)
        download_bytes, upload_bytes = log.bandwidth_summary()
        self.assertEqual(download_bytes, 0)
        self.assertEqual(upload_bytes, 0)

    def test_bandwidth_no_upload_download(self) -> None:
        record = log_pb2.LogRecord()
        record.remote_metadata.action_digest = "ccddcdcdcdd/13"
        log_dump = log_pb2.LogDump(records=[record])
        log = reproxy_logs.ReproxyLog(log_dump)
        download_bytes, upload_bytes = log.bandwidth_summary()
        self.assertEqual(download_bytes, 0)
        self.assertEqual(upload_bytes, 0)

    def test_bandwidth_nonzero(self) -> None:
        record1 = log_pb2.LogRecord()
        record1.remote_metadata.real_bytes_downloaded = 100
        record1.remote_metadata.real_bytes_uploaded = 10
        record2 = log_pb2.LogRecord()
        record2.remote_metadata.real_bytes_downloaded = 20
        record2.remote_metadata.real_bytes_uploaded = 5
        log_dump = log_pb2.LogDump(records=[record1, record2])
        log = reproxy_logs.ReproxyLog(log_dump)
        download_bytes, upload_bytes = log.bandwidth_summary()
        self.assertEqual(download_bytes, 120)
        self.assertEqual(upload_bytes, 15)

    def test_download_profile_no_events(self) -> None:
        log_dump = log_pb2.LogDump()
        log = reproxy_logs.ReproxyLog(log_dump)
        data = log.download_profile(interval_seconds=5.0)
        self.assertEqual(data, [])

    def test_download_profile_one_event(self) -> None:
        record = log_pb2.LogRecord()
        record.remote_metadata.real_bytes_downloaded = 100
        # kwargs dict to workaround 'from' being a Python keyword
        interval_kwargs = {
            "from": timestamp_pb2.Timestamp(seconds=55, nanos=0),
            "to": timestamp_pb2.Timestamp(seconds=65, nanos=0),
        }
        record.remote_metadata.event_times["DownloadResults"].MergeFrom(
            command_pb2.TimeInterval(**interval_kwargs)
        )
        log_dump = log_pb2.LogDump(records=[record])
        log = reproxy_logs.ReproxyLog(log_dump)
        data = log.download_profile(interval_seconds=5.0)
        for (
            (actual_bytes, actual_concurrent),
            expected_bytes,
            expected_concurrent,
        ) in zip(data, [50.0, 50.0, 0.0], [1.0, 1.0, 0.0]):
            self.assertAlmostEqual(actual_bytes, expected_bytes)
            self.assertAlmostEqual(actual_concurrent, expected_concurrent)


class SetupLogdirForLogDumpTest(unittest.TestCase):
    def test_already_a_directory(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            reproxy_logs.setup_logdir_for_logdump(Path(td))

    def test_log_file_copied_to_new_dir_missing_reproxy_prefix(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            log_file = Path(td) / "test.rrpl"
            log_contents = "fake log contents, not checked"
            _write_file_contents(log_file, log_contents)
            with mock.patch.object(Path, "mkdir") as mock_mkdir:
                with mock.patch.object(shutil, "copy2") as mock_copy:
                    new_log_dir = reproxy_logs.setup_logdir_for_logdump(
                        log_file
                    )
            mock_mkdir.assert_called_with(parents=True, exist_ok=True)
            mock_copy.assert_called_with(
                log_file, new_log_dir / "reproxy_test.rrpl"
            )

    def test_log_file_copied_to_new_dir_with_reproxy_prefix(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            log_base = "reproxy_foo.rrpl"
            log_file = Path(td) / log_base
            log_contents = "fake log contents, not checked"
            _write_file_contents(log_file, log_contents)
            with mock.patch.object(Path, "mkdir") as mock_mkdir:
                with mock.patch.object(shutil, "copy2") as mock_copy:
                    new_log_dir = reproxy_logs.setup_logdir_for_logdump(
                        log_file
                    )
            mock_mkdir.assert_called_with(parents=True, exist_ok=True)
            mock_copy.assert_called_with(log_file, new_log_dir / log_base)


class ConvertReproxyActionsLogTest(unittest.TestCase):
    def test_basic(self) -> None:
        empty_log = log_pb2.LogDump()
        with mock.patch.object(subprocess, "check_call") as mock_process_call:
            with mock.patch.object(
                reproxy_logs, "_log_dump_from_pb", return_value=empty_log
            ) as mock_parse:
                log_dump = reproxy_logs.convert_reproxy_actions_log(
                    reproxy_logdir=Path("/tmp/reproxy.log.dir"),
                    reclient_bindir=Path("/usr/local/reclient/bin"),
                )
        mock_process_call.assert_called_once()
        mock_parse.assert_called_once()
        self.assertEqual(log_dump, log_dump)


class DiffLogsTests(unittest.TestCase):
    def test_flow(self) -> None:
        empty_log = log_pb2.LogDump()
        empty_logs = [reproxy_logs.ReproxyLog(empty_log)] * 2
        args = argparse.Namespace(
            reproxy_logs=[Path("log1"), Path("log2")],
        )
        with mock.patch.object(
            reproxy_logs, "parse_logs", return_value=empty_logs
        ) as mock_multi_parse:
            status = reproxy_logs.diff_logs(args)
        self.assertEqual(status, 0)


class WarmthLogsTests(unittest.TestCase):
    def test_logs_comparison(self) -> None:
        record1 = log_pb2.LogRecord()
        record1.remote_metadata.action_digest = "aaaa/44"
        record1.remote_metadata.result.status = (
            command_pb2.CommandResultStatus.SUCCESS
        )

        record2 = log_pb2.LogRecord()
        record2.remote_metadata.action_digest = (
            record1.remote_metadata.action_digest
        )
        record2.remote_metadata.result.status = (
            command_pb2.CommandResultStatus.CACHE_HIT
        )

        record3 = log_pb2.LogRecord()
        record3.remote_metadata.action_digest = "bbbb/55"
        record3.remote_metadata.result.status = (
            command_pb2.CommandResultStatus.CACHE_HIT
        )

        record4 = log_pb2.LogRecord()
        record4.remote_metadata.action_digest = "cccc/66"
        record4.remote_metadata.result.status = (
            command_pb2.CommandResultStatus.CACHE_HIT
        )

        record5 = log_pb2.LogRecord()
        record5.remote_metadata.action_digest = "dddd/77"
        record5.remote_metadata.result.status = (
            command_pb2.CommandResultStatus.CACHE_HIT
        )

        ref_actions = reproxy_logs.ReproxyLog(
            log_pb2.LogDump(records=[record1, record3, record4])
        )
        test_actions = reproxy_logs.ReproxyLog(
            log_pb2.LogDump(
                records=[
                    record2,
                    record3,
                    record5,
                ]
            )
        )
        with mock.patch.object(
            reproxy_logs, "parse_logs", return_value=(ref_actions, test_actions)
        ) as mock_parse:
            result = io.StringIO()
            with contextlib.redirect_stdout(result):
                status = reproxy_logs.warmth_logs(
                    args=argparse.Namespace(
                        reference_log=Path("first.rrpl"),
                        test_log=Path("second.rrpl"),
                    )
                )
                self.assertEqual(status, 0)
                self.assertEqual(
                    result.getvalue(),
                    textwrap.dedent(
                        """\
The first build had 3 actions.
The second build had 3 actions.
There are 2 actions in common with the first build.
Among those common actions, 2 were cache hits.
Cache hits warmed up directly by the first build: 1
Cache hits warmed up before the first build: 1
                                                                  """
                    ),
                )


class LookupOutputFileDigestTests(unittest.TestCase):
    def test_output_path_not_found(self) -> None:
        empty_log = log_pb2.LogDump()
        log = reproxy_logs.ReproxyLog(empty_log)
        args = argparse.Namespace(
            log=Path("foo.rrpl"),
            path=Path("output/does/not/exist.o"),
        )
        with mock.patch.object(
            reproxy_logs, "parse_log", return_value=log
        ) as mock_parse:
            self.assertEqual(
                reproxy_logs.lookup_output_file_digest_command(args), 1
            )

    def test_output_path_found(self) -> None:
        record = log_pb2.LogRecord()
        path1 = "obj/aaa.o"
        path2 = "obj/bbb.o"
        digest1 = "ababababa/111"
        digest2 = "efefefefe/222"
        file_digests = [(path1, digest1), (path2, digest2)]
        for path, digest in file_digests:
            record.command.output.output_files.append(path)
            record.remote_metadata.output_file_digests[path] = digest

        log_dump = log_pb2.LogDump(records=[record])
        log = reproxy_logs.ReproxyLog(log_dump)

        for path, digest in file_digests:
            args = argparse.Namespace(
                log=Path("foo.rrpl"),
                path=Path(path),
            )
            with mock.patch.object(
                reproxy_logs, "parse_log", return_value=log
            ) as mock_parse:
                result = io.StringIO()
                with contextlib.redirect_stdout(result):
                    self.assertEqual(
                        reproxy_logs.lookup_output_file_digest_command(args), 0
                    )
                self.assertEqual(result.getvalue(), digest + "\n")


class FilterRecordsTests(unittest.TestCase):
    def test_basic_counter(self) -> None:
        counter = 0

        def action(unused: Any) -> None:
            nonlocal counter  # reference the one in the above scope
            counter += 1

        record = log_pb2.LogRecord()
        reproxy_logs.filter_and_apply_records(
            records=[record, record],
            predicate=lambda record: True,
            action=action,
        )
        self.assertEqual(counter, 2)

    def test_basic_no_matches(self) -> None:
        counter = 0

        def action(unused: Any) -> None:
            nonlocal counter  # reference the one in the above scope
            counter += 1

        record = log_pb2.LogRecord()
        reproxy_logs.filter_and_apply_records(
            records=[record, record],
            predicate=lambda record: False,
            action=action,
        )
        self.assertEqual(counter, 0)


class FilterRlibsCommandTest(unittest.TestCase):
    def test_filter(self) -> None:
        record1 = log_pb2.LogRecord()
        record2 = log_pb2.LogRecord()
        record1.command.output.output_files.extend(["foo.rlib"])  # match this
        record2.command.output.output_files.extend(["bar.cc.o"])  # but not this
        logdump = log_pb2.LogDump(records=[record1, record2])
        with mock.patch.object(
            reproxy_logs,
            "parse_log",
            return_value=reproxy_logs.ReproxyLog(logdump),
        ) as mock_parse:
            printed_text = io.StringIO()
            with contextlib.redirect_stdout(printed_text):
                exit_code = reproxy_logs.filter_rlibs_command(
                    argparse.Namespace(log=Path("x.rrpl"))
                )

        self.assertEqual(exit_code, 0)
        mock_parse.assert_called_once()
        self.assertEqual(printed_text.getvalue(), str(record1) + "\n\n")


class MainTests(unittest.TestCase):
    def test_diff(self) -> None:
        empty_log = log_pb2.LogDump()
        empty_logs = [reproxy_logs.ReproxyLog(empty_log)] * 2
        with mock.patch.object(
            reproxy_logs, "parse_logs", return_value=empty_logs
        ) as mock_multi_parse:
            status = reproxy_logs.main(["diff", "left.rrpl", "right.rrpl"])

        self.assertEqual(status, 0)
        mock_multi_parse.assert_called()

    def test_output_file_digest(self) -> None:
        record = log_pb2.LogRecord()
        path1 = "obj/aaa.o"
        digest1 = "ababababa/111"
        record.command.output.output_files.append(path1)
        record.remote_metadata.output_file_digests[path1] = digest1

        log_dump = log_pb2.LogDump(records=[record])
        log = reproxy_logs.ReproxyLog(log_dump)

        with mock.patch.object(
            reproxy_logs, "parse_log", return_value=log
        ) as mock_multi_parse:
            status = reproxy_logs.main(
                ["output_file_digest", "--log", "log.rrpl", "--path", path1]
            )

        self.assertEqual(status, 0)
        mock_multi_parse.assert_called_once()

    def test_bandwidth(self) -> None:
        up = 33
        down = 999
        record = log_pb2.LogRecord()
        record.remote_metadata.real_bytes_uploaded = up
        record.remote_metadata.real_bytes_downloaded = down

        log_dump = log_pb2.LogDump(records=[record])
        log = reproxy_logs.ReproxyLog(log_dump)

        result = io.StringIO()
        with mock.patch.object(
            reproxy_logs, "parse_log", return_value=log
        ) as mock_multi_parse:
            with contextlib.redirect_stdout(result):
                status = reproxy_logs.main(["bandwidth", "log.rrpl"])

        self.assertEqual(
            result.getvalue(),
            f"total_download_bytes = {down}\ntotal_upload_bytes   = {up}\n",
        )
        self.assertEqual(status, 0)
        mock_multi_parse.assert_called_once()

    def test_plot_download(self) -> None:
        record = log_pb2.LogRecord()
        record.remote_metadata.real_bytes_downloaded = 1000000
        # kwargs dict to workaround 'from' being a Python keyword
        interval_kwargs = {
            "from": timestamp_pb2.Timestamp(seconds=20, nanos=0),
            "to": timestamp_pb2.Timestamp(seconds=40, nanos=0),
        }
        record.remote_metadata.event_times["DownloadResults"].MergeFrom(
            command_pb2.TimeInterval(**interval_kwargs)
        )
        log_dump = log_pb2.LogDump(records=[record])
        log = reproxy_logs.ReproxyLog(log_dump)

        result = io.StringIO()
        with mock.patch.object(
            reproxy_logs, "parse_log", return_value=log
        ) as mock_multi_parse:
            with contextlib.redirect_stdout(result):
                status = reproxy_logs.main(
                    ["plot_download", "--interval", "10", "log.rrpl"]
                )

        self.assertEqual(
            result.getvalue(),
            "\n".join(
                [
                    "time,bytes,concurrent",
                    "0.0,500000.0,1.0",
                    "10.0,500000.0,1.0",
                    "20.0,0.0,0.0",
                ]
            )
            + "\n",
        )
        self.assertEqual(status, 0)
        mock_multi_parse.assert_called_once()

    def test_filter_rlibs(self) -> None:
        log = reproxy_logs.ReproxyLog(log_pb2.LogDump())
        with mock.patch.object(
            reproxy_logs, "parse_log", return_value=log
        ) as mock_parse:
            with mock.patch.object(
                reproxy_logs, "filter_and_apply_records"
            ) as mock_filter:
                status = reproxy_logs.main(["filter_rlibs", "log.rrpl"])
        self.assertEqual(status, 0)
        mock_parse.assert_called_once()
        mock_filter.assert_called_once()

    def test_json_export(self) -> None:
        record = log_pb2.LogRecord()
        log = reproxy_logs.ReproxyLog(log_pb2.LogDump(records=[record]))
        with mock.patch.object(
            reproxy_logs, "parse_log", return_value=log
        ) as mock_parse:
            with tempfile.TemporaryDirectory() as td:
                outfile_path = Path(td) / "output.json"
                status = reproxy_logs.main(
                    ["json_export", "log.rrpl", str(outfile_path)]
                )
                self.assertEqual(status, 0)
                with open(outfile_path) as outfile:
                    output_json = json.load(outfile)
        mock_parse.assert_called_once()
        self.assertEqual(output_json, [{}])


if __name__ == "__main__":
    unittest.main()
