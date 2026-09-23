// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/maybe-standalone-test/maybe-standalone.h>
#include <lib/standalone-test/standalone.h>
#include <lib/zx/channel.h>
#include <lib/zx/resource.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>

#include <string_view>

// Redeclare the standalone-test function as weak here.
[[gnu::weak]] decltype(standalone::GetIoportResource) standalone::GetIoportResource;
[[gnu::weak]] decltype(standalone::GetIrqResource) standalone::GetIrqResource;
[[gnu::weak]] decltype(standalone::GetMmioResource) standalone::GetMmioResource;
[[gnu::weak]] decltype(standalone::GetSystemResource) standalone::GetSystemResource;
[[gnu::weak]] decltype(standalone::GetVmo) standalone::GetVmo;
[[gnu::weak]] decltype(standalone::GetNsDir) standalone::GetNsDir;

namespace maybe_standalone {

zx::unowned_resource GetIoportResource() {
  zx::unowned_resource ioport_resource;
  if (standalone::GetIoportResource) {
    ioport_resource = standalone::GetIoportResource();
  }
  return ioport_resource;
}

zx::unowned_resource GetIrqResource() {
  zx::unowned_resource irq_resource;
  if (standalone::GetIrqResource) {
    irq_resource = standalone::GetIrqResource();
  }
  return irq_resource;
}

zx::unowned_resource GetMmioResource() {
  zx::unowned_resource mmio_resource;
  if (standalone::GetMmioResource) {
    mmio_resource = standalone::GetMmioResource();
  }
  return mmio_resource;
}

zx::unowned_resource GetSystemResource() {
  zx::unowned_resource system_resource;
  if (standalone::GetSystemResource) {
    system_resource = standalone::GetSystemResource();
  }
  return system_resource;
}

zx::unowned_vmo GetVmo(std::string_view name) {
  zx::unowned_vmo vmo;
  if (standalone::GetVmo) {
    vmo = standalone::GetVmo(name);
  }
  return vmo;
}

zx::unowned_channel GetNsDir(std::string_view name) {
  zx::unowned_channel channel;
  if (standalone::GetNsDir) {
    channel = standalone::GetNsDir(name);
  }
  return channel;
}

zx::result<zx::resource> GetSystemResourceWithBase(zx::unowned_resource& system_resource,
                                                   uint64_t base) {
  zx::resource new_resource;
  const zx_status_t status = zx::resource::create(*system_resource, ZX_RSRC_KIND_SYSTEM, base, 1,
                                                  nullptr, 0, &new_resource);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(new_resource));
}

}  // namespace maybe_standalone
