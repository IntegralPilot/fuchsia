// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "passthrough.h"

#include <lib/driver/testing/cpp/driver_test.h>

#include <gtest/gtest.h>

namespace {

class HciTransportServer : public fidl::WireServer<fuchsia_hardware_bluetooth::HciTransport> {
 public:
  fuchsia_hardware_bluetooth::HciService::InstanceHandler GetInstanceHandler() {
    fuchsia_hardware_bluetooth::HciService::InstanceHandler handler;
    zx::result result = handler.add_hci_transport(bindings_.CreateHandler(
        this, fdf::Dispatcher::GetCurrent()->async_dispatcher(), fidl::kIgnoreBindingClosure));
    ZX_ASSERT(result.is_ok());
    return handler;
  }

  std::vector<std::vector<uint8_t>>& sent_command_packets() { return sent_command_packets_; }

 private:
  // WireServer<HciTransport> overrides:
  void Send(::fuchsia_hardware_bluetooth::wire::SentPacket* request,
            SendCompleter::Sync& completer) override {
    if (request->is_command()) {
      sent_command_packets_.emplace_back(request->command().begin(), request->command().end());
      completer.Reply();
    }
  }
  void AckReceive(AckReceiveCompleter::Sync& completer) override {}
  void ConfigureSco(::fuchsia_hardware_bluetooth::wire::HciTransportConfigureScoRequest* request,
                    ConfigureScoCompleter::Sync& completer) override {}
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_bluetooth::HciTransport> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

  std::vector<std::vector<uint8_t>> sent_command_packets_;

  fidl::ServerBindingGroup<fuchsia_hardware_bluetooth::HciTransport> bindings_;
};

}  // namespace

class PassthroughTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    auto hci_proto_handler = hci_server_.GetInstanceHandler();
    return to_driver_vfs.AddService<fuchsia_hardware_bluetooth::HciService>(
        std::move(hci_proto_handler));
  }

  HciTransportServer& hci_server() { return hci_server_; }

 private:
  HciTransportServer hci_server_;
};

class TestConfig final {
 public:
  using DriverType = bt::passthrough::PassthroughDevice;
  using EnvironmentType = PassthroughTestEnvironment;
};

class PassthroughTest : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  fdf_testing::ForegroundDriverTest<TestConfig>& driver_test() { return driver_test_; }

  async_dispatcher_t* dispatcher() { return fdf::Dispatcher::GetCurrent()->async_dispatcher(); }

 private:
  fdf_testing::ForegroundDriverTest<TestConfig> driver_test_;
};

TEST_F(PassthroughTest, Lifecycle) {}

TEST_F(PassthroughTest, ConnectAndSendCommand) {
  zx::result<fidl::ClientEnd<fuchsia_hardware_bluetooth::Vendor>> client_end =
      driver_test().Connect<fuchsia_hardware_bluetooth::Service::Vendor>();
  ASSERT_TRUE(client_end.is_ok());
  fidl::WireClient<fuchsia_hardware_bluetooth::Vendor> client(std::move(client_end.value()),
                                                              dispatcher());

  std::optional<fidl::ClientEnd<fuchsia_hardware_bluetooth::HciTransport>> hci_client_end;
  client->OpenHciTransport().Then(
      [&](fidl::WireUnownedResult<fuchsia_hardware_bluetooth::Vendor::OpenHciTransport>& result) {
        ASSERT_TRUE(result->is_ok());
        hci_client_end = std::move(result->value()->channel);
      });
  driver_test().runtime().RunUntilIdle();
  ASSERT_TRUE(hci_client_end);

  fidl::WireClient<fuchsia_hardware_bluetooth::HciTransport> hci_client(std::move(*hci_client_end),
                                                                        dispatcher());
  uint8_t arr[1] = {3};
  fidl::VectorView vec_view = fidl::VectorView<uint8_t>::FromExternal(arr);
  fidl::ObjectView object_view =
      fidl::ObjectView<fidl::VectorView<uint8_t>>::FromExternal(&vec_view);
  hci_client->Send(::fuchsia_hardware_bluetooth::wire::SentPacket::WithCommand(object_view))
      .Then([&](fidl::WireUnownedResult<fuchsia_hardware_bluetooth::HciTransport::Send>& result) {
        driver_test().runtime().Quit();
      });

  driver_test().runtime().Run();

  driver_test().RunInEnvironmentTypeContext([&](PassthroughTestEnvironment& env) {
    ASSERT_EQ(env.hci_server().sent_command_packets().size(), 1u);
    EXPECT_EQ(env.hci_server().sent_command_packets()[0][0], arr[0]);
  });

  auto endpoint = hci_client.UnbindMaybeGetEndpoint();
}
