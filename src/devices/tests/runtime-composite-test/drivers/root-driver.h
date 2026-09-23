// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_TESTS_RUNTIME_COMPOSITE_TEST_DRIVERS_ROOT_DRIVER_H_
#define SRC_DEVICES_TESTS_RUNTIME_COMPOSITE_TEST_DRIVERS_ROOT_DRIVER_H_

#include <fidl/fuchsia.runtime.composite.test/cpp/driver/fidl.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/fdf/cpp/dispatcher.h>

#include <ddktl/device.h>

namespace root_driver {

class RootDriver;

using DeviceType = ddk::Device<RootDriver>;

class RootDriver : public DeviceType,
                   public fdf::Server<fuchsia_runtime_composite_test::RuntimeCompositeProtocol> {
 public:
  explicit RootDriver(zx_device_t* parent, fdf_dispatcher_t* dispatcher = nullptr)
      : DeviceType(parent), dispatcher_(dispatcher) {
    if (dispatcher_->get()) {
      outgoing_ = fdf::OutgoingDirectory::Create(dispatcher_->get());
    }
  }

  // RuntimeCompositeProtocol implementation
  void Handshake(HandshakeCompleter::Sync& completer) override;

  static zx_status_t Bind(void* ctx, zx_device_t* dev);
  void DdkRelease();

 private:
  zx::result<fidl::ClientEnd<fuchsia_io::Directory>> RegisterRuntimeService();

  fdf::UnownedDispatcher dispatcher_;

  std::optional<fdf::OutgoingDirectory> outgoing_;
};

}  // namespace root_driver

#endif  // SRC_DEVICES_TESTS_RUNTIME_COMPOSITE_TEST_DRIVERS_ROOT_DRIVER_H_
