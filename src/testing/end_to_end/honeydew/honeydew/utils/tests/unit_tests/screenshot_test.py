# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Tests for screenshot.py"""

import unittest

from honeydew.affordances.ui.screenshot.types import ScreenshotImage
from honeydew.affordances.ui.user_input.types import Size
from honeydew.utils import screenshot
from parameterized import param, parameterized


class ScreenshotUtilsTest(unittest.TestCase):
    def test_is_square(self) -> None:
        self.assertTrue(screenshot.is_square(_gradient_image(Size(5, 5))))
        self.assertFalse(screenshot.is_square(_gradient_image(Size(3, 1))))

    def test_change_dimensions(self) -> None:
        old_img = _gradient_image(Size(2, 2))
        new_image = screenshot.change_dimensions(old_img, Size(1, 4))
        self.assertEqual(old_img.data, new_image.data)

    def test_crop(self) -> None:
        old_img = _gradient_image(Size(10, 10))
        new_image = screenshot.crop(old_img, Size(5, 5))
        self.assertEqual(new_image, _gradient_image(Size(5, 5)))

    @parameterized.expand(
        [
            param(
                "enlarge uniformly", old_size=Size(7, 7), new_size=Size(10, 10)
            ),
            param(
                "shrink uniformly", old_size=Size(10, 10), new_size=Size(7, 7)
            ),
            param(
                "shrink x enlarge y", old_size=Size(10, 7), new_size=Size(7, 10)
            ),
        ]
    )
    def test_resample(self, _: object, old_size: Size, new_size: Size) -> None:
        old_img = _gradient_image(old_size)
        new_image = screenshot.resample(old_img, new_size)

        for y in range(0, new_size.height):
            for x in range(0, new_size.width):
                old_x = int(x * (old_size.width / new_size.width))
                old_y = int(y * (old_size.height / new_size.height))
                old_pixel = old_img.get_pixel(old_x, old_y)
                new_pixel = new_image.get_pixel(x, y)
                self.assertEqual(
                    old_pixel,
                    new_pixel,
                    f"Pixel at position {x},{y} does not match pixel at original position {old_x},{old_y}",
                )


def _gradient_image(
    size: Size,
    origin_value: list[int] | None = None,
    x_gradient: list[int] | None = None,
    y_gradient: list[int] | None = None,
) -> ScreenshotImage:
    """Creates a test image with a color gradient.


    Color gradient ensured that image manipulation is correct by ensuring
    that every pixel in the image is different, eliminating false positives
    where unintended parts of the image are manipulated instead.

    Args:
        size (Size): Size of the image.
        origin_value (list[int], optional): The pixel at 0,0. Defaults to [0,10,20,30].
        x_gradient (list[int], optional): x-axis gradient. Defaults to [1,0,0,0].
        y_gradient (list[int], optional): y-axis gradient. Defaults to [0,1,0,0].

    Returns:
        ScreenshotImage: the test image.
    """
    if origin_value is None:
        origin_value = [0, 10, 20, 30]
    if x_gradient is None:
        x_gradient = [1, 0, 0, 0]
    if y_gradient is None:
        y_gradient = [0, 1, 0, 0]

    assert len(origin_value) == 4
    assert len(y_gradient) == 4
    assert len(origin_value) == 4

    def add_gradient(v1: list[int], v2: list[int]) -> list[int]:
        return [v1[i] + v2[i] for i in range(0, len(v1))]

    output = bytearray()
    row_value = origin_value
    for _ in range(0, size.height):
        cell_value = row_value
        row_value = add_gradient(row_value, y_gradient)
        for _ in range(0, size.width):
            output.extend(cell_value)
            cell_value = add_gradient(cell_value, x_gradient)

    return ScreenshotImage(size=size, data=bytes(output))


if __name__ == "__main__":
    unittest.main()
