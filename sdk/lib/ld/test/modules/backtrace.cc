// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "backtrace.h"

#include <lib/arch/asm.h>

#include "test-start.h"

[[gnu::visibility("hidden")]] extern arch::AsmLabel _start, _start_end;

extern "C" [[gnu::noinline]] int64_t TestStart() {
  const uintptr_t ra = reinterpret_cast<uintptr_t>(__builtin_return_address(0));
  const FramePointer* fp = FramePointer::Get();

  // The immediate caller is _start, which pushed its own frame pointer.
  if (fp->ra != ra) {
    return 1;
  }
  if (fp->fp == nullptr) {
    return 2;
  }

  const uintptr_t start = arch::kAsmLabelAddress<_start>;
  const uintptr_t end = arch::kAsmLabelAddress<_start_end>;
  if (ra <= start) {
    return 3;
  }
  if (ra > end) {
    return 4;
  }

  // _start is the root of the call graph, so its "caller's" return address and
  // frame pointer should both be zero.
  fp = fp->Next();
  if (fp->ra != 0) {
    return 4;
  }
  if (fp->fp != nullptr) {
    return 5;
  }

  return 17;
}
