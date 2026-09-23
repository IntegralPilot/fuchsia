# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Utility library for checking if a tool is being run by an automated agent."""

import os
import pathlib
import typing
from importlib.resources import files

try:
    from agents import data  # type: ignore[attr-defined]
except ImportError:
    data = None


# Relative path from this file's directory to tools/devshell/lib/agents.txt in the source tree.
_SOURCE_AGENTS_TXT_RELPATH = (
    pathlib.Path("..")
    / ".."
    / ".."
    / "tools"
    / "devshell"
    / "lib"
    / "agents.txt"
)


def _get_source_agents_txt_path(
    from_file: str | pathlib.Path,
) -> pathlib.Path:
    """Resolve the path to tools/devshell/lib/agents.txt relative to the given source file."""
    base_file = pathlib.Path(from_file).resolve()
    return (base_file.parent / _SOURCE_AGENTS_TXT_RELPATH).resolve()


def get_agent_env_vars() -> typing.List[str]:
    """Read the list of agent environment variable names from agents.txt."""
    if data is not None:
        try:
            with files(data).joinpath("agents.txt").open(
                "r", encoding="utf-8"
            ) as f:
                return [
                    line.strip()
                    for line in f
                    if line.strip() and not line.strip().startswith("#")
                ]
        except (FileNotFoundError, TypeError):
            # importlib.resources.files() raises TypeError if `data` is not a
            # package (e.g. when mocked in tests or when imported outside a
            # packaged environment), and open() raises FileNotFoundError if
            # agents.txt is missing from the package resources. In either case,
            # fall through to reading agents.txt directly from the source tree.
            pass

    # Fallback to source tree location if imported without GN packaging.
    agents_txt = _get_source_agents_txt_path(__file__)
    if agents_txt.is_file():
        with agents_txt.open("r", encoding="utf-8") as f:
            return [
                line.strip()
                for line in f
                if line.strip() and not line.strip().startswith("#")
            ]

    raise FileNotFoundError(
        f"Could not locate agents.txt in packaged resources or at {agents_txt}"
    )


def is_invoked_by_agent(env: typing.Mapping[str, str] | None = None) -> bool:
    """Determine if the given environment (or os.environ) indicates execution by an AI agent.

    Args:
        env: Optional mapping representing the environment. Defaults to os.environ.

    Returns:
        bool: True if an agent environment variable is present, False otherwise.
    """
    if env is None:
        env = os.environ
    for var in get_agent_env_vars():
        if env.get(var) is not None:
            return True
    return False
