// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_SYSCALLS_THREAD_PRIV_H_
#define ZIRCON_KERNEL_LIB_SYSCALLS_THREAD_PRIV_H_

#include <stdint.h>
#include <zircon/types.h>

#include <kernel/ffi.h>

class VmObjectDispatcher;

extern "C" {
// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_thread_reset_rseq();
zx_status_t cpp_thread_set_rseq(const VmObjectDispatcher* vmo_dispatcher, uint64_t offset);
}

#endif  // ZIRCON_KERNEL_LIB_SYSCALLS_THREAD_PRIV_H_
