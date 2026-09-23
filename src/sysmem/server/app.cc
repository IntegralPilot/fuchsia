// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "app.h"

#include <fidl/fuchsia.feedback/cpp/fidl.h>
#include <fidl/fuchsia.hardware.sysmem/cpp/fidl.h>
#include <fidl/fuchsia.metrics/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/vfs/cpp/pseudo_dir.h>

#include <sdk/lib/syslog/cpp/macros.h>

App::App(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {
  sysmem_service::Sysmem::CreateArgs create_args{
      .create_bti = true,
      .expect_structured_config = true,
      .serve_outgoing = true,
      .set_scheduler_profile = true,
  };
  auto create_result = sysmem_service::Sysmem::Create(dispatcher_, create_args);
  ZX_ASSERT_MSG(create_result.is_ok(), "sysmem_service::Sysmem::Create() failed: %s",
                create_result.status_string());
  device_ = std::move(create_result.value());
}

App::~App() {
  // Run ~Device first regardless of field order in App.
  device_.reset();
  FX_LOGS(INFO) << "device_.reset() done";
}
