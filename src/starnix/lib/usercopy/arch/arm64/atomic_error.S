// Copyright 2023 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// "returns" from faulting atomic operation.
// The error value should be loaded in the high 32 bits of x0 before jumping to this routine.
// This library's atomic routines do not use any stack space, so we just need to retq.

.globl atomic_error
atomic_error:
  ret
