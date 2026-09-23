// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TESTING_STARTUP_LD_ABI_H_
#define LIB_LD_TESTING_STARTUP_LD_ABI_H_

#include <lib/ld/abi.h>

namespace ld {
namespace [[gnu::visibility("hidden")]] testing {

// This is prepopulated at program startup by collecting the test program's own
// data via dl_iterate_phdr.  It can be used as test data representing the ELF
// modules and Initial Exec TLS layout in the test itself.
//
// **Note**: The pointers found within all layers of this data structure should
// be valid forever.  However, in the event of static constructors that run
// before this one and use `dlopen` to load modules that are later unloaded,
// some dangling pointers will be left here.  Test code must not use `dlopen`
// in static constructors.
extern const abi::Abi<> gStartupLdAbi;

}  // namespace testing
}  // namespace ld

#endif  // LIB_LD_TESTING_STARTUP_LD_ABI_H_
