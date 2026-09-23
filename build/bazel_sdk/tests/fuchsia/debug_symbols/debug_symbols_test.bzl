# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Fuchsia Debug Symbols support."""

load("@bazel_skylib//lib:unittest.bzl", "analysistest", "asserts")
load(
    "@rules_fuchsia//fuchsia/private:fuchsia_debug_symbols.bzl",
    "fuchsia_debug_symbols",
)
load(
    "@rules_fuchsia//fuchsia_rules_common/debug_symbols:debug_symbols.bzl",
    "fuchsia_collect_all_debug_symbols_infos_aspect",
    "transform_collected_debug_symbols_infos",
)
load(
    "@rules_fuchsia//fuchsia_rules_common/debug_symbols:providers.bzl",
    "FuchsiaDebugSymbolInfo",
)
load("//test_utils:make_file.bzl", "make_file")

def _top_level_target_impl(ctx):
    return transform_collected_debug_symbols_infos(
        ctx.attr.deps,
        ctx.attr.dep,
    )

_top_level_target = rule(
    implementation = _top_level_target_impl,
    attrs = {
        "deps": attr.label_list(aspects = [fuchsia_collect_all_debug_symbols_infos_aspect]),
        "dep": attr.label(aspects = [fuchsia_collect_all_debug_symbols_infos_aspect]),
    },
)

def _debug_symbols_collection_test_impl(ctx):
    env = analysistest.begin(ctx)

    target_under_test = analysistest.target_under_test(env)

    asserts.true(
        env,
        FuchsiaDebugSymbolInfo in target_under_test,
        "target does not contain FuchsiaDebugSymbolInfo",
    )

    converted_build_ids = {}
    for key, build_id_dirs in target_under_test[FuchsiaDebugSymbolInfo].build_id_dirs_mapping.items():
        if type(key) == "File":
            source_search_root = key.short_path
        else:
            source_search_root = key

        converted_build_ids[source_search_root] = [f.short_path for f in build_id_dirs.to_list()]

    expected = json.decode(ctx.attr.expected_build_id_dirs)
    asserts.equals(
        env,
        expected,
        converted_build_ids,
        msg = "{} failed to match debug symbol infos:".format(ctx.label.name),
    )

    return analysistest.end(env)

debug_symbols_collection_test = analysistest.make(
    _debug_symbols_collection_test_impl,
    attrs = {
        "expected_build_id_dirs": attr.string(
            doc = """A json string of { string -> [ string ]} mappings.

            The string values should represent the result of calling short_path
            on the File objects in the build_id_dirs depset.
        """,
        ),
    },
)

def _make_files(filenames):
    for f in filenames:
        make_file(
            name = f,
            filename = f,
            content = "",
        )

def _test_debug_symbols_collection():
    # Test that an empty debug symbols info is returned when no deps have debug
    # symbols
    _top_level_target(
        name = "empty_deps",
    )

    debug_symbols_collection_test(
        name = "test_empty_deps",
        target_under_test = ":empty_deps",
        expected_build_id_dirs = "{}",
    )

    # Test that a single dep works
    _make_files(["single_dep_root", "single_dep_build_dir"])

    fuchsia_debug_symbols(
        name = "single_dep_debug_symbols",
        source_search_root = ":single_dep_root",
        build_id_dirs = [":single_dep_build_dir"],
    )

    _top_level_target(
        name = "single_dep",
        dep = ":single_dep_debug_symbols",
    )

    debug_symbols_collection_test(
        name = "test_single_dep",
        target_under_test = ":single_dep",
        expected_build_id_dirs = """
        {
            "fuchsia/debug_symbols/single_dep_root": ["fuchsia/debug_symbols/single_dep_build_dir"]
        }
        """,
    )

    # Test that we can have nested aspects
    _top_level_target(
        name = "nested_top_level",
        dep = ":single_dep",
    )

    debug_symbols_collection_test(
        name = "test_nested_aspects_works",
        target_under_test = ":nested_top_level",
        expected_build_id_dirs = """
        {
            "fuchsia/debug_symbols/single_dep_root": ["fuchsia/debug_symbols/single_dep_build_dir"]
        }
        """,
    )

    # Test that label_lists work
    _make_files(["multi_dep_root", "multi_dep_build_dir_1", "multi_dep_build_dir_2"])

    fuchsia_debug_symbols(
        name = "multi_deps_debug_symbols_1",
        source_search_root = ":multi_dep_root",
        build_id_dirs = [":multi_dep_build_dir_1"],
    )

    fuchsia_debug_symbols(
        name = "multi_deps_debug_symbols_2",
        source_search_root = ":multi_dep_root",
        build_id_dirs = [":multi_dep_build_dir_2"],
    )

    _top_level_target(
        name = "multi_deps",
        deps = [
            ":multi_deps_debug_symbols_1",
            ":multi_deps_debug_symbols_2",
        ],
    )

    debug_symbols_collection_test(
        name = "test_label_list_works",
        target_under_test = ":multi_deps",
        expected_build_id_dirs = """
        {
            "fuchsia/debug_symbols/multi_dep_root": [
                "fuchsia/debug_symbols/multi_dep_build_dir_1",
                "fuchsia/debug_symbols/multi_dep_build_dir_2"
            ]
        }
        """,
    )

    # Test that all attributes set will merge correctly
    _make_files(["all_attrs_root_1", "all_attrs_root_2", "all_attrs_root_3", "all_attrs_build_dir_1", "all_attrs_build_dir_2", "all_attrs_build_dir_3"])

    fuchsia_debug_symbols(
        name = "all_attrs_debug_symbols_1",
        source_search_root = ":all_attrs_root_1",
        build_id_dirs = [":all_attrs_build_dir_1"],
    )

    fuchsia_debug_symbols(
        name = "all_attrs_debug_symbols_2",
        source_search_root = ":all_attrs_root_2",
        build_id_dirs = [":all_attrs_build_dir_2"],
    )

    fuchsia_debug_symbols(
        name = "all_attrs_debug_symbols_3",
        source_search_root = ":all_attrs_root_3",
        build_id_dirs = [":all_attrs_build_dir_3", ":all_attrs_build_dir_1"],
    )

    _top_level_target(
        name = "all_attrs",
        deps = [
            ":all_attrs_debug_symbols_1",
            ":all_attrs_debug_symbols_2",
            ":all_attrs_debug_symbols_3",
        ],
    )

    debug_symbols_collection_test(
        name = "test_label_merge_all_attrs_works",
        target_under_test = ":all_attrs",
        expected_build_id_dirs = """
        {
            "fuchsia/debug_symbols/all_attrs_root_1": [
                "fuchsia/debug_symbols/all_attrs_build_dir_1"
            ],
            "fuchsia/debug_symbols/all_attrs_root_2": [
                "fuchsia/debug_symbols/all_attrs_build_dir_2"
            ],
            "fuchsia/debug_symbols/all_attrs_root_3": [
                "fuchsia/debug_symbols/all_attrs_build_dir_3",
                "fuchsia/debug_symbols/all_attrs_build_dir_1"
            ]
        }
        """,
    )

def debug_symbols_test_suite(name, **kwargs):
    _test_debug_symbols_collection()

    native.test_suite(
        name = name,
        tests = [
            ":test_empty_deps",
            ":test_single_dep",
            ":test_nested_aspects_works",
            ":test_label_list_works",
            ":test_label_merge_all_attrs_works",
        ],
        **kwargs
    )
