// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_POWER_DRIVERS_FUSB302_FUSB302_IDENTITY_H_
#define SRC_DEVICES_POWER_DRIVERS_FUSB302_FUSB302_IDENTITY_H_

#include <fidl/fuchsia.hardware.i2c/cpp/wire.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <lib/zx/result.h>

namespace fusb302 {

// Owns the device's Identity* register.
//
// This class reports the FUSB302 chip variant via logging and Inspect.
class Fusb302Identity {
 public:
  // `i2c_channel` must remain alive throughout the new instance's lifetime.
  explicit Fusb302Identity(fidl::ClientEnd<fuchsia_hardware_i2c::Device>& i2c_channel,
                           inspect::Node root_node);

  Fusb302Identity(const Fusb302Identity&) = delete;
  Fusb302Identity& operator=(const Fusb302Identity&) = delete;

  // Trivially destructible.
  ~Fusb302Identity() = default;

  // Populates Inspect data with the FUSB302 chip version info.
  zx::result<> ReadIdentity();

 private:
  // The referenced instances are guaranteed to outlive this instance, because
  // they're owned by this instance's owner (Fusb302).
  fidl::ClientEnd<fuchsia_hardware_i2c::Device>& i2c_;

  inspect::Node root_node_;
};

}  // namespace fusb302

#endif  // SRC_DEVICES_POWER_DRIVERS_FUSB302_FUSB302_IDENTITY_H_
