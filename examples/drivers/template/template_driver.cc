// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "examples/drivers/template/template_driver.h"

#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>

#include <bind/fuchsia/test/cpp/bind.h>

namespace template_driver {

TemplateDriver::TemplateDriver() : DriverBase2("template_driver") {}

zx::result<> TemplateDriver::Start(fdf::DriverContext context) {
  // Instructions: Put driver initialization logic in this function, such as adding children
  // and setting up client-server transport connections.
  // If the initialization logic is asynchronous, prefer to override
  // DriverBase::Start(fdf::StartCompleter completer) over this function.

  // Set up an example child node for testing purposes
  auto child_name = "example_child";

  // Add a child node.
  auto properties = std::vector{fdf::MakeProperty2(bind_fuchsia_test::TEST_CHILD, "simple")};
  zx::result child_result = AddChild(child_name, properties, {});
  if (child_result.is_error()) {
    return child_result.take_error();
  }

  child_controller_.Bind(std::move(child_result.value()));
  return zx::ok();
}

void TemplateDriver::Stop(fdf::StopCompleter completer) {
  fdf::info(
      "TemplateDriver::Stop() invoked. This is called before "
      "the driver dispatchers are shutdown. Only implement this function "
      "if you need to manually clean up objects (ex/ unique_ptrs) in the driver dispatchers.");
  completer(zx::ok());
}

}  // namespace template_driver

FUCHSIA_DRIVER_EXPORT2(template_driver::TemplateDriver);
