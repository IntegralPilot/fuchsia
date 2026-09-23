// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.boot/cpp/fidl.h>
#include <fidl/fuchsia.kernel/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>
#include <lib/zx/channel.h>
#include <lib/zx/job.h>
#include <stdio.h>

#include <zxtest/zxtest.h>

zx::resource GetProfileResource() {
  zx::result local = component::Connect<fuchsia_kernel::ProfileResource>();
  if (!local.is_ok()) {
    EXPECT_OK(local.status_value(), "unable to open fuchsia.boot.ProfileResource channel");
    return {};
  }

  auto result = fidl::WireCall(*local)->Get();
  if (!result.ok()) {
    EXPECT_OK(result.error().status(), "unable to get profile resource");
    return {};
  }

  return std::move(result->resource);
}
