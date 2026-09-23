// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_THIRD_PARTY_LIB_JITTERENTROPY_ARCH_INTERNAL_H_
#define ZIRCON_THIRD_PARTY_LIB_JITTERENTROPY_ARCH_INTERNAL_H_

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <zircon/compiler.h>

__BEGIN_CDECLS

bool jent_have_clock(void);

void jent_get_nstime(uint64_t* out);

static inline void* jent_zalloc(size_t len) { return NULL; }

static inline void jent_zfree(void* ptr, size_t len) {}

static inline int jent_fips_enabled(void) { return 0; }

static inline uint64_t rol64(uint64_t x, uint32_t n) { return (x << n) | (x >> (64 - n)); }

__END_CDECLS

#endif  // ZIRCON_THIRD_PARTY_LIB_JITTERENTROPY_ARCH_INTERNAL_H_
