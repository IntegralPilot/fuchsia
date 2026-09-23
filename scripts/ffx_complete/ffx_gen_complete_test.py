# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
import argparse
import json
import logging
import os
import subprocess
import sys
import unittest
from pathlib import Path

import ffx_gen_complete

logger = logging.getLogger(__name__)

parser = argparse.ArgumentParser()
parser.add_argument("--ffx_path", help="Path to the ffx binary.", required=True)

ARGS, sys.argv = parser.parse_known_args(sys.argv)


def test_outdir() -> Path:
    return Path(os.environ["FUCHSIA_TEST_OUTDIR"]).resolve()


class CompletionTest(unittest.TestCase):
    rc_script_path: Path

    @classmethod
    def setUpClass(cls) -> None:
        print(f"ARGS.ffx_path={ARGS.ffx_path}")
        ffx_proc = subprocess.run(
            [ARGS.ffx_path, "--machine", "json", "--help"],
            capture_output=True,
            text=True,
            check=True,
        )

        cls.rc_script_path = test_outdir() / "completion_script.sh"
        with open(cls.rc_script_path, mode="wt") as rc_script:
            root_command = json.loads(ffx_proc.stdout)
            command = ffx_gen_complete.Command.from_dict(root_command)  # type: ignore[attr-defined]
            ffx_gen_complete.print_script(command, file=rc_script)

    @classmethod
    def complete(cls, text: str) -> str:
        words = text.split(" ")
        script = [
            f"source {cls.rc_script_path}",
            f"COMP_CWORD={len(words)-1}",
            f"COMP_WORDS=({' '.join(words)})",
            "_ffx_completions",
            'echo -n "${COMPREPLY[*]}"',
        ]
        cmd: list[str] = [
            "/usr/bin/bash",
            "--noprofile",
            "--norc",
            "-c",
            " ; ".join(script),
        ]
        return subprocess.check_output(cmd, env={}).decode()

    def test_root_flags(self) -> None:
        self.assertEqual("--help", self.complete("ffx --hel"))

    def test_short_flags(self) -> None:
        self.assertIn(" -t ", self.complete("ffx -"))

    def test_known_flags_value(self) -> None:
        self.assertEqual(
            "json-pretty",
            self.complete("ffx --machine json-prett"),
        )

    def test_subcommand(self) -> None:
        self.assertIn("target", self.complete("ffx targe").split())
        self.assertEqual(
            "list",
            self.complete("ffx target lis"),
        )

    def test_command_and_flag_mixture(self) -> None:
        items = self.complete("ffx target ").split()
        self.assertIn("list", items)
        self.assertIn("--help", items)
