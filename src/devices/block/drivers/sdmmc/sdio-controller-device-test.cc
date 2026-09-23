// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdio-controller-device.h"

#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <fidl/fuchsia.power.broker/cpp/test_base.h>
#include <fidl/fuchsia.power.system/cpp/fidl.h>
#include <fidl/fuchsia.power.system/cpp/test_base.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fdio/include/lib/fdio/directory.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/sdio/hw.h>
#include <lib/zx/vmo.h>
#include <stdio.h>

#include <memory>
#include <vector>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/sdio/cpp/bind.h>
#include <gtest/gtest.h>

#include "fake-sdmmc-device.h"
#include "sdmmc-root-device.h"
#include "src/lib/testing/predicates/status.h"

namespace {

constexpr uint32_t OpCondFunctions(uint32_t functions) {
  return SDIO_SEND_OP_COND_RESP_IORDY | (functions << SDIO_SEND_OP_COND_RESP_NUM_FUNC_LOC);
}

}  // namespace

namespace sdmmc {

using fuchsia_hardware_sdio::wire::SdioRwTxn;
using fuchsia_hardware_sdmmc::wire::SdmmcBuffer;
using fuchsia_hardware_sdmmc::wire::SdmmcBufferRegion;

class TestSdmmcRootDevice : public SdmmcRootDevice {
 public:
  // Modify these static variables to configure the behaviour of this test device.
  static FakeSdmmcDevice sdmmc_;

  explicit TestSdmmcRootDevice() : SdmmcRootDevice() {}

 protected:
  zx_status_t Init(const fuchsia_hardware_sdmmc::SdmmcMetadata& metadata) override {
    const bool use_fidl = metadata.use_fidl().value_or(true);

    std::unique_ptr<SdmmcDevice> sdmmc;
    if (use_fidl) {
      zx::result client_end = sdmmc_.GetFidlClientEnd();
      if (client_end.is_error()) {
        return client_end.error_value();
      }
      sdmmc = std::make_unique<SdmmcDevice>(this, std::move(*client_end));
    } else {
      sdmmc = std::make_unique<SdmmcDevice>(this, sdmmc_.GetClient());
    }

    zx_status_t status;
    if (status = sdmmc->RefreshHostInfo(); status != ZX_OK) {
      return status;
    }
    if (status = sdmmc->HwReset(); status != ZX_OK) {
      return status;
    }

    std::unique_ptr<SdioControllerDevice> sdio_controller_device;
    if (status = SdioControllerDevice::Create(this, std::move(sdmmc), &sdio_controller_device);
        status != ZX_OK) {
      return status;
    }
    if (status = sdio_controller_device->Probe(metadata); status != ZX_OK) {
      return status;
    }
    if (status = sdio_controller_device->AddDevice(); status != ZX_OK) {
      return status;
    }
    child_device_ = std::move(sdio_controller_device);
    return ZX_OK;
  }
};

class FakeActivityGovernor
    : public fidl::testing::TestBase<fuchsia_power_system::ActivityGovernor> {
 public:
  void GetPowerElements(GetPowerElementsCompleter::Sync& completer) override {
    completer.Reply({});
  }
  void AcquireWakeLease(AcquireWakeLeaseRequest& request,
                        AcquireWakeLeaseCompleter::Sync& completer) override {
    zx::eventpair token, peer;
    zx::eventpair::create(0, &token, &peer);
    completer.Reply(fit::success(std::move(token)));
  }
  void AcquireWakeLeaseWithToken(AcquireWakeLeaseWithTokenRequest& request,
                                 AcquireWakeLeaseWithTokenCompleter::Sync& completer) override {
    completer.Reply(fit::success());
  }
  void TakeApplicationActivityLease(
      TakeApplicationActivityLeaseRequest& request,
      TakeApplicationActivityLeaseCompleter::Sync& completer) override {
    zx::eventpair token, peer;
    zx::eventpair::create(0, &token, &peer);
    completer.Reply(std::move(token));
  }
  void RegisterSuspendBlocker(RegisterSuspendBlockerRequest& request,
                              RegisterSuspendBlockerCompleter::Sync& completer) override {
    zx::eventpair token, peer;
    zx::eventpair::create(0, &token, &peer);
    completer.Reply(fit::success(std::move(token)));
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override { FAIL(); }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_power_system::ActivityGovernor> md,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}

  fidl::ProtocolHandler<fuchsia_power_system::ActivityGovernor> CreateHandler() {
    return bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                   fidl::kIgnoreBindingClosure);
  }

 private:
  fidl::ServerBindingGroup<fuchsia_power_system::ActivityGovernor> bindings_;
};

class FakePowerBroker : public fidl::Server<fuchsia_power_broker::Topology>,
                        public fidl::Server<fuchsia_power_broker::Lessor>,
                        public fidl::testing::TestBase<fuchsia_power_broker::ElementControl>,
                        public fidl::WireServer<fuchsia_hardware_power::PowerTokenProvider> {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) {
    zx::result<> result =
        to_driver_vfs.component().AddUnmanagedProtocol<fuchsia_power_broker::Topology>(
            topology_bindings_.CreateHandler(this,
                                             fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                             fidl::kIgnoreBindingClosure));
    if (result.is_error()) {
      return result;
    }

    return to_driver_vfs.AddService<fuchsia_hardware_power::PowerTokenService>(
        fuchsia_hardware_power::PowerTokenService::InstanceHandler({
            .token_provider = power_token_provider_bindings_.CreateHandler(
                this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                fidl::kIgnoreBindingClosure),
        }));
  }

  std::vector<uint8_t> lease_power_levels() const { return lease_power_levels_; }

  std::vector<zx::event> TakeDependencyTokens() { return std::move(dependency_tokens_); }

  std::vector<fidl::ServerEnd<fuchsia_power_broker::LeaseControl>> TakeLeaseControlServerEnds() {
    return std::move(lease_control_server_ends_);
  }

  std::vector<fidl::ClientEnd<fuchsia_power_broker::ElementRunner>> TakeElementRunnerClientEnds() {
    return std::move(element_runner_client_ends_);
  }

  void AddHardwarePowerElement(
      fidl::ServerEnd<fuchsia_power_broker::ElementControl> element_control_server_end,
      fidl::ClientEnd<fuchsia_power_broker::ElementRunner> element_runner_client_end,
      fidl::ServerEnd<fuchsia_power_broker::Lessor> lessor_server_end) {
    element_control_bindings_.AddBinding(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                         std::move(element_control_server_end), this,
                                         fidl::kIgnoreBindingClosure);
    element_runner_client_ends_.push_back(std::move(element_runner_client_end));
    lessor_bindings_.AddBinding(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                std::move(lessor_server_end), this, fidl::kIgnoreBindingClosure);
  }

 private:
  // fuchsia.power.broker/Topology
  void AddElement(fuchsia_power_broker::ElementSchema& req,
                  AddElementCompleter::Sync& completer) override {
    if (!req.lessor_channel() || !req.element_control() || !req.element_runner()) {
      completer.Reply(fit::error(fuchsia_power_broker::AddElementError::kInvalid));
      return;
    }

    if (!req.element_name()) {
      completer.Reply(fit::error(fuchsia_power_broker::AddElementError::kInvalid));
      return;
    }

    uint32_t function = SDIO_MAX_FUNCS;
    int result = sscanf(req.element_name()->c_str(), "sdio-function-%u-hardware", &function);
    if (result != 1 || function >= SDIO_MAX_FUNCS) {
      completer.Reply(fit::error(fuchsia_power_broker::AddElementError::kInvalid));
      return;
    }

    // Verify that the dependency token was previously registered.
    if (!req.dependencies() || req.dependencies()->empty()) {
      completer.Reply(fit::error(fuchsia_power_broker::AddElementError::kInvalid));
      return;
    }
    if (!token_.is_valid()) {
      completer.Reply(fit::error(fuchsia_power_broker::AddElementError::kInvalid));
      return;
    }

    for (const auto& dependency : *req.dependencies()) {
      zx_info_handle_basic_t dependency_info{}, token_info{};

      ASSERT_TRUE(dependency.requires_token().has_value());
      ASSERT_OK(dependency.requires_token()->get_info(ZX_INFO_HANDLE_BASIC, &dependency_info,
                                                      sizeof(dependency_info), nullptr, nullptr));
      ASSERT_OK(
          token_.get_info(ZX_INFO_HANDLE_BASIC, &token_info, sizeof(token_info), nullptr, nullptr));

      if (token_info.koid != dependency_info.koid) {
        completer.Reply(fit::error(fuchsia_power_broker::AddElementError::kInvalid));
        return;
      }
    }

    lessor_bindings_.AddBinding(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                *std::move(req.lessor_channel()), this,
                                fidl::kIgnoreBindingClosure);
    element_control_bindings_.AddBinding(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                         *std::move(req.element_control()), this,
                                         fidl::kIgnoreBindingClosure);
    element_runner_client_ends_.push_back(*std::move(req.element_runner()));

    completer.Reply(fit::success());
  }
  void Lease(
      fidl::Server<fuchsia_power_broker::Topology>::LeaseRequest& req,
      fidl::Server<fuchsia_power_broker::Topology>::LeaseCompleter::Sync& completer) override {
    completer.Reply(fit::success());
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_power_broker::Topology> md,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    FAIL();
  }

  // fuchsia.power.broker/Lessor
  void Lease(fidl::Server<fuchsia_power_broker::Lessor>::LeaseRequest& request,
             fidl::Server<fuchsia_power_broker::Lessor>::LeaseCompleter::Sync& completer) override {
    lease_power_levels_.push_back(request.level());

    auto [lease_control_client_end, lease_control_server_end] =
        fidl::Endpoints<fuchsia_power_broker::LeaseControl>::Create();
    lease_control_server_ends_.push_back(std::move(lease_control_server_end));
    completer.Reply(fit::ok(std::move(lease_control_client_end)));
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_power_broker::Lessor> md,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    FAIL();
  }

  // fuchsia.power.broker/ElementControl
  void RegisterDependencyToken(RegisterDependencyTokenRequest& request,
                               RegisterDependencyTokenCompleter::Sync& completer) override {
    dependency_tokens_.push_back(std::move(request.token()));
    completer.Reply(fit::ok());
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_power_broker::ElementControl> md,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    FAIL();
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override { FAIL(); }

  // fuchsia.hardware.power/PowerTokenProvider
  void GetToken(GetTokenCompleter::Sync& completer) override {
    if (!token_.is_valid()) {
      if (zx_status_t status = zx::event::create(0, &token_); status != ZX_OK) {
        completer.ReplyError(status);
        return;
      }
    }

    zx::event dup;
    if (zx_status_t status = token_.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup); status == ZX_OK) {
      completer.ReplySuccess(std::move(dup));
    } else {
      completer.ReplyError(status);
    }
  }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_power::PowerTokenProvider> md,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    FAIL();
  }

  fidl::ServerBindingGroup<fuchsia_power_broker::Topology> topology_bindings_;
  fidl::ServerBindingGroup<fuchsia_hardware_power::PowerTokenProvider>
      power_token_provider_bindings_;

  fidl::ServerBindingGroup<fuchsia_power_broker::Lessor> lessor_bindings_;
  fidl::ServerBindingGroup<fuchsia_power_broker::ElementControl> element_control_bindings_;
  std::vector<fidl::ClientEnd<fuchsia_power_broker::ElementRunner>> element_runner_client_ends_;

  zx::event token_;
  std::vector<uint8_t> lease_power_levels_;
  std::vector<fidl::ServerEnd<fuchsia_power_broker::LeaseControl>> lease_control_server_ends_;
  std::vector<zx::event> dependency_tokens_;
};

FakeSdmmcDevice TestSdmmcRootDevice::sdmmc_;

class Environment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    fuchsia_hardware_sdmmc::SdmmcMetadata metadata{{
        .vccq_off_with_controller_off = true,
    }};
    if (zx::result result = metadata_server_.Serve(
            to_driver_vfs, fdf::Dispatcher::GetCurrent()->async_dispatcher(), metadata);
        result.is_error()) {
      return result.take_error();
    }
    if (zx::result result = fake_power_broker_.Serve(to_driver_vfs); result.is_error()) {
      return result.take_error();
    }
    if (zx::result result =
            to_driver_vfs.component().AddUnmanagedProtocol<fuchsia_power_system::ActivityGovernor>(
                activity_governor_.CreateHandler());
        result.is_error()) {
      return result.take_error();
    }

    auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    zx_status_t status = fdio_open3("/pkg/", static_cast<uint64_t>(fuchsia_io::wire::kPermReadable),
                                    server.TakeChannel().release());
    if (status != ZX_OK) {
      return zx::error(status);
    }

    return to_driver_vfs.AddDirectory(std::move(client), "pkg");
  }

  FakePowerBroker& fake_power_broker() { return fake_power_broker_; }

 private:
  fidl::Arena<> arena_;
  fdf_metadata::MetadataServer<fuchsia_hardware_sdmmc::SdmmcMetadata> metadata_server_;
  FakePowerBroker fake_power_broker_;
  FakeActivityGovernor activity_governor_;
};

class TestConfig final {
 public:
  using DriverType = TestSdmmcRootDevice;
  using EnvironmentType = Environment;
};

class SdioControllerDeviceTest : public ::testing::Test {
 public:
  void SetUp() override {
    sdmmc_.Reset();

    // Set all function block sizes (and the host max transfer size) to 1 so that the initialization
    // checks pass. Individual test cases can override these by overwriting the CIS or creating a
    // new one and overwriting the CIS pointer.
    sdmmc_.Write(0x0009, std::vector<uint8_t>{0x00, 0x20, 0x00}, 0);

    sdmmc_.Write(0x2000,
                 std::vector<uint8_t>{
                     0x22,  // Function extensions tuple.
                     0x04,  // Function extensions tuple size.
                     0x00,  // Type of extended data.
                     0x01,
                     0x00,  // Function 0 block size.
                 },
                 0);

    sdmmc_.Write(0x1000, std::vector<uint8_t>{0x22, 0x2a, 0x01}, 0);
    sdmmc_.Write(0x100e, std::vector<uint8_t>{0x01, 0x00}, 0);

    sdmmc_.Write(0x0109, std::vector<uint8_t>{0x00, 0x10, 0x00}, 0);
    sdmmc_.Write(0x0209, std::vector<uint8_t>{0x00, 0x10, 0x00}, 0);
    sdmmc_.Write(0x0309, std::vector<uint8_t>{0x00, 0x10, 0x00}, 0);
    sdmmc_.Write(0x0409, std::vector<uint8_t>{0x00, 0x10, 0x00}, 0);
    sdmmc_.Write(0x0509, std::vector<uint8_t>{0x00, 0x10, 0x00}, 0);
    sdmmc_.Write(0x0609, std::vector<uint8_t>{0x00, 0x10, 0x00}, 0);
    sdmmc_.Write(0x0709, std::vector<uint8_t>{0x00, 0x10, 0x00}, 0);

    sdmmc_.set_host_info({
        .caps = 0,
        .max_transfer_size = 1,
    });
  }

  void TearDown() override {
    // SdmmcRootDevice::PrepareStop() invokes SdioControllerDevice::StopSdioIrqDispatcher().
    zx::result<> result = driver_test().StopDriver();
    ASSERT_OK(result);
  }

