# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import ipaddress
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from threading import Event, Thread
from typing import Any

TEST_DATA_DIR = "host_x64/test_data/fidlcat_e2e_tests"  # relative to $PWD
FIDLCAT_TIMEOUT = 60  # timeout when invoking fidlcat

# Convert FUCHSIA_SSH_KEY into an absolute path. Otherwise ffx cannot find
# key and complains "Timeout attempting to reach target".
# See https://fxbug.dev/42051867.
os.environ.update(
    FUCHSIA_ANALYTICS_DISABLED="1",
    FUCHSIA_SSH_KEY=os.path.abspath(os.environ["FUCHSIA_SSH_KEY"]),
)


class Ffx:
    # Relative to outdir.
    _path = "host-tools/ffx"
    # Automatically deleted when |self| is destructed.
    _isolate_dir = tempfile.TemporaryDirectory()
    _isolate_args = [
        "--isolate-dir",
        _isolate_dir.name,
    ]
    # General FFX configuration.
    _ffx_config = [
        "--config",
        "ffx.subtool-search-paths=" + os.getcwd() + "/host-tools",
        "--config",
        "log.level=DEBUG,log.dir=" + os.environ["FUCHSIA_TEST_OUTDIR"],
        "--config",
        "fastboot.usb.disabled=true",
        "--config",
        "discovery.mdns.enabled=false",
    ]
    # The target address will be in environment variables and determined at initialization.
    _target = ["--target"]
    # The actual ffx command to run.
    _command: list[str] = []

    def __init__(self, *args: str) -> None:
        self._command = list(args)
        self._target.append(self.get_target())

    @staticmethod
    def get_target() -> str:
        if not "FUCHSIA_DEVICE_ADDR" in os.environ.keys():
            raise RuntimeError("FUCHSIA_DEVICE_ADDR must be specified.")

        addr = ipaddress.ip_address(os.environ["FUCHSIA_DEVICE_ADDR"])
        target = ""

        # Fixup IPv6 address.
        if addr.version == 6:
            target = "[" + str(addr) + "]"
        else:
            target = str(addr)

        # FUCHSIA_SSH_PORT is set when the test is run from `fx test`.
        if "FUCHSIA_SSH_PORT" in os.environ.keys():
            port = os.environ["FUCHSIA_SSH_PORT"]
            target = target + ":" + port

        return target

    # Run the requested ffx command.
    def start(self) -> None:
        self.process = subprocess.Popen(
            [self._path]
            + self._isolate_args
            + self._ffx_config
            + self._target
            + self._command,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )

    def wait(self) -> int:
        self.process.communicate()
        return self.process.returncode


class Fidlcat:
    _path = "host_x64/fidlcat"
    _arg: list[str] = []
    _ffx_bridge = None
    _debug_agent_socket_path = None

    def __init__(self, *args: str, merge_stderr: bool = False) -> None:
        """
        merge_stderr: whether to merge stderr to stdout.
        """
        assert self._ffx_bridge is not None, "must call setup first"
        stderr = subprocess.PIPE
        if merge_stderr:
            stderr = subprocess.STDOUT
        self.process: subprocess.Popen[str] = subprocess.Popen(
            [self._path] + self._args + list(args),
            text=True,
            stdout=subprocess.PIPE,
            stderr=stderr,
        )
        self.stdout: str = (
            ""  # Contains both stdout and stderr, if merge_stderr.
        )
        self.stderr: str = ""
        self._timeout_cancel: Event = Event()
        Thread(target=self._timeout_thread).start()

    def _timeout_thread(self) -> None:
        self._timeout_cancel.wait(FIDLCAT_TIMEOUT)
        if not self._timeout_cancel.is_set():
            self.process.kill()
            self.wait()
            raise TimeoutError("Fidlcat timeouts\n" + self.get_diagnose_msg())

    def wait(self) -> int:
        """Wait for the process to terminate, assert the returncode, fill the stdout and stderr."""
        (stdout, stderr) = self.process.communicate()
        self.stdout += stdout
        if stderr:  # None if merge_stderr
            self.stderr += stderr
        self._timeout_cancel.set()
        return self.process.returncode

    def read_until(self, pattern: str) -> bool:
        """
        Read the stdout until EOF or a line contains pattern. Returns whether the pattern matches.

        Note: A deadlock could happen if we only read from stdout but the stderr buffer is full.
              Consider setting merge_stderr if you want to use this function.
        """
        stdout = self.process.stdout
        if not stdout:
            return False

        while True:
            line = stdout.readline()
            if not line:
                return False
            self.stdout += line
            if pattern in line:
                return True

    def get_diagnose_msg(self) -> str:
        return (
            "\n=== stdout ===\n"
            + self.stdout
            + "\n\n=== stderr===\n"
            + self.stderr
            + "\n"
        )

    @classmethod
    def setup(cls: Any) -> None:
        cls._ffx_bridge = Ffx("debug", "connect", "--agent-only", "--new-agent")
        cls._ffx_bridge.start()
        cls._debug_agent_socket_path = (
            cls._ffx_bridge.process.stdout.readline().strip()
            if cls._ffx_bridge.process.stdout is not None
            else ""
        )

        assert os.path.exists(cls._debug_agent_socket_path)

        cls._args = [
            "--unix-connect",
            cls._debug_agent_socket_path,
            "--fidl-ir-path",
            TEST_DATA_DIR,
            "--symbol-path",
            TEST_DATA_DIR,
        ]

    @classmethod
    def teardown(cls: Any) -> None:
        if cls._ffx_bridge:
            cls._ffx_bridge.process.terminate()

        if cls._debug_agent_socket_path is None:
            return

        socket_path = pathlib.Path(cls._debug_agent_socket_path)

        # The host end of debug_agent's socket is supposed to be cleaned up when the ffx isolate is
        # destroyed, but sometimes doesn't for unknown reasons. Clean it up explicitly here just in
        # case.
        socket_path.unlink(missing_ok=True)


