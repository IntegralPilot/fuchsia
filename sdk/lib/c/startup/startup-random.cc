// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "startup-random.h"

#include <lib/ld/tls.h>
#include <zircon/syscalls.h>
#include <zircon/tls.h>

#include <cstddef>
#include <cstdint>
#include <span>
#include <utility>

namespace LIBC_NAMESPACE_DECL {

// These are defined implicitly by the linker.  Together they form an array
// containing all the LIBC_STARTUP_RANDOM_VAR variables.  There will always be
// at least one the static one defined here for the stack guard, so these
// declarations are not weak and thus can use direct PC-relative references.
// Apparently when using explicit linkage names, GCC also needs the visibility
// attribute on each variable despite the namespace-scope attribute.
[[gnu::visibility("hidden")]]
extern std::byte kBegin[] LIBC_ASM_LINKAGE_DECLARE(StartupRandom, __start_);
[[gnu::visibility("hidden")]]
extern std::byte kEnd[] LIBC_ASM_LINKAGE_DECLARE(StartupRandom, __stop_);

void InitStartupRandom() {
  // Fill it all with random bits.
  std::span bytes{kBegin, kEnd};
  zx_cprng_draw(bytes.data(), bytes.size_bytes());

  // That just filled this variable too.  Make sure the compiler thinks
  // something did, so it doesn't presume that the value is zero.
  LIBC_STARTUP_RANDOM_VAR static uintptr_t stack_guard;
  __asm__ volatile("" : "=m"(stack_guard));

  // Move the random value into the ABI slot, but don't leave it behind to be
  // seen in the static variable.
  *ld::TpRelative<uintptr_t>(ZX_TLS_STACK_GUARD_OFFSET) = std::exchange(stack_guard, 0);

  // Make sure the compiler doesn't think it can elide the zero store.
  __asm__ volatile("" : : "m"(stack_guard));
}

}  // namespace LIBC_NAMESPACE_DECL
