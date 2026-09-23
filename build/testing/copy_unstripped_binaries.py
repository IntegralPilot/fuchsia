# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import sys
from pathlib import Path
from shutil import rmtree

# Import //build/python/modules/elf/elfinfo.py
sys.path.insert(0, os.path.dirname(__file__) + "/../python/modules/elf")
import elfinfo


def try_link(binary: str, build_id_dir: Path) -> Path:
    info = elfinfo.get_elf_info(binary)
    build_id = info.build_id
    if not build_id or len(build_id) <= 2:
        return
    dest_dir = build_id_dir / build_id[:2]
    dest_dir.mkdir(exist_ok=True)

    # There are two files, the (stripped) binary file, and the unstripped "debug" file. The build_id
    # directory should have both. This will be the stripped binary file.
    dest = dest_dir / build_id[2:]
    if not dest.exists():  # When two source binaries resolves to the same.
        os.link(binary, dest)

    # And this is the unstripped "debug" file.
    dest = dest_dir / (build_id[2:] + ".debug")
    if not dest.exists():  # When two source binaries resolves to the same.
        os.link(binary, dest)

    return dest


def main() -> int:
    assert len(sys.argv) == 6, "Incorrect number of arguments"

    unstripped_binaries_list_file = Path(sys.argv[1])
    build_id_dir = Path(sys.argv[2])
    depfile = Path(sys.argv[3])
    unstripped_libc = sys.argv[4]
    stampfile = Path(sys.argv[5])

    # Always rebuild the build-id directory for garbage collection, and os.link is fast.
    if build_id_dir.exists():
        rmtree(build_id_dir)

    build_id_dir.mkdir(parents=True)

    depfile_content = ""
    with unstripped_binaries_list_file.open() as f:
        for line in f:
            source = line.rstrip("\n")
            if source.startswith("host_"):
                continue
            dest = try_link(source, build_id_dir)
            depfile_content += f"{dest}: {source}\n"

    libc_dest = try_link(unstripped_libc, build_id_dir)
    depfile_content += f"{libc_dest}: {unstripped_libc}"

    depfile.write_text(depfile_content + "\n")
    stampfile.write_text("done!")

    return 0


if __name__ == "__main__":
    sys.exit(main())
