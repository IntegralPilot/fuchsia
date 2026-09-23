// Copyright 2023 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// "returns" from hermetic_copy with exception information.
// preconditions:
//   - saved rbp in (%rsp)
//   - faulting address in %rax

.globl hermetic_copy_error
hermetic_copy_error:
  popq %rbp
  retq
  int3
  int3
  int3
  int3
  int3
  int3
  int3
  int3
