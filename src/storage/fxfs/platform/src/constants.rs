// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// NOTE: These constants are used as part of the host build, which means there can be version skew
// between what might be used on a device and what a host tool might be using.

// VMO holding blobs served by fxfs have a name starting with this prefix.
pub const BLOB_NAME_PREFIX: &str = "blob-";

// Length of the hash hex representation appended to the VMO name.
pub const BLOB_NAME_HASH_LENGTH: usize = 8;
