// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/shared/arch.h"

namespace debug {

const char* ArchToString(Arch a) {
  switch (a) {
    case Arch::kUnknown:
      return "Unknown architecture";
    case Arch::kX64:
      return "x86-64";
    case Arch::kArm64:
      return "aarch64";
    case Arch::kRiscv64:
      return "riscv64";
  }
  return "Invalid architecture";
}

}  // namespace debug
