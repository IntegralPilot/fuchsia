#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Linux-specific host hardware and filesystem capability profiler.

Outputs a structured, single-record JSON snapshot of mounts, disks, CPUs, RAM, and network interfaces.

Security & PII Boundaries:
- Workstation usernames in directory/mount paths are automatically sanitized and redacted with '$USER'.
- Sensitive network configurations—including physical MAC addresses and IP addresses—are completely
  unread and never collected by this utility.
"""

import argparse
import getpass
import json
import os
from collections.abc import Iterator
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any

# --- Immutable Dataclass Data Structures representing parsed states ---


@dataclass(frozen=True, slots=True)
class MountInfo:
    """Represents file system mount configuration details.

    Attributes:
        device: The source block device or virtual filesystem (e.g., '/dev/sda1').
        mount_point: The path where the filesystem is mounted (e.g., '/').
        fstype: The filesystem type (e.g., 'ext4', 'tmpfs').
        options: The active comma-separated mount flags (e.g., 'rw,noatime').
    """

    device: str
    mount_point: str
    fstype: str
    options: str

    def sanitize(self, username: str) -> "MountInfo":
        """Returns a new MountInfo with matching path components redacted."""
        if not username:
            return self
        return replace(
            self,
            device=_sanitize_path(self.device, username),
            mount_point=_sanitize_path(self.mount_point, username),
        )

    def to_dict(self) -> dict[str, str]:
        """Converts the mount info record to a clean JSON-serializable dictionary."""
        return {
            "device": self.device,
            "mount_point": self.mount_point,
            "fstype": self.fstype,
            "options": self.options,
        }


@dataclass(frozen=True, slots=True)
class DiskInfo:
    """Represents a physical or virtual storage block device's properties.

    Attributes:
        rotational: True if the disk is a spinning platter HDD, False if SSD/NVMe.
        size_gb: The storage capacity of the disk in decimal Gigabytes.
    """

    rotational: bool | None = None
    size_gb: float | None = None

    def to_dict(self) -> dict[str, Any]:
        """Converts the disk info record to a clean JSON-serializable dictionary."""
        res: dict[str, Any] = {}
        if self.rotational is not None:
            res["rotational"] = self.rotational
        if self.size_gb is not None:
            res["size_gb"] = self.size_gb
        return res


@dataclass(frozen=True, slots=True)
class CpuInfo:
    """Represents the host processor hardware specs.

    Attributes:
        model_name: The brand/model name of the CPU (e.g., 'AMD EPYC 7B12').
        logical_processors: The total count of active logical threads/cores.
        physical_cores: The count of distinct physical cores on the CPU.
    """

    model_name: str
    logical_processors: int
    physical_cores: int

    def to_dict(self) -> dict[str, Any]:
        """Converts the CPU info record to a clean JSON-serializable dictionary."""
        return {
            "model_name": self.model_name,
            "logical_processors": self.logical_processors,
            "physical_cores": self.physical_cores,
        }


@dataclass(frozen=True, slots=True)
class MemoryInfo:
    """Represents the total physical system memory available on the host.

    Attributes:
        total_ram_gb: Total installed RAM capacity in Gigabytes.
    """

    total_ram_gb: float

    def to_dict(self) -> dict[str, Any]:
        """Converts the memory info record to a clean JSON-serializable dictionary."""
        return {
            "total_ram_gb": self.total_ram_gb,
        }


@dataclass(frozen=True, slots=True)
class NetworkInterfaceInfo:
    """Represents an active or configured network interface card on the host.

    Attributes:
        name: The interface device name (e.g., 'eth0', 'wlan0').
        operstate: The current operational state (e.g., 'up', 'down').
        speed_mbps: Optional active link bandwidth capacity in Megabits per second.
    """

    name: str
    operstate: str
    speed_mbps: int | None = None

    def to_dict(self) -> dict[str, Any]:
        """Converts the network interface info to a clean JSON-serializable dictionary."""
        res: dict[str, Any] = {
            "name": self.name,
            "operstate": self.operstate,
        }
        if self.speed_mbps is not None:
            res["speed_mbps"] = self.speed_mbps
        return res


# --- System Hardware Profiling Helpers ---


def _sanitize_path(path_str: str, username: str) -> str:
    """Replaces any directory path component exactly equal to the username with $USER."""
    if not username or not path_str:
        return path_str
    # Retrieve the path parts natively (e.g. ('/', 'home', 'fangism'))
    parts = Path(path_str).parts
    sanitized_parts = ["$USER" if p == username else p for p in parts]
    return str(Path(*sanitized_parts))


def _is_workspace_mount(mount_point: str, workspace_dir: str) -> bool:
    """Checks if a mount point is a parent directory of or equals the workspace directory.

    Examples:
      workspace_dir = "/home/fuchsia/src"
      _is_workspace_mount("/", workspace_dir)             -> True
      _is_workspace_mount("/home", workspace_dir)         -> True
      _is_workspace_mount("/home/fuchsia/src", workspace_dir) -> True
      _is_workspace_mount("/run", workspace_dir)          -> False
      _is_workspace_mount("/hom", workspace_dir)          -> False (prefix matches, but not directory-bound)
    """
    if workspace_dir == mount_point:
        return True

    # Enforce trailing slash to guarantee directory-level matching
    # (prevents "/hom" from falsely matching "/home/fuchsia")
    prefix = mount_point if mount_point.endswith("/") else mount_point + "/"
    return workspace_dir.startswith(prefix)


def _parse_mounts_content(
    content: str, workspace_dir: str
) -> Iterator[MountInfo]:
    """Purely parses the raw /proc/mounts content string.

    Filters and yields MountInfo records that are parent to or equal the workspace_dir.

    Assumes the standard Linux /proc/mounts format, which is whitespace-separated
    and maps columns sequentially as follows:
      Col 0: Device node or virtual filesystem source (e.g. '/dev/sda1' or 'tmpfs')
      Col 1: Mount point absolute path (e.g. '/' or '/home/fuchsia')
      Col 2: Filesystem type identifier (e.g. 'ext4' or 'tmpfs')
      Col 3: Comma-separated mount options flags (e.g. 'rw,noatime,discard')
      Col 4: Dummy integer for dump (usually '0')
      Col 5: Dummy integer for fsck pass (usually '0')

    Example /proc/mounts line:
      /dev/sda1 / ext4 rw,noatime,discard,errors=remount-ro 0 0
    """
    for line in content.splitlines():
        parts = line.split()
        if len(parts) >= 4:
            mount_point = parts[1]
            if _is_workspace_mount(mount_point, workspace_dir):
                yield MountInfo(
                    device=parts[0],
                    mount_point=mount_point,
                    fstype=parts[2],
                    options=parts[3],
                )


def _parse_mount_options(
    proc_dir: str, workspace_dir: str
) -> Iterator[MountInfo]:
    """Reads mounts from proc_dir and delegates to _parse_mounts_content."""
    proc_mounts = Path(proc_dir) / "mounts"
    if not proc_mounts.exists():
        return

    try:
        content = proc_mounts.read_text(encoding="utf-8")
        yield from _parse_mounts_content(content, workspace_dir)
    except OSError:
        pass


def _query_block_device_info(dev_dir: Path) -> DiskInfo:
    """Reads size and rotational state of a block device, failing gracefully.

    Expected file formats and metrics:
      1. dev_dir / "queue" / "rotational":
         - Content: "0" (SSD / Solid State / NVMe / Non-rotational)
         - Content: "1" (HDD / Magnetic rotational hard drive)
      2. dev_dir / "size":
         - Content: size in 512-byte sectors as an integer string (e.g., "488281250" for 250 GB)
    """
    rotational = None
    rot_file = dev_dir / "queue" / "rotational"
    if rot_file.exists():
        try:
            # 1 = Magnetic HDD, 0 = Solid State SSD/NVMe
            rotational = rot_file.read_text(encoding="utf-8").strip() == "1"
        except OSError:
            pass

    size_gb = None
    size_file = dev_dir / "size"
    if size_file.exists():
        try:
            raw_size = size_file.read_text(encoding="utf-8").strip()
            # Size is stored in 512-byte sectors; convert to GB
            size_gb = round((int(raw_size) * 512) / 1e9, 2)
        except OSError:
            pass
        except ValueError:
            pass

    return DiskInfo(rotational=rotational, size_gb=size_gb)


def _parse_disk_properties(sys_dir: str) -> dict[str, DiskInfo]:
    """Scans and extracts properties for physical and virtual block devices.

    Scans directories inside sys_dir/block representing active storage devices:
      - SCSI/SATA drives: "sda", "sdb", etc.
      - NVMe SSD drives: "nvme0n1", "nvme1n1", etc.
      - Virtualised VirtIO partitions: "vda", "vdb", "xvda", etc.
    """
    disks: dict[str, DiskInfo] = {}
    sys_block = Path(sys_dir) / "block"
    if not sys_block.exists():
        return disks

    # Match physical SCSI (sd), NVMe (nvme), and virtualised VirtIO (vd / xvd) devices
    valid_prefixes = ("sd", "nvme", "vd", "xvd")
    try:
        for dev in sys_block.iterdir():
            if dev.name.startswith(valid_prefixes):
                try:
                    info = _query_block_device_info(dev)
                    # Only append if we successfully collected at least some info
                    if info.rotational is not None or info.size_gb is not None:
                        disks[dev.name] = info
                except OSError:
                    pass
    except OSError:
        pass
    return disks


def _parse_cpuinfo_content(content: str) -> CpuInfo:
    """Purely parses /proc/cpuinfo content and returns a CpuInfo record.

    Example /proc/cpuinfo snippet:
      processor : 0
      model name : Intel(R) Xeon(R) CPU @ 2.20GHz
      cpu cores : 2
    """
    model_name = "Unknown"
    logical_processors = 0
    physical_cores = 0

    for line in content.splitlines():
        if ":" in line:
            key, val = [parts.strip() for parts in line.split(":", 1)]
            if key == "processor":
                logical_processors += 1
            elif key == "model name" and model_name == "Unknown":
                model_name = val
            elif key == "cpu cores" and physical_cores == 0:
                try:
                    physical_cores = int(val)
                except ValueError:
                    pass

    # Fallback in case cpu cores isn't reported on some architectures (like virtual VMs)
    if physical_cores == 0:
        physical_cores = logical_processors

    return CpuInfo(
        model_name=model_name,
        logical_processors=logical_processors,
        physical_cores=physical_cores,
    )


def _get_cpu_info(proc_dir: str) -> CpuInfo:
    """Reads /proc/cpuinfo and returns a CpuInfo record."""
    proc_cpuinfo = Path(proc_dir) / "cpuinfo"
    if not proc_cpuinfo.exists():
        return CpuInfo("Unknown", 1, 1)
    try:
        content = proc_cpuinfo.read_text(encoding="utf-8")
        return _parse_cpuinfo_content(content)
    except OSError:
        return CpuInfo("Unknown", 1, 1)


def _parse_meminfo_total(content: str) -> MemoryInfo:
    """Purely parses MemTotal from /proc/meminfo content.

    Example /proc/meminfo line:
      MemTotal:       65873920 kB
    """
    total_ram_gb = 0.0
    for line in content.splitlines():
        if line.startswith("MemTotal:"):
            parts = line.split()
            if len(parts) >= 2:
                try:
                    # Convert kB to bytes, then to GB
                    total_ram_gb = round((int(parts[1]) * 1024) / 1e9, 2)
                except ValueError:
                    pass
            break
    return MemoryInfo(total_ram_gb=total_ram_gb)


def _get_system_memory_info(proc_dir: str) -> MemoryInfo:
    """Reads /proc/meminfo and returns total physical memory."""
    proc_meminfo = Path(proc_dir) / "meminfo"
    if not proc_meminfo.exists():
        return MemoryInfo(0.0)
    try:
        content = proc_meminfo.read_text(encoding="utf-8")
        return _parse_meminfo_total(content)
    except OSError:
        return MemoryInfo(0.0)


def _query_net_interface_info(iface_dir: Path) -> NetworkInterfaceInfo:
    """Reads operstate and speed of a network interface, failing gracefully."""
    operstate = "unknown"
    oper_file = iface_dir / "operstate"
    if oper_file.exists():
        try:
            operstate = oper_file.read_text(encoding="utf-8").strip()
        except OSError:
            pass

    speed_mbps = None
    speed_file = iface_dir / "speed"
    if speed_file.exists():
        try:
            speed_mbps = int(speed_file.read_text(encoding="utf-8").strip())
        except (OSError, ValueError):
            pass

    return NetworkInterfaceInfo(
        name=iface_dir.name, operstate=operstate, speed_mbps=speed_mbps
    )


def _parse_network_interfaces(sys_dir: str) -> Iterator[NetworkInterfaceInfo]:
    """Scans and extracts properties for physical and active network interfaces."""
    sys_net = Path(sys_dir) / "class" / "net"
    if not sys_net.exists():
        return

    try:
        for iface in sys_net.iterdir():
            # Skip loopback interface 'lo'
            if iface.name == "lo":
                continue
            try:
                yield _query_net_interface_info(iface)
            except OSError:
                pass
    except OSError:
        pass


def collect_hardware_profile(
    proc_dir: str, sys_dir: str, workspace_dir: str, username: str
) -> dict[str, Any]:
    """Gathers a lightweight system, disk, CPU, RAM, and net snapshot dict."""
    return {
        "cpu": _get_cpu_info(proc_dir).to_dict(),
        "memory": _get_system_memory_info(proc_dir).to_dict(),
        "disks": {
            k: v.to_dict() for k, v in _parse_disk_properties(sys_dir).items()
        },
        "network": [
            net.to_dict() for net in _parse_network_interfaces(sys_dir)
        ],
        "mounts": [
            m.sanitize(username).to_dict()
            for m in _parse_mount_options(proc_dir, workspace_dir)
        ],
    }


def capture_hardware_profile(
    output_path: Path,
    proc_dir: str,
    sys_dir: str,
    workspace_dir: str,
    username: str,
) -> None:
    """Collects and writes the hardware profile JSON to disk."""
    try:
        profile = collect_hardware_profile(
            proc_dir, sys_dir, workspace_dir, username
        )
        output_path.parent.mkdir(parents=True, exist_ok=True)
        with open(output_path, "w") as f:
            json.dump(profile, f, indent=2)
    except Exception:
        # Fail silently to guarantee we never interrupt the user's build
        pass


def main() -> None:
    """Core command-line execution entry point for the utility."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output",
        type=Path,
        required=True,
        help="Path to write hardware profile JSON snapshot",
    )
    parser.add_argument(
        "--proc-dir",
        default="/proc",
        help="Custom path for proc filesystem (for testing)",
    )
    parser.add_argument(
        "--sys-dir",
        default="/sys",
        help="Custom path for sys filesystem (for testing)",
    )
    args = parser.parse_args()

    try:
        username = getpass.getuser()
    except KeyError:
        username = ""

    capture_hardware_profile(
        args.output,
        proc_dir=args.proc_dir,
        sys_dir=args.sys_dir,
        workspace_dir=os.getcwd(),
        username=username,
    )


if __name__ == "__main__":
    main()
