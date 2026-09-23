// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_LIB_TEST_UTILS_TRACE_CONTROLLER_H_
#define SRC_PERFORMANCE_LIB_TEST_UTILS_TRACE_CONTROLLER_H_

#include <fidl/fuchsia.tracing.controller/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>

#include <future>

struct Tracer {
  fidl::SyncClient<fuchsia_tracing_controller::Session> controller;
  std::future<zx_status_t> future;
};

fit::result<fit::failed, Tracer> StartTracing(fuchsia_tracing_controller::TraceConfig trace_config,
                                              const char* output_file);
fit::result<fit::failed> StopTracing(Tracer tracer);

#endif  // SRC_PERFORMANCE_LIB_TEST_UTILS_TRACE_CONTROLLER_H_
