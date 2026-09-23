// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_TESTS_V2_RELOAD_DRIVER_DRIVER_HELPERS_H_
#define SRC_DEVICES_TESTS_V2_RELOAD_DRIVER_DRIVER_HELPERS_H_

#include <fidl/fuchsia.driver.framework/cpp/natural_messaging.h>
#include <lib/driver/component/cpp/driver_base.h>

namespace reload_test_driver_helpers {

zx::result<fidl::ClientEnd<fuchsia_driver_framework::NodeController>> AddChild(
    fdf::Logger& logger, std::string_view child_name,
    fidl::SyncClient<fuchsia_driver_framework::Node>& node_client,
    std::string_view child_test_property);

zx::result<> SendAck(fdf::Logger& logger, std::string_view node_name,
                     const std::shared_ptr<fdf::Namespace>& incoming, std::string_view driver_name);

}  // namespace reload_test_driver_helpers

#endif  // SRC_DEVICES_TESTS_V2_RELOAD_DRIVER_DRIVER_HELPERS_H_