  zx_status_t StartDriver(bool supply_power_framework = false) {
    std::optional<fuchsia_driver_framework::PowerElementArgs> power_args;

    if (supply_power_framework) {
      auto [element_control_client, element_control_server] =
          fidl::Endpoints<fuchsia_power_broker::ElementControl>::Create();
      auto [element_runner_client, element_runner_server] =
          fidl::Endpoints<fuchsia_power_broker::ElementRunner>::Create();
      auto [lessor_client, lessor_server] = fidl::Endpoints<fuchsia_power_broker::Lessor>::Create();
      fuchsia_power_broker::DependencyToken element_token;
      fuchsia_power_broker::DependencyToken element_token_copy;
      EXPECT_OK(zx::event::create(0, &element_token));
      EXPECT_OK(element_token.duplicate(ZX_RIGHT_SAME_RIGHTS, &element_token_copy));

      fuchsia_driver_framework::PowerElementArgs local_power_args;
      local_power_args.control_client() = std::move(element_control_client);
      local_power_args.runner_server() = std::move(element_runner_server);
      local_power_args.lessor_client() = std::move(lessor_client);
      local_power_args.token() = std::move(element_token);

      power_args = std::move(local_power_args);

      driver_test().RunInEnvironmentTypeContext<void>(
          [control_server = std::move(element_control_server),
           runner_client = std::move(element_runner_client),
           lessor_server = std::move(lessor_server)](Environment& env) mutable {
            env.fake_power_broker().AddHardwarePowerElement(
                std::move(control_server), std::move(runner_client), std::move(lessor_server));
          });
    }

    if (zx_status_t status = zx::event::create(0, &node_token_); status != ZX_OK) {
      return status;
    }
    zx::event token_copy;
    if (zx_status_t status = node_token_.duplicate(ZX_RIGHT_SAME_RIGHTS, &token_copy);
        status != ZX_OK) {
      return status;
    }

    zx::result<> result =
        driver_test().StartDriverWithCustomStartArgs([&](fdf::DriverStartArgs& start_args) mutable {
          sdmmc_config::Config fake_config;
          fake_config.enable_suspend() = supply_power_framework;
          start_args.config(fake_config.ToVmo());
          if (supply_power_framework) {
            start_args.power_element_args(std::move(power_args.value()));
          }
          start_args.node_token(std::move(token_copy));
        });
    if (result.is_error()) {
      return result.status_value();
    }
    return ZX_OK;
  }

  fdf_testing::ForegroundDriverTest<TestConfig>& driver_test() { return driver_test_; }

 protected:
  fidl::WireClient<fuchsia_hardware_sdio::Device> ConnectDeviceClient(uint8_t function) {
    char instance[15];
    snprintf(instance, sizeof(instance), "sdmmc-sdio-%u", function);

    auto sdio_client_end = driver_test().Connect<fuchsia_hardware_sdio::Service::Device>(instance);
    if (sdio_client_end.is_error()) {
      return {};
    }

    return fidl::WireClient<fuchsia_hardware_sdio::Device>(
        *std::move(sdio_client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());
  }

  FakeSdmmcDevice& sdmmc_ = TestSdmmcRootDevice::sdmmc_;
  zx::event node_token_;

 private:
  fdf_testing::ForegroundDriverTest<TestConfig> driver_test_;
};

class SdioScatterGatherTest : public SdioControllerDeviceTest {
 public:
  SdioScatterGatherTest() {}

  void SetUp() override { sdmmc_.Reset(); }

  void Init(const uint8_t function, const bool multiblock) {
    sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
      out_response[0] = OpCondFunctions(5);
    });
    sdmmc_.Write(
        SDIO_CIA_CCCR_CARD_CAPS_ADDR,
        std::vector<uint8_t>{static_cast<uint8_t>(multiblock ? SDIO_CIA_CCCR_CARD_CAP_SMB : 0)}, 0);

    sdmmc_.Write(0x0009, std::vector<uint8_t>{0x00, 0x20, 0x00}, 0);
    sdmmc_.Write(0x2000, std::vector<uint8_t>{0x22, 0x04, 0x00, 0x00, 0x02}, 0);

    // Set the maximum block size for function 1-5 to eight bytes.
    sdmmc_.Write(0x0109, std::vector<uint8_t>{0x00, 0x10, 0x00}, 0);
    sdmmc_.Write(0x0209, std::vector<uint8_t>{0x00, 0x10, 0x00}, 0);
    sdmmc_.Write(0x0309, std::vector<uint8_t>{0x00, 0x10, 0x00}, 0);
    sdmmc_.Write(0x0409, std::vector<uint8_t>{0x00, 0x10, 0x00}, 0);
    sdmmc_.Write(0x0509, std::vector<uint8_t>{0x00, 0x10, 0x00}, 0);
    sdmmc_.Write(0x1000, std::vector<uint8_t>{0x22, 0x2a, 0x01}, 0);
    sdmmc_.Write(0x100e, std::vector<uint8_t>{0x08, 0x00}, 0);

    sdmmc_.set_host_info({
        .caps = 0,
        .max_transfer_size = 1024,
    });

    ASSERT_OK(StartDriver());

    fidl::WireClient client = ConnectDeviceClient(function);
    ASSERT_TRUE(client.is_valid());

    client->UpdateBlockSize(4, false).ThenExactlyOnce([](auto& result) {
      ASSERT_TRUE(result.ok());
      EXPECT_TRUE(result->is_ok());
    });
    driver_test().runtime().RunUntilIdle();

    sdmmc_.requests().clear();

    zx::vmo vmo1, vmo3;
    ASSERT_OK(mapper1_.CreateAndMap(zx_system_get_page_size(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE,
                                    nullptr, &vmo1));
    ASSERT_OK(mapper2_.CreateAndMap(zx_system_get_page_size(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE,
                                    nullptr, &vmo2_));
    ASSERT_OK(mapper3_.CreateAndMap(zx_system_get_page_size(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE,
                                    nullptr, &vmo3));

    zx::vmo vmo1_dup, vmo3_dup;
    ASSERT_OK(vmo1.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo1_dup));
    ASSERT_OK(vmo3.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo3_dup));

    const uint32_t vmo_rights{fuchsia_hardware_sdmmc::SdmmcVmoRight::kRead |
                              fuchsia_hardware_sdmmc::SdmmcVmoRight::kWrite};
    client->RegisterVmo(1, std::move(vmo1_dup), 0, zx_system_get_page_size(), vmo_rights)
        .ThenExactlyOnce([](auto& result) {
          ASSERT_TRUE(result.ok());
          EXPECT_TRUE(result->is_ok());
        });
    client->RegisterVmo(3, std::move(vmo3_dup), 8, zx_system_get_page_size() - 8, vmo_rights)
        .ThenExactlyOnce([](auto& result) {
          ASSERT_TRUE(result.ok());
          EXPECT_TRUE(result->is_ok());
        });
    driver_test().runtime().RunUntilIdle();
  }

 protected:
  static constexpr uint8_t kTestData1[] = {0x17, 0xc6, 0xf4, 0x4a, 0x92, 0xc6, 0x09, 0x0a,
                                           0x8c, 0x54, 0x08, 0x07, 0xde, 0x5f, 0x8d, 0x59};
  static constexpr uint8_t kTestData2[] = {0x0d, 0x90, 0x85, 0x6a, 0xe2, 0xa9, 0x00, 0x0e,
                                           0xdf, 0x26, 0xe2, 0x17, 0x88, 0x4d, 0x3a, 0x72};
  static constexpr uint8_t kTestData3[] = {0x34, 0x83, 0x15, 0x31, 0x29, 0xa8, 0x4b, 0xe8,
                                           0xd9, 0x1f, 0xa4, 0xf4, 0x8d, 0x3a, 0x27, 0x0c};

  static SdmmcBufferRegion MakeBufferRegion(const zx::vmo& vmo, uint64_t offset, uint64_t size) {
    zx::vmo vmo_dup;
    EXPECT_OK(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_dup));
    return {
        .buffer = SdmmcBuffer::WithVmo(std::move(vmo_dup)),
        .offset = offset,
        .size = size,
    };
  }

  static SdmmcBufferRegion MakeBufferRegion(uint32_t vmo_id, uint64_t offset, uint64_t size) {
    return {
        .buffer = SdmmcBuffer::WithVmoId(vmo_id),
        .offset = offset,
        .size = size,
    };
  }

  struct SdioCmd53 {
    static SdioCmd53 FromArg(uint32_t arg) {
      SdioCmd53 ret = {};
      ret.blocks_or_bytes = arg & SDIO_IO_RW_EXTD_BYTE_BLK_COUNT_MASK;
      ret.address = (arg & SDIO_IO_RW_EXTD_REG_ADDR_MASK) >> SDIO_IO_RW_EXTD_REG_ADDR_LOC;
      ret.op_code = (arg & SDIO_IO_RW_EXTD_OP_CODE_INCR) ? 1 : 0;
      ret.block_mode = (arg & SDIO_IO_RW_EXTD_BLOCK_MODE) ? 1 : 0;
      ret.function_number = (arg & SDIO_IO_RW_EXTD_FN_IDX_MASK) >> SDIO_IO_RW_EXTD_FN_IDX_LOC;
      ret.rw_flag = (arg & SDIO_IO_RW_EXTD_RW_FLAG) ? 1 : 0;
      return ret;
    }

    uint32_t blocks_or_bytes;
    uint32_t address;
    uint32_t op_code;
    uint32_t block_mode;
    uint32_t function_number;
    uint32_t rw_flag;
  };

  zx::vmo vmo2_;
  fzl::VmoMapper mapper1_, mapper2_, mapper3_;
};

TEST_F(SdioControllerDeviceTest, MultiplexInterrupts) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(7);
  });

  ASSERT_OK(StartDriver());

  fidl::WireClient client1 = ConnectDeviceClient(1);
  ASSERT_TRUE(client1.is_valid());

  fidl::WireClient client2 = ConnectDeviceClient(2);
  ASSERT_TRUE(client2.is_valid());

  fidl::WireClient client4 = ConnectDeviceClient(4);
  ASSERT_TRUE(client4.is_valid());

  fidl::WireClient client7 = ConnectDeviceClient(7);
  ASSERT_TRUE(client7.is_valid());

  zx::port port;
  ASSERT_OK(zx::port::create(ZX_PORT_BIND_TO_INTERRUPT, &port));

  zx::interrupt interrupt1, interrupt2, interrupt4, interrupt7;

  auto set_interrupt = [](zx::interrupt* interrupt) {
    return
        [interrupt](
            fidl::WireUnownedResult<fuchsia_hardware_sdio::Device::GetInBandIntr>& result) mutable {
          ASSERT_TRUE(result.ok());
          ASSERT_TRUE(result->is_ok());
          *interrupt = std::move(result->value()->irq);
        };
  };

  client1->GetInBandIntr().ThenExactlyOnce(set_interrupt(&interrupt1));
  client2->GetInBandIntr().ThenExactlyOnce(set_interrupt(&interrupt2));
  client4->GetInBandIntr().ThenExactlyOnce(set_interrupt(&interrupt4));
  client7->GetInBandIntr().ThenExactlyOnce(set_interrupt(&interrupt7));
  driver_test().runtime().RunUntilIdle();

  ASSERT_OK(interrupt1.bind(port, 1, 0));
  ASSERT_OK(interrupt2.bind(port, 2, 0));
  ASSERT_OK(interrupt4.bind(port, 4, 0));
  ASSERT_OK(interrupt7.bind(port, 7, 0));

  sdmmc_.Write(SDIO_CIA_CCCR_INTx_INTR_PEN_ADDR, std::vector<uint8_t>{0b0000'0010}, 0);
  EXPECT_OK(sdmmc_.TriggerInBandInterrupt());

  zx_port_packet_t packet;
  EXPECT_OK(port.wait(zx::time::infinite(), &packet));
  EXPECT_EQ(packet.key, uint64_t{1});
  EXPECT_OK(interrupt1.ack());
  EXPECT_TRUE(client1->AckInBandIntr().ok());
  driver_test().runtime().RunUntilIdle();

  sdmmc_.Write(SDIO_CIA_CCCR_INTx_INTR_PEN_ADDR, std::vector<uint8_t>{0b1111'1110}, 0);
  EXPECT_OK(sdmmc_.TriggerInBandInterrupt());

  EXPECT_OK(port.wait(zx::time::infinite(), &packet));
  EXPECT_EQ(packet.key, uint64_t{1});
  EXPECT_OK(interrupt1.ack());
  EXPECT_TRUE(client1->AckInBandIntr().ok());
  driver_test().runtime().RunUntilIdle();

  EXPECT_OK(port.wait(zx::time::infinite(), &packet));
  EXPECT_EQ(packet.key, uint64_t{2});
  EXPECT_OK(interrupt2.ack());
  EXPECT_TRUE(client2->AckInBandIntr().ok());
  driver_test().runtime().RunUntilIdle();

  EXPECT_OK(port.wait(zx::time::infinite(), &packet));
  EXPECT_EQ(packet.key, uint64_t{4});
  EXPECT_OK(interrupt4.ack());
  EXPECT_TRUE(client4->AckInBandIntr().ok());
  driver_test().runtime().RunUntilIdle();

  EXPECT_OK(port.wait(zx::time::infinite(), &packet));
  EXPECT_EQ(packet.key, uint64_t{7});
  EXPECT_OK(interrupt7.ack());
  EXPECT_TRUE(client7->AckInBandIntr().ok());
  driver_test().runtime().RunUntilIdle();

  sdmmc_.Write(SDIO_CIA_CCCR_INTx_INTR_PEN_ADDR, std::vector<uint8_t>{0b1010'0010}, 0);
  EXPECT_OK(sdmmc_.TriggerInBandInterrupt());

  EXPECT_OK(port.wait(zx::time::infinite(), &packet));
  EXPECT_EQ(packet.key, uint64_t{1});
  EXPECT_OK(interrupt1.ack());
  EXPECT_TRUE(client1->AckInBandIntr().ok());
  driver_test().runtime().RunUntilIdle();

  EXPECT_OK(port.wait(zx::time::infinite(), &packet));
  EXPECT_EQ(packet.key, uint64_t{7});
  EXPECT_OK(interrupt7.ack());
  EXPECT_TRUE(client7->AckInBandIntr().ok());
  driver_test().runtime().RunUntilIdle();

  sdmmc_.Write(SDIO_CIA_CCCR_INTx_INTR_PEN_ADDR, std::vector<uint8_t>{0b0011'0110}, 0);
  EXPECT_OK(sdmmc_.TriggerInBandInterrupt());

  EXPECT_OK(port.wait(zx::time::infinite(), &packet));
  EXPECT_EQ(packet.key, uint64_t{1});
  EXPECT_OK(interrupt1.ack());
  EXPECT_TRUE(client1->AckInBandIntr().ok());
  driver_test().runtime().RunUntilIdle();

  EXPECT_OK(port.wait(zx::time::infinite(), &packet));
  EXPECT_EQ(packet.key, uint64_t{2});
  EXPECT_OK(interrupt2.ack());
  EXPECT_TRUE(client2->AckInBandIntr().ok());
  driver_test().runtime().RunUntilIdle();

  EXPECT_OK(port.wait(zx::time::infinite(), &packet));
  EXPECT_EQ(packet.key, uint64_t{4});
  EXPECT_OK(interrupt4.ack());
}

TEST_F(SdioControllerDeviceTest, InterruptNotSupported) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(7);
  });

  sdmmc_.set_in_band_interrupt_supported(false);

  ASSERT_OK(StartDriver());

  fidl::WireClient client1 = ConnectDeviceClient(1);
  ASSERT_TRUE(client1.is_valid());

  client1->GetInBandIntr().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_error());
  });
  driver_test().runtime().RunUntilIdle();

  // The SDIO driver should have created an interrupt dispatcher, then stopped it after the fake
  // SDMMC driver returned an error. Verify that the SDIO driver can still shut down cleanly.
}

TEST_F(SdioControllerDeviceTest, IoNotReadyOnFirstCmd5) {
  uint32_t count = 0;
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [&count](uint32_t out_response[4]) mutable {
    out_response[0] = (5 << SDIO_SEND_OP_COND_RESP_NUM_FUNC_LOC) | SDIO_SEND_OP_COND_IO_OCR_33V;
    if (count++ > 1) {
      // Set IORDY after the second cmd5.
      out_response[0] |= SDIO_SEND_OP_COND_RESP_IORDY;
    }
  });

  ASSERT_OK(StartDriver());

  EXPECT_EQ(sdmmc_.command_counts().at(SDIO_SEND_OP_COND), 3u);
}

