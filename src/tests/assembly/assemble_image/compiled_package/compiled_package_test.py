#!/usr/bin/env fuchsia-vendored-python
# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import os
import sys
import unittest

assembly_outdir = sys.argv.pop()


class CompiledPackageTest(unittest.TestCase):
    """
    Validate the assembly outputs when using a compiled package
    """

    def test_assembly_has_core_package(self) -> None:
        outdir = os.path.join(assembly_outdir, "outdir")
        manifest = json.load(open(os.path.join(outdir, "image_assembly.json")))
        self.assertIn(
            os.path.join(outdir, "core", "package_manifest.json"),
            manifest["base"],
            "The image assembly config should have 'core' in the base set",
        )

    def test_assembly_has_compiled_packages(self) -> None:
        outdir = os.path.join(assembly_outdir, "outdir")
        manifest = json.load(open(os.path.join(outdir, "image_assembly.json")))

        # The package is compiled, as well having assembly-set structured config that causes it to
        # be repackaged.
        self.assertIn(
            os.path.join(
                outdir, "repackaged", "for-test", "package_manifest.json"
            ),
            manifest["base"],
            "The image assembly config should have 'for-test' in the base set",
        )

        # Make sure the components were compiled
        self.assertTrue(
            os.path.exists(os.path.join(outdir, "for-test/bar/bar.cm")),
            "The bar component should have been compiled",
        )
        self.assertTrue(
            os.path.exists(os.path.join(outdir, "for-test/baz/baz.cm")),
            "The baz component should have been compiled",
        )

    def test_assembly_has_bootfs_compiled_packages(self) -> None:
        outdir = os.path.join(assembly_outdir, "outdir")
        manifest = json.load(open(os.path.join(outdir, "image_assembly.json")))

        self.assertNotIn(
            os.path.join(outdir, "for-test2", "package_manifest.json"),
            manifest["base"],
            "The image assembly config should not have for-test2 in the base package list since it should be in bootfs",
        )

        self.assertIn(
            "for-test2",
            [
                os.path.basename(os.path.dirname(file))
                for file in manifest["bootfs_packages"]
            ],
            "The image assembly config should have for-test2 in the bootfs packages",
        )

        # Make sure the components were compiled
        self.assertTrue(
            os.path.exists(os.path.join(outdir, "for-test2/qux/qux.cm")),
            "The qux component should have been compiled",
        )


if __name__ == "__main__":
    unittest.main()
