# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Library to parse /proc/*/smaps into python dataclasses.


Example usage:

    import pandas
    import smaps

    pandas.DataFrame(smaps.parse("/proc/123/smaps"))

"""
import re
from dataclasses import dataclass
from pathlib import Path
from typing import List, Optional


@dataclass
class SmapEntry:
    addr_start: str
    addr_end: str
    perms: str
    offset: str
    dev: str
    inode: str
    pathname: Optional[str] = None
    # Add smap fields below as needed with the same case as in smaps output.
    Size: Optional[int] = None
    Rss: Optional[int] = None
    Pss: Optional[int] = None


HEADER_RE = re.compile(
    r"(?P<addr_start>\w+)-(?P<addr_end>\w+)"
    r" (?P<perms>[\w-]{4})"
    r" (?P<offset>\w+)"
    r" (?P<dev>\w+:\w+)"
    r" (?P<inode>\d+)"
    r"(\s+(?P<pathname>.*))?"
)
ATTR_RE = re.compile(
    r"(?P<name>\w+):\s*(((?P<value_kb>\d+) kB)|(?P<value>\d+)|(?P<flags>( \w\w)+))"
)


def parse(smaps_path: Path) -> List[SmapEntry]:
    """Reads /proc/*/smaps file into dataclasses."""
    entries = []
    with open(smaps_path, "rt") as f:
        for line_no, line in enumerate(f, start=1):
            line = line.strip()
            if not line:
                continue
            hm = HEADER_RE.match(line)
            if hm:
                entry = SmapEntry(**hm.groupdict())
                entries.append(entry)
                continue
            am = ATTR_RE.match(line)
            if am:
                if am.group("value_kb") is not None:
                    value = int(am.group("value_kb"))
                elif am.group("value") is not None:
                    value = int(am.group("value"))
                else:
                    value = am.group("flags")
                setattr(entry, am.group("name"), value)
                continue
            raise Exception(f"{smaps_path}:{line_no} does not match\n{line!r}")
        return entries
