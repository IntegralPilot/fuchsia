#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import contextlib
import io
import unittest
from pathlib import Path
from unittest import mock

import action_diff
import remotetool
import reproxy_logs
from api.log import log_pb2


def add_output_file_digests_to_record(
    log_record: log_pb2.LogRecord, digests: dict[Path, str]
):
    log_record.command.output.output_files.extend(digests.keys())
    for k, v in digests.items():
        log_record.remote_metadata.output_file_digests[str(k)] = v


class ActionDifferTests(unittest.TestCase):
    def test_construction_only(self):
        left_log_dump = log_pb2.LogDump()
        right_log_dump = log_pb2.LogDump()
        left = reproxy_logs.ReproxyLog(left_log_dump)
        right = reproxy_logs.ReproxyLog(right_log_dump)
        cfg = dict()
        action_diff.ActionDiffer(left, right, cfg)

    def test_trace_output_not_in_record(self):
        left_log_dump = log_pb2.LogDump()
        right_log_dump = log_pb2.LogDump()
        left = reproxy_logs.ReproxyLog(left_log_dump)
        right = reproxy_logs.ReproxyLog(right_log_dump)
        cfg = dict()
        diff = action_diff.ActionDiffer(left, right, cfg)
        path = Path("does/not/exist.txt")
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            root_causes = list(diff.trace_artifact(path))

        check_message = "File not found"
        self.assertIn(check_message, out.getvalue())
        self.assertIn(str(path), out.getvalue())
        self.assertEqual(len(root_causes), 1)
        self.assertIn(str(path), root_causes[0].explanation[0])
        self.assertTrue(
            any(check_message in line for line in root_causes[0].explanation)
        )

    def test_trace_output_already_matches(self):
        path = Path("obj/foo.o")
        left_record = log_pb2.LogRecord()
        right_record = log_pb2.LogRecord()
        digests = {str(path): "abcdef/234"}
        add_output_file_digests_to_record(left_record, digests)
        add_output_file_digests_to_record(right_record, digests)
        left_log_dump = log_pb2.LogDump(records=[left_record])
        right_log_dump = log_pb2.LogDump(records=[right_record])
        left = reproxy_logs.ReproxyLog(left_log_dump)
        right = reproxy_logs.ReproxyLog(right_log_dump)
        cfg = dict()
        diff = action_diff.ActionDiffer(left, right, cfg)

        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            root_causes = list(diff.trace_artifact(path))

        check_message = f"Digest of {path} already matches"
        self.assertIn(check_message, out.getvalue())
        self.assertEqual(len(root_causes), 0)

    def test_trace_output_to_originating_action_with_command_diff(self):
        path = Path("obj/foo.o")
        left_record = log_pb2.LogRecord()
        left_record.remote_metadata.action_digest = "18723601ce7d/145"
        right_record = log_pb2.LogRecord()
        right_record.remote_metadata.action_digest = "73733ba65ade/145"
        left_digest = {str(path): "abcdef/234"}
        right_digest = {str(path): "987654/234"}
        add_output_file_digests_to_record(left_record, left_digest)
        add_output_file_digests_to_record(right_record, right_digest)
        left_log_dump = log_pb2.LogDump(records=[left_record])
        right_log_dump = log_pb2.LogDump(records=[right_record])
        left = reproxy_logs.ReproxyLog(left_log_dump)
        right = reproxy_logs.ReproxyLog(right_log_dump)
        cfg = dict()
        diff = action_diff.ActionDiffer(left, right, cfg)

        left_action = remotetool.ShowActionResult(
            command=["twiddle", "dum"],
            inputs=dict(),
            platform=dict(),
            output_files=left_digest,
        )
        right_action = remotetool.ShowActionResult(
            command=["twiddle", "dee"],  # different
            inputs=dict(),
            platform=dict(),
            output_files=right_digest,
        )

        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            with mock.patch.object(
                remotetool.RemoteTool,
                "show_action",
                side_effect=[left_action, right_action],
            ) as mock_show_action:
                root_causes = list(diff.trace_artifact(path))

        check_message = f"have different remote commands"
        self.assertIn(check_message, out.getvalue())
        self.assertEqual(len(root_causes), 1)
        self.assertTrue(
            any(check_message in line for line in root_causes[0].explanation)
        )

    def test_trace_output_to_action_with_platform_diff(self):
        path = Path("obj/foo.o")
        left_record = log_pb2.LogRecord()
        left_record.remote_metadata.action_digest = "18723601ce7d/145"
        right_record = log_pb2.LogRecord()
        right_record.remote_metadata.action_digest = "73733ba65ade/145"
        left_digest = {str(path): "abcdef/234"}
        right_digest = {str(path): "987654/234"}
        add_output_file_digests_to_record(left_record, left_digest)
        add_output_file_digests_to_record(right_record, right_digest)
        left_log_dump = log_pb2.LogDump(records=[left_record])
        right_log_dump = log_pb2.LogDump(records=[right_record])
        left = reproxy_logs.ReproxyLog(left_log_dump)
        right = reproxy_logs.ReproxyLog(right_log_dump)
        cfg = dict()
        diff = action_diff.ActionDiffer(left, right, cfg)

        left_action = remotetool.ShowActionResult(
            command=["twiddle"],
            inputs=dict(),
            platform={"key": "value-A"},
            output_files=left_digest,
        )
        right_action = remotetool.ShowActionResult(
            command=["twiddle"],
            inputs=dict(),
            platform={"key": "value-B"},  # different
            output_files=right_digest,
        )

        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            with mock.patch.object(
                remotetool.RemoteTool,
                "show_action",
                side_effect=[left_action, right_action],
            ) as mock_show_action:
                root_causes = list(diff.trace_artifact(path))

        check_message = f"differences in remote action platform"
        self.assertIn(check_message, out.getvalue())
        self.assertEqual(len(root_causes), 1)
        self.assertTrue(
            any(check_message in line for line in root_causes[0].explanation)
        )

    def test_trace_output_to_action_with_extra_input(self):
        path = Path("obj/foo.o")
        left_record = log_pb2.LogRecord()
        left_record.remote_metadata.action_digest = "18723601ce7d/145"
        right_record = log_pb2.LogRecord()
        right_record.remote_metadata.action_digest = "73733ba65ade/145"
        left_digest = {str(path): "abcdef/234"}
        right_digest = {str(path): "987654/234"}
        add_output_file_digests_to_record(left_record, left_digest)
        add_output_file_digests_to_record(right_record, right_digest)
        left_log_dump = log_pb2.LogDump(records=[left_record])
        right_log_dump = log_pb2.LogDump(records=[right_record])
        left = reproxy_logs.ReproxyLog(left_log_dump)
        right = reproxy_logs.ReproxyLog(right_log_dump)
        cfg = dict()
        diff = action_diff.ActionDiffer(left, right, cfg)

        left_action = remotetool.ShowActionResult(
            command=["twiddle"],
            inputs=dict(),
            platform=dict(),
            output_files=left_digest,
        )
        right_action = remotetool.ShowActionResult(
            command=["twiddle"],
            inputs={Path("include/header.h"): "bb987aaad663fd/282"},  # extra
            platform=dict(),
            output_files=right_digest,
        )

        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            with mock.patch.object(
                remotetool.RemoteTool,
                "show_action",
                side_effect=[left_action, right_action],
            ) as mock_show_action:
                root_causes = list(diff.trace_artifact(path))

        check_message = f"Input sets are not identical"
        self.assertIn(check_message, out.getvalue())
        self.assertEqual(len(root_causes), 1)
        self.assertTrue(
            any(check_message in line for line in root_causes[0].explanation)
        )

    def test_trace_output_to_action_with_different_source_input(self):
        path = Path("obj/foo.o")
        left_record = log_pb2.LogRecord()
        left_record.remote_metadata.action_digest = "18723601ce7d/145"
        left_record.command.remote_working_directory = "build/here"
        right_record = log_pb2.LogRecord()
        right_record.remote_metadata.action_digest = "73733ba65ade/145"
        right_record.command.remote_working_directory = "build/here"
        left_digest = {str(path): "abcdef/234"}
        right_digest = {str(path): "987654/234"}
        add_output_file_digests_to_record(left_record, left_digest)
        add_output_file_digests_to_record(right_record, right_digest)
        left_log_dump = log_pb2.LogDump(records=[left_record])
        right_log_dump = log_pb2.LogDump(records=[right_record])
        left = reproxy_logs.ReproxyLog(left_log_dump)
        right = reproxy_logs.ReproxyLog(right_log_dump)
        cfg = dict()
        diff = action_diff.ActionDiffer(left, right, cfg)

        left_action = remotetool.ShowActionResult(
            command=["twiddle"],
            inputs={Path("include/header.h"): "55773311bb987a/282"},
            platform=dict(),
            output_files={Path(k): v for k, v in left_digest.items()},
        )
        right_action = remotetool.ShowActionResult(
            command=["twiddle"],
            inputs={
                Path("include/header.h"): "bb987aaad663fd/282"
            },  # different
            platform=dict(),
            output_files={Path(k): v for k, v in right_digest.items()},
        )

        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            with mock.patch.object(
                remotetool.RemoteTool,
                "show_action",
                side_effect=[left_action, right_action],
            ) as mock_show_action:
                root_causes = list(diff.trace_artifact(path))

        check_message = f"does not come from a remote action"
        self.assertIn(check_message, out.getvalue())
        self.assertEqual(len(root_causes), 1)
        self.assertTrue(
            any(check_message in line for line in root_causes[0].explanation)
        )

    def test_trace_output_to_action_with_intermediate_input_not_in_record(self):
        path = Path("obj/foo.o")
        intermediate = Path("gen/include/header.h")
        remote_working_dir = Path("build/here")
        left_record = log_pb2.LogRecord()
        left_record.remote_metadata.action_digest = "18723601ce7d/145"
        left_record.command.remote_working_directory = str(remote_working_dir)
        right_record = log_pb2.LogRecord()
        right_record.remote_metadata.action_digest = "73733ba65ade/145"
        right_record.command.remote_working_directory = str(remote_working_dir)
        left_digest = {str(path): "abcdef/234"}  # is an intermediate output
        right_digest = {str(path): "987654/234"}
        add_output_file_digests_to_record(left_record, left_digest)
        add_output_file_digests_to_record(right_record, right_digest)

        left_log_dump = log_pb2.LogDump(records=[left_record])
        right_log_dump = log_pb2.LogDump(records=[right_record])
        left = reproxy_logs.ReproxyLog(left_log_dump)
        right = reproxy_logs.ReproxyLog(right_log_dump)
        cfg = dict()
        diff = action_diff.ActionDiffer(left, right, cfg)

        # actions for the outer call to trace_artifact()
        outer_left_action = remotetool.ShowActionResult(
            command=["twiddle"],
            inputs={remote_working_dir / intermediate: "55773311bb987a/282"},
            platform=dict(),
            output_files={Path(k): v for k, v in left_digest.items()},
        )
        outer_right_action = remotetool.ShowActionResult(
            command=["twiddle"],
            inputs={
                remote_working_dir / intermediate: "bb987aaad663fd/282"
            },  # different
            platform=dict(),
            output_files={Path(k): v for k, v in right_digest.items()},
        )

        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            with mock.patch.object(
                remotetool.RemoteTool,
                "show_action",
                side_effect=[
                    outer_left_action,
                    outer_right_action,
                ],
            ) as mock_show_action:
                root_causes = list(diff.trace_artifact(path))
        # outer level of trace_artifact()
        check_messages = [
            # outer level of trace_artifact()
            f"Remote output file {intermediate} differs",
            # inner level of trace_artifact() -- the root cause
            f"File not found among left log's action outputs: {intermediate}",
        ]

        for m in check_messages:
            self.assertIn(m, out.getvalue())

        self.assertEqual(len(root_causes), 1)
        self.assertIn(str(intermediate), root_causes[0].explanation[0])
        self.assertTrue(
            any(
                check_messages[1] in line for line in root_causes[0].explanation
            )
        )

    def test_trace_output_to_action_with_different_intermediate_input(self):
        path = Path("obj/foo.o")
        intermediate = Path("gen/include/header.h")
        source = Path("lib/api.h")
        remote_working_dir = Path("build/here")
        left_record = log_pb2.LogRecord()
        left_record.remote_metadata.action_digest = "18723601ce7d/145"
        left_record.command.remote_working_directory = str(remote_working_dir)
        right_record = log_pb2.LogRecord()
        right_record.remote_metadata.action_digest = "73733ba65ade/145"
        right_record.command.remote_working_directory = str(remote_working_dir)
        left_digest = {str(path): "abcdef/234"}  # is an intermediate output
        right_digest = {str(path): "987654/234"}
        add_output_file_digests_to_record(left_record, left_digest)
        add_output_file_digests_to_record(right_record, right_digest)

        inner_left_record = log_pb2.LogRecord()
        inner_left_record.remote_metadata.action_digest = "22233342341/45"
        inner_left_record.command.remote_working_directory = str(
            remote_working_dir
        )
        inner_right_record = log_pb2.LogRecord()
        inner_right_record.remote_metadata.action_digest = "777889112ef/45"
        inner_right_record.command.remote_working_directory = str(
            remote_working_dir
        )
        inner_left_digest = {
            str(intermediate): "00abcdef/34"
        }  # is an intermediate output
        inner_right_digest = {str(intermediate): "00987654/34"}
        add_output_file_digests_to_record(inner_left_record, inner_left_digest)
        add_output_file_digests_to_record(
            inner_right_record, inner_right_digest
        )

        left_log_dump = log_pb2.LogDump(
            records=[left_record, inner_left_record]
        )
        right_log_dump = log_pb2.LogDump(
            records=[inner_right_record, right_record]
        )
        left = reproxy_logs.ReproxyLog(left_log_dump)
        right = reproxy_logs.ReproxyLog(right_log_dump)
        cfg = dict()
        diff = action_diff.ActionDiffer(left, right, cfg)

        # actions for the outer call to trace_artifact()
        outer_left_action = remotetool.ShowActionResult(
            command=["twiddle"],
            inputs={remote_working_dir / intermediate: "55773311bb987a/282"},
            platform=dict(),
            output_files={Path(k): v for k, v in left_digest.items()},
        )
        outer_right_action = remotetool.ShowActionResult(
            command=["twiddle"],
            inputs={
                remote_working_dir / intermediate: "bb987aaad663fd/282"
            },  # different
            platform=dict(),
            output_files={Path(k): v for k, v in right_digest.items()},
        )

        # actions for the inner call to trace_artifact()
        inner_left_action = remotetool.ShowActionResult(
            command=["fiddle"],
            inputs={source: "aa81557311b987a/189"},
            platform=dict(),
            output_files={Path(k): v for k, v in inner_left_digest.items()},
        )
        inner_right_action = remotetool.ShowActionResult(
            command=["fiddle"],
            inputs={source: "87be001aa4efbfd/182"},  # different
            platform=dict(),
            output_files={Path(k): v for k, v in inner_right_digest.items()},
        )
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            with mock.patch.object(
                remotetool.RemoteTool,
                "show_action",
                side_effect=[
                    outer_left_action,
                    outer_right_action,
                    inner_left_action,
                    inner_right_action,
                ],
            ) as mock_show_action:
                root_causes = list(diff.trace_artifact(path))

        check_messages = [
            # outer level of trace_artifact()
            f"Remote output file {intermediate} differs",
            # inner level of trace_artifact() -- root cause
            f"Input {source} does not come from a remote action",
        ]

        for m in check_messages:
            self.assertIn(m, out.getvalue())

        self.assertEqual(len(root_causes), 1)
        self.assertTrue(
            any(
                check_messages[1] in line for line in root_causes[0].explanation
            )
        )

    def test_trace_actions_missing_digest_from_record(self):
        left_action_digest = "44182360ce7d/76"
        right_action_digest = "217733ba6ade/76"
        left_record = log_pb2.LogRecord()
        left_record.remote_metadata.action_digest = left_action_digest
        right_record = log_pb2.LogRecord()
        right_record.remote_metadata.action_digest = right_action_digest
        left_log_dump = log_pb2.LogDump()  # omit record
        right_log_dump = log_pb2.LogDump(records=[right_record])
        left = reproxy_logs.ReproxyLog(left_log_dump)
        right = reproxy_logs.ReproxyLog(right_log_dump)
        cfg = dict()
        diff = action_diff.ActionDiffer(left, right, cfg)

        left_action = remotetool.ShowActionResult(
            command=["twiddle", "dum"],
            inputs=dict(),
            platform=dict(),
            output_files=dict(),
        )
        right_action = remotetool.ShowActionResult(
            command=["twiddle", "dee"],  # different
            inputs=dict(),
            platform=dict(),
            output_files=dict(),
        )

        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            root_causes = list(
                diff.trace_actions(left_action_digest, right_action_digest)
            )

        check_message = f"Unable to find log record"
        self.assertIn(check_message, out.getvalue())
        self.assertEqual(len(root_causes), 0)

    def test_trace_actions_with_command_diff(self):
        left_action_digest = "44182360ce7d/76"
        right_action_digest = "217733ba6ade/76"
        left_record = log_pb2.LogRecord()
        left_record.remote_metadata.action_digest = left_action_digest
        right_record = log_pb2.LogRecord()
        right_record.remote_metadata.action_digest = right_action_digest
        left_log_dump = log_pb2.LogDump(records=[left_record])
        right_log_dump = log_pb2.LogDump(records=[right_record])
        left = reproxy_logs.ReproxyLog(left_log_dump)
        right = reproxy_logs.ReproxyLog(right_log_dump)
        cfg = dict()
        diff = action_diff.ActionDiffer(left, right, cfg)

        left_action = remotetool.ShowActionResult(
            command=["twiddle", "dum"],
            inputs=dict(),
            platform=dict(),
            output_files=dict(),
        )
        right_action = remotetool.ShowActionResult(
            command=["twiddle", "dee"],  # different
            inputs=dict(),
            platform=dict(),
            output_files=dict(),
        )

        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            with mock.patch.object(
                remotetool.RemoteTool,
                "show_action",
                side_effect=[left_action, right_action],
            ) as mock_show_action:
                root_causes = list(
                    diff.trace_actions(left_action_digest, right_action_digest)
                )

        check_message = f"have different remote commands"
        self.assertIn(check_message, out.getvalue())
        self.assertEqual(len(root_causes), 1)
        self.assertTrue(
            any(check_message in line for line in root_causes[0].explanation)
        )

    def test_trace_actions_with_different_intermediate_input(self):
        left_action_digest = "18723601ce7d/145"
        right_action_digest = "73733ba65ade/145"
        intermediate = Path("gen/include/header.h")
        source = Path("lib/api.h")
        remote_working_dir = Path("build/here")
        left_record = log_pb2.LogRecord()
        left_record.remote_metadata.action_digest = left_action_digest
        left_record.command.remote_working_directory = str(remote_working_dir)
        right_record = log_pb2.LogRecord()
        right_record.remote_metadata.action_digest = right_action_digest
        right_record.command.remote_working_directory = str(remote_working_dir)

        inner_left_record = log_pb2.LogRecord()
        inner_left_record.remote_metadata.action_digest = "22233342341/45"
        inner_left_record.command.remote_working_directory = str(
            remote_working_dir
        )
        inner_right_record = log_pb2.LogRecord()
        inner_right_record.remote_metadata.action_digest = "777889112ef/45"
        inner_right_record.command.remote_working_directory = str(
            remote_working_dir
        )
        inner_left_digest = {
            str(intermediate): "00abcdef/34"
        }  # is an intermediate output
        inner_right_digest = {str(intermediate): "00987654/34"}
        add_output_file_digests_to_record(inner_left_record, inner_left_digest)
        add_output_file_digests_to_record(
            inner_right_record, inner_right_digest
        )

        left_log_dump = log_pb2.LogDump(
            records=[left_record, inner_left_record]
        )
        right_log_dump = log_pb2.LogDump(
            records=[inner_right_record, right_record]
        )
        left = reproxy_logs.ReproxyLog(left_log_dump)
        right = reproxy_logs.ReproxyLog(right_log_dump)
        cfg = dict()
        diff = action_diff.ActionDiffer(left, right, cfg)

        # actions for the outer call to trace_artifact()
        outer_left_action = remotetool.ShowActionResult(
            command=["twiddle"],
            inputs={remote_working_dir / intermediate: "55773311bb987a/282"},
            platform=dict(),
            output_files=dict(),
        )
        outer_right_action = remotetool.ShowActionResult(
            command=["twiddle"],
            inputs={
                remote_working_dir / intermediate: "bb987aaad663fd/282"
            },  # different
            platform=dict(),
            output_files=dict(),
        )

        # actions for the inner call to trace_artifact()
        inner_left_action = remotetool.ShowActionResult(
            command=["fiddle"],
            inputs={source: "aa81557311b987a/189"},
            platform=dict(),
            output_files=dict(),
        )
        inner_right_action = remotetool.ShowActionResult(
            command=["fiddle"],
            inputs={source: "87be001aa4efbfd/182"},  # different
            platform=dict(),
            output_files=dict(),
        )
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            with mock.patch.object(
                remotetool.RemoteTool,
                "show_action",
                side_effect=[
                    outer_left_action,
                    outer_right_action,
                    inner_left_action,
                    inner_right_action,
                ],
            ) as mock_show_action:
                root_causes = list(
                    diff.trace_actions(left_action_digest, right_action_digest)
                )

        check_messages = [
            # outer level of trace_artifact()
            f"Remote output file {intermediate} differs",
            # inner level of trace_artifact() -- root cause
            f"Input {source} does not come from a remote action",
        ]

        for m in check_messages:
            self.assertIn(m, out.getvalue())

        self.assertEqual(len(root_causes), 1)
        self.assertTrue(
            any(
                check_messages[1] in line for line in root_causes[0].explanation
            )
        )


if __name__ == "__main__":
    unittest.main()
