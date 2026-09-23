# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import tempfile
import unittest
from pathlib import Path
from typing import List

import smaps


def smap_parse(content: str) -> List[smaps.SmapEntry]:
    with tempfile.NamedTemporaryFile("wt") as tf:
        tf.write(content)
        tf.flush()
        return smaps.parse(Path(tf.name))


class SmapsTest(unittest.TestCase):
    def test_parse_empty(self) -> None:
        self.assertEqual(smap_parse(""), [])

    def test_parse_one(self) -> None:
        self.assertEqual(
            smap_parse(
                """ffffffffff600000-ffffffffff601000 --xp 00000000 00:00 0                  [vsyscall]
Size:                  4 kB
KernelPageSize:        4 kB
MMUPageSize:           4 kB
Rss:                   0 kB
Pss:                   0 kB
Pss_Dirty:             0 kB
Shared_Clean:          0 kB
Shared_Dirty:          0 kB
Private_Clean:         0 kB
Private_Dirty:         0 kB
Referenced:            0 kB
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
VmFlags: ex
"""
            ),
            [
                smaps.SmapEntry(
                    addr_start="ffffffffff600000",
                    addr_end="ffffffffff601000",
                    perms="--xp",
                    offset="00000000",
                    dev="00:00",
                    inode="0",
                    pathname="[vsyscall]",
                    Size=4,
                    Rss=0,
                    Pss=0,
                )
            ],
        )

    def test_two_entries(self) -> None:
        self.assertEqual(
            smap_parse(
                """7f97d3ce5000-7f97d3ce6000 r--p 00008000 fe:05 2059                       /system/lib64/libnetd_client.so
Size:                  4 kB
KernelPageSize:        4 kB
MMUPageSize:           4 kB
Rss:                   4 kB
Pss:                   4 kB
Pss_Dirty:             4 kB
Shared_Clean:          0 kB
Shared_Dirty:          0 kB
Private_Clean:         0 kB
Private_Dirty:         4 kB
Referenced:            4 kB
Anonymous:             4 kB
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
VmFlags: rd mr mw me ac
7f97d3ce6000-7f97d3ce9000 ---p 00000000 00:00 0
Size:                 12 kB
KernelPageSize:        4 kB
MMUPageSize:           4 kB
Rss:                   0 kB
Pss:                   0 kB
Pss_Dirty:             0 kB
Shared_Clean:          0 kB
Shared_Dirty:          0 kB
Private_Clean:         0 kB
Private_Dirty:         0 kB
Referenced:            0 kB
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
VmFlags: mr mw me
"""
            ),
            [
                smaps.SmapEntry(
                    addr_start="7f97d3ce5000",
                    addr_end="7f97d3ce6000",
                    perms="r--p",
                    offset="00008000",
                    dev="fe:05",
                    inode="2059",
                    pathname="/system/lib64/libnetd_client.so",
                    Size=4,
                    Rss=4,
                    Pss=4,
                ),
                smaps.SmapEntry(
                    addr_start="7f97d3ce6000",
                    addr_end="7f97d3ce9000",
                    perms="---p",
                    offset="00000000",
                    dev="00:00",
                    inode="0",
                    pathname=None,
                    Size=12,
                    Rss=0,
                    Pss=0,
                ),
            ],
        )


if __name__ == "__main__":
    unittest.main()
