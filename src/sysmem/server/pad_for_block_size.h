// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYSMEM_SERVER_PAD_FOR_BLOCK_SIZE_H_
#define SRC_SYSMEM_SERVER_PAD_FOR_BLOCK_SIZE_H_

#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/image-format/image_format.h>
#include <zircon/assert.h>

#include <limits>

#include <fbl/algorithm.h>

#include "src/sysmem/server/logging.h"

namespace sysmem_service {

// The result is at least one more than the offset of the last byte that a participant accessing in
// blocks of block_size will ever access, given the buffer_settings_size_bytes constraint on
// ImageFormatImageSize, and the image_format_constraints. The return value is only required to be a
// reasonably tight upper bound, not the minimum value that satisfies the requirement.
//
// This isn't expected to fail outside of adversarial client behavior (and in tests). If this fails,
// it's fine for the caller to log a lot at ERROR. For unit testing it's better if this function
// (and code called by this function) doesn't itself log.
fit::result<fit::failed, uint64_t> PaddedSizeFromBlockSize(
    const fuchsia_sysmem2::ImageFormatConstraints& image_format_constraints,
    uint64_t buffer_settings_size_bytes, const ComplainFunction& complain);

}  // namespace sysmem_service

#endif  // SRC_SYSMEM_SERVER_PAD_FOR_BLOCK_SIZE_H_
