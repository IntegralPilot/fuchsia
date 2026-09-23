#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import contextlib
import io
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import remotetool
import reproxy_logs
import rpl_tool
from api.log import log_pb2


class NormalizeInputPathPrefixTests(unittest.TestCase):
    def test_no_working_dirs_no_change(self):
        p = Path("out/build-1/foo/bar.txt")
        new_p = rpl_tool.normalize_input_path_prefix(p, [])
        self.assertEqual(new_p, str(p))


_reproxy_cfg = {
    "service": "some.remote.service.com:443",
    "instance": "projects/my-project/instance/default",
}

_FAKE_REMOTETOOL = remotetool.RemoteTool(reproxy_cfg=_reproxy_cfg)


class InferRecordCommandAndInputsTests(unittest.TestCase):
    def test_one_record(self):
        command = ["cat", "hello.txt"]
        inputs = {Path("hello.txt"): "00de7abc8771/7"}
        action_digest = "90123eeecbd19898/147"
        record = log_pb2.LogRecord()
        record.remote_metadata.action_digest = action_digest
        record.command.working_directory = "."
        record.command.remote_working_directory = "."
        record_original = log_pb2.LogRecord()
        record_original.CopyFrom(record)
        with mock.patch.object(
            remotetool.RemoteTool,
            "show_action",
            return_value=remotetool.ShowActionResult(
                command=command, inputs=inputs, output_files={}, platform={}
            ),
        ) as mock_show_action:
            rpl_tool.infer_record_command_and_inputs(record, _FAKE_REMOTETOOL)

        mock_show_action.assert_called_once_with(action_digest)
        self.assertEqual(record.command.args, command)
        self.assertEqual(
            record.command.input.inputs, [str(k) for k in inputs.keys()]
        )
        # Make sure all other fields are the same as before
        record_original.command.args.extend(record.command.args)
        record_original.command.input.inputs.extend(record.command.input.inputs)
        self.assertEqual(record, record_original)

    def test_one_record_input_in_working_dir(self):
        working_dir = "out/construction/site"
        input = "w00f.txt"
        command = ["cat", input]
        inputs = {Path(working_dir, input): "991919c8e71/6"}
        action_digest = "d19898a009bd80001/147"
        record = log_pb2.LogRecord()
        record.remote_metadata.action_digest = action_digest
        record.command.working_directory = working_dir
        record_original = log_pb2.LogRecord()
        record_original.CopyFrom(record)
        with mock.patch.object(
            remotetool.RemoteTool,
            "show_action",
            return_value=remotetool.ShowActionResult(
                command=command, inputs=inputs, output_files={}, platform={}
            ),
        ) as mock_show_action:
            rpl_tool.infer_record_command_and_inputs(record, _FAKE_REMOTETOOL)

        mock_show_action.assert_called_once_with(action_digest)
        self.assertEqual(record.command.args, command)
        self.assertEqual(
            record.command.input.inputs,
            [os.path.join(rpl_tool._WORKING_DIR_SYMBOL, "w00f.txt")],
        )
        # Make sure all other fields are the same as before
        record_original.command.args.extend(record.command.args)
        record_original.command.input.inputs.extend(record.command.input.inputs)
        self.assertEqual(record, record_original)

    def test_one_record_input_in_remote_working_dir(self):
        remote_working_dir = "set/by/some/tool"
        input = "w00f.txt"
        command = ["cat", input]
        inputs = {Path(remote_working_dir) / input: "f91fe9c02e71/6"}
        action_digest = "ff787d009d19898a009/147"
        record = log_pb2.LogRecord()
        record.remote_metadata.action_digest = action_digest
        record.command.remote_working_directory = remote_working_dir
        record_original = log_pb2.LogRecord()
        record_original.CopyFrom(record)
        with mock.patch.object(
            remotetool.RemoteTool,
            "show_action",
            return_value=remotetool.ShowActionResult(
                command=command, inputs=inputs, output_files={}, platform={}
            ),
        ) as mock_show_action:
            rpl_tool.infer_record_command_and_inputs(record, _FAKE_REMOTETOOL)

        mock_show_action.assert_called_once_with(action_digest)
        self.assertEqual(record.command.args, command)
        self.assertEqual(
            record.command.input.inputs,
            [os.path.join(rpl_tool._WORKING_DIR_SYMBOL, "w00f.txt")],
        )
        # Make sure all other fields are the same as before
        record_original.command.args.extend(record.command.args)
        record_original.command.input.inputs.extend(record.command.input.inputs)
        self.assertEqual(record, record_original)


