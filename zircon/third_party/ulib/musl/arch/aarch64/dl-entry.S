// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "asm.h"

// Call the C _dl_start, which returns a dl_start_return_t containing the
// user entry point and its argument.  Then jump to that entry point with
// the argument in the first argument register (x0, where it was placed by
// the C function's return), clearing the return address and frame pointer
// registers so the user entry point is the base of the call stack.

.hidden _start
ENTRY(_start)
    bl _dl_start
    // 1st return value (x0) -> 1st user argument register (x0).
    mov x16, x1                 // 2nd return value -> user PC, use x16 temp.
    mov x29, xzr                // Clear frame pointer.
    mov x30, xzr                // Clear return address register.
    mov x1, xzr                 // Zero -> 2nd user argument register.
    mov x2, xzr                 // Zero -> 3nd user argument register.
    br x16
END(_start)
