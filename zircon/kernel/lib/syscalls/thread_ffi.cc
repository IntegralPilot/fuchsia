// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/page-map.h>
#include <zircon/syscalls/rseq.h>
#include <zircon/types.h>

#include <kernel/ffi.h>
#include <kernel/thread.h>
#include <object/vm_object_dispatcher.h>
#include <vm/vm_object_paged.h>

#include "thread_priv.h"

extern "C" {

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_thread_reset_rseq() { Thread::Current::Get()->set_rseq_accessor({}); }

zx_status_t cpp_thread_set_rseq(const VmObjectDispatcher* vmo_dispatcher, uint64_t offset) {
  // Get the VMO.
  fbl::RefPtr<VmObject> vmo = vmo_dispatcher->vmo();
  if (vmo->is_user_pager_backed()) {
    return ZX_ERR_INVALID_ARGS;
  }
  fbl::RefPtr<VmObjectPaged> vmo_paged = DownCastVmObject<VmObjectPaged>(ktl::move(vmo));
  if (!vmo_paged) {
    return ZX_ERR_INVALID_ARGS;
  }

  // Create the accessor.
  zx::result<page_map::Accessor<zx_rseq_t>> accessor =
      page_map::PageMap::Get().MakeAccessor<zx_rseq_t>(ktl::move(vmo_paged), offset);
  if (accessor.is_error()) {
    return accessor.error_value();
  }

  Thread* const current_thread = Thread::Current::Get();
  current_thread->set_rseq_accessor(ktl::move(*accessor));

  // Set the thread signal to ensure we write out the cpu_id field before returning to user mode.
  current_thread->SignalCheckRestartableSequenceIfNeeded();

  return ZX_OK;
}

}  // extern "C"
