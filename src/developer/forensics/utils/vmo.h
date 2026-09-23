// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_UTILS_VMO_H_
#define SRC_DEVELOPER_FORENSICS_UTILS_VMO_H_

#include <lib/zx/result.h>
#include <lib/zx/vmo.h>

#include <string>
#include <string_view>

namespace forensics {

// Reads the contents of |vmo| as a string.
//
// The size of the returned string is determined by the stream size of |vmo|.
// Returns a string on success, or a zx_status_t error if retrieving the stream size or
// reading fails.
zx::result<std::string> StringFromVmo(const zx::unowned_vmo& vmo);
zx::result<std::string> StringFromVmo(const zx::vmo& vmo);

// Creates a new VMO containing |string|.
//
// The stream size of the returned VMO is set to |string.size()|.
// Returns the VMO on success, or a zx_status_t error if creating or writing to the VMO fails.
zx::result<zx::vmo> VmoFromString(std::string_view string);

}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_UTILS_VMO_H_
