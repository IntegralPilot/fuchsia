// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_UTILS_CHECK_IS_ON_THREAD_H_
#define SRC_UI_SCENIC_LIB_UTILS_CHECK_IS_ON_THREAD_H_

#include <lib/async/dispatcher.h>
#include <lib/syslog/cpp/macros.h>

namespace utils {

// Asserts (via FX_DCHECK) that the current thread is the main thread by
// comparing the current default dispatcher with the stored main dispatcher.
void CheckIsOnMainThread();

// Asserts (via FX_DCHECK) that the current thread is the input thread by
// comparing the current default dispatcher with the stored input dispatcher.
void CheckIsOnInputThread();

// RAII object to set the main and input thread dispatchers for testing or
// production initialization.
//
// This object ensures that the dispatchers are set for the duration of its scope
// and automatically reset to nullptr when it goes out of scope.
//
// It is a fatal error (FX_DCHECK) to create a ScopedThreadDispatcherSetter if
// the dispatchers are already set (i.e., not nullptr). This prevents nested
// overrides and ensures test isolation.
class ScopedThreadDispatcherSetter {
 public:
  // Sets the main and input thread dispatchers.
  ScopedThreadDispatcherSetter(async_dispatcher_t* main_dispatcher,
                               async_dispatcher_t* input_dispatcher);

  ~ScopedThreadDispatcherSetter();
};

}  // namespace utils

#endif  // SRC_UI_SCENIC_LIB_UTILS_CHECK_IS_ON_THREAD_H_
