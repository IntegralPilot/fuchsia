// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_DRIVER_IMAGE_ID_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_DRIVER_IMAGE_ID_H_

#include <fidl/fuchsia.hardware.display.engine/cpp/wire.h>

#include <cstdint>

#include "src/graphics/display/lib/api-types/cpp/id-type.h"

namespace display::internal {

using DriverImageIdTraits =
    DefaultIdTypeTraits<uint64_t, fuchsia_hardware_display_engine::wire::ImageId>;

}  // namespace display::internal

namespace display {

// More useful representation of `fuchsia.hardware.display.engine/ImageId`.
//
// The Banjo API between the Display Coordinator and drivers currently refers to
// this concept as "image handle". This name will be phased out when the
// API is migrated from Banjo to FIDL.
//
// See `ImageId` for the type used at the interface between the display
// coordinator and clients such as Scenic.
//
// TODO(https://fxbug.dev/42079544): Unify this type with `DriverCaptureImageId`
// when unifying image ID namespaces.
using DriverImageId = display::internal::IdType<display::internal::DriverImageIdTraits>;

constexpr DriverImageId kInvalidDriverImageId(fuchsia_hardware_display_engine::wire::kInvalidId);

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_DRIVER_IMAGE_ID_H_
