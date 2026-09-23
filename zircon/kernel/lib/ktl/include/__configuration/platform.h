// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_KTL_INCLUDE___CONFIGURATION_PLATFORM_H_
#define ZIRCON_KERNEL_LIB_KTL_INCLUDE___CONFIGURATION_PLATFORM_H_

// Other libc++ headers use <__configuration/platform.h> but libc++'s
// <__configuration/platform.h> does not yet recognize the `__PE_COFF__`
// predefine. Until we support this new predefine upstream,
// we must define _LIBCPP_OBJECT_FORMAT_COFF for UEFI targets here.
// TODO(https://fxbug.dev/435771251) -- Support `__PE_COFF__` predefine
// upstream.

#if defined(__PE_COFF__)
#define _LIBCPP_OBJECT_FORMAT_COFF 2  // Tell libc++ this is PE-COFF.
#endif // __PE_COFF__

#include_next <__configuration/platform.h>

#endif  // ZIRCON_KERNEL_LIB_KTL_INCLUDE___CONFIGURATION_PLATFORM_H_
