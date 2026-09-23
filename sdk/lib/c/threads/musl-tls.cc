// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/assert.h>

#include "libc.h"
#include "thread-storage.h"
#include "threads_impl.h"

// This completes the ThreadStorage implementation when using the legacy
// integrated dynamic linker.  Its counterpart for the new dynamic linker
// passive ABI is in ../ld/ld-tls.cc.

namespace LIBC_NAMESPACE_DECL {
namespace {

constexpr ptrdiff_t OffsetForModule(const tls_module* module) {
#ifdef TLS_ABOVE_TP
  return static_cast<ptrdiff_t>(module->offset);
#else
  return -static_cast<ptrdiff_t>(module->offset);
#endif
}

}  // namespace

elfldltl::TlsLayout<> ThreadStorage::GetTlsLayout() {
  // The tls_size actually includes space for the initial DTV array too, but
  // that's off the end of where static TLS block assignments went, so it's
  // equivalent to a final TLS block of sizeof(void*[libc.tls_cnt]) and
  // InitializeTls() will expect to use it there.
  const struct tls_layout layout = _dl_tls_layout();
  return elfldltl::TlsLayout<>{layout.size - sizeof(Thread), layout.align};
}

void ThreadStorage::InitializeTls(std::span<std::byte> thread_block, size_t tp_offset) {
  // This code is moved mostly verbatim from the legacy `copy_tls` code in the
  // old implementation for both process start and thread allocation.  It's
  // only been adjusted very slightly for C++ syntax.

  unsigned char* mem = reinterpret_cast<unsigned char*>(thread_block.data());

  const struct tls_layout layout = _dl_tls_layout();

  Thread* td;
  void** dtv;

#ifdef TLS_ABOVE_TP
  // *-----------------------------------------------------------------------*
  // | pthread | tcb | X | tls_1 | ... | tlsN | ... | tls_cnt | dtv[1] | ... |
  // *-----------------------------------------------------------------------*
  // ^         ^         ^             ^            ^
  // td        tp      dtv[1]       dtv[n+1]       dtv
  //
  // Note: The TCB is actually the last member of pthread.
  // See: "Addenda to, and Errata in, the ABI for the ARM Architecture"

  dtv = reinterpret_cast<void**>(mem + layout.size) - (layout.tls_cnt + 1);
  // We need to make sure that the thread pointer is maximally aligned so
  // that tp + dtv[N] is aligned to align_N no matter what N is. So we need
  // 'mem' to be such that if mem == td then td->head is maximially aligned.
  // To do this we need take &td->head (e.g. mem + offset of head) and align
  // it then subtract out the offset of ->head to ensure that &td->head is
  // aligned.
  {
    uintptr_t tp = reinterpret_cast<uintptr_t>(mem) + PTHREAD_TP_OFFSET;
    tp = (tp + layout.align - 1) & -layout.align;
    td = reinterpret_cast<Thread*>(tp - PTHREAD_TP_OFFSET);
    // Now mem should be the new thread pointer.
    mem = reinterpret_cast<unsigned char*>(tp);
  }
#else
  // *-----------------------------------------------------------------------*
  // | tls_cnt | dtv[1] | ... | tls_n | ... | tls_1 | tcb | pthread | unused |
  // *-----------------------------------------------------------------------*
  // ^                        ^             ^       ^
  // dtv                   dtv[n+1]       dtv[1]  tp/td
  //
  // Note: The TCB is actually the first member of pthread.
  dtv = reinterpret_cast<void**>(mem);

  mem += thread_block.size_bytes() - sizeof(Thread);
  mem -= reinterpret_cast<uintptr_t>(mem) & (layout.align - 1);
  td = reinterpret_cast<Thread*>(mem);
#endif

  [[maybe_unused]] void* tp = pthread_to_tp(td);
  [[maybe_unused]] void* mem_tp = thread_block.data() + tp_offset;
  ZX_DEBUG_ASSERT_MSG(tp == mem_tp,
                      ": $tp %p / pthread* %p != %p / [%p, %p) + tp_offset %zu"
                      "; TLS %#zx aligned to %zu; sizeof(pthread) == %zu",
                      tp, td, mem_tp, thread_block.data(), &thread_block.back() + 1, tp_offset,
                      _dl_tls_layout().size, _dl_tls_layout().align, sizeof(pthread));

  size_t i;
  const tls_module* p;
  for (i = 1, p = layout.tls_head; p; i++, p = p->next) {
    dtv[i] = mem + OffsetForModule(p);
    memcpy(dtv[i], p->image, p->len);
  }

  dtv[0] = reinterpret_cast<void*>(static_cast<uintptr_t>(layout.tls_cnt));
  td->head.dtv = dtv;
}

}  // namespace LIBC_NAMESPACE_DECL
