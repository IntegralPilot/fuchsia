// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/utils/check_is_on_thread.h"

#include <lib/async/default.h>

namespace utils {

namespace {
async_dispatcher_t* g_main_dispatcher = nullptr;
async_dispatcher_t* g_input_dispatcher = nullptr;
}  // namespace

void CheckIsOnMainThread() { FX_DCHECK(g_main_dispatcher == async_get_default_dispatcher()); }

void CheckIsOnInputThread() { FX_DCHECK(g_input_dispatcher == async_get_default_dispatcher()); }

utils::ScopedThreadDispatcherSetter::ScopedThreadDispatcherSetter(async_dispatcher_t* main,
                                                                  async_dispatcher_t* input) {
  FX_DCHECK(main != nullptr);
  FX_DCHECK(input != nullptr);
  FX_DCHECK(g_main_dispatcher == nullptr);
  FX_DCHECK(g_input_dispatcher == nullptr);
  g_main_dispatcher = main;
  g_input_dispatcher = input;
}

utils::ScopedThreadDispatcherSetter::~ScopedThreadDispatcherSetter() {
  FX_DCHECK(g_main_dispatcher != nullptr);
  FX_DCHECK(g_input_dispatcher != nullptr);
  g_main_dispatcher = nullptr;
  g_input_dispatcher = nullptr;
}

}  // namespace utils
