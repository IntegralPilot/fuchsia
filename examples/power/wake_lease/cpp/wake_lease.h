// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef EXAMPLES_POWER_WAKE_LEASE_CPP_WAKE_LEASE_H_
#define EXAMPLES_POWER_WAKE_LEASE_CPP_WAKE_LEASE_H_

#include <fidl/fuchsia.power.system/cpp/fidl.h>
#include <lib/fidl/cpp/client.h>
#include <lib/fpromise/promise.h>

#include <string>

namespace examples::power {

using Error = std::string;

class WakeLease {
 public:
  // Constructs a WakeLease object after awaiting a response from ActivityGovernor. The lease will
  // remain active until this object is destroyed. The ActivityGovernor client should outlive the
  // returned WakeLease object.
  static fpromise::promise<WakeLease, Error> Take(
      const fidl::Client<fuchsia_power_system::ActivityGovernor>& client, const std::string& name);

  // Destroying this WakeLease will destroy the underlying LeaseToken, thereby removing the
  // lease with the server.
  ~WakeLease() = default;

  // Move constructor and assignment operator are required for underlying implementation.
  WakeLease(WakeLease&&) = default;
  WakeLease& operator=(WakeLease&&) = default;

 private:
  explicit WakeLease(fuchsia_power_system::LeaseToken token) : token_(std::move(token)) {}

  fuchsia_power_system::LeaseToken token_;
};

}  // namespace examples::power

#endif  // EXAMPLES_POWER_WAKE_LEASE_CPP_WAKE_LEASE_H_
