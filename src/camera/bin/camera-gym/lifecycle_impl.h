// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CAMERA_BIN_CAMERA_GYM_LIFECYCLE_IMPL_H_
#define SRC_CAMERA_BIN_CAMERA_GYM_LIFECYCLE_IMPL_H_

#include <fuchsia/process/lifecycle/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/fit/function.h>

// Implementation of the fuchsia.process.lifecycle FIDL protocol that invokes a caller-provided
// closure on Terminate.
class LifecycleImpl : public fuchsia::process::lifecycle::Lifecycle {
 public:
  explicit LifecycleImpl(fit::closure on_terminate);
  ~LifecycleImpl() override = default;

  void Stop() override;

 private:
  async::Loop loop_;
  fit::closure on_terminate_;
  fidl::BindingSet<fuchsia::process::lifecycle::Lifecycle> bindings_;
};

#endif  // SRC_CAMERA_BIN_CAMERA_GYM_LIFECYCLE_IMPL_H_
