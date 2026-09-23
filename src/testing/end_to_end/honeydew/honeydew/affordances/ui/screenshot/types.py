# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Abstract base class for Screenshot affordance."""

from __future__ import annotations

import enum
from dataclasses import dataclass
from importlib import resources as impresources

import png
from honeydew.affordances.ui.user_input import types as ui_types

_BYTES_PER_PIXEL: int = 4


class ScreenshotImageType(enum.StrEnum):
    """Supported screenshot image types"""

    PNG = "png"
    BGRA = "bgra"

    @staticmethod
    def from_file_path(path: str) -> ScreenshotImageType:
        """Extract ScreenshotImageType from a path's extension.

        Args:
            path (str): File path.

        Raises:
            ValueError: If the extension isn't supported.

        Returns:
            ScreenshotImageType: The matching type.
        """
        path_suffix = path.split(".")[-1].lower()
        for t in ScreenshotImageType:
            if path_suffix == t:
                return t
        raise ValueError(
            f"Only {[t for t in ScreenshotImageType]} files are supported but got '{path}'"
        )


@dataclass(frozen=True)
class ScreenshotImage:
    """Image from screenshot and the size of image

    The format is 32bit RGBA pixels in sRGB color space.
    """

    # Size of the image
    size: ui_types.Size
    # pixel data - 4 bytes per pixel
    data: bytes

    def __post_init__(self) -> None:
        """Validates image size and data length.

        Raises:
            ValueError: For invalid size and data mismatches.
        """
        if self.size.width < 0 or self.size.height < 0:
            raise ValueError(f"Invalid image size {self.size}")
        if len(self.data) % _BYTES_PER_PIXEL:
            raise ValueError(
                f"Data length must be a multiple of {_BYTES_PER_PIXEL}, got {len(self.data)}"
            )
        expected_data_len = (
            self.size.width * self.size.height * _BYTES_PER_PIXEL
        )
        actual_data_len = len(self.data)
        if expected_data_len != actual_data_len:
            raise ValueError(
                f"Expected data length {expected_data_len} = {self.size}*{_BYTES_PER_PIXEL} but got {actual_data_len}"
            )

    @staticmethod
    def load_from_path(path: str) -> ScreenshotImage:
        """Reads screenshot image from a given path.

        Args:
            path (str): Path to load the image from.

        Raises:
            ValueError: Only .bgra files are supported now per b/318880340.

        Returns:
            ScreenshotImage: The loaded image.
        """
        file_type = ScreenshotImageType.from_file_path(path)
        with open(path, "rb") as f:
            return ScreenshotImage._from_data(f.read(), file_type)

    @staticmethod
    def load_from_resource(
        package_name: impresources.Package | str, file_name: str
    ) -> ScreenshotImage:
        """Reads screenshot image from a python packaged resource.

        This is useful for packaging screenshot golden files within a test:
        1. Create a python package a follows:
        ```
        my_test.py
        resources/
        ┣━__init__.py
        ┣━golden1.bgra
        ┣━golden2.bgra
        ┗━golden2.bgra
        ```
        2. Package the resources sources into a python library name `resources`.
        3. `import resources` in my_tests.py.
        4. Call `ScreenshotImage.load_from_resource(resources, "golden1.bgra")`

        See a real world example in this screenshot_image_test.py.

        Args:
            package_name (impresources.Package): The python package literal the
                resource resides in.
            file_name (str): The filename of the resource.

        Raises:
            ValueError: Only .bgra files are supported now per b/318880340.

        Returns:
            ScreenshotImage: An image read from the resources.
        """
        file_type = ScreenshotImageType.from_file_path(file_name)
        resource_file = impresources.open_binary(package_name, file_name)
        return ScreenshotImage._from_data(resource_file.read(), file_type)

    @staticmethod
    def _from_data(
        data: bytes, file_type: ScreenshotImageType
    ) -> ScreenshotImage:
        match file_type:
            case ScreenshotImageType.BGRA:
                # Since `ffx target screenshot` bgra files don't contain size
                # information assume a single row of pixels.
                width = len(data) // _BYTES_PER_PIXEL
                height = 1
                data = ScreenshotImage._swap_rgba_and_bgra(data)
                return ScreenshotImage(ui_types.Size(width, height), data)
            case ScreenshotImageType.PNG:
                (width, height, rows, _) = png.Reader(bytes=data).asRGBA8()
                # Flatten 2d array in a 1d array:
                image_data = bytearray()
                for row in rows:
                    image_data.extend(row)
                return ScreenshotImage(
                    ui_types.Size(width, height), bytes(image_data)
                )

    @staticmethod
    def _swap_rgba_and_bgra(pixel_data: bytes) -> bytes:
        """Converts pixel_data from rgba to bgra, or vice versa by swapping the r and b channels."""
        swapped = bytearray(pixel_data)
        for i in range(0, len(swapped), 4):
            c = swapped[i]
            swapped[i] = swapped[i + 2]
            swapped[i + 2] = c
        return bytes(swapped)

    def save(self, path: str) -> None:
        """Saves the image to the given path.

        Raises:
            ValueError: Only .bgra and .png file output is currently supported.

        Args:
            path (str): Destination for file storage.
        """

        file_type = ScreenshotImageType.from_file_path(path)

        match file_type:
            case ScreenshotImageType.BGRA:
                with open(path, "wb") as f:
                    f.write(ScreenshotImage._swap_rgba_and_bgra(self.data))
            case ScreenshotImageType.PNG:
                # png lib needs the png data in 2d array
                png_rows = []
                for y in range(0, self.size.height):
                    row_offset = self._data_offset(0, y)
                    row_end_offset = (
                        row_offset + self.size.width * _BYTES_PER_PIXEL
                    )
                    png_row = self.data[row_offset:row_end_offset]
                    png_rows.append(png_row)
                png.from_array(png_rows, "RGBA").save(path)

    def _data_offset(self, x: int, y: int) -> int:
        """Returns the data offset for a given coordinate.

        Raises:
            ValueError: If the pixels are outside of the image.

        Returns:
            int: The data offset for a given coordinate.
        """
        if x < 0 or y < 0 or x >= self.size.width or y >= self.size.height:
            raise ValueError(
                f"Pixel coordinates {x},{y} are outside of image {self.size}"
            )
        return (x + y * self.size.width) * _BYTES_PER_PIXEL

    def get_pixel(self, x: int, y: int) -> ui_types.Pixel:
        """Returns the pixel at the given x,y

        Raises:
            ValueError: If the pixels are outside of the image.

        Returns:
            Pixel: The pixel at the given x,y positions.
        """
        return self._get_pixel_at_offset(self._data_offset(x, y))

    def _get_pixel_at_offset(self, offset: int) -> ui_types.Pixel:
        # Parse rgba bytes:
        return ui_types.Pixel(
            red=self.data[offset + 0],
            green=self.data[offset + 1],
            blue=self.data[offset + 2],
            alpha=self.data[offset + 3],
        )

    def _pixel_count(self) -> int:
        return self.size.width * self.size.height

    def pixel_similarity(self, other: ScreenshotImage) -> float:
        """Returns the ratio of pixels that are equal in this and another images.

        For example, image 1:
        ┏━━━━━━┳━━━━━━┓
        ┃ BLUE ┃  RED ┃
        ┣━━━━━━╋━━━━━━┫
        ┃YELLOW┃ BLUE ┃
        ┗━━━━━━┻━━━━━━┛
        And image 2:
        ┏━━━━━━┳━━━━━━┓
        ┃GREEN ┃  RED ┃
        ┣━━━━━━╋━━━━━━┫
        ┃BLUE  ┃YELLOW┃
        ┗━━━━━━┻━━━━━━┛
        Will have a 0.25 similarity (the common top-right red pixel).

        Args:
            other: Another image.
        Raises:
            ValueError: If the images are of a different size.
        Returns:
             If they are identical, the similarity is 1. If, for example,
             %50 of pixels are the same, the similarity is 0.5.
        """
        if self.size != other.size:
            raise ValueError(
                f"Cannot compare similarity for images of different sizes: {self.size} and {other.size}"
            )
        count = 0
        for y in range(0, self.size.height):
            for x in range(0, self.size.width):
                p1 = self.get_pixel(x, y)
                p2 = other.get_pixel(x, y)
                if p1 == p2:
                    count += 1
        return float(count) / self._pixel_count()

    def histogram(self) -> dict[ui_types.Pixel, int]:
        """Returns color histogram for the image.

        For example, the histogram for this image:
        ┏━━━━━━┳━━━━━━┓
        ┃ BLUE ┃  RED ┃
        ┣━━━━━━╋━━━━━━┫
        ┃YELLOW┃ BLUE ┃
        ┗━━━━━━┻━━━━━━┛
        Is:
        {
          BLUE: 2,
          RED: 1,
          YELLOW: 1
        }

        Returns:
            A dictionary with a key:value for every pixel color
            and its frequency in the image.
        """
        hist: dict[ui_types.Pixel, int] = {}
        for y in range(0, self.size.height):
            for x in range(0, self.size.width):
                p = self.get_pixel(x, y)
                hist[p] = hist.get(p, 0) + 1
        return hist

    def histogram_similarity(self, other: ScreenshotImage) -> float:
        """Returns the fraction of pixel colors that are the same in this and another image.

        This similarity measurement is useful for comparing screenshot subject to rotation,
        movement and other effects, by matching pixels distributions while ignoring pixels positions.

        The similarity is computed by computing the histogram of each image, and calculating
        the fraction of the area of intersection between the histograms. I.e. the number of
        pixel colors that are in both images.

        For example, image 1:
        ┏━━━━━━┳━━━━━━┓
        ┃ BLUE ┃  RED ┃
        ┣━━━━━━╋━━━━━━┫
        ┃YELLOW┃ BLUE ┃
        ┗━━━━━━┻━━━━━━┛
        and image 2:
        ┏━━━━━━┳━━━━━━┓
        ┃GREEN ┃ BLUE ┃
        ┣━━━━━━╋━━━━━━┫
        ┃GREEN ┃  RED ┃
        ┗━━━━━━┻━━━━━━┛
        will have the histogram:
        { BLUE: 2, RED: 1, YELLOW: 1} and { GREEN:2, BLUE:1, RED:1 } respectively,
        and therefore a 0.5 similarity (1 blue and 1 red common pixels).

        Args:
            other: Another image.
        Raises:
            ValueError: If the images are of a different size.
        """
        if self.size != other.size:
            raise ValueError(
                f"Cannot compare similarity for images of different sizes: {self.size} and {other.size}"
            )
        h1 = self.histogram()
        h2 = other.histogram()
        common_pixels = 0
        for pixel, count in h1.items():
            if pixel in h2:
                common_pixels += min(count, h2[pixel])
        return float(common_pixels) / self._pixel_count()