TEST_F(SdioControllerDeviceTest, SdioDoRwTxn) {
  // Report five IO functions.
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(5);
  });
  sdmmc_.Write(SDIO_CIA_CCCR_CARD_CAPS_ADDR, std::vector<uint8_t>{0x00}, 0);

  // Set the maximum block size for function three to eight bytes.
  sdmmc_.Write(0x0309, std::vector<uint8_t>{0x00, 0x30, 0x00}, 0);
  sdmmc_.Write(0x3000, std::vector<uint8_t>{0x22, 0x2a, 0x01}, 0);
  sdmmc_.Write(0x300e, std::vector<uint8_t>{0x08, 0x00}, 0);

  sdmmc_.set_host_info({
      .caps = 0,
      .max_transfer_size = 16,
  });

  ASSERT_OK(StartDriver());

  fidl::WireClient client = ConnectDeviceClient(3);
  ASSERT_TRUE(client.is_valid());

  client->UpdateBlockSize(0, true).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });

  client->GetBlockSize().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->cur_blk_size, 8);
  });
  driver_test().runtime().RunUntilIdle();

  constexpr uint8_t kTestData[52] = {
      0xff, 0x7c, 0xa6, 0x24, 0x6f, 0x69, 0x7a, 0x39, 0x63, 0x68, 0xef, 0x28, 0xf3,
      0x18, 0x91, 0xf1, 0x68, 0x48, 0x78, 0x2f, 0xbb, 0xb2, 0x9a, 0x63, 0x51, 0xd4,
      0xe1, 0x94, 0xb4, 0x5c, 0x81, 0x94, 0xc7, 0x86, 0x50, 0x33, 0x61, 0xf8, 0x97,
      0x4c, 0x68, 0x71, 0x7f, 0x17, 0x59, 0x82, 0xc5, 0x36, 0xe0, 0x20, 0x0b, 0x56,
  };

  sdmmc_.requests().clear();

  zx::vmo vmo;
  EXPECT_OK(zx::vmo::create(sizeof(kTestData), 0, &vmo));
  EXPECT_OK(vmo.write(kTestData, 0, sizeof(kTestData)));

  zx::vmo vmo_dup;
  ASSERT_OK(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_dup));

  SdmmcBufferRegion region = {
      .buffer = SdmmcBuffer::WithVmo(std::move(vmo_dup)),
      .offset = 16,
      .size = 36,
  };
  SdioRwTxn txn = {
      .addr = 0x1ab08,
      .incr = false,
      .write = true,
      .buffers = fidl::VectorView<SdmmcBufferRegion>::FromExternal(&region, 1),
  };
  client->DoRwTxn(std::move(txn)).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();

  EXPECT_EQ(sdmmc_.requests().size(), size_t{5});
  sdmmc_.requests().clear();

  // The write sequence should be: four writes of blocks of eight, one write of four bytes. This is
  // a FIFO write, meaning the data will get overwritten each time. Verify the final state of the
  // device.
  const std::vector<uint8_t> read_data = sdmmc_.Read(0x1ab08, 16, 3);
  EXPECT_EQ(0, memcmp(read_data.data(), kTestData + sizeof(kTestData) - 4, 4));
  EXPECT_EQ(0, memcmp(read_data.data() + 4, kTestData + sizeof(kTestData) - 8, 4));

  sdmmc_.Write(0x12308, cpp20::span<const uint8_t>(kTestData, sizeof(kTestData)), 3);

  uint8_t buffer[sizeof(kTestData)] = {};
  EXPECT_OK(vmo.write(buffer, 0, sizeof(buffer)));

  ASSERT_OK(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_dup));

  region = {
      .buffer = SdmmcBuffer::WithVmo(std::move(vmo_dup)),
      .offset = 16,
      .size = 36,
  };
  txn = {
      .addr = 0x12308,
      .incr = true,
      .write = false,
      .buffers = fidl::VectorView<SdmmcBufferRegion>::FromExternal(&region, 1),
  };
  client->DoRwTxn(std::move(txn)).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();

  EXPECT_EQ(sdmmc_.requests().size(), size_t{5});

  EXPECT_OK(vmo.read(buffer, 0, sizeof(buffer)));
  EXPECT_EQ(0, memcmp(buffer + 16, kTestData, 36));
}

TEST_F(SdioControllerDeviceTest, SdioDoRwTxnBuffersGreaterThan511) {
  // Report five IO functions.
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(5);
  });
  sdmmc_.Write(SDIO_CIA_CCCR_CARD_CAPS_ADDR, std::vector<uint8_t>{0x00}, 0);

  // Set the maximum block size for function three to 2048 bytes (0x0800 in little-endian).
  sdmmc_.Write(0x0309, std::vector<uint8_t>{0x00, 0x30, 0x00}, 0);
  sdmmc_.Write(0x3000, std::vector<uint8_t>{0x22, 0x2a, 0x01}, 0);
  sdmmc_.Write(0x300e, std::vector<uint8_t>{0x00, 0x08}, 0);

  sdmmc_.set_host_info({
      .caps = 0,
      .max_transfer_size = 2048,
  });

  ASSERT_OK(StartDriver());

  fidl::WireClient client = ConnectDeviceClient(3);
  ASSERT_TRUE(client.is_valid());

  client->UpdateBlockSize(0, true).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();

  // Create a transaction with 600 buffers of 1 byte each (total 600 bytes < block size 2048).
  constexpr size_t kNumBuffers = 600;
  std::vector<uint8_t> test_data(kNumBuffers);
  for (size_t i = 0; i < kNumBuffers; ++i) {
    test_data[i] = static_cast<uint8_t>(i & 0xff);
  }

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kNumBuffers, 0, &vmo));
  ASSERT_OK(vmo.write(test_data.data(), 0, kNumBuffers));

  zx::vmo vmo_dup;
  ASSERT_OK(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_dup));

  const uint32_t vmo_rights{fuchsia_hardware_sdmmc::SdmmcVmoRight::kRead |
                            fuchsia_hardware_sdmmc::SdmmcVmoRight::kWrite};
  client->RegisterVmo(1, std::move(vmo_dup), 0, kNumBuffers, vmo_rights)
      .ThenExactlyOnce([](auto& result) {
        ASSERT_TRUE(result.ok());
        EXPECT_TRUE(result->is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  std::vector<SdmmcBufferRegion> regions;
  regions.reserve(kNumBuffers);
  for (size_t i = 0; i < kNumBuffers; ++i) {
    regions.push_back(SdmmcBufferRegion{
        .buffer = SdmmcBuffer::WithVmoId(1),
        .offset = i,
        .size = 1,
    });
  }

  sdmmc_.requests().clear();

  SdioRwTxn txn = {
      .addr = 0x1000,
      .incr = true,
      .write = true,
      .buffers = fidl::VectorView<SdmmcBufferRegion>::FromExternal(regions.data(), regions.size()),
  };

  client->DoRwTxn(std::move(txn)).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();

  // Since max buffers per command is 511, this transfer must be split into 2 requests (511 + 89).
  EXPECT_EQ(sdmmc_.requests().size(), size_t{2});

  // Verify all 600 bytes were written to the device.
  const std::vector<uint8_t> read_data = sdmmc_.Read(0x1000, kNumBuffers, 3);
  EXPECT_EQ(read_data, test_data);
}

TEST_F(SdioControllerDeviceTest, SdioDoRwTxnMultiBlock) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(7);
  });

  sdmmc_.Write(SDIO_CIA_CCCR_CARD_CAPS_ADDR, std::vector<uint8_t>{SDIO_CIA_CCCR_CARD_CAP_SMB}, 0);

  // Set the maximum block size for function seven to eight bytes.
  sdmmc_.Write(0x709, std::vector<uint8_t>{0x00, 0x30, 0x00}, 0);
  sdmmc_.Write(0x3000, std::vector<uint8_t>{0x22, 0x2a, 0x01}, 0);
  sdmmc_.Write(0x300e, std::vector<uint8_t>{0x08, 0x00}, 0);

  sdmmc_.set_host_info({
      .caps = 0,
      .max_transfer_size = 32,
  });

  ASSERT_OK(StartDriver());

  fidl::WireClient client = ConnectDeviceClient(7);
  ASSERT_TRUE(client.is_valid());

  client->UpdateBlockSize(0, true).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });

  client->GetBlockSize().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->cur_blk_size, 8);
  });
  driver_test().runtime().RunUntilIdle();

  constexpr uint8_t kTestData[132] = {
      // clang-format off
      0x94, 0xfa, 0x41, 0x93, 0x40, 0x81, 0xae, 0x83, 0x85, 0x88, 0x98, 0x6d,
      0x52, 0x1c, 0x53, 0x9c, 0xa7, 0x7a, 0x19, 0x74, 0xc9, 0xa9, 0x47, 0xd9,
      0x64, 0x2b, 0x76, 0x47, 0x55, 0x0b, 0x3d, 0x34, 0xd6, 0xfc, 0xca, 0x7b,
      0xae, 0xe0, 0xff, 0xe3, 0xa2, 0xd3, 0xe5, 0xb6, 0xbc, 0xa4, 0x3d, 0x01,
      0x99, 0x92, 0xdc, 0xac, 0x68, 0xb1, 0x88, 0x22, 0xc4, 0xf4, 0x1a, 0x45,
      0xe9, 0xd3, 0x5e, 0x8c, 0x24, 0x98, 0x7b, 0xf5, 0x32, 0x6d, 0xe5, 0x01,
      0x36, 0x03, 0x9b, 0xee, 0xfa, 0x23, 0x2f, 0xdd, 0xc6, 0xa4, 0x34, 0x58,
      0x23, 0xaa, 0xc9, 0x00, 0x73, 0xb8, 0xe0, 0xd8, 0xde, 0xc4, 0x59, 0x66,
      0x76, 0xd3, 0x65, 0xe0, 0xfa, 0xf7, 0x89, 0x40, 0x3a, 0xa8, 0x83, 0x53,
      0x63, 0xf4, 0x36, 0xea, 0xb3, 0x94, 0xe7, 0x5f, 0x3c, 0xed, 0x8d, 0x3e,
      0xee, 0x1b, 0x75, 0xea, 0xb3, 0x95, 0xd2, 0x25, 0x7c, 0xb9, 0x6d, 0x37,
      // clang-format on
  };

  zx::vmo vmo;
  EXPECT_OK(zx::vmo::create(sizeof(kTestData), 0, &vmo));
  EXPECT_OK(vmo.write(kTestData, 0, sizeof(kTestData)));

  sdmmc_.Write(0x1ab08, cpp20::span<const uint8_t>(kTestData, sizeof(kTestData)), 7);

  zx::vmo vmo_dup;
  ASSERT_OK(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_dup));

  SdmmcBufferRegion region = {
      .buffer = SdmmcBuffer::WithVmo(std::move(vmo_dup)),
      .offset = 64,
      .size = 68,
  };
  SdioRwTxn txn = {
      .addr = 0x1ab08,
      .incr = false,
      .write = false,
      .buffers = fidl::VectorView<SdmmcBufferRegion>::FromExternal(&region, 1),
  };
  client->DoRwTxn(std::move(txn)).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();

  uint8_t buffer[sizeof(kTestData)];
  EXPECT_OK(vmo.read(buffer, 0, sizeof(buffer)));

  EXPECT_EQ(0, memcmp(buffer + 64, kTestData, 64));
  EXPECT_EQ(0, memcmp(buffer + 128, kTestData, 4));

  EXPECT_OK(vmo.write(kTestData, 0, sizeof(kTestData)));

  ASSERT_OK(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_dup));

  region = {
      .buffer = SdmmcBuffer::WithVmo(std::move(vmo_dup)),
      .offset = 64,
      .size = 68,
  };
  txn = {
      .addr = 0x12308,
      .incr = true,
      .write = true,
      .buffers = fidl::VectorView<SdmmcBufferRegion>::FromExternal(&region, 1),
  };
  client->DoRwTxn(std::move(txn)).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();

  EXPECT_EQ(0, memcmp(sdmmc_.Read(0x12308, 68, 7).data(), kTestData + 64, 68));
}

