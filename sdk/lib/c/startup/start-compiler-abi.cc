// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/channel.h>
#include <lib/zx/process.h>
#include <lib/zx/thread.h>
#include <lib/zx/vmar.h>
#include <zircon/assert.h>
#include <zircon/startup.h>
#include <zircon/status.h>

#include <atomic>
#include <string_view>
#include <tuple>
#include <utility>

#include "../ld/log.h"
#include "../threads/stack-abi.h"
#include "../threads/thread-storage.h"
#include "../threads/zxr-tls.h"
#include "start-main.h"
#include "startup-random.h"
#include "startup-relocate.h"
#include "startup-trampoline.h"
#include "threads_impl.h"

namespace LIBC_NAMESPACE_DECL {
namespace {

auto GetStartupHandles(zx::handle bootstrap) {
  auto [process_self, thread_self, allocation_vmar, image_vmar, log, hook] =
      _zx_startup_get_handles(bootstrap.release());
  return std::make_tuple(zx::process{process_self}, zx::thread{thread_self},
                         zx::vmar{allocation_vmar}, zx::vmar{image_vmar}, zx::handle{log}, hook);
}

}  // namespace

// This is responsible for allocating stacks and the thread area.  When it
// returns, the thread pointer and the shadow-call-stack and unsafe-stack
// pointers have been initialized, along with the stack guard word.  It returns
// the new machine stack pointer.  When the trampoline switches to that, the
// Fuchsia Compiler ABI is ready.
StartupTrampoline StartCompilerAbi(zx_handle_t bootstrap, const void* vdso_base) {
  const StartupRelocate reloc = StartupRelocate::Create(vdso_base);

  // This takes ownership of the bootstrap handle.
  // Take ownership of all the handles it returned.
  auto [process_self, thread_self, allocation_vmar, image_vmar, log, hook] =
      GetStartupHandles(zx::handle{bootstrap});
  ZX_DEBUG_ASSERT(allocation_vmar);

  if (log) {
    // Enable logging immediately if a place to do it was provided.
    gLog.TakeLogHandle(std::move(log));
  }

  // If it yielded the VMAR for the executable image, use it to protect RELRO
  // immediately.  This will also drop the handle so RELRO pages cannot be made
  // writable again.
  if (image_vmar) {
    std::move(reloc).ProtectRelro(std::move(image_vmar));
  }

  std::array<char, ZX_MAX_NAME_LEN> name_property;
  if (zx_status_t status =
          thread_self.get_property(ZX_PROP_NAME, &name_property, sizeof(name_property));
      status != ZX_OK) [[unlikely]] {
    ZX_PANIC("zx_object_get_property(ZX_PROP_NAME) on initial thread handle: %s",
             zx_status_get_string(status));
  }
  std::string_view thread_name{name_property.data(), name_property.size()};
  thread_name = thread_name.substr(0, thread_name.find_first_of('\0'));

  const PageRoundedSize default_guard_size = PageRoundedSize::Page();
  const PageRoundedSize stack_size = InitialStackSize();
  ZX_DEBUG_ASSERT(stack_size);
  ThreadStorage storage;
  zx::result<Thread*> new_thread = storage.Allocate(
      {
          .machine_stack_vmar = allocation_vmar.get(),
          .security_stack_vmar = allocation_vmar.get(),
          .thread_block_vmar = allocation_vmar.get(),
      },
      thread_name, stack_size, default_guard_size);
  if (new_thread.is_error()) [[unlikely]] {
    ZX_PANIC(
        "cannot allocate initial thread stacks (%#zx bytes + %#zx guard)"
        " and TCB: %s",
        stack_size.get(), default_guard_size.get(), new_thread.status_string());
  }

  uint64_t* const sp = storage.machine_sp();
  if constexpr (kShadowCallStackAbi) {
    // Install the initial shadow-call-stack pointer in its register.
    ShadowCallStackSet(storage.shadow_call_sp());
  }

  // Transfer ownership of the mappings to the new TCB.  Further initialization
  // of the TCB beyond this and the Fuchsia Compiler (and TLS) ABI pieces is
  // left to be done after the transition to the full ABI.
  std::move(storage).ToThread(**new_thread);

  // Install the thread pointer.
  zxr_tp_set(thread_self.get(), pthread_to_tp(*new_thread));

  // This fills the stack-guard ABI slot, among other things.
  InitStartupRandom();

  // At this point it is actually safe to start using the full Compiler ABI,
  // even though the original machine stack is still in use.
  std::atomic_signal_fence(std::memory_order_seq_cst);

  SetStartHandles(std::move(process_self), std::move(allocation_vmar), std::move(thread_self));

  return {.hook = hook, .sp = sp};
}

}  // namespace LIBC_NAMESPACE_DECL
