// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>

#include "src/devices/tests/v2/reload-driver/driver_helpers.h"

namespace helpers = reload_test_driver_helpers;

namespace {

class LeafDriver : public fdf::DriverBase2 {
 public:
  LeafDriver() : fdf::DriverBase2("leaf") {}

  zx::result<> Start(fdf::DriverContext context) override {
    auto incoming_ptr = std::shared_ptr<fdf::Namespace>(context.take_incoming());
    return helpers::SendAck(logger(), context.node_name().value_or("None"), incoming_ptr, name());
  }
};

}  // namespace

FUCHSIA_DRIVER_EXPORT2(LeafDriver);
