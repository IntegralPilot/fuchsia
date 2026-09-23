// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ZBITL_INCLUDE_LIB_ZBITL_ITEMS_GRAPHICS_H_
#define SRC_LIB_ZBITL_INCLUDE_LIB_ZBITL_ITEMS_GRAPHICS_H_

#include <lib/zbi-format/graphics.h>

#include <cstdint>

namespace zbitl {

// Returns the number of bytes per pixel for a given format.
constexpr uint32_t BytesPerPixel(zbi_pixel_format_t format) { return (format >> 16) & 7; }

}  // namespace zbitl

#endif  // SRC_LIB_ZBITL_INCLUDE_LIB_ZBITL_ITEMS_GRAPHICS_H_
