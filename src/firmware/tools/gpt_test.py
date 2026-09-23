#!/usr/bin/env fuchsia-vendored-python

# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for gpt.py."""

import dataclasses
import pathlib
import re
import tempfile
import unittest
from typing import Dict, Tuple

import gpt

_MY_DIR = pathlib.Path(__file__).parent

# A real disk image for testing against, generated via:
#  $ dd if=/dev/zero of=gpt_test_disk.img bs=4M count=1
#  $ sgdisk gpt_test_disk.img \
#      --clear \
#      --new=0:+0:+1MiB \
#      --change-name=1:foo \
#      --typecode=1:9B37FFF6-2E58-466A-983A-F7926D0B04E0 \
#      --partition-guid=1:A271DF36-BAAC-45B7-B806-A636996DAF75 \
#      --new=0:+0:+1MiB \
#      --change-name=2:bar \
#      --typecode=2:421A8BFC-85D9-4D85-ACDA-B64EEC0133E9 \
#      --partition-guid=2:8F1ED054-0475-43FE-AB8D-13C6E709D5A7
_TEST_DISK_PATH = _MY_DIR / "gpt_test_disk.img"

# Expected partitions based on the above disk image.
_TEST_DISK_FOO_PARTITION = gpt.Partition(
    index=1,
    name="foo",
    first_lba=2048,
    last_lba=4095,
    size_lba=2048,
    type_guid="9B37FFF6-2E58-466A-983A-F7926D0B04E0",
    unique_guid="A271DF36-BAAC-45B7-B806-A636996DAF75",
    attr_flags="0000000000000000",
)
_TEST_DISK_BAR_PARTITION = gpt.Partition(
    index=2,
    name="bar",
    first_lba=4096,
    last_lba=6143,
    size_lba=2048,
    type_guid="421A8BFC-85D9-4D85-ACDA-B64EEC0133E9",
    unique_guid="8F1ED054-0475-43FE-AB8D-13C6E709D5A7",
    attr_flags="0000000000000000",
)
_TEST_DISK_PARTITIONS = [_TEST_DISK_FOO_PARTITION, _TEST_DISK_BAR_PARTITION]

# The matching input spec that should generate the above image.
_TEST_DISK_SPEC = {
    "size_mib": 4,
    "block_size": 512,
    "alignment_kib": 1024,
    "partitions": [
        {
            "name": "foo",
            "size_kib": 1024,
            "unique_guid": "A271DF36-BAAC-45B7-B806-A636996DAF75",
            "type_guid": "9B37FFF6-2E58-466A-983A-F7926D0B04E0",
        },
        {
            "name": "bar",
            "size_kib": 1024,
            "unique_guid": "8F1ED054-0475-43FE-AB8D-13C6E709D5A7",
            "type_guid": "421A8BFC-85D9-4D85-ACDA-B64EEC0133E9",
        },
    ],
}


def default_test_spec(
    num_partitions: int, *, disk_size_mib: int = 1, part_size_kib: int = 64
) -> Dict:
    """Returns a partition spec for testing.

    The idea is that this returns a reasonable default, and tests can then
    modify just the pieces that they want to exercise.
    """
    return {
        "size_mib": 1,
        "block_size": 512,
        "alignment_kib": 1,
        "partitions": [
            {
                "name": f"part_{i}",
                "size_kib": part_size_kib,
                "unique_guid": "00000000-1111-2222-3333-444444444444",
                "type_guid": "55555555-6666-7777-8888-999999999999",
            }
            for i in range(num_partitions)
        ],
    }


def create_and_read_gpt(spec: Dict) -> Tuple[Dict, str]:
    """Wraps create_gpt() to also fetch the resulting partitions.

    create_gpt() provides the raw mbr/primary/backup blobs, this
    function re-assembles the blobs into a disk image and calls
    get_partition() so we can verify the blobs were correct.

    Args:
        spec: specification to pass to create_gpt()

    Returns:
        A tuple containing:
            * The result from get_partitions()
            * The create_gpt() README contents
    """
    with tempfile.TemporaryDirectory() as temp_dir:
        temp_dir = pathlib.Path(temp_dir)
        gpt.create_gpt(spec, temp_dir)

        # Re-assemble the disk so we can use get_partitions() to validate.
        mbr = (temp_dir / "mbr.bin").read_bytes()
        primary = (temp_dir / "primary.bin").read_bytes()
        backup = (temp_dir / "backup.bin").read_bytes()
        disk_size = spec["size_mib"] * 1024 * 1024

        image_path = temp_dir / "result.img"
        with open(image_path, "wb") as image_file:
            image_file.write(mbr)
            image_file.write(primary)
            image_file.truncate(disk_size - len(backup))
            image_file.write(backup)

        result = gpt.get_partitions(image_path)
        readme = (temp_dir / "README.md").read_text()
        return (result, readme)


