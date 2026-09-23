# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rule for creating a bootloader partition mapping."""

load(":providers.bzl", "FuchsiaPartitionInfo")

def _fuchsia_bootloader_partition_impl(ctx):
    bootloader_partition = {
        "name": ctx.attr.partition_name,
        "image": ctx.file.image.path,
        "type": ctx.attr.type,
    }

    return [
        DefaultInfo(files = depset(direct = [ctx.file.image])),
        FuchsiaPartitionInfo(
            partition = bootloader_partition,
        ),
    ]

fuchsia_bootloader_partition = rule(
    doc = """Define a partition mapping from partition to image.""",
    implementation = _fuchsia_bootloader_partition_impl,
    provides = [FuchsiaPartitionInfo],
    attrs = {
        "partition_name": attr.string(
            doc = "Name of the partition.",
            mandatory = True,
        ),
        "image": attr.label(
            doc = "The bootloader image file.",
            allow_single_file = True,
            mandatory = True,
        ),
        "type": attr.string(
            doc = """The firmware type provided to the update system.
            This value is a unique identifier for a partition known by the paver driver.""",
            mandatory = True,
        ),
    },
)
