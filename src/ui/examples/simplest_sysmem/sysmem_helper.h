// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_EXAMPLES_SIMPLEST_SYSMEM_SYSMEM_HELPER_H_
#define SRC_UI_EXAMPLES_SIMPLEST_SYSMEM_SYSMEM_HELPER_H_

#include <fuchsia/images2/cpp/fidl.h>
#include <fuchsia/sysmem2/cpp/fidl.h>
#include <fuchsia/ui/composition/cpp/fidl.h>

namespace sysmem_helper {

using fuchsia::ui::composition::BufferCollectionExportToken;
using fuchsia::ui::composition::BufferCollectionImportToken;

using fuchsia::images2::PixelFormat;
using fuchsia::sysmem2::BufferCollectionConstraints;
using fuchsia::sysmem2::BufferCollectionInfo;

// Convenience function which allows clients to easily create a valid |BufferCollectionExportToken|
// |BufferCollectionImportToken| pair for use between Allocator and Flatland.
struct BufferCollectionImportExportTokens {
  static BufferCollectionImportExportTokens New();

  BufferCollectionExportToken export_token;
  BufferCollectionImportToken import_token;
};

struct BufferConstraint {
  uint32_t buffer_count;
  uint32_t image_width;
  uint32_t image_height;
  uint32_t bytes_per_pixel;
  PixelFormat pixel_format_type;
};

// Create default constraints used to allocate a sysmem buffer.
BufferCollectionConstraints CreateDefaultConstraints(BufferConstraint buffer_constraint);

// Operates on vmo allocated by sysmem. Implement the |callback| to populate the vmo with the
// desired image data.
void MapHostPointer(const BufferCollectionInfo& collection_info, uint32_t vmo_idx,
                    std::function<void(uint8_t*, uint64_t)> callback);

}  // namespace sysmem_helper
#endif  // SRC_UI_EXAMPLES_SIMPLEST_SYSMEM_SYSMEM_HELPER_H_
