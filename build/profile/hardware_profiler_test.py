#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for the hardware profiler."""

import shutil
import tempfile
import unittest
from pathlib import Path

import hardware_profiler

# --- Abstract Test Base Class with Mock /proc File System Lifecycle ---


class ProcMockTestCase(unittest.TestCase):
    """Base test case that automatically manages a hermetic mock /proc filesystem."""

    def setUp(self) -> None:
        super().setUp()
        self.test_dir = Path(tempfile.mkdtemp())
        self.proc_dir = self.test_dir / "proc"
        self.proc_dir.mkdir()

    def tearDown(self) -> None:
        shutil.rmtree(self.test_dir)
        super().tearDown()


class HardwareProfileTest(ProcMockTestCase):
    def setUp(self) -> None:
        super().setUp()
        self.sys_dir = self.test_dir / "sys"
        self.sys_dir.mkdir()
        (self.sys_dir / "block").mkdir()

    def write_mock_mounts(self, content: str) -> None:
        with open(self.proc_dir / "mounts", "w") as f:
            f.write(content)

    def write_mock_disk(
        self, dev_name: str, rotational: str, size: int
    ) -> None:
        dev_dir = self.sys_dir / "block" / dev_name
        dev_dir.mkdir(parents=True)
        (dev_dir / "queue").mkdir()
        with open(dev_dir / "queue" / "rotational", "w") as f:
            f.write(rotational + "\n")
        with open(dev_dir / "size", "w") as f:
            f.write(str(size) + "\n")

    def write_mock_cpuinfo(self, content: str) -> None:
        with open(self.proc_dir / "cpuinfo", "w") as f:
            f.write(content)

    def write_mock_meminfo(self, content: str) -> None:
        with open(self.proc_dir / "meminfo", "w") as f:
            f.write(content)

    def write_mock_net_interface(
        self, name: str, operstate: str, speed: int | None
    ) -> None:
        iface_dir = self.sys_dir / "class" / "net" / name
        iface_dir.mkdir(parents=True)
        with open(iface_dir / "operstate", "w") as f:
            f.write(operstate + "\n")
        if speed is not None:
            with open(iface_dir / "speed", "w") as f:
                f.write(str(speed) + "\n")

    def test_sanitize_path(self) -> None:
        # 1. Exact full-component match must be redacted
        self.assertEqual(
            hardware_profiler._sanitize_path(
                "/home/fangism/workspace", "fangism"
            ),
            "/home/$USER/workspace",
        )
        # 2. Partial substring component match must NOT be redacted (boundary protection)
        self.assertEqual(
            hardware_profiler._sanitize_path(
                "/home/fangism2/workspace", "fang"
            ),
            "/home/fangism2/workspace",
        )
        # 3. Empty username must return the original path unchanged
        self.assertEqual(
            hardware_profiler._sanitize_path("/home/fangism/workspace", ""),
            "/home/fangism/workspace",
        )

    def test_parse_mounts_content_pure(self) -> None:
        raw_mounts = (
            "/dev/sda1 / ext4 rw,noatime,discard,errors=remount-ro 0 0\n"
            "tmpfs /run tmpfs rw,nosuid,nodev 0 0\n"
            "/dev/sdb1 /home/fuchsia ext4 rw,relatime 0 0\n"
        )
        workspace_dir = "/home/fuchsia/my_workspace"

        # 1. Mount point '/' is a parent of '/home/fuchsia/my_workspace' -> Match!
        # 2. Mount point '/home/fuchsia' is a parent of '/home/fuchsia/my_workspace' -> Match!
        # 3. Mount point '/run' is NOT a parent of '/home/fuchsia/my_workspace' -> Ignored!
        results = list(
            hardware_profiler._parse_mounts_content(raw_mounts, workspace_dir)
        )

        self.assertEqual(len(results), 2)
        self.assertEqual(results[0].mount_point, "/")
        self.assertEqual(
            results[0].options, "rw,noatime,discard,errors=remount-ro"
        )
        # Call .sanitize() explicitly to check that MountInfo.sanitize redacts username components correctly!
        sanitized_mount = results[1].sanitize("fuchsia")
        self.assertEqual(sanitized_mount.mount_point, "/home/$USER")
        self.assertEqual(sanitized_mount.options, "rw,relatime")

    def test_parse_disk_properties_pure(self) -> None:
        # Verify VirtIO virtual disk parsing (vda) and DiskInfo dataclass generation
        self.write_mock_disk("vda", "0", 488281250)  # 250 GB SSD

        disks = hardware_profiler._parse_disk_properties(str(self.sys_dir))

        self.assertEqual(len(disks), 1)
        self.assertIsInstance(disks["vda"], hardware_profiler.DiskInfo)
        self.assertEqual(disks["vda"].rotational, False)
        self.assertEqual(disks["vda"].size_gb, 250.0)

    def test_parse_cpuinfo_pure(self) -> None:
        raw_cpuinfo = (
            "processor\t: 0\n"
            "model name\t: Intel(R) Xeon(R) CPU @ 2.20GHz\n"
            "cpu cores\t: 2\n"
            "\n"
            "processor\t: 1\n"
            "model name\t: Intel(R) Xeon(R) CPU @ 2.20GHz\n"
            "cpu cores\t: 2\n"
        )
        info = hardware_profiler._parse_cpuinfo_content(raw_cpuinfo)
        self.assertEqual(info.model_name, "Intel(R) Xeon(R) CPU @ 2.20GHz")
        self.assertEqual(info.logical_processors, 2)
        self.assertEqual(info.physical_cores, 2)

    def test_parse_meminfo_pure(self) -> None:
        raw_meminfo = (
            "MemTotal:       65873920 kB\n" "MemFree:        4312690 kB\n"
        )
        info = hardware_profiler._parse_meminfo_total(raw_meminfo)
        self.assertEqual(
            info.total_ram_gb, 67.45
        )  # 65873920 * 1024 / 1e9 = 67.4548...

    def test_collect_hardware_profile(self) -> None:
        self.write_mock_mounts(
            "/dev/sda1 / ext4 rw,noatime,discard,errors=remount-ro 0 0\n"
        )
        self.write_mock_disk("sda", "0", 488281250)
        self.write_mock_cpuinfo(
            "processor\t: 0\n"
            "model name\t: AMD EPYC 7B12\n"
            "cpu cores\t: 8\n"
        )
        self.write_mock_meminfo("MemTotal:       16000000 kB\n")
        self.write_mock_net_interface("eth0", "up", 10000)
        self.write_mock_net_interface("lo", "unknown", None)  # Skip loopback

        profile = hardware_profiler.collect_hardware_profile(
            proc_dir=str(self.proc_dir),
            sys_dir=str(self.sys_dir),
            workspace_dir=str(self.test_dir / "workspace"),
            username="mockuser",
        )

        # 1. Verify CPU
        self.assertEqual(profile["cpu"]["model_name"], "AMD EPYC 7B12")
        self.assertEqual(profile["cpu"]["physical_cores"], 8)

        # 2. Verify Memory
        self.assertEqual(profile["memory"]["total_ram_gb"], 16.38)

        # 3. Verify Disks
        self.assertEqual(profile["disks"]["sda"]["size_gb"], 250.0)

        # 4. Verify Network
        self.assertEqual(len(profile["network"]), 1)
        self.assertEqual(profile["network"][0]["name"], "eth0")
        self.assertEqual(profile["network"][0]["operstate"], "up")
        self.assertEqual(profile["network"][0]["speed_mbps"], 10000)

        # 5. Verify Mounts
        self.assertEqual(len(profile["mounts"]), 1)
        self.assertEqual(profile["mounts"][0]["mount_point"], "/")


if __name__ == "__main__":
    unittest.main()
