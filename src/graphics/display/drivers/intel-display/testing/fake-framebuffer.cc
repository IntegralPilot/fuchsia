// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/testing/fake-framebuffer.h"

#include <lib/zbi-format/graphics.h>
#include <lib/zbi-format/zbi.h>
#include <zircon/compiler.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <cstdint>

namespace fake_framebuffer {

namespace {
zx::result<std::pair<zx::vmo, uint32_t>> GetBootItem(const zbi_swfb_t& framebuffer_info,
                                                     uint32_t type) {
  zx::vmo vmo;
  uint32_t length;
  switch (type) {
    case ZBI_TYPE_FRAMEBUFFER: {
      zx_status_t status = zx::vmo::create(sizeof(framebuffer_info), 0, &vmo);
      if (status != ZX_OK) {
        return zx::error(status);
      }
      status = vmo.write(&framebuffer_info, 0, sizeof(framebuffer_info));
      if (status != ZX_OK) {
        return zx::error(status);
      }
      length = sizeof(framebuffer_info);
      break;
    }
    default:
      return zx::error(ZX_ERR_NOT_FOUND);
  }
  return zx::ok(std::make_pair(std::move(vmo), length));
}
}  // namespace

void FakeBootItems::Serve(async_dispatcher_t* dispatcher,
                          fidl::ServerEnd<fuchsia_boot::Items> server_end) {
  bindings_.AddBinding(dispatcher, std::move(server_end), this, fidl::kIgnoreBindingClosure);
}

void FakeBootItems::Get(GetRequestView request, GetCompleter::Sync& completer) {
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

void FakeBootItems::Get2(Get2RequestView request, Get2Completer::Sync& completer) {
  if (status_ != ZX_OK) {
    completer.Reply(zx::error(status_));
    return;
  }
  zx::result boot_item = GetBootItem(framebuffer_, request->type);
  if (boot_item.is_error()) {
    completer.Reply(boot_item.take_error());
    return;
  }
  auto& [vmo, length] = boot_item.value();
  std::vector<fuchsia_boot::wire::RetrievedItems> result;
  fuchsia_boot::wire::RetrievedItems items = {
      .payload = std::move(vmo), .length = length, .extra = 0};
  result.emplace_back(std::move(items));
  completer.ReplySuccess(
      fidl::VectorView<fuchsia_boot::wire::RetrievedItems>::FromExternal(result));
}

void FakeBootItems::GetBootloaderFile(GetBootloaderFileRequestView request,
                                      GetBootloaderFileCompleter::Sync& completer) {
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

void FakeBootItems::SetFramebuffer(const Framebuffer& buffer) {
  status_ = buffer.status;
  framebuffer_.width = buffer.width;
  framebuffer_.height = buffer.height;
  framebuffer_.stride = buffer.stride;
  framebuffer_.format = buffer.format;
}

}  // namespace fake_framebuffer
