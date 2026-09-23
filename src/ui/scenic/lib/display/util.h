// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_DISPLAY_UTIL_H_
#define SRC_UI_SCENIC_LIB_DISPLAY_UTIL_H_

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>

#include <cstdint>

#include "src/ui/scenic/lib/allocation/id.h"
#include "src/ui/scenic/lib/display/coordinator_proxy.h"
#include "src/ui/scenic/lib/display/fidl_id_types.h"

namespace display {

// Imports a sysmem buffer collection token to a display controller, and sets the constraints.
// A successful import will return true, otherwise it will return false.
bool ImportBufferCollection(
    allocation::GlobalBufferCollectionId buffer_collection_id,
    const fidl::WireSharedClient<fuchsia_hardware_display::Coordinator>& display_coordinator,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token,
    const fuchsia_hardware_display_types::wire::ImageBufferUsage& image_buffer_usage);

// Imports a zx::event to the provided display controller. The return value is an ID to
// reference that event on other display controller functions that take an event as an
// argument. On failure, the return value will be fuchsia_hardware_display_types::kInvalidDispId.
//
// Deprecated.  Prefer to use `CoordinatorProxy::ImportEvent()`.
EventId ImportEventForTest(
    const fidl::WireSharedClient<fuchsia_hardware_display::Coordinator>& display_coordinator,
    const zx::event& event);

// Returns true if the display controller backend is capable of image capture (i.e.
// screenshotting).
bool IsCaptureSupported(
    const fidl::WireSharedClient<fuchsia_hardware_display::Coordinator>& display_coordinator);

// Imports an image to be used for capture (i.e. screenshotting) of the display
// controller, associated with `capture_image_id`.
//
// `capture_image_id` must not be used for any images, including images
// imported for display.
//
// On failure, this function returns the error value.
//
// TODO(https://fxbug.dev/42080575): Unify this method with ImportBufferImage().
zx_status_t ImportImageForCapture(CoordinatorProxy& display_coordinator,
                                  const WireImageMetadata& image_metadata,
                                  allocation::GlobalBufferCollectionId buffer_collection_id,
                                  uint32_t vmo_idx, allocation::GlobalImageId capture_image_id);

inline WireBufferCollectionId ToDisplayFidlBufferCollectionId(
    allocation::GlobalBufferCollectionId global_buffer_collection_id) {
  return {.value = global_buffer_collection_id};
}

}  // namespace display

#endif  // SRC_UI_SCENIC_LIB_DISPLAY_UTIL_H_
