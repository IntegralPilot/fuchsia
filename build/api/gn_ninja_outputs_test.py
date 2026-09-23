#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import sys
import tempfile
import typing as T
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import gn_ninja_outputs

_NINJA_OUTPUTS = {
    "//:foo": ["obj/foo.stamp"],
    "//:bar": [
        "obj/bar.output",
        "obj/bar.stamp",
    ],
    "//:zoo": [
        "obj/zoo.output",
        "obj/zoo.stamp",
    ],
    "//:zoo(//toolchain:secondary)": [
        "secondary/obj/zoo.output",
        "secondary/obj/zoo.stamp",
    ],
}

# Phony outputs should not appear, but they do, the tests
# below verify that they are ignored. This dictionary
# should match the content of _NINJA_OUTPUTS, except for
# additional phony paths, and some GN labels that only
# contain phony paths, and will be filtered out too.
_NINJA_PHONY_OUTPUTS = {
    "//:foo": ["obj/foo.stamp", "phony/objy/foo.validation"],
    "//:foo.validation": ["phony/obj/foo.validation"],
    "//:bar": [
        "obj/bar.output",
        "phony/obj/bar.validation",
        "obj/bar.stamp",
    ],
    "//:zoo": [
        "obj/zoo.output",
        "obj/zoo.stamp",
    ],
    "//:zoo(//toolchain:secondary)": [
        "secondary/obj/zoo.output",
        "secondary/obj/zoo.stamp",
        "secondary/phony/obj/zoo.validation",
    ],
    "//:zoo.validation(//toolchain:secondary)": [
        "secondary/phony/obj/zoo.validation",
    ],
}


class TestNinjaOutputsDatabase(unittest.TestCase):
    def setUp(self) -> None:
        self._td = tempfile.TemporaryDirectory()
        self._root = Path(self._td.name)

    def tearDown(self) -> None:
        self._td.cleanup()

    def run_tests(self, db: gn_ninja_outputs.NinjaOutputsBase) -> None:
        self.assertEqual([], db.gn_label_to_paths("//:unknown"))
        for label, paths in _NINJA_OUTPUTS.items():
            self.assertListEqual(
                paths,
                db.gn_label_to_paths(label),
                f"When querying label {label}",
            )

        self.assertEqual("", db.path_to_gn_label("unknown_path"))
        for label, paths in _NINJA_OUTPUTS.items():
            for path in paths:
                self.assertEqual(
                    label,
                    db.path_to_gn_label(path),
                    f"When querying path {path}",
                )

        self.assertListEqual(sorted(_NINJA_OUTPUTS.keys()), db.get_labels())

        self.assertListEqual(
            ["//:zoo", "//:zoo(//toolchain:secondary)"],
            db.target_name_to_gn_labels("zoo.output"),
        )
        self.assertListEqual(
            ["//:bar"], db.target_name_to_gn_labels("bar.output")
        )
        self.assertListEqual([], db.target_name_to_gn_labels("unknown_target"))

        expected_paths = sorted(
            path for sublist in _NINJA_OUTPUTS.values() for path in sublist
        )
        self.assertListEqual(expected_paths, db.get_paths())

    def run_tests_for_class(
        self,
        db_class: type[gn_ninja_outputs.NinjaOutputsBase],
        outputs_json: T.Any = _NINJA_OUTPUTS,
    ) -> None:
        db = db_class()
        input_path = self._root / "test_outputs.json"
        with input_path.open("wt") as f:
            json.dump(outputs_json, f)

        db.load_from_json(input_path)
        self.run_tests(db)

        database_path = self._root / "test_database"
        db.save_to_file(database_path)

        db = db_class()
        db.load_from_file(database_path)
        self.run_tests(db)

    def test_json_database(self) -> None:
        self.run_tests_for_class(gn_ninja_outputs.NinjaOutputsJSON)

    def test_tabular_database(self) -> None:
        self.run_tests_for_class(gn_ninja_outputs.NinjaOutputsTabular)

    def test_json_database_with_phony_outputs(self) -> None:
        # Verify that phony outputs are removed.
        input_path = self._root / "test_outputs.json"
        with input_path.open("wt") as f:
            json.dump(_NINJA_PHONY_OUTPUTS, f)

        db = gn_ninja_outputs.NinjaOutputsJSON()
        db.load_from_json(input_path)
        self.run_tests(db)

    def test_json_database_with_duplicate_outputs(self) -> None:
        _NINJA_DUPLICATE_OUTPUTS = {
            "//:foo": ["obj/foo.stamp"],
            "//:bar": ["obj/bar.stamp", "obj/foo.stamp"],
            "//:zoo": ["obj/zoo.outputs", "obj/zoo.stamp", "obj/foo.stamp"],
            "//:qux": ["obj/qux.stamp", "obj/bar.stamp"],
        }
        with self.assertRaises(gn_ninja_outputs.DuplicateOutputsError) as cm:
            self.run_tests_for_class(
                gn_ninja_outputs.NinjaOutputsJSON,
                outputs_json=_NINJA_DUPLICATE_OUTPUTS,
            )

        error = cm.exception
        self.assertDictEqual(
            error.duplicate_outputs,
            {
                "obj/bar.stamp": ["//:bar", "//:qux"],
                "obj/foo.stamp": ["//:bar", "//:foo", "//:zoo"],
            },
        )


if __name__ == "__main__":
    unittest.main()
