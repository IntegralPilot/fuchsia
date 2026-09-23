# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import tempfile
import unittest
from pathlib import Path
from typing import List

import debug_json
import smaps
import snapshots


def setup_workdir(workdir: Path) -> None:
    linux_dir = workdir / "linux"
    linux_dir.mkdir()
    (linux_dir / "android-ps.txt").write_text(
        """    PID ARGS
    185 netd
    186 zygote64
"""
    )
    (linux_dir / "proc" / "185").mkdir(parents=True)
    (linux_dir / "proc" / "185" / "smaps").write_text(
        """56354da90000-56354db31000 r--p 00000000 fe:21 13                         /apex/com.android.adbd/bin/adbd
Size:                644 kB
KernelPageSize:        4 kB
MMUPageSize:           4 kB
Rss:                  80 kB
Pss:                  80 kB
Pss_Dirty:             0 kB
Shared_Clean:          0 kB
Shared_Dirty:          0 kB
Private_Clean:        80 kB
Private_Dirty:         0 kB
Referenced:           32 kB
Anonymous:             0 kB
KSM:                   0 kB
LazyFree:              0 kB
AnonHugePages:         0 kB
ShmemPmdMapped:        0 kB
FilePmdMapped:         0 kB
Shared_Hugetlb:        0 kB
Private_Hugetlb:       0 kB
Swap:                  0 kB
SwapPss:               0 kB
Locked:                0 kB
THPeligible:           0
VmFlags: rd mr mw me
"""
    )

    fuchsia_dir = workdir / "fuchsia"
    fuchsia_dir.mkdir()

    (fuchsia_dir / "ffx-profile-memory-debug-json.json").write_text(
        json.dumps(
            dict(
                Capture=dict(
                    Time=1,
                    Processes=[],
                    Kernel=dict(
                        total=2,
                        free=2,
                        wired=2,
                        total_heap=2,
                        free_heap=2,
                        vmo=2,
                        mmu=2,
                        ipc=2,
                        other=2,
                        vmo_pager_total=2,
                        vmo_pager_newest=2,
                        vmo_pager_oldest=2,
                        vmo_discardable_locked=2,
                        vmo_discardable_unlocked=2,
                        vmo_reclaim_disabled=2,
                    ),
                    VmoNames=[],
                    Vmos=[
                        [
                            "koid",
                            "name",
                            "parent_koid",
                            "committed_bytes",
                            "allocated_bytes",
                            "populated_bytes",
                        ],
                    ],
                    kmem_stats_compression=dict(
                        uncompressed_storage_bytes=3,
                        compressed_storage_bytes=3,
                        compressed_fragmentation_bytes=3,
                        compression_time=3,
                        decompression_time=3,
                        total_page_compression_attempts=3,
                        failed_page_compression_attempts=3,
                        total_page_decompressions=3,
                        compressed_page_evictions=3,
                        eager_page_compressions=3,
                        memory_pressure_page_compressions=3,
                        critical_memory_page_compressions=3,
                        pages_decompressed_unit_ns=3,
                        pages_decompressed_within_log_time=[1, 2, 3, 4, 5],
                    ),
                ),
                Buckets=[dict(event_code=1, name="b1", process="p1", vmo="v1")],
            )
        )
    )
    (fuchsia_dir / "snapshot").mkdir()
    (fuchsia_dir / "snapshot" / "inspect.json").write_text(
        json.dumps(
            [
                dict(
                    moniker="core/starnix_runner/kernels",
                    payload=dict(
                        root=dict(
                            container=dict(
                                kernel=dict(
                                    thread_groups=dict(
                                        a=dict(koid=123, pid=321), b=dict()
                                    )
                                )
                            )
                        )
                    ),
                )
            ]
        )
    )

    (fuchsia_dir / "images.json").write_text(
        json.dumps(
            [
                dict(
                    type="type_a",
                    name="name_a",
                    path="path_a",
                    contents=dict(
                        maximum_contents_size=64,
                        packages=dict(
                            base=[
                                dict(
                                    name="package_a",
                                    manifest="manifest_a",
                                    blobs=[
                                        dict(
                                            merkle="merkle7890",
                                            path="p1",
                                            used_space_in_blobfs=75,
                                        )
                                    ],
                                )
                            ],
                            cache=[],
                        ),
                    ),
                ),
            ]
        )
    )


class SmapsTest(unittest.TestCase):
    def test_parse_one(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdirname:
            workdir = Path(tmpdirname)
            setup_workdir(workdir)

            pair = snapshots.Pair(workdir)
            self.assertEqual(pair.fuchsia.name, "fuchsia")
            self.assertEqual(pair.fuchsia.isfuchsia, True)
            self.assertEqual(pair.fuchsia.koid_by_pid, {321: 123})
            self.assertEqual(pair.fuchsia.pid_by_koid, {123: 321})
            self.assertTrue(isinstance(pair.fuchsia.images_json, List))
            self.assertEqual(
                pair.fuchsia.blob_file_by_vmo_name, {"blob-merkle78": {"p1"}}
            )

            self.assertEqual(pair.linux.name, "linux")
            self.assertEqual(pair.linux.isfuchsia, False)
            self.assertEqual(
                pair.linux.command_by_pid, {185: "netd", 186: "zygote64"}
            )
            self.assertEqual(
                pair.linux.pids_by_command, {"netd": [185], "zygote64": [186]}
            )
            self.assertEqual(
                pair.linux.smaps(185),
                [
                    smaps.SmapEntry(
                        addr_start="56354da90000",
                        addr_end="56354db31000",
                        perms="r--p",
                        offset="00000000",
                        dev="fe:21",
                        inode="13",
                        pathname="/apex/com.android.adbd/bin/adbd",
                        Size=644,
                        Rss=80,
                        Pss=80,
                    )
                ],
            )
            self.assertTrue(
                isinstance(
                    pair.fuchsia.debug_json, debug_json.ProfileMemoryDebug
                )
            )


if __name__ == "__main__":
    unittest.main()
