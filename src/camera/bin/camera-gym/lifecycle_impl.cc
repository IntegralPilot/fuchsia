// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/camera/bin/camera-gym/lifecycle_impl.h"

#include <lib/async-loop/default.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/processargs.h>

LifecycleImpl::LifecycleImpl(fit::closure on_terminate)
    : loop_(&kAsyncLoopConfigNoAttachToCurrentThread), on_terminate_(std::move(on_terminate)) {
  loop_.StartThread("Lifecycle Thread");

  zx::channel channel = zx::channel(zx_take_startup_handle(PA_LIFECYCLE));
  if (!channel.is_valid()) {
    FX_LOGS(FATAL) << "PA_LIFECYCLE startup handle is required.";
  }

  bindings_.AddBinding(
      this, fidl::InterfaceRequest<fuchsia::process::lifecycle::Lifecycle>(std::move(channel)),
      loop_.dispatcher());
}

void LifecycleImpl::Stop() {
  on_terminate_();
  loop_.Shutdown();
  bindings_.CloseAll();
}
