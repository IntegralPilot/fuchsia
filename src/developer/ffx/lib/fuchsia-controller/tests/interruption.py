# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import signal
import subprocess
import sys
import time
import unittest


class TestInterruption(unittest.TestCase):
    def _run_test_executable_and_send_signal(
        self, sig: signal.Signals
    ) -> subprocess.Popen[str]:
        code = """
import fuchsia_controller_py
import sys
import time

try:
    # _block_forever blocks forever but should be interruptible by SIGINT.
    fuchsia_controller_py._block_forever()
except KeyboardInterrupt:
    print("Caught KeyboardInterrupt")
    sys.exit(0)
except Exception as e:
    print(f"Caught exception: {e}")
    sys.exit(1)
print("Exited without exception")
sys.exit(2)
"""
        env = os.environ.copy()
        env["PYTHONPATH"] = ":".join(sys.path)
        p = subprocess.Popen(
            [sys.executable, "-c", code],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=env,
        )

        time.sleep(5)

        p.send_signal(sig)

        try:
            stdout, stderr = p.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            p.kill()
            stdout, stderr = p.communicate()
            self.fail(
                f"Subprocess timed out after {str(sig)}. stderr: {stderr}, stdout: {stdout}"
            )

        self.assertTrue(
            p.returncode == 0 or p.returncode == -sig,
            f"Subprocess failed with exit code {p.returncode}. stderr: {stderr}, stdout: {stdout}",
        )
        self.assertTrue(
            "Caught KeyboardInterrupt" in stdout
            or "KeyboardInterrupt" in stderr,
            f"Expected KeyboardInterrupt in output. stderr: {stderr}, stdout: {stdout}",
        )
        return p

    def test_block_forever_interruptible_sigint(self) -> None:
        self._run_test_executable_and_send_signal(signal.SIGINT)

    def test_block_forever_interruptible_sigterm(self) -> None:
        self._run_test_executable_and_send_signal(signal.SIGTERM)


if __name__ == "__main__":
    unittest.main()
