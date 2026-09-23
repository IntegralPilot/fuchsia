# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Macros and aspects for defining runtime test data for Fuchsia `host_test()` targets."""

load("//build/bazel/aspects:utils.bzl", "get_target_deps_from_attributes")

FuchsiaHostTestDataInfo = provider(
    doc = "Runtime test data information for Fuchsia host_test() targets.",
    fields = {
        # LINT.IfChange(FuchsiaHostTestDataInfo)
        "label": "A Label value for the target providing this value. Used for debugging.",
        "files": """
            A { test_data_path -> artifact } dictionary, whose keys are path strings,
            relative to the test runtime directory, and whose values are Files pointing
            to a Bazel artifact or source file that will appear there.
        """,
        # LINT.ThenChange(//build/bazel/scripts/generate_host_test_wrapper.py:FuchsiaHostTestDataInfo)
    },
)

def _host_test_data_map_impl(ctx):
    files = {}
    for dest_path, src in ctx.attr.files_map.items():
        if DefaultInfo in src:
            files[dest_path] = src[DefaultInfo].files.to_list()[0]
        else:
            fail("No DefaultInfo in {}".format(src))

    return FuchsiaHostTestDataInfo(
        label = ctx.label,
        files = files,
    )

host_test_data_map = rule(
    implementation = _host_test_data_map_impl,
    doc = """
        Define runtime test data for Fuchsia host_test() targets using a dictionary.
        This rule is used to copy source files or Bazel artifacts to the test's runtime directory.

        This rule provides an explicit mapping from Bazel labels to paths relative to
        the host test's current directory at runtime. For example:

           host_test(
                name = "my_test",
                ...
                data = [ ":my_test_data" ]
           )

           genrule(
               name = "generator",
               ...
               outs = [ "generated1", "generated2" ],
           )

           host_test_data_map(
                name = "my_test_data",
                files_map = {
                    "generated": ":generator",
                    "test_data/file1": "data_file_1.txt",
                }
           )

        Will ensure that `my_test` will be able to directly load `generated` and `test_data/file1`
        at runtime, whose content will match the `generated1` artifact, and the `data_file_1.txt`
        source file, respectively.

        Note that only files, not directories, are supported by this rule.
        """,
    provides = [FuchsiaHostTestDataInfo],
    attrs = {
        "files_map": attr.string_keyed_label_dict(
            doc = """
                A dictionary mapping runtime paths to source or target labels.

                Each key is a file path, relative to the test's runtime directory.
                Each value is a label that can point to a source file, or to a Bazel target,
                in which case only the first artifact it generates will be used as the source.
                """,
            allow_files = True,
            mandatory = True,
        ),
    },
)

def _host_test_data_files_impl(ctx):
    files = {}
    dest_dir = ctx.attr.dest_dir
    for src in ctx.files.srcs:
        dest_path = "{}/{}".format(dest_dir, src.basename) if dest_dir else src.basename
        files[dest_path] = src

    return [FuchsiaHostTestDataInfo(
        label = ctx.label,
        files = files,
    )]

host_test_data_files = rule(
    doc = """
        Define runtime test data for Fuchsia host_test() targets using a label list.

        This rule is used to copy source files or Bazel artifacts to the test's runtime directory
        or one of its sub-directories if "dest_dir" is set. Filenames are always preserved,
        and no sub-directory information from source file paths is preserved.

        See host_test_data_map() if changing the install filenames is needed.

        For example:

           host_test(
                name = "my_test",
                ...
                data = [ ":my_test_data" ]
           )

           genrule(
               name = "generator",
               ...
               outs = [ "generated1", "generated2" ],
           )

           host_test_data_files(
                name = "my_test_data",
                srcs = [
                    ":generator",
                    "data_file_1.txt",
                    "subdir/data_file_2.txt",
                ],
                dest_dir = "test_data",
           )

        Will ensure that `my_test` will be able to directly load "test_data/generated1" and
        "test_data/data_file_1.txt" and "test_data/data_file_2.txt" at runtime.

        Note that only files, not directories, are supported by this rule.
    """,
    implementation = _host_test_data_files_impl,
    provides = [FuchsiaHostTestDataInfo],
    attrs = {
        "srcs": attr.label_list(
            doc = """
                A list of source files or target labels. For targets, only its first generated
                artifact will be used as the source.
                """,
            mandatory = True,
            allow_files = True,
        ),
        "dest_dir": attr.string(
            doc = """
                Optional installation sub-directory, relative to the test's runtime directory.
            """,
            default = "",
        ),
    },
)

CollectedFuchsiaHostTestDataInfo = provider(
    doc = "Collected FuchsiaHostTestDataInfo providers from dependencies.",
    fields = {
        "infos": "A depset[FuchsiaHostTestDataInfo]",
    },
)

# An aspect to collect FuchsiaHostTestDataInfo values from dependencies.
def _collect_fuchsia_host_test_data_aspect_impl(target, aspect_ctx):
    target_deps = get_target_deps_from_attributes(aspect_ctx.rule.attr)
    direct_infos = []
    if FuchsiaHostTestDataInfo in target:
        direct_infos = [target[FuchsiaHostTestDataInfo]]

    transitive_infos = [
        dep[CollectedFuchsiaHostTestDataInfo].infos
        for dep in target_deps
        if CollectedFuchsiaHostTestDataInfo in dep
    ]
    return [CollectedFuchsiaHostTestDataInfo(
        infos = depset(direct_infos, transitive = transitive_infos),
    )]

collect_fuchsia_host_test_data_aspect = aspect(
    doc = "An aspect used to collect FuchsiaHostTestDataInfo values from dependencies.",
    implementation = _collect_fuchsia_host_test_data_aspect_impl,
    attr_aspects = ["*"],
    provides = [CollectedFuchsiaHostTestDataInfo],
)