class ExpandToRplTests(unittest.TestCase):
    def test_expand_two_records(self):
        command1 = ["cat", "hello.txt"]
        command2 = ["cow", "moo.txt"]
        inputs1 = {Path("hello.txt"): "00de7abc8771/7"}
        inputs2 = {Path("moo.txt"): "eedafabc1201/8"}
        action_digest1 = "90123eeecbd19898/147"
        action_digest2 = "23eeecbd1986cca1/147"
        record1 = log_pb2.LogRecord()
        record1.remote_metadata.action_digest = action_digest1
        record2 = log_pb2.LogRecord()
        record2.remote_metadata.action_digest = action_digest2
        logdump = log_pb2.LogDump(records=[record1, record2])
        logdump_original = log_pb2.LogDump()
        logdump_original.CopyFrom(logdump)
        with mock.patch.object(
            remotetool.RemoteTool,
            "show_action",
            side_effect=[
                remotetool.ShowActionResult(
                    command=command1,
                    inputs=inputs1,
                    output_files={},
                    platform={},
                ),
                remotetool.ShowActionResult(
                    command=command2,
                    inputs=inputs2,
                    output_files={},
                    platform={},
                ),
            ],
        ) as mock_show_action:
            new_logdump = rpl_tool.expand_to_rpl(logdump, _FAKE_REMOTETOOL)

        mock_show_action.assert_has_calls(
            [
                mock.call(action_digest1),
                mock.call(action_digest2),
            ]
        )
        new_record1 = new_logdump.records[0]
        new_record2 = new_logdump.records[1]
        self.assertEqual(new_record1.command.args, command1)
        self.assertEqual(new_record2.command.args, command2)
        self.assertEqual(
            new_record1.command.input.inputs, [str(k) for k in inputs1.keys()]
        )
        self.assertEqual(
            new_record2.command.input.inputs, [str(k) for k in inputs2.keys()]
        )
        # Make sure all other fields are the same as before
        logdump_original.records[0].command.args.extend(
            new_record1.command.args
        )
        logdump_original.records[1].command.args.extend(
            new_record2.command.args
        )
        logdump_original.records[0].command.input.inputs.extend(
            new_record1.command.input.inputs
        )
        logdump_original.records[1].command.input.inputs.extend(
            new_record2.command.input.inputs
        )
        self.assertEqual(new_logdump, logdump_original)


class MainTests(unittest.TestCase):
    def test_expand_to_rpl_to_file(self):
        empty_logdump = log_pb2.LogDump()
        log = reproxy_logs.ReproxyLog(empty_logdump)
        with mock.patch.object(
            reproxy_logs, "parse_log", return_value=log
        ) as mock_parse_log:
            with mock.patch.object(
                remotetool,
                "configure_remotetool",
                return_value=_FAKE_REMOTETOOL,
            ) as mock_remotetool:
                with mock.patch.object(
                    rpl_tool, "expand_to_rpl"
                ) as mock_expand:
                    with tempfile.TemporaryDirectory() as td:
                        exit_code = rpl_tool.main(
                            [
                                "expand_to_rpl",
                                "reduced.rrpl",
                                "-o",
                                os.path.join(td, "expanded.rpl"),
                            ]
                        )
        self.assertEqual(exit_code, 0)
        mock_parse_log.assert_called_once()
        mock_remotetool.assert_called_once()
        mock_expand.assert_called_once()
        expand_args, unused_kwargs = mock_expand.call_args_list[0]
        self.assertEqual(expand_args[0], log.proto)
        self.assertEqual(expand_args[1], _FAKE_REMOTETOOL)

    def test_expand_to_rpl_to_stdout(self):
        empty_logdump = log_pb2.LogDump()
        log = reproxy_logs.ReproxyLog(empty_logdump)
        with mock.patch.object(
            reproxy_logs, "parse_log", return_value=log
        ) as mock_parse_log:
            with mock.patch.object(
                remotetool,
                "configure_remotetool",
                return_value=_FAKE_REMOTETOOL,
            ) as mock_remotetool:
                with mock.patch.object(
                    rpl_tool, "expand_to_rpl"
                ) as mock_expand:
                    with contextlib.redirect_stdout(io.StringIO()):
                        exit_code = rpl_tool.main(
                            [
                                "expand_to_rpl",
                                "reduced.rrpl",
                            ]
                        )
        self.assertEqual(exit_code, 0)
        mock_parse_log.assert_called_once()
        mock_remotetool.assert_called_once()
        mock_expand.assert_called_once()
        expand_args, unused_kwargs = mock_expand.call_args_list[0]
        self.assertEqual(expand_args[0], log.proto)
        self.assertEqual(expand_args[1], _FAKE_REMOTETOOL)


if __name__ == "__main__":
    unittest.main()
