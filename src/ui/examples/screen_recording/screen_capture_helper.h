// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_EXAMPLES_SCREEN_RECORDING_SCREEN_CAPTURE_HELPER_H_
#define SRC_UI_EXAMPLES_SCREEN_RECORDING_SCREEN_CAPTURE_HELPER_H_

#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>

namespace screen_recording_example {

using fuchsia_ui_composition::RegisterBufferCollectionUsages;

// Returns default buffer collection constraints for allocating `buffer_count` image buffers of the
// specified `width`, `height`, and `format` (defaulting to B8G8R8A8).
fuchsia_sysmem2::BufferCollectionConstraints CreateDefaultConstraints(
    uint32_t buffer_count, uint32_t width, uint32_t height,
    fuchsia_images2::PixelFormat format = fuchsia_images2::PixelFormat::kB8G8R8A8);

// Registers a shared Sysmem buffer collection with Flatland's Allocator and sets client
// constraints to allocate the buffers. This blocks until all participants have set their
// constraints and Sysmem has allocated the buffers.
void AllocateBufferCollection(
    fuchsia_sysmem2::BufferCollectionConstraints constraints,
    fuchsia_ui_composition::BufferCollectionExportToken export_token,
    fidl::SyncClient<fuchsia_ui_composition::Allocator>& flatland_allocator,
    fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
    RegisterBufferCollectionUsages usage);

}  // namespace screen_recording_example

#endif  // SRC_UI_EXAMPLES_SCREEN_RECORDING_SCREEN_CAPTURE_HELPER_H_
