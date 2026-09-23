// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "fan-controller.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/fdio/directory.h>
#include <unistd.h>

#include <queue>

#include <zxtest/zxtest.h>

namespace {

class FakeWatcher : public fidl::Server<fuchsia_thermal::ClientStateWatcher> {
 public:
  explicit FakeWatcher(fidl::ServerEnd<fuchsia_thermal::ClientStateWatcher> server)
      : binding_(async_get_default_dispatcher(), std::move(server), this,
                 fidl::kIgnoreBindingClosure) {}
  ~FakeWatcher() {
    if (completer_) {
      completer_->Close(ZX_ERR_CANCELED);
      completer_.reset();
    }
  }

  void Watch(WatchCompleter::Sync& completer) override { completer_ = completer.ToAsync(); }

 private:
  friend class FakeClientStateServer;

  fidl::ServerBinding<fuchsia_thermal::ClientStateWatcher> binding_;

  std::optional<WatchCompleter::Async> completer_;
};

class FakeClientStateServer : public fidl::Server<fuchsia_thermal::ClientStateConnector> {
 public:
  explicit FakeClientStateServer(fidl::ServerEnd<fuchsia_thermal::ClientStateConnector> server)
      : binding_(async_get_default_dispatcher(), std::move(server), this,
                 fidl::kIgnoreBindingClosure) {}
  ~FakeClientStateServer() override { EXPECT_TRUE(expected_connect_.empty()); }

  void ExpectConnect(const std::string& client_type) { expected_connect_.emplace(client_type); }
  void Connect(ConnectRequest& request, ConnectCompleter::Sync& completer) override {
    EXPECT_FALSE(expected_connect_.empty());
    EXPECT_STREQ(expected_connect_.front(), request.client_type());
    expected_connect_.pop();
    watchers_.emplace(request.client_type(), std::move(request.watcher()));
  }

  void ReplyToWatch(const std::string& client_type, uint64_t state) {
    auto& watcher = watchers_.at(client_type);
    ASSERT_TRUE(watcher.completer_.has_value());
    watcher.completer_->Reply(state);
    watcher.completer_.reset();
  }

  bool watch_called(const std::string& client_type) {
    return watchers_.find(client_type) != watchers_.end() &&
           watchers_.at(client_type).completer_.has_value();
  }

 private:
  fidl::ServerBinding<fuchsia_thermal::ClientStateConnector> binding_;

  std::queue<std::string> expected_connect_;
  std::map<std::string, FakeWatcher> watchers_;
};

class FakeFanDevice : public fidl::Server<fuchsia_hardware_fan::Device> {
 public:
  explicit FakeFanDevice(std::string client_type) : client_type_(std::move(client_type)) {}
  ~FakeFanDevice() override { EXPECT_TRUE(expected_set_fan_level_.empty()); }

  // fuchsia_hardware_fan.Device protocol implementation.
  void GetFanLevel(GetFanLevelCompleter::Sync& completer) override {
    completer.Reply({ZX_ERR_NOT_SUPPORTED, 0});
  }
  void SetFanLevel(SetFanLevelRequest& request, SetFanLevelCompleter::Sync& completer) override {
    EXPECT_FALSE(expected_set_fan_level_.empty());
    EXPECT_EQ(expected_set_fan_level_.front(), request.fan_level());
    expected_set_fan_level_.pop();
    completer.Reply(ZX_OK);
  }
  void GetClientType(GetClientTypeCompleter::Sync& completer) override {
    completer.Reply({client_type_});
  }

  fidl::ProtocolHandler<fuchsia_hardware_fan::Device> handler() {
    return bindings_.CreateHandler(this, async_get_default_dispatcher(),
                                   fidl::kIgnoreBindingClosure);
  }

  void ExpectSetFanLevel(uint32_t level) { expected_set_fan_level_.emplace(level); }

 private:
  fidl::ServerBindingGroup<fuchsia_hardware_fan::Device> bindings_;
  const std::string client_type_;

  std::queue<uint32_t> expected_set_fan_level_;
};

class FanControllerTest : public zxtest::Test {
 public:
  FanControllerTest() {
    background_loop_.StartThread("background-loop");

    auto endpoints = fidl::Endpoints<fuchsia_thermal::ClientStateConnector>::Create();
    client_state_.emplace(std::move(endpoints.server));
    client_end_ = std::move(endpoints.client);
  }

  void StartFanController() {
    auto [root_client, root_request] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ASSERT_OK(outgoing_.SyncCall([root_request = std::move(root_request)](auto* outgoing) mutable {
      return outgoing->Serve(std::move(root_request));
    }));

    auto svc_client = component::OpenDirectoryAt(root_client, "svc");
    ASSERT_OK(svc_client);

    fan_controller_.emplace(loop_.dispatcher(), std::move(client_end_), std::move(*svc_client));
  }

  // Holds a ref to the outgoing directory that removes the instance when this object goes out of
  // scope.
  class FakeDevice {
   public:
    FakeDevice(std::string name, async_dispatcher_t* dispatcher,
               async_patterns::TestDispatcherBound<component::OutgoingDirectory>& outgoing,
               const std::string& client_type)
        : name_(std::move(name)),
          outgoing_(outgoing),
          fan_(dispatcher, std::in_place, client_type) {
      auto handler = fan_.SyncCall([&](FakeFanDevice* fan) { return fan->handler(); });
      ASSERT_OK(outgoing_.SyncCall([this, handler = std::move(handler)](
                                       component::OutgoingDirectory* outgoing) mutable {
        return outgoing->AddService<fuchsia_hardware_fan::Service>(
            fuchsia_hardware_fan::Service::InstanceHandler({.device = std::move(handler)}), name_);
      }));
    }
    ~FakeDevice() {
      std::ignore = outgoing_.SyncCall([this](component::OutgoingDirectory* outgoing) {
        return outgoing->RemoveService<fuchsia_hardware_fan::Service>(name_);
      });
    }

