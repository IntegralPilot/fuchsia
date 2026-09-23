// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef THIRD_PARTY_PIGWEED_BACKENDS_PW_LOG_PUBLIC_PW_LOG_SINK_LOG_SINK_H_
#define THIRD_PARTY_PIGWEED_BACKENDS_PW_LOG_PUBLIC_PW_LOG_SINK_LOG_SINK_H_

#include <lib/async/dispatcher.h>

namespace pw_log_sink {

void InitializeLogging(async_dispatcher_t* dispatcher);

}  // namespace pw_log_sink

#endif  // THIRD_PARTY_PIGWEED_BACKENDS_PW_LOG_PUBLIC_PW_LOG_SINK_LOG_SINK_H_
