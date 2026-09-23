// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/compiler.h>

#include <cstdint>

#include "src/lib/debug/backtrace-request.h"
#include "src/lib/debug/debug.h"

// Offers symbols that can be called from rust code.

__EXPORT extern "C" void backtrace_request_all_threads_for_rust() {
  backtrace_request_all_threads();
}

__EXPORT extern "C" void backtrace_request_current_thread_for_rust() {
  backtrace_request_current_thread();
}

__EXPORT extern "C" void backtrace_request_thread_for_rust(uint64_t koid) {
  backtrace_request_thread(koid);
}

__EXPORT extern "C" bool is_debugger_attached_for_rust() { return debug::IsDebuggerAttached(); }

__EXPORT extern "C" void wait_for_debugger_for_rust(uint32_t seconds) {
  debug::WaitForDebugger(seconds);
}
