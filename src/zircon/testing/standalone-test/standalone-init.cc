// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/lazy_init/lazy_init.h>
#include <lib/standalone-test/standalone.h>
#include <lib/zx/channel.h>
#include <lib/zx/resource.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>
#include <zircon/processargs.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/log.h>
#include <zircon/types.h>

#include <map>
#include <string>
#include <string_view>

namespace standalone {
namespace {

zx::resource ioport_resource, irq_resource, mmio_resource, system_resource;

constexpr std::string_view kStartupMessage =
    "*** Running standalone test directly from userboot ***\n";

lazy_init::LazyInit<std::map<std::string, zx::vmo, std::less<>>> gVmos;
lazy_init::LazyInit<std::map<std::string, zx::channel, std::less<>>> gNsDir;

}  // namespace

zx::unowned_vmo GetVmo(std::string_view name) {
  auto it = gVmos->find(name);
  if (it == gVmos->end()) {
    return {};
  }
  return it->second.borrow();
}

zx::unowned_channel GetNsDir(std::string_view name) {
  auto it = gNsDir->find(name);
  if (it == gNsDir->end()) {
    return {};
  }

  return it->second.borrow();
}

zx::unowned_resource GetIoportResource() {
  ZX_ASSERT_MSG(ioport_resource, "standalone test didn't receive Ioport resource");
  return ioport_resource.borrow();
}

zx::unowned_resource GetIrqResource() {
  ZX_ASSERT_MSG(irq_resource, "standalone test didn't receive IRQ resource");
  return irq_resource.borrow();
}

zx::unowned_resource GetMmioResource() {
  ZX_ASSERT_MSG(mmio_resource, "standalone test didn't receive MMIO resource");
  return mmio_resource.borrow();
}

zx::unowned_resource GetSystemResource() {
  ZX_ASSERT_MSG(system_resource, "standalone test didn't receive system  resource");
  return system_resource.borrow();
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

// This overrides a weak definition in libc, replacing the hook that's
// ordinarily defined by fdio.  The retain attribute makes sure that linker
// doesn't decide it can elide this definition and let libc use its weak one.
extern "C" [[gnu::retain]] __EXPORT void __libc_extensions_init(uint32_t count,
                                                                zx_handle_t handle[],
                                                                uint32_t info[],
                                                                uint32_t name_count, char** names) {
  gVmos.Initialize();
  gNsDir.Initialize();

  auto take_handle = [&handle, &info](size_t index) {
    info[index] = 0;
    return std::exchange(handle[index], ZX_HANDLE_INVALID);
  };

  for (unsigned n = 0; n < count; n++) {
    switch (PA_HND_TYPE(info[n])) {
      case PA_IOPORT_RESOURCE:
        ioport_resource.reset(take_handle(n));
        break;

      case PA_IRQ_RESOURCE:
        irq_resource.reset(take_handle(n));
        break;

      case PA_MMIO_RESOURCE:
        mmio_resource.reset(take_handle(n));
        break;

      case PA_SYSTEM_RESOURCE:
        system_resource.reset(take_handle(n));
        break;

      case PA_NS_DIR: {
        uint32_t name_index = PA_HND_ARG(info[n]);
        if (name_index >= name_count) {
          continue;
        }
        gNsDir->try_emplace(std::string(names[name_index]), take_handle(n));
        break;
      }

      case PA_VMO_BOOTDATA:
      case PA_VMO_BOOTFS:
      case PA_VMO_KERNEL_FILE: {
        // Store it in the map by VMO name for StandaloneGetVmo to find later.
        char vmo_name[ZX_MAX_NAME_LEN];
        zx_status_t status =
            zx_object_get_property(handle[n], ZX_PROP_NAME, vmo_name, sizeof(vmo_name));
        if (status == ZX_OK) {
          zx::vmo vmo{take_handle(n)};
          std::string_view prop(vmo_name, sizeof(vmo_name));
          std::string name(prop.substr(0, prop.find_first_of('\0')));
          gVmos->try_emplace(std::move(name), std::move(vmo));
        }
        break;
      }
    }
  }

  // Eagerly write a message. This ensures that every standalone test links
  // in the standalone-io code that overrides functions like write from libc.
  LogWrite(kStartupMessage);
}

}  // namespace standalone
