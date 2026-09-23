// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_VSYNC_ACK_COOKIE_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_VSYNC_ACK_COOKIE_H_

#include <fidl/fuchsia.hardware.display/cpp/wire.h>

#include <cstdint>

#include "src/graphics/display/lib/api-types/cpp/id-type.h"

namespace display::internal {

using VsyncAckCookieTraits =
    DefaultIdTypeTraits<uint64_t, fuchsia_hardware_display::wire::VsyncAckCookie>;

}  // namespace display::internal

namespace display {

// More useful representation of `fuchsia.hardware.display/VsyncAckCookie`.
using VsyncAckCookie = display::internal::IdType<display::internal::VsyncAckCookieTraits>;

constexpr VsyncAckCookie kInvalidVsyncAckCookie(
    fuchsia_hardware_display_types::wire::kInvalidDispId);

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_VSYNC_ACK_COOKIE_H_
