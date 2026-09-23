// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_MUTABLE_ABI_H_
#define LIB_LD_MUTABLE_ABI_H_

#include <lib/ld/abi.h>
#include <lib/ld/tls.h>

namespace ld {

// This is the mutable view of the exported ld::abi::_ld_abi symbol.
// In the startup dynamic linker, it's actually mutable data.
extern abi::Abi<> mutable_abi __asm__("_ld_abi");

// This is not part of the passive ABI proper but is an exported symbol for
// the benefit of debuggers that look for it traditionally.
extern abi::Abi<>::RDebug mutable_r_debug __asm__("_r_debug");

}  // namespace ld

#endif  // LIB_LD_MUTABLE_ABI_H_
