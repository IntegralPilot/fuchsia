# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import io
import pathlib
import unittest
from unittest import mock

import discover_migration_candidates
from discover_migration_candidates import GnTargetInfo


class TestDiscoverMigrationCandidates(unittest.TestCase):
    def test_complexity_calculator_third_party(self) -> None:
        calc = discover_migration_candidates.ComplexityCalculator(
            pathlib.Path("/root"), [], []
        )

        self.assertTrue(
            calc._is_third_party_target("//third_party/rust_crates:foo")
        )
        self.assertTrue(calc._is_third_party_target("//third_party/golibs:bar"))
        self.assertFalse(calc._is_third_party_target("//src/settings:a"))
        self.assertFalse(calc._is_third_party_target("//third_party/other:b"))
        with self.assertRaises(ValueError):
            calc._is_third_party_target("not/fully/qualified/label")

    def test_complexity_calculator_bazel(self) -> None:
        calc = discover_migration_candidates.ComplexityCalculator(
            pathlib.Path("/root"), [], []
        )

        with mock.patch.object(
            pathlib.Path, "exists"
        ) as mock_exists, mock.patch.object(
            pathlib.Path, "read_text"
        ) as mock_read:
            mock_exists.return_value = True
            mock_read.return_value = 'rust_library(name = "t1")'
            self.assertTrue(calc._is_bazel_target("//root/a:t1"))
            self.assertFalse(calc._is_bazel_target("//root/a:other"))
            with self.assertRaises(ValueError):
                calc._is_bazel_target("not/fully/qualified/label")

            mock_exists.return_value = False
            self.assertFalse(calc._is_bazel_target("//root/a:t1"))

    def test_complexity_for_label(self) -> None:
        root = pathlib.Path("/root")
        calc = discover_migration_candidates.ComplexityCalculator(root, [], [])

        with mock.patch.object(
            discover_migration_candidates.ComplexityCalculator,
            "_is_bazel_target",
        ) as mock_is_bazel:
            mock_is_bazel.return_value = True
            self.assertEqual(calc.complexity_for_label("//src/foo:bar"), 0)

            mock_is_bazel.return_value = False
            self.assertEqual(
                calc.complexity_for_label("//third_party/rust_crates:a"), 0
            )
            self.assertEqual(
                calc.complexity_for_label("//src/foo:bar"),
                discover_migration_candidates._UNKNOWN_DEP_COMPLEXITY,
            )

    def test_complexity_for_label_calculation(self) -> None:
        root = pathlib.Path("/root")
        calc = discover_migration_candidates.ComplexityCalculator(root, [], [])

        target = GnTargetInfo(
            path=root / "BUILD.gn",
            name="t1",
            type="action",
            deps=["//a:a", "//b:b"],
            fields=["sources", "configs"],  # configs is complex (2)
        )
        # Populate cache manually to avoid mocking filesystem access.
        calc._target_cache["//root:t1"] = target
        calc._target_cache["//a:a"] = GnTargetInfo(
            name="a", type="action", path=root / "a/BUILD.gn"
        )
        calc._target_cache["//b:b"] = GnTargetInfo(
            name="b", type="action", path=root / "b/BUILD.gn"
        )

        # 1 + (2 deps * (1 + 1)) + 2 fields (configs=2) -> 1 + 4 + 2 = 7
        with mock.patch.object(
            discover_migration_candidates.ComplexityCalculator,
            "_is_bazel_target",
            return_value=False,
        ):
            self.assertEqual(calc.complexity_for_label("//root:t1"), 7)

        with mock.patch.object(
            discover_migration_candidates.ComplexityCalculator,
            "_is_bazel_target",
            return_value=True,
        ):
            self.assertEqual(calc.complexity_for_label("//root:t1"), 0)

    def test_complexity_for_file(self) -> None:
        root = pathlib.Path("/root")

        content = """
        rustc_binary("t1") {
            deps = ["//foo:bar"]
        }
        rustc_binary("t2") {
            deps = ["//bar:baz"]
        }
        cc_binary("t3") {
            deps = ["//baz:qux"]
        }
        """
        path = root / "BUILD.gn"

        calc = discover_migration_candidates.ComplexityCalculator(root, [], [])

        with mock.patch.object(
            pathlib.Path, "read_text", return_value=content
        ), mock.patch.object(
            pathlib.Path, "exists", return_value=True
        ), mock.patch.object(
            discover_migration_candidates.ComplexityCalculator,
            "complexity_for_label",
        ) as mock_complexity_for_label:
            mock_complexity_for_label.return_value = 1

            result1 = calc.complexity_for_file(path, ["rustc_binary"])
            self.assertEqual(result1.total_complexity, 2)
            self.assertEqual(len(result1.targets), 2)
            self.assertEqual(result1.targets[0].name, "t1")
            self.assertEqual(result1.targets[0].complexity, 1)
            self.assertEqual(result1.targets[1].name, "t2")
            self.assertEqual(result1.targets[1].complexity, 1)

            result2 = calc.complexity_for_file(path, ["cc_binary"])
            self.assertEqual(result2.total_complexity, 1)
            self.assertEqual(len(result2.targets), 1)
            self.assertEqual(result2.targets[0].name, "t3")
            self.assertEqual(result2.targets[0].complexity, 1)

            result3 = calc.complexity_for_file(
                path, ["rustc_binary", "cc_binary"]
            )
            self.assertEqual(result3.total_complexity, 3)
            self.assertEqual(len(result3.targets), 3)
            self.assertEqual(result3.targets[0].name, "t1")
            self.assertEqual(result3.targets[0].complexity, 1)
            self.assertEqual(result3.targets[1].name, "t2")
            self.assertEqual(result3.targets[1].complexity, 1)
            self.assertEqual(result3.targets[2].name, "t3")
            self.assertEqual(result3.targets[2].complexity, 1)

    def test_end_pos_for_single_target(self) -> None:
        content = 'target("name") { deps = [] }'
        # Start after '{'
        start_pos = content.find("{") + 1
        end_pos = discover_migration_candidates.end_pos_for_target(
            content, start_pos
        )
        self.assertEqual(content[end_pos - 1], "}")
        self.assertEqual(end_pos, len(content))

        content = 'target("name") { if (true) { } }'
        start_pos = content.find("{") + 1
        end_pos = discover_migration_candidates.end_pos_for_target(
            content, start_pos
        )
        self.assertEqual(end_pos, len(content))

        content = 'target("name") { invalid'
        start_pos = content.find("{") + 1
        with self.assertRaises(ValueError):
            discover_migration_candidates.end_pos_for_target(content, start_pos)

    def test_end_pos_for_multiple_targets(self) -> None:
        content = """target("name1") {
            deps = []
        }
        target("name2") {
            sources = []
        }"""
        # Find the start of the first target's body.
        start_pos_name1 = len('target("name1") {')
        end_pos_name1 = discover_migration_candidates.end_pos_for_target(
            content, start_pos_name1
        )
        # Check that it correctly identifies the end of the first target.
        self.assertEqual(content[end_pos_name1 - 1], "}")
        # The character after the '}' of the first target should be a newline or whitespace before the next target.
        self.assertEqual(content[end_pos_name1], "\n")

        # Find the start of the second target's body.
        start_pos_name2 = content.find('target("name2") {') + len(
            'target("name2") {'
        )
        end_pos_name2 = discover_migration_candidates.end_pos_for_target(
            content, start_pos_name2
        )
        self.assertEqual(content[end_pos_name2 - 1], "}")
        self.assertEqual(end_pos_name2, len(content))

    def test_deps_from_target_body(self) -> None:
        body = 'deps = [ "//a", "//b" ]'
        deps = discover_migration_candidates.deps_from_target_body(body, {})
        self.assertEqual(deps, ["//a", "//b"])

        body = 'deps += [ "//c" ]'
        deps = discover_migration_candidates.deps_from_target_body(body, {})
        self.assertEqual(deps, ["//c"])

        body = 'deps = [ "//a",\n "//b" ]'
        deps = discover_migration_candidates.deps_from_target_body(body, {})
        self.assertEqual(deps, ["//a", "//b"])

        body = """
            deps = ["//a", "//b"]
            deps += ["//c"]
            inputs = ["//d"]
            deps += ["//e"]
        """
        deps = discover_migration_candidates.deps_from_target_body(body, {})
        self.assertEqual(deps, ["//a", "//b", "//c", "//e"])

        body = "no_deps = []"
        deps = discover_migration_candidates.deps_from_target_body(body, {})
        self.assertEqual(deps, [])

        body = """
            deps = ["//a", "//b"]
            public_deps = ["//public_a"]
            data = ["//data"]
            public_deps += ["//public_b"]
        """
        deps = discover_migration_candidates.deps_from_target_body(body, {})
        self.assertEqual(
            deps, ["//a", "//b", "//public_a", "//public_b", "//data"]
        )

        body = """
            deps = ["//a", "//b"] + common_deps
        """
        context = {"common_deps": ["//c"]}
        deps = discover_migration_candidates.deps_from_target_body(
            body, context
        )
        self.assertEqual(sorted(deps), ["//a", "//b", "//c"])

        body = """
            deps = [
                "//foo",
                "//bar",
            ] + crate_deps + ["//c"]
        """
        context = {"crate_deps": ["//a", "//b"]}
        deps = discover_migration_candidates.deps_from_target_body(
            body, context
        )
        self.assertEqual(
            sorted(deps),
            [
                "//a",
                "//b",
                "//bar",
                "//c",
                "//foo",
            ],
        )

    def test_shared_variables_from(self) -> None:
        context = ""
        shared_variables = discover_migration_candidates.shared_variables_from(
            context
        )
        self.assertEqual(shared_variables, {})

        context = """
        common_deps = [":foo"]
        common_srcs = ["a.cc", "b.cc"]
        empty_list = []
        common_deps += ["//bar"]
        """
        shared_variables = discover_migration_candidates.shared_variables_from(
            context
        )
        self.assertEqual(
            shared_variables,
            {
                "common_deps": [":foo", "//bar"],
                "common_srcs": ["a.cc", "b.cc"],
                "empty_list": [],
            },
        )

    def test_fields_from_target_body(self) -> None:
        body = 'sources = ["a.cc"]\nconfigs += ["//c"]'
        fields = discover_migration_candidates.fields_from_target_body(body)
        self.assertEqual(sorted(fields), ["configs", "sources"])

        body = 'inputs = ["a.cc"]\ninputs += ["b.cc"]'
        fields = discover_migration_candidates.fields_from_target_body(body)
        self.assertEqual(sorted(fields), ["inputs"])

    def test_targets_from_gn_file(self) -> None:
        content = """
        source_set("lib") {
            sources = ["lib.cc"]
        }
        executable("bin") {
            deps = [":lib"]
        }
        """
        with mock.patch.object(
            pathlib.Path, "read_text", return_value=content
        ), mock.patch.object(pathlib.Path, "exists", return_value=True):
            path = pathlib.Path("BUILD.gn")
            targets = discover_migration_candidates.targets_from_gn_file(
                path, ["source_set", "executable"]
            )
            self.assertListEqual(
                targets,
                [
                    GnTargetInfo(
                        name="lib",
                        type="source_set",
                        path=path,
                        deps=[],
                        fields=["sources"],
                    ),
                    GnTargetInfo(
                        name="bin",
                        type="executable",
                        path=path,
                        deps=[":lib"],
                        fields=["deps"],
                    ),
                ],
            )

    def test_complexity_calculator_initialization(self) -> None:
        root = pathlib.Path("/root")
        gn_file = pathlib.Path("src/BUILD.gn")
        content = 'executable("foo") { }'

        with mock.patch.object(
            pathlib.Path, "read_text", return_value=content
        ), mock.patch.object(pathlib.Path, "exists", return_value=True):
            calc = discover_migration_candidates.ComplexityCalculator(
                root, [gn_file], ["executable"]
            )
            expected_label = "//src:foo"
            self.assertIn(expected_label, calc._target_cache)
            self.assertEqual(calc._target_cache[expected_label].name, "foo")
            self.assertEqual(
                calc._target_cache[expected_label].type, "executable"
            )

    def test_to_fully_qualified_label(self) -> None:
        calc = discover_migration_candidates.ComplexityCalculator(
            pathlib.Path("/root"), [], []
        )
        self.assertEqual(
            calc._to_fully_qualified_label(
                pathlib.Path("src"), "//src/foo:bar"
            ),
            "//src/foo:bar",
        )
        self.assertEqual(
            calc._to_fully_qualified_label(pathlib.Path("src"), "foo:bar"),
            "//src/foo:bar",
        )
        self.assertEqual(
            calc._to_fully_qualified_label(pathlib.Path("src"), "bar"),
            "//src:bar",
        )

    def test_resolve_template_subtarget(self) -> None:
        calc = discover_migration_candidates.ComplexityCalculator(
            pathlib.Path("/root"), [], []
        )
        calc._target_cache["//src:my_tool"] = GnTargetInfo(
            path=pathlib.Path("/root/BUILD.gn"),
            name="my_tool",
            type="ffx_tool",
        )

        self.assertEqual(
            calc._resolve_template_subtarget("//src:my_tool_bin"),
            "//src:my_tool",
        )
        self.assertIsNone(
            calc._resolve_template_subtarget("//src:other_tool_bin")
        )
        self.assertIsNone(calc._resolve_template_subtarget("invalid_label"))

    def test_parts_from_fully_qualified_label(self) -> None:
        calc = discover_migration_candidates.ComplexityCalculator(
            pathlib.Path("/root"), [], []
        )
        self.assertEqual(
            calc._parts_from_fully_qualified_label("//src/foo:bar"),
            (pathlib.Path("src/foo"), "bar"),
        )
        with self.assertRaises(ValueError):
            calc._parts_from_fully_qualified_label("invalid")

    def test_to_fully_qualified_label_shorthand(self) -> None:
        calc = discover_migration_candidates.ComplexityCalculator(
            pathlib.Path("/root"), [], []
        )
        self.assertEqual(
            calc._to_fully_qualified_label(
                pathlib.Path("src"), "//src/lib/fdomain/client"
            ),
            "//src/lib/fdomain/client:client",
        )

    @mock.patch("sys.stderr", new_callable=io.StringIO)
    def test_complexity_for_label_cycle(self, mock_stderr: io.StringIO) -> None:
        root = pathlib.Path("/root")
        calc = discover_migration_candidates.ComplexityCalculator(root, [], [])

        calc._target_cache["//root:t1"] = GnTargetInfo(
            path=root / "BUILD.gn", name="t1", type="action", deps=["//root:t2"]
        )
        calc._target_cache["//root:t2"] = GnTargetInfo(
            path=root / "BUILD.gn", name="t2", type="action", deps=["//root:t1"]
        )

        with mock.patch.object(
            discover_migration_candidates.ComplexityCalculator,
            "_is_bazel_target",
            return_value=False,
        ):
            self.assertGreater(calc.complexity_for_label("//root:t1"), 0)
            self.assertIn(
                "WARNING: Dependency cycle detected at //root:t1.",
                mock_stderr.getvalue(),
            )


if __name__ == "__main__":
    unittest.main()
