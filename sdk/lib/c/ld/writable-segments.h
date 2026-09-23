// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_LD_WRITABLE_SEGMENTS_H_
#define LIB_C_LD_WRITABLE_SEGMENTS_H_

#include <lib/ld/module.h>
#include <zircon/compiler.h>
#include <zircon/sanitizer.h>

#include <cstddef>
#include <span>

#include "../dlfcn/dlfcn-abi.h"
#include "src/__support/macros/config.h"

namespace LIBC_NAMESPACE_DECL {

// Make callbacks on behalf of __sanitizer_memory_snapshot for the writable
// segments of every module.  This is run when all other threads were
// previously suspended while holding the kDlfcnLock; but it's no longer held.
// All other threads remain suspended during this call.  (So it's as if the
// lock _were_ held in that no other thread will contend for it.)  Thus this
// function can safely take locks but also can probably safely ignore locking.
void WritableSegmentsMemorySnapshot(sanitizer_memory_snapshot_callback_t*, void*)
    __TA_EXCLUDES(kDlfcnLock);

// This returns a void(const ld::abi::Abi<>::Module&) callable to make such
// callbacks for one module.
constexpr auto ModuleWritableSegmentsCallback(sanitizer_memory_snapshot_callback_t* callback,
                                              void* callback_arg) {
  auto report_segment = [callback, callback_arg](std::span<std::byte> segment) {
    callback(segment.data(), segment.size_bytes(), callback_arg);
    return true;
  };
  return [report_segment](const ld::abi::Abi<>::Module& module) {
    ld::OnModuleWritableSegments(module, report_segment);
  };
}

}  // namespace LIBC_NAMESPACE_DECL

#endif  // LIB_C_LD_WRITABLE_SEGMENTS_H_
