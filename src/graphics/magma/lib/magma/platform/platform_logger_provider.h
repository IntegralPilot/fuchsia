// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_PLATFORM_PLATFORM_LOGGER_PROVIDER_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_PLATFORM_PLATFORM_LOGGER_PROVIDER_H_

#include <lib/magma/platform/platform_handle.h>
namespace magma {

class PlatformLoggerProvider {
 public:
  static bool Initialize(std::unique_ptr<PlatformHandle> channel);
  static bool IsInitialized();
};

}  // namespace magma
#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_PLATFORM_PLATFORM_LOGGER_PROVIDER_H_
