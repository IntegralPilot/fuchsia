# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Generate a filtered version of the Fuchsia Build API module files. See https://fxbug.dev/359845634"""

import json
import typing as T


class BuildApiFilter(object):
    """Convenience class used to filter build API module files."""

    def __init__(
        self, ninja_artifacts: T.Sequence[str], ninja_sources: T.Sequence[str]
    ):
        """Initialization. |ninja_artifacts| is a list of Ninja file paths."""
        self._ninja_artifacts = set(ninja_artifacts)
        self._ninja_sources = set(ninja_sources)
        self._ninja_all = self._ninja_artifacts | self._ninja_sources

    def _filter_json_scopes(
        self, input_json: T.Any, key_name: str
    ) -> list[str]:
        assert isinstance(input_json, list)
        return [s for s in input_json if s[key_name] in self._ninja_all]

    def _filter_json_path_list(
        self, input_paths: T.Sequence[str]
    ) -> T.Sequence[str]:
        return [p for p in input_paths if p in self._ninja_all]

    def _filter_json_scopes_with_multiple_keys(
        self, input_json: T.Any, key_names: T.Sequence[str]
    ) -> list[str]:
        assert isinstance(input_json, list)
        return [
            s
            for s in input_json
            if any(
                s[key_name] in self._ninja_all
                for key_name in key_names
                if key_name in s
            )
        ]

    def filter_api_json(self, api_module: str, json_content: T.Any) -> T.Any:
        """Filter the content of a given build API module.

        Args:
            api_module: Name of build API module, without the .json suffix.
            json_content: File content, as a JSON value.

        Returns:
            New version of the input content, where all items that are not
            related to the set of ninja inputs passed to the constructor are
            removed.
        """
        if api_module in (
            "assembly_input_archives",
            "images",
            "package-repositories",
            "platform_artifacts",
            "sdk_archives",
            "tool_paths",
        ):
            return self._filter_json_scopes(json_content, "path")

        if api_module == "product_bundles":
            # Do not filter on "path" which is a directory that does not appear
            # directly as a Ninja output. Use "json" field which points to the
            # product bundle's JSON manifest file instead.See https://fxbug.dev/365039385.
            return self._filter_json_scopes(json_content, "json")

        if api_module == "assembly_manifests":
            return self._filter_json_scopes(
                json_content, "assembly_manifest_path"
            )

        if api_module == "build_events_log":
            return self._filter_json_scopes(json_content, "build_events_log")

        if api_module == "binaries":
            return self._filter_json_scopes(json_content, "debug")

        if api_module == "debug_symbols":
            # LINT.IfChange(bazel_host_tests_debug_symbols_json)
            # bazel_host_tests.debug_symbols.json is generated out-of-band by
            # regenerator.py (and not built by Ninja). Because it never enters
            # `self._ninja_all`, `_filter_json_scopes_with_multiple_keys` will
            # silently drop its metadata pointer. We explicitly exempt it here.
            res = [
                s
                for s in json_content
                if s.get("manifest") == "bazel_host_tests.debug_symbols.json"
            ] + self._filter_json_scopes_with_multiple_keys(
                json_content, ("debug", "manifest")
            )
            # LINT.ThenChange(//build/bazel/scripts/bazel_tests_utils.py:bazel_host_tests_debug_symbols_json)
            return res

        if api_module == "boards":
            return self._filter_json_scopes(json_content, "outdir")

        if api_module == "clippy_target_mapping":
            return self._filter_json_scopes(json_content, "output")

        if api_module == "generated_sources":
            return self._filter_json_path_list(json_content)

        if api_module == "golden_files":
            return self._filter_json_scopes(json_content, "stamp")

        return json_content

    def filter_api_file(self, api_module: str, content: str) -> str:
        """Filter the content of a given build API module.

        Args:
            api_module: Name of build API module, without the .json suffix.
            content: File content, as a string.

        Returns:
            New version of the input content, pretty-printed, where all items that
            are not related to the set of ninja inputs passed to the constructor
            are removed.
        """
        json_content = self.filter_api_json(api_module, json.loads(content))
        return json.dumps(
            json_content, sort_keys=True, indent=2, separators=(",", ": ")
        )
