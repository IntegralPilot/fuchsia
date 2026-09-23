#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(__file__))
from runfiles_utils import RepoMapping, RunfilesManifest


class RepoMappingTest(unittest.TestCase):
    # Some input test cases shared by the tests below.
    # These all correspond to the same custom Bazel project
    # built with different versions of Bazel and with or without
    # BzlMod enabled.

    _BAZEL8_BZLMOD_WITH_MODULE_EXTENSION_REPO_MAPPING = "\n".join(
        [
            ",the_module,_main",
            ",the_repo,+custom_repo_ext+the_repo",
            "+custom_repo_ext+the_repo,the_module,_main",
            "+custom_repo_ext+the_repo,the_repo,+custom_repo_ext+the_repo",
        ]
    )

    _BAZEL7_BZLMOD_WITH_MODULE_EXTENSION_REPO_MAPPING = "\n".join(
        [
            ",the_module,_main",
            ",the_repo,_main~custom_repo_ext~the_repo",
            ",the_workspace,_main",
            "_main~custom_repo_ext~the_repo,the_module,_main",
            "_main~custom_repo_ext~the_repo,the_repo,_main~custom_repo_ext~the_repo",
        ]
    )

    _BAZEL7_WORKSPACE_WITH_REPOSITORY_REPO_MAPPING = "\n".join(
        [
            ",the_workspace,the_workspace",
            "bazel_tools,__main__,the_workspace",
            "local_config_cc,the_workspace,the_workspace",
            "local_config_xcode,the_workspace,the_workspace",
            "the_repo,the_workspace,the_workspace",
        ]
    )

    def test_parse_repo_mapping(self) -> None:
        _TEST_CASES = [
            (
                "bazel8_bzlmod_with_module_extension",
                self._BAZEL8_BZLMOD_WITH_MODULE_EXTENSION_REPO_MAPPING,
                {
                    "": {
                        "the_module": "_main",
                        "the_repo": "+custom_repo_ext+the_repo",
                    },
                    "+custom_repo_ext+the_repo": {
                        "the_module": "_main",
                        "the_repo": "+custom_repo_ext+the_repo",
                    },
                },
            ),
            (
                "bazel7_bzlmod_with_module_extension",
                self._BAZEL7_BZLMOD_WITH_MODULE_EXTENSION_REPO_MAPPING,
                {
                    "": {
                        "the_module": "_main",
                        "the_repo": "_main~custom_repo_ext~the_repo",
                        "the_workspace": "_main",
                    },
                    "_main~custom_repo_ext~the_repo": {
                        "the_module": "_main",
                        "the_repo": "_main~custom_repo_ext~the_repo",
                    },
                },
            ),
            (
                "bazel7_workspace_with_custom_repository",
                self._BAZEL7_WORKSPACE_WITH_REPOSITORY_REPO_MAPPING,
                {
                    "": {"the_workspace": "the_workspace"},
                    "bazel_tools": {"__main__": "the_workspace"},
                    "local_config_cc": {"the_workspace": "the_workspace"},
                    "local_config_xcode": {"the_workspace": "the_workspace"},
                    "the_repo": {"the_workspace": "the_workspace"},
                },
            ),
        ]
        for test_case in _TEST_CASES:
            name, content, expected = test_case
            result = RepoMapping.parse_repo_mapping(content)
            self.assertEqual(result, expected, name)

    def test_map_apparent_name(self) -> None:
        _TEST_CASES = [
            (
                "bazel8_bzlmod_with_module_extension",
                self._BAZEL8_BZLMOD_WITH_MODULE_EXTENSION_REPO_MAPPING,
                [
                    # (source_name,apparent_name,runfile_name)
                    ("the_module", "", "_main"),
                    ("the_module", "+custom_repo_ext+the_repo", "_main"),
                    ("the_module", "unknown_source", ""),
                    ("the_repo", "", "+custom_repo_ext+the_repo"),
                    (
                        "the_repo",
                        "+custom_repo_ext+the_repo",
                        "+custom_repo_ext+the_repo",
                    ),
                    ("the_repo", "unknown_source", ""),
                ],
            ),
            (
                "bazel7_bzlmod_with_module_extension",
                self._BAZEL7_BZLMOD_WITH_MODULE_EXTENSION_REPO_MAPPING,
                [
                    # (source_name,apparent_name,runfile_name)
                    ("the_module", "", "_main"),
                    ("the_module", "_main~custom_repo_ext~the_repo", "_main"),
                    ("the_module", "unknown_source", ""),
                    ("the_repo", "", "_main~custom_repo_ext~the_repo"),
                    (
                        "the_repo",
                        "_main~custom_repo_ext~the_repo",
                        "_main~custom_repo_ext~the_repo",
                    ),
                    ("the_repo", "unknown_source", ""),
                ],
            ),
            (
                "bazel7_workspace_with_custom_repository",
                self._BAZEL7_WORKSPACE_WITH_REPOSITORY_REPO_MAPPING,
                [
                    # (source_name,apparent_name,runfile_name)
                    ("the_workspace", "", "the_workspace"),
                    ("the_workspace", "local_config_cc", "the_workspace"),
                    ("the_workspace", "local_config_xcode", "the_workspace"),
                    ("the_workspace", "the_repo", "the_workspace"),
                    ("the_workspace", "unknown_source", ""),
                    ("__main__", "bazel_tools", "the_workspace"),
                ],
            ),
        ]
        for test_case in _TEST_CASES:
            name, content, subcases = test_case
            mapping = RepoMapping.CreateFrom(content)
            for apparent_name, source_name, expected_name in subcases:
                self.assertEqual(
                    expected_name,
                    mapping.map_apparent_name(apparent_name, source_name),
                    f"{name}.{apparent_name} in {source_name}",
                )

    def test_map_input_path(self) -> None:
        _TEST_CASES = [
            (
                "bazel8_bzlmod_with_module_extension",
                self._BAZEL8_BZLMOD_WITH_MODULE_EXTENSION_REPO_MAPPING,
                [
                    # (input_path,source_name,expected_path)
                    ("the_module", "", "the_module"),
                    ("the_module/foo/bar", "", "_main/foo/bar"),
                    (
                        "the_module/foo/bar",
                        "+custom_repo_ext+the_repo",
                        "_main/foo/bar",
                    ),
                    (
                        "the_module/foo/bar",
                        "unknown_source",
                        "the_module/foo/bar",
                    ),
                    (
                        "the_repo/data/file",
                        "",
                        "+custom_repo_ext+the_repo/data/file",
                    ),
                    (
                        "the_repo/data/file",
                        "+custom_repo_ext+the_repo",
                        "+custom_repo_ext+the_repo/data/file",
                    ),
                    (
                        "the_repo/data/file",
                        "unknown_source",
                        "the_repo/data/file",
                    ),
                ],
            ),
            (
                "bazel7_bzlmod_with_module_extension",
                self._BAZEL7_BZLMOD_WITH_MODULE_EXTENSION_REPO_MAPPING,
                [
                    # (input_path,source_name,expected_path)
                    ("the_module/src/foo", "", "_main/src/foo"),
                    (
                        "the_module/src/foo",
                        "_main~custom_repo_ext~the_repo",
                        "_main/src/foo",
                    ),
                    (
                        "the_module/src/foo",
                        "unknown_source",
                        "the_module/src/foo",
                    ),
                    (
                        "the_repo/data/file",
                        "",
                        "_main~custom_repo_ext~the_repo/data/file",
                    ),
                    (
                        "the_repo/data/file",
                        "_main~custom_repo_ext~the_repo",
                        "_main~custom_repo_ext~the_repo/data/file",
                    ),
                    (
                        "the_repo/data/file",
                        "unknown_source",
                        "the_repo/data/file",
                    ),
                ],
            ),
            (
                "bazel7_workspace_with_custom_repository",
                self._BAZEL7_WORKSPACE_WITH_REPOSITORY_REPO_MAPPING,
                [
                    # (input_path,source_name,expected_path)
                    ("the_workspace/src/foo", "", "the_workspace/src/foo"),
                    (
                        "the_workspace/src/foo",
                        "local_config_cc",
                        "the_workspace/src/foo",
                    ),
                    (
                        "the_workspace/src/foo",
                        "local_config_xcode",
                        "the_workspace/src/foo",
                    ),
                    (
                        "the_workspace/src/foo",
                        "the_repo",
                        "the_workspace/src/foo",
                    ),
                    (
                        "the_workspace/src/foo",
                        "unknown_source",
                        "the_workspace/src/foo",
                    ),
                    (
                        "__main__/data/file",
                        "bazel_tools",
                        "the_workspace/data/file",
                    ),
                ],
            ),
        ]
        for test_case in _TEST_CASES:
            name, content, subcases = test_case
            mapping = RepoMapping.CreateFrom(content)
            for input_path, source_name, expected_path in subcases:
                self.assertEqual(
                    expected_path,
                    mapping.map_input_path(input_path, source_name),
                    f"{name}.{input_path} from {source_name}",
                )


