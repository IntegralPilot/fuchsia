// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_SYSMEM_SERVICE_PROVIDER_H_
#define SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_SYSMEM_SERVICE_PROVIDER_H_

#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/zx/result.h>

namespace fake_display {

// Interface for a component that exposes Sysmem's Allocator protocol.
class SysmemServiceProvider {
 public:
  SysmemServiceProvider() = default;
  virtual ~SysmemServiceProvider() = default;

  SysmemServiceProvider(const SysmemServiceProvider&) = delete;
  SysmemServiceProvider& operator=(const SysmemServiceProvider&) = delete;
  SysmemServiceProvider(SysmemServiceProvider&&) = delete;
  SysmemServiceProvider& operator=(SysmemServiceProvider&&) = delete;

  virtual zx::result<fidl::ClientEnd<fuchsia_sysmem2::Allocator>> ConnectAllocator2() = 0;
};

}  // namespace fake_display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_SYSMEM_SERVICE_PROVIDER_H_
