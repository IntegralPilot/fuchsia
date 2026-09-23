// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_DISPLAY_FIDL_ID_TYPES_H_
#define SRC_UI_SCENIC_LIB_DISPLAY_FIDL_ID_TYPES_H_

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <fidl/fuchsia.hardware.display/cpp/fidl.h>

#include "src/ui/scenic/lib/display/fidl_typedefs.h"
#include "src/ui/scenic/lib/types/id_type.h"

namespace display::internal {

using types::DefaultIdTypeTraitsForWireFidl;

using BufferCollectionIdTraits = DefaultIdTypeTraitsForWireFidl<uint64_t, WireBufferCollectionId>;
using DisplayIdTraits = DefaultIdTypeTraitsForWireFidl<uint64_t, WireDisplayId>;
using EventIdTraits = DefaultIdTypeTraitsForWireFidl<uint64_t, WireEventId>;
using ImageIdTraits = DefaultIdTypeTraitsForWireFidl<uint64_t, WireImageId>;
using LayerIdTraits = DefaultIdTypeTraitsForWireFidl<uint64_t, WireLayerId>;

}  // namespace display::internal

namespace display {

using BufferCollectionId = types::IdType<display::internal::BufferCollectionIdTraits>;
using DisplayId = types::IdType<display::internal::DisplayIdTraits>;
using EventId = types::IdType<display::internal::EventIdTraits>;
using ImageId = types::IdType<display::internal::ImageIdTraits>;
using LayerId = types::IdType<display::internal::LayerIdTraits>;

constexpr BufferCollectionId kInvalidBufferCollectionId = BufferCollectionId(0);
constexpr DisplayId kInvalidDisplayId = DisplayId(0);
constexpr EventId kInvalidEventId = EventId(0);
constexpr ImageId kInvalidImageId = ImageId(0);
constexpr LayerId kInvalidLayerId = LayerId(0);

}  // namespace display

#endif  // SRC_UI_SCENIC_LIB_DISPLAY_FIDL_ID_TYPES_H_
