# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import pathlib


class GetBuildDirectoryError(Exception):
    pass


def get_build_directory() -> pathlib.Path:
    """Get the current Fuchsia build directory.

    This function uses the value set by the `fx` wrapper that is passed to
    the calling script.

    This function will raise an error if the appropriate environment variable
    is not set, which likely indicates that it is being executed outside of fx.

    Returns:
        Path: Path to the build directory

    Raises:
        GetBuildDirectoryError: If the build directory could not
            be determined, or if this function was called by a script
            that was not executed by `fx`.
    """

    path = os.getenv("FUCHSIA_BUILD_DIR_FROM_FX")
    if not path:
        raise GetBuildDirectoryError(
            "Could not determine Fuchsia build directory. Make sure that this script is being executed by `fx` and not directly."
        )

    return pathlib.Path(path)
