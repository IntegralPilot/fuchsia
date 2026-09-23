# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import itertools
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Iterator, List, Optional

from dataclasses_json_lite import Undefined, dataclass_json


@dataclass_json(undefined=Undefined.RAISE)
@dataclass
class Blob:
    merkle: str
    path: str
    used_space_in_blobfs: int


@dataclass_json(undefined=Undefined.RAISE)
@dataclass
class Package:
    name: str
    manifest: str
    blobs: List[Blob]


@dataclass_json(undefined=Undefined.RAISE)
@dataclass
class Packages:
    base: List[Package]
    cache: List[Package]


@dataclass_json(undefined=Undefined.RAISE)
@dataclass
class Contents:
    packages: Packages
    maximum_contents_size: int


@dataclass_json(undefined=Undefined.RAISE)
@dataclass
class Image:
    type: str
    name: str
    path: str
    contents: Optional[Contents] = None
    signed: Optional[bool] = None


def all_blobs(images: Iterable[Image]) -> Iterator[Blob]:
    """Iterates on all blobs of the specified image."""
    for image in images:
        contents = image.contents
        if contents:
            for package in itertools.chain(
                contents.packages.base, contents.packages.cache
            ):
                yield from package.blobs


def parse(path: Path) -> List[Image]:
    """Parses `images.json`."""
    with open(path, "rt") as f:
        return [Image.from_dict(image_data) for image_data in json.load(f)]
