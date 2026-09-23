// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

// This function exists only to have CFI that enumerates all the registers.
// It's called only to have its trap caught so the unwinder can be applied to
// its registers to see what it yields given CFI that save same-value for all.
.function TrapWithAllRegs, global
  .cfi.all_integer .cfi_same_value
  .cfi.all_vectorfp .cfi_same_value
  assert.fail

  // This actual function could look like others (that are just a trap) and get
  // ICF'd with them.  Then there's no guarantee which one is preserved and has
  // its CFI tables attached.  Even if this code is not unique in effect when
  // actually called, it's unique in attached CFI.  So add some distinctive,
  // unreachable garbage to disambiguate.
  .int 0xdeadbeef
.end_function
