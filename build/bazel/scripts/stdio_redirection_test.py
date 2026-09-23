# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(__file__))
import stdio_redirection


class OutputSinkTest(unittest.TestCase):
    def test_bytes_redirection_sink(self) -> None:
        sink = stdio_redirection.BytesOutputSink()
        sink.open()
        sink.write(b"Hello ")
        sink.write(b"World")
        sink.close()

        self.assertEqual(sink.data, b"Hello World")

    def test_file_redirection_sink(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            file_path = os.path.join(td, "out/temp.out")
            sink = stdio_redirection.FileOutputSink(file_path)
            self.assertFalse(os.path.exists(file_path))
            sink.open()
            self.assertTrue(os.path.exists(file_path))

            sink.write(b"Hello ")
            sink.write(b"Filesystem")
            sink.close()

            with open(file_path, "rb") as f:
                content = f.read()

            self.assertEqual(content, b"Hello Filesystem")


class StdioOutputSinkTest(unittest.TestCase):
    def setUp(self) -> None:
        self._td = tempfile.TemporaryDirectory()
        self._root = self._td.name

        self._test_out_path = os.path.join(self._root, "out")
        self._test_err_path = os.path.join(self._root, "err")

        self._prev_stdout = sys.stdout
        sys.stdout = open(self._test_out_path, "w")

        self._prev_stderr = sys.stderr
        sys.stderr = open(self._test_err_path, "w")

    def tearDown(self) -> None:
        sys.stdout.close()
        sys.stdout = self._prev_stdout

        sys.stderr.close()
        sys.stderr = self._prev_stderr

        self._td.cleanup()

    def test_stdout_redirection(self) -> None:
        with stdio_redirection.StdoutOutputSink() as sink:
            sink.write(b"Hello Stdout")

        with open(self._test_out_path, "rb") as f:
            content = f.read()

        self.assertEqual(content, b"Hello Stdout")

    def test_stderr_redirection(self) -> None:
        with stdio_redirection.StderrOutputSink() as sink:
            sink.write(b"Hello Stderr")

        with open(self._test_err_path, "rb") as f:
            content = f.read()

        self.assertEqual(content, b"Hello Stderr")

    def test_stdout_and_stderr_redirection(self) -> None:
        with stdio_redirection.StdoutOutputSink() as out_sink:
            with stdio_redirection.StderrOutputSink() as err_sink:
                out_sink.write(b"Hello Stdout")
                err_sink.write(b"Hello Stderr")

        with open(self._test_out_path, "rb") as f:
            content = f.read()

        self.assertEqual(content, b"Hello Stdout")

        with open(self._test_err_path, "rb") as f:
            content = f.read()
        self.assertEqual(content, b"Hello Stderr")


class PipeOutputSinkTest(unittest.TestCase):
    def setUp(self) -> None:
        self._td = tempfile.TemporaryDirectory()
        self._root = self._td.name

        self._test_out_path = os.path.join(self._root, "out")
        self._test_err_path = os.path.join(self._root, "err")

        self._prev_stdout = sys.stdout
        sys.stdout = open(self._test_out_path, "w")

        self._prev_stderr = sys.stderr
        sys.stderr = open(self._test_err_path, "w")

    def tearDown(self) -> None:
        sys.stdout.close()
        sys.stdout = self._prev_stdout

        sys.stderr.close()
        sys.stderr = self._prev_stderr

        self._td.cleanup()

    def test_pipe_redirection(self) -> None:
        out_sink = stdio_redirection.BytesOutputSink()
        with stdio_redirection.PipeOutputSink(out_sink, use_pty=False) as sink:
            self.assertFalse(os.isatty(sink.get_write_fd()))
            sink.write(b"hello pipe")
            os.write(sink.get_write_fd(), b" world")

        self.assertEqual(out_sink.data, b"hello pipe world")

    def test_pty_redirection(self) -> None:
        out_sink = stdio_redirection.BytesOutputSink()
        try:
            with stdio_redirection.PipeOutputSink(
                out_sink, use_pty=True
            ) as sink:
                self.assertTrue(os.isatty(sink.get_write_fd()))
                sink.write(b"hello pty")
                os.write(sink.get_write_fd(), b" world")

            self.assertEqual(out_sink.data, b"hello pty world")
        except OSError as e:
            # NOTE: Creating a pty on infra builders will raise OSError
            # due to a lack of ptys when running inside the nsjail.
            if str(e) == "out of pty devices":
                return
            raise e


if __name__ == "__main__":
    unittest.main()
