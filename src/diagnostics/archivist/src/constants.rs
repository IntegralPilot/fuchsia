// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// The maximum number of bytes in a formatted content VMO.
pub const FORMATTED_CONTENT_CHUNK_SIZE_TARGET: u64 = 1 << 20; // 1 MiB

/// For production archivist instances this value is retrieved from configuration but we still
/// provide a default here for internal testing purposes.
pub const LEGACY_DEFAULT_MAXIMUM_CACHED_LOGS_BYTES: u64 = 4 * 1024 * 1024;

/// Default path where pipeline configuration are located.
pub const DEFAULT_PIPELINES_PATH: &str = "/config/data";
