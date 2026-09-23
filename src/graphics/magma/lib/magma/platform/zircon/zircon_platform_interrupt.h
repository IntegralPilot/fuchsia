// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_PLATFORM_INTERRUPT_H
#define ZIRCON_PLATFORM_INTERRUPT_H

#include <lib/magma/platform/platform_interrupt.h>
#include <lib/magma/util/short_macros.h>
#include <lib/zx/clock.h>
#include <lib/zx/handle.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/time.h>
#include <zircon/status.h>

#include <utility>

#include "zircon_platform_port.h"

namespace magma {

class ZirconPlatformInterrupt : public PlatformInterrupt {
 public:
  explicit ZirconPlatformInterrupt(zx::handle interrupt_handle)
      : handle_(zx::interrupt(std::move(interrupt_handle))) {
    DASSERT(handle_.get() != ZX_HANDLE_INVALID);
    zx_info_handle_basic_t info{};
    zx_status_t status =
        handle_.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
    DASSERT(status == ZX_OK);
    koid_ = info.koid;
  }

  uint64_t global_id() const override { return koid_; }
  void Signal() override { handle_.destroy(); }

  bool Wait() override {
    zx_status_t status = handle_.wait(&timestamp_);
    if (status != ZX_OK)
      return DRETF(false, "zx_irq_wait failed (%d)", status);
    return true;
  }

  void Complete() override {}

  void Ack() override { handle_.ack(); }

  bool Bind(PlatformPort* port, uint64_t key) override {
    auto zircon_port = static_cast<ZirconPlatformPort*>(port);

    zx_status_t status = handle_.bind(zircon_port->zx_port(), key, ZX_INTERRUPT_BIND);
    if (status != ZX_OK)
      return DRETF(false, "interrupt_bind failed: %s", zx_status_get_string(status));

    return true;
  }

  bool Unbind(PlatformPort* port) override {
    auto zircon_port = static_cast<ZirconPlatformPort*>(port);

    zx_status_t status = handle_.bind(zircon_port->zx_port(), 0, ZX_INTERRUPT_UNBIND);
    if (status != ZX_OK)
      return DRETF(false, "interrupt_bind failed: %s", zx_status_get_string(status));

    return true;
  }

  uint64_t GetMicrosecondsSinceLastInterrupt() override {
    return (zx::clock::get_boot() - timestamp_).to_usecs();
  }

 private:
  zx::interrupt handle_;
  uint64_t koid_;
  zx::time_boot timestamp_;
};

}  // namespace magma

#endif  // ZIRCON_PLATFORM_INTERRUPT_H
