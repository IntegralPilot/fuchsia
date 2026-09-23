# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import asyncio
import collections
import dataclasses
import json
import os
import shutil
import subprocess
import sys
import tempfile
import typing

import build_dir
import fx_cmd
import statusinfo

HTTP_PORT = 6240


@dataclasses.dataclass
class StatsData:
    categories: dict[str, dict[str, int]] = dataclasses.field(
        default_factory=dict
    )
    tags: dict[str, int] = dataclasses.field(default_factory=dict)


class Options(argparse.Namespace):
    def __init__(self) -> None:
        super().__init__()
        self.web: bool = False
        self.stats: bool = False
        self.paths: list[str] = []
        self.port: int = HTTP_PORT


async def main(arg_override: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description="Manage test categories")
    parser.add_argument(
        "paths",
        nargs="*",
        help="Paths to query or show stats for",
    )
    parser.add_argument(
        "--stats",
        action="store_true",
        help="Output stats instead of details",
    )
    parser.add_argument(
        "--web",
        action="store_true",
        help="Run the web server to visualize test categories",
    )
    parser.add_argument(
        "-p",
        "--port",
        type=int,
        default=HTTP_PORT,
        help=f"Port to run the web server on (default: {HTTP_PORT})",
    )

    args = parser.parse_args(args=arg_override, namespace=Options())

    fuchsia_dir = os.environ.get("FUCHSIA_DIR")
    if not fuchsia_dir:
        print("FUCHSIA_DIR environment variable not set.")
        sys.exit(1)

    out_dir = os.path.join(fuchsia_dir, "out", "test-categories-only")

    # Check if metadata exists in current build directory
    current_build_dir = build_dir.get_build_directory()
    metadata_path = os.path.join(current_build_dir, "testing_metadata.json")

    if os.path.exists(metadata_path):
        important(f"Using existing metadata from {metadata_path}")
    else:
        metadata_path = os.path.join(out_dir, "testing_metadata.json")
        important(f"Metadata not found in build dir. Generating in {out_dir}")
        await ensure_metadata(out_dir)

    metadata = load_metadata_from_path(metadata_path)

    if args.web:
        await run_web_server(fuchsia_dir, metadata_path, args.port)
    elif args.stats:
        show_stats(metadata, args.paths, fuchsia_dir=fuchsia_dir)
    else:
        show_details(metadata, args.paths, fuchsia_dir=fuchsia_dir)


def important(text: str) -> None:
    print(statusinfo.highlight(text))


def green(text: str) -> None:
    print(statusinfo.green_highlight(text))


def dim(text: str) -> None:
    print(statusinfo.dim(text))


async def ensure_metadata(out_dir: str) -> None:
    fx = fx_cmd.FxCmd(build_directory=out_dir)

    set_cmd = [
        "set",
        "--no-change-env",
        "minimal.x64",
        "--with",
        "//tools/testing_metadata",
    ]
    important(f"Running: fx --dir {out_dir} " + " ".join(set_cmd))

    def output_callback(label: str, event: typing.Any) -> None:
        print(
            f"[{label}] {event.text.decode('utf-8', errors='replace')}",
            end="",
            file=sys.stderr,
        )

    await fx.sync(
        *set_cmd,
        stdout_callback=lambda e: output_callback("set", e),
        stderr_callback=lambda e: output_callback("set", e),
    )

    build_cmd = ["build", "//tools/testing_metadata"]
    important(f"Running: fx --dir {out_dir} " + " ".join(build_cmd))
    await fx.sync(
        *build_cmd,
        stdout_callback=lambda e: output_callback("build", e),
        stderr_callback=lambda e: output_callback("build", e),
    )


def load_metadata_from_path(path: str) -> dict[str, typing.Any]:
    if not os.path.exists(path):
        print(f"Error: Metadata file not found at {path}")
        sys.exit(1)
    with open(path, "r") as f:
        return json.load(f)


def show_details(
    data: dict[str, typing.Any], paths: list[str], fuchsia_dir: str
) -> None:
    metadata = data.get("metadata", {})
    paths = [p.rstrip("/") for p in paths]
    if not paths:
        # If no paths listed, show current directory only.
        paths = [os.path.relpath(os.getcwd(), fuchsia_dir)]

    if paths == ["."]:
        paths = [""]

    for path in paths:
        info = metadata.get(path)
        if info:
            print(f"Path: {path}")
            print(json.dumps(info, indent=2))
        else:
            print(f"Path: {path} (No metadata found)")


def calculate_stats(
    data: dict[str, typing.Any], paths: list[str], fuchsia_dir: str
) -> StatsData:
    metadata = data.get("metadata", {})
    paths = [p.rstrip("/") for p in paths]

    # Filter metadata by paths
    filtered_metadata: dict[str, typing.Any] = {}
    if not paths:
        # If no paths listed, show current directory only.
        paths = [os.path.relpath(os.getcwd(), fuchsia_dir)]

    if paths == ["."]:
        filtered_metadata = metadata
    else:
        for p in paths:
            for k, v in metadata.items():
                if k.startswith(p):
                    filtered_metadata[k] = v

    cat_subcat: dict[str, collections.Counter[str]] = collections.defaultdict(
        collections.Counter
    )
    tags: collections.Counter[str] = collections.Counter()

    for k, v in filtered_metadata.items():
        coverage = v.get("coverage", {})
        cat = coverage.get("category")
        subcat = coverage.get("subcategory") or "None"
        tgs = coverage.get("tags", [])

        if cat:
            cat_subcat[cat][subcat] += 1
        for t in tgs:
            tags[t] += 1

    return StatsData(
        categories={cat: dict(subcats) for cat, subcats in cat_subcat.items()},
        tags=dict(tags),
    )


def show_stats(
    data: dict[str, typing.Any], paths: list[str], fuchsia_dir: str
) -> None:
    stats = calculate_stats(data, paths, fuchsia_dir)

    print("Categories and Subcategories:")
    categories = stats.categories
    sorted_cats = sorted(
        categories.items(), key=lambda x: sum(x[1].values()), reverse=True
    )

    for cat, subcats in sorted_cats:
        total = sum(subcats.values())
        print(f"  {cat}: {total}")
        sorted_subcats = sorted(
            subcats.items(), key=lambda x: x[1], reverse=True
        )
        for subcat, count in sorted_subcats:
            print(f"    {subcat}: {count}")

    print("\nTags:")
    tags = stats.tags
    sorted_tags = sorted(tags.items(), key=lambda x: x[1], reverse=True)
    for k, v in sorted_tags:
        print(f"  {k}: {v}")


async def run_web_server(
    fuchsia_dir: str, metadata_path: str, port: int
) -> None:
    tmpdir = tempfile.mkdtemp()
    try:
        shutil.copy(metadata_path, os.path.join(tmpdir, "metadata.json"))
        shutil.copy(
            os.path.join(
                fuchsia_dir, "tools/testing_metadata/visualization/index.html"
            ),
            os.path.join(tmpdir, "index.html"),
        )

        server_process = subprocess.Popen(
            [
                sys.executable,
                "-m",
                "http.server",
                "-b",
                "127.0.0.1",
                str(port),
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            cwd=tmpdir,
        )

        green(f"View categories at http://localhost:{port}?files=metadata.json")
        dim(
            "Not seeing the categories you expect? Run `touch BUILD.gn` and then `fx build`"
        )
        print("")
        print("Press enter to stop the server.")

        await asyncio.to_thread(input)

        server_process.terminate()
        server_process.wait()
    finally:
        shutil.rmtree(tmpdir)
