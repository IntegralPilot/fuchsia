// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_TESTING_FAKE_FRAMEBUFFER_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_TESTING_FAKE_FRAMEBUFFER_H_

#include <fidl/fuchsia.boot/cpp/fidl.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/zbi-format/graphics.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

namespace fake_framebuffer {

struct Framebuffer {
  zx_status_t status = ZX_OK;
  uint32_t format = 0u;
  uint32_t width = 0u;
  uint32_t height = 0u;
  uint32_t stride = 0u;
};

class FakeBootItems final : public fidl::WireServer<fuchsia_boot::Items> {
 public:
  FakeBootItems() = default;
  ~FakeBootItems() final = default;
  FakeBootItems(const FakeBootItems&) = delete;
  FakeBootItems& operator=(const FakeBootItems&) = delete;

  void SetFramebuffer(const Framebuffer& buffer);
  void Serve(async_dispatcher_t* dispatcher, fidl::ServerEnd<fuchsia_boot::Items> server_end);
  fidl::ProtocolHandler<fuchsia_boot::Items> CreateHandler(async_dispatcher_t* dispatcher) {
    return bindings_.CreateHandler(this, dispatcher, fidl::kIgnoreBindingClosure);
  }

  void Get(GetRequestView request, GetCompleter::Sync& completer) override;
  void Get2(Get2RequestView request, Get2Completer::Sync& completer) override;
  void GetBootloaderFile(GetBootloaderFileRequestView request,
                         GetBootloaderFileCompleter::Sync& completer) override;

 private:
  zx_status_t status_ = ZX_OK;
  zbi_swfb_t framebuffer_ = {};
  fidl::ServerBindingGroup<fuchsia_boot::Items> bindings_;
};

}  // namespace fake_framebuffer

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_TESTING_FAKE_FRAMEBUFFER_H_
