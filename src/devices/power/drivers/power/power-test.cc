// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "power.h"

#include <fuchsia/hardware/powerimpl/c/banjo.h>
#include <fuchsia/hardware/powerimpl/cpp/banjo.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>

#include <memory>

#include <zxtest/zxtest.h>

#include "src/devices/testing/mock-ddk/mock-device.h"

namespace power {

class FakePower : public fidl::WireServer<fuchsia_hardware_power::Device> {
 public:
  void RegisterPowerDomain(RegisterPowerDomainRequestView request,
                           RegisterPowerDomainCompleter::Sync& completer) override {
    power_domain_registered_count_++;
    completer.ReplySuccess();
  }

  void UnregisterPowerDomain(UnregisterPowerDomainCompleter::Sync& completer) override {
    power_domain_unregistered_count_++;
    completer.ReplySuccess();
  }

  void GetPowerDomainStatus(GetPowerDomainStatusCompleter::Sync& completer) override {
    completer.ReplySuccess(::fuchsia_hardware_power::wire::PowerDomainStatus::kEnabled);
  }

  void GetSupportedVoltageRange(GetSupportedVoltageRangeCompleter::Sync& completer) override {
    completer.ReplySuccess(0, 10);
  }

  void RequestVoltage(RequestVoltageRequestView request,
                      RequestVoltageCompleter::Sync& completer) override {
    completer.ReplySuccess(request->voltage);
  }

  void GetCurrentVoltage(GetCurrentVoltageRequestView request,
                         GetCurrentVoltageCompleter::Sync& completer) override {
    completer.ReplySuccess(0);
  }

  void WritePmicCtrlReg(WritePmicCtrlRegRequestView request,
                        WritePmicCtrlRegCompleter::Sync& completer) override {
    completer.ReplySuccess();
  }

  void ReadPmicCtrlReg(ReadPmicCtrlRegRequestView request,
                       ReadPmicCtrlRegCompleter::Sync& completer) override {
    completer.ReplySuccess(0);
  }

  uint32_t power_domain_registered_count() { return power_domain_registered_count_; }
  uint32_t power_domain_unregistered_count() { return power_domain_unregistered_count_; }

  zx::result<fidl::ClientEnd<fuchsia_hardware_power::Device>> BindServer() {
    zx::result endpoints = fidl::CreateEndpoints<fuchsia_hardware_power::Device>();
    if (endpoints.is_error()) {
      return endpoints.take_error();
    }
    binding_.AddBinding(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                        std::move(endpoints->server), this, fidl::kIgnoreBindingClosure);
    return zx::ok(std::move(endpoints->client));
  }

 private:
  fidl::ServerBindingGroup<fuchsia_hardware_power::Device> binding_;
  uint32_t power_domain_registered_count_ = 0;
  uint32_t power_domain_unregistered_count_ = 0;
};

class FakePowerImpl : public ddk::PowerImplProtocol<FakePowerImpl> {
 public:
  FakePowerImpl() : proto_{.ops = &power_impl_protocol_ops_, .ctx = this} {}
  ddk::PowerImplProtocolClient GetClient() const { return ddk::PowerImplProtocolClient(&proto_); }
  zx_status_t PowerImplGetCurrentVoltage(uint32_t index, uint32_t* current_voltage) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  zx_status_t PowerImplDisablePowerDomain(uint32_t index) {
    power_domain_disabled_count_++;
    return ZX_OK;
  }

  zx_status_t PowerImplEnablePowerDomain(uint32_t index) {
    power_domain_enabled_count_++;
    return ZX_OK;
  }

  zx_status_t PowerImplGetPowerDomainStatus(uint32_t index, power_domain_status_t* out_status) {
    return ZX_OK;
  }

  zx_status_t PowerImplGetSupportedVoltageRange(uint32_t index, uint32_t* min_voltage,
                                                uint32_t* max_voltage) {
    return ZX_OK;
  }

  zx_status_t PowerImplRequestVoltage(uint32_t index, uint32_t voltage, uint32_t* actual_voltage) {
    *actual_voltage = voltage;
    return ZX_OK;
  }
  zx_status_t PowerImplWritePmicCtrlReg(uint32_t index, uint32_t reg_addr, uint32_t value) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  zx_status_t PowerImplReadPmicCtrlReg(uint32_t index, uint32_t addr, uint32_t* value) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  uint32_t power_domain_enabled_count() { return power_domain_enabled_count_; }

  uint32_t power_domain_disabled_count() { return power_domain_disabled_count_; }

