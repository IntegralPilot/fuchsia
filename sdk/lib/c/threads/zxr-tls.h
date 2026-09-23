// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_THREADS_ZXR_TLS_H_
#define LIB_C_THREADS_ZXR_TLS_H_

#include <zircon/compiler.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/object.h>

__BEGIN_CDECLS

// Set the thread pointer.
static inline void zxr_tp_set(zx_handle_t self, void* tp);

// These are used in very early and low-level places where most kinds
// of instrumentation are not safe.
#ifdef __clang__
#define ZXR_NO_SANITIZERS __attribute__((no_sanitize("all")))
#else
#define ZXR_NO_SANITIZERS
#endif

// These are tiny functions meant to be inlined, where a call would often
// actually take more instruction bytes than just inlining it.  However,
// Clang avoids inlining across functions with differing attributes and the
// necessary `no_sanitize` attribute trips that logic.  So tell the
// compiler to force inlining here.
#define ZXR_TLS_INLINE ZXR_NO_SANITIZERS __attribute__((always_inline))

#if defined(__aarch64__)

ZXR_TLS_INLINE static inline void zxr_tp_set(zx_handle_t self, void* tp) {
  __asm__ volatile("msr tpidr_el0, %0" : : "r"(tp));
}

#elif defined(__x86_64__)

ZXR_TLS_INLINE static inline void zxr_tp_set(zx_handle_t self, void* tp) {
  zx_status_t status =
      _zx_object_set_property(self, ZX_PROP_REGISTER_FS, (uintptr_t*)&tp, sizeof(uintptr_t));
  if (status != ZX_OK)
    __builtin_trap();
}

#elif defined(__riscv)

ZXR_TLS_INLINE static inline void zxr_tp_set(zx_handle_t self, void* tp) {
  __asm__ volatile("mv tp, %0" : : "r"(tp));
}

#else

#error Unsupported architecture

#endif

__END_CDECLS

#endif  // LIB_C_THREADS_ZXR_TLS_H_
