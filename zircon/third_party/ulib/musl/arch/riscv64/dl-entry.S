// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "asm.h"

// Call the C _dl_start, which returns a dl_start_return_t containing the
// user entry point and its argument.  Then jump to that entry point with
// the argument in the first argument register (a0, where it was placed by
// the C function's return), clearing the return address and frame pointer
// registers so the user entry point is the base of the call stack.

.hidden _start
ENTRY(_start)
  call _dl_start
  // That returned the argument value for the user in a0, where it stays; and
  // the user entry PC in a1, which we move to t1.  Then zero any interesting
  // state: fp & ra to show base frame; 2nd and 3rd user argument registers for
  // unused __libc_start_main arguments _start will leave in place.
  mv t1, a1                     // 2nd return value -> user PC (t1)
  mv fp, zero
  mv ra, zero
  mv a1, zero
  mv a2, zero
  jr t1                         // Avoid `jr t0`, which is hinted as return.
END(_start)
