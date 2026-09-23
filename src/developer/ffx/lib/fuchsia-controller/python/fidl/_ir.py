# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""The library module handles creating Python classes and types based on FIDL IR rules."""

from __future__ import annotations

import os
from typing import Dict, Mapping

from fuchsia_controller_py import Context

FIDL_IR_PATH_ENV: str = "FIDL_IR_PATH"
FIDL_IR_PATH_CONFIG: str = "fidl.ir.path"
LIB_MAP: Dict[str, str] = {}
MAP_INIT = False


def find_fx_root(starting_dir: str | os.PathLike[str]) -> str | None:
    """Returns the Fuchsia checkout root using `.fx-root` if it can be found, else `None`."""
    current_dir = os.path.realpath(starting_dir)
    while True:
        fx_root_path = os.path.join(current_dir, ".fx-root")
        if os.path.isfile(fx_root_path):
            return current_dir
        else:
            next_dir = os.path.join(current_dir, os.pardir)
            next_dir = os.path.realpath(next_dir)
            # Only happens if we're the root directory and we try to go up once
            # more.
            if current_dir == next_dir:
                return None
            current_dir = next_dir


def get_fidl_ir_map() -> Mapping[str, str]:
    """Returns a singleton mapping of library names to FIDL files."""
    global MAP_INIT
    if MAP_INIT:
        return LIB_MAP
    ctx = Context()
    # Relative path once we've found the "root" directory that should contain
    # the FIDL IR.
    ir_root_relpath = os.path.join("fidling", "gen", "ir_root")
    # TODO(b/308723467): Handle multiple paths.
    default_ir_path = ctx.config_get_string(FIDL_IR_PATH_CONFIG)
    if not default_ir_path:
        if FIDL_IR_PATH_ENV in os.environ:
            default_ir_path = os.environ[FIDL_IR_PATH_ENV]
        else:
            # We look for the root dir without using FUCHSIA_DIR env, since
            # there's a possibility the user is running multiple fuchsia
            # checkouts. We just want to check where we're running the command.
            #
            # This is the same approach as for the `//scripts/fx` script.
            if root_dir := find_fx_root(os.curdir):
                with open(os.path.join(root_dir, ".fx-build-dir"), "r") as f:
                    build_dir = f.readlines()[0].strip()
                    default_ir_path = os.path.join(
                        root_dir, build_dir, ir_root_relpath
                    )
            else:
                # TODO(b/311250297): Remove last resort backstop for
                # unconfigured in-tree build config
                default_ir_path = ir_root_relpath
    if not os.path.isdir(default_ir_path):
        raise RuntimeError(
            "FIDL IR path not found via ffx config under"
            + f" '{FIDL_IR_PATH_CONFIG}', and no Fuchsia"
            + " checkout found in any parent of:"
            + f" '{os.path.realpath(os.curdir)}'."
            + f" IR also not found at default path '{default_ir_path}'."
            + f" You may need to set {FIDL_IR_PATH_ENV} in your environment,"
            + " or re-run this script from a different directory."
        )
    for _, dirs, _ in os.walk(default_ir_path):
        for d in dirs:
            LIB_MAP[d] = os.path.join(default_ir_path, d, f"{d}.fidl.json")
    MAP_INIT = True
    return LIB_MAP
