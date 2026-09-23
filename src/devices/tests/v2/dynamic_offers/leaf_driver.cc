// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.offers.test/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

namespace ft = fuchsia_offers_test;

namespace {

class LeafDriver : public fdf::DriverBase2 {
 public:
  LeafDriver() : fdf::DriverBase2("leaf") {}

  zx::result<> Start(fdf::DriverContext context) override {
    auto handshake = context.incoming().Connect<ft::Service::Device>();
    if (handshake.is_error()) {
      return handshake.take_error();
    }
    handshake_.Bind(*std::move(handshake), dispatcher());

    auto waiter = context.incoming().Connect<ft::Waiter>();
    if (waiter.is_error()) {
      return waiter.take_error();
    }
    waiter_.Bind(*std::move(waiter), dispatcher());

    AsyncCallDoThenAck();
    return zx::ok();
  }

 private:
  void AsyncCallDoThenAck() {
    auto callback = [this](fidl::Result<ft::Handshake::Do>& result) mutable {
      if (!result.is_ok()) {
        fdf::error("Handshake Do failed: {}", result.error_value().status_string());
        UnbindNode(result.error_value().status());
        return;
      }
      auto ack_result = waiter_->Ack();
      if (ack_result.is_error()) {
        fdf::error("Ack failed: {}", ack_result.error_value().status_string());
        UnbindNode(result.error_value().status());
      }
    };
    handshake_->Do().Then(std::move(callback));
  }

  void UnbindNode(const zx_status_t& status) {
    fdf::error("Failed to start leaf driver: {}", zx_status_get_string(status));
    take_node().reset();
  }

  fidl::SharedClient<ft::Handshake> handshake_;
  fidl::SharedClient<ft::Waiter> waiter_;
};

}  // namespace

FUCHSIA_DRIVER_EXPORT2(LeafDriver);
