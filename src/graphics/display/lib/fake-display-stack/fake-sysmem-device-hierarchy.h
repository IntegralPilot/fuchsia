// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_FAKE_SYSMEM_DEVICE_HIERARCHY_H_
#define SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_FAKE_SYSMEM_DEVICE_HIERARCHY_H_

#include <lib/async-loop/cpp/loop.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/internal/driver_lifecycle.h>
#include <lib/driver/testing/cpp/internal/test_environment.h>
#include <lib/driver/testing/cpp/test_node.h>

#include <memory>

#include "src/graphics/display/lib/fake-display-stack/sysmem-service-provider.h"
#include "src/sysmem/server/sysmem.h"

namespace fake_display {

// WARNING: Don't use this code as a template for new tests as it uses the old driver testing
// library.

// Manages a self-contained Sysmem service instance.
//
// Tests that create this class do not need Sysmem capabilities routed to them.
class FakeSysmemDeviceHierarchy : public SysmemServiceProvider {
 public:
  static zx::result<std::unique_ptr<FakeSysmemDeviceHierarchy>> Create();

  FakeSysmemDeviceHierarchy();
  ~FakeSysmemDeviceHierarchy() override;

  FakeSysmemDeviceHierarchy(const FakeSysmemDeviceHierarchy&) = delete;
  FakeSysmemDeviceHierarchy& operator=(const FakeSysmemDeviceHierarchy&) = delete;
  FakeSysmemDeviceHierarchy(FakeSysmemDeviceHierarchy&&) = delete;
  FakeSysmemDeviceHierarchy& operator=(FakeSysmemDeviceHierarchy&&) = delete;

  // Initialization logic not suitable for the constructor.
  zx::result<> Initialize();

  // SysmemServiceProvider:
  zx::result<fidl::ClientEnd<fuchsia_sysmem2::Allocator>> ConnectAllocator2() override;

 private:
  async::Loop sysmem_client_loop_;

  // Set in the constructor, reset in the destructor.
  std::unique_ptr<sysmem_service::Sysmem> sysmem_service_;
};

}  // namespace fake_display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_FAKE_SYSMEM_DEVICE_HIERARCHY_H_
