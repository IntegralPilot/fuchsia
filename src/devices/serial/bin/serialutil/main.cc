// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.serial/cpp/fidl.h>

#include "serialutil.h"

int main(int argc, char* argv[]) {
  // Setup an empty connection as this isn't a test.
  fidl::ClientEnd<fuchsia_hardware_serial::DeviceProxy> device_for_test{};
  return serial::SerialUtil::Execute(argc, argv, std::move(device_for_test));
}
