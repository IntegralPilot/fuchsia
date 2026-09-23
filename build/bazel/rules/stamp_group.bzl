# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""stamp_group() custom rule definition."""

# Use when defining a rule that uses `stamp_group_impl()` as its implementation.
# `"deps": attr.label_list(...)` must also be included in the rule's attributes.
STAMP_GROUP_NON_DEPS_ATTRS = {
    "stamp": attr.output(
        doc = "Output file path.",
        mandatory = True,
    ),
    "_script": attr.label(
        doc = "The stamping script.",
        default = ":stamp_group.sh",
        allow_single_file = True,
    ),
}

def stamp_group_impl(ctx):
    """Implementation of stamp group rules.

    Ensures that the stamp file is generated once all dependencies have been built.

    It is public so that it can be used by other rules, such as those that need
    to apply a transition to the `deps`.
    """
    output = ctx.outputs.stamp

    dep_outputs = depset(
        transitive = [dep[DefaultInfo].files for dep in ctx.attr.deps],
    )

    # Run a tiny script that generates `output`, and tell Bazel
    # that it inputs are the output files of dependencies.
    ctx.actions.run(
        outputs = [output],
        inputs = dep_outputs,
        executable = ctx.file._script,
        arguments = [output.path],
    )
    return [DefaultInfo(files = depset([output]))]

stamp_group = rule(
    implementation = stamp_group_impl,
    doc = "A filegroup() like target that generates a stamp file once all its dependencies have been built." +
          "Useful for targets of GN bazel_action() targets, which require at least one output.",
    attrs = {
        "deps": attr.label_list(
            doc = "List of labels to dependencies.",
            mandatory = True,
        ),
    } | STAMP_GROUP_NON_DEPS_ATTRS,
)
