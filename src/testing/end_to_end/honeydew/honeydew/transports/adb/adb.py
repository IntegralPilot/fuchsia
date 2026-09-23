# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Provides methods for Host-(Fuchsia)Target interactions via ADB."""

import asyncio
import atexit
import glob
import logging
import os
import shutil
import stat
import tempfile
from collections.abc import Awaitable, Callable
from importlib import resources

from honeydew import errors
from honeydew.affordances.affordance import AsyncLazyReady, ensure_ready
from honeydew.transports.adb import errors as adb_errors
from honeydew.transports.adb.adb_server import AdbServer
from honeydew.utils import decorators

_ADB_PATH_ENV_VAR = "HONEYDEW_ADB_OVERRIDE"

_LOGGER: logging.Logger = logging.getLogger(__name__)


def _get_adb_binary() -> str:
    """Returns the path to the `adb` binary.

    Prefers resolving from environment variable `HONEYDEW_ADB_OVERRIDE` if
    provided; otherwise, extract from Python resource, set permissions to
    executable, and store on disk. If running outside the build system without
    data resources, falls back to `PATH`.

    Returns:
        Absolute path to `adb` binary.

    Raises:
        adb_errors.InitializationError: If ADB binary is not found.
    """

    bin_path: str | None = os.getenv(_ADB_PATH_ENV_VAR)
    if bin_path is not None:
        return bin_path

    try:
        from honeydew import data  # type: ignore[attr-defined,unused-ignore]

        bin_fd = tempfile.NamedTemporaryFile(suffix="adb", delete=False)
        bin_path = bin_fd.name
        bin_fd.close()
        with resources.as_file(resources.files(data).joinpath("adb")) as f:
            f.chmod(f.stat().st_mode | stat.S_IEXEC)
            shutil.copy2(f, bin_path)
        atexit.register(os.unlink, bin_path)
        return bin_path
    except (ImportError, FileNotFoundError, AttributeError, TypeError):
        pass

    bin_path = shutil.which("adb")
    if bin_path is not None:
        return bin_path

    raise adb_errors.InitializationError(
        "ADB binary was not found in Python data resources, in PATH, or via "
        f"`{_ADB_PATH_ENV_VAR}`."
    )


def _find_usb_device_path(target_serial: str | None = None) -> str | None:
    """Finds the sysfs device path for the given serial (or Google VID)."""
    for dev_path in glob.glob("/sys/bus/usb/devices/*"):
        try:
            if target_serial:
                serial_file = os.path.join(dev_path, "serial")
                if not os.path.exists(serial_file):
                    continue
                with open(serial_file, "r", encoding="utf-8") as f:
                    ser = f.read().strip()
                if ser == target_serial:
                    return dev_path
            else:
                vendor_file = os.path.join(dev_path, "idVendor")
                if not os.path.exists(vendor_file):
                    continue
                with open(vendor_file, "r", encoding="utf-8") as f:
                    vid = f.read().strip()
                if vid == "18d1":
                    return dev_path
        except OSError:
            pass
    return None


def _has_adb_interface(dev_path: str) -> bool:
    """Checks if the given sysfs USB device path exposes an ADB interface."""
    for intf_path in glob.glob(os.path.join(dev_path, "*:*.*/bInterfaceClass")):
        intf_dir = os.path.dirname(intf_path)
        try:
            with open(
                os.path.join(intf_dir, "bInterfaceClass"), "r", encoding="utf-8"
            ) as f:
                cls = f.read().strip()
            with open(
                os.path.join(intf_dir, "bInterfaceSubClass"),
                "r",
                encoding="utf-8",
            ) as f:
                subcls = f.read().strip()
            with open(
                os.path.join(intf_dir, "bInterfaceProtocol"),
                "r",
                encoding="utf-8",
            ) as f:
                proto = f.read().strip()

            if cls == "ff" and subcls == "42" and proto == "01":
                return True
        except OSError:
            pass
    return False


def _check_adb_sysfs(target_serial: str | None = None) -> bool:
    """Checks sysfs to see if there is an ADB interface for the given serial."""
    dev_path = _find_usb_device_path(target_serial)
    if dev_path:
        return _has_adb_interface(dev_path)
    return False


