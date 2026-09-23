# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Screenshot utilities - standardize screenshot,crop,resample."""


import logging

from honeydew.affordances.ui.screenshot.types import ScreenshotImage
from honeydew.affordances.ui.user_input.types import Size

_LOGGER: logging.Logger = logging.getLogger(__name__)

_BYTES_PER_PIXEL = 4
_STANDARD_SCREENSHOT_SIZE = Size(480, 480)


def standardize_screenshot(
    image: ScreenshotImage,
    standard_size: Size = _STANDARD_SCREENSHOT_SIZE,
) -> ScreenshotImage:
    """Crops/resized/resamples a screenshot to a standard size.

    To facilitate testing across devices with various monitor types, we
    canonicalize screenshots into a standard square size:
    1. bgra images have no size information - we need to guess it.
    2. if the image is not square, crop it. Watch screens are square.
    3. if the image is too large or small, resample the image.

    Args:
        image: Input image.
        standard_size: The preferred image size.
    """
    assert (
        standard_size.width == standard_size.height
    ), "standard_size must be square"

    # Make the screenshot square
    if image.size.width > image.size.height:
        image = crop(image, Size(image.size.height, image.size.height))
    elif image.size.height > image.size.width:
        image = crop(image, Size(image.size.width, image.size.width))

    # At this point, we should be square
    assert is_square(image)

    if image.size != standard_size:
        image = resample(image, standard_size)

    return image


def is_square(image: ScreenshotImage) -> bool:
    return image.size.width == image.size.height


def change_dimensions(
    image: ScreenshotImage, new_size: Size
) -> ScreenshotImage:
    assert new_size.width * new_size.height * _BYTES_PER_PIXEL == len(
        image.data
    )
    _LOGGER.debug("change_dimensions: %s->%s", image.size, new_size)
    return ScreenshotImage(size=new_size, data=image.data)


def crop(image: ScreenshotImage, new_size: Size) -> ScreenshotImage:
    """Crops the image to a specified new size"""
    _LOGGER.debug("crop: %s->%s", image.size, new_size)
    assert new_size.width <= image.size.width
    assert new_size.height <= image.size.height
    new_data = bytearray()
    old_row_data_len = image.size.width * _BYTES_PER_PIXEL
    new_row_data_len = new_size.width * _BYTES_PER_PIXEL
    for y in range(0, new_size.height):
        row_data_offset = y * old_row_data_len
        new_data.extend(
            image.data[row_data_offset : row_data_offset + new_row_data_len]
        )
    return ScreenshotImage(size=new_size, data=bytes(new_data))


def crop_percent_of_height(
    percent: float, screenshot: ScreenshotImage
) -> ScreenshotImage:
    """Returns the top [percent] percentage of the image for comparison. Meant to be used for
    cases where the bottom icons vary in tests, so we only compare the top n%. For example,
    to get the top half of the image, ratio is 0.5
    """
    new_size = Size(
        screenshot.size.width, int(screenshot.size.height * percent)
    )
    _LOGGER.info("Using top %f percent screenshot", percent * 100)
    return crop(screenshot, new_size)


def resample(image: ScreenshotImage, new_size: Size) -> ScreenshotImage:
    """Resamples an image to a new size"""
    _LOGGER.debug("resample: %s->%s", image.size, new_size)
    dx: float = image.size.width / new_size.width
    dy: float = image.size.height / new_size.height

    new_data = bytearray()
    for y in range(0, new_size.height):
        old_y = int(y * dy)
        for x in range(0, new_size.width):
            old_x = int(x * dx)
            data_offset = (old_y * image.size.width + old_x) * _BYTES_PER_PIXEL
            new_data.extend(
                image.data[data_offset : data_offset + _BYTES_PER_PIXEL]
            )
    return ScreenshotImage(size=new_size, data=bytes(new_data))
