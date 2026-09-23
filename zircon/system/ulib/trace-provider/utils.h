// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_ULIB_TRACE_PROVIDER_UTILS_H_
#define ZIRCON_SYSTEM_ULIB_TRACE_PROVIDER_UTILS_H_

#include <lib/zx/result.h>
#include <zircon/types.h>

namespace trace {
namespace internal {

zx_koid_t GetPid();

/// Tries to find the process name and put it in the buffer.
/// If it fails to find the process name, it prints a warning and puts \0 in the buffer.
/// [buffer] must not be NULL, and must hold at least [buffer_size] bytes.
zx::result<std::string> GetProcessName();

}  // namespace internal
}  // namespace trace

#endif  // ZIRCON_SYSTEM_ULIB_TRACE_PROVIDER_UTILS_H_
