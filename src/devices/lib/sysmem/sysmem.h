// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_LIB_SYSMEM_SYSMEM_H_
#define SRC_DEVICES_LIB_SYSMEM_SYSMEM_H_

#include <fidl/fuchsia.sysmem/cpp/wire_types.h>
#include <fuchsia/sysmem/c/banjo.h>
#include <fuchsia/sysmem/cpp/fidl.h>

namespace sysmem {

fuchsia_sysmem::wire::PixelFormat banjo_to_fidl(const pixel_format_t& source);

image_format_2_t fidl_to_banjo(const fuchsia_sysmem::wire::ImageFormat2& source);

fuchsia_sysmem::wire::ImageFormat2 banjo_to_fidl(const image_format_2_t& source);

buffer_collection_info_2_t fidl_to_banjo(const fuchsia::sysmem::BufferCollectionInfo_2& source);

// the vmo handle values in the returned struct are the same as those managed by source, so the
// caller must ensure that the source out-lasts the returned struct (or at least, outlasts any usage
// of the vmo handles in the returned struct)
buffer_collection_info_2_t fidl_to_banjo(const fuchsia::sysmem2::BufferCollectionInfo& source);

}  // namespace sysmem

#endif  // SRC_DEVICES_LIB_SYSMEM_SYSMEM_H_
