// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_PLATFORM_ZIRCON_ZIRCON_PLATFORM_EVENT_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_PLATFORM_ZIRCON_ZIRCON_PLATFORM_EVENT_H_

#include <lib/magma/platform/platform_event.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma/util/utils.h>
#include <lib/zx/event.h>
#include <lib/zx/time.h>

namespace magma {

class ZirconPlatformEvent : public PlatformEvent {
 public:
  ZirconPlatformEvent(zx::event event) : zx_event_(std::move(event)) {}

  void Signal() override {
    zx_status_t status = zx_event_.signal(0u, GetZxSignal());
    DASSERT(status == ZX_OK);
  }

  magma::Status Wait(uint64_t timeout_ms) override {
    zx_status_t status = zx_event_.wait_one(
        GetZxSignal(), zx::deadline_after(zx::duration(magma::ms_to_signed_ns(timeout_ms))),
        nullptr);
    switch (status) {
      case ZX_OK:
        return MAGMA_STATUS_OK;
      case ZX_ERR_TIMED_OUT:
        return MAGMA_STATUS_TIMED_OUT;
      case ZX_ERR_CANCELED:
        return MAGMA_STATUS_CONNECTION_LOST;
      default:
        return DRET_MSG(MAGMA_STATUS_INTERNAL_ERROR, "Unexpected wait() status: %d.", status);
    }
  }

  static zx_signals_t GetZxSignal() { return ZX_EVENT_SIGNALED; }

 private:
  zx::event zx_event_;
};

}  // namespace magma

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_PLATFORM_ZIRCON_ZIRCON_PLATFORM_EVENT_H_
