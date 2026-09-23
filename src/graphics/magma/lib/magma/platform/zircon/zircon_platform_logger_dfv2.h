// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_PLATFORM_ZIRCON_ZIRCON_PLATFORM_LOGGER_DFV2_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_PLATFORM_ZIRCON_ZIRCON_PLATFORM_LOGGER_DFV2_H_

#include <lib/driver/logging/cpp/logger.h>
#include <lib/fit/defer.h>

#include <string>

namespace magma {

// Returns a deferred callback that tears down the logger on release.
fit::deferred_callback InitializePlatformLoggerForDFv2(fdf::Logger* logger, std::string tag);
}  // namespace magma

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_PLATFORM_ZIRCON_ZIRCON_PLATFORM_LOGGER_DFV2_H_
