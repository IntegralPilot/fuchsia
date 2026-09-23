// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WEAVE_ADAPTATION_TESTS_FAKE_GATT_SERVER_H_
#define SRC_CONNECTIVITY_WEAVE_ADAPTATION_TESTS_FAKE_GATT_SERVER_H_

#include <fuchsia/bluetooth/gatt2/cpp/fidl_test_base.h>

#include <gtest/gtest.h>

#include "fuchsia/bluetooth/cpp/fidl.h"
#include "fuchsia/bluetooth/gatt2/cpp/fidl.h"
#include "lib/async/default.h"

namespace weave::adaptation::testing {

using fuchsia::bluetooth::gatt2::Handle;
using fuchsia::bluetooth::gatt2::LocalService_WriteValue_Result;

// Fake implementation of the |fuchsia.bluetooth.gatt2.Server| capability.
class FakeGATTService : public fuchsia::bluetooth::gatt2::testing::Server_TestBase {
 public:
  static constexpr uint8_t kBtpConnectReqValue[] = {0x6E, 0x6C, 0x3, 0x0, 0x0, 0x0, 0x0, 0x0, 0x5};
  static constexpr Handle kCharacteristicHandle = Handle{1};
  static constexpr fuchsia::bluetooth::PeerId kPeerId{123456};

  void NotImplemented_(const std::string& name) override { FAIL() << name; }

  void PublishService(fuchsia::bluetooth::gatt2::ServiceInfo info,
                      fidl::InterfaceHandle<fuchsia::bluetooth::gatt2::LocalService> service,
                      PublishServiceCallback callback) override {
    local_service_.Bind(std::move(service), gatt_server_dispatcher_);
    local_service_.events().OnIndicateValue =
        [this](fuchsia::bluetooth::gatt2::ValueChangedParameters update, zx::eventpair confirm) {
          this->OnIndicateValue(std::move(update), std::move(confirm));
        };
    callback(fpromise::ok());
  }

  fidl::InterfaceRequestHandler<fuchsia::bluetooth::gatt2::Server> GetHandler(
      async_dispatcher_t* dispatcher) {
    gatt_server_dispatcher_ = dispatcher;
    return [this, dispatcher](fidl::InterfaceRequest<fuchsia::bluetooth::gatt2::Server> request) {
      binding_.Bind(std::move(request), dispatcher);
    };
  }

  void WriteRequest(
      fuchsia::bluetooth::gatt2::testing::LocalService_TestBase::WriteValueCallback callback) {
    fuchsia::bluetooth::gatt2::LocalServiceWriteValueRequest params;
    params.set_peer_id(kPeerId);
    params.set_handle(Handle{0});
    params.set_offset(0);
    params.set_value(
        std::vector<uint8_t>(std::begin(kBtpConnectReqValue), std::end(kBtpConnectReqValue)));

    local_service_->WriteValue(std::move(params), std::move(callback));
  }

  void OnCharacteristicConfiguration() {
    local_service_->CharacteristicConfiguration(kPeerId, kCharacteristicHandle, false /* notify */,
                                                true /* indicate */, []() {});
  }

  void OnIndicateValue(fuchsia::bluetooth::gatt2::ValueChangedParameters update,
                       zx::eventpair confirmation) {
    gatt_subscribe_confirmed_ = true;
    confirmation.signal_peer(/*clear_mask=*/0, ZX_EVENTPAIR_SIGNALED);
  }

  bool WeaveConnectionConfirmed() const { return gatt_subscribe_confirmed_; }

 private:
  fidl::Binding<fuchsia::bluetooth::gatt2::Server> binding_{this};
  async_dispatcher_t* gatt_server_dispatcher_;
  fuchsia::bluetooth::gatt2::LocalServicePtr local_service_;
  // If a GATT indication has been received.
  bool gatt_subscribe_confirmed_{false};
};

}  // namespace weave::adaptation::testing

#endif  // SRC_CONNECTIVITY_WEAVE_ADAPTATION_TESTS_FAKE_GATT_SERVER_H_
