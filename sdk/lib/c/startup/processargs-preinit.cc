// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ld/processargs.h>
#include <lib/zx/clock.h>
#include <zircon/assert.h>
#include <zircon/startup.h>
#include <zircon/utc.h>

#include <string_view>
#include <utility>

#include "../ld/log.h"
#include "../weak.h"
#include "processargs.h"
#include "zircon_impl.h"

namespace LIBC_NAMESPACE_DECL {
namespace {

constexpr std::string_view kSvcName = "/svc";

zx_startup_arguments_t StartupArguments(Processargs& saved) {
  return {
      .argc = saved.argc(),
      .argv = saved.argv().data(),
      .envp = saved.envp().data(),
  };
}

void TakeLegacy(zx::handle handle, zx_handle_t& legacy) {
  // Things still used by legacy C code are stored as zx_handle_t but owned.
  legacy = handle.release();
}

void TakeUtcClock(zx::clock clock) {
  zx::clock old_clock;
  _zx_utc_reference_swap(clock.release(), old_clock.reset_and_get_address());
}

// When nothing like fdio is present, steal the stderr fd for
// __sanitizer_log_write to use.
void NoExtensionsInit(uint32_t handle_count, zx_handle_t handle[], uint32_t handle_info[],
                      uint32_t name_count, char** names) {
  if (gLog) {  // Don't bother looking if it's already set up.
    return;
  }
  const std::span handles{handle, handle_count};
  const std::span info{handle_info, handle_count};
  for (auto [info, take] : Processargs::HandleTakers(info, handles)) {
    if (ld::IsProcessargsLogHandle(info)) {
      gLog.TakeLogHandle(take());
      break;
    }
  }
}

void Preinit(Processargs& saved) {
  // Collect the remaining essential handles that live in permanent libc state.
  for (auto [info, take] : Processargs::HandleTakers(saved.handle_info(), saved.handles())) {
    const uint32_t arg = PA_HND_ARG(info);
    switch (PA_HND_TYPE(info)) {
      case PA_JOB_DEFAULT:
        TakeLegacy(take(), __zircon_job_default);
        break;
      case PA_CLOCK_UTC:
        TakeUtcClock(zx::clock{take()});
        break;
      case PA_NS_DIR:
        if (__zircon_namespace_svc == ZX_HANDLE_INVALID &&  //
            arg < saved.names().size() && saved.names()[arg] == kSvcName) {
          // This just borrows the handle, leaving it in the name table to be
          // used at higher layers.  If this handle becomes invalid later,
          // there is no provision for updating it.
          __zircon_namespace_svc = take(std::in_place)->get();
        }
        break;
      default:
        // Others are not for libc to consume.
        break;
    }
  }

  // Hand over the name table and the handle/info table of unclaimed entries to
  // something like fdio (if present).  This can claim entries by clearing both
  // corresponding entries in the two parallel arrays that form that table.
  const uint32_t nhandles = static_cast<uint32_t>(saved.handles().size());
  const uint32_t namec = static_cast<uint32_t>(saved.names().size());
  Weak<__libc_extensions_init>::Fallback<NoExtensionsInit>(
      nhandles, saved.handles().data(), saved.handle_info().data(), namec, saved.names().data());

  // Give any unclaimed handles to zx_take_startup_handle().  This function
  // takes ownership of the data, but not the memory: it assumes that the
  // arrays are valid as long as the process is alive.
  __libc_startup_handles_init(nhandles, saved.handles().data(), saved.handle_info().data());
}

Processargs& Check(void* hook) {
  // This function can't be in this same file and so could be overridden when
  // this file is still linked in.  Make sure that doesn't happen.
  if (_zx_startup_get_handles != Processargs::GetHandles) [[unlikely]] {
    ZX_PANIC("All <zircon/startup.h> functions must be replaced or none!");
  }
  Processargs* saved = static_cast<Processargs*>(hook);
  assert(hook);
  return *saved;
}

}  // namespace
}  // namespace LIBC_NAMESPACE_DECL

using LIBC_NAMESPACE::Check;

zx_startup_arguments_t _zx_startup_get_arguments(void* hook) {
  return LIBC_NAMESPACE::StartupArguments(Check(hook));
}

void _zx_startup_preinit(void* hook) { LIBC_NAMESPACE::Preinit(Check(hook)); }