TEST_F(SdioControllerDeviceTest, SdioIntrPending) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(7);
  });

  ASSERT_OK(StartDriver());

  fidl::WireClient client1 = ConnectDeviceClient(1);
  ASSERT_TRUE(client1.is_valid());

  fidl::WireClient client2 = ConnectDeviceClient(2);
  ASSERT_TRUE(client2.is_valid());

  fidl::WireClient client3 = ConnectDeviceClient(3);
  ASSERT_TRUE(client3.is_valid());

  fidl::WireClient client4 = ConnectDeviceClient(4);
  ASSERT_TRUE(client4.is_valid());

  fidl::WireClient client7 = ConnectDeviceClient(7);
  ASSERT_TRUE(client7.is_valid());

  auto expect_pending = [](bool pending_expected) {
    return [pending_expected](
               fidl::WireUnownedResult<fuchsia_hardware_sdio::Device::IntrPending>& result) {
      ASSERT_TRUE(result.ok());
      ASSERT_TRUE(result->is_ok());
      EXPECT_EQ(result->value()->pending, pending_expected);
    };
  };

  sdmmc_.Write(SDIO_CIA_CCCR_INTx_INTR_PEN_ADDR, std::vector<uint8_t>{0b0011'0010}, 0);
  client4->IntrPending().ThenExactlyOnce(expect_pending(true));
  driver_test().runtime().RunUntilIdle();

  sdmmc_.Write(SDIO_CIA_CCCR_INTx_INTR_PEN_ADDR, std::vector<uint8_t>{0b0010'0010}, 0);
  client4->IntrPending().ThenExactlyOnce(expect_pending(false));
  driver_test().runtime().RunUntilIdle();

  sdmmc_.Write(SDIO_CIA_CCCR_INTx_INTR_PEN_ADDR, std::vector<uint8_t>{0b1000'0000}, 0);
  client7->IntrPending().ThenExactlyOnce(expect_pending(true));
  driver_test().runtime().RunUntilIdle();

  sdmmc_.Write(SDIO_CIA_CCCR_INTx_INTR_PEN_ADDR, std::vector<uint8_t>{0b0000'0000}, 0);
  client7->IntrPending().ThenExactlyOnce(expect_pending(false));
  driver_test().runtime().RunUntilIdle();

  sdmmc_.Write(SDIO_CIA_CCCR_INTx_INTR_PEN_ADDR, std::vector<uint8_t>{0b0000'1110}, 0);
  client1->IntrPending().ThenExactlyOnce(expect_pending(true));
  driver_test().runtime().RunUntilIdle();

  sdmmc_.Write(SDIO_CIA_CCCR_INTx_INTR_PEN_ADDR, std::vector<uint8_t>{0b0000'1110}, 0);
  client2->IntrPending().ThenExactlyOnce(expect_pending(true));
  driver_test().runtime().RunUntilIdle();

  sdmmc_.Write(SDIO_CIA_CCCR_INTx_INTR_PEN_ADDR, std::vector<uint8_t>{0b0000'1110}, 0);
  client3->IntrPending().ThenExactlyOnce(expect_pending(true));
  driver_test().runtime().RunUntilIdle();
}

TEST_F(SdioControllerDeviceTest, EnableDisableFnIntr) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(7);
  });

  ASSERT_OK(StartDriver());

  fidl::WireClient client4 = ConnectDeviceClient(4);
  ASSERT_TRUE(client4.is_valid());

  fidl::WireClient client7 = ConnectDeviceClient(7);
  ASSERT_TRUE(client7.is_valid());

  sdmmc_.Write(0x04, std::vector<uint8_t>{0b0000'0000}, 0);

  client4->EnableFnIntr().ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
    EXPECT_EQ(sdmmc_.Read(0x04, 1, 0)[0], 0b0001'0001);
  });
  driver_test().runtime().RunUntilIdle();

  client7->EnableFnIntr().ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
    EXPECT_EQ(sdmmc_.Read(0x04, 1, 0)[0], 0b1001'0001);
  });
  driver_test().runtime().RunUntilIdle();

  client4->EnableFnIntr().ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
    EXPECT_EQ(sdmmc_.Read(0x04, 1, 0)[0], 0b1001'0001);
  });

  client4->DisableFnIntr().ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
    EXPECT_EQ(sdmmc_.Read(0x04, 1, 0)[0], 0b1000'0001);
  });
  driver_test().runtime().RunUntilIdle();

  client7->DisableFnIntr().ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
    EXPECT_EQ(sdmmc_.Read(0x04, 1, 0)[0], 0b0000'0000);
  });

  client7->DisableFnIntr().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_FALSE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();
}

TEST_F(SdioControllerDeviceTest, ProcessCccrWithCaps) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(1);
  });

  sdmmc_.Write(0x00, std::vector<uint8_t>{0x43}, 0);  // CCCR/SDIO revision.
  sdmmc_.Write(0x08, std::vector<uint8_t>{0xc2}, 0);  // Card capability.
  sdmmc_.Write(0x13, std::vector<uint8_t>{0xa9}, 0);  // Bus speed select.
  sdmmc_.Write(0x14, std::vector<uint8_t>{0x3f}, 0);  // UHS-I support.
  sdmmc_.Write(0x15, std::vector<uint8_t>{0xb7}, 0);  // Driver strength.

  ASSERT_OK(StartDriver());

  fidl::WireClient client = ConnectDeviceClient(1);
  ASSERT_TRUE(client.is_valid());

  using fuchsia_hardware_sdio::SdioDeviceCapabilities;

  client->GetDevHwInfo().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->hw_info.dev_hw_info.caps,
              SdioDeviceCapabilities::kMultiBlock | SdioDeviceCapabilities::kLowSpeed |
                  SdioDeviceCapabilities::kFourBitBus | SdioDeviceCapabilities::kHighSpeed |
                  SdioDeviceCapabilities::kUhsSdr50 | SdioDeviceCapabilities::kUhsSdr104 |
                  SdioDeviceCapabilities::kUhsDdr50 | SdioDeviceCapabilities::kTypeA |
                  SdioDeviceCapabilities::kTypeB | SdioDeviceCapabilities::kTypeD);
  });
  driver_test().runtime().RunUntilIdle();
}

TEST_F(SdioControllerDeviceTest, ProcessCccrWithNoCaps) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(1);
  });

  sdmmc_.Write(0x00, std::vector<uint8_t>{0x43}, 0);  // CCCR/SDIO revision.
  sdmmc_.Write(0x08, std::vector<uint8_t>{0x00}, 0);
  sdmmc_.Write(0x13, std::vector<uint8_t>{0x00}, 0);
  sdmmc_.Write(0x14, std::vector<uint8_t>{0x00}, 0);
  sdmmc_.Write(0x15, std::vector<uint8_t>{0x00}, 0);

  ASSERT_OK(StartDriver());

  fidl::WireClient client = ConnectDeviceClient(1);
  ASSERT_TRUE(client.is_valid());

  using fuchsia_hardware_sdio::SdioDeviceCapabilities;

  client->GetDevHwInfo().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->hw_info.dev_hw_info.caps, SdioDeviceCapabilities{0});
  });
  driver_test().runtime().RunUntilIdle();
}

TEST_F(SdioControllerDeviceTest, ProcessCccrRevisionError1) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(1);
  });

  sdmmc_.Write(0x00, std::vector<uint8_t>{0x41}, 0);  // Incorrect
  sdmmc_.Write(0x08, std::vector<uint8_t>{0x00}, 0);
  sdmmc_.Write(0x13, std::vector<uint8_t>{0x00}, 0);
  sdmmc_.Write(0x14, std::vector<uint8_t>{0x00}, 0);
  sdmmc_.Write(0x15, std::vector<uint8_t>{0x00}, 0);

  EXPECT_NE(StartDriver(), ZX_OK);
}

TEST_F(SdioControllerDeviceTest, ProcessCccrRevisionError2) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(1);
  });

  sdmmc_.Write(0x00, std::vector<uint8_t>{0x33}, 0);  // Incorrect
  sdmmc_.Write(0x08, std::vector<uint8_t>{0x00}, 0);
  sdmmc_.Write(0x13, std::vector<uint8_t>{0x00}, 0);
  sdmmc_.Write(0x14, std::vector<uint8_t>{0x00}, 0);
  sdmmc_.Write(0x15, std::vector<uint8_t>{0x00}, 0);

  EXPECT_NE(StartDriver(), ZX_OK);
}

TEST_F(SdioControllerDeviceTest, ProcessCis) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(5);
  });

  sdmmc_.Write(0x0000'0509, std::vector<uint8_t>{0xa2, 0xc2, 0x00}, 0);  // CIS pointer.

  sdmmc_.Write(0x0000'c2a2,
               std::vector<uint8_t>{
                   0x20,  // Manufacturer ID tuple.
                   0x04,  // Manufacturer ID tuple size.
                   0x01,
                   0xc0,  // Manufacturer code.
                   0xce,
                   0xfa,  // Manufacturer information (part number/revision).
                   0x00,  // Null tuple.
                   0x22,  // Function extensions tuple.
                   0x2a,  // Function extensions tuple size.
                   0x01,  // Type of extended data.
               },
               0);
  sdmmc_.Write(0x0000'c2b7, std::vector<uint8_t>{0x00, 0x01}, 0);  // Function block size.
  sdmmc_.Write(0x0000'c2d5, std::vector<uint8_t>{0x00}, 0);        // End-of-chain tuple.

  ASSERT_OK(StartDriver());

  fidl::WireClient client = ConnectDeviceClient(5);
  ASSERT_TRUE(client.is_valid());

  client->GetDevHwInfo().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());

    EXPECT_EQ(result->value()->hw_info.func_hw_info.max_blk_size, uint32_t{256});
    EXPECT_EQ(result->value()->hw_info.func_hw_info.manufacturer_id, uint32_t{0xc001});
    EXPECT_EQ(result->value()->hw_info.func_hw_info.product_id, uint32_t{0xface});
  });
  driver_test().runtime().RunUntilIdle();
}

TEST_F(SdioControllerDeviceTest, ProcessCisFunction0) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(5);
  });

  sdmmc_.set_host_info({
      .caps = 0,
      .max_transfer_size = 1024,
  });

  sdmmc_.Write(0x0000'0000, std::vector<uint8_t>{0x43}, 0);  // CCCR/SDIO version 3.

  sdmmc_.Write(0x0000'0009, std::vector<uint8_t>{0xf5, 0x61, 0x01}, 0);  // CIS pointer.

  sdmmc_.Write(0x0001'61f5,
               std::vector<uint8_t>{
                   0x22,  // Function extensions tuple.
                   0x04,  // Function extensions tuple size.
                   0x00,  // Type of extended data.
                   0x00,
                   0x02,  // Function 0 block size.
                   0x32,  // Max transfer speed.
                   0x00,  // Null tuple.
                   0x20,  // Manufacturer ID tuple.
                   0x04,  // Manufacturer ID tuple size.
                   0xef,
                   0xbe,  // Manufacturer code.
                   0xfe,
                   0xca,  // Manufacturer information (part number/revision).
                   0xff,  // End-of-chain tuple.
               },
               0);

  ASSERT_OK(StartDriver());

  fidl::WireClient client = ConnectDeviceClient(1);
  ASSERT_TRUE(client.is_valid());

  client->GetDevHwInfo().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());

    EXPECT_EQ(result->value()->hw_info.dev_hw_info.num_funcs, uint32_t{6});
    EXPECT_EQ(result->value()->hw_info.dev_hw_info.sdio_vsn, uint32_t{SDIO_SDIO_VER_3});
    EXPECT_EQ(result->value()->hw_info.dev_hw_info.cccr_vsn, uint32_t{SDIO_CCCR_FORMAT_VER_3});
    EXPECT_EQ(result->value()->hw_info.dev_hw_info.max_tran_speed, uint32_t{25000});
  });
  driver_test().runtime().RunUntilIdle();
}

TEST_F(SdioControllerDeviceTest, ProcessFbr) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(7);
  });

  sdmmc_.Write(0x100, std::vector<uint8_t>{0x83}, 0);
  sdmmc_.Write(0x500, std::vector<uint8_t>{0x00}, 0);
  sdmmc_.Write(0x600, std::vector<uint8_t>{0xcf}, 0);
  sdmmc_.Write(0x601, std::vector<uint8_t>{0xab}, 0);
  sdmmc_.Write(0x700, std::vector<uint8_t>{0x4e}, 0);

  ASSERT_OK(StartDriver());

  fidl::WireClient client1 = ConnectDeviceClient(1);
  ASSERT_TRUE(client1.is_valid());

  fidl::WireClient client5 = ConnectDeviceClient(5);
  ASSERT_TRUE(client5.is_valid());

  fidl::WireClient client6 = ConnectDeviceClient(6);
  ASSERT_TRUE(client6.is_valid());

  fidl::WireClient client7 = ConnectDeviceClient(7);
  ASSERT_TRUE(client7.is_valid());

  client1->GetDevHwInfo().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->hw_info.dev_hw_info.num_funcs, uint32_t{8});
    EXPECT_EQ(result->value()->hw_info.func_hw_info.fn_intf_code, uint8_t{0x03});
  });

  client5->GetDevHwInfo().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->hw_info.func_hw_info.fn_intf_code, uint8_t{0x00});
  });

  client6->GetDevHwInfo().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->hw_info.func_hw_info.fn_intf_code, uint8_t{0xab});
  });

  client7->GetDevHwInfo().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->hw_info.func_hw_info.fn_intf_code, uint8_t{0x0e});
  });

  driver_test().runtime().RunUntilIdle();
}

TEST_F(SdioControllerDeviceTest, ProbeFail) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(5);
  });

  // Set the function 3 CIS pointer to zero. This should cause InitFunc and subsequently Probe
  // to fail.
  sdmmc_.Write(0x0309, std::vector<uint8_t>{0x00, 0x00, 0x00}, 0);

  EXPECT_NE(StartDriver(), ZX_OK);
}

TEST_F(SdioControllerDeviceTest, ProbeSdr104) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(5) | SDIO_SEND_OP_COND_RESP_S18A;
  });

  sdmmc_.Write(0x0014, std::vector<uint8_t>{0x07}, 0);

  sdmmc_.set_host_info({
      .caps = SDMMC_HOST_CAP_VOLTAGE_330 | SDMMC_HOST_CAP_SDR104 | SDMMC_HOST_CAP_SDR50 |
              SDMMC_HOST_CAP_DDR50,
      .max_transfer_size = 0x1000,
  });

  ASSERT_OK(StartDriver());

  EXPECT_EQ(sdmmc_.signal_voltage(), SDMMC_VOLTAGE_V180);
  EXPECT_EQ(sdmmc_.bus_width(), SDMMC_BUS_WIDTH_FOUR);
  EXPECT_EQ(sdmmc_.bus_freq(), uint32_t{208'000'000});
  EXPECT_EQ(sdmmc_.timing(), SDMMC_TIMING_SDR104);
}

TEST_F(SdioControllerDeviceTest, ProbeSdr50LimitedByHost) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(5) | SDIO_SEND_OP_COND_RESP_S18A;
  });

  sdmmc_.Write(0x0014, std::vector<uint8_t>{0x07}, 0);

  sdmmc_.set_host_info({
      .caps = SDMMC_HOST_CAP_VOLTAGE_330 | SDMMC_HOST_CAP_SDR50,
      .max_transfer_size = 0x1000,
  });

  ASSERT_OK(StartDriver());

  EXPECT_EQ(sdmmc_.signal_voltage(), SDMMC_VOLTAGE_V180);
  EXPECT_EQ(sdmmc_.bus_width(), SDMMC_BUS_WIDTH_FOUR);
  EXPECT_EQ(sdmmc_.bus_freq(), uint32_t{100'000'000});
  EXPECT_EQ(sdmmc_.timing(), SDMMC_TIMING_SDR50);
}

TEST_F(SdioControllerDeviceTest, ProbeSdr50LimitedByCard) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(5) | SDIO_SEND_OP_COND_RESP_S18A;
  });

  sdmmc_.Write(0x0014, std::vector<uint8_t>{0x01}, 0);

  sdmmc_.set_host_info({
      .caps = SDMMC_HOST_CAP_VOLTAGE_330 | SDMMC_HOST_CAP_SDR104 | SDMMC_HOST_CAP_SDR50 |
              SDMMC_HOST_CAP_DDR50,
      .max_transfer_size = 0x1000,
  });

  ASSERT_OK(StartDriver());

  EXPECT_EQ(sdmmc_.signal_voltage(), SDMMC_VOLTAGE_V180);
  EXPECT_EQ(sdmmc_.bus_width(), SDMMC_BUS_WIDTH_FOUR);
  EXPECT_EQ(sdmmc_.bus_freq(), uint32_t{100'000'000});
  EXPECT_EQ(sdmmc_.timing(), SDMMC_TIMING_SDR50);
}

TEST_F(SdioControllerDeviceTest, ProbeSetVoltageMax) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(5);
  });

  ASSERT_OK(StartDriver());

  // Card does not report 1.8V support so we don't request a change from the host.
  EXPECT_EQ(sdmmc_.signal_voltage(), SDMMC_VOLTAGE_MAX);
}

TEST_F(SdioControllerDeviceTest, ProbeSetVoltageV180) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(5) | SDIO_SEND_OP_COND_RESP_S18A;
  });

  ASSERT_OK(StartDriver());

  EXPECT_EQ(sdmmc_.signal_voltage(), SDMMC_VOLTAGE_V180);
}

TEST_F(SdioControllerDeviceTest, ProbeRetriesRequests) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(5) | SDIO_SEND_OP_COND_RESP_S18A;
  });
  uint32_t tries = 0;
  sdmmc_.set_command_callback(SDIO_IO_RW_DIRECT, [&](const sdmmc_req_t& req) -> zx_status_t {
    const bool write = req.arg & SDIO_IO_RW_DIRECT_RW_FLAG;
    const uint32_t fn_idx =
        (req.arg & SDIO_IO_RW_DIRECT_FN_IDX_MASK) >> SDIO_IO_RW_DIRECT_FN_IDX_LOC;
    const uint32_t addr =
        (req.arg & SDIO_IO_RW_DIRECT_REG_ADDR_MASK) >> SDIO_IO_RW_DIRECT_REG_ADDR_LOC;

    const bool read_fn0_fbr = !write && (fn_idx == 0) && (addr == SDIO_CIA_FBR_CIS_ADDR);
    return (read_fn0_fbr && tries++ < 7) ? ZX_ERR_IO : ZX_OK;
  });

  ASSERT_OK(StartDriver());
}

