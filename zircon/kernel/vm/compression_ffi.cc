// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "vm/compression_ffi.h"

#include <kernel/ffi.h>
#include <ktl/memory.h>

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" {

FFI_ALWAYS_INLINE void cpp_vmcompression_destroy(VmCompression* compression) {
  ktl::destroy_at(compression);
}

FFI_ALWAYS_INLINE void cpp_vmcompression_free(VmCompression* compression) { delete compression; }

FFI_ALWAYS_INLINE fbl::RefCounted<VmCompression>* cpp_vmcompression_get_ref_counted(
    VmCompression* compression) {
  return const_cast<fbl::RefCounted<VmCompression>*>(
      static_cast<const fbl::RefCounted<VmCompression>*>(compression));
}

FFI_ALWAYS_INLINE void cpp_vmcompression_acquire_compressor(
    ffi::Uninitialized<VmCompression::CompressorGuard>* guard, VmCompression* compression) {
  // `Initialize` move-constructs into `guard` and destroys the temporary returned by
  // `AcquireCompressor()`. This is safe because the move constructor transfers the lock, so the
  // temporary's destructor does not release it.
  guard->Initialize(compression->AcquireCompressor());
}

FFI_ALWAYS_INLINE void cpp_vmcompression_compressor_guard_destroy(
    VmCompression::CompressorGuard* guard) {
  ktl::destroy_at(guard);
}

FFI_ALWAYS_INLINE VmCompressor* cpp_vmcompression_compressor_guard_get(
    VmCompression::CompressorGuard* guard) {
  return &guard->get();
}

}  // extern "C"
