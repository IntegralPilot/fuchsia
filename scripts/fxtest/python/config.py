# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
from dataclasses import dataclass
from dataclasses import field
import os
import re

import args


@dataclass
class ConfigFile:
    # The path searched for the config file, if one was searched.
    path: str | None

    # The flags provided by the configuration file, or None if it could not be loaded.
    default_flags: args.Flags | None = None

    # The command line arguments provided in the config file.
    command_line: list[str] = field(default_factory=list)

    def is_loaded(self) -> bool:
        """Determine if a config was loaded.

        Returns:
            bool: True if this config was loaded, False otherwise.
        """
        return self.default_flags is not None


WHITESPACE_REGEX = re.compile(r"\s+")


def load_config(path: str | None = None) -> ConfigFile:
    """Load a set of default flags from a config file.

    Args:
        path (os.PathLike, optional): If set, load config from this
            path instead of searching HOME.

    Returns:
        LoadedConfig: Details on the loaded config, including whether
            it was successfully loaded.
    """

    if not path:
        home_dir = os.getenv("HOME")
        if home_dir is None:
            return ConfigFile(path)
        path = os.path.join(home_dir, ".fxtestrc")

    if not os.path.exists(path):
        return ConfigFile(path)

    lines: list[str]
    with open(path) as f:
        lines = f.readlines()

    command_line: list[str] = []
    line_num = 0
    for line in lines:
        line_num += 1
        line = line.strip()
        if line.startswith("#"):
            # Skip comments
            continue
        command_line.extend(filter(bool, WHITESPACE_REGEX.split(line)))

    try:
        defaults = args.parse_args(command_line)
    except (SystemExit, argparse.ArgumentError) as e:
        print(f"Error occurred while applying flags from config file {path}")
        raise e

    return ConfigFile(
        path=path, default_flags=defaults, command_line=command_line
    )
