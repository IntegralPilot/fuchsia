# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Fuchsia USB peripheral configuration management library.

Provides utilities for querying, parsing, applying, and restoring USB peripheral
configurations on Fuchsia devices via Honeydew FFX transports and `usb-cli`,
as well as extracting target device identifiers directly from Honeydew device
objects.
"""

import asyncio
import collections.abc
import concurrent.futures
import inspect
import json
import logging
import os
import re
import shlex
import socket
import time
from typing import Any

_LOGGER: logging.Logger = logging.getLogger(__name__)
_DONE_TOKEN: str = "[usb-cli:DONE]"
_IDENT_RE: re.Pattern[str] = re.compile(r"^[a-zA-Z0-9_-]+$")


def _invoke_maybe_async(
    callable_obj: collections.abc.Callable[[], Any],
    timeout_sec: float = 30.0,
) -> Any:
    """Invoke a sync callable or async coroutine function safely."""
    try:
        loop = asyncio.get_running_loop()
    except RuntimeError:
        loop = None

    def _call_worker() -> Any:
        val = callable_obj()
        if inspect.iscoroutine(val):
            return asyncio.run(val)
        return val

    if loop and loop.is_running():
        pool = concurrent.futures.ThreadPoolExecutor(max_workers=1)
        try:
            return pool.submit(_call_worker).result(timeout=timeout_sec)
        finally:
            pool.shutdown(wait=False, cancel_futures=True)
    else:
        return _call_worker()


def get_dut_serial(dut: Any) -> str | None:
    """Extract serial number or hardware identifier from a Honeydew
    FuchsiaDevice.

    Attempts multiple Honeydew properties and affordances:
    1. Synchronous or cached `dut.serial_number` attribute (or async coroutine).
    2. `dut._device_info.serial_number` or `dut._device_info.name`.
    3. `dut.ffx.get_target_information()` serial.
    4. `dut.fastboot.get_var("serialno")` if fastboot transport is active.

    Args:
        dut: FuchsiaDevice object from Honeydew.

    Returns:
        The device serial number string, or None if unavailable.
    """
    if dut is None:
        return None

    # 0. Check if target_serial was already resolved on dut or test instance
    target_serial = getattr(dut, "target_serial", None)
    if target_serial:
        return str(target_serial).strip()

    # 1. Check direct serial_number attribute/property
    if hasattr(dut, "serial_number"):
        sn = getattr(dut, "serial_number")
        if isinstance(sn, str) and sn:
            return sn.strip()
        elif callable(sn):
            try:
                res = _invoke_maybe_async(sn, timeout_sec=5.0)
                if isinstance(res, str) and res:
                    return res.strip()
            except Exception as e:
                _LOGGER.debug("Failed calling dut.serial_number(): %s", e)

    # 2. Check _device_info on FuchsiaDevice (ensure single serial, not
    # multi-device string).
    # Access Honeydew's internal _device_info attribute as a low-level fallback
    # when inspecting hardware DUT serials in end-to-end USB test setups.
    device_info = getattr(dut, "_device_info", None)
    if device_info:
        sn = getattr(device_info, "serial_number", None)
        if sn:
            sn_str = str(sn).strip()
            if "\n" not in sn_str and " " not in sn_str:
                return sn_str

    # 3. Check ffx target info if available (supports dict and TargetInfoData).
    try:
        if hasattr(dut, "ffx") and hasattr(dut.ffx, "get_target_information"):
            info = dut.ffx.get_target_information()
            if isinstance(info, dict) and info.get("serial"):
                return str(info["serial"]).strip()
            dev_sn = getattr(
                getattr(info, "device", None), "serial_number", None
            )
            if dev_sn:
                return str(dev_sn).strip()
    except Exception as e:
        _LOGGER.debug("Failed querying ffx target info for serial: %s", e)

    # 4. Check fastboot transport if available
    try:
        if hasattr(dut, "fastboot") and hasattr(dut.fastboot, "get_var"):
            fastboot_sn = dut.fastboot.get_var("serialno")
            if fastboot_sn:
                return str(fastboot_sn).strip()
    except Exception as e:
        _LOGGER.debug("Failed querying fastboot serial: %s", e)

    return None


def get_usb_config(dut: Any) -> str:
    """Retrieve the current USB peripheral policy configuration from the DUT.

    Queries the device via SSH first while online, falling back to serial socket
    if target networking is disconnected or offline.

    Args:
        dut: Fuchsia DUT device object.

    Returns:
        The raw configuration string from usb-cli get-config.
    """
    _LOGGER.info("Querying current USB peripheral configuration on target...")

    # 1. Primary: Query via SSH while target is online and CDC/VSOCK is active
    try:
        if hasattr(dut, "ffx") and hasattr(dut.ffx, "run_ssh_cmd"):
            ssh_res = dut.ffx.run_ssh_cmd("usb-cli get-config")
            if (
                ssh_res
                and "Target does not connect via networking" not in str(ssh_res)
                and "BUG:" not in str(ssh_res)
            ):
                res_str = str(ssh_res).strip()
                if _DONE_TOKEN in res_str or "{" in res_str:
                    return res_str
    except Exception as e:
        _LOGGER.debug("Failed querying usb config via SSH (%s)", e)

    # 2. Fallback: Serial command if target offline or network disconnected.
    res = _send_serial_command("usb-cli get-config", timeout_sec=5.0, dut=dut)
    if res and (
        _DONE_TOKEN in res
        or "{" in res
        or "functions" in res
        or "configurations" in res
    ):
        return res.strip()

    return ""


def normalize_usb_functions(functions: list[str]) -> list[str]:
    """Normalize function names, deduplicating while preserving order.

    Args:
        functions: List of USB peripheral function names (e.g. ['cdc',
            'vsock']).

    Returns:
        A deduplicated and normalized list of function names.
    """
    if not functions:
        return []
    return list(dict.fromkeys(functions))


def parse_usb_config_functions(config_str: str) -> list[str]:
    """Parse list of active function names from usb-cli get-config output.

    Args:
        config_str: Output string from usb-cli get-config.

    Returns:
        List of function names (e.g. ['cdc', 'adb', 'test']).
    """
    raw_functions: list[str] = []
    try:
        # usb-cli get-config outputs JSON: {"functions": ["cdc", "adb", ...]}
        start_idx = config_str.find("{")
        end_idx = config_str.rfind("}")
        if start_idx != -1 and end_idx != -1 and start_idx < end_idx:
            data = json.loads(config_str[start_idx : end_idx + 1])
            if "configurations" in data and isinstance(
                data["configurations"], list
            ):
                raw_functions = [
                    str(f)
                    for cfg in data["configurations"]
                    if isinstance(cfg, list)
                    for f in cfg
                ]
            elif "functions" in data and isinstance(data["functions"], list):
                raw_functions = [str(f) for f in data["functions"]]
    except (json.JSONDecodeError, KeyError, TypeError) as e:
        _LOGGER.debug(
            "Failed to parse JSON configuration (%s) from: %s",
            e,
            config_str,
        )

    if not raw_functions:
        # Fallback to comma-separated splitting with identifier validation
        cleaned = (
            config_str.strip()
            .replace(_DONE_TOKEN, "")
            .replace("[", "")
            .replace("]", "")
            .replace('"', "")
        )
        raw_functions = [
            p.strip()
            for p in cleaned.split(",")
            if p.strip()
            and p.strip() != "usb-cli:DONE"
            and _IDENT_RE.match(p.strip())
        ]

    return normalize_usb_functions(raw_functions)


def _send_serial_command(
    cmd: str,
    timeout_sec: float = 5.0,
    dut: Any | None = None,
) -> str | None:
    """Send a command over the Honeydew device serial transport.

    Args:
        cmd: Shell command string to execute over the serial socket.
        timeout_sec: Maximum duration in seconds to wait for command execution.
        dut: Honeydew FuchsiaDevice instance managing the target device.

    Returns:
        The captured command output string if successful, else None.
    """
    if dut is None:
        return None

    try:
        serial = getattr(dut, "serial", None)
        if serial is None:
            return None

        # Access internal _socket_path on Honeydew Serial transport for direct
        # socket connection fallback in low-level end-to-end USB test setup.
        sp = getattr(serial, "_socket_path", None)
        if sp and os.path.exists(sp):
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
                s.settimeout(timeout_sec)
                s.connect(sp)
                payload = f"{cmd.strip()}\r".encode("utf-8")
                s.sendall(payload)
                buf = b""
                start = time.monotonic()
                while time.monotonic() - start < timeout_sec:
                    try:
                        data = s.recv(1024)
                        if not data:
                            break
                        buf += data
                        if (
                            b"[usb-cli:DONE]" in buf
                            or b"[usb-cli:ERROR]" in buf
                            or buf.strip().endswith(b"$")
                        ):
                            break
                    except socket.timeout:
                        break
            if buf:
                res = buf.decode("utf-8", errors="replace")
                _LOGGER.info(
                    "Sent serial command '%s' via dut.serial socket: %s",
                    cmd,
                    res.strip(),
                )
                return res
        elif hasattr(serial, "send") and hasattr(serial, "read"):
            serial.send(f"{cmd.strip()}\r")
            text_buf: str = ""
            start_time = time.monotonic()
            while time.monotonic() - start_time < timeout_sec:
                chunk = serial.read(1024)
                if chunk:
                    if isinstance(chunk, bytes):
                        text_buf += chunk.decode("utf-8", errors="replace")
                    else:
                        text_buf += str(chunk)
                    if _DONE_TOKEN in text_buf or text_buf.strip().endswith(
                        "$"
                    ):
                        break
                time.sleep(0.1)
            if text_buf:
                _LOGGER.info(
                    "Sent serial command '%s' via dut.serial: %s",
                    cmd,
                    text_buf.strip(),
                )
                return text_buf
    except Exception as e:
        _LOGGER.debug("Serial transport is unavailable (%s)", e)
        return None

    return None


def set_usb_config(
    dut: Any, config: str, reboot_if_needed: bool = True
) -> None:
    """Apply a new USB peripheral configuration on the DUT via usb-cli.

    Args:
        dut: Fuchsia DUT device object.
        config: Function or configuration string (e.g. 'cdc,adb',
            'sourcesink', 'loopback').
        reboot_if_needed: If True and restoring non-test config, triggers
            reboot so CDC Ethernet re-enumerates.
    """
    _LOGGER.info(
        "Applying USB peripheral configuration '%s' on target...", config
    )

    applied = False
    ssh_res: Any | None = None
    ssh_err: Exception | None = None
    quoted_cfg = shlex.quote(config)

    # 1. Attempt serial command if serial transport is available
    serial_res = _send_serial_command(
        f"usb-cli set-config {quoted_cfg}", timeout_sec=8.0, dut=dut
    )
    if serial_res and (
        _DONE_TOKEN in serial_res or "Cold reboot" in serial_res
    ):
        applied = True

    # 2. Fallback to SSH via run_ssh_cmd if serial command did not apply it
    if not applied:
        ffx = getattr(dut, "ffx", None)
        if ffx and hasattr(ffx, "run_ssh_cmd"):
            try:
                ssh_res = ffx.run_ssh_cmd(
                    cmd=f"usb-cli set-config {quoted_cfg}"
                )
                if ssh_res and (
                    _DONE_TOKEN in str(ssh_res) or "Cold reboot" in str(ssh_res)
                ):
                    applied = True
                    _LOGGER.info(
                        "Applied USB peripheral configuration '%s' via SSH",
                        config,
                    )
            except Exception as e:
                ssh_err = e
                _LOGGER.debug("SSH execution failed: %s", e)

    is_test = any(
        f in config.lower() for f in ("sourcesink", "loopback", "test")
    )

    # If test config failed to apply over serial and SSH, fail fast instead
    # of hanging later.
    if not applied and is_test:
        raise RuntimeError(
            f"Failed to apply test USB config '{config}' over serial "
            f"console or SSH: serial_res={serial_res!r}, ssh_res={ssh_res!r}"
        ) from ssh_err

    # If target was offline or restore failed, coordinate reboot to reload
    # default functions.
    if not applied and not is_test:
        _LOGGER.info(
            "Device unresponsive during restore of '%s'; triggering reboot "
            "to restore default functions...",
            config,
        )
        if hasattr(dut, "reboot"):
            try:
                _invoke_maybe_async(dut.reboot)
                return
            except Exception as e:
                _LOGGER.warning("dut.reboot() failed: %s", e)
        _send_serial_command("dm reboot", timeout_sec=3.0, dut=dut)
        return

    # If restoring non-test config and reboot was requested, reboot to
    # rebind CDC interface.
    if applied and reboot_if_needed and not is_test:
        _LOGGER.info("Rebooting device to rebind CDC network interface...")
        if hasattr(dut, "reboot"):
            try:
                _invoke_maybe_async(dut.reboot)
                return
            except Exception as e:
                _LOGGER.warning("dut.reboot() failed: %s", e)
        if _send_serial_command("dm reboot", timeout_sec=3.0, dut=dut) is None:
            ffx = getattr(dut, "ffx", None)
            if ffx and hasattr(ffx, "run_ssh_cmd"):
                try:
                    ffx.run_ssh_cmd("dm reboot")
                except Exception as reboot_err:
                    _LOGGER.debug("SSH reboot failed: %s", reboot_err)
