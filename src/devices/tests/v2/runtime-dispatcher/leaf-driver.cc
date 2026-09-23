// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.runtime.test/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

namespace ft = fuchsia_runtime_test;

namespace {

class LeafDriver : public fdf::DriverBase2 {
 public:
  LeafDriver() : fdf::DriverBase2("leaf") {}

  zx::result<> Start(fdf::DriverContext context) override {
    fdf::info("Start hook reached leaf");
    // Test we can block on the dispatcher thread.
    ZX_ASSERT(ZX_OK == DoHandshakeSynchronously(context.incoming()));

    auto waiter = context.incoming().Connect<ft::Waiter>();
    if (waiter.is_error()) {
      take_node().reset();
      fdf::info("failed to connect to waiter");
      return waiter.take_error();
    }

    const fidl::WireSharedClient<ft::Waiter> client(std::move(waiter.value()), dispatcher());
    auto result = client.sync()->Ack();
    if (!result.ok()) {
      take_node().reset();
      fdf::info("failed to ack waiter");
      return zx::error(result.error().status());
    }

    return zx::ok();
  }

 private:
  zx_status_t DoHandshakeSynchronously(const fdf::Namespace& incoming) {
    ZX_ASSERT((*driver_dispatcher()->options() & FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS) ==
              FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS);

    auto result = incoming.Connect<ft::Service::Handshake>();
    if (result.is_error()) {
      return result.status_value();
    }
    const fidl::WireSharedClient<ft::Handshake> client(std::move(*result), dispatcher());
    return client.sync()->Do().status();
  }
};

}  // namespace

FUCHSIA_DRIVER_EXPORT2(LeafDriver);
