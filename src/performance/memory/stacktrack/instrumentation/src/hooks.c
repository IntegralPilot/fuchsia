// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/sanitizer.h>
#include <zircon/syscalls.h>

// Updates the stack trace for the current thread, if deeper than the previous recorded one.
//
// Implemented in lib.rs.
void stacktrack_update_current_thread(void);

// Tears down the current thread's state in stacktrack.
// This should be called when a thread exits to clean up resources in the VMO.
//
// Implemented in lib.rs.
void stacktrack_remove_current_thread(void);

#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Winvalid-noreturn"

#define _ZX_SYSCALL_ANNO(attr)
#define _ZX_SYSCALL_DECL(name, type, attrs, nargs, arglist, prototype) \
  __EXPORT type zx_##name prototype {                                  \
    stacktrack_update_current_thread();                                \
    return _zx_##name arglist;                                         \
  }

#include <zircon/syscalls/gen/cdecls.inc>
#undef _ZX_SYSCALL_ANNO
#undef _ZX_SYSCALL_DECL

#pragma GCC diagnostic pop

__EXPORT
void __sanitizer_thread_exit_hook(void* hook, thrd_t self) { stacktrack_remove_current_thread(); }
