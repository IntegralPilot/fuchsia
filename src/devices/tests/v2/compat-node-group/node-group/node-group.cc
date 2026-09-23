// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.compat.nodegroup.test/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>

namespace fcdt = fuchsia_compat_nodegroup_test;

namespace {

class TestCompositeDriver : public fdf::DriverBase2 {
 public:
  TestCompositeDriver() : fdf::DriverBase2("node_group") {}

  zx::result<> Start(fdf::DriverContext context) override {
    auto connect_result = context.incoming().Connect<fcdt::Waiter>();
    if (connect_result.is_error()) {
      fdf::error("Failed to start node-group driver: {}", connect_result);
      return connect_result.take_error();
    }

    const fidl::WireSharedClient<fcdt::Waiter> client{std::move(connect_result.value()),
                                                      dispatcher()};
    [[maybe_unused]] auto result = client->Ack(ZX_OK);

    return zx::ok();
  }
};

}  // namespace

FUCHSIA_DRIVER_EXPORT2(TestCompositeDriver);