# fuchsia-pkg URL for an echo realm. The echo realm contains echo client and echo server components.
# The echo client is an eager child of the realm and will start when the realm is started/run.
#
# Note that the actual echo client is in a standalone component echo_client.cm so we almost always
# need to specify "--remote-component=echo_client.cm" in the test cases below.
ECHO_REALM_URL = (
    "fuchsia-pkg://fuchsia.com/echo_realm_placeholder#meta/echo_realm.cm"
)
ECHO_REALM_MONIKER = "/core/ffx-laboratory:fidlcat_test_echo_realm"


class FidlcatE2eTests(unittest.TestCase):
    maxDiff = None

    @classmethod
    def setUpClass(cls: Any) -> None:
        Fidlcat.setup()

    @classmethod
    def tearDownClass(cls: Any) -> None:
        Fidlcat.teardown()

    # Ensure debug_agent exits correctly after each test case. See https://fxbug.dev/42051863.
    def tearDown(self) -> None:
        # FUCHSIA_DEVICE_ADDR and FUCHSIA_SSH_KEY must be defined.
        # FUCHSIA_SSH_PORT is only defined when invoked from `fx test`.
        cmd = [
            "ssh",
            "-F",
            "none",
            "-o",
            "CheckHostIP=no",
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-i",
            os.environ["FUCHSIA_SSH_KEY"],
        ]
        if os.environ.get("FUCHSIA_SSH_PORT"):
            cmd += ["-p", os.environ["FUCHSIA_SSH_PORT"]]
        cmd += [
            os.environ["FUCHSIA_DEVICE_ADDR"],
            "killall /pkg/bin/debug_agent",
        ]
        res = subprocess.run(
            cmd, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT
        )
        if res.returncode == 0 and "Killed" in res.stdout:
            print("Killed dangling debug_agent", file=sys.stderr)
        else:
            # The return code will be 255 if no task found so don't check it.
            assert "no tasks found" in res.stdout, res.stdout

    def test_run_echo(self) -> None:
        fidlcat = Fidlcat(
            "--remote-component=echo_client.cm", "run", ECHO_REALM_URL
        )
        self.assertEqual(fidlcat.wait(), 0, fidlcat.get_diagnose_msg())

        self.assertIn(
            "sent request test.placeholders/Echo.EchoString = {\n"
            '    value: string = "hello world"\n'
            "  }",
            fidlcat.stdout,
        )

    # TODO(https://fxbug.dev/42064761): This test flakes on core.x64-debug, where fidlcat fails to exit after
    # receiving the SIGTERM signal.
    def disabled_test_stay_alive(self) -> None:
        fidlcat = Fidlcat(
            "--remote-name=echo_client", "--stay-alive", merge_stderr=True
        )
        fidlcat.read_until("Connected!")

        self.assertEqual(
            Ffx("component", "run", ECHO_REALM_MONIKER, ECHO_REALM_URL).wait(),
            0,
        )
        self.assertEqual(
            Ffx("component", "destroy", ECHO_REALM_MONIKER).wait(), 0
        )
        fidlcat.read_until("Waiting for more processes to monitor.")

        # Because, with the --stay-alive version, fidlcat never ends,
        # we need to kill it to end the test.
        fidlcat.process.terminate()
        self.assertEqual(fidlcat.wait(), 0, fidlcat.get_diagnose_msg())

    def test_extra_component(self) -> None:
        fidlcat = Fidlcat(
            "--remote-component=echo_client.cm",
            "--extra-component=echo_server.cm",
            "run",
            ECHO_REALM_URL,
        )
        self.assertEqual(fidlcat.wait(), 0, fidlcat.get_diagnose_msg())

        self.assertIn("Monitoring echo_server.cm koid=", fidlcat.stdout)

    def test_trigger(self) -> None:
        fidlcat = Fidlcat(
            "--remote-component=echo_client.cm",
            "--trigger=.*EchoString",
            "run",
            ECHO_REALM_URL,
        )
        self.assertEqual(fidlcat.wait(), 0, fidlcat.get_diagnose_msg())

        # The first displayed message must be EchoString.
        lines = fidlcat.stdout.split("\n\n")
        self.assertIn(
            "sent request test.placeholders/Echo.EchoString = {", lines[2]
        )

    def test_messages(self) -> None:
        fidlcat = Fidlcat(
            "--remote-component=echo_client.cm",
            "--messages=.*EchoString",
            "run",
            ECHO_REALM_URL,
        )
        self.assertEqual(fidlcat.wait(), 0, fidlcat.get_diagnose_msg())

        # The first and second displayed messages must be EchoString (everything else has been
        # filtered out).
        lines = fidlcat.stdout.split("\n\n")
        self.assertIn(
            "sent request test.placeholders/Echo.EchoString = {\n"
            '    value: string = "hello world"\n'
            "  }",
            lines[2],
        )
        self.assertIn(
            "received response test.placeholders/Echo.EchoString = {\n"
            '      response: string = "hello world"\n'
            "    }",
            lines[3],
        )

    def test_save_replay(self) -> None:
        save_path = tempfile.NamedTemporaryFile(suffix="_save.pb")
        fidlcat = Fidlcat(
            "--remote-component=echo_client.cm",
            "--to",
            save_path.name,
            "run",
            ECHO_REALM_URL,
        )
        self.assertEqual(fidlcat.wait(), 0, fidlcat.get_diagnose_msg())
        self.assertIn(
            "sent request test.placeholders/Echo.EchoString = {\n"
            '    value: string = "hello world"\n'
            "  }",
            fidlcat.stdout,
        )

        fidlcat = Fidlcat("--from", save_path.name)
        self.assertEqual(fidlcat.wait(), 0, fidlcat.get_diagnose_msg())
        self.assertIn(
            "sent request test.placeholders/Echo.EchoString = {\n"
            '    value: string = "hello world"\n'
            "  }",
            fidlcat.stdout,
        )

    def test_with_summary(self) -> None:
        fidlcat = Fidlcat(
            "--with=summary", "--from", TEST_DATA_DIR + "/echo.pb"
        )
        self.assertEqual(fidlcat.wait(), 0, fidlcat.get_diagnose_msg())

        self.assertEqual(
            fidlcat.stdout,
            """\
--------------------------------------------------------------------------------echo_client.cm 38724: 14 handles

  Process:25ce275f(proc-self)

  startup Thread:25ce269f(thread-self)

  startup Vmar:21ee277b(vmar-root)

  startup Channel:25de241f(dir:/pkg)

  startup Channel:25de25ff(dir:/svc)
      32.413004 write request  fuchsia.io/Openable.Open(".")
        -> 252e2acb

  startup Channel:252e2df7(directory-request:/)

  startup Clock:25de2597()

  startup Socket:25ae25bf(fd:1)

  startup Socket:25ce25b3(fd:2)

  startup Job:2efe217f(job-default)

  startup Vmo:251e264b(vdso-vmo)

  startup Vmo:20ee265f(stack-vmo)

  Channel:25fe2aef(channel:0)
      32.462118 write request  fuchsia.io/Openable.Open("test.placeholders.Echo")
        -> 257e2a53

  Channel:24ce2c27()
    linked to Channel:8b811f73(directory-request:/svc/test.placeholders.Echo) in process echo_server.cm:39310
      32.512762 write request  test.placeholders/Echo.EchoString
      33.267383 read  response test.placeholders/Echo.EchoString

--------------------------------------------------------------------------------echo_server.cm 39310: 13 handles

  Process:8b11102f(proc-self)

  startup Thread:8be11a67(thread-self)

  startup Vmar:8b111fe7(vmar-root)

  startup Channel:88b11863(dir:/pkg)

  startup Channel:8bf11e73(dir:/svc)
      33.093308 write request  fuchsia.io/Openable.Open(".")
        -> 8be11273

  startup Channel:8bb11d3b(directory-request:/)
      33.140662 read  request  fuchsia.io/Openable.Open("svc/test.placeholders.Echo")
        -> Channel:8b811f73(directory-request:/svc/test.placeholders.Echo)

  startup Clock:8b311e27()

  startup Socket:8be11e3b(fd:1)

  startup Socket:8b011e1f(fd:2)

  startup Job:8be1100b(job-default)

  startup Vmo:8bd11947(vdso-vmo)

  startup Vmo:8bb11fdb(stack-vmo)

  Channel:8b811f73(directory-request:/svc/test.placeholders.Echo)
    linked to Channel:24ce2c27() in process echo_client.cm:38724
    created by Channel:8bb11d3b(directory-request:/) receiving fuchsia.io/Openable.Open
      33.192134 read  request  test.placeholders/Echo.EchoString
      33.241406 write response test.placeholders/Echo.EchoString
""",
        )

    def test_with_top(self) -> None:
        fidlcat = Fidlcat("--with=top", "--from", TEST_DATA_DIR + "/echo.pb")
        self.assertEqual(fidlcat.wait(), 0, fidlcat.get_diagnose_msg())

        self.assertEqual(
            fidlcat.stdout,
            """\
--------------------------------------------------------------------------------echo_client.cm 38724: 4 events
  fuchsia.io/Openable: 2 events
    Open: 2 events
      32.413004 write request  fuchsia.io/Openable.Open(Channel:25de25ff(dir:/svc), ".")
        -> 252e2acb
      32.462118 write request  fuchsia.io/Openable.Open(Channel:25fe2aef(channel:0), "test.placeholders.Echo")
        -> 257e2a53

  test.placeholders/Echo: 2 events
    EchoString: 2 events
      32.512762 write request  test.placeholders/Echo.EchoString(Channel:24ce2c27())
      33.267383 read  response test.placeholders/Echo.EchoString(Channel:24ce2c27())

--------------------------------------------------------------------------------echo_server.cm 39310: 4 events
  fuchsia.io/Openable: 2 events
    Open: 2 events
      33.093308 write request  fuchsia.io/Openable.Open(Channel:8bf11e73(dir:/svc), ".")
        -> 8be11273
      33.140662 read  request  fuchsia.io/Openable.Open(Channel:8bb11d3b(directory-request:/), "svc/test.placeholders.Echo")
        -> Channel:8b811f73(directory-request:/svc/test.placeholders.Echo)

  test.placeholders/Echo: 2 events
    EchoString: 2 events
      33.192134 read  request  test.placeholders/Echo.EchoString(Channel:8b811f73(directory-request:/svc/test.placeholders.Echo))
      33.241406 write response test.placeholders/Echo.EchoString(Channel:8b811f73(directory-request:/svc/test.placeholders.Echo))
""",
        )

    def test_with_top_and_unknown_message(self) -> None:
        fidlcat = Fidlcat("--with=top", "--from", TEST_DATA_DIR + "/unknown.pb")
        self.assertEqual(fidlcat.wait(), 0, fidlcat.get_diagnose_msg())

        self.assertIn(
            "  unknown interfaces: : 1 event\n"
            "      6862061079.791403 call   ordinal=36dadb5482dc1d55("
            "Channel:9b71d5c7(dir:/svc/fuchsia.feedback.DataProvider))\n",
            fidlcat.stdout,
        )

    def test_with_messages_and_unknown_message(self) -> None:
        fidlcat = Fidlcat(
            "--messages=.*x.*", "--from", TEST_DATA_DIR + "/unknown.pb"
        )
        self.assertEqual(fidlcat.wait(), 0, fidlcat.get_diagnose_msg())

        # We only check that fidlcat didn't crash.
        self.assertIn(
            "Stop monitoring exceptions.cmx koid 19884\n", fidlcat.stdout
        )


if __name__ == "__main__":
    unittest.main()