TEST_F(SdioControllerDeviceTest, IoAbortSetsAbortFlag) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(5);
  });

  ASSERT_OK(StartDriver());

  fidl::WireClient client = ConnectDeviceClient(3);
  ASSERT_TRUE(client.is_valid());

  sdmmc_.set_command_callback(SDIO_IO_RW_DIRECT, [](const sdmmc_req_t& req) -> void {
    EXPECT_EQ(req.cmd_idx, uint32_t{SDIO_IO_RW_DIRECT});
    EXPECT_FALSE(req.cmd_flags & SDMMC_CMD_TYPE_ABORT);
    EXPECT_EQ(req.arg, uint32_t{0xb024'68ab});
  });
  client->WriteByte(0x1234, 0xab, false, false).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();

  sdmmc_.set_command_callback(SDIO_IO_RW_DIRECT, [](const sdmmc_req_t& req) -> void {
    EXPECT_EQ(req.cmd_idx, uint32_t{SDIO_IO_RW_DIRECT});
    EXPECT_TRUE(req.cmd_flags & SDMMC_CMD_TYPE_ABORT);
    EXPECT_EQ(req.arg, uint32_t{0x8000'0c03});
  });
  client->IoAbort().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();
}

TEST_F(SdioControllerDeviceTest, DifferentManufacturerProductIds) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(4);
  });

  // Function 0-4 CIS pointers.
  sdmmc_.Write(0x0000'0009, std::vector<uint8_t>{0xf5, 0x61, 0x01}, 0);
  sdmmc_.Write(0x0000'0109, std::vector<uint8_t>{0xa0, 0x56, 0x00}, 0);
  sdmmc_.Write(0x0000'0209, std::vector<uint8_t>{0xe9, 0xc3, 0x00}, 0);
  sdmmc_.Write(0x0000'0309, std::vector<uint8_t>{0xb7, 0x6e, 0x01}, 0);
  sdmmc_.Write(0x0000'0409, std::vector<uint8_t>{0x86, 0xb7, 0x00}, 0);

  // clang-format off
  sdmmc_.Write(0x0001'61f5,
               std::vector<uint8_t>{
                   0x22, 0x04,
                   0x00, 0x01, 0x00, 32,
                   0x20,        // Manufacturer ID tuple.
                   0x04,        // Manufacturer ID tuple size.
                   0xef, 0xbe,  // Manufacturer code.
                   0xfe, 0xca,  // Manufacturer information (part number/revision).
                   0xff,        // End-of-chain tuple.
               },
               0);

  sdmmc_.Write(0x0000'56a0,
               std::vector<uint8_t>{
                   0x20, 0x04,            // Manufacturer ID tuple.
                   0x7b, 0x31,
                   0x8f, 0xa8,
                   0x22, 0x2a,            // Function extensions tuple.
                   0x01,
                   0, 0, 0, 0,0, 0, 0, 0, // Padding to max block size field.
                   0x01, 0x00,            // Max block size.
               },
               0);

  sdmmc_.Write(0x0000'c3e9,
               std::vector<uint8_t>{
                   0x20, 0x04,
                   0xbd, 0x6d,
                   0x0d, 0x24,
                   0x22, 0x2a,
                   0x01,
                   0, 0, 0, 0,0, 0, 0, 0,
                   0x01, 0x00,
               },
               0);

  sdmmc_.Write(0x0001'6eb7,
               std::vector<uint8_t>{
                   0x20, 0x04,
                   0xca, 0xb8,
                   0x52, 0x98,
                   0x22, 0x2a,
                   0x01,
                   0, 0, 0, 0,0, 0, 0, 0,
                   0x01, 0x00,
               },
               0);

  sdmmc_.Write(0x0000'b786,
               std::vector<uint8_t>{
                   0x20, 0x04,
                   0xee, 0xf5,
                   0xde, 0x30,
                   0x22, 0x2a,
                   0x01,
                   0, 0, 0, 0,0, 0, 0, 0,
                   0x01, 0x00,
               },
               0);
  // clang-format on

  ASSERT_OK(StartDriver());

  const std::pair<std::string, uint32_t> kExpectedProps[4][4] = {
      {
          {bind_fuchsia::PROTOCOL, bind_fuchsia_sdio::BIND_PROTOCOL_DEVICE},
          {bind_fuchsia::SDIO_VID, 0x317b},
          {bind_fuchsia::SDIO_PID, 0xa88f},
          {bind_fuchsia::SDIO_FUNCTION, 1},
      },
      {
          {bind_fuchsia::PROTOCOL, bind_fuchsia_sdio::BIND_PROTOCOL_DEVICE},
          {bind_fuchsia::SDIO_VID, 0x6dbd},
          {bind_fuchsia::SDIO_PID, 0x240d},
          {bind_fuchsia::SDIO_FUNCTION, 2},
      },
      {
          {bind_fuchsia::PROTOCOL, bind_fuchsia_sdio::BIND_PROTOCOL_DEVICE},
          {bind_fuchsia::SDIO_VID, 0xb8ca},
          {bind_fuchsia::SDIO_PID, 0x9852},
          {bind_fuchsia::SDIO_FUNCTION, 3},
      },
      {
          {bind_fuchsia::PROTOCOL, bind_fuchsia_sdio::BIND_PROTOCOL_DEVICE},
          {bind_fuchsia::SDIO_VID, 0xf5ee},
          {bind_fuchsia::SDIO_PID, 0x30de},
          {bind_fuchsia::SDIO_FUNCTION, 4},
      },
  };

  driver_test().RunInNodeContext([&](fdf_testing::TestNode& node) {
    fdf_testing::TestNode& sdmmc_node = node.children().at("sdmmc");
    fdf_testing::TestNode& controller_node = sdmmc_node.children().at("sdmmc-sdio");
    EXPECT_EQ(controller_node.children().size(), std::size(kExpectedProps) * 2);

    for (size_t i = 0; i < std::size(kExpectedProps); i++) {
      const std::string node_name = "sdmmc-sdio-" + std::to_string(i + 1);
      fdf_testing::TestNode& function_node = controller_node.children().at(node_name);

      std::vector properties = function_node.GetProperties();
      ASSERT_GE(properties.size(), std::size(kExpectedProps[0]));
      for (size_t j = 0; j < std::size(kExpectedProps[0]); j++) {
        const fuchsia_driver_framework::NodeProperty2& prop = properties[j];
        EXPECT_EQ(prop.key(), kExpectedProps[i][j].first);
        EXPECT_EQ(prop.value().int_value().value(), kExpectedProps[i][j].second);
      }

      const std::string driver_node_name = node_name + "-driver";
      fdf_testing::TestNode& driver_node = controller_node.children().at(driver_node_name);

      std::vector driver_properties = driver_node.GetProperties();
      ASSERT_GE(driver_properties.size(), std::size(kExpectedProps[0]));
      for (size_t j = 0; j < std::size(kExpectedProps[0]); j++) {
        const fuchsia_driver_framework::NodeProperty2& prop = driver_properties[j];
        EXPECT_EQ(prop.key(), kExpectedProps[i][j].first);
        EXPECT_EQ(prop.value().int_value().value(), kExpectedProps[i][j].second);
      }
    }
  });
}

TEST_F(SdioControllerDeviceTest, FunctionZeroInvalidBlockSize) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(4);
  });

  sdmmc_.Write(0x2000, std::vector<uint8_t>{0x22, 0x04, 0x00, 0x00, 0x00}, 0);

  sdmmc_.Write(0x0009, std::vector<uint8_t>{0x00, 0x20, 0x00}, 0);

  EXPECT_NE(StartDriver(), ZX_OK);
}

TEST_F(SdioControllerDeviceTest, IOFunctionInvalidBlockSize) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(4);
  });

  sdmmc_.Write(0x3000, std::vector<uint8_t>{0x22, 0x2a, 0x01}, 0);
  sdmmc_.Write(0x300e, std::vector<uint8_t>{0x00, 0x00}, 0);

  sdmmc_.Write(0x0209, std::vector<uint8_t>{0x00, 0x30, 0x00}, 0);

  EXPECT_NE(StartDriver(), ZX_OK);
}

TEST_F(SdioControllerDeviceTest, FunctionZeroNoBlockSize) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(4);
  });

  sdmmc_.Write(0x3000, std::vector<uint8_t>{0xff}, 0);

  sdmmc_.Write(0x0009, std::vector<uint8_t>{0x00, 0x30, 0x00}, 0);

  EXPECT_NE(StartDriver(), ZX_OK);
}

TEST_F(SdioControllerDeviceTest, IOFunctionNoBlockSize) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(4);
  });

  sdmmc_.Write(0x3000, std::vector<uint8_t>{0xff}, 0);

  sdmmc_.Write(0x0209, std::vector<uint8_t>{0x00, 0x30, 0x00}, 0);

  EXPECT_NE(StartDriver(), ZX_OK);
}

TEST_F(SdioControllerDeviceTest, UpdateBlockSizeMultiBlock) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(4);
  });
  sdmmc_.Write(SDIO_CIA_CCCR_CARD_CAPS_ADDR, std::vector<uint8_t>{SDIO_CIA_CCCR_CARD_CAP_SMB}, 0);

  sdmmc_.Write(0x3000, std::vector<uint8_t>{0x22, 0x2a, 0x01}, 0);
  sdmmc_.Write(0x300e, std::vector<uint8_t>{0x00, 0x02}, 0);

  sdmmc_.Write(0x0209, std::vector<uint8_t>{0x00, 0x30, 0x00}, 0);

  sdmmc_.set_host_info({
      .caps = 0,
      .max_transfer_size = 2048,
  });

  sdmmc_.Write(0x210, std::vector<uint8_t>{0x00, 0x00}, 0);

  ASSERT_OK(StartDriver());

  fidl::WireClient client = ConnectDeviceClient(2);
  ASSERT_TRUE(client.is_valid());

  EXPECT_EQ(sdmmc_.Read(0x210, 2)[0], 0x00);
  EXPECT_EQ(sdmmc_.Read(0x210, 2)[1], 0x02);

  client->GetBlockSize().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->cur_blk_size, 512);
  });
  client->UpdateBlockSize(128, false).ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());

    EXPECT_EQ(sdmmc_.Read(0x210, 2)[0], 0x80);
    EXPECT_EQ(sdmmc_.Read(0x210, 2)[1], 0x00);
  });

  client->GetBlockSize().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->cur_blk_size, 128);
  });
  client->UpdateBlockSize(0, true).ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());

    EXPECT_EQ(sdmmc_.Read(0x210, 2)[0], 0x00);
    EXPECT_EQ(sdmmc_.Read(0x210, 2)[1], 0x02);
  });

  client->GetBlockSize().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->cur_blk_size, 512);
  });
  client->UpdateBlockSize(0, false).ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_FALSE(result->is_ok());

    EXPECT_EQ(sdmmc_.Read(0x210, 2)[0], 0x00);
    EXPECT_EQ(sdmmc_.Read(0x210, 2)[1], 0x02);
  });

  client->GetBlockSize().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->cur_blk_size, 512);
  });
  client->UpdateBlockSize(1024, false).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_FALSE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();
}

TEST_F(SdioControllerDeviceTest, UpdateBlockSizeNoMultiBlock) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(4);
  });
  sdmmc_.Write(SDIO_CIA_CCCR_CARD_CAPS_ADDR, std::vector<uint8_t>{0}, 0);

  sdmmc_.Write(0x3000, std::vector<uint8_t>{0x22, 0x2a, 0x01}, 0);
  sdmmc_.Write(0x300e, std::vector<uint8_t>{0x00, 0x02}, 0);

  sdmmc_.Write(0x0209, std::vector<uint8_t>{0x00, 0x30, 0x00}, 0);

  sdmmc_.set_host_info({
      .caps = 0,
      .max_transfer_size = 2048,
  });

  // Placeholder value that should not get written or returned.
  sdmmc_.Write(0x210, std::vector<uint8_t>{0xa5, 0xa5}, 0);

  ASSERT_OK(StartDriver());

  fidl::WireClient client = ConnectDeviceClient(2);
  ASSERT_TRUE(client.is_valid());

  EXPECT_EQ(sdmmc_.Read(0x210, 2)[0], 0xa5);
  EXPECT_EQ(sdmmc_.Read(0x210, 2)[1], 0xa5);

  client->GetBlockSize().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->cur_blk_size, 512);
  });

  client->UpdateBlockSize(128, false).ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());

    EXPECT_EQ(sdmmc_.Read(0x210, 2)[0], 0xa5);
    EXPECT_EQ(sdmmc_.Read(0x210, 2)[1], 0xa5);
  });

  client->GetBlockSize().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->cur_blk_size, 128);
  });

  client->UpdateBlockSize(0, true).ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());

    EXPECT_EQ(sdmmc_.Read(0x210, 2)[0], 0xa5);
    EXPECT_EQ(sdmmc_.Read(0x210, 2)[1], 0xa5);
  });

  client->GetBlockSize().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->cur_blk_size, 512);
  });

  client->UpdateBlockSize(0, false).ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_FALSE(result->is_ok());

    EXPECT_EQ(sdmmc_.Read(0x210, 2)[0], 0xa5);
    EXPECT_EQ(sdmmc_.Read(0x210, 2)[1], 0xa5);
  });

  client->GetBlockSize().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->cur_blk_size, 512);
  });

  client->UpdateBlockSize(1024, false).ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_FALSE(result->is_ok());
  });

  driver_test().runtime().RunUntilIdle();
}

TEST_F(SdioScatterGatherTest, ScatterGatherByteMode) {
  Init(3, true);

  fidl::WireClient client = ConnectDeviceClient(3);
  ASSERT_TRUE(client.is_valid());

  memcpy(mapper1_.start(), kTestData1, sizeof(kTestData1));
  memcpy(mapper2_.start(), kTestData2, sizeof(kTestData2));
  memcpy(mapper3_.start(), kTestData3, sizeof(kTestData3));

  SdmmcBufferRegion buffers[3];
  buffers[0] = MakeBufferRegion(1, 8, 2);
  buffers[1] = MakeBufferRegion(vmo2_, 4, 1);
  buffers[2] = MakeBufferRegion(3, 0, 2);

  SdioRwTxn txn = {
      .addr = 0x1000,
      .incr = true,
      .write = true,
      .buffers = fidl::VectorView<SdmmcBufferRegion>::FromExternal(buffers, std::size(buffers)),
  };
  client->DoRwTxn(std::move(txn)).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();

  std::vector<uint8_t> actual = sdmmc_.Read(0x1000, 6, 3);
  EXPECT_EQ(0, memcmp(actual.data(), kTestData1 + 8, 2));
  EXPECT_EQ(0, memcmp(actual.data() + 2, kTestData2 + 4, 1));
  EXPECT_EQ(0, memcmp(actual.data() + 3, kTestData3 + 8, 2));
  EXPECT_EQ(actual[5], 0xff);

  ASSERT_EQ(sdmmc_.requests().size(), size_t{2});

  const SdioCmd53 req1 = SdioCmd53::FromArg(sdmmc_.requests()[0].arg);
  EXPECT_EQ(req1.blocks_or_bytes, uint32_t{4});
  EXPECT_EQ(req1.address, uint32_t{0x1000});
  EXPECT_EQ(req1.op_code, uint32_t{1});
  EXPECT_EQ(req1.block_mode, uint32_t{0});
  EXPECT_EQ(req1.function_number, uint32_t{3});
  EXPECT_EQ(req1.rw_flag, uint32_t{1});

  const SdioCmd53 req2 = SdioCmd53::FromArg(sdmmc_.requests()[1].arg);
  EXPECT_EQ(req2.blocks_or_bytes, uint32_t{1});
  EXPECT_EQ(req2.address, uint32_t{0x1000 + 4});
  EXPECT_EQ(req2.op_code, uint32_t{1});
  EXPECT_EQ(req2.block_mode, uint32_t{0});
  EXPECT_EQ(req2.function_number, uint32_t{3});
  EXPECT_EQ(req2.rw_flag, uint32_t{1});
}

