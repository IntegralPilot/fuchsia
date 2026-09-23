// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/edid-reader.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/fit/function.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <algorithm>
#include <array>
#include <cstdint>
#include <span>

#include <fbl/alloc_checker.h>
#include <fbl/vector.h>

#include "src/graphics/display/lib/edid/edid.h"

namespace intel_display {

zx::result<fbl::Vector<uint8_t>> ReadExtendedEdid(ReadEdidBlockFunction read_edid_block) {
  std::array<uint8_t, edid::kBlockSize> base_edid;
  zx::result<> base_edid_result = read_edid_block(0, base_edid);
  if (base_edid_result.is_error()) {
    fdf::error("Failed to read EDID base block: {}", base_edid_result);
    return base_edid_result.take_error();
  }

  // VESA Enhanced Extended Display Identification Data (E-EDID) Standard,
  // Release A, Revision 2, dated September 25, 2006, revised December 31, 2020.
  // Section 3.1 "EDID Format Overview", page 19.
  static constexpr int kBaseEdidExtensionBlockCountOffset = 126;
  const int extension_block_count = base_edid[kBaseEdidExtensionBlockCountOffset];

  fbl::Vector<uint8_t> extended_edid;
  fbl::AllocChecker alloc_checker;
  const size_t extended_edid_size =
      static_cast<size_t>(extension_block_count + 1) * edid::kBlockSize;
  extended_edid.resize(extended_edid_size, 0, &alloc_checker);

  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate {} bytes for E-EDID", extended_edid_size);
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  std::ranges::copy(base_edid, extended_edid.begin());

  for (int extension_block_index = 1; extension_block_index <= extension_block_count;
       ++extension_block_index) {
    int extension_block_offset = extension_block_index * static_cast<int>(edid::kBlockSize);
    std::span<uint8_t, edid::kBlockSize> extension_block(
        extended_edid.begin() + extension_block_offset, edid::kBlockSize);
    zx::result<> extension_block_result = read_edid_block(extension_block_index, extension_block);
    if (extension_block_result.is_error()) {
      fdf::error("Failed to read EDID extension block #{}: {}", extension_block_index,
                 extension_block_result);
      return extension_block_result.take_error();
    }
  }

  return zx::ok(std::move(extended_edid));
}

}  // namespace intel_display
