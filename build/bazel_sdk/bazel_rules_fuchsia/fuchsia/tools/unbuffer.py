#!/usr/bin/env python3
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import pty
import subprocess
import sys


def main(command: list[str]) -> int:
    # Workaround for https://github.com/bazel-contrib/rules_python/issues/3518
    # Clean up environment to avoid RUNFILES_DIR/RUNFILES_MANIFEST_FILE inheritance
    # which can confuse child Python processes.
    env = dict(os.environ)
    env.pop("RUNFILES_DIR", None)
    env.pop("RUNFILES_MANIFEST_FILE", None)

    try:
        if sys.stdout.isatty() and sys.stderr.isatty():
            return subprocess.run(command, env=env).returncode
        else:
            # https://fxbug.dev/42070263: Force process not to buffer by faking a tty.
            # Running test targets in a non-tty environment (eg: via `bazel run`,
            # `bazel test`, or via infra) implicitly enables stdout buffering for all
            # subprocesses while leaving stderr unbuffered, causing stderr/stdout to
            # appear out of order.
            # We emulate a tty here in order to fix this.
            host_fd, child_fd = pty.openpty()
            os.environ["NO_COLOR"] = "1"
            proc = subprocess.Popen(
                command, env=env, stdout=child_fd, stderr=child_fd
            )
            os.close(child_fd)

            while True:
                try:
                    stdout = os.read(host_fd, 1024)
                    sys.stdout.buffer.write(stdout)
                    sys.stdout.flush()
                except OSError:
                    break
                if not stdout:
                    break

            return proc.wait()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
