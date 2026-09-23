// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_FFI_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_FFI_H_

#include <lib/lazy_init/lazy_init.h>

// This header defines helper macros and types for C++ to Rust FFI routines in the Zircon kernel.
//
// Short C++ FFI helper routines (such as trivial forwarding shims or simple accessors)
// should be annotated with `FFI_ALWAYS_INLINE` to ensure inlining under Clang without
// triggering `-Werror=attributes` under GCC (which requires the `inline` keyword for
// `[[gnu::always_inline]]` definitions, but adding `inline` alters non-static linkage
// semantics across translation units).
//
// TODO(https://fxbug.dev/537458631): Remove this header and annotations once cross-language
// inlining and toolchain support are resolved.
#ifdef __clang__
#define FFI_ALWAYS_INLINE [[gnu::always_inline]]
#else
#define FFI_ALWAYS_INLINE
#endif

namespace ffi {

// Type alias for lazy storage with runtime checking and destructors disabled.
//
// Used for uninitialized storage allocated in Rust (e.g. `core::mem::MaybeUninit<T>`)
// or on the stack and passed into C++ FFI routines to construct/initialize an object.
template <typename T>
using Uninitialized =
    lazy_init::LazyInit<T, lazy_init::CheckType::None, lazy_init::Destructor::Disabled>;

}  // namespace ffi

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_FFI_H_
