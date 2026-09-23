// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_KTL_INCLUDE___CHRONO_SYSTEM_CLOCK_H_
#define ZIRCON_KERNEL_LIB_KTL_INCLUDE___CHRONO_SYSTEM_CLOCK_H_

// libc++'s <atomic> includes this file via various internal headers.
// The declarations it provides aren't needed for the kernel and rely
// on other things stubbed out by the kernel.  So this placeholder file
// just prevents compiling the libc++ code that doesn't work.

#endif  // ZIRCON_KERNEL_LIB_KTL_INCLUDE___CHRONO_SYSTEM_CLOCK_H_
