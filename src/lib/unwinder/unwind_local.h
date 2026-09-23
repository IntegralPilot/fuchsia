// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_UNWINDER_UNWIND_LOCAL_H_
#define SRC_LIB_UNWINDER_UNWIND_LOCAL_H_

#include "src/lib/unwinder/frame.h"
#include "src/lib/unwinder/memory.h"

namespace unwinder {

// Unwind from the current location. The first frame in the returned value is the return address
// of this function call. This function is not available on macOS.
std::vector<Frame> UnwindLocal();

// Asynchronous version of the above.
void UnwindLocalAsync(Memory* local_memory, AsyncMemory::Delegate* delegate,
                      fit::callback<void(std::vector<Frame> frames)> on_done);

}  // namespace unwinder

#endif  // SRC_LIB_UNWINDER_UNWIND_LOCAL_H_
