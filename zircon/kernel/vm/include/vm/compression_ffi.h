// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_COMPRESSION_FFI_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_COMPRESSION_FFI_H_

#include <zircon/compiler.h>

#include <kernel/ffi.h>

#include "vm/compression.h"

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
__BEGIN_CDECLS

FFI_ALWAYS_INLINE void cpp_vmcompression_destroy(VmCompression* compression);
FFI_ALWAYS_INLINE void cpp_vmcompression_free(VmCompression* compression);
FFI_ALWAYS_INLINE fbl::RefCounted<VmCompression>* cpp_vmcompression_get_ref_counted(
    VmCompression* compression);
FFI_ALWAYS_INLINE void cpp_vmcompression_acquire_compressor(
    ffi::Uninitialized<VmCompression::CompressorGuard>* guard, VmCompression* compression);
FFI_ALWAYS_INLINE void cpp_vmcompression_compressor_guard_destroy(
    VmCompression::CompressorGuard* guard);
FFI_ALWAYS_INLINE VmCompressor* cpp_vmcompression_compressor_guard_get(
    VmCompression::CompressorGuard* guard);

__END_CDECLS

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_COMPRESSION_FFI_H_
