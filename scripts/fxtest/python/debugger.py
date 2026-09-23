# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import atexit
from collections.abc import Awaitable
import os
import random
import signal
import string
import subprocess
import sys
import tempfile
import typing

from daemon_manager import DaemonManager
from portpicker import portpicker

from event import EventRecorder
import test_list_file


async def _start_zxdb_daemon(
    recorder: EventRecorder | None, port: int
) -> DaemonManager:
    try:
        manager = DaemonManager(port=port, connect_to_existing=True)
        proc = await manager.start()
        if proc is None:
            raise RuntimeError(
                "Daemon process was not started (already running or failed)"
            )
    except Exception as e:
        raise RuntimeError(f"Failed to start zxdb-daemon: {e}") from e

    # Print a hint when using DAP mode for how to connect with zxdb-cli.
    if recorder is not None:
        recorder.emit_info_message(
            "A new fx debug cli session has been started automatically, "
            "get started with `fx debug cli wait-for-event`."
        )

    return manager


def spawn(
    tests: list[test_list_file.Test],
    on_debugger_ready: typing.Callable[[], Awaitable[typing.Any]],
    recorder: EventRecorder | None = None,
    break_on_failure: bool = False,
    enable_debug_adapter: bool = False,
    debug_adapter_port: int | None = None,
    breakpoints: list[str] | None = None,
) -> subprocess.Popen[bytes]:
    """Spawn zxdb in a subprocess.

    Spawn zxdb and attach to |tests|, while waiting for a test failure reported by either
    exception or software breakpoint. Standard output for this program is redirected to a fifo,
    which zxdb will stream to the console. The debugger is spawned in a synchronous process since
    zxdb will be handling all of the stdio streams itself and will take control of forwarding IO
    from python back to the console. The caller has no responsibility to deal with any input or
    output from the spawned process.

    Args:
        tests (list[test_list_file.Test]): List of tests selected to be executed.
        on_debugger_ready (typing.Callable): An async closure to be issued when the debugger is
                                             ready for processes to be spawned.
        recorder (event.EventRecorder | None): Recorder for events. Defaults to None.
        break_on_failure (bool): Whether or not we are in break-on-failure mode.
        enable_debug_adapter (bool): Enables zxdb's DebugAdapter server.
        debug_adapter_port (int | None): Sets the port for the DebugAdapter server. If None, a
                                         random one is assigned.
        breakpoints (list[str]): List of breakpoint locations to install. Note: this may slow down
                                 test execution significantly.

    Returns:
        subprocess.Popen: process handle for the zxdb process group.

        Note: The caller is responsible for killing the process group associated with the returned
        process.
    """
    breakpoints = breakpoints or []
    fifo = os.path.join(
        tempfile.gettempdir()
        + "/zxdbpipe-"
        + "".join(
            random.choice(string.ascii_letters + string.digits)
            for _ in range(6)
        )
    )

    os.mkfifo(fifo)

    attach_args = []
    for test in tests:
        # If there are no explicit breakpoints, then we can weakly attach to all tests. Explicit
        # breakpoints require us to load symbols proactively.
        if not breakpoints:
            attach_args.extend(
                ["--execute", f"attach --weak --recursive {test.name()}"]
            )
        else:
            attach_args.extend(
                ["--execute", f"attach --recursive {test.name()}"]
            )

    # If only --breakpoint was specified on the command line (we won't get here if neither
    # debug option was specified), we want to output a more general message than "test
    # failure". Zxdb will default to filling in the type of exception in this slot if it's
    # unspecified, so we don't need to specify any additional text. If both options are
    # specified, it is impossible to know which one will happen first, so use the more specific
    # text.
    embedded_mode_context_args = []
    if break_on_failure:
        embedded_mode_context_args = ["--embedded-mode-context", "test failure"]

    port: int | None = None
    debug_adapter_args = []

    # |enable_debug_adapter| means we're always going to spawn a zxdb-daemon process now.
    # zxdb-daemon will connect to the zxdb that we create below.
    if enable_debug_adapter:
        port = (
            debug_adapter_port
            if debug_adapter_port is not None
            else portpicker.pick_unused_port()
        )

        debug_adapter_args = ["--enable-debug-adapter"]

        debug_adapter_args.append("--debug-adapter-port")
        debug_adapter_args.append(f"{port}")

    zxdb_args = [
        "fx",
        "ffx",
        "debug",
        "connect",
        "--new-agent",
        "--",
        *attach_args,
        "--console-mode",
        "embedded",
        *embedded_mode_context_args,
        "--stream-file",
        fifo,
        "--signal-when-ready",
        str(os.getpid()),
        *debug_adapter_args,
    ]

    # Add the requested breakpoints.
    for bp in breakpoints:
        zxdb_args += ["--execute", f"break {bp}"]

    # Use os.setpgrp instead of start_new_session=True, because we're going to explicitly handoff
    # control of the foreground terminal to zxdb (and take it back during _cleanup below). Using
    # start_new_session=True would create the process in a separate session, which will raise EPERM
    # if we try to give it our TTY.
    debugger_process = subprocess.Popen(
        args=zxdb_args, preexec_fn=os.setpgrp, stderr=subprocess.STDOUT
    )

    # Give control of the tty to zxdb. This lets zxdb know that it has control of the TTY that we
    # started with.
    #
    # Note that we do NOT need to set this back to our pgrp since our output will be routed through
    # zxdb until the very end. If we try to call tcsetpgrp again during shutdown, we'll race with
    # the shell and could leave a job suspended while waiting on an unnecessary SIGTTOU. This has no
    # effect when running in debug_adapter mode since zxdb will not make use of terminal features in
    # this mode.
    try:
        if hasattr(sys.stdin, "fileno") and os.isatty(sys.stdin.fileno()):
            os.tcsetpgrp(sys.stdin.fileno(), debugger_process.pid)
    except (OSError, Exception):
        pass

    daemon_manager: DaemonManager | None = None

    async def init_debugger() -> None:
        nonlocal daemon_manager
        try:
            if enable_debug_adapter and port is not None:
                daemon_manager = await _start_zxdb_daemon(recorder, port)
        except Exception as e:
            if recorder is not None:
                recorder.emit_warning_message(
                    f"Failed to start zxdb-daemon: {e}"
                )
        await on_debugger_ready()

    def on_sigusr1() -> None:
        # Remove the signal handler immediately so it doesn't fire again.
        loop.remove_signal_handler(signal.SIGUSR1)

        asyncio.create_task(init_debugger())

        # Replace stdout with the named pipe we created and enable line buffering.
        # Note: 1 == line buffered. See https://docs.python.org/3/library/functions.html#open.
        sys.stdout = open(fifo, "w", buffering=1)

    # zxdb will send us a SIGUSR1 when it has successfully connected to DebugAgent and is ready to
    # stream output.
    loop = asyncio.get_event_loop()
    loop.add_signal_handler(signal.SIGUSR1, on_sigusr1)

    def _cleanup() -> None:
        # Close stdout. This may have already been done at the end of all the tests in main.py, but
        # we do it again here to catch the ctrl+c case and still try to cleanly restore the terminal
        # and clean up the socket to DebugAgent.
        try:
            sys.stdout.close()
        except Exception:
            pass
        sys.stdout = open(os.devnull, "w")

        try:
            os.remove(fifo)
        except FileNotFoundError:
            # The tests for this don't actually create a file for the fifo, and we don't want to
            # throw an exception, since we were trying to remove the file anyway.
            pass

        # Clean up daemon process if it was running
        if daemon_manager is not None:
            try:
                daemon_manager.stop_sync(timeout=5.0)
            except BaseException:
                pass

        try:
            # Give zxdb a chance to gracefully shutdown, in the normal case this should return
            # immediately, but when handling ctrl+c in embedded mode we wait some time to run
            # cleanup routines.
            debugger_process.wait(5)
        except subprocess.TimeoutExpired as e:
            sys.stderr.write(f"{e}\n")
            sys.stderr.flush()
        finally:
            # zxdb should have gracefully exited by now, if it hasn't forcefully terminate and
            # inform the user that they may need to reset their terminal.
            if debugger_process.poll() is None:
                sys.stderr.write(
                    "⚠️  Warning: zxdb did not exit normally. `reset` will fix your terminal ⚠️\n"
                )
                sys.stderr.flush()
                debugger_process.terminate()

    atexit.register(_cleanup)

    return debugger_process
