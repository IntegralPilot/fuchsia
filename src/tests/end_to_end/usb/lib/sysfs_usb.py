# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Generic Linux sysfs and character device (/dev/bus/usb) USB discovery library.

Provides helper routines for scanning Linux sysfs to locate USB devices by VID,
PID, and serial number (or via Honeydew FuchsiaDevice), resolving devfs character
device nodes (/dev/bus/usb/BBB/DDD), querying device speeds, and querying active
configuration values via sysfs.
"""

import logging
import os
import time
from typing import Any, Optional, Sequence, Tuple

from usb_lib.usb_config import get_dut_serial

_LOGGER: logging.Logger = logging.getLogger(__name__)

SYSFS_USB_DEVICES_PATH = "/sys/bus/usb/devices"


def find_usb_device_node(
    dut: Optional[Any] = None,
    target_vid: Optional[int] = None,
    target_pid: Optional[int] = None,
    target_serial: Optional[str] = None,
    known_devices: Optional[Sequence[Tuple[int, int]]] = None,
) -> Optional[str]:
    """Scan /sys/bus/usb/devices/ to find matching /dev/bus/usb/BBB/DDD character device.

    Args:
        dut: Optional Honeydew FuchsiaDevice object (serial will be extracted automatically if provided).
        target_vid: Optional USB Vendor ID to match.
        target_pid: Optional USB Product ID to match.
        target_serial: Optional USB device serial number string override.
        known_devices: Optional list of (VID, PID) pairs to match against.

    Returns:
        The /dev/bus/usb/BBB/DDD character device path if found, or None.
    """
    if not os.path.exists(SYSFS_USB_DEVICES_PATH):
        return None

    # If target_serial wasn't explicitly supplied, try extracting it from dut
    if target_serial is None and dut is not None:
        target_serial = get_dut_serial(dut)

    try:
        entries = os.listdir(SYSFS_USB_DEVICES_PATH)
    except OSError as e:
        _LOGGER.debug(f"Error listing {SYSFS_USB_DEVICES_PATH}: {e}")
        return None

    for entry in entries:
        p = os.path.join(SYSFS_USB_DEVICES_PATH, entry)
        id_vendor_p = os.path.join(p, "idVendor")
        id_product_p = os.path.join(p, "idProduct")
        if os.path.exists(id_vendor_p) and os.path.exists(id_product_p):
            try:
                with open(id_vendor_p) as fv, open(id_product_p) as fp:
                    vid = int(fv.read().strip(), 16)
                    pid = int(fp.read().strip(), 16)
            except (ValueError, OSError):
                continue

            # Match by explicit VID/PID, known_devices list, or target_serial alone
            matches = False
            if target_vid is not None and target_pid is not None:
                if vid == target_vid and pid == target_pid:
                    matches = True
            elif target_vid is not None:
                if vid == target_vid:
                    matches = True
            elif target_pid is not None:
                if pid == target_pid:
                    matches = True
            elif known_devices is not None:
                if (vid, pid) in known_devices:
                    matches = True
            elif target_serial is not None:
                matches = True

            if not matches:
                continue

            # If target_serial is provided, verify matching serial number
            if target_serial:
                serial_p = os.path.join(p, "serial")
                if not os.path.exists(serial_p):
                    continue
                try:
                    with open(serial_p) as fs:
                        serial_val = fs.read().strip()
                    if serial_val != target_serial:
                        continue
                except OSError:
                    continue

            busnum_p = os.path.join(p, "busnum")
            devnum_p = os.path.join(p, "devnum")
            if os.path.exists(busnum_p) and os.path.exists(devnum_p):
                try:
                    with open(busnum_p) as fb, open(devnum_p) as fd:
                        busnum = int(fb.read().strip())
                        devnum = int(fd.read().strip())
                    dev_path = f"/dev/bus/usb/{busnum:03d}/{devnum:03d}"
                    if os.path.exists(dev_path) and os.access(
                        dev_path, os.R_OK | os.W_OK
                    ):
                        return dev_path
                except (ValueError, OSError):
                    continue
    return None


def wait_for_usb_device(
    dut: Optional[Any] = None,
    target_vid: Optional[int] = None,
    target_pid: Optional[int] = None,
    target_serial: Optional[str] = None,
    known_devices: Optional[Sequence[Tuple[int, int]]] = None,
    timeout_sec: float = 15.0,
    poll_interval_sec: float = 0.5,
) -> str:
    """Poll until the matching USB device node is enumerated and accessible.

    Args:
        dut: Optional Honeydew FuchsiaDevice object (serial will be extracted automatically if provided).
        target_vid: Optional USB Vendor ID to match.
        target_pid: Optional USB Product ID to match.
        target_serial: Optional USB device serial number to match.
        known_devices: Optional list of (VID, PID) pairs to match against.
        timeout_sec: Maximum time to wait in seconds.
        poll_interval_sec: Polling interval in seconds.

    Returns:
        The character device path (/dev/bus/usb/BBB/DDD).

    Raises:
        TimeoutError: If the device fails to enumerate within timeout_sec.
    """
    if target_serial is None and dut is not None:
        target_serial = get_dut_serial(dut)

    start_time = time.monotonic()
    desc_parts = []
    if target_vid is not None:
        desc_parts.append(f"vid=0x{target_vid:04x}")
    if target_pid is not None:
        desc_parts.append(f"pid=0x{target_pid:04x}")
    if target_serial is not None:
        desc_parts.append(f"serial={target_serial}")
    if known_devices is not None:
        desc_parts.append(f"known_devices={len(known_devices)}")
    desc = ", ".join(desc_parts) if desc_parts else "any device"

    _LOGGER.info(
        f"Waiting up to {timeout_sec}s for USB device ({desc}) enumeration..."
    )

    while time.monotonic() - start_time < timeout_sec:
        dev_node = find_usb_device_node(
            dut=None,
            target_vid=target_vid,
            target_pid=target_pid,
            target_serial=target_serial,
            known_devices=known_devices,
        )
        if dev_node:
            _LOGGER.info(f"Found active USB device node: {dev_node}")
            return dev_node
        time.sleep(poll_interval_sec)

    raise TimeoutError(
        f"Timed out after {timeout_sec}s waiting for USB device ({desc}) to enumerate"
    )


def get_usb_device_sysfs_path(busnum: int, devnum: int) -> Optional[str]:
    """Find the sysfs path corresponding to a USB bus and device number.

    Args:
        busnum: USB bus number (e.g. 1).
        devnum: USB device address/number on the bus (e.g. 33).

    Returns:
        The directory path under /sys/bus/usb/devices/ if found, else None.
    """
    if not os.path.exists(SYSFS_USB_DEVICES_PATH):
        return None
    try:
        entries = os.listdir(SYSFS_USB_DEVICES_PATH)
    except OSError as e:
        _LOGGER.debug(f"Error listing sysfs device paths: {e}")
        return None

    for entry in entries:
        p = os.path.join(SYSFS_USB_DEVICES_PATH, entry)
        devnum_p = os.path.join(p, "devnum")
        busnum_p = os.path.join(p, "busnum")
        if os.path.exists(devnum_p) and os.path.exists(busnum_p):
            try:
                with open(devnum_p) as f1, open(busnum_p) as f2:
                    d = int(f1.read().strip())
                    b = int(f2.read().strip())
                if d == devnum and b == busnum:
                    return p
            except (ValueError, OSError):
                continue
    return None


def _resolve_sysfs_path(dev_node_or_sysfs_path: str) -> Optional[str]:
    """Resolve a device node or sysfs path to its canonical sysfs directory.

    Args:
        dev_node_or_sysfs_path: A device node path (e.g. '/dev/bus/usb/001/033')
            or a direct sysfs directory path.

    Returns:
        The corresponding /sys/bus/usb/devices/... directory path, or None
        if unresolvable.
    """
    if not dev_node_or_sysfs_path.startswith("/dev/bus/usb/"):
        return (
            dev_node_or_sysfs_path
            if os.path.exists(dev_node_or_sysfs_path)
            else None
        )
    parts = dev_node_or_sysfs_path.rstrip("/").split("/")
    if len(parts) < 2:
        return None
    try:
        busnum = int(parts[-2])
        devnum = int(parts[-1])
        return get_usb_device_sysfs_path(busnum, devnum)
    except ValueError:
        return None


def get_usb_device_speed(dev_node_or_sysfs_path: str) -> str:
    """Read the negotiated USB device speed string from sysfs.

    Args:
        dev_node_or_sysfs_path: A device node path or sysfs directory path.

    Returns:
        The speed string (e.g. 'high', 'super', 'full', 'low'), or 'unknown'
        if the speed cannot be determined from sysfs.
    """
    sysfs_path = _resolve_sysfs_path(dev_node_or_sysfs_path)
    if not sysfs_path:
        return "unknown"

    speed_file = os.path.join(sysfs_path, "speed")
    if os.path.exists(speed_file):
        try:
            with open(speed_file) as f:
                spd = f.read().strip()
                if spd == "480":
                    return "high"
                elif spd in ("5000", "10000"):
                    return "super"
                elif spd == "12":
                    return "full"
                elif spd == "1.5":
                    return "low"
                return spd
        except OSError:
            pass
    return "unknown"


def get_usb_device_configuration(dev_node_or_sysfs_path: str) -> Optional[int]:
    """Read active bConfigurationValue from sysfs for a USB device.

    Args:
        dev_node_or_sysfs_path: A device node path or sysfs directory path.

    Returns:
        The active configuration value (e.g. 1 for Source/Sink, 2 for
        Loopback), or None if unavailable.
    """
    sysfs_path = _resolve_sysfs_path(dev_node_or_sysfs_path)
    if not sysfs_path:
        return None

    cfg_file = os.path.join(sysfs_path, "bConfigurationValue")
    if os.path.exists(cfg_file):
        try:
            with open(cfg_file) as f:
                return int(f.read().strip())
        except (ValueError, OSError):
            pass
    return None
