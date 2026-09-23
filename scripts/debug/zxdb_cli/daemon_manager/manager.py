# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import os
import signal
import socket
import subprocess
from collections.abc import Callable
from pathlib import Path
from typing import Final

from fx_cmd.lib import FxCmd
from pydantic import ValidationError
from shared.protocol import PROTOCOL_VERSION, Response, serialize
from shared.protocol.hello import HelloRequest
from shared.protocol.start import StartRequest
from shared.protocol.stop import StopRequest

UDS_PATH: Final[Path] = Path("/tmp/fx-debug-daemon.sock")


class DaemonManagerError(Exception):
    """Base class for all DaemonManager errors."""


class DaemonAlreadyRunningError(DaemonManagerError):
    """Raised when the daemon socket exists and responds to handshake, but we didn't request connecting to it."""


class DaemonHandshakeError(DaemonManagerError):
    """Raised when handshake with the daemon fails (e.g. version mismatch or protocol error)."""


class DaemonStartupTimeoutError(DaemonManagerError):
    """Raised when waiting for the daemon to signal readiness times out."""


class DaemonCrashError(DaemonManagerError):
    """Raised when the daemon process exits prematurely before signaling readiness."""


class DaemonConnectionError(DaemonManagerError):
    """Raised when connecting to the daemon socket fails."""


def _send_signal_and_wait(
    proc: subprocess.Popen[bytes],
    sig: int,
    fallback_fn: Callable[[], None],
    timeout: float,
) -> None:
    """Sends a terminating signal to the process group or calls a fallback function, then waits for exit.

    Signals the entire process group if running in its own group (pgid != os.getpgrp()),
    otherwise calls the direct fallback function to prevent terminating the caller. Assumes
    immediate process exit; non-terminating signals cause unnecessary block-waiting.
    """
    pgid = os.getpgid(proc.pid)
    if pgid != os.getpgrp():
        os.killpg(pgid, sig)
    else:
        fallback_fn()
    proc.wait(timeout=timeout)


def _cleanup_process(proc: subprocess.Popen[bytes]) -> None:
    """Cleans up the process by terminating it and killing it if it doesn't exit."""
    if proc.poll() is not None:
        return
    try:
        _send_signal_and_wait(proc, signal.SIGTERM, proc.terminate, 3.0)
    except Exception:
        try:
            _send_signal_and_wait(proc, signal.SIGKILL, proc.kill, 2.0)
        except Exception:
            pass


