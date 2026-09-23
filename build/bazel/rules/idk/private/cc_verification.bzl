# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rules related to verification of C++ atoms."""

visibility("private")

def _verify_no_pragma_once_impl(ctx):
    stamp_file = ctx.actions.declare_file(ctx.label.name + ".stamp")
    args = ctx.actions.args()
    args.add("--stamp", stamp_file.path)
    args.add_all("--headers", ctx.files.files)
    ctx.actions.run(
        executable = ctx.executable._script,
        arguments = [args],
        inputs = ctx.files.files,
        outputs = [stamp_file],
        tools = [ctx.executable._script],
        mnemonic = "VerifyNoPragmaOnceInHeaders",
    )
    return [DefaultInfo(files = depset([stamp_file]))]

verify_no_pragma_once = rule(
    doc = "Verifies that a group of (header) files does not contain `#pragma once` directives.",
    implementation = _verify_no_pragma_once_impl,
    attrs = {
        "files": attr.label_list(
            doc = "The list of (header) files to check.",
            allow_files = True,
            mandatory = True,
        ),
        "_script": attr.label(
            doc = "The script to run.",
            default = "//build/cpp:verify_pragma_once",
            executable = True,
            cfg = "exec",
        ),
    },
)

def create_verify_pragma_once_target(
        *,
        name,
        files,
        testonly,
        visibility):
    """Creates a target that ensures there are no #pragma once directives in `files`.

    Args:
        name: Name of the target for which the verification is being performed.
        files: List of files to check.
        testonly: Standard meaning.
        visibility: Standard meaning.
    Returns:
        The relative label of the target.
    """
    target_name = "%s.verify_pragma_once" % name
    verify_no_pragma_once(
        name = target_name,
        files = files,
        testonly = testonly,
        visibility = visibility,
    )
    return ":%s" % target_name

def create_verify_no_duplicate_files_target(
        *,
        name,
        hdrs,
        hdrs_for_internal_use,
        srcs,
        testonly,
        visibility):
    """Creates a target that ensures there are no duplicate files specified.

    It works by providing all source files as a single list of labels. Bazel
    will report an error if the combined list containes duplicates.

    Args:
        name: Name of the target for which the verification is being performed.
        hdrs: See idk_cc_source_library().
        hdrs_for_internal_use: See idk_cc_source_library().
        srcs: See idk_cc_source_library().
        testonly: Standard meaning.
        visibility: Standard meaning.
    Returns:
        The relative label of the target.
    """
    target_name = "%s.verify_no_duplicate_files" % name
    native.filegroup(
        name = target_name,
        data = hdrs + hdrs_for_internal_use + srcs,
        testonly = testonly,
        visibility = visibility,
    )
    return ":%s" % target_name
