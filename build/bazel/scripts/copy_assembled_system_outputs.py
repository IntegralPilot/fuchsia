#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Copy assembled system outputs by dynamically parsing assembled_system.json."""

import argparse
import json
import os
import shutil
import sys
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--manifest",
        type=Path,
        required=True,
        help="Path to assembled_system.json",
    )
    parser.add_argument(
        "--bazel-dir",
        type=Path,
        required=True,
        help="Path to Bazel output directory",
    )
    parser.add_argument(
        "--ninja-dir",
        type=Path,
        required=True,
        help="Path to Ninja output directory",
    )
    parser.add_argument(
        "--fxfs",
        help="Copy the Fxfs block image using this destination filename",
    )
    parser.add_argument(
        "--fvm",
        help="Copy the FVM block image using this destination filename",
    )
    parser.add_argument(
        "--vbmeta",
        help="Copy the vbmeta image using this destination filename",
    )
    args = parser.parse_args()

    if not args.manifest.exists():
        raise FileNotFoundError(f"Manifest {args.manifest} not found.")

    with open(args.manifest, "r") as f:
        manifest = json.load(f)

    images = manifest.get("images", [])

    if args.ninja_dir.exists():
        shutil.rmtree(args.ninja_dir)
    args.ninja_dir.mkdir(parents=True, exist_ok=True)

    # Always copy the manifest itself to the root of the Ninja directory
    shutil.copy2(args.manifest, args.ninja_dir / "assembled_system.json")

    # 1. The ZBI is always present, always find and copy it.
    zbi_path = find_image(images, "zbi")
    if not zbi_path:
        raise ValueError("ZBI image not found in manifest.")
    copy_image_file(zbi_path, "fuchsia.zbi", args.bazel_dir, args.ninja_dir)

    # 2. If fxfs is specified, find and copy it.
    if args.fxfs:
        fxfs_path = find_image(images, "blk", "fxfs")
        if not fxfs_path:
            raise ValueError("Fxfs image requested but not found in manifest.")
        copy_image_file(fxfs_path, args.fxfs, args.bazel_dir, args.ninja_dir)

    # 3. If fvm is specified, find and copy it.
    if args.fvm:
        fvm_path = find_image(images, "blk", "fvm")
        if not fvm_path:
            raise ValueError("FVM image requested but not found in manifest.")
        copy_image_file(fvm_path, args.fvm, args.bazel_dir, args.ninja_dir)

    # 4. If vbmeta is specified, find and copy it.
    if args.vbmeta:
        vbmeta_path = find_image(images, "vbmeta")
        if not vbmeta_path:
            raise ValueError(
                "VBMeta image requested but not found in manifest."
            )
        copy_image_file(
            vbmeta_path, args.vbmeta, args.bazel_dir, args.ninja_dir
        )

    return 0


def find_image(
    images: list[dict[str, str]],
    img_type: str,
    name_hint: str | None = None,
) -> Path | None:
    for img in images:
        t = img.get("type")
        path = img.get("path")
        if t == img_type and path is not None:
            if name_hint:
                if name_hint in path:
                    return Path(path)
            else:
                return Path(path)
    return None


def copy_image_file(
    img_path: Path, dest_name: str, bazel_dir: Path, ninja_dir: Path
) -> None:
    src_file: Path = bazel_dir / img_path
    dst_file: Path = ninja_dir / dest_name

    try:
        # Use hardlink for zero-overhead copy
        os.link(src_file, dst_file)
    except Exception as e:
        raise RuntimeError(
            f"Failed to link {src_file} -> {dst_file}: {e}"
        ) from e


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)