    void ExpectSetFanLevel(uint32_t level) {
      fan_.SyncCall(&FakeFanDevice::ExpectSetFanLevel, level);
    }

   private:
    std::string name_;
    async_patterns::TestDispatcherBound<component::OutgoingDirectory>& outgoing_;
    async_patterns::TestDispatcherBound<FakeFanDevice> fan_;
  };

  FakeDevice AddDevice(const std::string& client_type) {
    return FakeDevice(client_type + std::to_string(next_device_number_++),
                      background_loop_.dispatcher(), outgoing_, client_type);
  }

  void WaitForDevice(const std::string& client_type, size_t count) {
    while (fan_controller_->controller_fan_count(client_type) != count) {
      loop_.RunUntilIdle();
    }
    while (!client_state_.SyncCall(&FakeClientStateServer::watch_called, client_type)) {
      loop_.RunUntilIdle();
    }
  }

  void ReplyToWatch(const std::string& client_type, uint32_t state) {
    while (!client_state_.SyncCall(&FakeClientStateServer::watch_called, client_type)) {
      loop_.RunUntilIdle();
    }
    client_state_.SyncCall(&FakeClientStateServer::ReplyToWatch, client_type, state);
    while (!client_state_.SyncCall(&FakeClientStateServer::watch_called, client_type)) {
      loop_.RunUntilIdle();
    }
  }

 private:
  // Foreground loop for the test thread/fan controller
  async::Loop loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  // Background loop for the fake fans and outgoing directory.
  async::Loop background_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};

  fidl::ClientEnd<fuchsia_thermal::ClientStateConnector> client_end_;

  async_patterns::TestDispatcherBound<component::OutgoingDirectory> outgoing_{
      background_loop_.dispatcher(), std::in_place, async_patterns::PassDispatcher};
  uint32_t next_device_number_ = 0;
  std::map<std::string, size_t> client_type_count_;

 protected:
  std::optional<fan_controller::FanController> fan_controller_;
  async_patterns::TestDispatcherBound<FakeClientStateServer> client_state_{
      background_loop_.dispatcher()};
};

TEST_F(FanControllerTest, DeviceBeforeStart) {
  const std::string kClientType = "fan";
  client_state_.SyncCall(&FakeClientStateServer::ExpectConnect, kClientType);
  auto dev = AddDevice(kClientType);

  StartFanController();
  WaitForDevice(kClientType, 1);

  dev.ExpectSetFanLevel(3);
  ReplyToWatch(kClientType, 3);
}

TEST_F(FanControllerTest, DeviceAfterStart) {
  // We need to add and remove a device to ensure the service directory exists.
  auto _ = AddDevice("initialize");

  StartFanController();

  const std::string kClientType = "fan";
  client_state_.SyncCall(&FakeClientStateServer::ExpectConnect, kClientType);
  auto dev = AddDevice(kClientType);
  WaitForDevice(kClientType, 1);

  dev.ExpectSetFanLevel(3);
  ReplyToWatch(kClientType, 3);
}

TEST_F(FanControllerTest, MultipleDevicesSameClientType) {
  // We need to add and remove a device to ensure the service directory exists.
  auto _ = AddDevice("initialize");

  StartFanController();

  const std::string kClientType = "fan";
  client_state_.SyncCall(&FakeClientStateServer::ExpectConnect, kClientType);
  auto dev0 = AddDevice(kClientType);
  WaitForDevice(kClientType, 1);

  auto dev1 = AddDevice(kClientType);
  WaitForDevice(kClientType, 2);

  dev0.ExpectSetFanLevel(3);
  dev1.ExpectSetFanLevel(3);
  ReplyToWatch(kClientType, 3);
}

TEST_F(FanControllerTest, MultipleDevicesDifferentClientTypes) {
  // We need to add and remove a device to ensure the service directory exists.
  auto _ = AddDevice("initialize");

  StartFanController();

  const std::string kClientType0 = "fan0";
  client_state_.SyncCall(&FakeClientStateServer::ExpectConnect, kClientType0);
  auto dev0 = AddDevice(kClientType0);
  WaitForDevice(kClientType0, 1);

  const std::string kClientType1 = "fan1";
  client_state_.SyncCall(&FakeClientStateServer::ExpectConnect, kClientType1);
  auto dev1 = AddDevice(kClientType1);
  WaitForDevice(kClientType1, 1);

  dev0.ExpectSetFanLevel(3);
  ReplyToWatch(kClientType0, 3);

  dev1.ExpectSetFanLevel(2);
  ReplyToWatch(kClientType1, 2);
}

TEST_F(FanControllerTest, DeviceRemoval) {
  const std::string kClientType = "fan";
  client_state_.SyncCall(&FakeClientStateServer::ExpectConnect, kClientType);
  auto dev0 = AddDevice(kClientType);

  StartFanController();
  WaitForDevice(kClientType, 1);

  {
    auto dev1 = AddDevice(kClientType);
    WaitForDevice(kClientType, 2);

    dev0.ExpectSetFanLevel(3);
    dev1.ExpectSetFanLevel(3);
    ReplyToWatch(kClientType, 3);

    // Remove fan1 by letting it go out of scope. This expects an error log.
  }

  dev0.ExpectSetFanLevel(6);
  ReplyToWatch(kClientType, 6);
  EXPECT_EQ(fan_controller_->controller_fan_count(kClientType), 1);
}

}  // namespace
