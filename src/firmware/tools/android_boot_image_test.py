#!/usr/bin/env fuchsia-vendored-python

# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for android_boot_image.py."""

import pathlib
import tempfile
import unittest

import android_boot_image
import test_utils
from android_boot_image import AndroidBootImage, ChunkType

# Contents written to images by `generate_testdata.py`
_TEST_KERNEL = b"test kernel contents"
_TEST_RAMDISK = b"test ramdisk contents"
_TEST_DTB = b"test dtb contents"


def test_image(version: int, vendor: bool) -> bytes:
    """Returns the test_boot_image.bin contents"""
    image_type = "vendor_boot" if vendor else "boot"
    return test_utils.load_test_data(
        pathlib.Path("android_boot_image")
        / f"test_{image_type}_image_v{version}.bin"
    )


def create_boot_image(
    version: int,
    vendor: bool,
    chunks: dict[ChunkType, bytes],
) -> AndroidBootImage:
    """Helper to create a boot image with the given chunk contents.

    Any chunks not specified in `chunks` will be empty.
    """
    # Ideally this would generate a fresh image via `mkbootimg` but it's difficult
    # to access that script from this test, so modify our test image instead.
    #
    # This assumes that boot image modification is working properly, so we need
    # at least a few tests that do not use this function but instead use the
    # generated test image directly.
    image = android_boot_image.load_images(test_image(version, vendor))[0]
    # Make sure we're not trying to assign chunks that don't exist in this image
    # type/version.
    if not chunks.keys() <= set([chunk.chunk_type for chunk in image.chunks]):
        raise ValueError(
            f"Image cannot support the requested chunk types {chunks}"
        )
    for chunk_type in ChunkType:
        new_data = chunks.get(chunk_type, b"")
        if (
            image.has_chunk(chunk_type)
            and image.get_chunk_data(chunk_type) != new_data
        ):
            image.replace_chunk(chunk_type, new_data)
    return image


