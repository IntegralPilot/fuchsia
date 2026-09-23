#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Linux kernel usbtest ioctl backend for testusb.

Interfaces directly with Linux USB device nodes (/dev/bus/usb/BBB/DDD or
/dev/usbtest) using USBDEVFS ioctls to drive kernel usbtest driver test cases
and bidirectional loopback transfers.
"""

from __future__ import annotations

import collections.abc
import contextlib
import ctypes
import ctypes.util
import enum
import errno
import functools
import logging
import os
import time

from .backend import USBTestBackend
from .models import (
    ALL_TEST_CASES,
    EP0_GENERIC_TESTS,
    USB_DT_CONFIG,
    USB_DT_DEVICE,
    TestParams,
    TestResult,
    TestStatus,
    UsbDescriptorType,
)


@functools.cache
def _get_libc() -> ctypes.CDLL:
    """Load and return C standard library with ioctl types configured."""
    libc_name = ctypes.util.find_library("c") or "libc.so.6"
    libc = ctypes.CDLL(libc_name, use_errno=True)
    libc.ioctl.argtypes = [ctypes.c_int, ctypes.c_ulong, ctypes.c_void_p]
    libc.ioctl.restype = ctypes.c_int
    return libc


def _ioc(direction: int, io_type: int, seq_number: int, size: int) -> int:
    """Calculate a Linux ioctl command number from directional/type fields."""
    return (direction << 30) | (io_type << 8) | (seq_number << 0) | (size << 16)


def _ior(type_char: str, seq_number: int, size: int) -> int:
    """Calculate a Linux _IOR ioctl command number."""
    return _ioc(2, ord(type_char), seq_number, size)


def _iowr(type_char: str, seq_number: int, size: int) -> int:
    """Calculate a Linux _IOWR ioctl command number."""
    return _ioc(3, ord(type_char), seq_number, size)


def _io(type_char: str, seq_number: int) -> int:
    """Calculate a Linux _IO ioctl command number."""
    return _ioc(0, ord(type_char), seq_number, 0)


class UsbdevfsIoctl(ctypes.Structure):
    """Ctypes representation of struct usbdevfs_ioctl (<linux/usbdevice_fs.h>)."""

    _fields_ = [
        ("ifno", ctypes.c_int),
        ("ioctl_code", ctypes.c_int),
        ("data", ctypes.c_void_p),
    ]


class UsbdevfsSetinterface(ctypes.Structure):
    """Ctypes representation of struct usbdevfs_setinterface (<linux/usbdevice_fs.h>)."""

    _fields_ = [
        ("interface", ctypes.c_uint),
        ("altsetting", ctypes.c_uint),
    ]


class UsbdevfsBulktransfer(ctypes.Structure):
    """Ctypes representation of struct usbdevfs_bulktransfer (<linux/usbdevice_fs.h>)."""

    _fields_ = [
        ("ep", ctypes.c_uint),
        ("len", ctypes.c_uint),
        ("timeout", ctypes.c_uint),
        ("data", ctypes.c_void_p),
    ]


class UsbdevfsCtrltransfer(ctypes.Structure):
    """Ctypes representation of struct usbdevfs_ctrltransfer (<linux/usbdevice_fs.h>)."""

    _fields_ = [
        ("bRequestType", ctypes.c_uint8),
        ("bRequest", ctypes.c_uint8),
        ("wValue", ctypes.c_uint16),
        ("wIndex", ctypes.c_uint16),
        ("wLength", ctypes.c_uint16),
        ("timeout", ctypes.c_uint32),
        ("data", ctypes.c_void_p),
    ]


class UsbdevfsDisconnectClaim(ctypes.Structure):
    """Ctypes representation of struct usbdevfs_disconnect_claim (<linux/usbdevice_fs.h>)."""

    _fields_ = [
        ("interface", ctypes.c_uint),
        ("flags", ctypes.c_uint),
        ("driver", ctypes.c_char * 256),
    ]


class Timeval(ctypes.Structure):
    """Ctypes representation of struct timeval (<sys/time.h>)."""

    _fields_ = [
        ("tv_sec", ctypes.c_long),
        ("tv_usec", ctypes.c_long),
    ]


class UsbtestParam(ctypes.Structure):
    """Ctypes representation of struct usbtest_param (<linux/usb/test.h>)."""

    _fields_ = [
        ("test_num", ctypes.c_uint32),
        ("iterations", ctypes.c_uint32),
        ("length", ctypes.c_uint32),
        ("vary", ctypes.c_uint32),
        ("sglen", ctypes.c_uint32),
        ("duration", Timeval),
    ]


USBDEVFS_CONTROL = _iowr("U", 0, ctypes.sizeof(UsbdevfsCtrltransfer))
USBDEVFS_BULK = _iowr("U", 2, ctypes.sizeof(UsbdevfsBulktransfer))
USBDEVFS_SETINTERFACE = _ior("U", 4, ctypes.sizeof(UsbdevfsSetinterface))
USBDEVFS_SETCONFIGURATION = _ior("U", 5, ctypes.sizeof(ctypes.c_uint))
USBDEVFS_CLAIMINTERFACE = _ior("U", 15, ctypes.sizeof(ctypes.c_uint))
USBDEVFS_RELEASEINTERFACE = _ior("U", 16, ctypes.sizeof(ctypes.c_uint))
USBDEVFS_IOCTL = _iowr("U", 18, ctypes.sizeof(UsbdevfsIoctl))
USBTEST_REQUEST = _iowr("U", 100, ctypes.sizeof(UsbtestParam))
USBDEVFS_DISCONNECT_CLAIM = _ior(
    "U", 27, ctypes.sizeof(UsbdevfsDisconnectClaim)
)
USBDEVFS_DISCONNECT = _io("U", 22)
USBDEVFS_CONNECT = _io("U", 23)
USBDEVFS_RESET = _io("U", 20)
USBDEVFS_CLEAR_HALT = _ior("U", 21, ctypes.sizeof(ctypes.c_uint))
USBDEVFS_GET_SPEED = _io("U", 31)

USBDEVFS_DISCONNECT_CLAIM_IF_DRIVER = 0x01

USB_DIR_OUT = 0x00
USB_DIR_IN = 0x80


USB_ENDPOINT_XFER_CONTROL = 0x00
USB_ENDPOINT_XFER_ISOC = 0x01
USB_ENDPOINT_XFER_BULK = 0x02
USB_ENDPOINT_XFER_INT = 0x03
USB_ENDPOINT_XFERTYPE_MASK = 0x03


class MalformedDescriptorError(Exception):
    """Raised when a USB configuration descriptor payload is malformed."""


def iter_descriptors(
    config_descriptor: bytes,
) -> collections.abc.Iterator[bytes]:
    """Yield descriptor chunks from a raw USB configuration descriptor."""
    idx = 0
    total_len = len(config_descriptor)
    while idx < total_len:
        b_length = config_descriptor[idx]
        if b_length < 2 or idx + b_length > total_len:
            raise MalformedDescriptorError(
                f"Malformed descriptor at offset {idx}: length {b_length}, "
                f"total remaining {total_len - idx}"
            )
        yield config_descriptor[idx : idx + b_length]
        idx += b_length


def parse_endpoints_from_config_descriptor(
    config_descriptor: bytes,
) -> tuple[int | None, int | None, int | None]:
    """Walk raw USB config descriptor bytes to locate paired endpoints."""
    curr_intf = None
    intf_eps: dict[int, tuple[int | None, int | None]] = {}

    try:
        for descriptor in iter_descriptors(config_descriptor):
            if len(descriptor) < 2:
                continue
            b_descriptor_type = descriptor[1]
            match b_descriptor_type:
                case UsbDescriptorType.INTERFACE:
                    if len(descriptor) >= 9:
                        curr_intf = descriptor[2]
                        if curr_intf not in intf_eps:
                            intf_eps[curr_intf] = (None, None)
                case UsbDescriptorType.ENDPOINT:
                    if len(descriptor) >= 7 and curr_intf is not None:
                        ep_addr = descriptor[2]
                        ep_attr = descriptor[3]
                        if (
                            ep_attr & USB_ENDPOINT_XFERTYPE_MASK
                        ) == USB_ENDPOINT_XFER_BULK:
                            out_ep, in_ep = intf_eps[curr_intf]
                            if (ep_addr & USB_DIR_IN) != 0 and in_ep is None:
                                intf_eps[curr_intf] = out_ep, ep_addr
                            elif (ep_addr & USB_DIR_IN) == 0 and out_ep is None:
                                intf_eps[curr_intf] = ep_addr, in_ep
                case _:
                    pass
    except MalformedDescriptorError as err:
        _LOGGER.warning(
            "Malformed descriptor encountered while parsing USB endpoints: %s",
            err,
        )
        return None, None, None

    for ifnum, (out_ep, in_ep) in intf_eps.items():
        if out_ep is not None and in_ep is not None:
            assert ifnum is not None
            return out_ep, in_ep, ifnum

    for ifnum, (out_ep, in_ep) in intf_eps.items():
        if out_ep is not None or in_ep is not None:
            assert ifnum is not None
            return out_ep, in_ep, ifnum

    return None, None, None


_LOGGER = logging.getLogger(__name__)

_MAX_USB_INTERFACES: int = 8

# Standard USB Request Codes (USB 2.0 Spec Table 9-4)
_USB_REQ_GET_STATUS: int = 0x00
_USB_REQ_CLEAR_FEATURE: int = 0x01
_USB_REQ_SET_FEATURE: int = 0x03
_USB_REQ_SET_ADDRESS: int = 0x05
_USB_REQ_GET_DESCRIPTOR: int = 0x06
_USB_REQ_SET_DESCRIPTOR: int = 0x07
_USB_REQ_GET_CONFIGURATION: int = 0x08
_USB_REQ_SET_CONFIGURATION: int = 0x09
_USB_REQ_GET_INTERFACE: int = 0x0A
_USB_REQ_SET_INTERFACE: int = 0x0B
_USB_REQ_SYNCH_FRAME: int = 0x0C

# USB Request Types and Recipients
_USB_TYPE_STANDARD: int = 0x00
_USB_RECIP_DEVICE: int = 0x00
_USB_RECIP_INTERFACE: int = 0x01
_USB_RECIP_ENDPOINT: int = 0x02

_USB_DT_DEVICE_SIZE: int = 18
_USB_DT_CONFIG_SIZE: int = 9
_DEFAULT_CTRL_TIMEOUT_MS: int = 1000

_SYSFS_SPEED_MAP: dict[str, str] = {
    "1.5": "low",
    "12": "full",
    "480": "high",
    "5000": "super",
    "10000": "super",
}

_USB_SPEED_NAMES: tuple[str, ...] = (
    "unknown",
    "low",
    "full",
    "high",
    "wireless",
    "super",
    "super-plus",
)

_EP0_GENERIC_TESTS: frozenset[int] = EP0_GENERIC_TESTS
_ZERO_BYTE_TRANSFER_TESTS: frozenset[int] = frozenset({0, 9, 10, 13, 14, 21})
_SCATTER_GATHER_TESTS: frozenset[int] = frozenset(
    {5, 6, 7, 8, 11, 12, 27, 28, 30, 31}
)


def _make_ctrl_transfer(
    request_type: int,
    request: int,
    value: int = 0,
    index: int = 0,
    length: int = 0,
    timeout_ms: int = _DEFAULT_CTRL_TIMEOUT_MS,
    data_ptr: int = 0,
) -> UsbdevfsCtrltransfer:
    """Builds and initializes a UsbdevfsCtrltransfer structure."""
    ctrl = UsbdevfsCtrltransfer()
    ctrl.bRequestType = request_type
    ctrl.bRequest = request
    ctrl.wValue = value
    ctrl.wIndex = index
    ctrl.wLength = length
    ctrl.timeout = timeout_ms
    ctrl.data = data_ptr
    return ctrl


def _make_get_descriptor_transfer(
    desc_type: int,
    desc_index: int = 0,
    length: int = 256,
    timeout_ms: int = _DEFAULT_CTRL_TIMEOUT_MS,
    data_ptr: int = 0,
) -> UsbdevfsCtrltransfer:
    """Builds a UsbdevfsCtrltransfer for standard GET_DESCRIPTOR."""
    return _make_ctrl_transfer(
        request_type=USB_DIR_IN | _USB_TYPE_STANDARD | _USB_RECIP_DEVICE,
        request=_USB_REQ_GET_DESCRIPTOR,
        value=(desc_type << 8) | (desc_index & 0xFF),
        index=0,
        length=length,
        timeout_ms=timeout_ms,
        data_ptr=data_ptr,
    )


def _make_get_configuration_transfer(
    timeout_ms: int = _DEFAULT_CTRL_TIMEOUT_MS,
    data_ptr: int = 0,
) -> UsbdevfsCtrltransfer:
    """Builds a UsbdevfsCtrltransfer for standard GET_CONFIGURATION."""
    return _make_ctrl_transfer(
        request_type=USB_DIR_IN | _USB_TYPE_STANDARD | _USB_RECIP_DEVICE,
        request=_USB_REQ_GET_CONFIGURATION,
        value=0,
        index=0,
        length=1,
        timeout_ms=timeout_ms,
        data_ptr=data_ptr,
    )


class LoopbackTestId(enum.IntEnum):
    """Test IDs for userspace bidirectional bulk loopback tests."""

    BULK_FIXED = 32
    BULK_VARY = 33
    BULK_SHORT = 34
    BULK_ZLP = 35


class USBTestIoctlBackend(USBTestBackend):
    """Linux usbtest ioctl backend.

    Interfaces via /dev/bus/usb/BBB/DDD or /dev/usbtest.

    Attributes:
        device_path: Optional path to the USB character device node.
        ifnum: Target USB interface number for test execution.
        is_open: Boolean indicating whether the device is currently opened.
        speed_name: String name of the device operating speed.
        current_config: Currently active configuration value.
    """

    def __init__(self, device_path: str | None = None, ifnum: int = 0) -> None:
        """Initializes the USBTestIoctlBackend.

        Args:
            device_path: Optional path to the USB character device.
            ifnum: Target USB interface number for test execution.
        """
        super().__init__(device_path)
        self.ifnum = ifnum
        self._fd: int | None = None
        self._maxpacket: int | None = None

    def open(self) -> None:
        """Opens the USB character device node and reads device parameters.

        Raises:
            RuntimeError: If no suitable USB device can be discovered, or if
                opening the device node fails due to permissions or OS errors.
        """
        if self.is_open:
            return

        if not self.device_path:
            devices = self.discover_devices()
            if not devices:
                raise RuntimeError(
                    "No USB test devices found on /dev/bus/usb. "
                    "Specify -D <path> or ensure a test gadget is connected."
                )
            self.device_path = devices[0]

        assert self.device_path is not None
        try:
            self._fd = os.open(self.device_path, os.O_RDWR)
        except OSError as e:
            raise RuntimeError(
                f"Failed to open USB device '{self.device_path}': {e}. "
                "Ensure you have write permissions (e.g., sudo chmod 666)."
            ) from e

        self.is_open = True
        try:
            self.speed_name = self.get_speed()
            self.current_config = self.get_configuration()
        except Exception:
            self.is_open = False
            os.close(self._fd)
            self._fd = None
            raise

    def close(self) -> None:
        """Closes the device node and restores kernel driver bindings."""
        if self._fd is not None:
            try:
                # Re-attach kernel drivers across interfaces before closing fd
                self._reconnect_kernel_drivers()
            except OSError as err:
                _LOGGER.debug(
                    "Failed to reconnect kernel drivers on close: %s", err
                )
            finally:
                try:
                    os.close(self._fd)
                except OSError as err:
                    _LOGGER.debug(
                        "Error closing file descriptor %d: %s", self._fd, err
                    )
                self._fd = None
        self.is_open = False

    def _reconnect_kernel_drivers(self) -> None:
        """Re-bind detached kernel drivers (usbtest) across all interfaces."""
        if self._fd is None:
            return
        cdll = _get_libc()
        for ifnum in range(_MAX_USB_INTERFACES):
            try:
                wrapper = UsbdevfsIoctl()
                wrapper.ifno = ifnum
                wrapper.ioctl_code = USBDEVFS_CONNECT
                wrapper.data = None
                res = cdll.ioctl(
                    self._fd, USBDEVFS_IOCTL, ctypes.byref(wrapper)
                )
                if res < 0:
                    err = ctypes.get_errno()
                    _LOGGER.debug(
                        "USBDEVFS_CONNECT on iface %d error %d: %s",
                        ifnum,
                        err,
                        os.strerror(err),
                    )
            except OSError as err:
                _LOGGER.debug(
                    "Exception during USBDEVFS_CONNECT on iface %d: %s",
                    ifnum,
                    err,
                )

    def get_speed(self) -> str:
        """Queries the USB connection speed of the target device.

        Returns:
            A string describing connection speed ('low', 'full', 'high',
            'super', 'super-plus', or 'unknown').
        """
        if self._fd is None:
            return "unknown"

        try:
            # USBDEVFS_GET_SPEED returns an integer speed code directly
            res = _get_libc().ioctl(self._fd, USBDEVFS_GET_SPEED, 0)
            if 0 <= res < len(_USB_SPEED_NAMES):
                return _USB_SPEED_NAMES[res]
        except OSError as err:
            _LOGGER.debug("USBDEVFS_GET_SPEED ioctl failed: %s", err)

        # Sysfs fallback
        sysfs_path = self._get_sysfs_device_path()
        if sysfs_path:
            speed_file = os.path.join(sysfs_path, "speed")
            if os.path.exists(speed_file):
                try:
                    with open(speed_file, encoding="utf-8") as f:
                        spd = f.read().strip()
                        return _SYSFS_SPEED_MAP.get(spd, "unknown")
                except OSError as err:
                    _LOGGER.debug("Failed reading sysfs speed file: %s", err)
        return "unknown"

    def _get_sysfs_device_path(self) -> str | None:
        """Finds corresponding /sys/bus/usb/devices/ path for self.device_path.

        Returns:
            The sysfs directory path if resolved, or None.
        """
        if not self.device_path or not os.path.exists("/sys/bus/usb/devices"):
            return None

        # Parse target bus and device numbers from /dev/bus/usb/BBB/DDD
        parts = self.device_path.rstrip("/").split("/")
        if len(parts) < 2:
            return None
        try:
            target_bus = int(parts[-2])
            target_dev = int(parts[-1])
        except ValueError:
            # Non-bus paths like /dev/usbtest do not encode numeric bus/dev
            return None

        try:
            for entry in os.listdir("/sys/bus/usb/devices"):
                sysfs_dir_path = os.path.join("/sys/bus/usb/devices", entry)
                devnum_p = os.path.join(sysfs_dir_path, "devnum")
                busnum_p = os.path.join(sysfs_dir_path, "busnum")
                if os.path.exists(devnum_p) and os.path.exists(busnum_p):
                    with (
                        open(devnum_p, encoding="utf-8") as f1,
                        open(busnum_p, encoding="utf-8") as f2,
                    ):
                        devnum = int(f1.read().strip())
                        busnum = int(f2.read().strip())
                    if target_bus == busnum and target_dev == devnum:
                        return sysfs_dir_path
        except (OSError, ValueError) as err:
            _LOGGER.debug("Failed resolving sysfs device path: %s", err)
        return None

    def _clear_halt(self, ep: int) -> None:
        """Clears stall/halt condition on an endpoint.

        Args:
            ep: Endpoint address to clear halt condition on.
        """
        if self._fd is not None:
            try:
                ep_c = ctypes.c_uint(ep)
                _get_libc().ioctl(
                    self._fd, USBDEVFS_CLEAR_HALT, ctypes.byref(ep_c)
                )
            except OSError as err:
                _LOGGER.debug(
                    "USBDEVFS_CLEAR_HALT failed for ep %#x: %s", ep, err
                )

    @contextlib.contextmanager
    def claimed_interface(self, ifnum: int) -> collections.abc.Iterator[None]:
        """Context manager to claim a USB interface and release it on exit.

        Disconnects existing kernel drivers before claiming, and re-attaches
        them upon exiting the context.

        Args:
            ifnum: USB interface number to claim.

        Yields:
            None once the interface is claimed.

        Raises:
            RuntimeError: If backend is not currently open.
            OSError: If claiming the interface fails.
        """
        if not self.is_open or self._fd is None:
            raise RuntimeError("Backend is not open")

        cdll = _get_libc()
        ifno_c = ctypes.c_uint(ifnum)

        # 1. Disconnect existing kernel driver via USBDEVFS_DISCONNECT if bound
        try:
            wrapper = UsbdevfsIoctl()
            wrapper.ifno = ifnum
            wrapper.ioctl_code = USBDEVFS_DISCONNECT
            wrapper.data = None
            cdll.ioctl(self._fd, USBDEVFS_IOCTL, ctypes.byref(wrapper))
        except OSError as err:
            _LOGGER.debug(
                "USBDEVFS_DISCONNECT on iface %d failed: %s", ifnum, err
            )

        # 2. Claim interface in userspace
        res = cdll.ioctl(
            self._fd, USBDEVFS_CLAIMINTERFACE, ctypes.byref(ifno_c)
        )
        if res < 0:
            errno_val = ctypes.get_errno()
            raise OSError(
                errno_val,
                f"Failed to claim interface {ifnum}: errno {errno_val} "
                f"({os.strerror(errno_val)})",
            )

        try:
            yield
        finally:
            # 1. Release userspace claim
            try:
                cdll.ioctl(
                    self._fd, USBDEVFS_RELEASEINTERFACE, ctypes.byref(ifno_c)
                )
            except OSError as err:
                _LOGGER.debug(
                    "USBDEVFS_RELEASEINTERFACE on iface %d failed: %s",
                    ifnum,
                    err,
                )
            # 2. Re-bind kernel driver (usbtest) via USBDEVFS_CONNECT
            try:
                wrapper = UsbdevfsIoctl()
                wrapper.ifno = ifnum
                wrapper.ioctl_code = USBDEVFS_CONNECT
                wrapper.data = None
                cdll.ioctl(self._fd, USBDEVFS_IOCTL, ctypes.byref(wrapper))
            except OSError as err:
                _LOGGER.debug(
                    "USBDEVFS_CONNECT on iface %d failed: %s", ifnum, err
                )

    def set_configuration(self, config_val: int) -> bool:
        """Sets USB configuration using USBDEVFS_SETCONFIGURATION and/or sysfs.

        Args:
            config_val: Target configuration value to activate.

        Returns:
            True if the configuration was successfully set or already active,
            False otherwise.

        Raises:
            RuntimeError: If backend is not currently open.
        """
        if not self.is_open or self._fd is None:
            raise RuntimeError("Backend is not open")

        if self.get_configuration() == config_val:
            self.current_config = config_val
            return True

        self._maxpacket = None

        cdll = _get_libc()

        # 1. Release userspace claims and disconnect kernel drivers on valid
        # interfaces for this specific USB device node (self._fd). Linux devio
        # proc_setconfig returns -EBUSY if any interface is claimed by userspace
        # or driver.
        # USBDEVFS ioctls are scoped strictly to self._fd
        # (/dev/bus/usb/BBB/DDD), so this does not affect other USB devices on
        # the host.
        for ifnum in self._get_valid_interfaces(config_val):
            ifno_c = ctypes.c_uint(ifnum)
            try:
                cdll.ioctl(
                    self._fd, USBDEVFS_RELEASEINTERFACE, ctypes.byref(ifno_c)
                )
            except OSError as err:
                _LOGGER.debug(
                    "USBDEVFS_RELEASEINTERFACE on iface %d failed: %s",
                    ifnum,
                    err,
                )
            try:
                wrapper = UsbdevfsIoctl()
                wrapper.ifno = ifnum
                wrapper.ioctl_code = USBDEVFS_DISCONNECT
                wrapper.data = None
                cdll.ioctl(self._fd, USBDEVFS_IOCTL, ctypes.byref(wrapper))
            except OSError as err:
                _LOGGER.debug(
                    "USBDEVFS_DISCONNECT on iface %d failed: %s",
                    ifnum,
                    err,
                )

        # 2. Try USBDEVFS_SETCONFIGURATION ioctl
        success = False
        try:
            cfg_c = ctypes.c_uint(config_val)
            res = cdll.ioctl(
                self._fd, USBDEVFS_SETCONFIGURATION, ctypes.byref(cfg_c)
            )
            if res >= 0:
                success = True
        except OSError as err:
            _LOGGER.debug("USBDEVFS_SETCONFIGURATION failed: %s", err)

        # 3. Fallback: try writing to sysfs bConfigurationValue
        if not success:
            sysfs_path = self._get_sysfs_device_path()
            if sysfs_path:
                cfg_file = os.path.join(sysfs_path, "bConfigurationValue")
                if os.path.exists(cfg_file) and os.access(cfg_file, os.W_OK):
                    try:
                        with open(cfg_file, "w", encoding="utf-8") as f:
                            f.write(str(config_val))
                        success = True
                    except OSError as err:
                        _LOGGER.debug(
                            "Failed writing to sysfs bConfigurationValue: %s",
                            err,
                        )

        # Always re-attach kernel drivers on interfaces so usbtest is probed
        self._reconnect_kernel_drivers()

        if success:
            self.current_config = config_val
            return True

        self.current_config = self.get_configuration()
        return self.current_config == config_val

    def get_num_configurations(self) -> int:
        """Gets the number of configurations supported by the device.

        Returns:
            Total number of configurations, defaulting to 1 on failure.
        """
        # 1. Issue standard USB Chapter 9 GET_DESCRIPTOR for Device Descriptor
        if self._fd is not None:
            try:
                desc_buf = ctypes.create_string_buffer(_USB_DT_DEVICE_SIZE)
                ctrl = _make_get_descriptor_transfer(
                    desc_type=USB_DT_DEVICE,
                    length=_USB_DT_DEVICE_SIZE,
                    data_ptr=ctypes.addressof(desc_buf),
                )
                cdll = _get_libc()
                res = cdll.ioctl(self._fd, USBDEVFS_CONTROL, ctypes.byref(ctrl))
                if res >= _USB_DT_DEVICE_SIZE:
                    # bNumConfigurations is byte 17 of the Device Descriptor
                    val = desc_buf.raw[17]
                    if val > 0:
                        return val
            except OSError as err:
                _LOGGER.debug("GET_DESCRIPTOR for device failed: %s", err)

        # 2. Sysfs fallback
        sysfs_path = self._get_sysfs_device_path()
        if sysfs_path:
            num_cfg_file = os.path.join(sysfs_path, "bNumConfigurations")
            if os.path.exists(num_cfg_file):
                try:
                    with open(num_cfg_file, encoding="utf-8") as f:
                        val = int(f.read().strip())
                        if val > 0:
                            return val
                except (OSError, ValueError) as err:
                    _LOGGER.debug(
                        "Failed reading sysfs bNumConfigurations: %s", err
                    )

        return 1

    def get_configuration(self) -> int:
        """Queries the currently active USB configuration value.

        Returns:
            The active configuration integer (typically 1 or 2).
        """
        # 1. Issue standard USB Chapter 9 GET_CONFIGURATION request via EP0
        if self._fd is not None:
            try:
                cfg_buf = ctypes.create_string_buffer(1)
                ctrl = _make_get_configuration_transfer(
                    data_ptr=ctypes.addressof(cfg_buf),
                )
                cdll = _get_libc()
                res = cdll.ioctl(self._fd, USBDEVFS_CONTROL, ctypes.byref(ctrl))
                if res >= 1:
                    val = cfg_buf.raw[0]
                    self.current_config = val
                    return val
            except OSError as err:
                _LOGGER.debug(
                    "GET_CONFIGURATION control transfer failed: %s", err
                )

        # 2. Sysfs fallback
        sysfs_path = self._get_sysfs_device_path()
        if sysfs_path:
            cfg_file = os.path.join(sysfs_path, "bConfigurationValue")
            if os.path.exists(cfg_file):
                try:
                    with open(cfg_file, encoding="utf-8") as f:
                        val = int(f.read().strip())
                        self.current_config = val
                        return val
                except (OSError, ValueError) as err:
                    _LOGGER.debug(
                        "Failed reading sysfs bConfigurationValue: %s", err
                    )
        return getattr(self, "current_config", 1)

    def _parse_endpoints_from_sysfs(
        self, sysfs_path: str, curr_cfg: int
    ) -> tuple[int | None, int | None, int | None]:
        """Finds Bulk OUT and IN endpoints from sysfs interface directories.

        Args:
            sysfs_path: Path to device sysfs directory.
            curr_cfg: Target configuration value to inspect.

        Returns:
            A tuple of (ep_out, ep_in, target_ifnum).
        """
        intf_eps: dict[int, dict[str, int]] = {}
        try:
            entries = os.listdir(sysfs_path)
        except OSError as err:
            _LOGGER.debug("Failed listing sysfs path %s: %s", sysfs_path, err)
            return None, None, None

        for item in entries:
            if f":{curr_cfg}." not in item:
                continue
            try:
                item_ifnum = int(item.rpartition(".")[2])
            except ValueError:
                item_ifnum = self.ifnum
            intf_dir = os.path.join(sysfs_path, item)
            if not os.path.isdir(intf_dir):
                continue

            if item_ifnum not in intf_eps:
                intf_eps[item_ifnum] = {}

            try:
                sub_entries = os.listdir(intf_dir)
            except OSError:
                continue

            for sub in sub_entries:
                if not sub.startswith("ep_"):
                    continue
                ep_dir = os.path.join(intf_dir, sub)
                type_file = os.path.join(ep_dir, "type")
                addr_file = os.path.join(ep_dir, "bEndpointAddress")
                if not (
                    os.path.exists(type_file) and os.path.exists(addr_file)
                ):
                    continue

                try:
                    with (
                        open(type_file, encoding="utf-8") as f_t,
                        open(addr_file, encoding="utf-8") as f_a,
                    ):
                        ep_type = f_t.read().strip()
                        addr_val = int(f_a.read().strip(), 16)
                except (OSError, ValueError):
                    continue

                if ep_type.lower() != "bulk":
                    continue

                is_in_ep = bool(addr_val & USB_DIR_IN)
                if is_in_ep and "in" not in intf_eps[item_ifnum]:
                    intf_eps[item_ifnum]["in"] = addr_val
                elif not is_in_ep and "out" not in intf_eps[item_ifnum]:
                    intf_eps[item_ifnum]["out"] = addr_val

                maxp_file = os.path.join(ep_dir, "wMaxPacketSize")
                if os.path.exists(maxp_file):
                    try:
                        with open(maxp_file, encoding="utf-8") as f_m:
                            maxp = int(f_m.read().strip(), 16) & 0x07FF
                            if maxp > 0:
                                intf_eps[item_ifnum]["maxpacket"] = maxp
                    except (ValueError, OSError):
                        pass

        # Prefer the first interface that contains both OUT and IN endpoints
        for ifnum in sorted(intf_eps.keys()):
            eps = intf_eps[ifnum]
            if "out" in eps and "in" in eps:
                if "maxpacket" in eps:
                    self._maxpacket = eps["maxpacket"]
                return eps["out"], eps["in"], ifnum

        # Fallback: interface with at least one endpoint
        for ifnum in sorted(intf_eps.keys()):
            eps = intf_eps[ifnum]
            if "out" in eps or "in" in eps:
                if "maxpacket" in eps:
                    self._maxpacket = eps["maxpacket"]
                return eps.get("out"), eps.get("in"), ifnum

        return None, None, None

    def _get_valid_interfaces(self, config_val: int | None = None) -> set[int]:
        """Discovers valid interface numbers for the specified configuration.

        Args:
            config_val: Optional configuration value to inspect.

        Returns:
            A set of integer interface numbers present in the configuration.
        """
        interfaces: set[int] = set()
        if config_val is None:
            config_val = (
                getattr(self, "current_config", None)
                or self.get_configuration()
            )

        # 1. Query configuration descriptor via EP0 control transfer
        if self._fd is not None:
            try:
                cfg_idx = max(config_val - 1, 0)
                cdll = _get_libc()

                hdr_buf = ctypes.create_string_buffer(_USB_DT_CONFIG_SIZE)
                ctrl_hdr = _make_get_descriptor_transfer(
                    desc_type=USB_DT_CONFIG,
                    desc_index=cfg_idx,
                    length=_USB_DT_CONFIG_SIZE,
                    data_ptr=ctypes.addressof(hdr_buf),
                )
                res_hdr = cdll.ioctl(
                    self._fd, USBDEVFS_CONTROL, ctypes.byref(ctrl_hdr)
                )
                if res_hdr >= _USB_DT_CONFIG_SIZE:
                    raw_hdr = hdr_buf.raw[:res_hdr]
                    total_len = raw_hdr[2] | (raw_hdr[3] << 8)
                    if _USB_DT_CONFIG_SIZE <= total_len <= 4096:
                        full_buf = ctypes.create_string_buffer(total_len)
                        ctrl_full = _make_get_descriptor_transfer(
                            desc_type=USB_DT_CONFIG,
                            desc_index=cfg_idx,
                            length=total_len,
                            data_ptr=ctypes.addressof(full_buf),
                        )
                        res_full = cdll.ioctl(
                            self._fd, USBDEVFS_CONTROL, ctypes.byref(ctrl_full)
                        )
                        if res_full >= _USB_DT_CONFIG_SIZE:
                            raw_desc = full_buf.raw[:res_full]
                            for desc in iter_descriptors(raw_desc):
                                if (
                                    len(desc) >= 9
                                    and desc[1] == UsbDescriptorType.INTERFACE
                                ):
                                    interfaces.add(desc[2])
            except (OSError, ValueError) as err:
                _LOGGER.debug(
                    "Failed querying configuration descriptor for "
                    "interfaces: %s",
                    err,
                )

        if interfaces:
            return interfaces

        # 2. Sysfs fallback
        sysfs_path = self._get_sysfs_device_path()
        if sysfs_path and os.path.exists(sysfs_path):
            try:
                for item in os.listdir(sysfs_path):
                    if f":{config_val}." in item:
                        try:
                            ifnum = int(item.rpartition(".")[2])
                            interfaces.add(ifnum)
                        except ValueError:
                            pass
            except OSError as err:
                _LOGGER.debug(
                    "Failed listing sysfs path for interfaces: %s", err
                )

        if interfaces:
            return interfaces

        # Fallback to default ifnum or 0 if nothing discovered
        return {getattr(self, "ifnum", 0)}

    def _find_endpoints(
        self, config_val: int | None = None
    ) -> tuple[int | None, int | None, int | None]:
        """Finds first Bulk OUT and Bulk IN endpoints for active configuration.

        Args:
            config_val: Optional configuration value to target.

        Returns:
            A tuple of (ep_out, ep_in, target_ifnum).
        """
        ep_out = None
        ep_in = None
        target_ifnum = None
        if config_val is not None:
            curr_cfg = config_val
        elif getattr(self, "current_config", None) is not None:
            curr_cfg = self.current_config
        else:
            curr_cfg = self.get_configuration()

        # 1. Dynamically query configuration descriptor via EP0 control transfer
        if self._fd is not None:
            try:
                cfg_idx = max(curr_cfg - 1, 0)
                cdll = _get_libc()

                # Step 1a: Request 9-byte header to read wTotalLength
                hdr_buf = ctypes.create_string_buffer(_USB_DT_CONFIG_SIZE)
                ctrl_hdr = _make_get_descriptor_transfer(
                    desc_type=USB_DT_CONFIG,
                    desc_index=cfg_idx,
                    length=_USB_DT_CONFIG_SIZE,
                    data_ptr=ctypes.addressof(hdr_buf),
                )

                res_hdr = cdll.ioctl(
                    self._fd, USBDEVFS_CONTROL, ctypes.byref(ctrl_hdr)
                )
                if res_hdr >= _USB_DT_CONFIG_SIZE:
                    raw_hdr = hdr_buf.raw[:res_hdr]
                    total_len = raw_hdr[2] | (raw_hdr[3] << 8)
                    if _USB_DT_CONFIG_SIZE <= total_len <= 4096:
                        # Step 1b: Fetch full configuration descriptor
                        full_buf = ctypes.create_string_buffer(total_len)
                        ctrl_full = _make_get_descriptor_transfer(
                            desc_type=USB_DT_CONFIG,
                            desc_index=cfg_idx,
                            length=total_len,
                            data_ptr=ctypes.addressof(full_buf),
                        )
                        res_full = cdll.ioctl(
                            self._fd, USBDEVFS_CONTROL, ctypes.byref(ctrl_full)
                        )
                        if res_full >= _USB_DT_CONFIG_SIZE:
                            raw_desc = full_buf.raw[:res_full]
                            (
                                ep_out,
                                ep_in,
                                target_ifnum,
                            ) = parse_endpoints_from_config_descriptor(raw_desc)
                            # Extract wMaxPacketSize using iter_descriptors
                            for desc in iter_descriptors(raw_desc):
                                if len(desc) >= 7 and desc[1] == 0x05:
                                    ep_addr = desc[2]
                                    if ep_addr in (ep_out, ep_in):
                                        maxp = (
                                            desc[4] | (desc[5] << 8)
                                        ) & 0x07FF
                                        if maxp > 0:
                                            self._maxpacket = maxp
            except (OSError, ValueError) as err:
                _LOGGER.debug(
                    "Failed querying configuration descriptor: %s", err
                )

        # 2. Sysfs fallback if endpoints not found via descriptor query
        if ep_out is None or ep_in is None:
            sysfs_path = self._get_sysfs_device_path()
            if sysfs_path and os.path.exists(sysfs_path):
                ep_out, ep_in, target_ifnum = self._parse_endpoints_from_sysfs(
                    sysfs_path, curr_cfg
                )

        return ep_out, ep_in, target_ifnum

    def run_bulk_loopback(
        self,
        params: TestParams,
        length_vary: bool = False,
        is_short: bool = False,
        is_zlp: bool = False,
        test_id: int = 32,
        test_name: str = "Test 32: LOOPBACK_BULK_FIXED",
        ep_out: int | None = None,
        ep_in: int | None = None,
    ) -> TestResult:
        """Executes bidirectional bulk loopback test via USBDEVFS_BULK.

        Args:
            params: Parameters controlling length, iterations, and timeout.
            length_vary: True if packet length should vary per iteration.
            is_short: True if transfers should alternate short packets.
            is_zlp: True if zero-length termination is tested.
            test_id: Integer test identifier.
            test_name: Human-readable test name.
            ep_out: Optional explicit Bulk OUT endpoint address.
            ep_in: Optional explicit Bulk IN endpoint address.

        Returns:
            TestResult recording status, duration, throughput, and errors.

        Raises:
            RuntimeError: If backend is not open.
        """
        if not self.is_open or self._fd is None:
            raise RuntimeError("Backend is not open")

        # Automatically ensure Loopback configuration is active
        num_configs = self.get_num_configurations()
        target_config = (
            2 if num_configs > 1 else (self.get_configuration() or 1)
        )
        if self.get_configuration() != target_config:
            if not self.set_configuration(target_config):
                return TestResult(
                    test_id=test_id,
                    test_name=test_name,
                    status=TestStatus.FAIL,
                    duration_secs=0.0,
                    error_message=(
                        f"Failed to set Configuration {target_config} "
                        "for loopback"
                    ),
                )

        disc_out, disc_in, disc_if = self._find_endpoints(
            config_val=target_config
        )
        ep_out = ep_out or disc_out
        ep_in = ep_in or disc_in

        if disc_out is not None and disc_in is not None:
            assert (
                disc_if is not None
            ), "Interface number must be present when endpoints are discovered"
            claim_ifnum = disc_if
        else:
            claim_ifnum = disc_if if disc_if is not None else self.ifnum

        if ep_out is None or ep_in is None:
            return TestResult(
                test_id=test_id,
                test_name=test_name,
                status=TestStatus.ERROR,
                duration_secs=0.0,
                error_message=(
                    "Could not discover Bulk OUT and Bulk IN endpoints from "
                    "USB configuration descriptor"
                ),
            )

        cdll = _get_libc()
        start_time = time.perf_counter()
        total_bytes = 0
        iterations_done = 0
        error_msg = None

        maxpacket = getattr(self, "_maxpacket", None) or (
            512
            if self.speed_name == "high"
            else (1024 if self.speed_name.startswith("super") else 64)
        )

        max_buf_len = max(params.length, 65536)
        pattern = bytes((j % 256) for j in range(max_buf_len))
        out_buf = ctypes.create_string_buffer(pattern, max_buf_len)
        in_buf = ctypes.create_string_buffer(max_buf_len)
        mv_out = memoryview(out_buf)
        mv_in = memoryview(in_buf)

        bulk_out = UsbdevfsBulktransfer()
        bulk_out.ep = ep_out
        bulk_out.timeout = params.timeout_ms
        bulk_out.data = ctypes.addressof(out_buf)

        bulk_in = UsbdevfsBulktransfer()
        bulk_in.ep = ep_in
        bulk_in.timeout = params.timeout_ms
        bulk_in.data = ctypes.addressof(in_buf)

        with self.claimed_interface(claim_ifnum):
            for i in range(params.iterations):
                # Calculate transfer length
                if length_vary:
                    step = (
                        params.vary
                        if (params.vary % params.length != 0)
                        else 127
                    )
                    curr_len = ((i * step) % params.length) + 1
                elif is_short:
                    curr_len = maxpacket - 1 if (i % 2 == 0) else maxpacket + 1
                elif is_zlp:
                    curr_len = maxpacket
                else:
                    curr_len = params.length

                # Bulk write OUT
                bulk_out.len = curr_len
                try:
                    res_out = cdll.ioctl(
                        self._fd, USBDEVFS_BULK, ctypes.byref(bulk_out)
                    )
                except OSError as err:
                    error_msg = (
                        f"Bulk OUT transfer failed at iteration {i}: {err}"
                    )
                    break
                if res_out < 0:
                    errno_val = ctypes.get_errno()
                    error_msg = (
                        f"Bulk OUT write failed at iteration {i}: "
                        f"errno {errno_val} ({os.strerror(errno_val)})"
                    )
                    break
                if res_out != curr_len:
                    error_msg = (
                        f"Bulk OUT short write at iteration {i}: "
                        f"requested {curr_len} bytes, sent {res_out} bytes"
                    )
                    break

                # Bulk read IN
                bulk_in.len = curr_len
                try:
                    res_in = cdll.ioctl(
                        self._fd, USBDEVFS_BULK, ctypes.byref(bulk_in)
                    )
                except OSError as err:
                    error_msg = (
                        f"Bulk IN transfer failed at iteration {i}: {err}"
                    )
                    break
                if res_in < 0:
                    errno_val = ctypes.get_errno()
                    error_msg = (
                        f"Bulk IN read failed at iteration {i}: "
                        f"errno {errno_val} ({os.strerror(errno_val)})"
                    )
                    break

                bytes_read = res_in
                if bytes_read != curr_len:
                    error_msg = (
                        f"Loopback short read at iteration {i}: "
                        f"requested {curr_len} bytes, "
                        f"got {bytes_read} bytes"
                    )
                    break

                # Compare data integrity via fast memoryview comparison
                if mv_in[:curr_len] != mv_out[:curr_len]:
                    recv_head = bytes(mv_in[: min(curr_len, 16)]).hex()
                    sent_head = bytes(mv_out[: min(curr_len, 16)]).hex()
                    error_msg = (
                        f"Loopback data corruption at iteration {i}: "
                        f"recv {bytes_read} bytes (head: {recv_head}) != "
                        f"sent {curr_len} bytes (head: {sent_head})"
                    )
                    break

                total_bytes += curr_len * 2
                iterations_done += 1

        elapsed = time.perf_counter() - start_time
        throughput_mbps = (
            (total_bytes * 8 / 1e6) / elapsed if elapsed > 0 else 0.0
        )
        throughput_mbs = (
            (total_bytes / (1024 * 1024)) / elapsed if elapsed > 0 else 0.0
        )

        if error_msg:
            if ep_out is not None:
                self._clear_halt(ep_out)
            if ep_in is not None:
                self._clear_halt(ep_in)
            return TestResult(
                test_id=test_id,
                test_name=test_name,
                status=TestStatus.FAIL,
                duration_secs=elapsed,
                error_message=error_msg,
                iterations_completed=iterations_done,
                bytes_transferred=total_bytes,
                throughput_mbps=throughput_mbps,
                throughput_mbs=throughput_mbs,
            )

        return TestResult(
            test_id=test_id,
            test_name=test_name,
            status=TestStatus.PASS,
            duration_secs=elapsed,
            iterations_completed=iterations_done,
            bytes_transferred=total_bytes,
            throughput_mbps=throughput_mbps,
            throughput_mbs=throughput_mbs,
        )

    def run_test(self, test_id: int, params: TestParams) -> TestResult:
        """Executes a single test case against the target USB device.

        Args:
            test_id: Integer identifier of the test case to execute.
            params: Parameters controlling iteration count, buffer length, etc.

        Returns:
            A TestResult recording execution outcome and metrics.

        Raises:
            RuntimeError: If the backend is not currently open.
        """
        if not self.is_open or self._fd is None:
            raise RuntimeError("Backend is not open")

        test_case = ALL_TEST_CASES.get(test_id)
        test_name = test_case.name if test_case else f"Test {test_id}"

        # Dispatch Loopback tests 32..35 via pattern matching
        match test_id:
            case LoopbackTestId.BULK_FIXED:
                return self.run_bulk_loopback(
                    params,
                    length_vary=False,
                    test_id=test_id,
                    test_name=test_name,
                )
            case LoopbackTestId.BULK_VARY:
                return self.run_bulk_loopback(
                    params,
                    length_vary=True,
                    test_id=test_id,
                    test_name=test_name,
                )
            case LoopbackTestId.BULK_SHORT:
                return self.run_bulk_loopback(
                    params,
                    is_short=True,
                    test_id=test_id,
                    test_name=test_name,
                )
            case LoopbackTestId.BULK_ZLP:
                return self.run_bulk_loopback(
                    params,
                    is_zlp=True,
                    test_id=test_id,
                    test_name=test_name,
                )

        # For bulk endpoint tests targeting Source/Sink endpoints (EP1),
        # ensure Configuration 1 is active.
        # EP0 generic control tests can run on either configuration.
        if test_id not in _EP0_GENERIC_TESTS and self.get_configuration() != 1:
            if not self.set_configuration(1):
                return TestResult(
                    test_id=test_id,
                    test_name=test_name,
                    status=TestStatus.FAIL,
                    duration_secs=0.0,
                    error_message="Failed to set Configuration 1 for test",
                )

        # Prepare UsbtestParam
        param = UsbtestParam()
        param.test_num = test_id
        param.length = params.length
        param.vary = params.vary
        param.sglen = params.sglen
        param.iterations = params.iterations

        # Prepare UsbdevfsIoctl wrapper
        wrapper = UsbdevfsIoctl()
        wrapper.ifno = self.ifnum
        wrapper.ioctl_code = USBTEST_REQUEST
        wrapper.data = ctypes.addressof(param)

        start_time = time.perf_counter()
        try:
            res = _get_libc().ioctl(
                self._fd, USBDEVFS_IOCTL, ctypes.byref(wrapper)
            )
            elapsed = time.perf_counter() - start_time

            if res < 0:
                err = ctypes.get_errno()
                if err == errno.EOPNOTSUPP:
                    return TestResult(
                        test_id=test_id,
                        test_name=test_name,
                        status=TestStatus.SKIP,
                        duration_secs=elapsed,
                        error_message=(
                            f"Operation not supported by kernel usbtest "
                            f"driver (errno {err} EOPNOTSUPP)"
                        ),
                    )
                elif err == errno.ENOTTY:
                    return TestResult(
                        test_id=test_id,
                        test_name=test_name,
                        status=TestStatus.FAIL,
                        duration_secs=elapsed,
                        error_message=(
                            "usbtest driver is not bound to interface "
                            "(errno 25 ENOTTY). Ensure usbtest kernel module "
                            "is loaded."
                        ),
                    )
                err_str = os.strerror(err)
                return TestResult(
                    test_id=test_id,
                    test_name=test_name,
                    status=TestStatus.FAIL,
                    duration_secs=elapsed,
                    error_message=f"ioctl failed with errno {err} ({err_str})",
                )

            # Use duration from kernel if available, else elapsed
            kernel_duration = param.duration.tv_sec + (
                param.duration.tv_usec / 1e6
            )
            duration = kernel_duration if kernel_duration > 0 else elapsed

            # Calculate bytes transferred and throughput
            if test_id in _ZERO_BYTE_TRANSFER_TESTS:
                bytes_transferred = 0
                throughput_mbps = 0.0
                throughput_mbs = 0.0
            else:
                sg_mult = (
                    param.sglen
                    if (param.sglen > 0 and test_id in _SCATTER_GATHER_TESTS)
                    else 1
                )
                bytes_transferred = param.iterations * sg_mult * params.length
                throughput_mbps = (
                    (bytes_transferred * 8 / 1e6) / duration
                    if duration > 0
                    else 0.0
                )
                throughput_mbs = (
                    (bytes_transferred / (1024 * 1024)) / duration
                    if duration > 0
                    else 0.0
                )

            return TestResult(
                test_id=test_id,
                test_name=test_name,
                status=TestStatus.PASS,
                duration_secs=duration,
                iterations_completed=param.iterations,
                bytes_transferred=bytes_transferred,
                throughput_mbps=throughput_mbps,
                throughput_mbs=throughput_mbs,
            )
        except OSError as e:
            elapsed = time.perf_counter() - start_time
            return TestResult(
                test_id=test_id,
                test_name=test_name,
                status=TestStatus.ERROR,
                duration_secs=elapsed,
                error_message=str(e),
            )
