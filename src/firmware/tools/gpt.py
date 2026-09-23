#!/usr/bin/env fuchsia-vendored-python
#
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Script to create Fuchsia GPT images.

This script wraps the `sgdisk` utility to provide a simpler interface tailored
specifically for Fuchsia GPT images.

When creating a new GPT, the layout is specified by a JSON file that looks like:

{
    "size_mib": disk size in MiB
    "block_size": disk R/W block size in bytes
    "alignment_kib": partition start alignment in KiB; should generally be a
                     multiple of the disk erase block size
    "partitions": [
        {
            "name": partition name
            "size_kib": size in KiB, or "fill" to use the rest of the disk
            "unique_guid": unique GUID; omit to assign randomly
            "type_guid": type GUID, or one of ("fvm", "vbmeta", "zircon") to use
                         the standard Fuchsia types
        }
    ]
}

Each partition starts immediately following the previous; this tool does not
support creating GPTs with out-of-order partitions or gaps between partitions.
"""

import argparse
import contextlib
import dataclasses
import json
import logging
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import textwrap
from typing import Dict, Iterator, List, Optional

# A regexp to capture GUIDs as reported by `sgdisk`.
# Example: 4B7A23A5-E9D1-456A-BACB-78B23D855324.
GUID_RE = r"[0-9A-F]{8}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{12}"

# Maps partition type names to known type GUIDs.
# https://cs.opensource.google/fuchsia/fuchsia/+/main:zircon/system/public/zircon/hw/gpt.h.
_TYPE_GUID_FROM_NAME = {
    "fvm": "49FD7CB8-DF15-4E73-B9D9-992070127F0F",
    "vbmeta": "421A8BFC-85D9-4D85-ACDA-B64EEC0133E9",
    "zircon": "9B37FFF6-2E58-466A-983A-F7926D0B04E0",
}


@dataclasses.dataclass
class Partition:
    """GPT partition info."""

    # 1-based index in the GPT entry array.
    index: int
    name: str

    # Partition first block, last block (inclusive), and size in blocks.
    # Logical block addressing (LBA) is used here because it's what GPT uses
    # internally and is indivisible so will always be integers for exact
    # calculations (unlike KiB/MiB).
    first_lba: int
    last_lba: int
    size_lba: int

    type_guid: str
    unique_guid: str
    attr_flags: str


@contextlib.contextmanager
def gen_disk_image(source_gpt: str, image_size_mib: int) -> Iterator[str]:
    """Generates empty disk image using the given source GPT."""
    print("Generating disk image")

    with tempfile.TemporaryDirectory() as temp_dir:
        image_path = os.path.join(temp_dir, "disk_image")

        with open(image_path, "wb") as file:
            file.truncate(image_size_mib * 1024 * 1024)

        subprocess.run(
            ["sgdisk", f"--load-backup={source_gpt}", image_path],
            capture_output=True,
            check=True,
        )
        yield image_path
        print("Deleting disk image")


# This function is a bit brittle as it depends heavily on `sgdisk` output format
# which is not guaranteed to be stable. If it becomes unreliable we may either
# need to bring a fixed version of sgdisk into //prebuilts, or just implement
# GPT handling ourselves.
def get_partitions(disk_file: str) -> List[Partition]:
    """Returns a list of all Partitions."""
    # Find all the partition indices first.
    # Output looks like:
    # -----------------------------------------------------------------
    # Disk test_disk.img: 131072 sectors, 64.0 MiB
    # Sector size (logical): 512 bytes
    # Disk identifier (GUID): F1D74D6E-1DB4-4E46-8D2A-87AC93543ACA
    # Partition table holds up to 128 entries
    # Main partition table begins at sector 2 and ends at sector 33
    # First usable sector is 34, last usable sector is 131038
    # Partitions will be aligned on 2048-sector boundaries
    # Total free space is 126909 sectors (62.0 MiB)
    #
    # Number  Start (sector)    End (sector)  Size       Code  Name
    #    1            2048            4095   1024.0 KiB  8300
    #    2            4096            6143   1024.0 KiB  8300
    # -----------------------------------------------------------------
    # Here we just care about the index, which will be the first entry on lines
    # that start with a number.
    output = subprocess.run(
        ["sgdisk", disk_file, "--print"],
        text=True,
        capture_output=True,
        check=True,
    ).stdout
    indices = re.findall(r"^\s*(\d+)\s+", output, re.MULTILINE)

    partitions = []
    for index in indices:
        # Now fill in the full partition information. Output looks like:
        # -----------------------------------------------------------------
        # Partition GUID code: 0FC63DAF-8483-4772-8E79-3D69D8477DE4 (Linux filesystem)
        # Partition unique GUID: 4B7A23A5-E9D1-456A-BACB-78B23D855324
        # First sector: 2048 (at 1024.0 KiB)
        # Last sector: 4095 (at 2.0 MiB)
        # Partition size: 2048 sectors (1024.0 KiB)
        # Attribute flags: 0000000000000000
        # Partition name: 'foo'
        # -----------------------------------------------------------------
        # We could potentially query all partitions in a single command, but
        # the extra delay is negligible and this is simpler.
        output = subprocess.run(
            ["sgdisk", disk_file, "--info", index],
            text=True,
            capture_output=True,
            check=True,
        ).stdout

        def get_param(pattern):
            return re.search(f"^{pattern}.*?$", output, re.MULTILINE).group(1)

        partitions.append(
            Partition(
                index=int(index),
                name=get_param(r"Partition name: '(.*?)'"),
                first_lba=int(get_param(r"First sector: (\d+)")),
                last_lba=int(get_param(r"Last sector: (\d+)")),
                size_lba=int(get_param(r"Partition size: (\d+) sectors")),
                type_guid=get_param(f"Partition GUID code: ({GUID_RE})"),
                unique_guid=get_param(f"Partition unique GUID: ({GUID_RE})"),
                attr_flags=get_param(r"Attribute flags: (\S*)"),
            )
        )

    return partitions


def get_partition(
    disk_file: str,
    *,
    part_name: Optional[str] = None,
    index: Optional[int] = None,
) -> Partition:
    """Returns a single Partition keyed by name and/or index.

    Raises ValueError if the requested partition couldn't be found.
    """
    if part_name is None and index is None:
        raise ValueError("Must specify either a name or index")

    def filter(part):
        return (part_name is None or part.name == part_name) and (
            index is None or part.index == index
        )

    partitions = get_partitions(disk_file)
    try:
        return next(p for p in partitions if filter(p))
    except StopIteration:
        raise ValueError(f"Failed to find partition {part_name}")


def get_part_num(disk_file: str, part_name: str) -> int:
    """Returns the numeric index of a given partition.

    Raises ValueError if the given partition name doesn't exist.
    """
    return get_partition(disk_file, part_name=part_name).index


def mib_to_lba(mib: int, blk_size: int) -> int:
    """Convert size in MB to lba"""
    return int(mib * 1024 * 1024 / blk_size)


def save_gpt(
    disk_path: pathlib.Path,
    block_size: int,
    *,
    mbr_path: Optional[pathlib.Path] = None,
    primary_path: Optional[pathlib.Path] = None,
    backup_path: Optional[pathlib.Path] = None,
):
    """Extracts and saves the GPT sections from a given disk image.

    Args:
        disk_path: path to the disk image.
        block_size: block size in bytes.
        mbr_path: the path to write the MBR binary, or None.
        primary_path: the path to write the primary GPT binary, or None.
        backup_path: the path to write the backup GPT binary, or None.
    """
    # The MBR is always the first block.
    mbr_size = block_size

    # GPT header is always one block, followed by 16KiB of space for partition
    # entries. Technically it can be more than 16KiB but none of our layouts
    # need this so we don't worry about it.
    gpt_size = block_size + (16 * 1024)

    with open(disk_path, "rb") as disk_file:
        mbr = disk_file.read(mbr_size)
        primary = disk_file.read(gpt_size)
        disk_file.seek(-gpt_size, 2)
        backup = disk_file.read()

    for path, contents in (
        (mbr_path, mbr),
        (primary_path, primary),
        (backup_path, backup),
    ):
        if path:
            path.write_bytes(contents)


def delete_part(disk_file: str, part_num: int):
    """Delete a partition from the GPT"""
    subprocess.run(
        ["sgdisk", "--delete", f"{part_num}", disk_file],
        capture_output=True,
        check=True,
    )


def delete_part_by_name(disk_file: str, name: str):
    """Delete a partition by name"""
    part_num = get_part_num(disk_file, name)
    delete_part(disk_file, part_num)


def new_part(disk_file: str, part_num: int, part: Partition):
    """Create a new partition"""
    # does not set attr_flags - this is fine for now as current Fuchsia devices
    # don't depend on it.
    subprocess.run(
        [
            "sgdisk",
            "-a",
            "1",
            "--new",
            f"{part_num}:{part.first_lba}:{part.last_lba}",
            disk_file,
        ],
        capture_output=True,
        check=True,
    )

    subprocess.run(
        ["sgdisk", "--change-name", f"{part_num}:{part.name}", disk_file],
        capture_output=True,
        check=True,
    )
    subprocess.run(
        ["sgdisk", "--typecode", f"{part_num}:{part.type_guid}", disk_file],
        capture_output=True,
        check=True,
    )

    # If we don't have a specific UUID to use, "R" creates a random one.
    guid = part.unique_guid or "R"
    subprocess.run(
        ["sgdisk", "--partition-guid", f"{part_num}:{guid}", disk_file],
        capture_output=True,
        check=True,
    )


def gpt_rename_part(
    disk_image: str,
    old_part_name: str,
    new_part_name: str,
    new_part_type_guid: str = None,
):
    """Rename a partition in the GPT"""
    orig_part = get_partition(disk_image, part_name=old_part_name)
    new_partition = Partition(
        index=orig_part.index,
        name=new_part_name,
        first_lba=orig_part.first_lba,
        last_lba=orig_part.last_lba,
        size_lba=orig_part.size_lba,
        type_guid=new_part_type_guid or orig_part.type_guid,
        unique_guid="",  # generate new
        attr_flags=orig_part.attr_flags,
    )

    delete_part(disk_image, orig_part.index)
    new_part(disk_image, orig_part.index, new_partition)


def create_gpt(spec: Dict, out_dir: pathlib.Path):
    """Creates a GPT from the given specification.

    See the file docstring for a description of the spec.

    Args:
        spec_path: the layout specification.
        out_dir: directory to write resulting images.

    Raises:
        ValueError if the given spec is invalid.
    """
    disk_size = spec["size_mib"] * 1024 * 1024
    alignment = spec["alignment_kib"] * 1024
    block_size = spec["block_size"]

    # TODO: sgdisk appears to hardcode a 512-byte block size when working with
    # raw image files and I can't find any option to modify this behavior. For
    # now all our disks also have 512-byte blocks so it's fine, just punt on
    # this until we need it.
    if block_size != 512:
        raise NotImplementedError(
            "Only 512-byte blocks are currently implemented"
        )

    if disk_size % alignment != 0:
        raise ValueError("Disk size must be a multiple of alignment")
    if alignment % block_size != 0:
        raise ValueError("Disk alignment must be a multiple of block size")

    with tempfile.TemporaryDirectory() as temp_dir:
        temp_dir = pathlib.Path(temp_dir)
        disk_path = temp_dir / "disk.img"

        with open(disk_path, "wb") as disk_file:
            disk_file.truncate(disk_size)

        # We generate a single big `sgdisk` command so that it only has to read
        # the file once, do all the operations in memory, and then write the
        # final result once.

        # Initialize the disk.
        command = ["sgdisk", disk_path, "--clear"]

        # Partition alignment is in block units.
        command += ["--set-alignment", str(alignment // block_size)]

        # Add the partitions.
        for i, part in enumerate(spec["partitions"], start=1):
            name = part["name"]

            size_kib = part["size_kib"]
            if size_kib == "fill":
                # "fill" means use the rest of the partition, which sgdisk does
                # if you pass "0" as the end sector.
                end = 0
            else:
                # Otherwise, use "+" notation to tell sgdisk to set the end
                # relative to the partition start (i.e. partition size rather
                # than end position).
                end = f"+{size_kib}K"

            # If a unique GUID isn't provided, "R" tells sgdisk to generate one
            # randomly.
            unique_guid = part.get("unique_guid", "R")

            # Fetch the type GUID from the name map if it exists.
            type_guid = part["type_guid"]
            if type_guid in _TYPE_GUID_FROM_NAME:
                type_guid = _TYPE_GUID_FROM_NAME[type_guid]

            command += ["--new", f"{i}:0:{end}"]
            command += ["--change-name", f"{i}:{name}"]
            command += ["--typecode", f"{i}:{type_guid}"]
            command += ["--partition-guid", f"{i}:{unique_guid}"]

        # Run the command to write the GPT.
        subprocess.run(command, check=True, capture_output=True, text=True)

        # Extract the pieces to individual files.
        out_dir.mkdir(exist_ok=True)
        save_gpt(
            disk_path,
            block_size,
            mbr_path=out_dir / "mbr.bin",
            primary_path=out_dir / "primary.bin",
            backup_path=out_dir / "backup.bin",
        )

        # Grab a high level summary and the final partition details for the
        # README file.
        summary = subprocess.run(
            ["sgdisk", disk_path, "--print"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()

        partitions = get_partitions(disk_path)

    # Write a README to document this GPT so it's easy for a reader to see
    # where it came from and what it looks like.
    (out_dir / "README.md").write_text(
        textwrap.dedent(
            """\
            # {name} GPT binaries

            Generated by
            https://cs.opensource.google/fuchsia/fuchsia/+/main:src/firmware/tools/gpt.py.

            ## Summary

            ```
            {summary}
            ```

            ## Details

            ```
            {details}
            ```

            ## Input specification

            ```
            {spec}
            ```

            ## Command

            ```
            {command}
            ```

            """
        ).format(
            name=out_dir.name,
            summary=summary,
            details=json.dumps(
                [dataclasses.asdict(p) for p in partitions], indent=2
            ),
            spec=json.dumps(spec, indent=2),
            command=" ".join(str(c) for c in command),
        )
    )


def parse_args() -> argparse.Namespace:
    """Parses commandline args."""
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )

    subparsers = parser.add_subparsers(
        title="Subcommands",
        dest="action",
        help="Action to perform",
        required=True,
    )

    create_parser = subparsers.add_parser(
        "create", help="create GPT images from a given specification"
    )
    create_parser.add_argument(
        "spec",
        type=pathlib.Path,
        help="layout specification JSON file; see --help for details",
    )
    create_parser.add_argument(
        "out_dir",
        type=pathlib.Path,
        help="directory to write the resulting files to, with names:"
        " [mbr.bin, primary.bin, backup.bin, README.md]",
    )

    return parser.parse_args()


def main():
    """Main function entry point. Raises an exception on failure."""
    logging.basicConfig(level=logging.INFO)

    args = parse_args()

    if args.action == "create":
        create_gpt(json.loads(args.spec.read_text()), args.out_dir)

    else:
        raise ValueError(f"Unknown action {args.action}")


if __name__ == "__main__":
    try:
        main()
        sys.exit(0)
    except KeyboardInterrupt:
        # Catch this to avoid printing exception context in this case.
        sys.exit(1)
    except subprocess.CalledProcessError as error:
        # If we failed with a subprocess that captured stdout/stderr,
        # dump it out to the user so they can see what went wrong.
        logging.error(
            "Command failed: `%s`", " ".join(str(c) for c in error.cmd)
        )
        if error.stdout or error.stderr:
            logging.error(
                "\n" "----\n" "%s\n" "----",
                (error.stdout or "") + (error.stderr or ""),
            )
        raise
