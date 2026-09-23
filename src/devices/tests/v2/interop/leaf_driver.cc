// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.interop.test/cpp/wire.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>

namespace ft = fuchsia_interop_test;

namespace {

class LeafDriver : public fdf::DriverBase2 {
 public:
  LeafDriver() : fdf::DriverBase2("leaf") {}

  zx::result<> Start(fdf::DriverContext context) override {
    auto waiter = context.incoming().Connect<ft::Waiter>();
    if (waiter.is_error()) {
      take_node().reset();
      return waiter.take_error();
    }
    const fidl::WireSharedClient<ft::Waiter> client{std::move(waiter.value()), dispatcher()};
    auto result = client.sync()->Ack();
    if (!result.ok()) {
      take_node().reset();
      return zx::error(result.error().status());
    }

    return zx::ok();
  }
};

}  // namespace

FUCHSIA_DRIVER_EXPORT2(LeafDriver);
