// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_DRIVER_BUFFER_COLLECTION_ID_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_DRIVER_BUFFER_COLLECTION_ID_H_

#include <fidl/fuchsia.hardware.display.engine/cpp/wire.h>

#include <cstdint>

#include "src/graphics/display/lib/api-types/cpp/id-type.h"

namespace display::internal {

using DriverBufferCollectionIdTraits =
    DefaultIdTypeTraits<uint64_t, fuchsia_hardware_display_engine::wire::BufferCollectionId>;

}  // namespace display::internal

namespace display {

// More useful representation of
// `fuchsia.hardware.display.engine/BufferCollectionId`.
//
// See `BufferCollectionId` for the type used at the interface between the
// display coordinator and clients such as Scenic.
using DriverBufferCollectionId =
    display::internal::IdType<display::internal::DriverBufferCollectionIdTraits>;

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_DRIVER_BUFFER_COLLECTION_ID_H_