 private:
  power_impl_protocol_t proto_;
  uint32_t power_domain_enabled_count_ = 0;
  uint32_t power_domain_disabled_count_ = 0;
};

class GenericPowerTest : public zxtest::Test {
 public:
  explicit GenericPowerTest() {}
  void SetUp() override {
    power_impl_ = std::make_unique<FakePowerImpl>();

    zx::result parent_power_client = parent_power_.SyncCall(&FakePower::BindServer);
    ASSERT_OK(parent_power_client);

    dut_ = std::make_unique<PowerDomain>(fake_parent_.get(), 0, power_impl_->GetClient(),
                                         std::move(parent_power_client.value()), 10, 1000, false);
  }

  fidl::ClientEnd<fuchsia_hardware_power::Device> ConnectDut() {
    auto endpoints = fidl::Endpoints<fuchsia_hardware_power::Device>::Create();
    dut_->GetHandler()(std::move(endpoints.server));
    return std::move(endpoints.client);
  }

  static void RunSyncClientTask(fit::closure task) {
    // Spawn a separate thread to run the client task using an async::Loop.
    async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};
    loop.StartThread();
    zx::result result;
    ASSERT_NO_FAILURES(result = fdf::RunOnDispatcherSync(loop.dispatcher(), std::move(task)));
    ASSERT_EQ(ZX_OK, result.status_value());
  }

 protected:
  std::shared_ptr<MockDevice> fake_parent_ = MockDevice::FakeRootParent();
  fdf::UnownedSynchronizedDispatcher env_dispatcher_ =
      fdf_testing::DriverRuntime::GetInstance()->StartBackgroundDispatcher();
  std::unique_ptr<PowerDomain> dut_;
  async_patterns::TestDispatcherBound<FakePower> parent_power_{env_dispatcher_->async_dispatcher(),
                                                               std::in_place};
  std::unique_ptr<FakePowerImpl> power_impl_;
};

TEST_F(GenericPowerTest, RegisterDomain) {
  fidl::WireSyncClient<fuchsia_hardware_power::Device> dut_client(ConnectDut());
  RunSyncClientTask([&dut_client]() {
    fidl::WireResult result = dut_client->RegisterPowerDomain(20, 800);
    ZX_ASSERT(result.ok() && result->is_ok());
  });
  EXPECT_EQ(dut_->GetDependentCount(), 1);
  EXPECT_EQ(parent_power_.SyncCall(&FakePower::power_domain_registered_count), 1);
  EXPECT_EQ(power_impl_->power_domain_enabled_count(), 1);
}

TEST_F(GenericPowerTest, RegisterTwice) {
  fidl::WireSyncClient<fuchsia_hardware_power::Device> dut_client(ConnectDut());
  RunSyncClientTask([&dut_client]() {
    fidl::WireResult result = dut_client->RegisterPowerDomain(20, 800);
    ZX_ASSERT(result.ok() && result->is_ok());
  });
  EXPECT_EQ(dut_->GetDependentCount(), 1);
  EXPECT_EQ(parent_power_.SyncCall(&FakePower::power_domain_registered_count), 1);
  EXPECT_EQ(power_impl_->power_domain_enabled_count(), 1);
  RunSyncClientTask([&dut_client]() {
    fidl::WireResult result = dut_client->RegisterPowerDomain(20, 800);
    ZX_ASSERT(result.ok() && result->is_ok());
  });
  EXPECT_EQ(dut_->GetDependentCount(), 1);
  EXPECT_EQ(parent_power_.SyncCall(&FakePower::power_domain_registered_count), 1);
  EXPECT_EQ(power_impl_->power_domain_enabled_count(), 1);
}

TEST_F(GenericPowerTest, UnregisterDomain) {
  fidl::WireSyncClient<fuchsia_hardware_power::Device> dut_client(ConnectDut());
  RunSyncClientTask([&dut_client]() {
    fidl::WireResult result = dut_client->RegisterPowerDomain(20, 800);
    ZX_ASSERT(result.ok() && result->is_ok());
  });
  EXPECT_EQ(dut_->GetDependentCount(), 1);
  EXPECT_EQ(parent_power_.SyncCall(&FakePower::power_domain_registered_count), 1);
  EXPECT_EQ(power_impl_->power_domain_enabled_count(), 1);
  RunSyncClientTask([&dut_client]() {
    fidl::WireResult result = dut_client->UnregisterPowerDomain();
    ZX_ASSERT(result.ok() && result->is_ok());
  });
  EXPECT_EQ(dut_->GetDependentCount(), 0);
  EXPECT_EQ(parent_power_.SyncCall(&FakePower::power_domain_unregistered_count), 1);
  EXPECT_EQ(power_impl_->power_domain_disabled_count(), 1);
}

