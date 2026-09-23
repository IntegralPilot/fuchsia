// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/utils/dispatcher_holder.h"

namespace utils {

LoopDispatcherHolder::LoopDispatcherHolder(const async_loop_config_t* config,
                                           DestroyLoop destroy_loop)
    : loop_(std::make_unique<async::Loop>(config)), destroy_loop_(std::move(destroy_loop)) {}

LoopDispatcherHolder::~LoopDispatcherHolder() { destroy_loop_(std::move(loop_)); }

}  // namespace utils
