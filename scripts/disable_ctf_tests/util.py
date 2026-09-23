# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Various useful bits and pieces."""

import enum
import json
import os
from dataclasses import dataclass, field
from typing import Any

""" The state-save file is at a fixed location in the source tree. """
_FUCHSIA_DIR = os.getenv("FUCHSIA_DIR")
assert (
    _FUCHSIA_DIR is not None
), "FUCHSIA_DIR is not set. Did you set your Fuchsia environment?"
assert os.path.isdir(_FUCHSIA_DIR), f'"{_FUCHSIA_DIR}" must be a directory'
_JSON_FILE = f"{_FUCHSIA_DIR}/sdk/ctf/disabled_tests.json"


class Status(enum.Enum):
    """Generic status values that can be returned from poll() functions."""

    WORKING = 1  # Haven't yet succeeded or failed
    FAILING = 2  # Detected failure, but haven't finalized
    SUCCESS = 3  # Finalized; worked as expected
    FAILURE = 4  # Finalized unsuccessfully


@dataclass
class Outputs:
    """Transfer human-readable information from business logic to UI.

    - updates are current status - in a GUI, the user probably just needs to
        see the latest.
    - prints are more durable - the user may want to see all of them.
    - errors mean something has gone wrong; the program is probably about to
        quit, and the user needs to know why.
    """

    updates: list[str] = field(default_factory=list)
    prints: list[str] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)

    def update(self, s: str) -> None:
        self.updates.append(s)

    def print(self, p: str) -> None:
        self.prints.append(p)

    def error(self, e: str) -> None:
        self.errors.append(e)


def _vars_or_obj(o: Any) -> Any:
    """Convert object/data into a json.dump'able thing."""
    try:
        return vars(o)
    except:
        return o


def save_program_state(state: Any) -> None:
    """Write state into a JSON-formatted file."""
    with open(_JSON_FILE, "w") as f:
        json.dump(state, f, default=_vars_or_obj, indent=2)
        f.write("\n")


def load_program_state() -> Any:
    """Load the state-save file into JSON data."""
    with open(_JSON_FILE) as f:
        return json.load(f)
