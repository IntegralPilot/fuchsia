# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import contextlib
import io
import json
import logging
import os
import tempfile
import typing
import unittest
from pathlib import Path
from unittest import mock

import data_for_test
import main
from async_utils.command import AsyncCommand, CommandOutput
from parameterized import parameterized

_LOGGER = logging.Logger(__file__)


class TestStatusCommand(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp_dir = tempfile.TemporaryDirectory()
        self.fuchsia_dir = Path(self.tmp_dir.name)
        self.out_dir = self.fuchsia_dir / "out" / "x64"

        os.makedirs(self.out_dir)

        self.args_gn = self.out_dir / "args.gn"
        with self.args_gn.open("w") as f:
            f.write(data_for_test.ARGS_GN_BASE)

        self.product_bundles_json = self.out_dir / "product_bundles.json"
        with self.product_bundles_json.open("w") as f:
            f.write(data_for_test.PRODUCT_BUNDLES_JSON)

        self.async_patch = mock.patch(
            "async_utils.command.AsyncCommand.create",
            mock.AsyncMock(side_effect=self.mock_command_result),
        )
        self.async_mock = self.async_patch.start()
        self.addCleanup(self.async_patch.stop)

        self.env_patch = mock.patch.dict(
            "os.environ", {"FUCHSIA_DIR": str(self.fuchsia_dir)}, clear=True
        )
        self.env_mock = self.env_patch.start()
        self.addCleanup(self.env_patch.stop)
        self.build_dir_patch = mock.patch(
            "build_dir.get_build_directory",
            mock.Mock(return_value=self.out_dir),
        )
        self.build_dir_mock = self.build_dir_patch.start()
        self.addCleanup(self.build_dir_patch.stop)

        # Appear to be two matching git hashes.
        self.stdout_git_command = "12" * 20 + "\n" + "12" * 20
        # By default, no overrides
        self.stdout_jiri_overrides = ""
        # Use the pre-computed output for args.gn formatting.
        self.stdout_gn_format = data_for_test.ARGS_GN_JSON

    def mock_command_result(
        self, *args: typing.Any, **kwargs: typing.Any
    ) -> typing.Any:
        return_code: int = 255
        stdout: str = ""

        if args[0] == "git":
            self.assertEqual(
                args[1:],
                (
                    "--no-optional-locks",
                    f"--git-dir={str(self.fuchsia_dir)}/.git",
                    "rev-parse",
                    "HEAD",
                    "JIRI_HEAD",
                ),
            )
            stdout = self.stdout_git_command
            return_code = 0
        elif args[0] == "fx" and args[3] == "gn":
            self.assertEqual(
                args[4:],
                (
                    "format",
                    "--dump-tree=json",
                    str(self.out_dir / "args.gn"),
                ),
            )
            stdout = self.stdout_gn_format
            return_code = 0
        elif args[0] == "fx" and args[3:] == ("jiri", "override", "-list"):
            stdout = self.stdout_jiri_overrides
            return_code = 0

        ret = mock.MagicMock(spec=AsyncCommand)
        setattr(
            ret,
            "run_to_completion",
            mock.AsyncMock(
                return_value=CommandOutput(
                    stdout,
                    stderr="",
                    return_code=return_code,
                    runtime=0,
                    wrapper_return_code=None,
                    was_timeout=False,
                )
            ),
        )
        _LOGGER.debug("Called with", args, kwargs)
        _LOGGER.debug("Returning", ret)
        return ret

    @parameterized.expand(
        [
            ("no arguments defaults to text", []),
            ("-f text", ["-f", "text"]),
            ("--format text", ["--format", "text"]),
        ]
    )
    def test_text_output(self, _name: str, flags: list[str]) -> None:
        """Check that text output is as expected"""
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            main.main(flags)

        self.maxDiff = None
        self.assertEqual(
            out.getvalue(),
            EXPECTED_TEXT_OUTPUT.lstrip().replace(
                "<<BUILD_DIR>>", str(self.out_dir)
            ),
            f"Output to copy/paste:\n{out.getvalue()}",
        )

    @parameterized.expand(
        [
            ("no arguments defaults to text", []),
            ("-f text", ["-f", "text"]),
            ("--format text", ["--format", "text"]),
        ]
    )
    def test_text_output_no_pb(self, _name: str, flags: list[str]) -> None:
        """Check that text output is as expected when no PB is set"""
        with self.args_gn.open("w") as f:
            f.write(
                data_for_test.ARGS_GN_BASE.replace(
                    'main_pb_label = "//bundles:main"\n', ""
                )
            )

        # Remove the main_pb_label from the mocked gn format output
        gn_json = json.loads(data_for_test.ARGS_GN_JSON)
        gn_json["child"] = [
            c
            for c in gn_json["child"]
            if c.get("child")[0].get("value") != "main_pb_label"
        ]
        self.stdout_gn_format = json.dumps(gn_json)

        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            main.main(flags)

        self.maxDiff = None
        self.assertEqual(
            out.getvalue(),
            EXPECTED_TEXT_OUTPUT_NO_PB.lstrip().replace(
                "<<BUILD_DIR>>", str(self.out_dir)
            ),
            f"Output to copy/paste:\n{out.getvalue()}",
        )

    @parameterized.expand(
        [
            ("-f json", ["-f", "json"]),
            ("--format json", ["--format", "json"]),
        ]
    )
    def test_json_output(self, _name: str, flags: list[str]) -> None:
        """Check that json output is as expected"""
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            main.main(["-f", "json"])

        formatted_output = json.dumps(json.loads(out.getvalue()), indent=2)

        self.maxDiff = None
        self.assertEqual(
            formatted_output,
            EXPECTED_JSON_OUTPUT.lstrip().replace(
                "<<BUILD_DIR>>", str(self.out_dir)
            ),
            f"Output to copy/paste:\n{formatted_output}",
        )

    @parameterized.expand(
        [
            ("-f json", ["-f", "json"]),
            ("--format json", ["--format", "json"]),
        ]
    )
    def test_json_output_no_pb(self, _name: str, flags: list[str]) -> None:
        """Check that json output is as expected when no PB is set"""
        with self.args_gn.open("w") as f:
            f.write(
                data_for_test.ARGS_GN_BASE.replace(
                    'main_pb_label = "//bundles:main"\n', ""
                )
            )

        # Remove the main_pb_label from the mocked gn format output
        gn_json = json.loads(data_for_test.ARGS_GN_JSON)
        gn_json["child"] = [
            c
            for c in gn_json["child"]
            if c.get("child")[0].get("value") != "main_pb_label"
        ]
        self.stdout_gn_format = json.dumps(gn_json)

        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            main.main(flags)

        formatted_output = json.dumps(json.loads(out.getvalue()), indent=2)

        self.maxDiff = None
        self.assertEqual(
            formatted_output,
            EXPECTED_JSON_OUTPUT_NO_PB.lstrip().replace(
                "<<BUILD_DIR>>", str(self.out_dir)
            ),
            f"Output to copy/paste:\n{formatted_output}",
        )

    @parameterized.expand(
        [
            ("-f json", ["-f", "json"]),
            ("--format json", ["--format", "json"]),
        ]
    )
    def test_extended_json_output(self, _name: str, flags: list[str]) -> None:
        """Check that json output is as expected for longer include paths and
        non-alphanumeric names"""
        out = io.StringIO()
        self.stdout_gn_format = self.stdout_gn_format.replace(
            "//boards/x64.gni", "//foo/bar/boards/x64-fb.gni"
        )
        with contextlib.redirect_stdout(out):
            main.main(["-f", "json"])

        formatted_output = json.dumps(json.loads(out.getvalue()), indent=2)

        self.maxDiff = None
        # Longer path
        new_expected_json_output = EXPECTED_JSON_OUTPUT.replace(
            "//boards/x64.gni", "//foo/bar/boards/x64.gni"
        )
        # Extended characters in name
        new_expected_json_output = new_expected_json_output.replace(
            "x64", "x64-fb"
        )
        self.assertEqual(
            formatted_output,
            new_expected_json_output.lstrip().replace(
                "<<BUILD_DIR>>", str(self.out_dir)
            ),
            f"Output to copy/paste:\n{formatted_output}",
        )

    def assertSetContains(self, input: set[str], *args: str) -> None:
        self.assertEqual(
            input,
            input | {*args},
            "\nSet was:\n  " + "\n  ".join(map(lambda x: f"'{x}'", input)),
        )

    def test_no_git_output(self) -> None:
        """When git returns no output, assume we're at JIRI_HEAD"""
        self.stdout_git_command = ""
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            main.main([])
        lines = set(out.getvalue().splitlines())
        self.assertSetContains(
            lines,
            "  Is fuchsia source project in JIRI_HEAD?: true",
        )

    def test_git_mismatch(self) -> None:
        """Git reports we are not at JIRI_HEAD"""
        self.stdout_git_command = "abcd\nefgh"
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            main.main([])
        lines = set(out.getvalue().splitlines())
        self.assertSetContains(
            lines,
            "  Is fuchsia source project in JIRI_HEAD?: false",
        )

    def test_jiri_override(self) -> None:
        """Jiri reports we have overrides"""
        self.stdout_jiri_overrides = "override: foo"
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            main.main([])
        lines = set(out.getvalue().splitlines())
        self.assertSetContains(
            lines,
            "  Has Jiri overrides?: true (output of 'jiri override -list')",
        )

    def test_args_gn_empty(self) -> None:
        """When args.gn is empty, no Build info is available"""
        self.stdout_gn_format = ""
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            main.main([])
        lines = set(out.getvalue().splitlines())
        self.assertEqual(lines, lines - {"Build Info:"})

    def test_device_name_from_environment(self) -> None:
        """Retrieve device name from environment variables"""

        self.env_mock["FUCHSIA_NODENAME"] = "foo"
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            main.main([])
        lines = set(out.getvalue().splitlines())
        self.assertSetContains(set(lines), "  Device name: foo (set by fx -t)")

    def test_device_name_from__build(self) -> None:
        """Retrieve device name from build file"""

        # Note that this still obtains the nodename from environment,
        # but we use an additional environment variable to determine
        # the source of the data.

        self.env_mock["FUCHSIA_NODENAME"] = "foo"
        self.env_mock["FUCHSIA_NODENAME_IS_FROM_FILE"] = "true"
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            main.main([])
        lines = set(out.getvalue().splitlines())
        self.assertSetContains(
            set(lines), "  Device name: foo (set by `fx set-device`)"
        )


EXPECTED_TEXT_OUTPUT = """
Environment Info:
  Current build directory: <<BUILD_DIR>>
Source Info:
  Is fuchsia source project in JIRI_HEAD?: true
  Has Jiri overrides?: false (output of 'jiri override -list')
Build Info:
  Board: x64 (//boards/x64.gni)
  Product: core (//products/core.gni)
  Product bundle: main (set with `fx set-main-pb`)
  Universe packages: [//scripts:tests, //tools/devshell/python:tests] (--with argument of `fx set`)
  Developer tests: [//src/other:tests] (--with-test argument of `fx set`)
  Compilation mode: debug
"""

EXPECTED_TEXT_OUTPUT_NO_PB = """
Environment Info:
  Current build directory: <<BUILD_DIR>>
Source Info:
  Is fuchsia source project in JIRI_HEAD?: true
  Has Jiri overrides?: false (output of 'jiri override -list')
Build Info:
  Board: x64 (//boards/x64.gni)
  Product: core (//products/core.gni)
  Universe packages: [//scripts:tests, //tools/devshell/python:tests] (--with argument of `fx set`)
  Developer tests: [//src/other:tests] (--with-test argument of `fx set`)
  Compilation mode: debug
"""


EXPECTED_JSON_OUTPUT = """
{
  "environmentInfo": {
    "name": "Environment Info",
    "items": {
      "build_dir": {
        "title": "Current build directory",
        "value": "<<BUILD_DIR>>",
        "notes": null
      }
    }
  },
  "sourceInfo": {
    "name": "Source Info",
    "items": {
      "is_in_jiri_head": {
        "title": "Is fuchsia source project in JIRI_HEAD?",
        "value": true,
        "notes": null
      },
      "has_jiri_overrides": {
        "title": "Has Jiri overrides?",
        "value": false,
        "notes": "output of 'jiri override -list'"
      }
    }
  },
  "buildInfo": {
    "name": "Build Info",
    "items": {
      "boards": {
        "title": "Board",
        "value": "x64",
        "notes": "//boards/x64.gni"
      },
      "products": {
        "title": "Product",
        "value": "core",
        "notes": "//products/core.gni"
      },
      "main_pb_label": {
        "title": "Product bundle",
        "value": "main",
        "notes": "set with `fx set-main-pb`"
      },
      "universe_package_labels": {
        "title": "Universe packages",
        "value": [
          "//scripts:tests",
          "//tools/devshell/python:tests"
        ],
        "notes": "--with argument of `fx set`"
      },
      "developer_test_labels": {
        "title": "Developer tests",
        "value": [
          "//src/other:tests"
        ],
        "notes": "--with-test argument of `fx set`"
      },
      "compilation_mode": {
        "title": "Compilation mode",
        "value": "debug",
        "notes": null
      }
    }
  }
}"""

EXPECTED_JSON_OUTPUT_NO_PB = """
{
  "environmentInfo": {
    "name": "Environment Info",
    "items": {
      "build_dir": {
        "title": "Current build directory",
        "value": "<<BUILD_DIR>>",
        "notes": null
      }
    }
  },
  "sourceInfo": {
    "name": "Source Info",
    "items": {
      "is_in_jiri_head": {
        "title": "Is fuchsia source project in JIRI_HEAD?",
        "value": true,
        "notes": null
      },
      "has_jiri_overrides": {
        "title": "Has Jiri overrides?",
        "value": false,
        "notes": "output of 'jiri override -list'"
      }
    }
  },
  "buildInfo": {
    "name": "Build Info",
    "items": {
      "boards": {
        "title": "Board",
        "value": "x64",
        "notes": "//boards/x64.gni"
      },
      "products": {
        "title": "Product",
        "value": "core",
        "notes": "//products/core.gni"
      },
      "universe_package_labels": {
        "title": "Universe packages",
        "value": [
          "//scripts:tests",
          "//tools/devshell/python:tests"
        ],
        "notes": "--with argument of `fx set`"
      },
      "developer_test_labels": {
        "title": "Developer tests",
        "value": [
          "//src/other:tests"
        ],
        "notes": "--with-test argument of `fx set`"
      },
      "compilation_mode": {
        "title": "Compilation mode",
        "value": "debug",
        "notes": null
      }
    }
  }
}"""
