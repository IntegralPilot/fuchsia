# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rule for creating a bootstrap partition mapping."""

load(":providers.bzl", "FuchsiaPartitionInfo")

def _fuchsia_bootstrap_partition_impl(ctx):
    bootstrap_partition = {
        "name": ctx.attr.partition_name,
        "image": ctx.file.image.path,
    }
    if ctx.attr.condition_variable != "":
        bootstrap_partition["condition"] = {
            "variable": ctx.attr.condition_variable,
            "value": ctx.attr.condition_value,
        }

    return [
        DefaultInfo(files = depset(direct = [ctx.file.image])),
        FuchsiaPartitionInfo(
            partition = bootstrap_partition,
        ),
    ]

fuchsia_bootstrap_partition = rule(
    doc = """Define a partition mapping from partition to image.""",
    implementation = _fuchsia_bootstrap_partition_impl,
    provides = [FuchsiaPartitionInfo],
    attrs = {
        "partition_name": attr.string(
            doc = "Name of the partition.",
            mandatory = True,
        ),
        "image": attr.label(
            doc = "The bootstrap image file.",
            allow_single_file = True,
            mandatory = True,
        ),
        "condition_variable": attr.string(
            doc = "Variable part of the condition that needs to be met before flash.",
        ),
        "condition_value": attr.string(
            doc = "Value part of the condition that needs to be met before flash.",
        ),
    },
)
