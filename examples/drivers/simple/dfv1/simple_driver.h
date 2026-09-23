// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef EXAMPLES_DRIVERS_SIMPLE_DFV1_SIMPLE_DRIVER_H_
#define EXAMPLES_DRIVERS_SIMPLE_DFV1_SIMPLE_DRIVER_H_

#include <ddktl/device.h>

namespace simple {

class SimpleDriver;

class SimpleDriver;
using DeviceType = ddk::Device<SimpleDriver, ddk::Initializable, ddk::Suspendable, ddk::Unbindable>;

class SimpleDriver : public DeviceType {
 public:
  explicit SimpleDriver(zx_device_t* parent);
  ~SimpleDriver();

  static zx_status_t Bind(void* ctx, zx_device_t* dev);

  // Device protocol implementation.
  void DdkInit(ddk::InitTxn txn);
  void DdkSuspend(ddk::SuspendTxn txn);
  void DdkUnbind(ddk::UnbindTxn txn);
  void DdkRelease();
};

}  // namespace simple

#endif  // EXAMPLES_DRIVERS_SIMPLE_DFV1_SIMPLE_DRIVER_H_
