// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_POWER_CPP_TESTING_FAKE_TOPOLOGY_H_
#define LIB_DRIVER_POWER_CPP_TESTING_FAKE_TOPOLOGY_H_

#include <fidl/fuchsia.power.broker/cpp/test_base.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fpromise/bridge.h>

#include "sdk/lib/driver/power/cpp/testing/fidl_test_base_default.h"

namespace fdf_power::testing {

using fuchsia_power_broker::ElementSchema;
using fuchsia_power_broker::LeaseSchema;
using fuchsia_power_broker::Topology;

class FakeTopology : public FidlTestBaseDefault<Topology> {
 public:
  FakeTopology() = default;

  // Takes a promise containing the next ElementSchema sent to this server via AddElement. This
  // promise can then be processed on a different async dispatcher (possibly on a different thread)
  // from the one used by this server.
  fpromise::promise<ElementSchema, void> TakeElementSchemaPromise() {
    return element_schema_bridge_.consumer.promise();
  }

  fpromise::promise<LeaseSchema, void> TakeLeaseSchemaPromise() {
    return lease_schema_bridge_.consumer.promise();
  }

 private:
  void AddElement(ElementSchema& schema, AddElementCompleter::Sync& completer) override {
    element_schema_bridge_.completer.complete_ok(std::move(schema));
    completer.Reply(fit::ok());
  }

  void Lease(LeaseSchema& schema, LeaseCompleter::Sync& completer) override {
    lease_schema_bridge_.completer.complete_ok(std::move(schema));
    completer.Reply(fit::ok());
  }

  fpromise::bridge<ElementSchema, void> element_schema_bridge_;
  fpromise::bridge<LeaseSchema, void> lease_schema_bridge_;
};

}  // namespace fdf_power::testing

#endif  // LIB_DRIVER_POWER_CPP_TESTING_FAKE_TOPOLOGY_H_
