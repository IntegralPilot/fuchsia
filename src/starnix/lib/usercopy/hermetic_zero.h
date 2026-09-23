// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_LIB_USERCOPY_HERMETIC_ZERO_H_
#define SRC_STARNIX_LIB_USERCOPY_HERMETIC_ZERO_H_

#include <stddef.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

__BEGIN_CDECLS

// hermetic_zero performs a memset(.., 0, ..). It is compiled as a hermetic code bundle meaning
// that it does not branch or call out into any locations outside of the bundle while
// zeroing data so that faults from this routine can be identified unambiguously.
uintptr_t hermetic_zero(volatile uint8_t* dest, size_t count);

__END_CDECLS

#endif  // SRC_STARNIX_LIB_USERCOPY_HERMETIC_ZERO_H_
