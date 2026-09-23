# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for honeydew.affordances.ui.screenshot_image.py."""

import pathlib
import tempfile
import unittest
from array import array

import png
from honeydew.affordances.ui.screenshot.types import ScreenshotImage
from honeydew.affordances.ui.user_input.types import Pixel, Size
from parameterized import param, parameterized

# Disabling pylint to reduce verbosity for widely used trivial types
# pylint: disable=g-importing-member


# pylint: enable=g-importing-member

RED = Pixel(255, 0, 0)
GREEN = Pixel(0, 255, 0)
BLUE = Pixel(0, 0, 255)
BLACK = Pixel(0, 0, 0)
WHITE = Pixel(255, 255, 255)
TRANSPARENT = Pixel(0, 0, 0, 0)


def rgba(pixel: Pixel) -> list[int]:
    """Converts the given pixel into a rgba list of ints"""
    return [pixel.red, pixel.green, pixel.blue, pixel.alpha]


def bgra(pixel: Pixel) -> list[int]:
    """Converts the given pixel into a bgra list of ints"""
    return [pixel.blue, pixel.green, pixel.red, pixel.alpha]


def rgba_data(*pixels: Pixel) -> bytes:
    """Converts the given pixels into a rgba byte array"""
    out = bytearray()
    for p in pixels:
        out.extend(rgba(p))
    return bytes(out)


def bgra_data(*pixels: Pixel) -> bytes:
    """Converts the given pixels into a bgra byte array"""
    out = bytearray()
    for p in pixels:
        out.extend(bgra(p))
    return bytes(out)


