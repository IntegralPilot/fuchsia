# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import pathlib
import sys
import unittest
from unittest import mock

# Ensure scripts/lib is importable when executed directly outside packaged environments.
_SCRIPTS_LIB = pathlib.Path(__file__).resolve().parents[2]
if str(_SCRIPTS_LIB) not in sys.path and _SCRIPTS_LIB.is_dir():
    sys.path.insert(0, str(_SCRIPTS_LIB))

import agents.agents as agents_lib

_SOURCE_AGENTS_PY_RELPATH = (
    pathlib.Path("scripts") / "lib" / "agents" / "agents.py"
)


def _find_source_agents_py() -> pathlib.Path | None:
    direct_path = pathlib.Path(agents_lib.__file__).resolve()
    if direct_path.name == "agents.py" and direct_path.is_file():
        return direct_path

    if "FUCHSIA_DIR" in os.environ:
        candidate = (
            pathlib.Path(os.environ["FUCHSIA_DIR"]) / _SOURCE_AGENTS_PY_RELPATH
        ).resolve()
        if candidate.is_file():
            return candidate

    cwd = pathlib.Path.cwd().resolve()
    for parent in [cwd] + list(cwd.parents):
        candidate = (parent / _SOURCE_AGENTS_PY_RELPATH).resolve()
        if candidate.is_file():
            return candidate

    return None


class TestAgents(unittest.TestCase):
    def test_get_agent_env_vars(self) -> None:
        vars_list = agents_lib.get_agent_env_vars()
        self.assertIn("ANTIGRAVITY_AGENT", vars_list)
        self.assertIn("GEMINI_CLI", vars_list)

    def test_source_agents_txt_relative_path(self) -> None:
        """Verify the expected relative path from agents.py to agents.txt exists.

        This test fails if either agents.py or agents.txt moves in the source
        tree without updating the expected relative path.
        """
        source_agents_py = _find_source_agents_py()
        if source_agents_py is None:
            self.skipTest(
                "Fuchsia source tree is not present in isolated test environment"
            )

        self.assertTrue(
            source_agents_py.is_file(),
            f"Expected source file {source_agents_py} to exist",
        )

        target_agents_txt = agents_lib._get_source_agents_txt_path(
            source_agents_py
        )
        self.assertTrue(
            target_agents_txt.is_file(),
            f"Expected agents.txt at {target_agents_txt} relative to {source_agents_py}, but file does not exist",
        )

        with mock.patch.object(agents_lib, "data", None):
            with mock.patch.object(
                agents_lib,
                "_get_source_agents_txt_path",
                return_value=target_agents_txt,
            ):
                vars_list = agents_lib.get_agent_env_vars()
                self.assertIn("ANTIGRAVITY_AGENT", vars_list)
                self.assertIn("GEMINI_CLI", vars_list)

    def test_get_agent_env_vars_missing_raises(self) -> None:
        """Verify get_agent_env_vars raises FileNotFoundError if agents.txt cannot be found."""
        with mock.patch.object(agents_lib, "data", None):
            with mock.patch.object(
                agents_lib,
                "_get_source_agents_txt_path",
                return_value=pathlib.Path("/nonexistent/path/to/agents.txt"),
            ):
                with self.assertRaises(FileNotFoundError):
                    agents_lib.get_agent_env_vars()

    def test_is_invoked_by_agent(self) -> None:
        env: dict[str, str] = {}
        self.assertFalse(agents_lib.is_invoked_by_agent(env))

        env = {"ANTIGRAVITY_AGENT": "1"}
        self.assertTrue(agents_lib.is_invoked_by_agent(env))

        env = {"GEMINI_CLI": "1"}
        self.assertTrue(agents_lib.is_invoked_by_agent(env))


if __name__ == "__main__":
    unittest.main()
