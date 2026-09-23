// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_DISPLAY_FIDL_TYPEDEFS_H_
#define SRC_UI_SCENIC_LIB_DISPLAY_FIDL_TYPEDEFS_H_

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <fidl/fuchsia.hardware.display/cpp/wire.h>
#include <fidl/fuchsia.math/cpp/wire.h>

namespace display {

using WireAlphaMode = fuchsia_hardware_display_types::wire::AlphaMode;
using WireBufferCollectionId = fuchsia_hardware_display::wire::BufferCollectionId;
using WireColor = fuchsia_hardware_display_types::wire::Color;
using WireConfigResult = fuchsia_hardware_display_types::wire::ConfigResult;
using WireConfigStamp = fuchsia_hardware_display::wire::ConfigStamp;
using WireCoordinateTransformation = fuchsia_hardware_display_types::wire::CoordinateTransformation;
using WireDisplayId = fuchsia_hardware_display_types::wire::DisplayId;
using WireDisplayInfo = fuchsia_hardware_display::wire::Info;
using WireDisplayMode = fuchsia_hardware_display_types::wire::Mode;
using WireEventId = fuchsia_hardware_display::wire::EventId;
using WireImageId = fuchsia_hardware_display::wire::ImageId;
using WireImageMetadata = fuchsia_hardware_display_types::wire::ImageMetadata;
using WireLayerId = fuchsia_hardware_display::wire::LayerId;
using WireRectU = fuchsia_math::wire::RectU;
using WireSizeU = fuchsia_math::wire::SizeU;
using WireVsyncAckCookie = fuchsia_hardware_display::wire::VsyncAckCookie;

}  // namespace display

#endif  // SRC_UI_SCENIC_LIB_DISPLAY_FIDL_TYPEDEFS_H_
