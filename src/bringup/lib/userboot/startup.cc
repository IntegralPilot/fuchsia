// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/userboot/startup.h"

#include <lib/zx/channel.h>
#include <lib/zx/time.h>
#include <lib/zx/vmar.h>
#include <zircon/assert.h>
#include <zircon/startup.h>
#include <zircon/status.h>
#include <zircon/syscalls/object.h>

#include <algorithm>
#include <array>
#include <cinttypes>
#include <span>
#include <utility>

namespace {

zx::channel gBootstrapChannel;

}  // namespace

zx_handle_t TakeBootstrapChannel() { return gBootstrapChannel.release(); }

zx_startup_handles_t _zx_startup_get_handles(zx_handle_t process_start_handle) {
  gBootstrapChannel.reset(process_start_handle);

  zx_signals_t pending;
  zx_status_t status =
      gBootstrapChannel.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), &pending);
  ZX_ASSERT_MSG(status == ZX_OK, "bootstrap channel wait: %s", zx_status_get_string(status));
  ZX_DEBUG_ASSERT(pending & ZX_CHANNEL_READABLE);

  std::array<zx_handle_info_t, ZX_CHANNEL_MAX_MSG_HANDLES> handle_buffer;
  uint32_t actual_handles = 0;
  status = gBootstrapChannel.read_etc(0, nullptr, handle_buffer.data(), 0, handle_buffer.size(),
                                      nullptr, &actual_handles);
  ZX_ASSERT_MSG(status == ZX_OK, "bootstrap channel read: %s", zx_status_get_string(status));
  std::span handles = std::span{handle_buffer}.subspan(0, actual_handles);

  zx_startup_handles_t startup = {};

  // For most things, there should only be one handle of that object type.
  constexpr auto take = [](zx_handle_t handle, zx_handle_t& startup_handle, const char* what) {
    if (startup_handle != ZX_HANDLE_INVALID) {
      ZX_PANIC("multiple %s handles in first bootstrap message", what);
    }
    startup_handle = handle;
  };

  // For VMARs there are two, and we don't presume about their order.
  auto take_vmar = [&startup, first_vmar = zx::vmar{}](zx_handle_t handle) mutable {
    if (startup.allocation_vmar != ZX_HANDLE_INVALID) {
      ZX_PANIC("more than two VMAR handles in first bootstrap message");
    }

    if (!first_vmar) {
      first_vmar.reset(handle);
      return;
    }
    zx::vmar second_vmar{handle};

    // Find the bounds of each VMAR to see which is which.
    auto get_vmar_info = [](const zx::vmar& vmar, zx_info_vmar_t& info) {
      zx_status_t status = vmar.get_info(ZX_INFO_VMAR, &info, sizeof(info), nullptr, nullptr);
      ZX_ASSERT_MSG(status == ZX_OK, "ZX_INFO_VMAR on VMAR from first bootstrap message: %s",
                    zx_status_get_string(status));
    };
    zx_info_vmar_t first, second;
    get_vmar_info(first_vmar, first);
    get_vmar_info(second_vmar, second);

    // The root VMAR will wholly contain the ELF image VMAR.
    if (first.len < second.len) {
      std::swap(first_vmar, second_vmar);
      std::swap(first, second);
    }
    ZX_ASSERT_MSG(first.len > second.len,
                  "{base=%#" PRIx64 ", len=%zx} vs {base=%#" PRIx64 ", len=%zx}", first.base,
                  first.len, second.base, second.len);
    ZX_ASSERT_MSG(first.base <= second.base,
                  "{base=%#" PRIx64 ", len=%zx} vs {base=%#" PRIx64 ", len=%zx}", first.base,
                  first.len, second.base, second.len);
    ZX_ASSERT_MSG(second.base - first.base < first.len,
                  "{base=%#" PRIx64 ", len=%zx} vs {base=%#" PRIx64 ", len=%zx}", first.base,
                  first.len, second.base, second.len);
    ZX_ASSERT_MSG(second.len <= first.len - (second.base - first.base),
                  "{base=%#" PRIx64 ", len=%zx} vs {base=%#" PRIx64 ", len=%zx}", first.base,
                  first.len, second.base, second.len);

    startup.allocation_vmar = first_vmar.release();
    startup.executable_image_vmar = second_vmar.release();
  };

  for (const auto& handle : handles) {
    switch (handle.type) {
      case ZX_OBJ_TYPE_DEBUGLOG:
        take(handle.handle, startup.log, "debuglog");
        break;
      case ZX_OBJ_TYPE_SOCKET:
        take(handle.handle, startup.log, "logging socket");
        break;
      case ZX_OBJ_TYPE_PROCESS:
        take(handle.handle, startup.process_self, "process");
        break;
      case ZX_OBJ_TYPE_THREAD:
        take(handle.handle, startup.thread_self, "thread");
        break;
      case ZX_OBJ_TYPE_VMAR:
        take_vmar(handle.handle);
        break;
      default:
        ZX_PANIC("unexpected handle type %u in bootstrap channel first message", handle.type);
    }
  }

  return startup;
}

zx_startup_arguments_t _zx_startup_get_arguments(void* hook) {
  // No arguments or environment provided for main.
  return {};
}

void _zx_startup_preinit(void* hook) {
  // No other initialization work to do.  The UTC clock is not available.
}
