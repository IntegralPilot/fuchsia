#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Tests for GnLabel."""

import unittest
from pathlib import Path

from gn_label import GnLabel


class GnLabelTest(unittest.TestCase):
    def test_from_str(self) -> None:
        label = GnLabel.from_str("//path/to/foo")
        self.assertEqual(label.gn_str, "//path/to/foo")
        self.assertEqual(label.path_str, "path/to/foo")
        self.assertEqual(label.name, "foo")
        self.assertFalse(label.is_local_name)
        self.assertIsNone(label.toolchain)

    def test_from_str_with_toolchain(self) -> None:
        toolchain = GnLabel.from_str("//some/toolchain")
        label = GnLabel.from_str("//path/to/foo(//some/toolchain)")
        self.assertEqual(label.gn_str, "//path/to/foo(//some/toolchain)")
        self.assertEqual(label.path_str, "path/to/foo")
        self.assertEqual(label.name, "foo")
        self.assertEqual(label.toolchain, toolchain)

    def test_from_str_with_local_name(self) -> None:
        label = GnLabel.from_str("//path/to/foo:bar")
        self.assertEqual(label.gn_str, "//path/to/foo:bar")
        self.assertEqual(label.path_str, "path/to/foo")
        self.assertEqual(label.name, "bar")
        self.assertTrue(label.is_local_name)
        self.assertIsNone(label.toolchain)

    def test_from_str_with_redundant_local_name(self) -> None:
        label = GnLabel.from_str("//path/to/foo:foo")
        self.assertEqual(label.gn_str, "//path/to/foo:foo")
        self.assertEqual(label.path_str, "path/to/foo")
        self.assertEqual(label.name, "foo")
        self.assertTrue(label.is_local_name)
        self.assertIsNone(label.toolchain)

    def test_from_str_with_local_name_and_toolchain(self) -> None:
        toolchain = GnLabel.from_str("//some/toolchain")
        label = GnLabel.from_str("//path/to/foo:bar(//some/toolchain)")
        self.assertEqual(label.gn_str, "//path/to/foo:bar(//some/toolchain)")
        self.assertEqual(label.path_str, "path/to/foo")
        self.assertEqual(label.name, "bar")
        self.assertEqual(label.toolchain, toolchain)

    def test_from_str_with_dot_dot(self) -> None:
        toolchain = GnLabel.from_str("//some/toolchain")
        label = GnLabel.from_str("//path/to/foo/../bar:baz(//some/toolchain)")
        self.assertEqual(label.gn_str, "//path/to/bar:baz(//some/toolchain)")
        self.assertEqual(label.path_str, "path/to/bar")
        self.assertEqual(label.name, "baz")
        self.assertEqual(label.toolchain, toolchain)

    def test_from_str_root_path(self) -> None:
        label = GnLabel.from_str("//")
        self.assertEqual(label.gn_str, "//")
        self.assertEqual(label.path_str, "")
        self.assertEqual(label.name, "")
        self.assertIsNone(label.toolchain)

    def test_from_root_path(self) -> None:
        label = GnLabel.from_path(Path(""))
        self.assertEqual(label.gn_str, "//")
        self.assertEqual(label.path_str, "")
        self.assertEqual(label.name, "")
        self.assertIsNone(label.toolchain)

    def test_from_path(self) -> None:
        label = GnLabel.from_path(Path("path/to/foo"))
        self.assertEqual(label.gn_str, "//path/to/foo")
        self.assertEqual(label.path_str, "path/to/foo")
        self.assertEqual(label.name, "foo")
        self.assertIsNone(label.toolchain)

    def test_parent_label(self) -> None:
        label = GnLabel.from_str("//path/to/foo")
        self.assertTrue(label.has_parent_label())
        self.assertEqual(label.parent_label(), GnLabel.from_str("//path/to"))

    def test_parent_label_for_local_name(self) -> None:
        label = GnLabel.from_str("//path/to/foo:name")
        self.assertTrue(label.has_parent_label())
        self.assertEqual(
            label.parent_label(), GnLabel.from_str("//path/to/foo")
        )

    def test_parent_label_for_redundant_local(self) -> None:
        label = GnLabel.from_str("//path/to/foo:foo")
        self.assertTrue(label.is_local_name)
        self.assertTrue(label.has_parent_label())
        self.assertEqual(
            label.parent_label(), GnLabel.from_str("//path/to/foo")
        )

    def test_parent_label_of_root_label(self) -> None:
        label = GnLabel.from_str("//")
        self.assertEqual(label.gn_str, "//")
        self.assertFalse(label.has_parent_label())

        with self.assertRaises(AssertionError) as context:
            label.parent_label()
        self.assertEqual("// has no parent label", str(context.exception))

    def test_parent_label_of_label_created_from_path(self) -> None:
        label = GnLabel.from_path(Path("path/to/foo"))
        self.assertTrue(label.has_parent_label())
        self.assertEqual(label.parent_label(), GnLabel.from_str("//path/to"))

    def test_without_toolchain(self) -> None:
        label = GnLabel.from_str(
            "//path/to/foo(//some:toolchain)"
        ).without_toolchain()
        self.assertEqual(label, GnLabel.from_str("//path/to/foo"))

    def test_ensure_toolchain(self) -> None:
        toolchain = GnLabel.from_str("//some/toolchain")
        label = GnLabel.from_str("//path/to/foo").ensure_toolchain(toolchain)
        self.assertEqual(
            label, GnLabel.from_str("//path/to/foo(//some/toolchain)")
        )

    def test_ensure_toolchain_does_not_replace(self) -> None:
        toolchain = GnLabel.from_str("//some/toolchain")
        label = GnLabel.from_str(
            "//path/to/foo(//some/other/toolchain)"
        ).ensure_toolchain(toolchain)
        self.assertEqual(
            label, GnLabel.from_str("//path/to/foo(//some/other/toolchain)")
        )

    def test_rebase_package_path(self) -> None:
        path = GnLabel.from_str("//path/to/foo").rebased_path(
            Path("rebase/dir")
        )
        self.assertEqual(path, Path("rebase/dir/path/to/foo"))

    def test_code_search_url(self) -> None:
        url = GnLabel.from_str("//path/to/foo:bar").url()
        self.assertEqual(
            url,
            "https://cs.opensource.google/fuchsia/fuchsia/+/main:path/to/foo",
        )

    def test_custom_url(self) -> None:
        input_url = "https://fuchsia.dev/whats-new/release-notes"
        url = GnLabel.from_str("//path/to/foo:bar", url=input_url).url()
        self.assertEqual(
            url,
            input_url,
        )

    def test_is_host_target(self) -> None:
        def is_host_target(s: str) -> bool:
            return GnLabel.from_str(s).is_host_target()

        self.assertFalse(is_host_target("//host_:foo"))
        self.assertFalse(is_host_target("//foo:host_"))
        self.assertTrue(is_host_target("//foo:bar(//host_baz)"))
        self.assertFalse(is_host_target("//foo:bar(//not_host)"))

    def test_is_third_party(self) -> None:
        def is_3p(s: str) -> bool:
            return GnLabel.from_str(s).is_3rd_party()

        self.assertFalse(is_3p("//foo:bar"))
        self.assertTrue(is_3p("//third_party/foo"))
        self.assertTrue(is_3p("//thirdparty/foo"))
        self.assertTrue(is_3p("//foo:third_party"))
        self.assertTrue(is_3p("//foo/third_party/bar"))

    def test_is_prebuilt(self) -> None:
        self.assertFalse(GnLabel.from_str("//foo:bar").is_prebuilt())
        self.assertTrue(GnLabel.from_str("//prebuilt/foo:bar").is_prebuilt())
        self.assertTrue(GnLabel.from_str("//foo/prebuilt/bar").is_prebuilt())
        self.assertTrue(GnLabel.from_str("//foo/bar:prebuilt").is_prebuilt())

    def test_is_3p_rust_crate(self) -> None:
        self.assertFalse(GnLabel.from_str("//foo:bar").is_3p_rust_crate())
        self.assertTrue(
            GnLabel.from_str("//third_party/rust_crates:foo").is_3p_rust_crate()
        )

    def test_is_3p_golib(self) -> None:
        self.assertFalse(GnLabel.from_str("//foo:bar").is_3p_golib())
        self.assertTrue(
            GnLabel.from_str("//third_party/golibs:foo").is_3p_golib()
        )
        self.assertTrue(
            GnLabel.from_str(
                "//third_party/golibs:google.golang.org/api/transport(//build/toolchain:host_x64)"
            ).is_3p_golib()
        )

    def test_is_spdx_json_document(self) -> None:
        self.assertFalse(GnLabel.from_str("//foo:bar").is_spdx_json_document())
        self.assertTrue(
            GnLabel.from_str("//foo:bar.spdx.json").is_spdx_json_document()
        )
        self.assertTrue(
            GnLabel.from_str("//foo.spdx.json").is_spdx_json_document()
        )

    def test_create_child_from_str(self) -> None:
        parent = GnLabel.from_str("//path1/to/foo:bar(//toolchain)")
        child = parent.create_child_from_str("path2/to/child:baz")
        self.assertEqual(
            child, GnLabel.from_str("//path1/to/foo/path2/to/child:baz")
        )

    def test_create_child_from_absolute_str(self) -> None:
        parent = GnLabel.from_str("//path1/to/foo:bar(//toolchain)")
        child = parent.create_child_from_str("//path2/to/child:baz")
        self.assertEqual(child, GnLabel.from_str("//path2/to/child:baz"))

    def test_create_child_from_str_with_dot_dot(self) -> None:
        parent = GnLabel.from_str("//path/to/foo")
        self.assertEqual(parent.parent_label(), GnLabel.from_str("//path/to"))

        self.assertEqual(
            parent.create_child_from_str("../baz:qux"),
            GnLabel.from_str("//path/to/baz:qux"),
        )

        self.assertEqual(
            parent.create_child_from_str("../../baz:qux"),
            GnLabel.from_str("//path/baz:qux"),
        )

        self.assertEqual(
            parent.create_child_from_str("../../../baz:qux"),
            GnLabel.from_str("//baz:qux"),
        )

        # Going back too much is not supported
        with self.assertRaises(AssertionError) as context:
            parent.create_child_from_str("../../../../baz:qux")
        self.assertEqual(
            ".. goes back beyond the base path: path/to/foo/../../../../baz",
            str(context.exception),
        )

        self.assertEqual(
            parent.create_child_from_str("bar/../baz"),
            GnLabel.from_str("//path/to/foo/baz"),
        )

        self.assertEqual(
            parent.create_child_from_str("bar/../../../baz"),
            GnLabel.from_str("//path/baz"),
        )

        # Going back too much is not supported
        with self.assertRaises(AssertionError) as context:
            child = parent.create_child_from_str("bar/../../../../../baz:qux")
        self.assertEqual(
            ".. goes back beyond the base path: path/to/foo/bar/../../../../../baz",
            str(context.exception),
        )

    def test_create_child_from_str_with_dot_dot_with_local_name(self) -> None:
        parent = GnLabel.from_str("//path/to/foo:bar")
        self.assertEqual(
            parent.parent_label(), GnLabel.from_str("//path/to/foo")
        )

        self.assertEqual(
            parent.create_child_from_str("../baz:qux"),
            GnLabel.from_str("//path/to/baz:qux"),
        )

        self.assertEqual(
            parent.create_child_from_str("../../baz:qux"),
            GnLabel.from_str("//path/baz:qux"),
        )

        self.assertEqual(
            parent.create_child_from_str("../../../baz:qux"),
            GnLabel.from_str("//baz:qux"),
        )

    def test_create_child_from_str_with_colon(self) -> None:
        self.assertEqual(
            GnLabel.from_str("//path/to/foo").create_child_from_str(":bar"),
            GnLabel.from_str("//path/to/foo:bar"),
        )

        with self.assertRaises(AssertionError) as context:
            GnLabel.from_str("//path/to/foo:bar").create_child_from_str(":baz")
        self.assertEqual(
            "Can't apply :baz to //path/to/foo:bar because both have :",
            str(context.exception),
        )

    def test_gt(self) -> None:
        # Testing greater_than indirectly by sorting
        sorted_labels = sorted(
            [
                GnLabel.from_str("//path2"),
                GnLabel.from_str("//path3:foo2"),
                GnLabel.from_str("//path3:foo1"),
                GnLabel.from_str("//path1"),
            ]
        )
        self.assertListEqual(
            sorted_labels,
            [
                GnLabel.from_str("//path1"),
                GnLabel.from_str("//path2"),
                GnLabel.from_str("//path3:foo1"),
                GnLabel.from_str("//path3:foo2"),
            ],
        )


if __name__ == "__main__":
    unittest.main()
