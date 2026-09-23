// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "start-main.h"

#include <lib/ld/fuchsia-debugdata.h>
#include <lib/zx/channel.h>
#include <zircon/sanitizer.h>
#include <zircon/startup.h>

#include <cassert>
#include <iterator>
#include <mutex>

#include "../threads/thread-list.h"
#include "../threads/thread-storage.h"
#include "asan_impl.h"
#include "src/stdlib/exit.h"
#include "threads_impl.h"
#include "zircon_impl.h"

namespace LIBC_NAMESPACE_DECL {
namespace {

void InitThreadList() {
  // Initialize the list of all threads.  Locking isn't really required since
  // there are no other threads yet, but it's harmlessly cheap and meets the
  // -Wthread-safety requirements.
  std::lock_guard lock(gAllThreadsLock);
  assert(gAllThreads == nullptr);
  pthread* self = __pthread_self();
  self->prevp = &gAllThreads;
  gAllThreads = self;
}

// If there was a deferred channel of /svc messages, forward that now.  If it
// has nowhere to go, drop it and its messages (and owned VMOs) now.
void ForwardSvc(zx::channel deferred) {
  zx::unowned_channel svc{__zircon_namespace_svc};
  if (deferred && *svc) {
    std::ignore = ld::Debugdata::Forward(svc->borrow(), std::move(deferred));
  }
}

// Let the sanitizer runtime initialize itself before constructors.
void SanitizerStartup(const zx_startup_arguments_t& args) {
  StartupSanitizerModuleLoaded();

  const std::span stack = ThreadStorage::ThreadMachineStack(*__pthread_self());
  __sanitizer_startup_hook(args.argc, args.argv, args.envp, stack.data(), stack.size_bytes());
}

void BeforeCtors(zx::channel deferred, zx_startup_arguments_t args) {
#if __has_feature(hwaddress_sanitizer)
  // This code itself is instrumented and access to any normal global variable
  // will compare the variable's tag bits to the shadow--which hasn't been set
  // up yet.  So the hwasan runtime must be initialized before even this code
  // does any such access.  The runtime's own initialization code will call
  // back into libc, but not for anything where the pre-ctor initialization not
  // yet completed here will matter.  Eventually StartupCtors() will indirectly
  // call __hwasan_init() again (which just harmlessly returns quickly since
  // it's already been called here).  But that would be too late.
  //
  // The hwasan runtime expects to have already received the callbacks to
  // __sanitizer_module_loaded and __sanitizer_startup_hook before its
  // constructor runs.  We know that __hwasan_init() doesn't interact with
  // other libc facilities that depend on the rest of libc initialization.
  // However, in the general case, the __sanitizer_* callbacks should only
  // be made after libc is fully initialized and entirely safe to use.
  SanitizerStartup(args);
  __hwasan_init();
#endif

  __environ = args.envp;

  InitThreadList();
  atomic_store(&libc.thread_count, 1);

  ForwardSvc(std::move(deferred));

  // Finish allocator setup.  Hereafter code outside libc proper can run and
  // might use most normal libc facilities, even though static constructors
  // haven't run yet.
  __libc_init_gwp_asan();

#if !__has_feature(hwaddress_sanitizer)
  SanitizerStartup(args);
#endif
}

// This does all the work of the final __libc_start_main before calling main.
// It's in a separate function so that main's direct caller will always be just
// __libc_start_main, but other calls won't be under the umbrella of the
// no_sanitizer attribute that is necessary to call main.
zx_startup_arguments_t PreMain(void* hook, zx_handle_t svc_server_end) {
  zx::channel deferred_svc{svc_server_end};

  // Now finish core libc initialization.
  zx_startup_arguments_t args = _zx_startup_get_arguments(hook);
  if (args.argc == 0) {
    static char* empty_argv[] = {nullptr};
    args.argv = empty_argv;
  } else {
    ZX_DEBUG_ASSERT(args.argv);
    ZX_DEBUG_ASSERT(!args.argv[args.argc]);
  }
  if (!args.envp) {
    static char* empty_envp[] = {nullptr};
    args.envp = empty_envp;
  } else {
    ZX_DEBUG_ASSERT(([](char** ep) {
                      int n = 0;
                      while (*ep++) {
                        ++n;
                      }
                      return n;
                    }(args.envp)) >= 0);
  }

  BeforeCtors(std::move(deferred_svc), args);

  // Do any final initialization that's contingent on the bootstrap protocol.
  _zx_startup_preinit(hook);

  // Finally run user constructors and then main.
  StartupCtors();

  return args;
}

}  // namespace

// This is called on the original machine stack, but outside the basic ABI
// bubble.  That makes it safe to use globals that might be instrumented.
void SetStartHandles(zx::process process_self, zx::vmar allocation_vmar, zx::thread thread_self) {
  __zircon_process_self = process_self.release();
  __zircon_vmar_root_self = allocation_vmar.release();

  // Initialize the zxr_thread first, taking ownership of the thread handle.
  // The locking code uses _zx_thread_self(), which fetches the handle stored
  // here.  So this is the bare minimum that must be done before more normal
  // operation, even taking locks, is possible.
  pthread* self = __pthread_self();
  self->handle_ = thread_self.release();
  // The zero-initialized value is already JOINABLE, so nothing to do there.
  static_assert(static_cast<int>(Thread::Lifecycle::JOINABLE) == 0);
  assert(self->lifecycle_ == Thread::Lifecycle::JOINABLE);

  // Initialize the rest of the struct pthread now, since it's simple.
  self->locale = &libc.global_locale;

  // These handles are used to create new threads.  They can be changed by
  // thrd_set_zx_create_handles() or thrd_set_zx_process(), and are inherited
  // in new threads as they are created.
  self->create_handles = {
      .process = __zircon_process_self,
      .machine_stack_vmar = __zircon_vmar_root_self,
      .security_stack_vmar = __zircon_vmar_root_self,
      .thread_block_vmar = __zircon_vmar_root_self,
  };

  // The same VMAR handles are kept separately for unmapping this initial
  // thread's ThreadStorage blocks.  These don't change.
  self->storage_handles = self->create_handles;
}

// This is called on the proper final machine stack.  The MainFunction type
// includes the attribute saying that the actual signature of the callee need
// not exactly match the type used here, since any of the three signatures is
// equally valid in the user's definition.
void __libc_start_main(  //
    void* hook, zx_handle_t svc_server_end, MainFunction* main) {
  zx_startup_arguments_t args = PreMain(hook, svc_server_end);
  exit((*main)(args.argc, args.argv, __environ));
}

}  // namespace LIBC_NAMESPACE_DECL