TEST_F(SdioScatterGatherTest, ScatterGatherBlockMode) {
  Init(3, true);

  fidl::WireClient client = ConnectDeviceClient(3);
  ASSERT_TRUE(client.is_valid());

  SdmmcBufferRegion buffers[3];
  buffers[0] = MakeBufferRegion(1, 8, 7);
  buffers[1] = MakeBufferRegion(vmo2_, 4, 3);
  buffers[2] = MakeBufferRegion(3, 10, 5);

  sdmmc_.Write(0x5000, cpp20::span(kTestData1, std::size(kTestData1)), 3);

  SdioRwTxn txn = {
      .addr = 0x5000,
      .incr = false,
      .write = false,
      .buffers = fidl::VectorView<SdmmcBufferRegion>::FromExternal(buffers, std::size(buffers)),
  };
  client->DoRwTxn(std::move(txn)).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();

  EXPECT_EQ(0, memcmp(static_cast<uint8_t*>(mapper1_.start()) + 8, kTestData1, 7));
  EXPECT_EQ(0, memcmp(static_cast<uint8_t*>(mapper2_.start()) + 4, kTestData1 + 7, 3));
  EXPECT_EQ(0, memcmp(static_cast<uint8_t*>(mapper3_.start()) + 18, kTestData1 + 10, 2));

  ASSERT_EQ(sdmmc_.requests().size(), size_t{2});

  const SdioCmd53 req1 = SdioCmd53::FromArg(sdmmc_.requests()[0].arg);
  EXPECT_EQ(req1.blocks_or_bytes, uint32_t{3});
  EXPECT_EQ(req1.address, uint32_t{0x5000});
  EXPECT_EQ(req1.op_code, uint32_t{0});
  EXPECT_EQ(req1.block_mode, uint32_t{1});
  EXPECT_EQ(req1.function_number, uint32_t{3});
  EXPECT_EQ(req1.rw_flag, uint32_t{0});

  const SdioCmd53 req2 = SdioCmd53::FromArg(sdmmc_.requests()[1].arg);
  EXPECT_EQ(req2.blocks_or_bytes, uint32_t{3});
  EXPECT_EQ(req2.address, uint32_t{0x5000});
  EXPECT_EQ(req2.op_code, uint32_t{0});
  EXPECT_EQ(req2.block_mode, uint32_t{0});
  EXPECT_EQ(req2.function_number, uint32_t{3});
  EXPECT_EQ(req2.rw_flag, uint32_t{0});
}

TEST_F(SdioScatterGatherTest, ScatterGatherBlockModeNoMultiBlock) {
  Init(5, false);

  fidl::WireClient client = ConnectDeviceClient(5);
  ASSERT_TRUE(client.is_valid());

  memcpy(mapper1_.start(), kTestData1, sizeof(kTestData1));
  memcpy(mapper2_.start(), kTestData2, sizeof(kTestData2));
  memcpy(mapper3_.start(), kTestData3, sizeof(kTestData3));

  SdmmcBufferRegion buffers[3];
  buffers[0] = MakeBufferRegion(1, 8, 7);
  buffers[1] = MakeBufferRegion(vmo2_, 4, 3);
  buffers[2] = MakeBufferRegion(3, 0, 5);

  SdioRwTxn txn = {
      .addr = 0x1000,
      .incr = true,
      .write = true,
      .buffers = fidl::VectorView<SdmmcBufferRegion>::FromExternal(buffers, std::size(buffers)),
  };
  client->DoRwTxn(std::move(txn)).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();

  std::vector<uint8_t> actual = sdmmc_.Read(0x1000, 16, 5);
  EXPECT_EQ(0, memcmp(actual.data(), kTestData1 + 8, 7));
  EXPECT_EQ(0, memcmp(actual.data() + 7, kTestData2 + 4, 3));
  EXPECT_EQ(0, memcmp(actual.data() + 10, kTestData3 + 8, 5));
  EXPECT_EQ(actual[15], 0xff);

  ASSERT_EQ(sdmmc_.requests().size(), size_t{4});

  const SdioCmd53 req1 = SdioCmd53::FromArg(sdmmc_.requests()[0].arg);
  EXPECT_EQ(req1.blocks_or_bytes, uint32_t{4});
  EXPECT_EQ(req1.address, uint32_t{0x1000});
  EXPECT_EQ(req1.op_code, uint32_t{1});
  EXPECT_EQ(req1.block_mode, uint32_t{0});
  EXPECT_EQ(req1.function_number, uint32_t{5});
  EXPECT_EQ(req1.rw_flag, uint32_t{1});

  const SdioCmd53 req2 = SdioCmd53::FromArg(sdmmc_.requests()[1].arg);
  EXPECT_EQ(req2.blocks_or_bytes, uint32_t{4});
  EXPECT_EQ(req2.address, uint32_t{0x1000 + 4});
  EXPECT_EQ(req2.op_code, uint32_t{1});
  EXPECT_EQ(req2.block_mode, uint32_t{0});
  EXPECT_EQ(req2.function_number, uint32_t{5});
  EXPECT_EQ(req2.rw_flag, uint32_t{1});

  const SdioCmd53 req3 = SdioCmd53::FromArg(sdmmc_.requests()[2].arg);
  EXPECT_EQ(req3.blocks_or_bytes, uint32_t{4});
  EXPECT_EQ(req3.address, uint32_t{0x1000 + 8});
  EXPECT_EQ(req3.op_code, uint32_t{1});
  EXPECT_EQ(req3.block_mode, uint32_t{0});
  EXPECT_EQ(req3.function_number, uint32_t{5});
  EXPECT_EQ(req3.rw_flag, uint32_t{1});

  const SdioCmd53 req4 = SdioCmd53::FromArg(sdmmc_.requests()[3].arg);
  EXPECT_EQ(req4.blocks_or_bytes, uint32_t{3});
  EXPECT_EQ(req4.address, uint32_t{0x1000 + 12});
  EXPECT_EQ(req4.op_code, uint32_t{1});
  EXPECT_EQ(req4.block_mode, uint32_t{0});
  EXPECT_EQ(req4.function_number, uint32_t{5});
  EXPECT_EQ(req4.rw_flag, uint32_t{1});
}

TEST_F(SdioScatterGatherTest, ScatterGatherBlockModeMultipleFinalBuffers) {
  Init(1, true);

  fidl::WireClient client = ConnectDeviceClient(1);
  ASSERT_TRUE(client.is_valid());

  sdmmc_.Write(0x3000, cpp20::span(kTestData1, std::size(kTestData1)), 1);

  SdmmcBufferRegion buffers[4];
  buffers[0] = MakeBufferRegion(1, 8, 7);
  buffers[1] = MakeBufferRegion(vmo2_, 4, 3);
  buffers[2] = MakeBufferRegion(3, 0, 3);
  buffers[3] = MakeBufferRegion(1, 0, 2);

  SdioRwTxn txn = {
      .addr = 0x3000,
      .incr = true,
      .write = false,
      .buffers = fidl::VectorView<SdmmcBufferRegion>::FromExternal(buffers, std::size(buffers)),
  };
  client->DoRwTxn(std::move(txn)).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();

  EXPECT_EQ(0, memcmp(static_cast<uint8_t*>(mapper1_.start()) + 8, kTestData1, 7));
  EXPECT_EQ(0, memcmp(static_cast<uint8_t*>(mapper2_.start()) + 4, kTestData1 + 7, 3));
  EXPECT_EQ(0, memcmp(static_cast<uint8_t*>(mapper3_.start()) + 8, kTestData1 + 10, 3));
  EXPECT_EQ(0, memcmp(static_cast<uint8_t*>(mapper1_.start()), kTestData1 + 13, 2));

  ASSERT_EQ(sdmmc_.requests().size(), size_t{2});

  const SdioCmd53 req1 = SdioCmd53::FromArg(sdmmc_.requests()[0].arg);
  EXPECT_EQ(req1.blocks_or_bytes, uint32_t{3});
  EXPECT_EQ(req1.address, uint32_t{0x3000});
  EXPECT_EQ(req1.op_code, uint32_t{1});
  EXPECT_EQ(req1.block_mode, uint32_t{1});
  EXPECT_EQ(req1.function_number, uint32_t{1});
  EXPECT_EQ(req1.rw_flag, uint32_t{0});

  const SdioCmd53 req2 = SdioCmd53::FromArg(sdmmc_.requests()[1].arg);
  EXPECT_EQ(req2.blocks_or_bytes, uint32_t{3});
  EXPECT_EQ(req2.address, uint32_t{0x3000 + 12});
  EXPECT_EQ(req2.op_code, uint32_t{1});
  EXPECT_EQ(req2.block_mode, uint32_t{0});
  EXPECT_EQ(req2.function_number, uint32_t{1});
  EXPECT_EQ(req2.rw_flag, uint32_t{0});
}

TEST_F(SdioScatterGatherTest, ScatterGatherBlockModeLastAligned) {
  Init(3, true);

  fidl::WireClient client = ConnectDeviceClient(3);
  ASSERT_TRUE(client.is_valid());

  memcpy(mapper1_.start(), kTestData1, sizeof(kTestData1));
  memcpy(mapper2_.start(), kTestData2, sizeof(kTestData2));
  memcpy(mapper3_.start(), kTestData3, sizeof(kTestData3));

  SdmmcBufferRegion buffers[3];
  buffers[0] = MakeBufferRegion(1, 8, 7);
  buffers[1] = MakeBufferRegion(vmo2_, 4, 5);
  buffers[2] = MakeBufferRegion(3, 0, 3);

  SdioRwTxn txn = {
      .addr = 0x1000,
      .incr = true,
      .write = true,
      .buffers = fidl::VectorView<SdmmcBufferRegion>::FromExternal(buffers, std::size(buffers)),
  };
  client->DoRwTxn(std::move(txn)).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();

  std::vector<uint8_t> actual = sdmmc_.Read(0x1000, 16, 3);
  EXPECT_EQ(0, memcmp(actual.data(), kTestData1 + 8, 7));
  EXPECT_EQ(0, memcmp(actual.data() + 7, kTestData2 + 4, 5));
  EXPECT_EQ(0, memcmp(actual.data() + 12, kTestData3 + 8, 3));
  EXPECT_EQ(actual[15], 0xff);

  ASSERT_EQ(sdmmc_.requests().size(), size_t{2});

  const SdioCmd53 req1 = SdioCmd53::FromArg(sdmmc_.requests()[0].arg);
  EXPECT_EQ(req1.blocks_or_bytes, uint32_t{3});
  EXPECT_EQ(req1.address, uint32_t{0x1000});
  EXPECT_EQ(req1.op_code, uint32_t{1});
  EXPECT_EQ(req1.block_mode, uint32_t{1});
  EXPECT_EQ(req1.function_number, uint32_t{3});
  EXPECT_EQ(req1.rw_flag, uint32_t{1});

  const SdioCmd53 req2 = SdioCmd53::FromArg(sdmmc_.requests()[1].arg);
  EXPECT_EQ(req2.blocks_or_bytes, uint32_t{3});
  EXPECT_EQ(req2.address, uint32_t{0x1000 + 12});
  EXPECT_EQ(req2.op_code, uint32_t{1});
  EXPECT_EQ(req2.block_mode, uint32_t{0});
  EXPECT_EQ(req2.function_number, uint32_t{3});
  EXPECT_EQ(req2.rw_flag, uint32_t{1});
}

TEST_F(SdioScatterGatherTest, ScatterGatherOnlyFullBlocks) {
  Init(3, true);

  fidl::WireClient client = ConnectDeviceClient(3);
  ASSERT_TRUE(client.is_valid());

  memcpy(mapper1_.start(), kTestData1, sizeof(kTestData1));
  memcpy(mapper2_.start(), kTestData2, sizeof(kTestData2));
  memcpy(mapper3_.start(), kTestData3, sizeof(kTestData3));

  SdmmcBufferRegion buffers[3];
  buffers[0] = MakeBufferRegion(1, 8, 7);
  buffers[1] = MakeBufferRegion(vmo2_, 4, 5);
  buffers[2] = MakeBufferRegion(3, 0, 4);

  SdioRwTxn txn = {
      .addr = 0x1000,
      .incr = true,
      .write = true,
      .buffers = fidl::VectorView<SdmmcBufferRegion>::FromExternal(buffers, std::size(buffers)),
  };
  client->DoRwTxn(std::move(txn)).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();

  std::vector<uint8_t> actual = sdmmc_.Read(0x1000, 17, 3);
  EXPECT_EQ(0, memcmp(actual.data(), kTestData1 + 8, 7));
  EXPECT_EQ(0, memcmp(actual.data() + 7, kTestData2 + 4, 5));
  EXPECT_EQ(0, memcmp(actual.data() + 12, kTestData3 + 8, 4));
  EXPECT_EQ(actual[16], 0xff);

  ASSERT_EQ(sdmmc_.requests().size(), size_t{1});

  const SdioCmd53 req1 = SdioCmd53::FromArg(sdmmc_.requests()[0].arg);
  EXPECT_EQ(req1.blocks_or_bytes, uint32_t{4});
  EXPECT_EQ(req1.address, uint32_t{0x1000});
  EXPECT_EQ(req1.op_code, uint32_t{1});
  EXPECT_EQ(req1.block_mode, uint32_t{1});
  EXPECT_EQ(req1.function_number, uint32_t{3});
  EXPECT_EQ(req1.rw_flag, uint32_t{1});
}

TEST_F(SdioScatterGatherTest, ScatterGatherOverMaxTransferSize) {
  Init(3, true);

  fidl::WireClient client = ConnectDeviceClient(3);
  ASSERT_TRUE(client.is_valid());

  memcpy(mapper1_.start(), kTestData1, sizeof(kTestData1));
  memcpy(mapper2_.start(), kTestData2, sizeof(kTestData2));
  memcpy(mapper3_.start(), kTestData3, sizeof(kTestData3));

  SdmmcBufferRegion buffers[3];
  buffers[0] = MakeBufferRegion(1, 8, 300 * 4);
  buffers[1] = MakeBufferRegion(vmo2_, 4, 800 * 4);
  buffers[2] = MakeBufferRegion(3, 0, 100);

  SdioRwTxn txn = {
      .addr = 0x1000,
      .incr = true,
      .write = true,
      .buffers = fidl::VectorView<SdmmcBufferRegion>::FromExternal(buffers, std::size(buffers)),
  };
  client->DoRwTxn(std::move(txn)).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();

  ASSERT_EQ(sdmmc_.requests().size(), size_t{3});

  const SdioCmd53 req1 = SdioCmd53::FromArg(sdmmc_.requests()[0].arg);
  EXPECT_EQ(req1.blocks_or_bytes, uint32_t{511});
  EXPECT_EQ(req1.address, uint32_t{0x1000});
  EXPECT_EQ(req1.op_code, uint32_t{1});
  EXPECT_EQ(req1.block_mode, uint32_t{1});
  EXPECT_EQ(req1.function_number, uint32_t{3});
  EXPECT_EQ(req1.rw_flag, uint32_t{1});

  const SdioCmd53 req2 = SdioCmd53::FromArg(sdmmc_.requests()[1].arg);
  EXPECT_EQ(req2.blocks_or_bytes, uint32_t{511});
  EXPECT_EQ(req2.address, uint32_t{0x1000 + (511 * 4)});
  EXPECT_EQ(req2.op_code, uint32_t{1});
  EXPECT_EQ(req2.block_mode, uint32_t{1});
  EXPECT_EQ(req2.function_number, uint32_t{3});
  EXPECT_EQ(req2.rw_flag, uint32_t{1});

  const SdioCmd53 req3 = SdioCmd53::FromArg(sdmmc_.requests()[2].arg);
  EXPECT_EQ(req3.blocks_or_bytes, uint32_t{103});
  EXPECT_EQ(req3.address, uint32_t{0x1000 + (511 * 4 * 2)});
  EXPECT_EQ(req3.op_code, uint32_t{1});
  EXPECT_EQ(req3.block_mode, uint32_t{1});
  EXPECT_EQ(req3.function_number, uint32_t{3});
  EXPECT_EQ(req3.rw_flag, uint32_t{1});
}

