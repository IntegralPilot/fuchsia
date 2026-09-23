// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/display/util.h"

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <lib/fidl/cpp/wire/status.h>
#include <lib/fit/defer.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>
#include <lib/zx/clock.h>
#include <lib/zx/event.h>
#include <zircon/status.h>

#include "src/ui/scenic/lib/allocation/id.h"

namespace display {

bool ImportBufferCollection(
    allocation::GlobalBufferCollectionId buffer_collection_id,
    const fidl::WireSharedClient<fuchsia_hardware_display::Coordinator>& display_coordinator,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token,
    const fuchsia_hardware_display_types::wire::ImageBufferUsage& image_buffer_usage) {
  TRACE_DURATION("gfx", "display::ImportBufferCollection");
  const WireBufferCollectionId display_buffer_collection_id =
      ToDisplayFidlBufferCollectionId(buffer_collection_id);

  auto import_buffer_collection_result = display_coordinator.sync()->ImportBufferCollection(
      display_buffer_collection_id, std::move(token));
  if (!import_buffer_collection_result.ok()) {
    FX_LOGS(ERROR) << "ImportBufferCollection transport failed: "
                   << import_buffer_collection_result.status_string();
    return false;
  }
  if (import_buffer_collection_result->is_error()) {
    FX_LOGS(ERROR) << "ImportBufferCollection method failed: "
                   << zx_status_get_string(import_buffer_collection_result->error_value());
    return false;
  }

  auto set_buffer_collection_constraints_result =
      display_coordinator.sync()->SetBufferCollectionConstraints(display_buffer_collection_id,
                                                                 image_buffer_usage);
  auto release_buffer_collection_on_failure = fit::defer([&] {
    fidl::OneWayStatus release_result =
        display_coordinator->ReleaseBufferCollection(display_buffer_collection_id);
    if (!release_result.ok()) {
      FX_LOGS(ERROR) << "ReleaseBufferCollection failed: " << release_result.status_string();
    }
  });
  if (!set_buffer_collection_constraints_result.ok()) {
    FX_LOGS(ERROR) << "SetBufferCollectionConstraints transport failed: "
                   << set_buffer_collection_constraints_result.status_string();
    return false;
  }
  if (set_buffer_collection_constraints_result->is_error()) {
    FX_LOGS(ERROR) << "SetBufferCollectionConstraints method failed: "
                   << zx_status_get_string(set_buffer_collection_constraints_result->error_value());
    return false;
  }

  release_buffer_collection_on_failure.cancel();
  return true;
}

EventId ImportEventForTest(
    const fidl::WireSharedClient<fuchsia_hardware_display::Coordinator>& display_coordinator,
    const zx::event& event) {
  // `CoordinatorProxy::ImportEvent()` has its own ID counter that starts at 1, so start this at
  // a big number to greatly reduce the chance of a collision.
  static EventId id_generator(987654321);

  zx::event dup;
  if (event.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup) != ZX_OK) {
    FX_LOGS(ERROR) << "Failed to duplicate display controller event.";
    return kInvalidEventId;
  }

  // Generate a new display ID after we've determined the event can be duplicated as to not
  // waste an id.
  EventId event_id = id_generator++;

  auto before = zx::clock::get_monotonic();
  fidl::OneWayStatus import_result =
      display_coordinator->ImportEvent(std::move(dup), event_id.ToFidl());
  if (!import_result.ok()) {
    auto after = zx::clock::get_monotonic();
    FX_LOGS(ERROR) << "Failed to import display controller event. Waited "
                   << (after - before).to_msecs()
                   << "msecs. Error code: " << import_result.status_string();
    return kInvalidEventId;
  }
  return event_id;
}

bool IsCaptureSupported(
    const fidl::WireSharedClient<fuchsia_hardware_display::Coordinator>& display_coordinator) {
  auto result = display_coordinator.sync()->IsCaptureSupported();
  if (!result.ok()) {
    FX_LOGS(ERROR) << "IsCaptureSupported call transport failed: " << result.status_string();
    return false;
  }
  if (result->is_error()) {
    FX_LOGS(ERROR) << "IsCaptureSupported call method failed: "
                   << zx_status_get_string(result->error_value());
    return false;
  }
  return (*result)->supported;
}

zx_status_t ImportImageForCapture(CoordinatorProxy& display_coordinator,
                                  const WireImageMetadata& image_metadata,
                                  allocation::GlobalBufferCollectionId buffer_collection_id,
                                  uint32_t vmo_idx, allocation::GlobalImageId image_id) {
  if (buffer_collection_id == 0) {
    FX_LOGS(ERROR) << "Buffer collection id is 0.";
    return 0;
  }

  if (image_metadata.tiling_type != fuchsia_hardware_display_types::kImageTilingTypeCapture) {
    FX_LOGS(ERROR) << "Image config tiling type must be IMAGE_TILING_TYPE_CAPTURE.";
    return 0;
  }

  const WireBufferCollectionId display_buffer_collection_id =
      ToDisplayFidlBufferCollectionId(buffer_collection_id);

  auto import_image_result = display_coordinator.ImportImage(
      Extent2::From(image_metadata.dimensions), image_metadata.tiling_type,
      display_buffer_collection_id, vmo_idx, image_id);
  return import_image_result.status_value();
}

}  // namespace display
