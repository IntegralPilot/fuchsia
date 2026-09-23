// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_TESTS_COMPOSITE_TEST_DRIVERS_COMPOSITE_DRIVER_H_
#define SRC_DEVICES_TESTS_COMPOSITE_TEST_DRIVERS_COMPOSITE_DRIVER_H_

#include <ddktl/device.h>

namespace test_composite_driver {

class TestCompositeDriver;

using DeviceType = ddk::Device<TestCompositeDriver>;

constexpr char kMetadataStr[] = "test-composite-metadata";

class TestCompositeDriver : public DeviceType {
 public:
  explicit TestCompositeDriver(zx_device_t* parent) : DeviceType(parent) {}

  static zx_status_t Bind(void* ctx, zx_device_t* device);

  void DdkUnbind(ddk::UnbindTxn txn) { txn.Reply(); }
  void DdkRelease() { delete this; }
};

}  // namespace test_composite_driver

#endif  // SRC_DEVICES_TESTS_COMPOSITE_TEST_DRIVERS_COMPOSITE_DRIVER_H_
