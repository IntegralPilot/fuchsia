// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_ULIB_INSPECTOR_BACKTRACE_H_
#define ZIRCON_SYSTEM_ULIB_INSPECTOR_BACKTRACE_H_

#include <stdio.h>
#include <zircon/types.h>

#include <vector>

#include "src/lib/unwinder/unwind.h"

namespace inspector {

std::vector<unwinder::Frame> get_frames(FILE* f, zx_handle_t process, zx_handle_t thread);

// If |pcs| is non-empty, only print modules containing a frame from |pcs|. However, this should
// only be used in certain situations. See https://fxbug.dev/42076491.
void print_markup_context(FILE* f, zx_handle_t process, std::vector<uint64_t> pcs);

}  // namespace inspector

#endif  // ZIRCON_SYSTEM_ULIB_INSPECTOR_BACKTRACE_H_