TEST_F(GenericPowerTest, UnregisterTwice) {
  fidl::WireSyncClient<fuchsia_hardware_power::Device> dut_client(ConnectDut());
  RunSyncClientTask([&dut_client]() {
    fidl::WireResult result = dut_client->RegisterPowerDomain(20, 800);
    ZX_ASSERT(result.ok() && result->is_ok());
  });
  EXPECT_EQ(dut_->GetDependentCount(), 1);
  EXPECT_EQ(parent_power_.SyncCall(&FakePower::power_domain_registered_count), 1);
  EXPECT_EQ(power_impl_->power_domain_enabled_count(), 1);
  RunSyncClientTask([&dut_client]() {
    fidl::WireResult result = dut_client->UnregisterPowerDomain();
    ZX_ASSERT(result.ok() && result->is_ok());
  });
  EXPECT_EQ(dut_->GetDependentCount(), 0);
  EXPECT_EQ(parent_power_.SyncCall(&FakePower::power_domain_registered_count), 1);
  EXPECT_EQ(power_impl_->power_domain_enabled_count(), 1);
  RunSyncClientTask([&dut_client]() {
    fidl::WireResult result = dut_client->UnregisterPowerDomain();
    ZX_ASSERT(result.ok());
    EXPECT_EQ(ZX_ERR_UNAVAILABLE, result.value().error_value());
  });
  EXPECT_EQ(dut_->GetDependentCount(), 0);
}

TEST_F(GenericPowerTest, DependentCount_TwoChildren) {
  fidl::WireSyncClient<fuchsia_hardware_power::Device> dut_client(ConnectDut());
  RunSyncClientTask([&dut_client]() {
    fidl::WireResult result = dut_client->RegisterPowerDomain(20, 800);
    ZX_ASSERT(result.ok() && result->is_ok());
  });
  EXPECT_EQ(dut_->GetDependentCount(), 1);
  EXPECT_EQ(parent_power_.SyncCall(&FakePower::power_domain_registered_count), 1);
  EXPECT_EQ(power_impl_->power_domain_enabled_count(), 1);

  fidl::WireSyncClient<fuchsia_hardware_power::Device> dut_client_2(ConnectDut());
  EXPECT_EQ(dut_->GetDependentCount(), 1);
  RunSyncClientTask([&dut_client_2]() {
    fidl::WireResult result = dut_client_2->RegisterPowerDomain(50, 400);
    ZX_ASSERT(result.ok() && result->is_ok());
  });
  EXPECT_EQ(dut_->GetDependentCount(), 2);
  EXPECT_EQ(parent_power_.SyncCall(&FakePower::power_domain_registered_count), 1);
  EXPECT_EQ(power_impl_->power_domain_enabled_count(), 1);
}

TEST_F(GenericPowerTest, GetSupportedVoltageRange) {
  fidl::WireSyncClient<fuchsia_hardware_power::Device> dut_client(ConnectDut());
  RunSyncClientTask([&dut_client]() {
    fidl::WireResult result = dut_client->GetSupportedVoltageRange();
    ZX_ASSERT(result.ok() && result->is_ok());
    EXPECT_EQ(result->value()->min, 10);
    EXPECT_EQ(result->value()->max, 1000);
  });
}

TEST_F(GenericPowerTest, RequestVoltage_UnsuppportedVoltage) {
  fidl::WireSyncClient<fuchsia_hardware_power::Device> dut_client(ConnectDut());
  RunSyncClientTask([&dut_client]() {
    fidl::WireResult result = dut_client->RegisterPowerDomain(20, 800);
    ZX_ASSERT(result.ok() && result->is_ok());

    fidl::WireResult get_result = dut_client->GetSupportedVoltageRange();
    ZX_ASSERT(get_result.ok() && get_result->is_ok());
    EXPECT_EQ(get_result.value()->min, 10);
    EXPECT_EQ(get_result.value()->max, 1000);

    fidl::WireResult request_result = dut_client->RequestVoltage(1010);
    ZX_ASSERT(request_result.ok());
    EXPECT_EQ(ZX_ERR_INVALID_ARGS, request_result.value().error_value());
  });
}

