// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TEST_MODULES_BACKTRACE_H_
#define LIB_LD_TEST_MODULES_BACKTRACE_H_

#include <stdint.h>

struct FramePointer {
  // The default argument is evaluated in the caller's context even if the
  // function doesn't get inlined.
  static FramePointer* Get(void* address = __builtin_frame_address(0)) {
    FramePointer* fp = static_cast<FramePointer*>(address);
#ifdef __riscv
    // The RISC-V fp points to the CFA, past the standard pair.
    // On all other machines the fp points to the standard pair.
    --fp;
#endif
    return fp;
  }

  FramePointer* Next() const { return fp ? Get(fp) : nullptr; }

  FramePointer* fp;
  uintptr_t ra;
};

#endif  // LIB_LD_TEST_MODULES_BACKTRACE_H_