class BootImageTests(unittest.TestCase):
    def assert_chunks(
        self,
        image: android_boot_image.AndroidBootImage,
        expected: list[tuple[ChunkType, bytes]],
    ) -> None:
        """Asserts that `image` contains exactly `expected` chunks."""
        self.assertEqual(len(image.chunks), len(expected))
        for chunk_type, contents in expected:
            self.assertEqual(image.get_chunk_data(chunk_type), contents)

    def test_load_boot_image_v2(self) -> None:
        """Loads a v2 boot image and verifies the chunk contents."""
        images = android_boot_image.load_images(
            test_image(version=2, vendor=False)
        )
        self.assertEqual(len(images), 1)
        image = images[0]

        # Expected contents taken from `generate_testdata.py`.
        self.assert_chunks(
            image,
            [
                (ChunkType.KERNEL, _TEST_KERNEL),
                (ChunkType.RAMDISK, _TEST_RAMDISK),
                (ChunkType.SECOND, b""),
                (ChunkType.RECOVERY_DTBO, b""),
                (ChunkType.DTB, _TEST_DTB),
            ],
        )

        # Header and non-empty chunks should each take up 1 4096-byte page.
        self.assertEqual(image.total_size, 4096 * 4)

    def test_load_boot_image_v4(self) -> None:
        """Loads a v4 boot image and verifies the chunk contents."""
        images = android_boot_image.load_images(
            test_image(version=4, vendor=False)
        )
        self.assertEqual(len(images), 1)
        image = images[0]

        # Expected contents taken from `generate_testdata.py`.
        self.assert_chunks(
            image,
            [
                (ChunkType.KERNEL, _TEST_KERNEL),
                (ChunkType.RAMDISK, _TEST_RAMDISK),
                (ChunkType.SIGNATURE, b""),
            ],
        )

        # Header and non-empty chunks should each take up 1 4096-byte page.
        self.assertEqual(image.total_size, 4096 * 3)

    def test_load_vendor_boot_image_v4(self) -> None:
        """Loads a v4 vendor boot image and verifies the chunk contents."""
        images = android_boot_image.load_images(
            test_image(version=4, vendor=True)
        )
        self.assertEqual(len(images), 1)
        image = images[0]

        # Expected contents taken from `generate_testdata.py`.
        self.assert_chunks(
            image,
            [
                # We don't yet support vendor ramdisks, we'd need to add logic
                # for ramdisk table metadata as well.
                (ChunkType.RAMDISK, b""),
                (ChunkType.DTB, _TEST_DTB),
                (ChunkType.RAMDISK_TABLE, b""),
                (ChunkType.BOOTCONFIG, b""),
            ],
        )

        # Header and non-empty chunks should each take up 1 4096-byte page.
        self.assertEqual(image.total_size, 4096 * 2)

    def test_replace_chunk(self) -> None:
        """Replaces a chunk with new data."""
        image = android_boot_image.load_images(
            test_image(version=2, vendor=False)
        )[0]

        image.replace_chunk(ChunkType.RAMDISK, b"new ramdisk")

        self.assertEqual(
            image.get_chunk_data(ChunkType.RAMDISK), b"new ramdisk"
        )

    def test_load_two_boot_images(self) -> None:
        """Loads two concatenated boot images."""
        image1 = create_boot_image(
            version=2,
            vendor=False,
            chunks={ChunkType.KERNEL: b"kernel1", ChunkType.SECOND: b"second1"},
        )
        image2 = create_boot_image(
            version=2,
            vendor=False,
            chunks={ChunkType.KERNEL: b"kernel2", ChunkType.DTB: b"dtb2"},
        )
        combined_contents = image1.image + image2.image

        images = android_boot_image.load_images(combined_contents)
        self.assertEqual(len(images), 2)

        self.assert_chunks(
            images[0],
            [
                (ChunkType.KERNEL, b"kernel1"),
                (ChunkType.RAMDISK, b""),
                (ChunkType.SECOND, b"second1"),
                (ChunkType.RECOVERY_DTBO, b""),
                (ChunkType.DTB, b""),
            ],
        )

        self.assert_chunks(
            images[1],
            [
                (ChunkType.KERNEL, b"kernel2"),
                (ChunkType.RAMDISK, b""),
                (ChunkType.SECOND, b""),
                (ChunkType.RECOVERY_DTBO, b""),
                (ChunkType.DTB, b"dtb2"),
            ],
        )

    def test_load_different_boot_images(self) -> None:
        """Loads concatenated boot images of different types."""
        image1 = create_boot_image(
            version=2,
            vendor=False,
            chunks={ChunkType.KERNEL: b"kernel1", ChunkType.SECOND: b"second1"},
        )
        image2 = create_boot_image(
            version=4,
            vendor=True,
            chunks={
                ChunkType.DTB: b"dtb2",
                ChunkType.BOOTCONFIG: b"bootconfig2",
            },
        )
        image3 = create_boot_image(
            version=4,
            vendor=False,
            chunks={
                ChunkType.KERNEL: b"kernel3",
                ChunkType.RAMDISK: b"ramdisk3",
            },
        )
        combined_contents = image1.image + image2.image + image3.image

        images = android_boot_image.load_images(combined_contents)
        self.assertEqual(len(images), 3)

        self.assert_chunks(
            images[0],
            [
                (ChunkType.KERNEL, b"kernel1"),
                (ChunkType.RAMDISK, b""),
                (ChunkType.SECOND, b"second1"),
                (ChunkType.RECOVERY_DTBO, b""),
                (ChunkType.DTB, b""),
            ],
        )

        self.assert_chunks(
            images[1],
            [
                (ChunkType.RAMDISK, b""),
                (ChunkType.DTB, b"dtb2"),
                (ChunkType.RAMDISK_TABLE, b""),
                (ChunkType.BOOTCONFIG, b"bootconfig2"),
            ],
        )

        self.assert_chunks(
            images[2],
            [
                (ChunkType.KERNEL, b"kernel3"),
                (ChunkType.RAMDISK, b"ramdisk3"),
                (ChunkType.SIGNATURE, b""),
            ],
        )

    def test_commandline_split(self) -> None:
        """Tests the "--split" commandline flag."""
        with tempfile.TemporaryDirectory() as temp_dir_str:
            temp_dir = pathlib.Path(temp_dir_str)

            images = [
                create_boot_image(
                    version=2,
                    vendor=False,
                    chunks={
                        ChunkType.KERNEL: b"kernel1",
                        ChunkType.SECOND: b"second1",
                    },
                ),
                create_boot_image(
                    version=2,
                    vendor=False,
                    chunks={
                        ChunkType.KERNEL: b"kernel2",
                        ChunkType.DTB: b"dtb2",
                    },
                ),
            ]
            combined_contents = images[0].image + images[1].image

            input_path = temp_dir / "boot.img"
            input_path.write_bytes(combined_contents)

            android_boot_image.main([str(input_path), "--split"])

            self.assertEqual(
                (temp_dir / "boot.img.0").read_bytes(), images[0].image
            )
            self.assertEqual(
                (temp_dir / "boot.img.1").read_bytes(), images[1].image
            )

    def test_commandline_dump_ramdisk(self) -> None:
        """Tests the "dump ramdisk" commandline option."""
        with tempfile.TemporaryDirectory() as temp_dir_str:
            temp_dir = pathlib.Path(temp_dir_str)
            input_path = temp_dir / "boot.img"
            output_path = temp_dir / "ramdisk.img"

            input_path.write_bytes(
                create_boot_image(
                    version=2,
                    vendor=False,
                    chunks={
                        ChunkType.KERNEL: b"test_kernel",
                        ChunkType.RAMDISK: b"test_ramdisk",
                    },
                ).image
            )

            android_boot_image.main(
                [str(input_path), "--dump_ramdisk", str(output_path)]
            )

            self.assertEqual(output_path.read_bytes(), b"test_ramdisk")

    def test_commandline_dump_ramdisk_multiple_images(self) -> None:
        """Tests the "dump ramdisk" commandline option on multiple images."""
        with tempfile.TemporaryDirectory() as temp_dir_str:
            temp_dir = pathlib.Path(temp_dir_str)
            input_path = temp_dir / "boot.img"
            output_path = temp_dir / "ramdisk.img"

            input_path.write_bytes(
                create_boot_image(
                    version=2,
                    vendor=False,
                    chunks={ChunkType.RAMDISK: b"ramdisk0"},
                ).image
                + create_boot_image(
                    version=2,
                    vendor=False,
                    chunks={ChunkType.RAMDISK: b"ramdisk1"},
                ).image
            )

            # Dump ramdisk from the first image.
            android_boot_image.main(
                [
                    str(input_path),
                    "--dump_ramdisk",
                    str(output_path),
                    "--select_image",
                    "0",
                ]
            )
            self.assertEqual(output_path.read_bytes(), b"ramdisk0")

            # Dump ramdisk from the second image.
            android_boot_image.main(
                [
                    str(input_path),
                    "--dump_ramdisk",
                    str(output_path),
                    "--select_image",
                    "1",
                ]
            )
            self.assertEqual(output_path.read_bytes(), b"ramdisk1")

    def test_commandline_replace_ramdisk(self) -> None:
        """Tests the "replace ramdisk" commandline option."""
        with tempfile.TemporaryDirectory() as temp_dir_str:
            temp_dir = pathlib.Path(temp_dir_str)
            input_path = temp_dir / "boot.img"
            ramdisk_path = temp_dir / "ramdisk.img"

            input_path.write_bytes(
                create_boot_image(
                    version=2,
                    vendor=False,
                    chunks={
                        ChunkType.KERNEL: b"kernel",
                        ChunkType.RAMDISK: b"ramdisk",
                        ChunkType.DTB: b"DTB",
                    },
                ).image
            )
            ramdisk_path.write_bytes(b"new ramdisk")

            android_boot_image.main(
                [str(input_path), "--replace_ramdisk", str(ramdisk_path)]
            )

            # Re-load the file and make sure only the ramdisk was modified.
            images = android_boot_image.load_images(input_path.read_bytes())
            self.assertEqual(len(images), 1)

            self.assert_chunks(
                images[0],
                [
                    (ChunkType.KERNEL, b"kernel"),
                    (ChunkType.RAMDISK, b"new ramdisk"),
                    (ChunkType.SECOND, b""),
                    (ChunkType.RECOVERY_DTBO, b""),
                    (ChunkType.DTB, b"DTB"),
                ],
            )

    def test_commandline_replace_ramdisk_multiple_images(self) -> None:
        """Tests the "replace ramdisk" commandline option on multiple images."""
        with tempfile.TemporaryDirectory() as temp_dir_str:
            temp_dir = pathlib.Path(temp_dir_str)
            input_path = temp_dir / "boot.img"
            ramdisk_path = temp_dir / "ramdisk0.img"

            input_path.write_bytes(
                create_boot_image(
                    version=4,
                    vendor=False,
                    chunks={ChunkType.RAMDISK: b"ramdisk0"},
                ).image
                + create_boot_image(
                    version=2,
                    vendor=False,
                    chunks={ChunkType.RAMDISK: b"ramdisk1"},
                ).image
            )

            # Replace ramdisk on the first image.
            ramdisk_path.write_bytes(b"new ramdisk 0")
            android_boot_image.main(
                [
                    str(input_path),
                    "--replace_ramdisk",
                    str(ramdisk_path),
                    "--select_image",
                    "0",
                ]
            )

            # Replace ramdisk on the second image.
            ramdisk_path.write_bytes(b"new ramdisk 1")
            android_boot_image.main(
                [
                    str(input_path),
                    "--replace_ramdisk",
                    str(ramdisk_path),
                    "--select_image",
                    "1",
                ]
            )

            # Re-load the file and make sure it has the expected changes.
            images = android_boot_image.load_images(input_path.read_bytes())
            self.assertEqual(len(images), 2)

            self.assert_chunks(
                images[0],
                [
                    (ChunkType.KERNEL, b""),
                    (ChunkType.RAMDISK, b"new ramdisk 0"),
                    (ChunkType.SIGNATURE, b""),
                ],
            )

            self.assert_chunks(
                images[1],
                [
                    (ChunkType.KERNEL, b""),
                    (ChunkType.RAMDISK, b"new ramdisk 1"),
                    (ChunkType.SECOND, b""),
                    (ChunkType.RECOVERY_DTBO, b""),
                    (ChunkType.DTB, b""),
                ],
            )

    def test_commandline_multiple_images_required_selection(self) -> None:
        """Interacting with multiple images requires explicit selection."""
        with tempfile.TemporaryDirectory() as temp_dir_str:
            temp_dir = pathlib.Path(temp_dir_str)
            input_path = temp_dir / "boot.img"
            output_path = temp_dir / "ramdisk.img"

            input_path.write_bytes(
                create_boot_image(
                    version=2,
                    vendor=False,
                    chunks={ChunkType.RAMDISK: b"ramdisk0"},
                ).image
                + create_boot_image(
                    version=2,
                    vendor=False,
                    chunks={ChunkType.RAMDISK: b"ramdisk1"},
                ).image
            )

            with self.assertRaises(ValueError) as error:
                android_boot_image.main(
                    [str(input_path), "--dump_ramdisk", str(output_path)]
                )
                self.assertIn("you must select", str(error))

            with self.assertRaises(ValueError) as error:
                android_boot_image.main(
                    [str(input_path), "--replace_ramdisk", str(output_path)]
                )
                self.assertIn("you must select", str(error))


if __name__ == "__main__":
    unittest.main()
