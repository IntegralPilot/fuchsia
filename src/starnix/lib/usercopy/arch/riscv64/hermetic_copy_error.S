// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// "returns" from hermetic_copy with exception information.
// preconditions:
//   - Only x1 (ra/return address) and x8 (s0/fp/frame pointer) on the stack.
//   - faulting address in a0

.globl hermetic_copy_error
hermetic_copy_error:
    // Restore the return address and frame pointer which the hermetic copy
    // function put on the stack.
    ld  ra, 8(sp)
    ld  fp, 0(sp)
    add sp, sp, 16

    // Return back to the caller of the hermetic copy function with the
    // faulting address as the return value (held in a0).
    ret

    // Invalid instructions that should never be executed.
    unimp
    unimp
