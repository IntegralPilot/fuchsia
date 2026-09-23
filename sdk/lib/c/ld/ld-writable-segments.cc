// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <algorithm>

#include "ld-abi.h"
#include "writable-segments.h"

namespace LIBC_NAMESPACE_DECL {

void WritableSegmentsMemorySnapshot(sanitizer_memory_snapshot_callback_t* callback,
                                    void* callback_arg) {
  auto report_module = ModuleWritableSegmentsCallback(callback, callback_arg);
  std::ranges::for_each(ld::AbiLoadedModules(_ld_abi), report_module);
}

}  // namespace LIBC_NAMESPACE_DECL
