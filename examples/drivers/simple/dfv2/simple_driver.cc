// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "simple_driver.h"

#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>

#include <bind/fuchsia/test/cpp/bind.h>

namespace simple {

SimpleDriver::SimpleDriver() : DriverBase2("simple_driver") {
  // This constructor is only implemented to demonstrate the driver lifecycle.
  // Drivers are not expected to add implementation in the constructor.
}

SimpleDriver::~SimpleDriver() {
  fdf::info(
      "SimpleDriver destructor invoked. This is called after Stop() is called and "
      "all driver dispatchers are shutdown. Use the destructor to perform any remaining teardowns.");
}

zx::result<> SimpleDriver::Start(fdf::DriverContext context) {
  fdf::info(
      "SimpleDriver::Start() invoked. In this function, perform the driver "
      "initialization, such as adding children and setting up the compat server.");

  auto incoming_ptr = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  auto child_name = "simple_child";

  // Initialize our compat server.
  {
    zx::result<> result = compat_server_.Initialize(incoming_ptr, outgoing(), context.node_name(),
                                                    child_name, compat::ForwardMetadata::None());
    if (result.is_error()) {
      return result.take_error();
    }
  }

  // [START add_child]
  // Add a child node.
  auto properties = std::vector{fdf::MakeProperty2(bind_fuchsia_test::TEST_CHILD, "simple")};
  zx::result child_result = AddChild(child_name, properties, compat_server_.CreateOffers2());
  if (child_result.is_error()) {
    return child_result.take_error();
  }

  child_controller_.Bind(std::move(child_result.value()));
  // [END add_child]

  // [START add_owned_child]
  // Add an owned child node.
  zx::result owned_child_result = AddOwnedChild("owned_child");
  if (owned_child_result.is_error()) {
    fdf::error("Failed to add owned child: {}", owned_child_result);
    return owned_child_result.take_error();
  }
  owned_child_ = std::move(owned_child_result.value());
  // [END add_owned_child]
  return zx::ok();
}

void SimpleDriver::Stop(fdf::StopCompleter completer) {
  fdf::info(
      "SimpleDriver::Stop() invoked. This is called before "
      "the driver dispatchers are shutdown. Only implement this function "
      "if you need to manually clean up objects (ex/ unique_ptrs) in the driver dispatchers.");
  completer(zx::ok());
}

}  // namespace simple

FUCHSIA_DRIVER_EXPORT2(simple::SimpleDriver);
