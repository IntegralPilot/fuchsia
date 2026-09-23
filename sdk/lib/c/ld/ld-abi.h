// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_LD_LD_ABI_H_
#define LIB_C_LD_LD_ABI_H_

// Code defined in LIBC_NAMESPACE should use `_ld_abi` without qualifier so it
// gets LIBC_NAMESPACE::_ld_abi as declared here.  In a production libc, this
// is just a namespace alias for ld::abi::_ld_abi (which has an `extern "C"`
// linkage name).  In the unittest build, this is a constexpr reference to the
// ld::testing mock-up for the test binary's startup modules.

#include <lib/ld/abi.h>

#include "src/__support/macros/config.h"

#ifndef LIBC_COPT_PUBLIC_PACKAGING
#include <lib/ld/testing/startup-ld-abi.h>
#endif

namespace LIBC_NAMESPACE_DECL {

#ifdef LIBC_COPT_PUBLIC_PACKAGING

using ld::abi::_ld_abi;

#else

inline constexpr decltype(ld::abi::_ld_abi)& _ld_abi = ld::testing::gStartupLdAbi;

#endif

}  // namespace LIBC_NAMESPACE_DECL

#endif  // LIB_C_LD_LD_ABI_H_