TEST_F(SdioControllerDeviceTest, RequestCardReset) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(5) | SDIO_SEND_OP_COND_RESP_S18A;
  });

  sdmmc_.Write(0x0014, std::vector<uint8_t>{0x07}, 0);

  sdmmc_.set_host_info({
      .caps = SDMMC_HOST_CAP_VOLTAGE_330 | SDMMC_HOST_CAP_SDR104 | SDMMC_HOST_CAP_SDR50 |
              SDMMC_HOST_CAP_DDR50,
      .max_transfer_size = 0x1000,
  });

  ASSERT_OK(StartDriver());

  fidl::WireClient client = ConnectDeviceClient(1);
  ASSERT_TRUE(client.is_valid());

  EXPECT_EQ(sdmmc_.signal_voltage(), SDMMC_VOLTAGE_V180);
  EXPECT_EQ(sdmmc_.bus_width(), SDMMC_BUS_WIDTH_FOUR);
  EXPECT_EQ(sdmmc_.bus_freq(), uint32_t{208'000'000});
  EXPECT_EQ(sdmmc_.timing(), SDMMC_TIMING_SDR104);

  client->RequestCardReset().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });
  driver_test().runtime().RunUntilIdle();

  EXPECT_EQ(sdmmc_.signal_voltage(), SDMMC_VOLTAGE_V180);
  EXPECT_EQ(sdmmc_.bus_width(), SDMMC_BUS_WIDTH_FOUR);
  EXPECT_EQ(sdmmc_.bus_freq(), uint32_t{208'000'000});
  EXPECT_EQ(sdmmc_.timing(), SDMMC_TIMING_SDR104);
}

TEST_F(SdioControllerDeviceTest, PerformTuning) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(2) | SDIO_SEND_OP_COND_RESP_S18A;
  });
  sdmmc_.set_host_info({
      .caps = SDMMC_HOST_CAP_VOLTAGE_330 | SDMMC_HOST_CAP_SDR104,
      .max_transfer_size = 0x1000,
  });

  ASSERT_OK(StartDriver());

  fidl::WireClient client = ConnectDeviceClient(1);
  ASSERT_TRUE(client.is_valid());

  client->PerformTuning().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  });

  driver_test().runtime().RunUntilIdle();
}

TEST_F(SdioControllerDeviceTest, IoReady) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(5) | SDIO_SEND_OP_COND_RESP_S18A;
  });
  sdmmc_.set_host_info({
      .caps = SDMMC_HOST_CAP_VOLTAGE_330,
      .max_transfer_size = 0x1000,
  });

  ASSERT_OK(StartDriver());

  fidl::WireClient function1 = ConnectDeviceClient(1);
  ASSERT_TRUE(function1.is_valid());

  fidl::WireClient function2 = ConnectDeviceClient(2);
  ASSERT_TRUE(function2.is_valid());

  fidl::WireClient function5 = ConnectDeviceClient(5);
  ASSERT_TRUE(function5.is_valid());

  sdmmc_.Write(0x0003, std::vector<uint8_t>{0b0010'0100}, 0);

  function2->IoReady().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_TRUE(result->value()->ready);
  });

  function5->IoReady().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_TRUE(result->value()->ready);
  });

  function1->IoReady().ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_FALSE(result->value()->ready);
  });

  driver_test().runtime().RunUntilIdle();

  sdmmc_.Write(0x0003, std::vector<uint8_t>{0b0000'0010}, 0);

  function5->IoReady().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_FALSE(result->value()->ready);
  });

  function1->IoReady().ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_TRUE(result->value()->ready);
  });

  driver_test().runtime().RunUntilIdle();
}

TEST_F(SdioControllerDeviceTest, ConfigurePowerManagement) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(3) | SDIO_SEND_OP_COND_RESP_S18A;
  });
  sdmmc_.set_host_info({
      .caps = SDMMC_HOST_CAP_VOLTAGE_330,
      .max_transfer_size = 0x1000,
  });

  // Call the driver's Start() method, and verify that it acquired leases on all three function
  // power elements at the BOOT level.
  ASSERT_OK(StartDriver(true));

  std::vector<uint8_t> lease_power_levels =
      driver_test().RunInEnvironmentTypeContext(fit::callback<std::vector<uint8_t>(Environment&)>(
          [](Environment& env) { return env.fake_power_broker().lease_power_levels(); }));
  ASSERT_EQ(lease_power_levels.size(), 3ul);
  EXPECT_EQ(lease_power_levels[0], SdioFunctionDevice::kBoot);
  EXPECT_EQ(lease_power_levels[1], SdioFunctionDevice::kBoot);
  EXPECT_EQ(lease_power_levels[2], SdioFunctionDevice::kBoot);

  std::vector element_runner_client_ends = driver_test().RunInEnvironmentTypeContext(
      fit::callback<std::vector<fidl::ClientEnd<fuchsia_power_broker::ElementRunner>>(
          Environment&)>(
          [](Environment& env) { return env.fake_power_broker().TakeElementRunnerClientEnds(); }));
  element_runner_client_ends.erase(element_runner_client_ends.begin());
  ASSERT_EQ(element_runner_client_ends.size(), 3ul);

  fidl::Client<fuchsia_power_broker::ElementRunner> function1(
      std::move(element_runner_client_ends[0]), fdf::Dispatcher::GetCurrent()->async_dispatcher());
  fidl::Client<fuchsia_power_broker::ElementRunner> function2(
      std::move(element_runner_client_ends[1]), fdf::Dispatcher::GetCurrent()->async_dispatcher());
  fidl::Client<fuchsia_power_broker::ElementRunner> function3(
      std::move(element_runner_client_ends[2]), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  // Do the initial SetLevel call and make sure that each element responds.
  uint32_t results = 0;
  function1->SetLevel(SdioFunctionDevice::kBoot)
      .ThenExactlyOnce(
          [&results](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
            EXPECT_TRUE(result.is_ok());
            results++;
          });
  function2->SetLevel(SdioFunctionDevice::kBoot)
      .ThenExactlyOnce(
          [&results](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
            EXPECT_TRUE(result.is_ok());
            results++;
          });
  function3->SetLevel(SdioFunctionDevice::kBoot)
      .ThenExactlyOnce(
          [&results](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
            EXPECT_TRUE(result.is_ok());
            results++;
          });

  driver_test().runtime().RunUntilIdle();
  EXPECT_EQ(results, 3u);
}

TEST_F(SdioControllerDeviceTest, OnStateDropsBootLease) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(3) | SDIO_SEND_OP_COND_RESP_S18A;
  });
  sdmmc_.set_host_info({
      .caps = SDMMC_HOST_CAP_VOLTAGE_330,
      .max_transfer_size = 0x1000,
  });

  ASSERT_OK(StartDriver(true));

  std::vector lease_control_server_ends = driver_test().RunInEnvironmentTypeContext(
      fit::callback<std::vector<fidl::ServerEnd<fuchsia_power_broker::LeaseControl>>(Environment&)>(
          [](Environment& env) { return env.fake_power_broker().TakeLeaseControlServerEnds(); }));
  ASSERT_EQ(lease_control_server_ends.size(), 3ul);

  zx_signals_t observed{};
  EXPECT_EQ(lease_control_server_ends[0].channel().wait_one(ZX_CHANNEL_PEER_CLOSED,
                                                            zx::time::infinite_past(), &observed),
            ZX_ERR_TIMED_OUT);
  EXPECT_FALSE(observed & ZX_CHANNEL_PEER_CLOSED);

  EXPECT_EQ(lease_control_server_ends[1].channel().wait_one(ZX_CHANNEL_PEER_CLOSED,
                                                            zx::time::infinite_past(), &observed),
            ZX_ERR_TIMED_OUT);
  EXPECT_FALSE(observed & ZX_CHANNEL_PEER_CLOSED);

  EXPECT_EQ(lease_control_server_ends[2].channel().wait_one(ZX_CHANNEL_PEER_CLOSED,
                                                            zx::time::infinite_past(), &observed),
            ZX_ERR_TIMED_OUT);
  EXPECT_FALSE(observed & ZX_CHANNEL_PEER_CLOSED);

  std::vector element_runner_client_ends = driver_test().RunInEnvironmentTypeContext(
      fit::callback<std::vector<fidl::ClientEnd<fuchsia_power_broker::ElementRunner>>(
          Environment&)>(
          [](Environment& env) { return env.fake_power_broker().TakeElementRunnerClientEnds(); }));
  element_runner_client_ends.erase(element_runner_client_ends.begin());
  ASSERT_EQ(element_runner_client_ends.size(), 3ul);

  fidl::Client<fuchsia_power_broker::ElementRunner> function1(
      std::move(element_runner_client_ends[0]), fdf::Dispatcher::GetCurrent()->async_dispatcher());
  fidl::Client<fuchsia_power_broker::ElementRunner> function2(
      std::move(element_runner_client_ends[1]), fdf::Dispatcher::GetCurrent()->async_dispatcher());
  fidl::Client<fuchsia_power_broker::ElementRunner> function3(
      std::move(element_runner_client_ends[2]), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  // Move the power elements to the ON state.
  function1->SetLevel(SdioFunctionDevice::kOn)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
        EXPECT_TRUE(result.is_ok());
      });
  function2->SetLevel(SdioFunctionDevice::kOn)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
        EXPECT_TRUE(result.is_ok());
      });
  function3->SetLevel(SdioFunctionDevice::kOn)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
        EXPECT_TRUE(result.is_ok());
      });

  driver_test().runtime().RunUntilIdle();

  // The driver should have dropped the leases on the boot power level.
  EXPECT_EQ(lease_control_server_ends[0].channel().wait_one(ZX_CHANNEL_PEER_CLOSED,
                                                            zx::time::infinite_past(), &observed),
            ZX_OK);
  EXPECT_TRUE(observed & ZX_CHANNEL_PEER_CLOSED);

  EXPECT_EQ(lease_control_server_ends[1].channel().wait_one(ZX_CHANNEL_PEER_CLOSED,
                                                            zx::time::infinite_past(), &observed),
            ZX_OK);
  EXPECT_TRUE(observed & ZX_CHANNEL_PEER_CLOSED);

  EXPECT_EQ(lease_control_server_ends[2].channel().wait_one(ZX_CHANNEL_PEER_CLOSED,
                                                            zx::time::infinite_past(), &observed),
            ZX_OK);
  EXPECT_TRUE(observed & ZX_CHANNEL_PEER_CLOSED);
}

TEST_F(SdioControllerDeviceTest, GetToken) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(3) | SDIO_SEND_OP_COND_RESP_S18A;
  });
  sdmmc_.set_host_info({
      .caps = SDMMC_HOST_CAP_VOLTAGE_330,
      .max_transfer_size = 0x1000,
  });

  ASSERT_OK(StartDriver(true));

  zx::result<fidl::ClientEnd<fuchsia_hardware_power::PowerTokenProvider>> client_end1 =
      driver_test().Connect<fuchsia_hardware_power::PowerTokenService::TokenProvider>(
          "sdmmc-sdio-1");
  ASSERT_TRUE(client_end1.is_ok());
  fidl::Client<fuchsia_hardware_power::PowerTokenProvider> token_client1(
      *std::move(client_end1), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  zx::result<fidl::ClientEnd<fuchsia_hardware_power::PowerTokenProvider>> client_end2 =
      driver_test().Connect<fuchsia_hardware_power::PowerTokenService::TokenProvider>(
          "sdmmc-sdio-2");
  ASSERT_TRUE(client_end2.is_ok());
  fidl::Client<fuchsia_hardware_power::PowerTokenProvider> token_client2(
      *std::move(client_end2), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  zx::result<fidl::ClientEnd<fuchsia_hardware_power::PowerTokenProvider>> client_end3 =
      driver_test().Connect<fuchsia_hardware_power::PowerTokenService::TokenProvider>(
          "sdmmc-sdio-3");
  ASSERT_TRUE(client_end3.is_ok());
  fidl::Client<fuchsia_hardware_power::PowerTokenProvider> token_client3(
      *std::move(client_end3), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  zx::event token1, token2, token3;
  token_client1->GetToken().ThenExactlyOnce(
      [&](fidl::Result<fuchsia_hardware_power::PowerTokenProvider::GetToken>& result) {
        ASSERT_TRUE(result.is_ok());
        EXPECT_TRUE(result->handle().is_valid());
        token1 = std::move(result->handle());
      });
  token_client2->GetToken().ThenExactlyOnce(
      [&](fidl::Result<fuchsia_hardware_power::PowerTokenProvider::GetToken>& result) {
        ASSERT_TRUE(result.is_ok());
        EXPECT_TRUE(result->handle().is_valid());
        token2 = std::move(result->handle());
      });
  token_client3->GetToken().ThenExactlyOnce(
      [&](fidl::Result<fuchsia_hardware_power::PowerTokenProvider::GetToken>& result) {
        ASSERT_TRUE(result.is_ok());
        EXPECT_TRUE(result->handle().is_valid());
        token3 = std::move(result->handle());
      });
  driver_test().runtime().RunUntilIdle();

  std::vector dependency_tokens =
      driver_test().RunInEnvironmentTypeContext(fit::callback<std::vector<zx::event>(Environment&)>(
          [](Environment& env) { return env.fake_power_broker().TakeDependencyTokens(); }));
  ASSERT_EQ(dependency_tokens.size(), 3ul);

  zx_info_handle_basic_t dependency_info{}, token_info{};

  ASSERT_OK(dependency_tokens[0].get_info(ZX_INFO_HANDLE_BASIC, &dependency_info,
                                          sizeof(dependency_info), nullptr, nullptr));
  ASSERT_OK(
      token1.get_info(ZX_INFO_HANDLE_BASIC, &token_info, sizeof(token_info), nullptr, nullptr));
  EXPECT_EQ(dependency_info.koid, token_info.koid);

  ASSERT_OK(dependency_tokens[1].get_info(ZX_INFO_HANDLE_BASIC, &dependency_info,
                                          sizeof(dependency_info), nullptr, nullptr));
  ASSERT_OK(
      token2.get_info(ZX_INFO_HANDLE_BASIC, &token_info, sizeof(token_info), nullptr, nullptr));
  EXPECT_EQ(dependency_info.koid, token_info.koid);

  ASSERT_OK(dependency_tokens[2].get_info(ZX_INFO_HANDLE_BASIC, &dependency_info,
                                          sizeof(dependency_info), nullptr, nullptr));
  ASSERT_OK(
      token3.get_info(ZX_INFO_HANDLE_BASIC, &token_info, sizeof(token_info), nullptr, nullptr));
  EXPECT_EQ(dependency_info.koid, token_info.koid);
}

TEST_F(SdioControllerDeviceTest, PowerOnProbesDevice) {
  uint32_t probe_count = 0;
  sdmmc_.set_command_callback(
      SDIO_SEND_OP_COND, [&probe_count](const sdmmc_req_t& req, uint32_t out_response[4]) -> void {
        out_response[0] = OpCondFunctions(3) | SDIO_SEND_OP_COND_RESP_S18A;
        if (req.arg != 0) {
          probe_count++;
        }
      });
  sdmmc_.set_host_info({
      .caps = SDMMC_HOST_CAP_VOLTAGE_330,
      .max_transfer_size = 0x1000,
  });

  ASSERT_OK(StartDriver(true));
  EXPECT_EQ(probe_count, 1u);

  std::vector element_runner_client_ends = driver_test().RunInEnvironmentTypeContext(
      fit::callback<std::vector<fidl::ClientEnd<fuchsia_power_broker::ElementRunner>>(
          Environment&)>(
          [](Environment& env) { return env.fake_power_broker().TakeElementRunnerClientEnds(); }));
  element_runner_client_ends.erase(element_runner_client_ends.begin());
  ASSERT_EQ(element_runner_client_ends.size(), 3ul);

  std::vector<fidl::Client<fuchsia_power_broker::ElementRunner>> function_runners;
  function_runners.reserve(3);
  for (auto& client_end : element_runner_client_ends) {
    function_runners.emplace_back(std::move(client_end),
                                  fdf::Dispatcher::GetCurrent()->async_dispatcher());
  }

  // Do the initial SetLevel calls to move from the functions to OFF. This simulates the behavior of
  // Power Framework.
  for (auto& runner : function_runners) {
    runner->SetLevel(SdioFunctionDevice::kOff)
        .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
          ASSERT_TRUE(result.is_ok());
        });
  }
  driver_test().runtime().RunUntilIdle();
  EXPECT_EQ(probe_count, 1u);

  // Move all functions from OFF to BOOT to simulate taking the boot leases. This should have no
  // effect as the functions were not actually off previously.
  for (auto& runner : function_runners) {
    runner->SetLevel(SdioFunctionDevice::kBoot)
        .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
          ASSERT_TRUE(result.is_ok());
        });
  }
  driver_test().runtime().RunUntilIdle();
  EXPECT_EQ(probe_count, 1u);

  // Now move all functions to ON to simulate a client connecting.
  for (auto& runner : function_runners) {
    runner->SetLevel(SdioFunctionDevice::kOn)
        .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
          ASSERT_TRUE(result.is_ok());
        });
  }
  driver_test().runtime().RunUntilIdle();
  EXPECT_EQ(probe_count, 1u);

  // Move all functions to OFF, then move one to ON and verify that the device is probed again.
  for (auto& runner : function_runners) {
    runner->SetLevel(SdioFunctionDevice::kOff)
        .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
          ASSERT_TRUE(result.is_ok());
        });
  }
  driver_test().runtime().RunUntilIdle();
  EXPECT_EQ(probe_count, 1u);

  function_runners[1]
      ->SetLevel(SdioFunctionDevice::kOn)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
        ASSERT_TRUE(result.is_ok());
      });
  driver_test().runtime().RunUntilIdle();
  EXPECT_EQ(probe_count, 2u);

  // Move another function to ON, which should not result in the device being probed.
  function_runners[0]
      ->SetLevel(SdioFunctionDevice::kOn)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
        ASSERT_TRUE(result.is_ok());
      });
  driver_test().runtime().RunUntilIdle();
  EXPECT_EQ(probe_count, 2u);
}

