#!/usr/bin/env fuchsia-vendored-python
# Copyright 2018 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import sys

sys.path.append(
    os.path.join(
        os.path.dirname(__file__),
        os.pardir,
        "python/modules/elf",
    )
)
import elfinfo


def get_sdk_debug_path(binary: str) -> str:
    elf_info = elfinfo.get_elf_info(binary)
    if not elf_info:
        raise RuntimeError(f"Unable to extract ELF info from {binary}")
    build_id = elf_info.build_id
    return ".build-id/" + build_id[:2] + "/" + build_id[2:] + ".debug"


# For testing.
def main() -> None:
    print(get_sdk_debug_path(sys.argv[1]))


if __name__ == "__main__":
    sys.exit(main())
