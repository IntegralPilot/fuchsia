// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_KTL_INCLUDE_WCHAR_H_
#define ZIRCON_KERNEL_LIB_KTL_INCLUDE_WCHAR_H_

// This file is included by <cstdio> and the like that are used from other
// libc++ headers that are wrapped by ktl.  This file exists to preempt those
// inclusions so that libc++'s <wchar.h> wrapper header won't be used, since
// it's not compatible with the kernel environment.  However, other libc++
// headers also use `#include_next <wchar.h>` to get to "the system libc"
// version of the header.  To satisfy those, include-after/wchar.h provides the
// actual declarations needed from the standard C <wchar.h> API.  So this
// header just defers to that one, but must exist separately in this directory
// so that it preempt's libc++'s wrapper.  This can't itself use #include_next
// because that would reach libc++'s wrapper before the include-after file.

#include "../include-after/wchar.h"

#endif  // ZIRCON_KERNEL_LIB_KTL_INCLUDE_WCHAR_H_
