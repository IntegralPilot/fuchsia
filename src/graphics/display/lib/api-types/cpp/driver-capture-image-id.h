// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_DRIVER_CAPTURE_IMAGE_ID_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_DRIVER_CAPTURE_IMAGE_ID_H_

#include <fidl/fuchsia.hardware.display.engine/cpp/wire.h>

#include <cstdint>

#include "src/graphics/display/lib/api-types/cpp/id-type.h"

namespace display::internal {

using DriverCaptureImageIdTraits =
    DefaultIdTypeTraits<uint64_t, fuchsia_hardware_display_engine::wire::ImageId>;

}  // namespace display::internal

namespace display {

// Type-safe wrapper around capture image IDs exposed to display drivers.
//
// The Banjo API between the Display Coordinator and drivers currently refers to
// this concept as "capture image handle". This name will be phased out when the
// API is migrated from Banjo to FIDL.
//
// TODO(https://fxbug.dev/42079544): Remove this type when unifying image ID namespaces.
using DriverCaptureImageId =
    display::internal::IdType<display::internal::DriverCaptureImageIdTraits>;

constexpr DriverCaptureImageId kInvalidDriverCaptureImageId(
    fuchsia_hardware_display_engine::wire::kInvalidId);

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_DRIVER_CAPTURE_IMAGE_ID_H_
