# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import tempfile
import unittest
from typing import Any, Dict, List

import images_json
from images_json import Blob, Contents, Image, Package, Packages


def images_json_parse(content: List[Dict[str, Any]]) -> List[Image]:
    with tempfile.NamedTemporaryFile("wt") as tf:
        json.dump(content, tf)
        tf.flush()
        return images_json.parse(tf.name)


class ImagesJsonTest(unittest.TestCase):
    def test_parse_one_without_content(self) -> None:
        self.assertEqual(
            images_json_parse([dict(type="a", name="n", path="p")]),
            [Image(type="a", name="n", path="p")],
        )

    def test_parse_two(self) -> None:
        images = images_json_parse(
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
                                            merkle="merkle1",
                                            path="p1",
                                            used_space_in_blobfs=75,
                                        )
                                    ],
                                )
                            ],
                            cache=[
                                dict(
                                    name="package_b",
                                    manifest="manifest_b",
                                    blobs=[],
                                )
                            ],
                        ),
                    ),
                ),
                dict(
                    type="type_b",
                    name="name_b",
                    path="path_b",
                    contents=dict(
                        maximum_contents_size=64,
                        packages=dict(
                            base=[
                                dict(
                                    name="package_a",
                                    manifest="manifest_a",
                                    blobs=[],
                                )
                            ],
                            cache=[
                                dict(
                                    name="package_b",
                                    manifest="manifest_b",
                                    blobs=[
                                        dict(
                                            merkle="merkle2",
                                            path="p2",
                                            used_space_in_blobfs=79,
                                        )
                                    ],
                                )
                            ],
                        ),
                    ),
                ),
            ]
        )
        self.assertEqual(
            images,
            [
                Image(
                    type="type_a",
                    name="name_a",
                    path="path_a",
                    contents=Contents(
                        packages=Packages(
                            base=[
                                Package(
                                    name="package_a",
                                    manifest="manifest_a",
                                    blobs=[
                                        Blob(
                                            merkle="merkle1",
                                            path="p1",
                                            used_space_in_blobfs=75,
                                        )
                                    ],
                                )
                            ],
                            cache=[
                                Package(
                                    name="package_b",
                                    manifest="manifest_b",
                                    blobs=[],
                                )
                            ],
                        ),
                        maximum_contents_size=64,
                    ),
                    signed=None,
                ),
                Image(
                    type="type_b",
                    name="name_b",
                    path="path_b",
                    contents=Contents(
                        packages=Packages(
                            base=[
                                Package(
                                    name="package_a",
                                    manifest="manifest_a",
                                    blobs=[],
                                )
                            ],
                            cache=[
                                Package(
                                    name="package_b",
                                    manifest="manifest_b",
                                    blobs=[
                                        Blob(
                                            merkle="merkle2",
                                            path="p2",
                                            used_space_in_blobfs=79,
                                        )
                                    ],
                                )
                            ],
                        ),
                        maximum_contents_size=64,
                    ),
                    signed=None,
                ),
            ],
        )

        self.assertEqual(
            list(images_json.all_blobs(images)),
            [
                Blob(merkle="merkle1", path="p1", used_space_in_blobfs=75),
                Blob(merkle="merkle2", path="p2", used_space_in_blobfs=79),
            ],
        )


if __name__ == "__main__":
    unittest.main()
