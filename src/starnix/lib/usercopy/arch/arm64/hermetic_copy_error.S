// Copyright 2023 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// "returns" from hermetic_copy with exception information.
// preconditions:
//   - Only x29 (frame pointer), x30 (link register) on the stack.
//   - faulting address in x0

.globl hermetic_copy_error
hermetic_copy_error:
  // Restore the frame pointer and link register which the hermetic copy
  // function put on the stack.
  ldp x29, x30, [sp], #0x10

  // Return back to the caller of the hermetic copy function with the
  // faulting address as the return value (held in x0).
  ret

  // Invalid instructions that should never be executed.
  udf 0
  udf 0
  udf 0
  udf 0
  udf 0
  udf 0
  udf 0
  udf 0
