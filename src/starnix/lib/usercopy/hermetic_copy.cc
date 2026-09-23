// Copyright 2023 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "hermetic_copy.h"

#include "hermetic_copy_common.h"

// Build with the following cc flags:
//  `-O3 -fno-omit-frame-pointer -mno-omit-leaf-frame-pointer`

[[gnu::section(".text.entry")]] uintptr_t hermetic_copy(volatile uint8_t* dest,
                                                        const volatile uint8_t* source,
                                                        size_t count, bool ret_dest) {
  return hermetic_copy_common<false>(dest, source, count, ret_dest);
}
