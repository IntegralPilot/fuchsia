# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Helper class to capture and process stdio streams from Python."""

import os
import pty
import sys
import threading
import typing as T


class OutputSink(object):
    """Abstract interface for redirection sinks, must be derived."""

    def __init__(self) -> None:
        pass

    def open(self) -> None:
        """Called when opening the sink before writing anything to it."""

    def write(self, data: bytes) -> bool:
        """Called repeatedly to write data to the sink.

        Args:
            data: data bytes to write to the sink.
        Returns:
            True on success, False on error, which will notify the caller
            to stop writing immediately.
        """
        raise NotImplementedError

    def close(self) -> None:
        """Called when closing the sink after writing has completed."""

    def __enter__(self) -> T.Self:
        self.open()
        return self

    def __exit__(
        self, exc_type: T.Any, exc_val: T.Any, exc_tb: T.Any
    ) -> T.Literal[False]:
        self.close()
        return False


class BytesOutputSink(OutputSink):
    """A OutputSink that stores all output in a bytes array."""

    def __init__(self) -> None:
        self.data = bytes()

    def write(self, data: bytes) -> bool:
        self.data += data
        return True


class FileOutputSink(OutputSink):
    """A OutputSink that writes all outputs directly to a file."""

    def __init__(self, output_path: str) -> None:
        self._output_path = output_path
        self._output_file: T.BinaryIO | None = None

    def open(self) -> None:
        os.makedirs(os.path.dirname(self._output_path), exist_ok=True)
        self._output_file = open(self._output_path, "wb")

    def write(self, data: bytes) -> bool:
        assert self._output_file
        self._output_file.write(data)
        return True

    def close(self) -> None:
        if self._output_file:
            self._output_file.close()
            self._output_file = None


class StdoutOutputSink(OutputSink):
    """A OutputSink that writes to sys.stdout."""

    def write(self, data: bytes) -> bool:
        sys.stdout.buffer.write(data)
        sys.stdout.flush()
        return True


class StderrOutputSink(OutputSink):
    """A OutputSink that writes to sys.stderr."""

    def write(self, data: bytes) -> bool:
        sys.stderr.buffer.write(data)
        sys.stderr.flush()
        return True


class PipeOutputSink(OutputSink):
    """A OutputSink that wraps a pipe or pty to write to another sink.

    This is mostly useful when launching a subprocess and redirecting
    its stdout or stderr to the pipe's write end (or the pty slave fd).

    Usage is the following:
       1) Create instance, passing a final output sink. This creates
          a pipe or a pty, and a background thread to read from it
          and send the results to the output sink.

       2) Call get_write_fd() to retrieve the write end of the pipe
          (or the slave pty descriptor). This can be passed to other
          function that need to it directly.

       2b) Alternatively, use this as a regular sink by calling its
           write() method directly.

       3) The background thread is stopped automatically on close().

    For example, here's how to launch a subprocess and have its stdout
    redirected to a pty while recording all output to a byte buffer:

       byte_sink = BytesOutputSink()
       with PipeOutputSink(byte_sync, use_pty=True) as pty_sink:
           subprocess.run([...], stdout=pty_sink.get_write_fd(), check=True)

       ... The collected output will be in byte_sink.data
    """

    def __init__(self, output_sink: OutputSink, use_pty: bool = False):
        """Create instance

        Args:
            output_sink: Output sink that will receive data read from
               the read end of the pipe. Its write() method will be
               called from a background thread.
            use_pty: Set to True to use a pty instead of a regular pipe.
        """
        self._output_sink = output_sink
        self._thread: None | threading.Thread = None
        self._pipe_read_fd: int = -1
        self._pipe_write_fd: int = -1
        self._use_pty = use_pty

    def open(self) -> None:
        self._output_sink.open()
        self._pipe_read_fd, self._pipe_write_fd = (
            pty.openpty() if self._use_pty else os.pipe()
        )

        self._thread = threading.Thread(target=self._reader_thread, args=())
        self._thread.start()

    def get_read_fd(self) -> int:
        return self._pipe_read_fd

    def get_write_fd(self) -> int:
        return self._pipe_write_fd

    def write(self, data: bytes) -> bool:
        try:
            os.write(self._pipe_write_fd, data)
            return True
        except OSError:
            return False

    def _reader_thread(self) -> None:
        while True:
            try:
                data = os.read(self._pipe_read_fd, 8192)
                if not data:
                    return
                if not self._output_sink.write(data):
                    return
            except OSError:
                return

    def close(self) -> None:
        if self._pipe_write_fd >= 0:
            os.close(self._pipe_write_fd)
        # NOTE(https://fxbug.dev/466166329): Wait for thread to finish
        # reading all pipe inputs before closing the read descriptor.
        if self._thread:
            self._thread.join()
        if self._pipe_read_fd >= 0:
            os.close(self._pipe_read_fd)
        self._output_sink.close()
