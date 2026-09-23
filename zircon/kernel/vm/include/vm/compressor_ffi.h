// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_COMPRESSOR_FFI_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_COMPRESSOR_FFI_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

#include <kernel/ffi.h>

#include "vm/compressor.h"

__BEGIN_CDECLS

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t cpp_vmcompressor_arm(VmCompressor* compressor);

__END_CDECLS

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_COMPRESSOR_FFI_H_
