// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_POWER_CPP_TESTING_FAKE_ELEMENT_CONTROL_H_
#define LIB_DRIVER_POWER_CPP_TESTING_FAKE_ELEMENT_CONTROL_H_

#include <fidl/fuchsia.power.broker/cpp/test_base.h>
#include <lib/fidl/cpp/wire/channel.h>

#include "sdk/lib/driver/power/cpp/testing/fidl_test_base_default.h"

namespace fdf_power::testing {

using fuchsia_power_broker::ElementControl;

class FakeElementControl : public FidlTestBaseDefault<ElementControl> {
 public:
  FakeElementControl() = default;

 private:
  void RegisterDependencyToken(RegisterDependencyTokenRequest& request,
                               RegisterDependencyTokenCompleter::Sync& completer) override {
    completer.Reply(fit::ok());
  }
};

}  // namespace fdf_power::testing

#endif  // LIB_DRIVER_POWER_CPP_TESTING_FAKE_ELEMENT_CONTROL_H_
