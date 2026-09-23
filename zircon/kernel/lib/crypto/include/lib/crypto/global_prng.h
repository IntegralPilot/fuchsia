// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_CRYPTO_INCLUDE_LIB_CRYPTO_GLOBAL_PRNG_H_
#define ZIRCON_KERNEL_LIB_CRYPTO_INCLUDE_LIB_CRYPTO_GLOBAL_PRNG_H_

#include <lib/crypto/prng.h>

#include <kernel/ffi.h>

namespace crypto {

namespace global_prng {

// Returns a pointer to the global PRNG singleton.  The pointer is
// guaranteed to be non-null.
Prng* GetInstance();

}  // namespace global_prng

}  // namespace crypto

extern "C" {
// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_global_prng_draw(uint8_t* buffer, size_t len);
// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_global_prng_add_entropy(const uint8_t* buffer, size_t len);
}

#endif  // ZIRCON_KERNEL_LIB_CRYPTO_INCLUDE_LIB_CRYPTO_GLOBAL_PRNG_H_
