// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_SYSMEM_SERVICE_FORWARDER_H_
#define SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_SYSMEM_SERVICE_FORWARDER_H_

#include <fidl/fuchsia.io/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/zx/result.h>

#include "src/graphics/display/lib/fake-display-stack/sysmem-service-provider.h"

namespace fake_display {

// Provider that uses the component's incoming service directory.
class SysmemServiceForwarder : public SysmemServiceProvider {
 public:
  // Factory method for production use.
  static zx::result<std::unique_ptr<SysmemServiceForwarder>> Create();

  // Production code must use the `Create()` factory method.
  SysmemServiceForwarder();
  ~SysmemServiceForwarder() override;

  SysmemServiceForwarder(const SysmemServiceForwarder&) = delete;
  SysmemServiceForwarder& operator=(const SysmemServiceForwarder&) = delete;
  SysmemServiceForwarder(SysmemServiceForwarder&&) = delete;
  SysmemServiceForwarder& operator=(SysmemServiceForwarder&&) = delete;

  // Initialization logic not suitable for the constructor.
  zx::result<> Initialize();

  // SysmemServiceProvider:
  zx::result<fidl::ClientEnd<fuchsia_sysmem2::Allocator>> ConnectAllocator2() override;

 private:
  fidl::ClientEnd<fuchsia_io::Directory> component_incoming_root_;
};

}  // namespace fake_display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_SYSMEM_SERVICE_FORWARDER_H_
