// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_X86_INCLUDE_ARCH_RESTRICTED_H_
#define ZIRCON_KERNEL_ARCH_X86_INCLUDE_ARCH_RESTRICTED_H_

#include <inttypes.h>

// When an x86 thread is inside restricted mode, it must additionally save
// the hidden FS and GS base registers that it must restore when reentering
// normal mode.
struct ArchSavedNormalState {
  uint64_t normal_fs_base_ = 0;
  uint64_t normal_gs_base_ = 0;
};

#endif  // ZIRCON_KERNEL_ARCH_X86_INCLUDE_ARCH_RESTRICTED_H_