class GptTests(unittest.TestCase):
    def test_get_partitions(self):
        self.assertEqual(
            gpt.get_partitions(_TEST_DISK_PATH), _TEST_DISK_PARTITIONS
        )

    def test_get_partition_by_name(self):
        self.assertEqual(
            gpt.get_partition(_TEST_DISK_PATH, part_name="foo"),
            _TEST_DISK_FOO_PARTITION,
        )

    def test_get_partition_by_index(self):
        self.assertEqual(
            gpt.get_partition(_TEST_DISK_PATH, index=2),
            _TEST_DISK_BAR_PARTITION,
        )

    def test_get_partition_by_name_and_index(self):
        self.assertEqual(
            gpt.get_partition(_TEST_DISK_PATH, part_name="bar", index=2),
            _TEST_DISK_BAR_PARTITION,
        )

    def test_get_partition_by_name_not_found(self):
        with self.assertRaises(ValueError):
            gpt.get_partition(_TEST_DISK_PATH, part_name="not_a_partition")

    def test_get_partition_by_index_not_found(self):
        with self.assertRaises(ValueError):
            gpt.get_partition(_TEST_DISK_PATH, index=5)

    def test_get_partition_by_name_and_index_bad_name(self):
        with self.assertRaises(ValueError):
            gpt.get_partition(_TEST_DISK_PATH, part_name="invalid", index=1)

    def test_get_partition_by_name_and_index_bad_index(self):
        with self.assertRaises(ValueError):
            gpt.get_partition(_TEST_DISK_PATH, part_name="bar", index=4)

    def test_create_gpt(self):
        result, readme = create_and_read_gpt(_TEST_DISK_SPEC)

        self.assertEqual(result, _TEST_DISK_PARTITIONS)

        # Check that our README at least contains a dump of all the partition
        # information by making sure all the values exist somewhere in the text.
        for part in _TEST_DISK_PARTITIONS:
            for value in dataclasses.asdict(part).values():
                self.assertIn(str(value), readme)

    def test_create_gpt_fill(self):
        spec = default_test_spec(
            num_partitions=3, disk_size_mib=1, part_size_kib=64
        )
        spec["partitions"][2]["size_kib"] = "fill"

        result, _ = create_and_read_gpt(spec)

        # The "fill" partition should take up the rest of the partition.
        # 2048 disk - 1 MBR - 33 GPT - 128 part_1 - 128 part_2 - 33 backup GPT.
        self.assertEqual(result[2].size_lba, 1725)

    def test_create_gpt_type_map(self):
        spec = default_test_spec(num_partitions=3)
        spec["partitions"][0]["type_guid"] = "zircon"
        spec["partitions"][1]["type_guid"] = "vbmeta"
        spec["partitions"][2]["type_guid"] = "fvm"

        result, _ = create_and_read_gpt(spec)

        self.assertEqual(
            result[0].type_guid, "9B37FFF6-2E58-466A-983A-F7926D0B04E0"
        )
        self.assertEqual(
            result[1].type_guid, "421A8BFC-85D9-4D85-ACDA-B64EEC0133E9"
        )
        self.assertEqual(
            result[2].type_guid, "49FD7CB8-DF15-4E73-B9D9-992070127F0F"
        )

    def test_create_gpt_random_unique_guid(self):
        spec = default_test_spec(num_partitions=2)
        spec["partitions"][0].pop("unique_guid")
        spec["partitions"][1].pop("unique_guid")

        result, _ = create_and_read_gpt(spec)

        self.assertTrue(re.match(gpt.GUID_RE, result[0].unique_guid))
        self.assertTrue(re.match(gpt.GUID_RE, result[1].unique_guid))
        self.assertNotEqual(result[0].unique_guid, result[1].unique_guid)


if __name__ == "__main__":
    unittest.main()
