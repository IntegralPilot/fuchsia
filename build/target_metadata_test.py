#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import collections
import dataclasses
import json
import os
import sys
import typing
import unittest

import serialization

"""Test for target metadata.

As a special case, running this test will also print statistics computed from
the content of the target metadata file. It will also dump a freeform JSON file
to FUCHSIA_TEST_OUTDIR if the latter is defined in the environment. See
//testing/testrunner/README.md for more details.

The `--target-metadata <PATH>` argument is required. Other command-line
arguments will be passed to unittest.main().
"""

metadata_path: str | None = None


@dataclasses.dataclass
class TargetStatsFreeformInfo:
    target_count: int = 0
    dep_counts: dict[int, int] = dataclasses.field(default_factory=dict)
    unique_input_file_count: int = 0
    unique_source_file_count: int = 0


class TargetMetadataTest(unittest.TestCase):
    def test_metadata_schema(self) -> None:
        """
        the target metadata file exists and is valid JSON that matches the schema
        """
        self.assertIsNotNone(metadata_path)
        assert metadata_path is not None
        self.assertTrue(os.path.exists(metadata_path))

        data: dict[str, typing.Any]
        with open(metadata_path, "r", encoding="utf-8") as f:
            data = json.load(f)
        self.assertIsInstance(data, dict, "Expected a top-level dict")
        self.assertEqual(
            data.get("version"),
            1,
            "Expected version to be 1. If you changed the version, make sure to update this test.",
        )
        targets: dict[str, typing.Any] | None = data.get("targets")
        self.assertIsNotNone(targets, "Expected targets to be present")
        assert targets is not None
        self.assertIsInstance(targets, dict, "Expected targets to be a dict")

        output = TargetStatsFreeformInfo()

        output.target_count = len(targets)

        output.dep_counts = collections.Counter(
            len(target.get("deps", [])) for target in targets.values()
        )

        unique_input_files: set[str] = set()
        unique_sources: set[str] = set()
        for target in targets.values():
            unique_input_files.update(target.get("inputs", []))
            unique_sources.update(target.get("sources", []))

        output.unique_input_file_count = len(unique_input_files)
        output.unique_source_file_count = len(unique_sources)

        print("")
        print("Target Stats:")
        print("  Total Targets: %d" % (output.target_count))
        print(
            "  Max deps: %d"
            % (max(output.dep_counts) if output.dep_counts else 0)
        )
        total_deps = sum(k * v for k, v in output.dep_counts.items())
        print(
            "  Mean deps: %.2f"
            % (
                total_deps / output.target_count
                if output.target_count > 0
                else 0
            )
        )
        print("  Unique Input Files: %d" % (output.unique_input_file_count))
        print("  Unique Source Files: %d" % (output.unique_source_file_count))
        print("")

        out_dir = os.environ.get("FUCHSIA_TEST_OUTDIR")
        if out_dir is not None:
            path = os.path.join(out_dir, "target_stats.freeform.json")
            print(f"Writing target stats to {path}\n")
            with open(path, "w") as f:
                json.dump(serialization.instance_to_dict(output), f, indent=2)


def main() -> None:
    global metadata_path
    try:
        target_metadata_index = sys.argv.index("--target-metadata")
    except ValueError:
        target_metadata_index = -1
    assert target_metadata_index >= 0 and target_metadata_index + 1 < len(
        sys.argv
    ), "--target-metadata is required with an argument for this test."
    metadata_path = sys.argv[target_metadata_index + 1]
    sys.argv.pop(target_metadata_index)
    sys.argv.pop(target_metadata_index)
    unittest.main()


if __name__ == "__main__":
    main()