class RunfilesManifestTest(unittest.TestCase):
    def test_escape(self) -> None:
        TEST_CASES = {
            "a path with spaces": "a\\spath\\swith\\sspaces",
            "a\npath\nwith\nnewlines": "a\\npath\\nwith\\nnewlines",
            "a\\path\\with\\backslashes": "a\\bpath\\bwith\\bbackslashes",
            " a mix\nof\\many things": "\\sa\\smix\\nof\\bmany\\sthings",
            "a_regular_path": "a_regular_path",
        }
        for path, expected in TEST_CASES.items():
            self.assertEqual(expected, RunfilesManifest.escape(path))

    def test_unescape(self) -> None:
        TEST_CASES = {
            "a\\spath\\swith\\sspaces": "a path with spaces",
            "a\\npath\\nwith\\nnewlines": "a\npath\nwith\nnewlines",
            "a\\bpath\\bwith\\bbackslashes": "a\\path\\with\\backslashes",
            "\\sa\\smix\\nof\\bmany\\sthings": " a mix\nof\\many things",
            "unescapable_\\%sequence": "unescapable_\\%sequence",
            "a_regular_path": "a_regular_path",
        }
        for path, expected in TEST_CASES.items():
            self.assertEqual(expected, RunfilesManifest.unescape(path))

    def test_parse_manifest(self) -> None:
        VALID_TEST_CASES = {
            "foo bar": {"foo": "bar"},
            "foo bar\nqux zoo\n": {"foo": "bar", "qux": "zoo"},
            " foo bar": {"foo": "bar"},
            " foo\\s bar": {"foo ": "bar"},
            " foo bar\\szoo": {"foo": "bar zoo"},
        }
        for content, expected in VALID_TEST_CASES.items():
            result = RunfilesManifest.parse_manifest(content)
            self.assertEqual(expected, result, content)

        ERROR_TEST_CASES = {
            "foo\n": "No space separator in manifest line: [foo]"
        }
        for content, error in ERROR_TEST_CASES.items():
            with self.assertRaises(ValueError) as cm:
                RunfilesManifest.parse_manifest(content)

        self.assertEqual(str(cm.exception), error, content)

    def test_as_dict(self) -> None:
        TEST_CASES = {
            "foo bar": {"foo": "bar"},
            "foo bar\nqux zoo\n": {"foo": "bar", "qux": "zoo"},
            " foo bar": {"foo": "bar"},
            " foo\\s bar": {"foo ": "bar"},
            " foo bar\\szoo": {"foo": "bar zoo"},
        }
        for content, expected_dict in TEST_CASES.items():
            manifest = RunfilesManifest.CreateFrom(content)
            self.assertDictEqual(manifest.as_dict(), expected_dict, msg=content)

    def test_generate_runtime_deps_json(self) -> None:
        pass

    def test_generate_content(self) -> None:
        TEST_CASES = [
            ({"foo": "bar"}, "foo bar\n"),
            ({"foo": "bar", "bar": "zoo"}, "bar zoo\nfoo bar\n"),
            ({"foo bar": "zoo"}, " foo\\sbar zoo\n"),
            ({"foo": "bar zoo"}, " foo bar\\szoo\n"),
            ({"foo\nbar": " zoo\\qux"}, " foo\\nbar \\szoo\\bqux\n"),
        ]
        for file_map, expected_content in TEST_CASES:
            manifest = RunfilesManifest(file_map)
            self.assertEqual(manifest.generate_content(), expected_content)


if __name__ == "__main__":
    unittest.main()