class ScreenshotImageTest(unittest.TestCase):
    """Unit tests for honeydew.affordances.ui.ScreenshotImage"""

    def test_ctor(self) -> None:
        image = ScreenshotImage(size=Size(1, 1), data=rgba_data(GREEN))
        self.assertEqual(image.size, Size(1, 1))
        self.assertEqual(image.data, rgba_data(GREEN))

    def test_ctor_empty_image(self) -> None:
        image = ScreenshotImage(size=Size(0, 0), data=b"")
        self.assertEqual(image.size, Size(0, 0))
        self.assertEqual(image.data, b"")

    def test_ctor_bad_args(self) -> None:
        with self.assertRaisesRegex(ValueError, "Invalid image size.*"):
            ScreenshotImage(size=Size(-1, 1), data=rgba_data(GREEN))

        with self.assertRaisesRegex(ValueError, "Invalid image size.*"):
            ScreenshotImage(size=Size(1, -1), data=rgba_data(GREEN))

        with self.assertRaisesRegex(
            ValueError, "Data length must be a multiple of 4"
        ):
            ScreenshotImage(size=Size(1, 1), data=bytes([0, 0, 0]))

        with self.assertRaisesRegex(ValueError, "Expected data length 24.*"):
            ScreenshotImage(
                size=Size(2, 3), data=rgba_data(GREEN, GREEN, GREEN, GREEN)
            )

    def test_save_bgra(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            image_file = pathlib.Path(tmpdir) / "image.bgra"
            image = ScreenshotImage(
                size=Size(2, 2), data=rgba_data(GREEN, BLUE, RED, TRANSPARENT)
            )
            image.save(str(image_file))
            self.assertEqual(
                image_file.read_bytes(),
                bytes(bgra(GREEN) + bgra(BLUE) + bgra(RED) + bgra(TRANSPARENT)),
            )

    def test_save_png(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            image_file = pathlib.Path(tmpdir) / "image.png"
            image = ScreenshotImage(
                size=Size(3, 2),
                data=rgba_data(GREEN, BLUE, RED, TRANSPARENT, BLACK, WHITE),
            )
            image.save(str(image_file))

            (actual_width, actual_height, actual_data, _) = png.Reader(
                str(image_file)
            ).read_flat()
            self.assertEqual(actual_width, 3)
            self.assertEqual(actual_height, 2)
            self.assertEqual(actual_data, array("B", image.data))

    def test_save_unsupported_file_type(self) -> None:
        image = ScreenshotImage(size=Size(1, 1), data=rgba_data(GREEN))
        with self.assertRaisesRegex(
            ValueError, "Only .* files are supported.*"
        ):
            image.save("foo.jpg")

    def test_load_from_path_bgra(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            image_file = pathlib.Path(tmpdir) / "image.bgra"

            # Prepare a a 2x3 image:
            image_file.write_bytes(
                bgra_data(GREEN, BLUE, RED, TRANSPARENT, BLACK, WHITE)
            )

            image = ScreenshotImage.load_from_path(str(image_file))
            # bgra files have no size information, so read as a 6x1 image:
            self.assertEqual(image.size.width, 6)
            self.assertEqual(image.size.height, 1)
            self.assertEqual(
                rgba_data(GREEN, BLUE, RED, TRANSPARENT, BLACK, WHITE),
                image.data,
            )

    def test_load_from_path_png(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            image_file = pathlib.Path(tmpdir) / "image.png"

            # Prepare a 2x3 image
            png.from_array(
                [
                    rgba(GREEN) + rgba(BLUE),
                    rgba(RED) + rgba(TRANSPARENT),
                    rgba(BLACK) + rgba(WHITE),
                ],
                mode="RGBA",
            ).save(str(image_file))

            image = ScreenshotImage.load_from_path(str(image_file))
            self.assertEqual(image.size.width, 2)
            self.assertEqual(image.size.height, 3)
            self.assertEqual(
                image.data,
                rgba_data(GREEN, BLUE, RED, TRANSPARENT, BLACK, WHITE),
            )

    def test_load_from_path_unsupported_file_type(self) -> None:
        with self.assertRaisesRegex(
            ValueError, "Only .* files are supported.*"
        ):
            ScreenshotImage.load_from_path("foo.jpg")

    @parameterized.expand([param(suffix="png"), param(suffix="bgra")])
    def test_load_from_resource(self, suffix: str) -> None:
        # Unfortunately cannot just import the `resources` subpackage since
        # it has a different absolute package name when the test runs in `fx test`
        # (just "resources") and in `conformance.sh` (named `tests.unit_tests.affordances_tests.ui.resources`)
        # so instead we compute it relative to __name__.
        resources_package_name = ".".join(
            __name__.split(".")[0:-1] + ["resources"]
        )
        image = ScreenshotImage.load_from_resource(
            resources_package_name, f"one_by_one_green.{suffix}"
        )
        self.assertEqual(image.size.width, 1)
        self.assertEqual(image.size.height, 1)
        self.assertEqual(image.data, rgba_data(GREEN))

    def test_load_from_resource_only_bgra_is_supported(self) -> None:
        with self.assertRaisesRegex(
            ValueError, "Only .* files are supported.*"
        ):
            ScreenshotImage.load_from_resource("foo", "bar.jpg")

    def test_get_pixel(self) -> None:
        image_bytes = rgba_data(RED, GREEN, BLUE, WHITE)
        image = ScreenshotImage(size=Size(2, 2), data=image_bytes)

        self.assertEqual(image.get_pixel(0, 0), RED)
        self.assertEqual(image.get_pixel(1, 0), GREEN)
        self.assertEqual(image.get_pixel(0, 1), BLUE)
        self.assertEqual(image.get_pixel(1, 1), WHITE)

        image = ScreenshotImage(size=Size(1, 1), data=rgba_data(RED))
        self.assertEqual(image.get_pixel(0, 0), RED)

    @parameterized.expand(
        [
            param(x=2, y=0),
            param(x=0, y=2),
            param(x=-1, y=0),
            param(x=0, y=-1),
        ]
    )
    def test_get_pixel_out_of_bounds(self, x: int, y: int) -> None:
        image_bytes = rgba_data(RED, GREEN, BLUE, WHITE)
        image = ScreenshotImage(size=Size(2, 2), data=image_bytes)

        with self.assertRaisesRegex(
            ValueError, "Pixel coordinates.*are outside of image.*"
        ):
            image.get_pixel(x, y)

    def test_histogram(self) -> None:
        image_bytes = rgba_data(RED, RED, BLUE)
        image = ScreenshotImage(size=Size(3, 1), data=image_bytes)

        self.assertDictEqual(
            image.histogram(),
            {
                RED: 2,
                BLUE: 1,
            },
        )

    @parameterized.expand(
        [
            param(
                pixels1=[RED, WHITE, BLUE, GREEN],
                pixels2=[RED, WHITE, BLUE, GREEN],
                expected_similarity=1.0,
            ),
            param(
                [RED, WHITE, BLUE, GREEN],
                [RED, WHITE, BLUE, BLACK],
                0.75,
            ),
            param(
                [RED, WHITE, BLUE, GREEN],
                [WHITE, RED, GREEN, GREEN],
                0.25,
            ),
            param(
                [RED, WHITE, BLUE, GREEN],
                [WHITE, RED, GREEN, RED],
                0.0,
            ),
        ]
    )
    def test_pixel_similarity(
        self,
        pixels1: list[Pixel],
        pixels2: list[Pixel],
        expected_similarity: float,
    ) -> None:
        image1 = ScreenshotImage(size=Size(2, 2), data=rgba_data(*pixels1))
        image2 = ScreenshotImage(size=Size(2, 2), data=rgba_data(*pixels2))
        self.assertEqual(
            image1.pixel_similarity(image2),
            expected_similarity,
        )

    def test_pixel_similarity_different_sized_images(self) -> None:
        image1 = ScreenshotImage(size=Size(1, 1), data=rgba_data(RED))
        image2 = ScreenshotImage(size=Size(2, 1), data=rgba_data(GREEN, RED))

        with self.assertRaises(ValueError):
            image1.pixel_similarity(image2)

    @parameterized.expand(
        [
            param(
                pixels1=[WHITE, RED, GREEN, BLUE],
                pixels2=[RED, WHITE, BLUE, GREEN],
                expected_similarity=1.0,
            ),
            param(
                pixels1=[WHITE, RED, TRANSPARENT, GREEN],
                pixels2=[RED, WHITE, BLUE, BLACK],
                expected_similarity=0.5,
            ),
            param(
                pixels1=[RED, RED, WHITE, WHITE],
                pixels2=[BLACK, BLACK, BLACK, RED],
                expected_similarity=0.25,
            ),
            param(
                pixels1=[RED, WHITE, BLUE, BLUE],
                pixels2=[BLACK, GREEN, TRANSPARENT, TRANSPARENT],
                expected_similarity=0.0,
            ),
        ]
    )
    def test_histogram_similarity(
        self,
        pixels1: list[Pixel],
        pixels2: list[Pixel],
        expected_similarity: float,
    ) -> None:
        image1 = ScreenshotImage(size=Size(2, 2), data=rgba_data(*pixels1))
        image2 = ScreenshotImage(size=Size(2, 2), data=rgba_data(*pixels2))
        self.assertEqual(
            image1.histogram_similarity(image2),
            expected_similarity,
        )

    def test_histogram_similarity_different_sized_images(self) -> None:
        image1 = ScreenshotImage(size=Size(1, 1), data=rgba_data(RED))
        image2 = ScreenshotImage(size=Size(2, 1), data=rgba_data(GREEN, RED))

        with self.assertRaises(ValueError):
            image1.histogram_similarity(image2)