class Adb(AsyncLazyReady):
    """Provides methods for Host-(Fuchsia)Target interactions via ADB.

    Note: The USB detection logic (`is_supported()`) relies on Linux-specific
    sysfs (`/sys/bus/usb/devices/*`) and is only supported on Linux hosts.

    Args:
        device_name: Fuchsia device name.
        serial_number: Optional serial number or async provider coroutine callback.
        run_isolated_server: Whether to run an isolated ADB server.
    """

    def __init__(
        self,
        device_name: str,
        serial_number: str | Callable[[], Awaitable[str]] | None = None,
        run_isolated_server: bool = False,
        vendor_keys_path: str | None = None,
    ) -> None:
        super().__init__()
        self._device_name: str = device_name
        self._serial_number_arg = serial_number
        self._serial_number: str | None = None
        self._adb_binary: str | None = None
        self._run_isolated_server: bool = run_isolated_server
        self._vendor_keys_path: str | None = vendor_keys_path
        self._adb_server: AdbServer | None = None

    async def _resolve_serial_number(self) -> str | None:
        """Resolves and caches the serial number."""
        if self._serial_number is None:
            if callable(self._serial_number_arg):
                try:
                    self._serial_number = await self._serial_number_arg()
                except Exception as e:
                    _LOGGER.warning(
                        "Failed to resolve serial number for ADB on '%s': %s",
                        self._device_name,
                        e,
                    )
            elif isinstance(self._serial_number_arg, str):
                self._serial_number = self._serial_number_arg
        return self._serial_number

    # TODO(b/559570418): Provide a mechanism so affordances using ADB can implement
    # verify_supported() in __init__(). Currently, is_supported() is async due to
    # serial number resolution.
    async def is_supported(self) -> bool:
        """Returns True if ADB is supported on the device."""
        serial = await self._resolve_serial_number()
        if not serial:
            return False
        return await asyncio.to_thread(_check_adb_sysfs, serial)

    async def make_ready(self) -> None:
        """Initializes the ADB transport."""
        if self._ready:
            return

        if not await self.is_supported():
            raise errors.NotSupportedError(
                f"ADB transport is not supported on {self._device_name}"
            )

        self._adb_binary = _get_adb_binary()

        serial = await self._resolve_serial_number()
        if serial is None:
            raise adb_errors.InitializationError(
                f"Serial number was not resolved for ADB transport on device '{self._device_name}'. "
                "A specific serial is required to ensure ADB commands target the correct device."
            )

        if self._run_isolated_server:
            _LOGGER.info(
                "Starting isolated ADB server for %s", self._device_name
            )
            self._adb_server = AdbServer(
                adb_binary_path=self._adb_binary,
                serial_id=serial,
                vendor_keys_path=self._vendor_keys_path,
            )
            await asyncio.to_thread(self._adb_server.start)
            _LOGGER.info("Waiting for device to be visible to ADB...")
            await self.run(["wait-for-device"])
            _LOGGER.info("Device is visible to ADB.")

        await super().make_ready()

    @ensure_ready
    @decorators.async_liveness_check
    async def run(
        self,
        cmd: list[str],
        timeout: float | None = None,
    ) -> str:
        """Runs an ADB command and returns the output.

        Args:
            cmd: ADB command as a list of strings (excluding 'adb' and '-s <serial>').
            timeout: Maximum amount of time in seconds to wait for the command to finish.
                Defaults to None (no time limit), which is recommended to avoid flakiness
                on slow or overloaded test environments. Only pass a timeout when explicitly
                required or for commands expected to fail fast.

        Returns:
            The combined stdout and stderr of the command.

        Raises:
            adb_errors.AdbCommandError: If the command fails or times out.
        """
        assert self._adb_binary is not None

        if timeout is not None:
            _LOGGER.info(
                "Timeout of %ss is set for ADB command '%s'. Note that timeouts "
                "can cause flakiness on overloaded test environments.",
                timeout,
                " ".join(cmd),
            )

        max_attempts = 3
        attempt = 1
        while True:
            # Construct adb_cmd inside the loop to ensure we use the updated port if restarted
            adb_cmd = [self._adb_binary]
            if self._adb_server:
                adb_cmd.extend(
                    [
                        "-H",
                        self._adb_server.host(),
                        "-P",
                        str(self._adb_server.port()),
                    ]
                )

            serial = await self._resolve_serial_number()
            if serial:
                adb_cmd.extend(["-s", serial])
            adb_cmd.extend(cmd)

            _LOGGER.debug(
                "Running ADB command (attempt %d/%d): %s",
                attempt,
                max_attempts,
                adb_cmd,
            )
            try:
                proc = await asyncio.create_subprocess_exec(
                    *adb_cmd,
                    stdout=asyncio.subprocess.PIPE,
                    stderr=asyncio.subprocess.STDOUT,
                )
                try:
                    stdout, _ = await asyncio.wait_for(
                        proc.communicate(),
                        timeout=timeout,
                    )
                except asyncio.TimeoutError:
                    proc.kill()
                    await proc.communicate()
                    raise
                output = stdout.decode("utf-8", errors="replace").strip()

                if proc.returncode != 0:
                    raise adb_errors.AdbCommandError(
                        f"ADB command '{adb_cmd}' failed with exit code "
                        f"{proc.returncode}: {output}"
                    )
                if self._adb_server:
                    self._adb_server.reset_restart_count()
                return output

            except asyncio.TimeoutError as err:
                if attempt < max_attempts and self._adb_server:
                    _LOGGER.warning(
                        "ADB command timed out. "
                        "Attempting to restart isolated ADB server and retry..."
                    )
                    await asyncio.to_thread(self._adb_server.restart)
                    await asyncio.sleep(5)
                    attempt += 1
                    continue
                raise adb_errors.AdbTimeoutError(
                    f"ADB command '{adb_cmd}' timed out after {timeout} seconds"
                ) from err

            except Exception as err:
                if isinstance(err, adb_errors.AdbError):
                    err_msg_lower = str(err).lower()
                    is_retriable = any(
                        x in err_msg_lower
                        for x in [
                            "offline",
                            "not found",
                            "unauthorized",
                            "connection refused",
                            "cannot connect",
                        ]
                    )
                    if (
                        is_retriable
                        and attempt < max_attempts
                        and self._adb_server
                    ):
                        _LOGGER.warning(
                            "ADB command failed with connection error: %s. "
                            "Attempting to restart isolated ADB server and retry...",
                            str(err),
                        )
                        await asyncio.to_thread(self._adb_server.restart)
                        await asyncio.sleep(5)
                        attempt += 1
                        continue
                    raise

                # Non-AdbError is not retriable
                raise adb_errors.AdbCommandError(
                    f"Failed to run ADB command '{adb_cmd}': {err}"
                ) from err

    async def close(self) -> None:
        """Cleans up the ADB transport."""
        if self._adb_server:
            _LOGGER.info(
                "Stopping isolated ADB server for %s", self._device_name
            )
            await asyncio.to_thread(self._adb_server.stop)
            self._adb_server = None
