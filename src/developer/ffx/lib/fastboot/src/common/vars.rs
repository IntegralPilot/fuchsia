// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub const REVISION_VAR: &str = "hw-revision";
pub const IS_USERSPACE_VAR: &str = "is-userspace";
pub const MAX_DOWNLOAD_SIZE_VAR: &str = "max-download-size";
pub const PRODUCT_VAR: &str = "product";

pub const LOCKED_VAR: &str = "vx-locked";

// Streaming flash variables

// Standalone
pub const STREAM_SEGMENT_SIZE: &str = "stream-segment-size";

// Takes a 'partition' argument
pub const PARTITION_SIZE: &str = "partition-size";
pub const PARTITION_START: &str = "partition-start";

/// Returns whether a fastboot variable is safe to cache in memory.
///
/// Only immutable variables describing hardware properties, static partition geometry,
/// or fixed version metadata are cached. Mutable variables such as `current-slot`,
/// `unlocked`, slot bootability metadata, or hardware sensors/telemetry are not cached
/// to ensure fresh state is always read from the target.
pub fn is_cacheable_variable(name: &str) -> bool {
    match name {
        "version"
        | "version-bootloader"
        | "version-baseband"
        | "product"
        | "hw-revision"
        | "variant"
        | "serialno"
        | "max-download-size"
        | "slot-count"
        | "is-userspace"
        | "secure"
        | "erase-block-size"
        | "logical-block-size"
        | "stream-segment-size" => true,
        var if var.starts_with("partition-type:")
            || var.starts_with("partition-size:")
            || var.starts_with("partition-start:")
            || var.starts_with("has-slot:")
            || var.starts_with("is-logical:") =>
        {
            true
        }
        _ => false,
    }
}
