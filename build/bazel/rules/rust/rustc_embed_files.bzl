# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""A rule and macro for embedding files as byte array constants in a Rust library."""

load("//build/bazel/rules/rust:rustc_library.bzl", "rustc_library")

def _relative_path(from_path, to_path):
    """
    Returns the relative path from `from_path` to `to_path`.

    Args:
        from_path: The path to calculate the relative path from.
        to_path: The target path.
    """
    from_parts = from_path.split("/")
    to_parts = to_path.split("/")

    # Strip common prefixes.
    common_prefix_len = 0
    for i in range(min(len(from_parts), len(to_parts))):
        if from_parts[i] != to_parts[i]:
            break
        common_prefix_len += 1

    up_levels = len(from_parts) - common_prefix_len
    return "/".join([".."] * up_levels + to_parts[common_prefix_len:])

def _rustc_embed_files_gen_impl(ctx):
    output_file = ctx.actions.declare_file(ctx.label.name + ".rs")

    content = []
    for target, const_name in ctx.attr.files.items():
        files = target.files.to_list()
        if len(files) != 1:
            fail("Target {} in rustc_embed_files files must produce exactly one file, but produced: {}".format(target.label, files))
        f = files[0]

        rel_path = _relative_path(output_file.dirname, f.path)
        content.append("pub const {} : &[u8] = include_bytes!(\"{}\");".format(const_name, rel_path))

    ctx.actions.write(
        output = output_file,
        content = "\n".join(content) + "\n",
    )

    return [
        DefaultInfo(
            files = depset([output_file]),
        ),
    ]

_rustc_embed_files_gen = rule(
    implementation = _rustc_embed_files_gen_impl,
    attrs = {
        "files": attr.label_keyed_string_dict(
            doc = "Map of file label to constant name. Each label should point to a single file, or a target that produces exactly one file.",
            mandatory = True,
            allow_files = True,
        ),
    },
)

def rustc_embed_files(*, name, files, **kwargs):
    """Defines a Rust library that embeds the contents of some files as constants.

    Args:
        name: The target name.
        files: A dictionary mapping file labels to their constant names.
        **kwargs: Additional arguments forwarded to the underlying rustc_library.
    """
    gen_target = name + "_gen"

    _rustc_embed_files_gen(
        name = gen_target,
        files = files,
    )

    compile_data = kwargs.pop("compile_data", [])
    for file_label in files.keys():
        if file_label not in compile_data:
            compile_data.append(file_label)

    rustc_library(
        name = name,
        srcs = [":" + gen_target],
        crate_root = ":" + gen_target,
        compile_data = compile_data,
        **kwargs
    )
