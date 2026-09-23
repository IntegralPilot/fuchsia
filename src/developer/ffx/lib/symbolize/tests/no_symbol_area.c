// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <stdint.h>

__asm__(
    ".pushsection .data\n"

    // Store 4 bytes that do not belong to any symbol.
    ".ascii \"ABCD\"\n"

    // Store 4 more bytes belonging to a symbol.
    "symbol_after_unnamed_bytes:"
    ".ascii \"EFGH\"\n"
    ".size symbol_after_unnamed_bytes, 4\n"

    ".popsection\n");

extern uint8_t symbol_after_unnamed_bytes[4];

__attribute__((visibility("default"))) const void* get_ptr_after_unnamed_bytes(void) {
  return symbol_after_unnamed_bytes;
}
