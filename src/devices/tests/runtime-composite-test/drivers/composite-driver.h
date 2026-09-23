// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_TESTS_RUNTIME_COMPOSITE_TEST_DRIVERS_COMPOSITE_DRIVER_H_
#define SRC_DEVICES_TESTS_RUNTIME_COMPOSITE_TEST_DRIVERS_COMPOSITE_DRIVER_H_

#include <fidl/fuchsia.runtime.composite.test/cpp/driver/fidl.h>

#include <ddktl/device.h>

namespace composite_driver {

class CompositeDriver;

using DeviceType = ddk::Device<CompositeDriver, ddk::Initializable>;

constexpr char kMetadataStr[] = "composite-metadata";

class CompositeDriver : public DeviceType {
 public:
  explicit CompositeDriver(zx_device_t* parent) : DeviceType(parent) {}

  static zx_status_t Bind(void* ctx, zx_device_t* device);

  void DdkInit(ddk::InitTxn txn);
  void DdkUnbind(ddk::UnbindTxn txn) { txn.Reply(); }
  void DdkRelease() { delete this; }

 private:
  fdf::Client<fuchsia_runtime_composite_test::RuntimeCompositeProtocol> client_;
};

}  // namespace composite_driver

#endif  // SRC_DEVICES_TESTS_RUNTIME_COMPOSITE_TEST_DRIVERS_COMPOSITE_DRIVER_H_