class DaemonManager:
    def __init__(
        self,
        socket_path: Path = UDS_PATH,
        port: int | None = None,
        connect_to_existing: bool = False,
        startup_timeout: float = 10.0,
        spawn_fn: Callable[[int], subprocess.Popen[bytes]] | None = None,
    ) -> None:
        self.socket_path = socket_path
        self.port = port
        self.connect_to_existing = connect_to_existing
        self.startup_timeout = startup_timeout
        self.spawn_fn = spawn_fn
        self._proc: subprocess.Popen[bytes] | None = None

    async def do_handshake_and_connect(self) -> bool | None:
        """Attempts to connect to the UDS and perform handshake.

        Returns:
            True if handshake succeeded.
            None if connection failed because the socket is not ready/refused.

        Raises:
            DaemonConnectionError: If connection to the socket fails unexpectedly.
            DaemonHandshakeError: If the handshake protocol or version check fails.
        """
        try:
            reader, writer = await asyncio.open_unix_connection(
                self.socket_path
            )
        except (ConnectionRefusedError, FileNotFoundError):
            return None
        except Exception as e:
            raise DaemonConnectionError(f"Error connecting to daemon: {e}")

        try:
            req = HelloRequest(version=PROTOCOL_VERSION)
            writer.write(serialize(req).encode("utf-8"))
            await writer.drain()

            try:
                response_line = await asyncio.wait_for(
                    reader.readline(), timeout=self.startup_timeout
                )
            except asyncio.TimeoutError:
                raise DaemonHandshakeError("Handshake timeout")

            if not response_line:
                raise DaemonHandshakeError(
                    "No response received during handshake"
                )

            try:
                resp = Response.model_validate_json(
                    response_line.decode("utf-8")
                )
            except ValidationError as e:
                raise DaemonHandshakeError(f"Malformed handshake response: {e}")

            if not resp.success:
                raise DaemonHandshakeError(f"Handshake failed: {resp.message}")

            if not isinstance(resp.body, dict):
                raise DaemonHandshakeError("Malformed handshake response body")

            daemon_version = resp.body.get("protocol_version")
            if daemon_version != PROTOCOL_VERSION:
                raise DaemonHandshakeError(
                    f"Protocol version mismatch. CLI: {PROTOCOL_VERSION}, Daemon: {daemon_version}"
                )

            return True
        except Exception as e:
            if not isinstance(e, DaemonHandshakeError):
                raise DaemonHandshakeError(f"Error during handshake: {e}")
            raise
        finally:
            writer.close()
            try:
                await writer.wait_closed()
            except Exception:
                pass

    async def is_running(self) -> bool:
        """Checks if the daemon is currently running and responding to handshakes."""
        try:
            return (await self.do_handshake_and_connect()) is True
        except Exception:
            return False

    async def connect_to_running_daemon(self) -> None:
        """Sends a StartRequest to the already running daemon to connect it to DAP."""
        if not self.socket_path.exists():
            raise DaemonConnectionError(
                f"Daemon socket not found at {self.socket_path}. Is it running?"
            )

        try:
            reader, writer = await asyncio.open_unix_connection(
                self.socket_path
            )
        except Exception as e:
            raise DaemonConnectionError(f"Error connecting to daemon: {e}")

        try:
            start_req = StartRequest(port=self.port, connect=True)
            writer.write(serialize(start_req).encode("utf-8"))
            await writer.drain()

            try:
                response_line = await asyncio.wait_for(
                    reader.readline(), timeout=self.startup_timeout
                )
            except asyncio.TimeoutError:
                raise DaemonConnectionError(
                    "Timed out waiting for response from daemon."
                )

            if not response_line:
                raise DaemonConnectionError("No response received from daemon.")

            try:
                resp = Response.model_validate_json(
                    response_line.decode("utf-8")
                )
            except ValidationError as e:
                raise DaemonConnectionError(
                    f"Malformed response from daemon: {e}"
                )

            if not resp.success:
                raise DaemonConnectionError(
                    resp.message or "Failed to connect to DAP"
                )

        except DaemonConnectionError:
            raise
        except Exception as e:
            raise DaemonConnectionError(f"Error communicating with daemon: {e}")
        finally:
            writer.close()
            try:
                await writer.wait_closed()
            except Exception:
                pass

    def _spawn_daemon_process(self, write_fd: int) -> subprocess.Popen[bytes]:
        """Spawns the zxdb-daemon process in the background using FxCmd."""
        fx_cmd = FxCmd()
        args = ["zxdb-daemon"]
        if self.port is not None:
            args.extend(["--port", str(self.port)])

        os.set_inheritable(write_fd, True)
        args.append(f"--ready-fd={write_fd}")

        command_line = fx_cmd.command_line(*args)

        return subprocess.Popen(
            command_line,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
            pass_fds=[write_fd],
        )

    async def _wait_for_ready_signal(self, read_fd: int) -> None:
        """Waits for the ready signal byte on the non-blocking pipe read end."""
        loop = asyncio.get_running_loop()
        future = loop.create_future()

        os.set_blocking(read_fd, False)

        def on_read() -> None:
            try:
                data = os.read(read_fd, 1)
                if not future.done():
                    if data == b"":
                        future.set_exception(
                            DaemonCrashError(
                                "Daemon exited prematurely and closed the sync pipe."
                            )
                        )
                    else:
                        future.set_result(data)
            except BlockingIOError:
                pass
            except Exception as e:
                if not future.done():
                    future.set_exception(e)

        loop.add_reader(read_fd, on_read)
        try:
            await asyncio.wait_for(future, timeout=self.startup_timeout)
        except asyncio.TimeoutError:
            raise DaemonStartupTimeoutError(
                "Timed out waiting for daemon to signal readiness."
            )
        finally:
            loop.remove_reader(read_fd)

    async def start(self) -> subprocess.Popen[bytes] | None:
        """Starts the daemon process or connects to an existing one.

        Returns:
            The spawned subprocess.Popen object, or None if successfully connected
            to an already running daemon.
        """
        read_fd = -1
        write_fd = -1
        proc = None
        startup_success = False

        try:
            # 1. Handle existing socket
            if self.socket_path.exists():
                handshake_result = await self.do_handshake_and_connect()

                if handshake_result is True:
                    if self.connect_to_existing:
                        await self.connect_to_running_daemon()
                        return None
                    raise DaemonAlreadyRunningError(
                        f"Daemon socket already exists at {self.socket_path}"
                    )
                else:
                    # handshake_result is None: socket exists but is stale
                    self.socket_path.unlink(missing_ok=True)

            # 2. Start a new daemon process
            read_fd, write_fd = os.pipe()

            if self.spawn_fn:
                proc = self.spawn_fn(write_fd)
            else:
                proc = self._spawn_daemon_process(write_fd)

            os.close(write_fd)
            write_fd = -1

            # Wait for daemon to signal readiness
            await self._wait_for_ready_signal(read_fd)

            # Verify handshake
            handshake_result = await self.do_handshake_and_connect()
            if handshake_result is not True:
                raise DaemonHandshakeError(
                    "Daemon started but failed to respond to handshake in time."
                )

            # Send StartRequest to establish the session
            try:
                reader, writer = await asyncio.open_unix_connection(
                    self.socket_path
                )
            except Exception as e:
                raise DaemonConnectionError(f"Error connecting to daemon: {e}")

            try:
                start_req = StartRequest(
                    port=self.port, connect=self.connect_to_existing
                )
                writer.write(serialize(start_req).encode("utf-8"))
                await writer.drain()
                try:
                    response_line = await asyncio.wait_for(
                        reader.readline(), timeout=self.startup_timeout
                    )
                except asyncio.TimeoutError:
                    raise DaemonConnectionError(
                        "Timed out waiting for response from daemon."
                    )
                if not response_line:
                    raise DaemonConnectionError(
                        "No response received from daemon on start request."
                    )
                try:
                    resp = Response.model_validate_json(
                        response_line.decode("utf-8")
                    )
                except ValidationError as e:
                    raise DaemonConnectionError(
                        f"Malformed response on start request: {e}"
                    )

                if not resp.success:
                    raise DaemonConnectionError(
                        resp.message or "Failed to initialize session"
                    )
            except DaemonConnectionError:
                raise
            except Exception as e:
                raise DaemonConnectionError(
                    f"Error communicating with daemon: {e}"
                )
            finally:
                writer.close()
                try:
                    await writer.wait_closed()
                except Exception:
                    pass

            startup_success = True
            self._proc = proc
            return proc

        finally:
            if not startup_success and proc is not None:
                _cleanup_process(proc)

            if write_fd != -1:
                try:
                    os.close(write_fd)
                except OSError:
                    pass
            if read_fd != -1:
                try:
                    os.close(read_fd)
                except OSError:
                    pass

    def stop_sync(self, timeout: float = 5.0) -> None:
        """Encapsulates daemon process lifecycle, polling, waiting, and process group termination fallback.

        If the socket file does not exist or the daemon is already stopped, this method
        handles it cleanly and returns immediately.
        """
        graceful_ipc_succeeded = False

        # 1. First attempt the graceful socket-based IPC StopRequest shutdown
        if self.socket_path.exists():
            try:
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
                    s.settimeout(timeout)
                    try:
                        s.connect(str(self.socket_path))
                        stop_req = StopRequest()
                        s.sendall(serialize(stop_req).encode("utf-8"))
                        # Read until EOF.
                        while True:
                            data = s.recv(4096)
                            if not data:
                                break
                        graceful_ipc_succeeded = True
                    except (ConnectionRefusedError, FileNotFoundError):
                        pass
            except Exception:
                pass
            finally:
                self.socket_path.unlink(missing_ok=True)

        # 2. If self._proc is not None, wait for it or fallback to killing it
        if self._proc is not None:
            if graceful_ipc_succeeded:
                try:
                    self._proc.wait(timeout=timeout)
                except (subprocess.TimeoutExpired, Exception):
                    pass
            _cleanup_process(self._proc)
            self._proc = None

    async def stop(self, timeout: float = 5.0) -> None:
        """Sends a StopRequest to the daemon, drains the connection, and waits for EOF.

        If the socket file does not exist or the daemon is already stopped, this method
        handles it cleanly and returns immediately.
        """
        await asyncio.to_thread(self.stop_sync, timeout)
