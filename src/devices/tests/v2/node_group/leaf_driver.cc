// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.nodegroup.test/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

namespace ft = fuchsia_nodegroup_test;

namespace {

class LeafDriver : public fdf::DriverBase2 {
 public:
  LeafDriver() : fdf::DriverBase2("leaf") {}

  zx::result<> Start(fdf::DriverContext context) override {
    incoming_ = context.take_incoming();
    node_ = take_node();
    auto result = async::PostTask(dispatcher(), [this]() { RunAsync(); });
    if (result == ZX_OK) {
      return zx::ok();
    }

    return zx::error(result);
  }

  void RunAsync() {
    auto connect_result = incoming_->Connect<ft::Waiter>();
    if (connect_result.is_error()) {
      fdf::error("Failed to start leaf driver: {}", connect_result);
      node_.reset();
      return;
    }

    const fidl::WireSharedClient<ft::Waiter> client{std::move(connect_result.value()),
                                                    dispatcher()};
    auto work_result = DoWork(client);
    if (work_result.is_error()) {
      fdf::error("DoWork was not successful: {}", work_result);
      return;
    }

    fdf::info("Completed RunAsync successfully.");
  }

 private:
  zx::result<uint32_t> GetNumber(std::string_view instance) {
    auto device = incoming_->Connect<ft::Service::Device>(instance);
    if (device.status_value() != ZX_OK) {
      fdf::warn("Failed to connect to {}: {}", instance.data(), device);
      return device.take_error();
    }

    auto result = fidl::WireCall(*device)->GetNumber();
    if (result.status() != ZX_OK) {
      fdf::warn("Failed to call number on {}: {}", instance.data(), result.lossy_description());
      return zx::error(result.status());
    }
    return zx::ok(result.value().number);
  }

  zx::result<> DoWork(const fidl::WireSharedClient<ft::Waiter>& waiter) {
    // Check the left device.
    auto number = GetNumber("left");
    if (number.is_error()) {
      [[maybe_unused]] auto result = waiter->Ack(number.error_value());
      return zx::ok();
    }
    if (*number != 1) {
      fdf::error("Wrong number for left: expecting 1, saw {}", *number);
      [[maybe_unused]] auto result = waiter->Ack(ZX_ERR_INTERNAL);
      return zx::ok();
    }

    // Check the right device.
    number = GetNumber("right");
    if (number.is_error()) {
      [[maybe_unused]] auto result = waiter->Ack(number.error_value());
      return zx::ok();
    }
    if (*number != 2) {
      fdf::error("Wrong number for right: expecting 2, saw {}", *number);
      [[maybe_unused]] auto result = waiter->Ack(ZX_ERR_INTERNAL);
      return zx::ok();
    }

    // Check the optional device.
    number = GetNumber("opt");
    if (number.is_error()) {
      fdf::info("No 'opt' parent.");
    } else if (*number != 3) {
      fdf::error("Wrong number for opt: expecting 3, saw {}", *number);
      [[maybe_unused]] auto result = waiter->Ack(ZX_ERR_INTERNAL);
      return zx::ok();
    }

    // Check the default device (which is the left device).
    number = GetNumber("default");
    if (number.is_error()) {
      [[maybe_unused]] auto result = waiter->Ack(number.error_value());
      return zx::ok();
    }
    if (*number != 1) {
      fdf::error("Wrong number for default: expecting 1, saw {}", *number);
      [[maybe_unused]] auto result = waiter->Ack(ZX_ERR_INTERNAL);
      return zx::ok();
    }

    [[maybe_unused]] auto result = waiter->Ack(ZX_OK);
    return zx::ok();
  }

  std::unique_ptr<fdf::Namespace> incoming_;
  fidl::ClientEnd<fuchsia_driver_framework::Node> node_;
};

}  // namespace

FUCHSIA_DRIVER_EXPORT2(LeafDriver);
