# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rule for creating a partition mapping."""

load(
    ":providers.bzl",
    "FuchsiaPartitionInfo",
)

SLOT = struct(
    A = "A",  # Primary slot
    B = "B",  # Alternate slot
    R = "R",  # Recovery slot
)

PARTITION_TYPE = struct(
    ZBI = "ZBI",
    RECOVERY_ZBI = "RecoveryZBI",
    VBMETA = "VBMeta",
    RECOVERY_VBMETA = "RecoveryVBMeta",
    DTBO = "Dtbo",
    FVM = "FVM",
    FXFS = "Fxfs",
)

def _fuchsia_partition_impl(ctx):
    partition = {
        "name": ctx.attr.partition_name,
        "type": ctx.attr.type,
    }
    if ctx.attr.slot != "":
        partition["slot"] = ctx.attr.slot
    if ctx.attr.size_kib:
        partition["size"] = ctx.attr.size_kib * 1024

    return [
        FuchsiaPartitionInfo(
            partition = partition,
        ),
    ]

fuchsia_partition = rule(
    doc = """Define a partition mapping from partition to image.""",
    implementation = _fuchsia_partition_impl,
    provides = [FuchsiaPartitionInfo],
    attrs = {
        "partition_name": attr.string(
            doc = "Name of the partition",
            mandatory = True,
        ),
        "slot": attr.string(
            doc = "The slot of the partition",
            values = [SLOT.A, SLOT.B, SLOT.R],
        ),
        "size_kib": attr.int(
            doc = "The size of the partition in kibibytes",
        ),
        "type": attr.string(
            doc = "Type of this partition",
            mandatory = True,
            values = [PARTITION_TYPE.ZBI, PARTITION_TYPE.RECOVERY_ZBI, PARTITION_TYPE.VBMETA, PARTITION_TYPE.RECOVERY_VBMETA, PARTITION_TYPE.DTBO, PARTITION_TYPE.FVM, PARTITION_TYPE.FXFS],
        ),
    },
)