TEST_F(SdioControllerDeviceTest, DoRwByteFailsWhenFunctionPoweredOff) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(3);
  });
  sdmmc_.set_host_info({
      .caps = 0,
      .max_transfer_size = 16,
  });

  ASSERT_OK(StartDriver(true));

  std::vector<fidl::ClientEnd<fuchsia_power_broker::ElementRunner>> element_runner_client_ends =
      driver_test().RunInEnvironmentTypeContext(
          fit::callback<std::vector<fidl::ClientEnd<fuchsia_power_broker::ElementRunner>>(
              Environment&)>([](Environment& env) {
            return env.fake_power_broker().TakeElementRunnerClientEnds();
          }));
  element_runner_client_ends.erase(element_runner_client_ends.begin());
  ASSERT_EQ(element_runner_client_ends.size(), 3ul);

  fidl::Client<fuchsia_power_broker::ElementRunner> function1_runner(
      std::move(element_runner_client_ends[0]), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  // Power off function 1, but don't touch the other functions.
  function1_runner->SetLevel(SdioFunctionDevice::kOn)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel>& result) {
        ASSERT_TRUE(result.is_ok());
      });
  function1_runner->SetLevel(SdioFunctionDevice::kOff)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel>& result) {
        ASSERT_TRUE(result.is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  fidl::WireClient client1 = ConnectDeviceClient(1);
  ASSERT_TRUE(client1.is_valid());

  fidl::WireClient client2 = ConnectDeviceClient(2);
  ASSERT_TRUE(client2.is_valid());

  sdmmc_.Write(0x1234, std::vector<uint8_t>{0xaa}, 1);
  sdmmc_.Write(0x1234, std::vector<uint8_t>{0x55}, 2);

  // This read should fail with ZX_ERR_BAD_STATE as the function is powered off.
  client1->ReadByte(0x1234, false).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_error());
    EXPECT_EQ(result->error_value(), ZX_ERR_BAD_STATE);
  });
  // This one should succeed as function 2 is still powered on.
  client2->ReadByte(0x1234, false).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->byte, 0x55);
  });
  driver_test().runtime().RunUntilIdle();

  // Power on function 1 and verify that the read now succeeds.
  function1_runner->SetLevel(SdioFunctionDevice::kOn)
      .ThenExactlyOnce([&](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel>& result) {
        ASSERT_TRUE(result.is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  client1->ReadByte(0x1234, false).ThenExactlyOnce([&](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->byte, 0xaa);
  });
  driver_test().runtime().RunUntilIdle();
}

TEST_F(SdioControllerDeviceTest, Function0AccessesSucceedWhenFunctionPoweredOff) {
  sdmmc_.set_command_callback(SDIO_SEND_OP_COND, [](uint32_t out_response[4]) -> void {
    out_response[0] = OpCondFunctions(3);
  });
  sdmmc_.set_host_info({
      .caps = 0,
      .max_transfer_size = 16,
  });

  ASSERT_OK(StartDriver(true));

  std::vector<fidl::ClientEnd<fuchsia_power_broker::ElementRunner>> element_runner_client_ends =
      driver_test().RunInEnvironmentTypeContext(
          fit::callback<std::vector<fidl::ClientEnd<fuchsia_power_broker::ElementRunner>>(
              Environment&)>([](Environment& env) {
            return env.fake_power_broker().TakeElementRunnerClientEnds();
          }));
  element_runner_client_ends.erase(element_runner_client_ends.begin());
  ASSERT_EQ(element_runner_client_ends.size(), 3ul);

  std::vector<fidl::Client<fuchsia_power_broker::ElementRunner>> function_runners;
  function_runners.reserve(3);
  for (auto& client_end : element_runner_client_ends) {
    function_runners.emplace_back(std::move(client_end),
                                  fdf::Dispatcher::GetCurrent()->async_dispatcher());
  }

  // Power off function 1, but don't touch the other functions.
  function_runners[0]
      ->SetLevel(SdioFunctionDevice::kOn)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel>& result) {
        ASSERT_TRUE(result.is_ok());
      });
  function_runners[0]
      ->SetLevel(SdioFunctionDevice::kOff)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel>& result) {
        ASSERT_TRUE(result.is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  fidl::WireClient client1 = ConnectDeviceClient(1);
  ASSERT_TRUE(client1.is_valid());

  sdmmc_.Write(0xf0, std::vector<uint8_t>{0xaa}, 0);

  // This read only accesses function 0, so it should succeed even though function 1 is powered off.
  client1->DoVendorControlRwByte(false, 0xf0, 0).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->read_byte, 0xaa);
  });
  driver_test().runtime().RunUntilIdle();

  // Power off the other functions.
  function_runners[1]
      ->SetLevel(SdioFunctionDevice::kOn)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel>& result) {
        ASSERT_TRUE(result.is_ok());
      });
  function_runners[1]
      ->SetLevel(SdioFunctionDevice::kOff)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel>& result) {
        ASSERT_TRUE(result.is_ok());
      });
  function_runners[2]
      ->SetLevel(SdioFunctionDevice::kOn)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel>& result) {
        ASSERT_TRUE(result.is_ok());
      });
  function_runners[2]
      ->SetLevel(SdioFunctionDevice::kOff)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel>& result) {
        ASSERT_TRUE(result.is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  // The read should now fail.
  client1->DoVendorControlRwByte(false, 0xf0, 0).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_error());
    EXPECT_EQ(result->error_value(), ZX_ERR_BAD_STATE);
  });
  driver_test().runtime().RunUntilIdle();

  // Power on another function and it should succeed again.
  function_runners[2]
      ->SetLevel(SdioFunctionDevice::kOn)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel>& result) {
        ASSERT_TRUE(result.is_ok());
      });
  driver_test().runtime().RunUntilIdle();

  client1->DoVendorControlRwByte(false, 0xf0, 0).ThenExactlyOnce([](auto& result) {
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->read_byte, 0xaa);
  });
  driver_test().runtime().RunUntilIdle();
}

TEST_F(SdioControllerDeviceTest, NoProbeAfterControllerOffToOn) {
  uint32_t probe_count = 0;
  sdmmc_.set_command_callback(
      SDIO_SEND_OP_COND, [&probe_count](const sdmmc_req_t& req, uint32_t out_response[4]) -> void {
        out_response[0] = OpCondFunctions(2) | SDIO_SEND_OP_COND_RESP_S18A;
        if (req.arg != 0) {
          probe_count++;
        }
      });
  sdmmc_.set_host_info({
      .caps = SDMMC_HOST_CAP_VOLTAGE_330,
      .max_transfer_size = 0x1000,
  });

  ASSERT_OK(StartDriver(true));
  EXPECT_EQ(probe_count, 1u);

  std::vector element_runner_client_ends = driver_test().RunInEnvironmentTypeContext(
      fit::callback<std::vector<fidl::ClientEnd<fuchsia_power_broker::ElementRunner>>(
          Environment&)>(
          [](Environment& env) { return env.fake_power_broker().TakeElementRunnerClientEnds(); }));
  element_runner_client_ends.erase(element_runner_client_ends.begin());
  ASSERT_EQ(element_runner_client_ends.size(), 2ul);

  fidl::Client<fuchsia_power_broker::ElementRunner> runner_1(
      std::move(element_runner_client_ends[0]), fdf::Dispatcher::GetCurrent()->async_dispatcher());
  fidl::Client<fuchsia_power_broker::ElementRunner> runner_2(
      std::move(element_runner_client_ends[1]), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  runner_1->SetLevel(SdioFunctionDevice::kOn)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
        ASSERT_TRUE(result.is_ok());
      });
  runner_2->SetLevel(SdioFunctionDevice::kOn)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
        ASSERT_TRUE(result.is_ok());
      });
  driver_test().runtime().RunUntilIdle();
  EXPECT_EQ(probe_count, 1u);

  runner_1->SetLevel(SdioFunctionDevice::kControllerOff)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
        ASSERT_TRUE(result.is_ok());
      });
  runner_2->SetLevel(SdioFunctionDevice::kControllerOff)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
        ASSERT_TRUE(result.is_ok());
      });
  driver_test().runtime().RunUntilIdle();
  EXPECT_EQ(probe_count, 1u);

  // CONTROLLER_OFF -> ON should not involve re-initializing the device.
  runner_1->SetLevel(SdioFunctionDevice::kOn)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
        ASSERT_TRUE(result.is_ok());
      });
  runner_2->SetLevel(SdioFunctionDevice::kOn)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
        ASSERT_TRUE(result.is_ok());
      });
  driver_test().runtime().RunUntilIdle();
  EXPECT_EQ(probe_count, 1u);
}

TEST_F(SdioControllerDeviceTest, ProbeAfterControllerOffToOn) {
  uint32_t probe_count = 0;
  sdmmc_.set_command_callback(
      SDIO_SEND_OP_COND, [&probe_count](const sdmmc_req_t& req, uint32_t out_response[4]) -> void {
        out_response[0] = OpCondFunctions(2) | SDIO_SEND_OP_COND_RESP_S18A;
        if (req.arg != 0) {
          probe_count++;
        }
      });
  sdmmc_.set_host_info({
      .caps = SDMMC_HOST_CAP_VOLTAGE_330,
      .max_transfer_size = 0x1000,
  });

  ASSERT_OK(StartDriver(true));
  EXPECT_EQ(probe_count, 1u);

  std::vector element_runner_client_ends = driver_test().RunInEnvironmentTypeContext(
      fit::callback<std::vector<fidl::ClientEnd<fuchsia_power_broker::ElementRunner>>(
          Environment&)>(
          [](Environment& env) { return env.fake_power_broker().TakeElementRunnerClientEnds(); }));
  element_runner_client_ends.erase(element_runner_client_ends.begin());
  ASSERT_EQ(element_runner_client_ends.size(), 2ul);

  fidl::Client<fuchsia_power_broker::ElementRunner> runner_1(
      std::move(element_runner_client_ends[0]), fdf::Dispatcher::GetCurrent()->async_dispatcher());
  fidl::Client<fuchsia_power_broker::ElementRunner> runner_2(
      std::move(element_runner_client_ends[1]), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  runner_1->SetLevel(SdioFunctionDevice::kOn)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
        ASSERT_TRUE(result.is_ok());
      });
  runner_2->SetLevel(SdioFunctionDevice::kOn)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
        ASSERT_TRUE(result.is_ok());
      });
  driver_test().runtime().RunUntilIdle();
  EXPECT_EQ(probe_count, 1u);

  runner_1->SetLevel(SdioFunctionDevice::kOff)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
        ASSERT_TRUE(result.is_ok());
      });
  runner_2->SetLevel(SdioFunctionDevice::kOff)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
        ASSERT_TRUE(result.is_ok());
      });
  driver_test().runtime().RunUntilIdle();
  EXPECT_EQ(probe_count, 1u);

  runner_1->SetLevel(SdioFunctionDevice::kControllerOff)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
        ASSERT_TRUE(result.is_ok());
      });
  runner_2->SetLevel(SdioFunctionDevice::kControllerOff)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
        ASSERT_TRUE(result.is_ok());
      });
  driver_test().runtime().RunUntilIdle();
  EXPECT_EQ(probe_count, 1u);

  // CONTROLLER_OFF -> ON requires re-initializing the device since we were previously at the OFF
  // level.
  runner_1->SetLevel(SdioFunctionDevice::kOn)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
        ASSERT_TRUE(result.is_ok());
      });
  driver_test().runtime().RunUntilIdle();
  EXPECT_EQ(probe_count, 2u);

  runner_2->SetLevel(SdioFunctionDevice::kOn)
      .ThenExactlyOnce([](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
        ASSERT_TRUE(result.is_ok());
      });
  driver_test().runtime().RunUntilIdle();
  EXPECT_EQ(probe_count, 2u);
}

}  // namespace sdmmc

FUCHSIA_DRIVER_EXPORT2(sdmmc::TestSdmmcRootDevice);
