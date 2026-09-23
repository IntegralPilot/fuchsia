#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""cas

Tools for interacting with CAS.
"""

import dataclasses
import os
import tempfile
from pathlib import Path

import cl_utils
import fuchsia

_SCRIPT_BASENAME = Path(__file__).name
_SCRIPT_DIR = Path(__file__).parent

PROJECT_ROOT = fuchsia.project_root_dir()
PROJECT_ROOT_REL = cl_utils.relpath(PROJECT_ROOT, Path(start=os.curdir))

# default path to `cas` tool
_CAS_TOOL = PROJECT_ROOT_REL / "prebuilt" / "tools" / "cas" / "cas"


def msg(text: str) -> None:
    print(f"[{_SCRIPT_BASENAME}] {text}")


@dataclasses.dataclass
class File(object):
    """Represents a single file archive in the CAS."""

    # CAS instance name (projects/*/instances/*)
    instance: str
    # hash and size of object to retrieve
    digest: str

    # Name of file in the CAS archive (relative to archive root).
    filename: Path

    def download(self, output: Path) -> cl_utils.SubprocessResult:
        """Download a single file from the CAS.

        In this case, user expects that the data at the given digest
        is a single file, as opposed to a directory.

        Args:
          output: path to save downloaded output

        Returns:
          subprocess result and exit code of the download process.
        """
        # download to temp dir first
        with tempfile.TemporaryDirectory() as tempdir:
            cmd = [
                str(_CAS_TOOL),
                "download",
                "--cas-instance",
                self.instance,
                "--digest",
                self.digest,
                "--dir",
                tempdir,
            ]
            result = cl_utils.subprocess_call(cmd)
            if result.returncode != 0:
                msg(
                    f"""Error downloading blob from {self.instance} with digest {self.digest} to {tempdir}.
If you see an authentication or credential error, retry after running: {_CAS_TOOL} login."""
                )
                return result

            # The 'archive' in the CAS has a single file that matches
            # the basename.
            (Path(tempdir) / self.filename).rename(output)

        return result