TEST_F(GenericPowerTest, RequestVoltage) {
  fidl::WireSyncClient<fuchsia_hardware_power::Device> dut_client(ConnectDut());
  RunSyncClientTask([&dut_client]() {
    fidl::WireResult result = dut_client->RegisterPowerDomain(20, 800);
    ZX_ASSERT(result.ok() && result->is_ok());
  });
  EXPECT_EQ(dut_->GetDependentCount(), 1);
  EXPECT_EQ(parent_power_.SyncCall(&FakePower::power_domain_registered_count), 1);
  EXPECT_EQ(power_impl_->power_domain_enabled_count(), 1);

  fidl::WireSyncClient<fuchsia_hardware_power::Device> dut_client_2(ConnectDut());
  EXPECT_EQ(dut_->GetDependentCount(), 1);

  RunSyncClientTask([&dut_client_2]() {
    fidl::WireResult result = dut_client_2->RegisterPowerDomain(10, 400);
    ZX_ASSERT(result.ok() && result->is_ok());
  });

  EXPECT_EQ(dut_->GetDependentCount(), 2);
  RunSyncClientTask([&dut_client_2]() {
    fidl::WireResult result = dut_client_2->RequestVoltage(900);
    ZX_ASSERT(result.ok() && result->is_ok());
    EXPECT_EQ(400, result.value()->actual_voltage);

    fidl::WireResult result2 = dut_client_2->RequestVoltage(15);
    ZX_ASSERT(result2.ok() && result2->is_ok());
    EXPECT_EQ(20, result2.value()->actual_voltage);

    fidl::WireResult unregister_result = dut_client_2->UnregisterPowerDomain();
    ZX_ASSERT(unregister_result.ok() && unregister_result->is_ok());
  });

  EXPECT_EQ(dut_->GetDependentCount(), 1);

  RunSyncClientTask([&dut_client]() {
    fidl::WireResult result = dut_client->RequestVoltage(900);
    ZX_ASSERT(result.ok() && result->is_ok());
    EXPECT_EQ(800, result.value()->actual_voltage);

    fidl::WireResult result2 = dut_client->RequestVoltage(15);
    ZX_ASSERT(result2.ok() && result2->is_ok());
    EXPECT_EQ(20, result2.value()->actual_voltage);
  });
}

TEST_F(GenericPowerTest, RequestVoltage_Unregistered) {
  fidl::WireSyncClient<fuchsia_hardware_power::Device> dut_client(ConnectDut());
  RunSyncClientTask([&dut_client]() {
    fidl::WireResult result = dut_client->RequestVoltage(900);
    ZX_ASSERT(result.ok());
    EXPECT_EQ(ZX_ERR_UNAVAILABLE, result.value().error_value());
  });
}

TEST_F(GenericPowerTest, FixedVoltageDomain) {
  zx::result parent_power_client = parent_power_.SyncCall(&FakePower::BindServer);
  ASSERT_OK(parent_power_client);

  auto dut_fixed =
      std::make_unique<PowerDomain>(fake_parent_.get(), 1, power_impl_->GetClient(),
                                    std::move(parent_power_client.value()), 1000, 1000, true);
  auto endpoints = fidl::Endpoints<fuchsia_hardware_power::Device>::Create();
  dut_fixed->GetHandler()(std::move(endpoints.server));
  fidl::WireSyncClient<fuchsia_hardware_power::Device> dut_client_2(std::move(endpoints.client));

  RunSyncClientTask([&dut_client_2]() {
    fidl::WireResult result = dut_client_2->RegisterPowerDomain(0, 0);
    ZX_ASSERT(result.ok() && result->is_ok());
  });

  EXPECT_EQ(dut_fixed->GetDependentCount(), 1);
  EXPECT_EQ(parent_power_.SyncCall(&FakePower::power_domain_registered_count), 1);
  EXPECT_EQ(power_impl_->power_domain_enabled_count(), 1);

  RunSyncClientTask([&dut_client_2]() {
    fidl::WireResult result = dut_client_2->GetSupportedVoltageRange();
    ZX_ASSERT(result.ok());
    EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, result.value().error_value());

    fidl::WireResult request_result = dut_client_2->RequestVoltage(900);
    ZX_ASSERT(request_result.ok());
    EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, request_result.value().error_value());

    fidl::WireResult unregister_result = dut_client_2->UnregisterPowerDomain();
    ZX_ASSERT(unregister_result.ok() && unregister_result->is_ok());
  });

  EXPECT_EQ(dut_fixed->GetDependentCount(), 0);
  EXPECT_EQ(parent_power_.SyncCall(&FakePower::power_domain_unregistered_count), 1);
  EXPECT_EQ(power_impl_->power_domain_disabled_count(), 1);
}

}  // namespace power
