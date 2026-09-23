// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdmmc-block-device.h"

#include <endian.h>
#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.block.volume/cpp/fidl.h>
#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <fidl/fuchsia.hardware.sdmmc/cpp/fidl.h>
#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <fidl/fuchsia.power.system/cpp/fidl.h>
#include <fidl/fuchsia.power.system/cpp/test_base.h>
#include <fidl/fuchsia.storage.block/cpp/fidl.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/driver/power/cpp/testing/fake_element_control.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/watcher.h>
#include <lib/fidl/cpp/wire/client.h>
#include <lib/fidl/cpp/wire/connect_service.h>
#include <lib/fidl/cpp/wire/server.h>
#include <lib/fit/defer.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/inspect/testing/cpp/zxtest/inspect.h>
#include <lib/sdmmc/hw.h>
#include <zircon/errors.h>

#include <memory>
#include <optional>

#include <fbl/algorithm.h>
#include <fbl/unique_fd.h>
#include <zxtest/zxtest.h>

#include "fake-sdmmc-device.h"
#include "sdmmc-partition-device.h"
#include "sdmmc-root-device.h"
#include "sdmmc-rpmb-device.h"
#include "sdmmc-types.h"
#include "src/storage/lib/block_client/cpp/reader_writer.h"
#include "src/storage/lib/block_client/cpp/remote_block_device.h"

namespace sdmmc {

using fdf_power::testing::FakeElementControl;

class TestSdmmcRootDevice : public SdmmcRootDevice {
 public:
  // Modify these static variables to configure the behaviour of this test device.
  static bool use_fidl_;
  static bool is_sd_;
  static FakeSdmmcDevice sdmmc_;
  static std::optional<fdf::ClientEnd<fuchsia_hardware_sdmmc::Sdmmc>> fidl_client_end_;

  explicit TestSdmmcRootDevice() : SdmmcRootDevice() {}

 protected:
  zx_status_t Init(const fuchsia_hardware_sdmmc::SdmmcMetadata& metadata) override {
    std::unique_ptr<SdmmcDevice> sdmmc;
    if (use_fidl_) {
      ZX_ASSERT(fidl_client_end_.has_value());
      sdmmc = std::make_unique<SdmmcDevice>(this, std::move(*fidl_client_end_));
      fidl_client_end_.reset();
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

    std::unique_ptr<SdmmcBlockDevice> block_device;
    if (status = SdmmcBlockDevice::Create(this, std::move(sdmmc), &block_device); status != ZX_OK) {
      return status;
    }

    block_device->SetMetadata(metadata);
    if (status = is_sd_ ? block_device->ProbeSd() : block_device->ProbeMmc(); status != ZX_OK) {
      return status;
    }
    if (status = block_device->AddDevice(); status != ZX_OK) {
      return status;
    }
    child_device_ = std::move(block_device);
    return ZX_OK;
  }
};

bool TestSdmmcRootDevice::use_fidl_;
bool TestSdmmcRootDevice::is_sd_;
FakeSdmmcDevice TestSdmmcRootDevice::sdmmc_;
std::optional<fdf::ClientEnd<fuchsia_hardware_sdmmc::Sdmmc>> TestSdmmcRootDevice::fidl_client_end_;

class FakeCpuElementManager
    : public fidl::testing::TestBase<fuchsia_power_system::CpuElementManager> {
 public:
  fidl::ProtocolHandler<fuchsia_power_system::CpuElementManager> CreateHandler() {
    return bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                   fidl::kIgnoreBindingClosure);
  }

  bool execution_state_dependency_added() const { return dependency_token_.is_valid(); }

  void AddExecutionStateDependency(AddExecutionStateDependencyRequest& request,
                                   AddExecutionStateDependencyCompleter::Sync& completer) override {
    if (!request.power_level() || *request.power_level() != SdmmcBlockDevice::kPowerLevelOn ||
        !request.dependency_token()) {
      completer.Reply(
          fit::error(fuchsia_power_system::AddExecutionStateDependencyError::kInvalidArgs));
      return;
    }
    if (dependency_token_.is_valid()) {
      completer.Reply(
          fit::error(fuchsia_power_system::AddExecutionStateDependencyError::kInvalidArgs));
      return;
    }

    dependency_token_ = *std::move(request.dependency_token());
    completer.Reply(fit::success());
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    ADD_FAILURE("%s is not implemented", name.c_str());
  }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_power_system::CpuElementManager> md,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

 private:
  fidl::ServerBindingGroup<fuchsia_power_system::CpuElementManager> bindings_;
  zx::event dependency_token_;
};
class FakeLessor : public fidl::Server<fuchsia_power_broker::Lessor> {
 public:
  fidl::ServerEnd<fuchsia_power_broker::LeaseControl> TakeLeaseControlServerEnd() {
    return std::move(lease_control_server_end_);
  }

  void Lease(LeaseRequest& req, LeaseCompleter::Sync& completer) override {
    auto [lease_control_client_end, lease_control_server_end] =
        fidl::Endpoints<fuchsia_power_broker::LeaseControl>::Create();
    lease_control_server_end_ = std::move(lease_control_server_end);
    completer.Reply(fit::success(std::move(lease_control_client_end)));
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_power_broker::Lessor> md,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}

 private:
  fidl::ServerEnd<fuchsia_power_broker::LeaseControl> lease_control_server_end_;
};

class PowerElement {
 public:
  explicit PowerElement(
      fidl::ServerBindingRef<fuchsia_power_broker::ElementControl> element_control,
      fidl::ServerBindingRef<fuchsia_power_broker::Lessor> lessor)
      : element_control_(std::move(element_control)), lessor_(std::move(lessor)) {}

  fidl::ServerBindingRef<fuchsia_power_broker::ElementControl> element_control_;
  fidl::ServerBindingRef<fuchsia_power_broker::Lessor> lessor_;
};

class FakePowerBroker {
 public:
  void AddHardwarePowerElement(
      fidl::ServerEnd<fuchsia_power_broker::ElementControl> element_control_server_end,
      fidl::ClientEnd<fuchsia_power_broker::ElementRunner> element_runner_client_end,
      fidl::ServerEnd<fuchsia_power_broker::Lessor> lessor_server_end) {
    // Instantiate (fake) element control implementation.
    auto element_control_impl = std::make_unique<FakeElementControl>();
    hardware_power_element_control_ = element_control_impl.get();
    fidl::ServerBindingRef<fuchsia_power_broker::ElementControl> element_control_binding =
        fidl::BindServer<fuchsia_power_broker::ElementControl>(
            fdf::Dispatcher::GetCurrent()->async_dispatcher(),
            std::move(element_control_server_end), std::move(element_control_impl),
            [](FakeElementControl* impl, fidl::UnbindInfo info,
               fidl::ServerEnd<fuchsia_power_broker::ElementControl> server_end) mutable {});

    // Instantiate (fake) lessor implementation.
    auto lessor_impl = std::make_unique<FakeLessor>();
    hardware_power_lessor_ = lessor_impl.get();
    fidl::ServerBindingRef<fuchsia_power_broker::Lessor> lessor_binding =
        fidl::BindServer<fuchsia_power_broker::Lessor>(
            fdf::Dispatcher::GetCurrent()->async_dispatcher(), std::move(lessor_server_end),
            std::move(lessor_impl),
            [](FakeLessor* impl, fidl::UnbindInfo info,
               fidl::ServerEnd<fuchsia_power_broker::Lessor> server_end) mutable {});

    hardware_power_element_runner_client_ = fidl::Client<fuchsia_power_broker::ElementRunner>(
        std::move(element_runner_client_end), fdf::Dispatcher::GetCurrent()->async_dispatcher());

    servers_.emplace_back(std::move(element_control_binding), std::move(lessor_binding));
  }

  FakeElementControl* hardware_power_element_control_ = nullptr;
  FakeLessor* hardware_power_lessor_ = nullptr;
  fidl::Client<fuchsia_power_broker::ElementRunner> hardware_power_element_runner_client_;
  std::vector<PowerElement> servers_;
};

class TestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    if (metadata_.has_value()) {
      zx::result result = metadata_server.Serve(
          to_driver_vfs, fdf::Dispatcher::GetCurrent()->async_dispatcher(), metadata_.value());
      if (result.is_error()) {
        return result.take_error();
      }
    }

    // Serve (fake) cpu_element_manager.
    zx::result result =
        to_driver_vfs.component().AddUnmanagedProtocol<fuchsia_power_system::CpuElementManager>(
            cpu_element_manager.CreateHandler());
    if (result.is_error()) {
      return result.take_error();
    }

    // Add our package
    auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    zx_status_t status = fdio_open3("/pkg/", static_cast<uint64_t>(fuchsia_io::wire::kPermReadable),
                                    server.TakeChannel().release());
    if (status != ZX_OK) {
      return zx::error(status);
    }
    result = to_driver_vfs.AddDirectory(std::move(client), "pkg");
    if (result.is_error()) {
      return result.take_error();
    }

    return zx::ok();
  }

  void SetMetadata(bool removable, fuchsia_hardware_sdmmc::SdmmcHostPrefs speed_capabilities,
                   bool use_fidl) {
    metadata_ = fuchsia_hardware_sdmmc::SdmmcMetadata{{
        .speed_capabilities = speed_capabilities,
        .enable_cache = true,
        .removable = removable,
        .max_command_packing = 16,
        .use_fidl = use_fidl,
    }};
  }

  fdf_metadata::MetadataServer<fuchsia_hardware_sdmmc::SdmmcMetadata> metadata_server;
  FakePowerBroker power_broker;
  FakeCpuElementManager cpu_element_manager;

 private:
  std::optional<fuchsia_hardware_sdmmc::SdmmcMetadata> metadata_;
};

class TestConfig final {
 public:
  using DriverType = TestSdmmcRootDevice;
  using EnvironmentType = TestEnvironment;
};

class SdmmcBlockDeviceTest : public zxtest::TestWithParam<bool> {
 public:
  SdmmcBlockDeviceTest() {}

  static void SetDefaultMmcExtCsd(cpp20::span<uint8_t> out_data) {
    *reinterpret_cast<uint32_t*>(&out_data[212]) = htole32(FakeSdmmcDevice::kBlockCount);
    out_data[MMC_EXT_CSD_CACHE_CTRL] = 1;
    out_data[MMC_EXT_CSD_CACHE_SIZE_LSB] = 0x78;
    out_data[MMC_EXT_CSD_CACHE_SIZE_250] = 0x56;
    out_data[MMC_EXT_CSD_CACHE_SIZE_251] = 0x34;
    out_data[MMC_EXT_CSD_CACHE_SIZE_MSB] = 0x12;
    out_data[MMC_EXT_CSD_PARTITION_CONFIG] = 0xa8;
    out_data[MMC_EXT_CSD_SEC_FEATURE_SUPPORT] = 0x1 << MMC_EXT_CSD_SEC_FEATURE_SUPPORT_SEC_GB_CL_EN;
    out_data[MMC_EXT_CSD_PARTITION_SWITCH_TIME] = 0;
    out_data[MMC_EXT_CSD_BOOT_SIZE_MULT] = 0x10;
    out_data[MMC_EXT_CSD_GENERIC_CMD6_TIME] = 0;
    out_data[MMC_EXT_CSD_BARRIER_SUPPORT] = 1;
    out_data[MMC_EXT_CSD_CACHE_FLUSH_POLICY] = 1;
    out_data[MMC_EXT_CSD_BARRIER_CTRL] = 1;
  }

  void SetUp() override {
    sdmmc_.Reset();

    sdmmc_.set_command_callback(
        MMC_SEND_OP_COND, [](uint32_t out_response[4]) -> void { out_response[0] = MMC_OCR_BUSY; });

    sdmmc_.set_command_callback(SDMMC_SEND_STATUS, [](uint32_t out_response[4]) -> void {
      out_response[0] = MMC_STATUS_CURRENT_STATE_TRAN;
    });

    sdmmc_.set_command_callback(SDMMC_SEND_CSD, [](uint32_t out_response[4]) -> void {
      uint8_t* response = reinterpret_cast<uint8_t*>(out_response);
      response[MMC_CSD_SPEC_VERSION] = MMC_CID_SPEC_VRSN_40 << 2;
      response[MMC_CSD_SIZE_START] = 0x03 << 6;
      response[MMC_CSD_SIZE_START + 1] = 0xff;
      response[MMC_CSD_SIZE_START + 2] = 0x03;
    });

    sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) -> void {
      SetDefaultMmcExtCsd(out_data);
    });
  }

  void TearDown() override {
    if (dut_) {
      auto stop_result = driver_test_.StopDriver();
      EXPECT_EQ(ZX_OK, stop_result.status_value());
    }
  }

  zx_status_t StartDriverForMmc(fuchsia_hardware_sdmmc::SdmmcHostPrefs speed_capabilities = {},
                                bool supply_power_framework = false) {
    return StartDriver(/*is_sd=*/false, speed_capabilities, supply_power_framework);
  }
  zx_status_t StartDriverForSd(fuchsia_hardware_sdmmc::SdmmcHostPrefs speed_capabilities = {},
                               bool supply_power_framework = false) {
    return StartDriver(/*is_sd=*/true, speed_capabilities, supply_power_framework);
  }

  zx_status_t StartDriver(bool is_sd, fuchsia_hardware_sdmmc::SdmmcHostPrefs speed_capabilities,
                          bool supply_power_framework) {
    TestSdmmcRootDevice::use_fidl_ = GetParam();
    TestSdmmcRootDevice::is_sd_ = is_sd;
    if (TestSdmmcRootDevice::use_fidl_) {
      zx::result client_end = sdmmc_.GetFidlClientEnd();
      ZX_ASSERT(client_end.is_ok());
      TestSdmmcRootDevice::fidl_client_end_ = std::move(*client_end);
    }
    if (is_sd) {
      sdmmc_.set_command_callback(SD_SEND_IF_COND,
                                  [](const sdmmc_req_t& req, uint32_t out_response[4]) {
                                    out_response[0] = req.arg & 0xfff;
                                  });

      sdmmc_.set_command_callback(SD_APP_SEND_OP_COND, [](uint32_t out_response[4]) {
        out_response[0] = 0xc000'0000;  // Set busy and CCS bits.
      });

      sdmmc_.set_command_callback(SD_SEND_RELATIVE_ADDR, [](uint32_t out_response[4]) {
        out_response[0] = 0x100;  // Set READY_FOR_DATA bit in SD status.
      });

      sdmmc_.set_command_callback(SDMMC_SEND_CSD, [](uint32_t out_response[4]) {
        out_response[1] = 0x1234'0000;
        out_response[2] = 0x0000'5678;
        out_response[3] = 0x4000'0000;  // Set CSD_STRUCTURE to indicate SDHC/SDXC.
      });
    }

    // Initialize driver test environment.
    driver_test_.RunInEnvironmentTypeContext([&](TestEnvironment& env) mutable {
      env.SetMetadata(is_sd, speed_capabilities, TestSdmmcRootDevice::use_fidl_);
    });

    std::optional<fuchsia_driver_framework::PowerElementArgs> power_args;

    if (supply_power_framework) {
      auto [element_control_client, element_control_server] =
          fidl::Endpoints<fuchsia_power_broker::ElementControl>::Create();
      auto [element_runner_client, element_runner_server] =
          fidl::Endpoints<fuchsia_power_broker::ElementRunner>::Create();
      auto [lessor_client, lessor_server] = fidl::Endpoints<fuchsia_power_broker::Lessor>::Create();
      fuchsia_power_broker::DependencyToken element_token;
      EXPECT_EQ(zx::event::create(0, &element_token), ZX_OK);

      fuchsia_driver_framework::PowerElementArgs local_power_args;

      local_power_args.control_client() = std::move(element_control_client);
      local_power_args.runner_server() = std::move(element_runner_server);
      local_power_args.lessor_client() = std::move(lessor_client);
      local_power_args.token() = std::move(element_token);

      power_args = std::move(local_power_args);

      // TODO, store the other side of the objects
      driver_test_.RunInEnvironmentTypeContext<void>(
          [control_server = std::move(element_control_server),
           runner_client = std::move(element_runner_client),
           lessor_server = std::move(lessor_server)](TestEnvironment& env) mutable {
            env.power_broker.AddHardwarePowerElement(
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

    // Start driver
    zx::result result =
        driver_test_.StartDriverWithCustomStartArgs([&](fdf::DriverStartArgs& args) {
          zx::event token;
          ZX_ASSERT(zx::event::create(0, &token) == ZX_OK);
          args.node_token(std::move(token));

          sdmmc_config::Config fake_config;
          fake_config.enable_suspend() = supply_power_framework;
          fake_config.storage_power_management_enabled() = supply_power_framework;
          args.config(fake_config.ToVmo());
          if (supply_power_framework) {
            args.power_element_args(std::move(power_args.value()));
          }
          args.node_token(std::move(token_copy));
        });
    if (result.is_error()) {
      return result.status_value();
    }

    driver_test_.RunInDriverContext([&](TestSdmmcRootDevice& driver) { dut_ = &driver; });

    auto* block_device = std::get_if<std::unique_ptr<SdmmcBlockDevice>>(&dut_->child_device());
    if (block_device == nullptr) {
      return ZX_ERR_BAD_STATE;
    }
    block_device_ = block_device->get();

    block_device_->SetBlockInfo(FakeSdmmcDevice::kBlockSize, FakeSdmmcDevice::kBlockCount);

    for (size_t i = 0; i < (FakeSdmmcDevice::kBlockSize / sizeof(kTestData)); i++) {
      test_block_.insert(test_block_.end(), kTestData, kTestData + sizeof(kTestData));
    }

    return ZX_OK;
  }

  void QueueBlockOps();
  void QueueRpmbRequests();
  fidl::WireSharedClient<fuchsia_hardware_rpmb::Rpmb>& rpmb_client() { return rpmb_client_; }
  std::atomic<bool>& run_threads() { return run_threads_; }

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> GetRemoteBlockDeviceForBlockServer(
      const char* instance_name) {
    zx::result client =
        driver_test_.Connect<fuchsia_hardware_block_volume::Service::Volume>(instance_name);
    if (client.is_error()) {
      return client.take_error();
    }

    return block_client::RemoteBlockDevice::Create(std::move(client.value()));
  }

 protected:
  static constexpr uint32_t kMaxOutstandingOps = 16;

  void BindRpmbClient() {
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_hardware_rpmb::Rpmb>::Create();

    auto dispatcher = driver_test_.runtime().StartBackgroundDispatcher()->async_dispatcher();
    binding_ = fidl::BindServer(dispatcher, std::move(server_end),
                                block_device_->child_rpmb_device().get());
    rpmb_client_.Bind(std::move(client_end), dispatcher);
  }

  void FillSdmmc(uint32_t length, uint64_t offset) {
    for (uint32_t i = 0; i < length; i++) {
      sdmmc_.Write((offset + i) * test_block_.size(), test_block_);
    }
  }

  void FillVmo(const fzl::VmoMapper& mapper, uint32_t length, uint64_t offset = 0) {
    auto* ptr = reinterpret_cast<uint8_t*>(mapper.start()) + (offset * test_block_.size());
    for (uint32_t i = 0; i < length; i++, ptr += test_block_.size()) {
      memcpy(ptr, test_block_.data(), test_block_.size());
    }
  }

  void CheckSdmmc(uint32_t length, uint64_t offset) {
    const std::vector<uint8_t> data =
        sdmmc_.Read(offset * test_block_.size(), length * test_block_.size());
    const uint8_t* ptr = data.data();
    for (uint32_t i = 0; i < length; i++, ptr += test_block_.size()) {
      EXPECT_BYTES_EQ(ptr, test_block_.data(), test_block_.size());
    }
  }

  void CheckVmo(const fzl::VmoMapper& mapper, uint32_t length, uint64_t offset = 0) {
    const uint8_t* ptr = reinterpret_cast<uint8_t*>(mapper.start()) + (offset * test_block_.size());
    for (uint32_t i = 0; i < length; i++, ptr += test_block_.size()) {
      EXPECT_BYTES_EQ(ptr, test_block_.data(), test_block_.size());
    }
  }

  void CheckVmoErased(const fzl::VmoMapper& mapper, uint32_t length, uint64_t offset = 0) {
    const size_t blocks_to_u32 = test_block_.size() / sizeof(uint32_t);
    const uint32_t* data = reinterpret_cast<uint32_t*>(mapper.start()) + (offset * blocks_to_u32);
    for (uint32_t i = 0; i < (length * blocks_to_u32); i++) {
      EXPECT_EQ(data[i], 0xffff'ffff);
    }
  }

  FakeSdmmcDevice& sdmmc_ = TestSdmmcRootDevice::sdmmc_;
  fdf_testing::BackgroundDriverTest<TestConfig> driver_test_;
  TestSdmmcRootDevice* dut_ = nullptr;
  SdmmcBlockDevice* block_device_ = nullptr;
  fidl::WireSharedClient<fuchsia_hardware_rpmb::Rpmb> rpmb_client_;
  std::atomic<bool> run_threads_ = true;
  zx::event node_token_;

 private:
  static constexpr uint8_t kTestData[] = {
      // clang-format off
      0xd0, 0x0d, 0x7a, 0xf2, 0xbc, 0x13, 0x81, 0x07,
      0x72, 0xbe, 0x33, 0x5f, 0x21, 0x4e, 0xd7, 0xba,
      0x1b, 0x0c, 0x25, 0xcf, 0x2c, 0x6f, 0x46, 0x3a,
      0x78, 0x22, 0xea, 0x9e, 0xa0, 0x41, 0x65, 0xf8,
      // clang-format on
  };
  static_assert(FakeSdmmcDevice::kBlockSize % sizeof(kTestData) == 0);

  fidl::Arena<> arena_;
  std::vector<uint8_t> test_block_;
  std::optional<fidl::ServerBindingRef<fuchsia_hardware_rpmb::Rpmb>> binding_;
};

TEST_P(SdmmcBlockDeviceTest, BlockImplQuery) {
  ASSERT_OK(StartDriverForMmc());

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> client_result =
      GetRemoteBlockDeviceForBlockServer("user");
  ASSERT_OK(client_result);
  auto client = std::move(client_result.value());

  fuchsia_storage_block::wire::BlockInfo info;
  EXPECT_OK(client->BlockGetInfo(&info));

  EXPECT_EQ(info.block_count, FakeSdmmcDevice::kBlockCount);
  EXPECT_EQ(info.block_size, FakeSdmmcDevice::kBlockSize);
  EXPECT_FALSE(info.flags & fuchsia_storage_block::wire::DeviceFlag::kRemovable);
}

TEST_P(SdmmcBlockDeviceTest, BlockImplQuerySdRemovable) {
  ASSERT_OK(StartDriverForSd());

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> client_result =
      GetRemoteBlockDeviceForBlockServer("user");
  ASSERT_OK(client_result);
  auto client = std::move(client_result.value());

  fuchsia_storage_block::wire::BlockInfo info;
  EXPECT_OK(client->BlockGetInfo(&info));

  EXPECT_EQ(info.block_size, FakeSdmmcDevice::kBlockSize);
  EXPECT_TRUE(info.flags & fuchsia_storage_block::wire::DeviceFlag::kRemovable);
}

TEST_P(SdmmcBlockDeviceTest, BlockImplQueue) {
  ASSERT_OK(StartDriverForMmc());

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> client_result =
      GetRemoteBlockDeviceForBlockServer("user");
  ASSERT_OK(client_result);
  auto client = std::move(client_result.value());

  fuchsia_storage_block::wire::BlockInfo info;
  EXPECT_OK(client->BlockGetInfo(&info));

  zx::vmo vmo;
  const size_t vmo_size =
      fbl::round_up<size_t, size_t>(20 * FakeSdmmcDevice::kBlockSize, zx_system_get_page_size());
  fzl::VmoMapper mapper;
  ASSERT_OK(mapper.CreateAndMap(vmo_size, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr, &vmo));

  storage::Vmoid owned_vmoid;
  EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));
  vmoid_t vmoid = owned_vmoid.TakeId();

  // Fill VMO with test data.
  FillVmo(mapper, 1, 0);
  FillVmo(mapper, 5, 1);
  FillSdmmc(1, 0x400);
  FillSdmmc(10, 0x2000);

  BlockFifoRequest requests[] = {
      {
          .command = {.opcode = BLOCK_OPCODE_WRITE},
          .vmoid = vmoid,
          .length = 1,
          .vmo_offset = 0,
          .dev_offset = 0,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_WRITE},
          .vmoid = vmoid,
          .length = 5,
          .vmo_offset = 1,
          .dev_offset = 0x8000,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_FLUSH},
      },
      {
          .command = {.opcode = BLOCK_OPCODE_READ},
          .vmoid = vmoid,
          .length = 1,
          .vmo_offset = 6,
          .dev_offset = 0x400,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_READ},
          .vmoid = vmoid,
          .length = 10,
          .vmo_offset = 7,
          .dev_offset = 0x2000,
      },
  };

  EXPECT_OK(client->FifoTransaction(requests, 5));

  // Verify results.
  ASSERT_NO_FATAL_FAILURE(CheckSdmmc(1, 0));
  ASSERT_NO_FATAL_FAILURE(CheckSdmmc(5, 0x8000));
  ASSERT_NO_FATAL_FAILURE(CheckVmo(mapper, 1, 6));
  ASSERT_NO_FATAL_FAILURE(CheckVmo(mapper, 10, 7));
}

TEST_P(SdmmcBlockDeviceTest, BlockImplQueueOutOfRange) {
  ASSERT_OK(StartDriverForMmc());

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> client_result =
      GetRemoteBlockDeviceForBlockServer("user");
  ASSERT_OK(client_result);
  auto client = std::move(client_result.value());

  fuchsia_storage_block::wire::BlockInfo info;
  EXPECT_OK(client->BlockGetInfo(&info));

  zx::vmo vmo;
  const size_t vmo_size =
      fbl::round_up<size_t, size_t>(16 * FakeSdmmcDevice::kBlockSize, zx_system_get_page_size());
  fzl::VmoMapper mapper;
  ASSERT_OK(mapper.CreateAndMap(vmo_size, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr, &vmo));

  storage::Vmoid owned_vmoid;
  EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));
  vmoid_t vmoid = owned_vmoid.TakeId();

  BlockFifoRequest requests[] = {
      {
          .command = {.opcode = BLOCK_OPCODE_WRITE},
          .vmoid = vmoid,
          .length = 1,
          .vmo_offset = 0,
          .dev_offset = 0x100000,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_READ},
          .vmoid = vmoid,
          .length = 10,
          .vmo_offset = 0,
          .dev_offset = 0x200000,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_WRITE},
          .vmoid = vmoid,
          .length = 8,
          .vmo_offset = 0,
          .dev_offset = 0xffff8,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_READ},
          .vmoid = vmoid,
          .length = 9,
          .vmo_offset = 0,
          .dev_offset = 0xffff8,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_WRITE},
          .vmoid = vmoid,
          .length = 16,
          .vmo_offset = 0,
          .dev_offset = 0xffff8,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_READ},
          .vmoid = vmoid,
          .length = 0,
          .vmo_offset = 0,
          .dev_offset = 0x80000,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_WRITE},
          .vmoid = vmoid,
          .length = 1,
          .vmo_offset = 0,
          .dev_offset = 0xfffff,
      },
  };

  EXPECT_STATUS(client->FifoTransaction(&requests[0], 1), ZX_ERR_OUT_OF_RANGE);
  EXPECT_STATUS(client->FifoTransaction(&requests[1], 1), ZX_ERR_OUT_OF_RANGE);
  EXPECT_OK(client->FifoTransaction(&requests[2], 1));
  EXPECT_STATUS(client->FifoTransaction(&requests[3], 1), ZX_ERR_OUT_OF_RANGE);
  EXPECT_STATUS(client->FifoTransaction(&requests[4], 1), ZX_ERR_OUT_OF_RANGE);
  EXPECT_STATUS(client->FifoTransaction(&requests[5], 1), ZX_ERR_INVALID_ARGS);
  EXPECT_OK(client->FifoTransaction(&requests[6], 1));
}

TEST_P(SdmmcBlockDeviceTest, NoCmd12ForSdBlockTransfer) {
  ASSERT_OK(StartDriverForSd());

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> client_result =
      GetRemoteBlockDeviceForBlockServer("user");
  ASSERT_OK(client_result);
  auto client = std::move(client_result.value());

  fuchsia_storage_block::wire::BlockInfo info;
  EXPECT_OK(client->BlockGetInfo(&info));

  zx::vmo vmo;
  const size_t vmo_size =
      fbl::round_up<size_t, size_t>(20 * FakeSdmmcDevice::kBlockSize, zx_system_get_page_size());
  fzl::VmoMapper mapper;
  ASSERT_OK(mapper.CreateAndMap(vmo_size, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr, &vmo));

  storage::Vmoid owned_vmoid;
  EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));
  vmoid_t vmoid = owned_vmoid.TakeId();

  sdmmc_.set_command_callback(SDMMC_READ_MULTIPLE_BLOCK, [](const sdmmc_req_t& req) -> void {
    EXPECT_FALSE(req.cmd_flags & SDMMC_CMD_AUTO12);
  });
  sdmmc_.set_command_callback(SDMMC_WRITE_MULTIPLE_BLOCK, [](const sdmmc_req_t& req) -> void {
    EXPECT_FALSE(req.cmd_flags & SDMMC_CMD_AUTO12);
  });

  BlockFifoRequest requests[] = {
      {
          .command = {.opcode = BLOCK_OPCODE_WRITE},
          .vmoid = vmoid,
          .length = 1,
          .vmo_offset = 0,
          .dev_offset = 0,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_WRITE},
          .vmoid = vmoid,
          .length = 5,
          .vmo_offset = 1,
          .dev_offset = 0x8000,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_FLUSH},
      },
      {
          .command = {.opcode = BLOCK_OPCODE_READ},
          .vmoid = vmoid,
          .length = 1,
          .vmo_offset = 6,
          .dev_offset = 0x400,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_READ},
          .vmoid = vmoid,
          .length = 10,
          .vmo_offset = 7,
          .dev_offset = 0x2000,
      },
  };

  EXPECT_OK(client->FifoTransaction(requests, 5));

  const std::map<uint32_t, uint32_t> command_counts = sdmmc_.command_counts();
  EXPECT_EQ(command_counts.find(SDMMC_STOP_TRANSMISSION), command_counts.end());
}

TEST_P(SdmmcBlockDeviceTest, NoCmd12ForMmcBlockTransfer) {
  ASSERT_OK(StartDriverForMmc());

  auto client_result = GetRemoteBlockDeviceForBlockServer("user");
  ASSERT_OK(client_result);
  auto client = std::move(client_result.value());

  fuchsia_storage_block::wire::BlockInfo info;
  EXPECT_OK(client->BlockGetInfo(&info));

  zx::vmo vmo;
  const size_t vmo_size =
      fbl::round_up<size_t, size_t>(20 * FakeSdmmcDevice::kBlockSize, zx_system_get_page_size());
  fzl::VmoMapper mapper;
  ASSERT_OK(mapper.CreateAndMap(vmo_size, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr, &vmo));

  storage::Vmoid owned_vmoid;
  EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));
  vmoid_t vmoid = owned_vmoid.TakeId();

  sdmmc_.set_command_callback(SDMMC_READ_MULTIPLE_BLOCK, [](const sdmmc_req_t& req) -> void {
    EXPECT_FALSE(req.cmd_flags & SDMMC_CMD_AUTO12);
  });
  sdmmc_.set_command_callback(SDMMC_WRITE_MULTIPLE_BLOCK, [](const sdmmc_req_t& req) -> void {
    EXPECT_FALSE(req.cmd_flags & SDMMC_CMD_AUTO12);
  });

  BlockFifoRequest requests[] = {
      {
          .command = {.opcode = BLOCK_OPCODE_WRITE},
          .vmoid = vmoid,
          .length = 1,
          .vmo_offset = 0,
          .dev_offset = 0,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_WRITE},
          .vmoid = vmoid,
          .length = 5,
          .vmo_offset = 1,
          .dev_offset = 0x8000,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_FLUSH},
      },
      {
          .command = {.opcode = BLOCK_OPCODE_READ},
          .vmoid = vmoid,
          .length = 1,
          .vmo_offset = 6,
          .dev_offset = 0x400,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_READ},
          .vmoid = vmoid,
          .length = 10,
          .vmo_offset = 7,
          .dev_offset = 0x2000,
      },
  };

  EXPECT_OK(client->FifoTransaction(requests, 5));

  const std::map<uint32_t, uint32_t> command_counts = sdmmc_.command_counts();
  EXPECT_EQ(command_counts.find(SDMMC_STOP_TRANSMISSION), command_counts.end());
}

TEST_P(SdmmcBlockDeviceTest, ErrorsPropagate) {
  ASSERT_OK(StartDriverForMmc());

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> client_result =
      GetRemoteBlockDeviceForBlockServer("user");
  ASSERT_OK(client_result);
  auto client = std::move(client_result.value());

  fuchsia_storage_block::wire::BlockInfo info;
  EXPECT_OK(client->BlockGetInfo(&info));

  zx::vmo vmo;
  const size_t vmo_size =
      fbl::round_up<size_t, size_t>(16 * FakeSdmmcDevice::kBlockSize, zx_system_get_page_size());
  fzl::VmoMapper mapper;
  ASSERT_OK(mapper.CreateAndMap(vmo_size, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr, &vmo));

  storage::Vmoid owned_vmoid;
  EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));
  vmoid_t vmoid = owned_vmoid.TakeId();

  BlockFifoRequest requests[] = {
      {
          .command = {.opcode = BLOCK_OPCODE_WRITE},
          .vmoid = vmoid,
          .length = 1,
          .vmo_offset = 0,
          .dev_offset = FakeSdmmcDevice::kBadRegionStart,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_WRITE},
          .vmoid = vmoid,
          .length = 5,
          .vmo_offset = 0,
          .dev_offset = FakeSdmmcDevice::kBadRegionStart | 0x80,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_FLUSH},
      },
      {
          .command = {.opcode = BLOCK_OPCODE_READ},
          .vmoid = vmoid,
          .length = 1,
          .vmo_offset = 0,
          .dev_offset = FakeSdmmcDevice::kBadRegionStart | 0x40,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_READ},
          .vmoid = vmoid,
          .length = 10,
          .vmo_offset = 0,
          .dev_offset = FakeSdmmcDevice::kBadRegionStart | 0x20,
      },
  };

  EXPECT_STATUS(client->FifoTransaction(&requests[0], 1), ZX_ERR_IO);
  EXPECT_STATUS(client->FifoTransaction(&requests[1], 1), ZX_ERR_IO);
  EXPECT_OK(client->FifoTransaction(&requests[2], 1));
  EXPECT_STATUS(client->FifoTransaction(&requests[3], 1), ZX_ERR_IO);
  EXPECT_STATUS(client->FifoTransaction(&requests[4], 1), ZX_ERR_IO);
}

TEST_P(SdmmcBlockDeviceTest, SendCmd12OnCommandFailure) {
  sdmmc_.set_host_info({
      .caps = 0,
      .max_transfer_size = fuchsia_hardware_sdmmc::wire::kMaxTransferUnbounded,
  });

  ASSERT_OK(StartDriverForMmc());

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> client_result =
      GetRemoteBlockDeviceForBlockServer("user");
  ASSERT_OK(client_result);
  auto client = std::move(client_result.value());

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(FakeSdmmcDevice::kBlockSize * 16, 0, &vmo));

  storage::Vmoid owned_vmoid;
  EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));
  vmoid_t vmoid = owned_vmoid.TakeId();

  BlockFifoRequest requests[] = {
      {
          .command = {.opcode = BLOCK_OPCODE_WRITE},
          .vmoid = vmoid,
          .length = 1,
          .vmo_offset = 0,
          .dev_offset = FakeSdmmcDevice::kBadRegionStart,
      },
  };

  EXPECT_STATUS(client->FifoTransaction(&requests[0], 1), ZX_ERR_IO);

  EXPECT_EQ(sdmmc_.command_counts().at(SDMMC_STOP_TRANSMISSION), 10);
}

TEST_P(SdmmcBlockDeviceTest, SendCmd12OnCommandFailureWhenAutoCmd12) {
  sdmmc_.set_host_info({
      .caps = SDMMC_HOST_CAP_AUTO_CMD12,
      .max_transfer_size = fuchsia_hardware_sdmmc::wire::kMaxTransferUnbounded,
  });

  ASSERT_OK(StartDriverForMmc());

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> client_result =
      GetRemoteBlockDeviceForBlockServer("user");
  ASSERT_OK(client_result);
  auto client = std::move(client_result.value());

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(FakeSdmmcDevice::kBlockSize * 16, 0, &vmo));

  storage::Vmoid owned_vmoid;
  EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));
  vmoid_t vmoid = owned_vmoid.TakeId();

  BlockFifoRequest requests[] = {
      {
          .command = {.opcode = BLOCK_OPCODE_WRITE},
          .vmoid = vmoid,
          .length = 1,
          .vmo_offset = 0,
          .dev_offset = FakeSdmmcDevice::kBadRegionStart,
      },
  };

  EXPECT_STATUS(client->FifoTransaction(&requests[0], 1), ZX_ERR_IO);

  EXPECT_EQ(sdmmc_.command_counts().at(SDMMC_STOP_TRANSMISSION), 10);
}

TEST_P(SdmmcBlockDeviceTest, Trim) {
  ASSERT_OK(StartDriverForMmc());

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> client_result =
      GetRemoteBlockDeviceForBlockServer("user");
  ASSERT_OK(client_result);
  auto client = std::move(client_result.value());

  zx::vmo vmo;
  const size_t vmo_size =
      fbl::round_up<size_t, size_t>(40 * FakeSdmmcDevice::kBlockSize, zx_system_get_page_size());
  fzl::VmoMapper mapper;
  ASSERT_OK(mapper.CreateAndMap(vmo_size, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr, &vmo));

  storage::Vmoid owned_vmoid;
  EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));
  vmoid_t vmoid = owned_vmoid.TakeId();

  FillVmo(mapper, 10, 0);

  BlockFifoRequest requests[] = {
      {
          .command = {.opcode = BLOCK_OPCODE_WRITE},
          .vmoid = vmoid,
          .length = 10,
          .vmo_offset = 0,
          .dev_offset = 100,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_FLUSH},
      },
      {
          .command = {.opcode = BLOCK_OPCODE_READ},
          .vmoid = vmoid,
          .length = 10,
          .vmo_offset = 10,
          .dev_offset = 100,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_TRIM},
          .length = 1,
          .dev_offset = 103,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_READ},
          .vmoid = vmoid,
          .length = 10,
          .vmo_offset = 20,
          .dev_offset = 100,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_TRIM},
          .length = 3,
          .dev_offset = 106,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_READ},
          .vmoid = vmoid,
          .length = 10,
          .vmo_offset = 30,
          .dev_offset = 100,
      },
  };

  EXPECT_OK(client->FifoTransaction(requests, 7));

  ASSERT_NO_FATAL_FAILURE(CheckVmo(mapper, 10, 10));

  ASSERT_NO_FATAL_FAILURE(CheckVmo(mapper, 3, 20));
  ASSERT_NO_FATAL_FAILURE(CheckVmoErased(mapper, 1, 23));
  ASSERT_NO_FATAL_FAILURE(CheckVmo(mapper, 6, 24));

  ASSERT_NO_FATAL_FAILURE(CheckVmo(mapper, 3, 30));
  ASSERT_NO_FATAL_FAILURE(CheckVmoErased(mapper, 1, 33));
  ASSERT_NO_FATAL_FAILURE(CheckVmo(mapper, 2, 34));
  ASSERT_NO_FATAL_FAILURE(CheckVmoErased(mapper, 3, 36));
  ASSERT_NO_FATAL_FAILURE(CheckVmo(mapper, 1, 39));
}

TEST_P(SdmmcBlockDeviceTest, TrimErrors) {
  ASSERT_OK(StartDriverForMmc());

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> client_result =
      GetRemoteBlockDeviceForBlockServer("user");
  ASSERT_OK(client_result);
  auto client = std::move(client_result.value());

  sdmmc_.set_command_callback(MMC_ERASE_GROUP_START,
                              [](const sdmmc_req_t& req, uint32_t out_response[4]) {
                                if (req.arg == 100) {
                                  out_response[0] |= MMC_STATUS_ERASE_SEQ_ERR;
                                }
                              });

  sdmmc_.set_command_callback(MMC_ERASE_GROUP_END,
                              [](const sdmmc_req_t& req, uint32_t out_response[4]) {
                                if (req.arg == 119) {
                                  out_response[0] |= MMC_STATUS_ADDR_OUT_OF_RANGE;
                                }
                              });

  BlockFifoRequest req1 = {
      .command = {.opcode = BLOCK_OPCODE_TRIM},
      .length = 10,
      .dev_offset = 10,
  };
  EXPECT_OK(client->FifoTransaction(&req1, 1));

  BlockFifoRequest req2 = {
      .command = {.opcode = BLOCK_OPCODE_TRIM},
      .length = 10,
      .dev_offset = FakeSdmmcDevice::kBadRegionStart | 0x40,
  };
  EXPECT_STATUS(client->FifoTransaction(&req2, 1), ZX_ERR_IO);

  BlockFifoRequest req3 = {
      .command = {.opcode = BLOCK_OPCODE_TRIM},
      .length = 10,
      .dev_offset = FakeSdmmcDevice::kBadRegionStart - 5,
  };
  EXPECT_STATUS(client->FifoTransaction(&req3, 1), ZX_ERR_IO);

  BlockFifoRequest req4 = {
      .command = {.opcode = BLOCK_OPCODE_TRIM},
      .length = 10,
      .dev_offset = 100,
  };
  EXPECT_STATUS(client->FifoTransaction(&req4, 1), ZX_ERR_IO);

  BlockFifoRequest req5 = {
      .command = {.opcode = BLOCK_OPCODE_TRIM},
      .length = 10,
      .dev_offset = 110,
  };
  EXPECT_STATUS(client->FifoTransaction(&req5, 1), ZX_ERR_IO);
}

TEST_P(SdmmcBlockDeviceTest, OnlyUserDataPartitionExists) {
  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    SetDefaultMmcExtCsd(out_data);
    out_data[MMC_EXT_CSD_BOOT_SIZE_MULT] = 0;
  });

  ASSERT_OK(StartDriverForMmc());

  EXPECT_EQ(block_device_->child_partition_devices().size(), 1);
  EXPECT_EQ(block_device_->child_partition_devices()[0]->partition(), USER_DATA_PARTITION);
  EXPECT_EQ(block_device_->child_rpmb_device(), nullptr);
}

TEST_P(SdmmcBlockDeviceTest, BootPartitionsExistButNotUsed) {
  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    SetDefaultMmcExtCsd(out_data);
    out_data[MMC_EXT_CSD_PARTITION_CONFIG] = 2;
    out_data[MMC_EXT_CSD_BOOT_SIZE_MULT] = 1;
  });

  ASSERT_OK(StartDriverForMmc());

  EXPECT_EQ(block_device_->child_partition_devices().size(), 1);
  EXPECT_EQ(block_device_->child_partition_devices()[0]->partition(), USER_DATA_PARTITION);
  EXPECT_EQ(block_device_->child_rpmb_device(), nullptr);
}

TEST_P(SdmmcBlockDeviceTest, WithBootPartitions) {
  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    out_data[MMC_EXT_CSD_CACHE_CTRL] = 1;
    out_data[MMC_EXT_CSD_CACHE_SIZE_LSB] = 0x78;
    out_data[MMC_EXT_CSD_CACHE_SIZE_250] = 0x56;
    out_data[MMC_EXT_CSD_CACHE_SIZE_251] = 0x34;
    out_data[MMC_EXT_CSD_CACHE_SIZE_MSB] = 0x12;
    out_data[MMC_EXT_CSD_PARTITION_CONFIG] = 0xa8;
    out_data[MMC_EXT_CSD_PARTITION_SWITCH_TIME] = 0;
    out_data[MMC_EXT_CSD_BOOT_SIZE_MULT] = 1;
    out_data[MMC_EXT_CSD_GENERIC_CMD6_TIME] = 0;
  });

  ASSERT_OK(StartDriverForMmc());

  EXPECT_EQ(block_device_->child_partition_devices().size(), 3);
  EXPECT_EQ(block_device_->child_partition_devices()[0]->partition(), USER_DATA_PARTITION);
  EXPECT_EQ(block_device_->child_partition_devices()[1]->partition(), BOOT_PARTITION_1);
  EXPECT_EQ(block_device_->child_partition_devices()[2]->partition(), BOOT_PARTITION_2);
  EXPECT_EQ(block_device_->child_rpmb_device(), nullptr);
}

TEST_P(SdmmcBlockDeviceTest, WithBootAndRpmbPartitions) {
  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    out_data[MMC_EXT_CSD_CACHE_CTRL] = 1;
    out_data[MMC_EXT_CSD_CACHE_SIZE_LSB] = 0x78;
    out_data[MMC_EXT_CSD_CACHE_SIZE_250] = 0x56;
    out_data[MMC_EXT_CSD_CACHE_SIZE_251] = 0x34;
    out_data[MMC_EXT_CSD_CACHE_SIZE_MSB] = 0x12;
    out_data[MMC_EXT_CSD_RPMB_SIZE_MULT] = 1;
    out_data[MMC_EXT_CSD_PARTITION_CONFIG] = 0xa8;
    out_data[MMC_EXT_CSD_PARTITION_SWITCH_TIME] = 0;
    out_data[MMC_EXT_CSD_BOOT_SIZE_MULT] = 1;
    out_data[MMC_EXT_CSD_GENERIC_CMD6_TIME] = 0;
  });

  ASSERT_OK(StartDriverForMmc());
  BindRpmbClient();

  EXPECT_EQ(block_device_->child_partition_devices().size(), 3);
  EXPECT_EQ(block_device_->child_partition_devices()[0]->partition(), USER_DATA_PARTITION);
  EXPECT_EQ(block_device_->child_partition_devices()[1]->partition(), BOOT_PARTITION_1);
  EXPECT_EQ(block_device_->child_partition_devices()[2]->partition(), BOOT_PARTITION_2);
  EXPECT_NE(block_device_->child_rpmb_device(), nullptr);
}

TEST_P(SdmmcBlockDeviceTest, CompleteTransactionsOnStop) {
  ASSERT_OK(StartDriverForMmc());

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> client_result =
      GetRemoteBlockDeviceForBlockServer("user");
  ASSERT_OK(client_result);
  auto client = std::move(client_result.value());

  // Suspend power so queued requests don't get completed.
  block_device_->SetPowerSuspendedForTest(true);

  zx::vmo vmo;
  const size_t vmo_size =
      fbl::round_up<size_t, size_t>(20 * FakeSdmmcDevice::kBlockSize, zx_system_get_page_size());
  fzl::VmoMapper mapper;
  ASSERT_OK(mapper.CreateAndMap(vmo_size, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr, &vmo));

  storage::Vmoid owned_vmoid;
  EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));
  vmoid_t vmoid = owned_vmoid.TakeId();

  BlockFifoRequest requests[] = {
      {
          .command = {.opcode = BLOCK_OPCODE_WRITE},
          .vmoid = vmoid,
          .length = 1,
          .vmo_offset = 0,
          .dev_offset = 0,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_WRITE},
          .vmoid = vmoid,
          .length = 5,
          .vmo_offset = 1,
          .dev_offset = 0x8000,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_FLUSH},
      },
      {
          .command = {.opcode = BLOCK_OPCODE_READ},
          .vmoid = vmoid,
          .length = 1,
          .vmo_offset = 6,
          .dev_offset = 0x400,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_READ},
          .vmoid = vmoid,
          .length = 10,
          .vmo_offset = 7,
          .dev_offset = 0x2000,
      },
  };

  std::thread t([&] {
    zx_status_t status = client->FifoTransaction(requests, 5);
    EXPECT_TRUE(status == ZX_ERR_PEER_CLOSED || status == ZX_ERR_CANCELED, "status is %d", status);
  });

  // Give the thread a chance to start and block.
  zx::nanosleep(zx::deadline_after(zx::msec(100)));

  EXPECT_OK(driver_test_.StopDriver());
  block_device_ = nullptr;
  dut_ = nullptr;

  t.join();
}

TEST_P(SdmmcBlockDeviceTest, ProbeMmcSendStatusRetry) {
  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    out_data[MMC_EXT_CSD_CACHE_CTRL] = 1;
    out_data[MMC_EXT_CSD_CACHE_SIZE_LSB] = 0x78;
    out_data[MMC_EXT_CSD_CACHE_SIZE_250] = 0x56;
    out_data[MMC_EXT_CSD_CACHE_SIZE_251] = 0x34;
    out_data[MMC_EXT_CSD_CACHE_SIZE_MSB] = 0x12;
    out_data[MMC_EXT_CSD_DEVICE_TYPE] = 1 << 4;
    out_data[MMC_EXT_CSD_GENERIC_CMD6_TIME] = 1;
  });
  sdmmc_.set_command_callback(SDMMC_SEND_STATUS, [](const sdmmc_req_t& req) {
    // Fail two out of three times during ProbeMmc, and then succeed for
    // SdmmcBlockDevice::AddDevice.
    static uint32_t call_count = 0;
    if (++call_count % 3 == 0 || call_count > 9) {
      return ZX_OK;
    } else {
      return ZX_ERR_IO_DATA_INTEGRITY;
    }
  });

  EXPECT_OK(StartDriverForMmc());
}

TEST_P(SdmmcBlockDeviceTest, ProbeMmcSendStatusFail) {
  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    out_data[MMC_EXT_CSD_CACHE_CTRL] = 1;
    out_data[MMC_EXT_CSD_CACHE_SIZE_LSB] = 0x78;
    out_data[MMC_EXT_CSD_CACHE_SIZE_250] = 0x56;
    out_data[MMC_EXT_CSD_CACHE_SIZE_251] = 0x34;
    out_data[MMC_EXT_CSD_CACHE_SIZE_MSB] = 0x12;
    out_data[MMC_EXT_CSD_DEVICE_TYPE] = 1 << 4;
    out_data[MMC_EXT_CSD_GENERIC_CMD6_TIME] = 1;
  });
  sdmmc_.set_command_callback(SDMMC_SEND_STATUS,
                              [](const sdmmc_req_t& req) { return ZX_ERR_IO_DATA_INTEGRITY; });

  EXPECT_NOT_OK(StartDriverForMmc());
}

TEST_P(SdmmcBlockDeviceTest, QueryBootPartitions) {
  ASSERT_OK(StartDriverForMmc());

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> boot1_result =
      GetRemoteBlockDeviceForBlockServer("boot1");
  ASSERT_OK(boot1_result);
  auto boot1_client = std::move(boot1_result.value());

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> boot2_result =
      GetRemoteBlockDeviceForBlockServer("boot2");
  ASSERT_OK(boot2_result);
  auto boot2_client = std::move(boot2_result.value());

  fuchsia_storage_block::wire::BlockInfo boot1_info, boot2_info;
  EXPECT_OK(boot1_client->BlockGetInfo(&boot1_info));
  EXPECT_OK(boot2_client->BlockGetInfo(&boot2_info));

  EXPECT_EQ(boot1_info.block_count, (0x10 * 128 * 1024) / FakeSdmmcDevice::kBlockSize);
  EXPECT_EQ(boot2_info.block_count, (0x10 * 128 * 1024) / FakeSdmmcDevice::kBlockSize);

  EXPECT_EQ(boot1_info.block_size, FakeSdmmcDevice::kBlockSize);
  EXPECT_EQ(boot2_info.block_size, FakeSdmmcDevice::kBlockSize);
}

TEST_P(SdmmcBlockDeviceTest, AccessBootPartitions) {
  ASSERT_OK(StartDriverForMmc());

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> user_result =
      GetRemoteBlockDeviceForBlockServer("user");
  ASSERT_OK(user_result);
  auto user_client = std::move(user_result.value());

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> boot1_result =
      GetRemoteBlockDeviceForBlockServer("boot1");
  ASSERT_OK(boot1_result);
  auto boot1_client = std::move(boot1_result.value());

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> boot2_result =
      GetRemoteBlockDeviceForBlockServer("boot2");
  ASSERT_OK(boot2_result);
  auto boot2_client = std::move(boot2_result.value());

  zx::vmo vmo;
  const size_t vmo_size =
      fbl::round_up<size_t, size_t>(16 * FakeSdmmcDevice::kBlockSize, zx_system_get_page_size());
  fzl::VmoMapper mapper;
  ASSERT_OK(mapper.CreateAndMap(vmo_size, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr, &vmo));

  storage::Vmoid owned_vmoid1, owned_vmoid2, owned_vmoid3;
  EXPECT_OK(boot1_client->BlockAttachVmo(vmo, &owned_vmoid1));
  EXPECT_OK(boot2_client->BlockAttachVmo(vmo, &owned_vmoid2));
  EXPECT_OK(user_client->BlockAttachVmo(vmo, &owned_vmoid3));

  vmoid_t vmoid1 = owned_vmoid1.TakeId();
  vmoid_t vmoid2 = owned_vmoid2.TakeId();
  vmoid_t vmoid3 = owned_vmoid3.TakeId();

  FillVmo(mapper, 1, 0);
  FillSdmmc(5, 10);
  FillVmo(mapper, 10, 6);

  sdmmc_.set_command_callback(MMC_SWITCH, [](const sdmmc_req_t& req) {
    const uint32_t index = (req.arg >> 16) & 0xff;
    const uint32_t value = (req.arg >> 8) & 0xff;
    EXPECT_EQ(index, MMC_EXT_CSD_PARTITION_CONFIG);
    EXPECT_EQ(value, 0xa8 | BOOT_PARTITION_1);
  });

  BlockFifoRequest req1 = {
      .command = {.opcode = BLOCK_OPCODE_WRITE},
      .vmoid = vmoid1,
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = 0,
  };
  EXPECT_OK(boot1_client->FifoTransaction(&req1, 1));

  sdmmc_.set_command_callback(MMC_SWITCH, [](const sdmmc_req_t& req) {
    const uint32_t index = (req.arg >> 16) & 0xff;
    const uint32_t value = (req.arg >> 8) & 0xff;
    EXPECT_EQ(index, MMC_EXT_CSD_PARTITION_CONFIG);
    EXPECT_EQ(value, 0xa8 | BOOT_PARTITION_2);
  });

  BlockFifoRequest req2 = {
      .command = {.opcode = BLOCK_OPCODE_READ},
      .vmoid = vmoid2,
      .length = 5,
      .vmo_offset = 1,
      .dev_offset = 10,
  };
  EXPECT_OK(boot2_client->FifoTransaction(&req2, 1));

  sdmmc_.set_command_callback(MMC_SWITCH, [](const sdmmc_req_t& req) {
    const uint32_t index = (req.arg >> 16) & 0xff;
    const uint32_t value = (req.arg >> 8) & 0xff;
    EXPECT_EQ(index, MMC_EXT_CSD_PARTITION_CONFIG);
    EXPECT_EQ(value, 0xa8 | USER_DATA_PARTITION);
  });

  BlockFifoRequest req3 = {
      .command = {.opcode = BLOCK_OPCODE_WRITE},
      .vmoid = vmoid3,
      .length = 10,
      .vmo_offset = 6,
      .dev_offset = 500,
  };
  EXPECT_OK(user_client->FifoTransaction(&req3, 1));

  ASSERT_NO_FATAL_FAILURE(CheckSdmmc(1, 0));
  ASSERT_NO_FATAL_FAILURE(CheckVmo(mapper, 5, 1));
  ASSERT_NO_FATAL_FAILURE(CheckSdmmc(10, 500));
}

TEST_P(SdmmcBlockDeviceTest, BootPartitionRepeatedAccess) {
  ASSERT_OK(StartDriverForMmc());

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> client_result =
      GetRemoteBlockDeviceForBlockServer("boot2");
  ASSERT_OK(client_result);
  auto client = std::move(client_result.value());

  zx::vmo vmo;
  const size_t vmo_size =
      fbl::round_up<size_t, size_t>(10 * FakeSdmmcDevice::kBlockSize, zx_system_get_page_size());
  fzl::VmoMapper mapper;
  ASSERT_OK(mapper.CreateAndMap(vmo_size, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr, &vmo));

  storage::Vmoid owned_vmoid;
  EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));
  vmoid_t vmoid = owned_vmoid.TakeId();

  FillSdmmc(1, 0);
  FillVmo(mapper, 5, 0);
  FillVmo(mapper, 2, 5);

  sdmmc_.set_command_callback(MMC_SWITCH, [](const sdmmc_req_t& req) {
    const uint32_t index = (req.arg >> 16) & 0xff;
    const uint32_t value = (req.arg >> 8) & 0xff;
    EXPECT_EQ(index, MMC_EXT_CSD_PARTITION_CONFIG);
    EXPECT_EQ(value, 0xa8 | BOOT_PARTITION_2);
  });

  BlockFifoRequest req1 = {
      .command = {.opcode = BLOCK_OPCODE_READ},
      .vmoid = vmoid,
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = 0,
  };
  EXPECT_OK(client->FifoTransaction(&req1, 1));

  // Repeated accesses to one partition should not generate more than one MMC_SWITCH command.
  sdmmc_.set_command_callback(MMC_SWITCH, [](const sdmmc_req_t& req) { FAIL(); });

  BlockFifoRequest reqs[] = {
      {
          .command = {.opcode = BLOCK_OPCODE_WRITE},
          .vmoid = vmoid,
          .length = 5,
          .vmo_offset = 0,
          .dev_offset = 10,
      },
      {
          .command = {.opcode = BLOCK_OPCODE_WRITE},
          .vmoid = vmoid,
          .length = 2,
          .vmo_offset = 5,
          .dev_offset = 5,
      },
  };
  EXPECT_OK(client->FifoTransaction(reqs, 2));

  ASSERT_NO_FATAL_FAILURE(CheckVmo(mapper, 1, 0));
  ASSERT_NO_FATAL_FAILURE(CheckSdmmc(5, 10));
  ASSERT_NO_FATAL_FAILURE(CheckSdmmc(2, 5));
}

TEST_P(SdmmcBlockDeviceTest, AccessBootPartitionOutOfRange) {
  ASSERT_OK(StartDriverForMmc());

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> client_result =
      GetRemoteBlockDeviceForBlockServer("boot1");
  ASSERT_OK(client_result);
  auto client = std::move(client_result.value());

  zx::vmo vmo;
  const size_t vmo_size =
      fbl::round_up<size_t, size_t>(16 * FakeSdmmcDevice::kBlockSize, zx_system_get_page_size());
  fzl::VmoMapper mapper;
  ASSERT_OK(mapper.CreateAndMap(vmo_size, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr, &vmo));

  storage::Vmoid owned_vmoid;
  EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));
  vmoid_t vmoid = owned_vmoid.TakeId();

  BlockFifoRequest req1 = {
      .command = {.opcode = BLOCK_OPCODE_WRITE},
      .vmoid = vmoid,
      .length = 1,
      .dev_offset = 4096,
  };
  EXPECT_STATUS(client->FifoTransaction(&req1, 1), ZX_ERR_OUT_OF_RANGE);

  BlockFifoRequest req2 = {
      .command = {.opcode = BLOCK_OPCODE_WRITE},
      .vmoid = vmoid,
      .length = 8,
      .dev_offset = 4088,
  };
  EXPECT_OK(client->FifoTransaction(&req2, 1));

  BlockFifoRequest req3 = {
      .command = {.opcode = BLOCK_OPCODE_READ},
      .vmoid = vmoid,
      .length = 9,
      .dev_offset = 4088,
  };
  EXPECT_STATUS(client->FifoTransaction(&req3, 1), ZX_ERR_OUT_OF_RANGE);

  BlockFifoRequest req4 = {
      .command = {.opcode = BLOCK_OPCODE_WRITE},
      .vmoid = vmoid,
      .length = 16,
      .dev_offset = 4088,
  };
  EXPECT_STATUS(client->FifoTransaction(&req4, 1), ZX_ERR_OUT_OF_RANGE);

  BlockFifoRequest req5 = {
      .command = {.opcode = BLOCK_OPCODE_READ},
      .vmoid = vmoid,
      .length = 0,
      .dev_offset = 2048,
  };
  EXPECT_STATUS(client->FifoTransaction(&req5, 1), ZX_ERR_INVALID_ARGS);

  BlockFifoRequest req6 = {
      .command = {.opcode = BLOCK_OPCODE_WRITE},
      .vmoid = vmoid,
      .length = 1,
      .dev_offset = 4095,
  };
  EXPECT_OK(client->FifoTransaction(&req6, 1));
}

TEST_P(SdmmcBlockDeviceTest, ProbeUsesPrefsHs) {
  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    out_data[MMC_EXT_CSD_CACHE_CTRL] = 1;
    out_data[MMC_EXT_CSD_CACHE_SIZE_LSB] = 0x78;
    out_data[MMC_EXT_CSD_CACHE_SIZE_250] = 0x56;
    out_data[MMC_EXT_CSD_CACHE_SIZE_251] = 0x34;
    out_data[MMC_EXT_CSD_CACHE_SIZE_MSB] = 0x12;
    out_data[MMC_EXT_CSD_DEVICE_TYPE] = 0b0101'0110;  // Card supports HS200/400, HS/DDR.
    out_data[MMC_EXT_CSD_PARTITION_SWITCH_TIME] = 0;
    out_data[MMC_EXT_CSD_GENERIC_CMD6_TIME] = 0;
  });

  const auto speed_capabilities = fuchsia_hardware_sdmmc::SdmmcHostPrefs::kDisableHs200 |
                                  fuchsia_hardware_sdmmc::SdmmcHostPrefs::kDisableHs400 |
                                  fuchsia_hardware_sdmmc::SdmmcHostPrefs::kDisableHsddr;
  EXPECT_OK(StartDriverForMmc(speed_capabilities));

  EXPECT_EQ(sdmmc_.timing(), SDMMC_TIMING_HS);
}

TEST_P(SdmmcBlockDeviceTest, ProbeUsesPrefsHsDdr) {
  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    out_data[MMC_EXT_CSD_CACHE_CTRL] = 1;
    out_data[MMC_EXT_CSD_CACHE_SIZE_LSB] = 0x78;
    out_data[MMC_EXT_CSD_CACHE_SIZE_250] = 0x56;
    out_data[MMC_EXT_CSD_CACHE_SIZE_251] = 0x34;
    out_data[MMC_EXT_CSD_CACHE_SIZE_MSB] = 0x12;
    out_data[MMC_EXT_CSD_DEVICE_TYPE] = 0b0101'0110;  // Card supports HS200/400, HS/DDR.
    out_data[MMC_EXT_CSD_PARTITION_SWITCH_TIME] = 0;
    out_data[MMC_EXT_CSD_GENERIC_CMD6_TIME] = 0;
  });

  const auto speed_capabilities = fuchsia_hardware_sdmmc::SdmmcHostPrefs::kDisableHs200 |
                                  fuchsia_hardware_sdmmc::SdmmcHostPrefs::kDisableHs400;
  EXPECT_OK(StartDriverForMmc(speed_capabilities));

  EXPECT_EQ(sdmmc_.timing(), SDMMC_TIMING_HSDDR);
}

TEST_P(SdmmcBlockDeviceTest, ProbeHs400) {
  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    out_data[MMC_EXT_CSD_CACHE_CTRL] = 1;
    out_data[MMC_EXT_CSD_CACHE_SIZE_LSB] = 0x78;
    out_data[MMC_EXT_CSD_CACHE_SIZE_250] = 0x56;
    out_data[MMC_EXT_CSD_CACHE_SIZE_251] = 0x34;
    out_data[MMC_EXT_CSD_CACHE_SIZE_MSB] = 0x12;
    out_data[MMC_EXT_CSD_DEVICE_TYPE] = 0b0101'0110;  // Card supports HS200/400, HS/DDR.
    out_data[MMC_EXT_CSD_PARTITION_SWITCH_TIME] = 0;
    out_data[MMC_EXT_CSD_GENERIC_CMD6_TIME] = 0;
  });

  uint32_t timing = MMC_EXT_CSD_HS_TIMING_LEGACY;
  sdmmc_.set_command_callback(SDMMC_SEND_STATUS, [&](const sdmmc_req_t& req) {
    // SDMMC_SEND_STATUS is the first command sent to the card after MMC_SWITCH. When initializing
    // HS400 mode the host sets the card timing to HS200 and then to HS, and should change the
    // timing and frequency on the host before issuing SDMMC_SEND_STATUS.
    if (timing == MMC_EXT_CSD_HS_TIMING_HS) {
      EXPECT_EQ(sdmmc_.timing(), SDMMC_TIMING_HS);
      EXPECT_LE(sdmmc_.bus_freq(), 52'000'000);
    }
  });

  sdmmc_.set_command_callback(MMC_SWITCH, [&](const sdmmc_req_t& req) {
    const uint32_t index = (req.arg >> 16) & 0xff;
    if (index == MMC_EXT_CSD_HS_TIMING) {
      const uint32_t value = (req.arg >> 8) & 0xff;
      EXPECT_GE(value, MMC_EXT_CSD_HS_TIMING_LEGACY);
      EXPECT_LE(value, MMC_EXT_CSD_HS_TIMING_HS400);
      timing = value;
    }
  });

  EXPECT_OK(StartDriverForMmc());

  EXPECT_EQ(sdmmc_.timing(), SDMMC_TIMING_HS400);
}

TEST_P(SdmmcBlockDeviceTest, ProbeHs400EnhancedStrobe) {
  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    out_data[MMC_EXT_CSD_DEVICE_TYPE] = 0b0101'0110;  // Card supports HS200/400, HS/DDR.
    out_data[MMC_EXT_CSD_STROBE_SUPPORT] = MMC_EXT_CSD_STROBE_SUPPORT_ENHANCED_STROBE;
  });

  sdmmc_.set_host_info({
      .caps = SDMMC_HOST_CAP_HS400_ENHANCED_STROBE,
      .max_transfer_size = fuchsia_hardware_sdmmc::wire::kMaxTransferUnbounded,
      .max_buffer_regions = 8,
  });

  uint32_t bus_width = MMC_EXT_CSD_BUS_WIDTH_1;
  uint32_t timing = MMC_EXT_CSD_HS_TIMING_LEGACY;
  sdmmc_.set_command_callback(MMC_SWITCH, [&](const sdmmc_req_t& req) {
    const uint32_t index = (req.arg >> 16) & 0xff;
    const uint32_t value = (req.arg >> 8) & 0xff;
    switch (index) {
      case MMC_EXT_CSD_HS_TIMING:
        timing = value;
        break;
      case MMC_EXT_CSD_BUS_WIDTH:
        bus_width = value;
        break;
      default:
        break;
    }
  });

  EXPECT_OK(StartDriverForMmc());

  EXPECT_EQ(sdmmc_.timing(), SDMMC_TIMING_HS400_ENHANCED_STROBE);
  EXPECT_EQ(sdmmc_.bus_freq(), 200'000'000);
  EXPECT_EQ(timing, MMC_EXT_CSD_HS_TIMING_HS400);
  EXPECT_EQ(bus_width, MMC_EXT_CSD_BUS_WIDTH_ENHANCED_STROBE | MMC_EXT_CSD_BUS_WIDTH_8_DDR);
}

TEST_P(SdmmcBlockDeviceTest, FallBackToHsIfTuningFails) {
  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    out_data[MMC_EXT_CSD_DEVICE_TYPE] = 0b0101'0110;  // Card supports HS200/400, HS/DDR.
  });

  // Make tuning fail and verify that probe still succeeds.
  sdmmc_.set_perform_tuning_status(ZX_ERR_IO);

  uint32_t timing = MMC_EXT_CSD_HS_TIMING_LEGACY;
  sdmmc_.set_command_callback(MMC_SWITCH, [&](const sdmmc_req_t& req) {
    const uint32_t index = (req.arg >> 16) & 0xff;
    if (index == MMC_EXT_CSD_HS_TIMING) {
      timing = (req.arg >> 8) & 0xff;
      // We should never reach HS400 as it requires a transition through HS400.
      EXPECT_NE(timing, MMC_EXT_CSD_HS_TIMING_HS400);
    }
  });

  EXPECT_OK(StartDriverForMmc());

  EXPECT_EQ(sdmmc_.timing(), SDMMC_TIMING_HSDDR);
  EXPECT_EQ(timing, MMC_EXT_CSD_HS_TIMING_HS);
}

TEST_P(SdmmcBlockDeviceTest, ProbeSd) {
  ASSERT_OK(StartDriverForSd());

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> client_result =
      GetRemoteBlockDeviceForBlockServer("user");
  ASSERT_OK(client_result);
  auto client = std::move(client_result.value());

  fuchsia_storage_block::wire::BlockInfo info;
  EXPECT_OK(client->BlockGetInfo(&info));

  EXPECT_EQ(info.block_size, 512);
  EXPECT_EQ(info.block_count, 0x38'1235 * 1024ul);
}

TEST_P(SdmmcBlockDeviceTest, RpmbPartition) {
  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    out_data[MMC_EXT_CSD_CACHE_CTRL] = 1;
    out_data[MMC_EXT_CSD_CACHE_SIZE_LSB] = 0x78;
    out_data[MMC_EXT_CSD_CACHE_SIZE_250] = 0x56;
    out_data[MMC_EXT_CSD_CACHE_SIZE_251] = 0x34;
    out_data[MMC_EXT_CSD_CACHE_SIZE_MSB] = 0x12;
    out_data[MMC_EXT_CSD_RPMB_SIZE_MULT] = 0x74;
    out_data[MMC_EXT_CSD_PARTITION_CONFIG] = 0xa8;
    out_data[MMC_EXT_CSD_REL_WR_SEC_C] = 1;
    out_data[MMC_EXT_CSD_BOOT_SIZE_MULT] = 0x10;
    out_data[MMC_EXT_CSD_GENERIC_CMD6_TIME] = 0;
  });

  ASSERT_OK(StartDriverForMmc());
  BindRpmbClient();

  sync_completion_t completion;
  rpmb_client_->GetDeviceInfo().ThenExactlyOnce(
      [&](fidl::WireUnownedResult<fuchsia_hardware_rpmb::Rpmb::GetDeviceInfo>& result) {
        if (!result.ok()) {
          FAIL("GetDeviceInfo failed: %s", result.error().FormatDescription().c_str());
          return;
        }
        auto* response = result.Unwrap();
        EXPECT_TRUE(response->info.is_emmc_info());
        EXPECT_EQ(response->info.emmc_info().rpmb_size, 0x74);
        EXPECT_EQ(response->info.emmc_info().reliable_write_sector_count, 1);
        sync_completion_signal(&completion);
      });

  sync_completion_wait(&completion, zx::duration::infinite().get());
  sync_completion_reset(&completion);

  fzl::VmoMapper tx_frames_mapper;
  fzl::VmoMapper rx_frames_mapper;

  zx::vmo tx_frames;
  zx::vmo rx_frames;

  ASSERT_OK(tx_frames_mapper.CreateAndMap(512 * 4, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr,
                                          &tx_frames));
  ASSERT_OK(rx_frames_mapper.CreateAndMap(512 * 4, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr,
                                          &rx_frames));

  fuchsia_hardware_rpmb::wire::Request write_read_request = {};
  ASSERT_OK(tx_frames.duplicate(ZX_RIGHT_SAME_RIGHTS, &write_read_request.tx_frames.vmo));

  write_read_request.tx_frames.offset = 1024;
  write_read_request.tx_frames.size = 1024;
  FillVmo(tx_frames_mapper, 2, 2);

  fuchsia_mem::wire::Range rx_frames_range = {};
  ASSERT_OK(rx_frames.duplicate(ZX_RIGHT_SAME_RIGHTS, &rx_frames_range.vmo));
  rx_frames_range.offset = 512;
  rx_frames_range.size = 1536;
  write_read_request.rx_frames =
      fidl::ObjectView<fuchsia_mem::wire::Range>::FromExternal(&rx_frames_range);

  sdmmc_.set_command_callback(MMC_SWITCH, [](const sdmmc_req_t& req) {
    const uint32_t index = (req.arg >> 16) & 0xff;
    const uint32_t value = (req.arg >> 8) & 0xff;
    EXPECT_EQ(index, MMC_EXT_CSD_PARTITION_CONFIG);
    EXPECT_EQ(value, 0xa8 | RPMB_PARTITION);
  });

  rpmb_client_->Request(std::move(write_read_request))
      .ThenExactlyOnce([&](fidl::WireUnownedResult<fuchsia_hardware_rpmb::Rpmb::Request>& result) {
        if (!result.ok()) {
          FAIL("Request failed: %s", result.error().FormatDescription().c_str());
          return;
        }
        EXPECT_FALSE(result->is_error());
        sync_completion_signal(&completion);
      });

  sync_completion_wait(&completion, zx::duration::infinite().get());
  sync_completion_reset(&completion);

  ASSERT_NO_FATAL_FAILURE(CheckSdmmc(2, 0));
  // The first two blocks were written by the RPMB write request, and read back by the read request.
  ASSERT_NO_FATAL_FAILURE(CheckVmo(rx_frames_mapper, 2, 1));

  fuchsia_hardware_rpmb::wire::Request write_request = {};
  ASSERT_OK(tx_frames.duplicate(ZX_RIGHT_SAME_RIGHTS, &write_request.tx_frames.vmo));

  write_request.tx_frames.offset = 0;
  write_request.tx_frames.size = 2048;
  FillVmo(tx_frames_mapper, 4, 0);

  // Repeated accesses to one partition should not generate more than one MMC_SWITCH command.
  sdmmc_.set_command_callback(MMC_SWITCH, []([[maybe_unused]] const sdmmc_req_t& req) { FAIL(); });

  sdmmc_.set_command_callback(SDMMC_SET_BLOCK_COUNT, [](const sdmmc_req_t& req) {
    EXPECT_TRUE(req.arg & MMC_SET_BLOCK_COUNT_RELIABLE_WRITE);
  });

  rpmb_client_->Request(std::move(write_request))
      .ThenExactlyOnce([&](fidl::WireUnownedResult<fuchsia_hardware_rpmb::Rpmb::Request>& result) {
        if (!result.ok()) {
          FAIL("Request failed: %s", result.error().FormatDescription().c_str());
          return;
        }
        EXPECT_FALSE(result->is_error());
        sync_completion_signal(&completion);
      });

  sync_completion_wait(&completion, zx::duration::infinite().get());
  sync_completion_reset(&completion);
}

TEST_P(SdmmcBlockDeviceTest, RpmbRequestLimit) {
  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    out_data[MMC_EXT_CSD_CACHE_CTRL] = 1;
    out_data[MMC_EXT_CSD_CACHE_SIZE_LSB] = 0x78;
    out_data[MMC_EXT_CSD_CACHE_SIZE_250] = 0x56;
    out_data[MMC_EXT_CSD_CACHE_SIZE_251] = 0x34;
    out_data[MMC_EXT_CSD_CACHE_SIZE_MSB] = 0x12;
    out_data[MMC_EXT_CSD_RPMB_SIZE_MULT] = 0x74;
    out_data[MMC_EXT_CSD_PARTITION_CONFIG] = 0xa8;
    out_data[MMC_EXT_CSD_REL_WR_SEC_C] = 1;
    out_data[MMC_EXT_CSD_BOOT_SIZE_MULT] = 0x10;
    out_data[MMC_EXT_CSD_GENERIC_CMD6_TIME] = 0;
  });

  ASSERT_OK(StartDriverForMmc());
  BindRpmbClient();
  sync_completion_t completion;
  block_device_->StopWorkerDispatcher(fdf::StopCompleter([&](zx::result<> result) {
    EXPECT_OK(result);
    sync_completion_signal(&completion);
  }));
  EXPECT_OK(sync_completion_wait(&completion, zx::duration::infinite().get()));

  zx::vmo tx_frames;
  ASSERT_OK(zx::vmo::create(512, 0, &tx_frames));

  for (int i = 0; i < 16; i++) {
    fuchsia_hardware_rpmb::wire::Request request = {};
    ASSERT_OK(tx_frames.duplicate(ZX_RIGHT_SAME_RIGHTS, &request.tx_frames.vmo));
    request.tx_frames.offset = 0;
    request.tx_frames.size = 512;
    rpmb_client_->Request(std::move(request))
        .ThenExactlyOnce(
            [&]([[maybe_unused]] fidl::WireUnownedResult<fuchsia_hardware_rpmb::Rpmb::Request>&
                    result) {});
  }

  fuchsia_hardware_rpmb::wire::Request error_request = {};
  ASSERT_OK(tx_frames.duplicate(ZX_RIGHT_SAME_RIGHTS, &error_request.tx_frames.vmo));
  error_request.tx_frames.offset = 0;
  error_request.tx_frames.size = 512;

  sync_completion_t error_completion;
  rpmb_client_->Request(std::move(error_request))
      .ThenExactlyOnce([&](fidl::WireUnownedResult<fuchsia_hardware_rpmb::Rpmb::Request>& result) {
        if (!result.ok()) {
          FAIL("Request failed: %s", result.error().FormatDescription().c_str());
          return;
        }
        EXPECT_TRUE(result->is_error());
        sync_completion_signal(&error_completion);
      });

  sync_completion_wait(&error_completion, zx::duration::infinite().get());
}

TEST_P(SdmmcBlockDeviceTest, RpmbPartitionReliableWrite) {
  constexpr uint16_t kWriteDataRequest = 3;
  constexpr uint16_t kReadDataRequest = 4;

  struct Frame {
    uint8_t stuff[196];
    uint8_t mac[32];
    uint8_t data[256];
    uint8_t nonce[16];
    uint32_t write_counter;
    uint16_t address;
    uint16_t block_count;
    uint16_t result;
    uint16_t request;
  };

  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    out_data[MMC_EXT_CSD_RPMB_SIZE_MULT] = 0x74;
  });

  ASSERT_OK(StartDriverForMmc());
  BindRpmbClient();

  std::vector<uint32_t> block_count_args;
  sdmmc_.set_command_callback(SDMMC_SET_BLOCK_COUNT,
                              [&](const sdmmc_req_t& req) { block_count_args.push_back(req.arg); });

  zx::vmo tx_frames, rx_frames;
  ASSERT_OK(zx::vmo::create(1024, 0, &tx_frames));
  ASSERT_OK(zx::vmo::create(512, 0, &rx_frames));

  {
    Frame frame{.request = htobe16(kWriteDataRequest)};
    EXPECT_OK(tx_frames.write(&frame, 512, sizeof(frame)));
  }

  fuchsia_mem::wire::Range rx_frames_range = {.offset = 0, .size = 512};
  ASSERT_OK(rx_frames.duplicate(ZX_RIGHT_SAME_RIGHTS, &rx_frames_range.vmo));

  fuchsia_hardware_rpmb::wire::Request write_read_request = {
      .tx_frames =
          {
              .offset = 512,  // Verify that the offset is applied by the RPMB driver.
              .size = 512,
          },
      .rx_frames = fidl::ObjectView<fuchsia_mem::wire::Range>::FromExternal(&rx_frames_range),
  };
  ASSERT_OK(tx_frames.duplicate(ZX_RIGHT_SAME_RIGHTS, &write_read_request.tx_frames.vmo));

  {
    auto result = rpmb_client_.sync()->Request(std::move(write_read_request));
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }

  // Write data request: reliable write should be used.
  EXPECT_EQ(block_count_args, (std::vector<uint32_t>{MMC_SET_BLOCK_COUNT_RELIABLE_WRITE | 1, 1}));
  block_count_args.clear();

  {
    Frame frame{.request = htobe16(kReadDataRequest)};
    EXPECT_OK(tx_frames.write(&frame, 0, sizeof(frame)));
  }

  rx_frames_range = {.offset = 0, .size = 512};
  ASSERT_OK(rx_frames.duplicate(ZX_RIGHT_SAME_RIGHTS, &rx_frames_range.vmo));

  write_read_request = {
      .tx_frames = {.offset = 0, .size = 512},
      .rx_frames = fidl::ObjectView<fuchsia_mem::wire::Range>::FromExternal(&rx_frames_range),
  };
  ASSERT_OK(tx_frames.duplicate(ZX_RIGHT_SAME_RIGHTS, &write_read_request.tx_frames.vmo));

  {
    auto result = rpmb_client_.sync()->Request(std::move(write_read_request));
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }

  // Read data request: reliable write should not be used.
  EXPECT_EQ(block_count_args, (std::vector<uint32_t>{1, 1}));
}

TEST_P(SdmmcBlockDeviceTest, RpmbMultipleRequests) {
  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    out_data[MMC_EXT_CSD_RPMB_SIZE_MULT] = 0x74;
    out_data[MMC_EXT_CSD_PARTITION_CONFIG] = 0xa8;
    out_data[MMC_EXT_CSD_REL_WR_SEC_C] = 1;
    // 32-frame writes not supported.
    out_data[MMC_EXT_CSD_WR_REL_PARAM] = 0;
  });

  ASSERT_OK(StartDriverForMmc());
  BindRpmbClient();

  sync_completion_t completion;

  fzl::VmoMapper tx_frames_mapper;
  zx::vmo tx_frames;

  ASSERT_OK(tx_frames_mapper.CreateAndMap(512 * 37ul, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr,
                                          &tx_frames));

  fuchsia_hardware_rpmb::wire::Request write_read_request = {};
  ASSERT_OK(tx_frames.duplicate(ZX_RIGHT_SAME_RIGHTS, &write_read_request.tx_frames.vmo));

  write_read_request.tx_frames.offset = 0;
  write_read_request.tx_frames.size = 512 * 37ul;
  FillVmo(tx_frames_mapper, 37, 0);

  // The SDMMC driver should make 18 two-frame requests followed by a one-frame request.
  uint32_t write_count = 0;
  sdmmc_.set_command_callback(SDMMC_SET_BLOCK_COUNT, [&write_count](const sdmmc_req_t& req) {
    if (write_count < 18) {
      EXPECT_EQ(req.arg & 0xffff, 2);
    } else if (write_count == 18) {
      EXPECT_EQ(req.arg & 0xffff, 1);
    }
  });
  sdmmc_.set_command_callback(SDMMC_WRITE_MULTIPLE_BLOCK,
                              [&write_count](cpp20::span<uint8_t> data) {
                                if (write_count < 18) {
                                  EXPECT_EQ(data.size(), 1024);
                                } else if (write_count == 18) {
                                  EXPECT_EQ(data.size(), 512);
                                }
                                write_count++;
                              });

  rpmb_client_->Request(std::move(write_read_request))
      .ThenExactlyOnce([&](fidl::WireUnownedResult<fuchsia_hardware_rpmb::Rpmb::Request>& result) {
        if (!result.ok()) {
          FAIL("Request failed: %s", result.error().FormatDescription().c_str());
          return;
        }
        EXPECT_FALSE(result->is_error());
        sync_completion_signal(&completion);
      });

  sync_completion_wait(&completion, zx::duration::infinite().get());
  sync_completion_reset(&completion);

  EXPECT_EQ(write_count, 19);
  // The address is always zero for RPMB requests, so the first two blocks will end up getting
  // overwritten.
  ASSERT_NO_FATAL_FAILURE(CheckSdmmc(2, 0));
}

TEST_P(SdmmcBlockDeviceTest, RpmbMultipleRequests32FramesSupported) {
  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    out_data[MMC_EXT_CSD_RPMB_SIZE_MULT] = 0x74;
    out_data[MMC_EXT_CSD_PARTITION_CONFIG] = 0xa8;
    out_data[MMC_EXT_CSD_REL_WR_SEC_C] = 1;
    // 32-frame writes supported.
    out_data[MMC_EXT_CSD_WR_REL_PARAM] = MMC_EXT_CSD_EN_RPMB_REL_WR_MASK;
  });

  ASSERT_OK(StartDriverForMmc());
  BindRpmbClient();

  sync_completion_t completion;

  fzl::VmoMapper tx_frames_mapper;
  zx::vmo tx_frames;

  ASSERT_OK(tx_frames_mapper.CreateAndMap(512 * 37ul, ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr,
                                          &tx_frames));

  fuchsia_hardware_rpmb::wire::Request write_read_request = {};
  ASSERT_OK(tx_frames.duplicate(ZX_RIGHT_SAME_RIGHTS, &write_read_request.tx_frames.vmo));

  write_read_request.tx_frames.offset = 0;
  write_read_request.tx_frames.size = 512 * 37ul;
  FillVmo(tx_frames_mapper, 37, 0);

  // The SDMMC driver should make a 32-frame request, two two-frame requests, and then a one-frame
  // request.
  uint32_t write_count = 0;
  sdmmc_.set_command_callback(SDMMC_SET_BLOCK_COUNT, [&write_count](const sdmmc_req_t& req) {
    if (write_count == 0) {
      EXPECT_EQ(req.arg & 0xffff, 32);
    } else if (write_count == 3) {
      EXPECT_EQ(req.arg & 0xffff, 1);
    } else if (write_count < 3) {
      EXPECT_EQ(req.arg & 0xffff, 2);
    }
  });
  sdmmc_.set_command_callback(SDMMC_WRITE_MULTIPLE_BLOCK,
                              [&write_count](cpp20::span<uint8_t> data) {
                                if (write_count == 0) {
                                  EXPECT_EQ(data.size(), 512 * 32);
                                } else if (write_count == 3) {
                                  EXPECT_EQ(data.size(), 512);
                                } else if (write_count < 3) {
                                  EXPECT_EQ(data.size(), 1024);
                                }
                                write_count++;
                              });

  rpmb_client_->Request(std::move(write_read_request))
      .ThenExactlyOnce([&](fidl::WireUnownedResult<fuchsia_hardware_rpmb::Rpmb::Request>& result) {
        if (!result.ok()) {
          FAIL("Request failed: %s", result.error().FormatDescription().c_str());
          return;
        }
        EXPECT_FALSE(result->is_error());
        sync_completion_signal(&completion);
      });

  sync_completion_wait(&completion, zx::duration::infinite().get());
  sync_completion_reset(&completion);

  EXPECT_EQ(write_count, 4);
  ASSERT_NO_FATAL_FAILURE(CheckSdmmc(32, 0));
}

void SdmmcBlockDeviceTest::QueueBlockOps() {
  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> client_result =
      GetRemoteBlockDeviceForBlockServer("user");
  ASSERT_OK(client_result);
  auto client = std::move(client_result.value());

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(FakeSdmmcDevice::kBlockSize, 0, &vmo));

  storage::Vmoid owned_vmoid;
  EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));
  vmoid_t vmoid = owned_vmoid.TakeId();

  while (run_threads_.load()) {
    BlockFifoRequest request = {
        .command = {.opcode = BLOCK_OPCODE_READ},
        .vmoid = vmoid,
        .length = 1,
        .vmo_offset = 0,
        .dev_offset = 0,
    };
    EXPECT_OK(client->FifoTransaction(&request, 1));
  }
}

void SdmmcBlockDeviceTest::QueueRpmbRequests() {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(512, 0, &vmo));

  std::atomic<uint32_t> outstanding_op_count = 0;
  sync_completion_t completion;

  while (run_threads_.load()) {
    for (uint32_t i = outstanding_op_count.load(); i < kMaxOutstandingOps;
         i = outstanding_op_count.fetch_add(1) + 1) {
      fuchsia_hardware_rpmb::wire::Request request = {};
      EXPECT_OK(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &request.tx_frames.vmo));
      request.tx_frames.offset = 0;
      request.tx_frames.size = 512;

      rpmb_client_->Request(std::move(request))
          .ThenExactlyOnce(
              [&](fidl::WireUnownedResult<fuchsia_hardware_rpmb::Rpmb::Request>& result) {
                if (!result.ok()) {
                  FAIL("Request failed: %s", result.error().FormatDescription().c_str());
                  return;
                }

                EXPECT_FALSE(result->is_error());
                if (outstanding_op_count.fetch_sub(1) == kMaxOutstandingOps / 2) {
                  sync_completion_signal(&completion);
                }
              });
    }

    sync_completion_wait(&completion, zx::duration::infinite().get());
    sync_completion_reset(&completion);
  }

  while (outstanding_op_count.load() > 0) {
  }
}

TEST_P(SdmmcBlockDeviceTest, RpmbRequestsGetToRun) {
  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    *reinterpret_cast<uint32_t*>(&out_data[212]) = htole32(FakeSdmmcDevice::kBlockCount);
    out_data[MMC_EXT_CSD_CACHE_CTRL] = 1;
    out_data[MMC_EXT_CSD_CACHE_SIZE_LSB] = 0x78;
    out_data[MMC_EXT_CSD_CACHE_SIZE_250] = 0x56;
    out_data[MMC_EXT_CSD_CACHE_SIZE_251] = 0x34;
    out_data[MMC_EXT_CSD_CACHE_SIZE_MSB] = 0x12;
    out_data[MMC_EXT_CSD_RPMB_SIZE_MULT] = 0x10;
    out_data[MMC_EXT_CSD_PARTITION_CONFIG] = 0xa8;
    out_data[MMC_EXT_CSD_PARTITION_SWITCH_TIME] = 0;
    out_data[MMC_EXT_CSD_BOOT_SIZE_MULT] = 0x10;
    out_data[MMC_EXT_CSD_GENERIC_CMD6_TIME] = 0;
  });

  ASSERT_OK(StartDriverForMmc());
  BindRpmbClient();

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> boot1_result =
      GetRemoteBlockDeviceForBlockServer("boot1");
  ASSERT_OK(boot1_result);

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> boot2_result =
      GetRemoteBlockDeviceForBlockServer("boot2");
  ASSERT_OK(boot2_result);

  thrd_t rpmb_thread;
  EXPECT_EQ(
      thrd_create_with_name(
          &rpmb_thread,
          [](void* ctx) -> int {
            auto test = reinterpret_cast<SdmmcBlockDeviceTest*>(ctx);

            zx::vmo vmo;
            if (zx::vmo::create(512, 0, &vmo) != ZX_OK) {
              return thrd_error;
            }

            std::atomic<uint32_t> ops_completed = 0;
            sync_completion_t completion;

            for (uint32_t i = 0; i < kMaxOutstandingOps; i++) {
              fuchsia_hardware_rpmb::wire::Request request = {};
              EXPECT_OK(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &request.tx_frames.vmo));
              request.tx_frames.offset = 0;
              request.tx_frames.size = 512;

              test->rpmb_client()
                  ->Request(std::move(request))
                  .ThenExactlyOnce(
                      [&](fidl::WireUnownedResult<fuchsia_hardware_rpmb::Rpmb::Request>& result) {
                        if (!result.ok()) {
                          FAIL("Request failed: %s", result.error().FormatDescription().c_str());
                          return;
                        }

                        EXPECT_FALSE(result->is_error());
                        if ((ops_completed.fetch_add(1) + 1) == kMaxOutstandingOps) {
                          sync_completion_signal(&completion);
                        }
                      });
            }

            sync_completion_wait(&completion, zx::duration::infinite().get());

            test->run_threads().store(false);

            return thrd_success;
          },
          this, "rpmb-queue-thread"),
      thrd_success);

  // Choose to run QueueBlockOps() using the foreground dispatcher, while
  // fuchsia_hardware_sdmmc::Sdmmc is being served by the background dispatcher.
  driver_test_.runtime().PerformBlockingWork([this] { QueueBlockOps(); });
  EXPECT_EQ(thrd_join(rpmb_thread, nullptr), thrd_success);
}

TEST_P(SdmmcBlockDeviceTest, BlockOpsGetToRun) {
  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    *reinterpret_cast<uint32_t*>(&out_data[212]) = htole32(FakeSdmmcDevice::kBlockCount);
    out_data[MMC_EXT_CSD_CACHE_CTRL] = 1;
    out_data[MMC_EXT_CSD_CACHE_SIZE_LSB] = 0x78;
    out_data[MMC_EXT_CSD_CACHE_SIZE_250] = 0x56;
    out_data[MMC_EXT_CSD_CACHE_SIZE_251] = 0x34;
    out_data[MMC_EXT_CSD_CACHE_SIZE_MSB] = 0x12;
    out_data[MMC_EXT_CSD_RPMB_SIZE_MULT] = 0x10;
    out_data[MMC_EXT_CSD_PARTITION_CONFIG] = 0xa8;
    out_data[MMC_EXT_CSD_PARTITION_SWITCH_TIME] = 0;
    out_data[MMC_EXT_CSD_BOOT_SIZE_MULT] = 0x10;
    out_data[MMC_EXT_CSD_GENERIC_CMD6_TIME] = 0;
  });

  ASSERT_OK(StartDriverForMmc());
  BindRpmbClient();

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> boot1_result =
      GetRemoteBlockDeviceForBlockServer("boot1");
  ASSERT_OK(boot1_result);

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> boot2_result =
      GetRemoteBlockDeviceForBlockServer("boot2");
  ASSERT_OK(boot2_result);

  thrd_t rpmb_thread;
  EXPECT_EQ(thrd_create_with_name(
                &rpmb_thread,
                [](void* ctx) -> int {
                  reinterpret_cast<SdmmcBlockDeviceTest*>(ctx)->QueueRpmbRequests();
                  return thrd_success;
                },
                this, "rpmb-queue-thread"),
            thrd_success);

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> user_result =
      GetRemoteBlockDeviceForBlockServer("user");
  ASSERT_OK(user_result);
  auto client = std::move(user_result.value());

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kMaxOutstandingOps * FakeSdmmcDevice::kBlockSize, 0, &vmo));
  storage::Vmoid owned_vmoid;
  EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));
  vmoid_t vmoid = owned_vmoid.TakeId();

  std::vector<BlockFifoRequest> requests;
  for (uint32_t i = 0; i < kMaxOutstandingOps; i++) {
    requests.push_back({
        .command = {.opcode = BLOCK_OPCODE_READ},
        .vmoid = vmoid,
        .length = 1,
        .vmo_offset = i,
        .dev_offset = i,
    });
  }

  driver_test_.runtime().PerformBlockingWork(
      [&] { EXPECT_OK(client->FifoTransaction(requests.data(), requests.size())); });

  run_threads_.store(false);
  EXPECT_EQ(thrd_join(rpmb_thread, nullptr), thrd_success);
}

TEST_P(SdmmcBlockDeviceTest, GetRpmbClient) {
  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    *reinterpret_cast<uint32_t*>(&out_data[212]) = htole32(FakeSdmmcDevice::kBlockCount);
    out_data[MMC_EXT_CSD_CACHE_CTRL] = 1;
    out_data[MMC_EXT_CSD_CACHE_SIZE_LSB] = 0x78;
    out_data[MMC_EXT_CSD_CACHE_SIZE_250] = 0x56;
    out_data[MMC_EXT_CSD_CACHE_SIZE_251] = 0x34;
    out_data[MMC_EXT_CSD_CACHE_SIZE_MSB] = 0x12;
    out_data[MMC_EXT_CSD_RPMB_SIZE_MULT] = 0x74;
    out_data[MMC_EXT_CSD_PARTITION_CONFIG] = 0xa8;
    out_data[MMC_EXT_CSD_REL_WR_SEC_C] = 1;
    out_data[MMC_EXT_CSD_PARTITION_SWITCH_TIME] = 0;
    out_data[MMC_EXT_CSD_BOOT_SIZE_MULT] = 0x10;
    out_data[MMC_EXT_CSD_GENERIC_CMD6_TIME] = 0;
  });

  ASSERT_OK(StartDriverForMmc());
  BindRpmbClient();

  sync_completion_t completion;
  rpmb_client_->GetDeviceInfo().ThenExactlyOnce(
      [&](fidl::WireUnownedResult<fuchsia_hardware_rpmb::Rpmb::GetDeviceInfo>& result) {
        if (!result.ok()) {
          FAIL("GetDeviceInfo failed: %s", result.error().FormatDescription().c_str());
          return;
        }
        auto* response = result.Unwrap();
        EXPECT_TRUE(response->info.is_emmc_info());
        EXPECT_EQ(response->info.emmc_info().rpmb_size, 0x74);
        EXPECT_EQ(response->info.emmc_info().reliable_write_sector_count, 1);
        sync_completion_signal(&completion);
      });

  sync_completion_wait(&completion, zx::duration::infinite().get());
  sync_completion_reset(&completion);
}

TEST_P(SdmmcBlockDeviceTest, Inspect) {
  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    SetDefaultMmcExtCsd(out_data);
    out_data[MMC_EXT_CSD_BARRIER_CTRL] = 1;
    out_data[MMC_EXT_CSD_CACHE_FLUSH_POLICY] = 1;
    out_data[MMC_EXT_CSD_DEVICE_LIFE_TIME_EST_TYP_A] = 3;
    out_data[MMC_EXT_CSD_DEVICE_LIFE_TIME_EST_TYP_B] = 7;
    out_data[MMC_EXT_CSD_BARRIER_SUPPORT] = 1;
    out_data[MMC_EXT_CSD_MAX_PACKED_WRITES] = 63;
    out_data[MMC_EXT_CSD_MAX_PACKED_READS] = 62;
  });

  ASSERT_OK(StartDriverForMmc());

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> client_result =
      GetRemoteBlockDeviceForBlockServer("user");
  ASSERT_OK(client_result);
  auto client = std::move(client_result.value());

  // IO error count should be zero after initialization.
  inspect::InspectTestHelper inspector;
  inspector.ReadInspect(block_device_->inspect());

  const inspect::Hierarchy* root = inspector.hierarchy().GetByPath({"sdmmc_core"});
  ASSERT_NOT_NULL(root);

  const auto* io_errors = root->node().get_property<inspect::UintPropertyValue>("io_errors");
  ASSERT_NOT_NULL(io_errors);
  EXPECT_EQ(io_errors->value(), 0);

  const auto* io_retries = root->node().get_property<inspect::UintPropertyValue>("io_retries");
  ASSERT_NOT_NULL(io_retries);
  EXPECT_EQ(io_retries->value(), 0);

  const auto* serial_number =
      root->node().get_property<inspect::UintPropertyValue>("serial_number");
  ASSERT_NOT_NULL(serial_number);
  EXPECT_EQ(serial_number->value(), 0);

  const auto* clock_rate = root->node().get_property<inspect::UintPropertyValue>("clock_rate");
  ASSERT_NOT_NULL(clock_rate);
  EXPECT_EQ(clock_rate->value(), 26'000'000);

  const auto* bus_width_bits =
      root->node().get_property<inspect::UintPropertyValue>("bus_width_bits");
  ASSERT_NOT_NULL(bus_width_bits);
  EXPECT_EQ(bus_width_bits->value(), 1);

  const auto* timing = root->node().get_property<inspect::StringPropertyValue>("timing");
  ASSERT_NOT_NULL(timing);
  EXPECT_EQ(timing->value(), "Legacy");

  const auto* type_a_lifetime =
      root->node().get_property<inspect::UintPropertyValue>("type_a_lifetime_used");
  ASSERT_NOT_NULL(type_a_lifetime);
  EXPECT_EQ(type_a_lifetime->value(), 3);

  const auto* type_b_lifetime =
      root->node().get_property<inspect::UintPropertyValue>("type_b_lifetime_used");
  ASSERT_NOT_NULL(type_b_lifetime);
  EXPECT_EQ(type_b_lifetime->value(), 7);

  const auto* max_lifetime =
      root->node().get_property<inspect::UintPropertyValue>("max_lifetime_used");
  ASSERT_NOT_NULL(max_lifetime);
  EXPECT_EQ(max_lifetime->value(), 7);

  const auto* cache_size = root->node().get_property<inspect::UintPropertyValue>("cache_size_bits");
  ASSERT_NOT_NULL(cache_size);
  EXPECT_EQ(cache_size->value(), 1024 * 0x12345678ull);

  const auto* cache_enabled =
      root->node().get_property<inspect::BoolPropertyValue>("cache_enabled");
  ASSERT_NOT_NULL(cache_enabled);
  EXPECT_TRUE(cache_enabled->value());

  const auto* cache_flush_fifo =
      root->node().get_property<inspect::BoolPropertyValue>("cache_flush_fifo");
  ASSERT_NOT_NULL(cache_flush_fifo);
  EXPECT_TRUE(cache_flush_fifo->value());

  const auto* barrier_supported =
      root->node().get_property<inspect::BoolPropertyValue>("barrier_supported");
  ASSERT_NOT_NULL(barrier_supported);
  EXPECT_TRUE(barrier_supported->value());

  const auto* max_packed_reads =
      root->node().get_property<inspect::UintPropertyValue>("max_packed_reads");
  ASSERT_NOT_NULL(max_packed_reads);
  EXPECT_EQ(max_packed_reads->value(), 62);

  const auto* max_packed_writes =
      root->node().get_property<inspect::UintPropertyValue>("max_packed_writes");
  ASSERT_NOT_NULL(max_packed_writes);
  EXPECT_EQ(max_packed_writes->value(), 63);

  const auto* max_packed_reads_effective =
      root->node().get_property<inspect::UintPropertyValue>("max_packed_reads_effective");
  ASSERT_NOT_NULL(max_packed_reads_effective);
  EXPECT_EQ(max_packed_reads_effective->value(), 16);

  const auto* max_packed_writes_effective =
      root->node().get_property<inspect::UintPropertyValue>("max_packed_writes_effective");
  ASSERT_NOT_NULL(max_packed_writes_effective);
  EXPECT_EQ(max_packed_writes_effective->value(), 16);

  // IO error count should be a successful block op.
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(5 * FakeSdmmcDevice::kBlockSize, 0, &vmo));
  storage::Vmoid owned_vmoid;
  EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));
  vmoid_t vmoid = owned_vmoid.TakeId();

  BlockFifoRequest req1 = {
      .command = {.opcode = BLOCK_OPCODE_WRITE},
      .vmoid = vmoid,
      .length = 5,
      .dev_offset = 0x8000,
  };
  EXPECT_OK(client->FifoTransaction(&req1, 1));

  inspector.ReadInspect(block_device_->inspect());

  root = inspector.hierarchy().GetByPath({"sdmmc_core"});
  ASSERT_NOT_NULL(root);

  io_errors = root->node().get_property<inspect::UintPropertyValue>("io_errors");
  ASSERT_NOT_NULL(io_errors);
  EXPECT_EQ(io_errors->value(), 0);

  io_retries = root->node().get_property<inspect::UintPropertyValue>("io_retries");
  ASSERT_NOT_NULL(io_retries);
  EXPECT_EQ(io_retries->value(), 0);

  // IO error count should be incremented after a failed block op.
  sdmmc_.set_command_callback(SDMMC_WRITE_MULTIPLE_BLOCK,
                              [](const sdmmc_req_t& req) -> zx_status_t { return ZX_ERR_IO; });

  BlockFifoRequest req2 = {
      .command = {.opcode = BLOCK_OPCODE_WRITE},
      .vmoid = vmoid,
      .length = 5,
      .dev_offset = 0x8000,
  };
  EXPECT_STATUS(client->FifoTransaction(&req2, 1), ZX_ERR_IO);

  inspector.ReadInspect(block_device_->inspect());

  root = inspector.hierarchy().GetByPath({"sdmmc_core"});
  ASSERT_NOT_NULL(root);

  io_errors = root->node().get_property<inspect::UintPropertyValue>("io_errors");
  ASSERT_NOT_NULL(io_errors);
  EXPECT_EQ(io_errors->value(), 1);

  io_retries = root->node().get_property<inspect::UintPropertyValue>("io_retries");
  ASSERT_NOT_NULL(io_retries);
  EXPECT_EQ(io_retries->value(), 9);
}

TEST_P(SdmmcBlockDeviceTest, InspectInvalidLifetime) {
  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    *reinterpret_cast<uint32_t*>(&out_data[212]) = htole32(FakeSdmmcDevice::kBlockCount);
    out_data[MMC_EXT_CSD_CACHE_CTRL] = 1;
    out_data[MMC_EXT_CSD_CACHE_SIZE_LSB] = 0x78;
    out_data[MMC_EXT_CSD_CACHE_SIZE_250] = 0x56;
    out_data[MMC_EXT_CSD_CACHE_SIZE_251] = 0x34;
    out_data[MMC_EXT_CSD_CACHE_SIZE_MSB] = 0x12;
    out_data[MMC_EXT_CSD_DEVICE_LIFE_TIME_EST_TYP_A] = 0xe;
    out_data[MMC_EXT_CSD_DEVICE_LIFE_TIME_EST_TYP_B] = 6;
  });

  ASSERT_OK(StartDriverForMmc());

  inspect::InspectTestHelper inspector;
  inspector.ReadInspect(block_device_->inspect());

  const inspect::Hierarchy* root = inspector.hierarchy().GetByPath({"sdmmc_core"});
  ASSERT_NOT_NULL(root);

  const auto* type_a_lifetime =
      root->node().get_property<inspect::UintPropertyValue>("type_a_lifetime_used");
  ASSERT_NOT_NULL(type_a_lifetime);
  EXPECT_EQ(type_a_lifetime->value(), 0xc);  // Value out of range should be normalized.

  const auto* type_b_lifetime =
      root->node().get_property<inspect::UintPropertyValue>("type_b_lifetime_used");
  ASSERT_NOT_NULL(type_b_lifetime);
  EXPECT_EQ(type_b_lifetime->value(), 6);

  const auto* max_lifetime =
      root->node().get_property<inspect::UintPropertyValue>("max_lifetime_used");
  ASSERT_NOT_NULL(max_lifetime);
  EXPECT_EQ(max_lifetime->value(), 6);  // Only the valid value should be used.
}

TEST_P(SdmmcBlockDeviceTest, InspectSerialNumber) {
  constexpr uint32_t kExpectedSerial = 0x12345678;
  sdmmc_.set_command_callback(SDMMC_ALL_SEND_CID, [kExpectedSerial](uint32_t out_response[4]) {
    uint8_t* raw_cid = reinterpret_cast<uint8_t*>(out_response);
    memcpy(&raw_cid[MMC_CID_SERIAL], &kExpectedSerial, sizeof(kExpectedSerial));
  });

  ASSERT_OK(StartDriverForMmc());

  inspect::InspectTestHelper inspector;
  inspector.ReadInspect(block_device_->inspect());

  const inspect::Hierarchy* root = inspector.hierarchy().GetByPath({"sdmmc_core"});
  ASSERT_NOT_NULL(root);

  const auto* serial_number =
      root->node().get_property<inspect::UintPropertyValue>("serial_number");
  ASSERT_NOT_NULL(serial_number);
  EXPECT_EQ(serial_number->value(), kExpectedSerial);
}

TEST_P(SdmmcBlockDeviceTest, PowerSuspendResume) {
  bool in_sleep_state = false;
  sdmmc_.set_command_callback(MMC_SLEEP_AWAKE,
                              [&](const sdmmc_req_t& req, uint32_t out_response[4]) {
                                in_sleep_state = (req.arg >> 15) & 0x1;
                                if (in_sleep_state) {
                                  out_response[0] |= MMC_STATUS_CURRENT_STATE_STBY;
                                } else {
                                  out_response[0] |= MMC_STATUS_CURRENT_STATE_SLP;
                                }
                              });

  ASSERT_OK(StartDriverForMmc(/*speed_capabilities=*/{}, /*supply_power_framework=*/true));
  bool dependency_added = driver_test_.RunInEnvironmentTypeContext<bool>([](TestEnvironment& env) {
    return env.cpu_element_manager.execution_state_dependency_added();
  });
  EXPECT_TRUE(dependency_added);

  inspect::InspectTestHelper inspector;
  inspector.ReadInspect(block_device_->inspect());

  const inspect::Hierarchy* root = inspector.hierarchy().GetByPath({"sdmmc_core"});
  ASSERT_NOT_NULL(root);

  const auto* power_suspended =
      root->node().get_property<inspect::BoolPropertyValue>("power_suspended");
  ASSERT_NOT_NULL(power_suspended);
  EXPECT_FALSE(power_suspended->value());
  EXPECT_FALSE(in_sleep_state);

  libsync::Completion set_level_complete;
  // Trigger power level change to kPowerLevelOff.
  driver_test_.RunInEnvironmentTypeContext([&](TestEnvironment& env) {
    env.power_broker.hardware_power_element_runner_client_
        ->SetLevel(SdmmcBlockDevice::kPowerLevelOff)
        .ThenExactlyOnce([&](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
          EXPECT_TRUE(result.is_ok());
          set_level_complete.Signal();
        });
  });
  driver_test_.runtime().PerformBlockingWork([&] { set_level_complete.Wait(); });

  inspector.ReadInspect(block_device_->inspect());

  root = inspector.hierarchy().GetByPath({"sdmmc_core"});
  ASSERT_NOT_NULL(root);

  // The transition should be ignored, and the device should be left on.
  power_suspended = root->node().get_property<inspect::BoolPropertyValue>("power_suspended");
  ASSERT_NOT_NULL(power_suspended);
  EXPECT_FALSE(power_suspended->value());
  EXPECT_FALSE(in_sleep_state);

  set_level_complete.Reset();
  // Trigger power level change to kPowerLevelOn.

  driver_test_.RunInEnvironmentTypeContext([&](TestEnvironment& env) {
    env.power_broker.hardware_power_element_runner_client_
        ->SetLevel(SdmmcBlockDevice::kPowerLevelOn)
        .ThenExactlyOnce([&](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
          EXPECT_TRUE(result.is_ok());
          set_level_complete.Signal();
        });
  });
  driver_test_.runtime().PerformBlockingWork([&] { set_level_complete.Wait(); });

  inspector.ReadInspect(block_device_->inspect());

  root = inspector.hierarchy().GetByPath({"sdmmc_core"});
  ASSERT_NOT_NULL(root);

  power_suspended = root->node().get_property<inspect::BoolPropertyValue>("power_suspended");
  ASSERT_NOT_NULL(power_suspended);
  EXPECT_FALSE(power_suspended->value());
  EXPECT_FALSE(in_sleep_state);

  set_level_complete.Reset();
  // Trigger power level change to kPowerLevelOn. This should be a no-op.

  driver_test_.RunInEnvironmentTypeContext([&](TestEnvironment& env) {
    env.power_broker.hardware_power_element_runner_client_
        ->SetLevel(SdmmcBlockDevice::kPowerLevelOn)
        .ThenExactlyOnce([&](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
          EXPECT_TRUE(result.is_ok());
          set_level_complete.Signal();
        });
  });

  // Wait until the level has been set.
  driver_test_.runtime().PerformBlockingWork([&] { set_level_complete.Wait(); });

  inspector.ReadInspect(block_device_->inspect());

  root = inspector.hierarchy().GetByPath({"sdmmc_core"});
  ASSERT_NOT_NULL(root);

  power_suspended = root->node().get_property<inspect::BoolPropertyValue>("power_suspended");
  ASSERT_NOT_NULL(power_suspended);
  EXPECT_FALSE(power_suspended->value());
  EXPECT_FALSE(in_sleep_state);

  set_level_complete.Reset();
  // Trigger power level change to kPowerLevelOff. This time the transition should be respected, and
  // the device should be put to sleep.
  driver_test_.RunInEnvironmentTypeContext([&](TestEnvironment& env) {
    env.power_broker.hardware_power_element_runner_client_
        ->SetLevel(SdmmcBlockDevice::kPowerLevelOff)
        .ThenExactlyOnce([&](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
          EXPECT_TRUE(result.is_ok());
          set_level_complete.Signal();
        });
  });
  driver_test_.runtime().PerformBlockingWork([&] { set_level_complete.Wait(); });

  inspector.ReadInspect(block_device_->inspect());

  root = inspector.hierarchy().GetByPath({"sdmmc_core"});
  ASSERT_NOT_NULL(root);

  power_suspended = root->node().get_property<inspect::BoolPropertyValue>("power_suspended");
  ASSERT_NOT_NULL(power_suspended);
  EXPECT_TRUE(power_suspended->value());
  EXPECT_TRUE(in_sleep_state);

  set_level_complete.Reset();
  // Trigger power level change back to kPowerLevelOn and wait for the device to be woken up.
  driver_test_.RunInEnvironmentTypeContext([&](TestEnvironment& env) {
    env.power_broker.hardware_power_element_runner_client_
        ->SetLevel(SdmmcBlockDevice::kPowerLevelOn)
        .ThenExactlyOnce([&](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
          EXPECT_TRUE(result.is_ok());
          set_level_complete.Signal();
        });
  });
  driver_test_.runtime().PerformBlockingWork([&] { set_level_complete.Wait(); });

  inspector.ReadInspect(block_device_->inspect());

  root = inspector.hierarchy().GetByPath({"sdmmc_core"});
  ASSERT_NOT_NULL(root);

  power_suspended = root->node().get_property<inspect::BoolPropertyValue>("power_suspended");
  ASSERT_NOT_NULL(power_suspended);
  EXPECT_FALSE(power_suspended->value());
  EXPECT_FALSE(in_sleep_state);
}

TEST_P(SdmmcBlockDeviceTest, PowerOffNotification) {
  uint8_t power_off_notification = 0;
  sdmmc_.set_command_callback(MMC_SWITCH, [&](const sdmmc_req_t& req, uint32_t out_response[4]) {
    const uint8_t index = (req.arg >> 16) & 0xff;
    if (index == MMC_EXT_CSD_POWER_OFF_NOTIFICATION) {
      power_off_notification = (req.arg >> 8) & 0xff;
    }
  });
  bool in_sleep_state = false;
  sdmmc_.set_command_callback(MMC_SLEEP_AWAKE,
                              [&](const sdmmc_req_t& req, uint32_t out_response[4]) {
                                in_sleep_state = (req.arg >> 15) & 0x1;
                                if (in_sleep_state) {
                                  out_response[0] |= MMC_STATUS_CURRENT_STATE_STBY;
                                } else {
                                  out_response[0] |= MMC_STATUS_CURRENT_STATE_SLP;
                                }
                              });

  ASSERT_OK(StartDriverForMmc(/*speed_capabilities=*/{}, /*supply_power_framework=*/true));

  // POWERED_ON should be set by default after probe.
  EXPECT_EQ(power_off_notification, MMC_EXT_CSD_POWERED_ON);

  EXPECT_TRUE(driver_test_.RunInEnvironmentTypeContext<bool>([](TestEnvironment& env) {
    return env.cpu_element_manager.execution_state_dependency_added();
  }));

  libsync::Completion set_level_complete;
  // Move to the ON state so that the transition to OFF can be made after.
  driver_test_.RunInEnvironmentTypeContext([&](TestEnvironment& env) {
    env.power_broker.hardware_power_element_runner_client_
        ->SetLevel(SdmmcBlockDevice::kPowerLevelOn)
        .ThenExactlyOnce([&](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
          EXPECT_TRUE(result.is_ok());
          set_level_complete.Signal();
        });
  });

  driver_test_.runtime().PerformBlockingWork([&] { set_level_complete.Wait(); });

  EXPECT_FALSE(in_sleep_state);

  libsync::Completion sleep_complete;
  // Transition to off, then Call PrepareStop().
  driver_test_.RunInEnvironmentTypeContext([&](TestEnvironment& env) {
    env.power_broker.hardware_power_element_runner_client_
        ->SetLevel(SdmmcBlockDevice::kPowerLevelOff)
        .ThenExactlyOnce([&](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
          EXPECT_TRUE(result.is_ok());
          sleep_complete.Signal();
        });
  });
  driver_test_.runtime().PerformBlockingWork([&] { sleep_complete.Wait(); });
  EXPECT_TRUE(in_sleep_state);

  EXPECT_OK(driver_test_.StopDriver());
  dut_ = nullptr;

  // The device should have been moved back to TRAN, and a power off notification should have been
  // sent.
  EXPECT_FALSE(in_sleep_state);
  EXPECT_EQ(power_off_notification, MMC_EXT_CSD_POWER_OFF_LONG);

  block_device_ = nullptr;
}

TEST_P(SdmmcBlockDeviceTest, BlockServer) {
  ASSERT_OK(StartDriverForMmc(/*speed_capabilities=*/{}, /*supply_power_framework=*/false));

  const char* instance_name = "user";
  auto test_fn = [&] {
    auto client = GetRemoteBlockDeviceForBlockServer(instance_name);
    ASSERT_OK(client);

    fuchsia_storage_block::wire::BlockInfo info;
    EXPECT_OK(client->BlockGetInfo(&info));

    const int len = 2 * info.block_size;

    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(len * 2, 0, &vmo));

    storage::Vmoid owned_vmoid;
    EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));

    // It doesn't matter if we leak the ID.
    vmoid_t vmoid = owned_vmoid.TakeId();

    auto buffer = std::make_unique<uint8_t[]>(len);
    uint8_t c = 0;
    for (int i = 0; i < len; ++i) {
      buffer[i] = c;
      c += 7;
    }

    EXPECT_OK(vmo.write(buffer.get(), 0, len));

    BlockFifoRequest requests[] = {
        {
            .command =
                {
                    .opcode = BLOCK_OPCODE_WRITE,
                },
            .vmoid = vmoid,
            .length = 1,
            .vmo_offset = 0,
            .dev_offset = 0,
        },
        {
            .command =
                {
                    .opcode = BLOCK_OPCODE_WRITE,
                },
            .vmoid = vmoid,
            .length = 1,
            .vmo_offset = 1,
            .dev_offset = 1,
        },
        {
            .command =
                {
                    .opcode = BLOCK_OPCODE_TRIM,
                },
            .length = 1,
            .dev_offset = 2,
        },
        {
            .command =
                {
                    .opcode = BLOCK_OPCODE_FLUSH,
                },
        },
    };

    EXPECT_OK(client->FifoTransaction(requests, 4));

    requests[0].command.opcode = BLOCK_OPCODE_READ;
    requests[0].vmo_offset += 2;
    requests[1].command.opcode = BLOCK_OPCODE_READ;
    requests[1].vmo_offset += 2;

    EXPECT_OK(client->FifoTransaction(requests, 4));

    auto read_buffer = std::make_unique<uint8_t[]>(len);
    EXPECT_OK(vmo.read(read_buffer.get(), len, len));

    EXPECT_BYTES_EQ(read_buffer.get(), buffer.get(), len);

    BlockFifoRequest bad_request{
        .command =
            {
                .opcode = BLOCK_OPCODE_WRITE,
            },
        .vmoid = vmoid,
        .length = 1,
        .vmo_offset = 1,
        .dev_offset = info.block_count,
    };
    EXPECT_STATUS(client->FifoTransaction(&bad_request, 1), ZX_ERR_OUT_OF_RANGE);
    bad_request.command.opcode = BLOCK_OPCODE_READ;
    EXPECT_STATUS(client->FifoTransaction(&bad_request, 1), ZX_ERR_OUT_OF_RANGE);
    bad_request.command.opcode = BLOCK_OPCODE_TRIM;
    EXPECT_STATUS(client->FifoTransaction(&bad_request, 1), ZX_ERR_OUT_OF_RANGE);
  };
  driver_test_.runtime().PerformBlockingWork(test_fn);
  instance_name = "boot1";
  driver_test_.runtime().PerformBlockingWork(test_fn);
  instance_name = "boot2";
  driver_test_.runtime().PerformBlockingWork(test_fn);
}

TEST_P(SdmmcBlockDeviceTest, TeardownWithActiveClient) {
  ASSERT_OK(StartDriverForMmc());

  const char* instance_name = "user";

  sync_completion_t completion;
  std::unique_ptr<block_client::RemoteBlockDevice> remote_device;
  std::unique_ptr<block_client::ReaderWriter> client;
  driver_test_.runtime().PerformBlockingWork([&] {
    zx::result remote = GetRemoteBlockDeviceForBlockServer(instance_name);
    ASSERT_OK(remote);
    remote_device = std::move(*remote);
    client = std::make_unique<block_client::ReaderWriter>(*remote_device);
    sync_completion_signal(&completion);
  });
  sync_completion_wait(&completion, ZX_TIME_INFINITE);
  std::atomic<bool> stopped = false;
  std::thread t([&]() {
    while (!stopped) {
      uint8_t buffer[FakeSdmmcDevice::kBlockSize];
      [[maybe_unused]] zx_status_t status = client->Read(0, sizeof(buffer), buffer);
    }
  });

  EXPECT_OK(driver_test_.StopDriver());
  dut_ = nullptr;
  stopped = true;
  t.join();
}

TEST_P(SdmmcBlockDeviceTest, BlockServerMaxTransferSize) {
  constexpr int kMaxTransferSize = 16384;

  sdmmc_.set_host_info({
      .caps = 0,
      .max_transfer_size = kMaxTransferSize,
  });

  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    *reinterpret_cast<uint32_t*>(&out_data[212]) = htole32(FakeSdmmcDevice::kBlockCount);
    out_data[MMC_EXT_CSD_CACHE_CTRL] = 1;
    out_data[MMC_EXT_CSD_CACHE_SIZE_LSB] = 0x78;
    out_data[MMC_EXT_CSD_CACHE_SIZE_250] = 0x56;
    out_data[MMC_EXT_CSD_CACHE_SIZE_251] = 0x34;
    out_data[MMC_EXT_CSD_CACHE_SIZE_MSB] = 0x12;
    out_data[MMC_EXT_CSD_PARTITION_CONFIG] = 0xa8;
    out_data[MMC_EXT_CSD_SEC_FEATURE_SUPPORT] = 0x1 << MMC_EXT_CSD_SEC_FEATURE_SUPPORT_SEC_GB_CL_EN;
    out_data[MMC_EXT_CSD_PARTITION_SWITCH_TIME] = 0;
    out_data[MMC_EXT_CSD_BOOT_SIZE_MULT] = 0x10;
    out_data[MMC_EXT_CSD_GENERIC_CMD6_TIME] = 0;
    // Enabled packed commands, even though they aren't used in this test case.
    out_data[MMC_EXT_CSD_MAX_PACKED_WRITES] = 63;
    out_data[MMC_EXT_CSD_MAX_PACKED_READS] = 63;
  });

  ASSERT_OK(StartDriverForMmc(/*speed_capabilities=*/{}, /*supply_power_framework=*/false));

  driver_test_.runtime().PerformBlockingWork([&] {
    auto client = GetRemoteBlockDeviceForBlockServer("user");
    ASSERT_OK(client);

    fuchsia_storage_block::wire::BlockInfo info;
    EXPECT_OK(client->BlockGetInfo(&info));

    ASSERT_EQ(kMaxTransferSize % info.block_size, 0);
    const uint32_t max_transfer_size_in_blocks = kMaxTransferSize / info.block_size;
    ASSERT_GT(max_transfer_size_in_blocks, 1);

    const int len = 2 * kMaxTransferSize;

    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(len * 2, 0, &vmo));

    storage::Vmoid owned_vmoid;
    EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));

    // It doesn't matter if we leak the ID.
    vmoid_t vmoid = owned_vmoid.TakeId();

    auto buffer = std::make_unique<uint8_t[]>(len);
    uint8_t c = 0;
    for (int i = 0; i < len / 2; ++i) {
      buffer[i] = c;
      c += 7;
    }

    EXPECT_OK(vmo.write(buffer.get(), 0, len));

    const uint32_t blocks1 = max_transfer_size_in_blocks - 1;
    const uint32_t blocks2 = 2 * max_transfer_size_in_blocks - blocks1;

    BlockFifoRequest requests[] = {{
                                       .command =
                                           {
                                               .opcode = BLOCK_OPCODE_WRITE,
                                           },
                                       .vmoid = vmoid,
                                       .length = blocks1,
                                       .vmo_offset = 0,
                                       .dev_offset = 0,
                                   },
                                   {
                                       .command =
                                           {
                                               .opcode = BLOCK_OPCODE_WRITE,
                                           },
                                       .vmoid = vmoid,
                                       .length = blocks2,
                                       .vmo_offset = blocks1,
                                       .dev_offset = blocks1,
                                   }};

    EXPECT_OK(client->FifoTransaction(requests, 2));

    // These requests should have resulted in two (non-packed) writes starting at address zero.
    std::vector<uint8_t> write_data = sdmmc_.Read(0, len);
    EXPECT_BYTES_EQ(write_data.data(), buffer.get(), len);

    requests[0].command.opcode = BLOCK_OPCODE_READ;
    requests[0].vmo_offset += max_transfer_size_in_blocks * 2;
    requests[1].command.opcode = BLOCK_OPCODE_READ;
    requests[1].vmo_offset += max_transfer_size_in_blocks * 2;

    EXPECT_OK(client->FifoTransaction(requests, 2));

    auto read_buffer = std::make_unique<uint8_t[]>(len);
    EXPECT_OK(vmo.read(read_buffer.get(), len, len));

    EXPECT_BYTES_EQ(read_buffer.get(), buffer.get(), len);
  });
}

TEST_P(SdmmcBlockDeviceTest, BlockServerSplitTransfer) {
  constexpr int kMaxTransferSize = 16384;

  sdmmc_.set_host_info({
      .caps = 0,
      .max_transfer_size = kMaxTransferSize,
  });

  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    *reinterpret_cast<uint32_t*>(&out_data[212]) = htole32(FakeSdmmcDevice::kBlockCount);
    out_data[MMC_EXT_CSD_CACHE_CTRL] = 1;
    out_data[MMC_EXT_CSD_CACHE_SIZE_LSB] = 0x78;
    out_data[MMC_EXT_CSD_CACHE_SIZE_250] = 0x56;
    out_data[MMC_EXT_CSD_CACHE_SIZE_251] = 0x34;
    out_data[MMC_EXT_CSD_CACHE_SIZE_MSB] = 0x12;
    out_data[MMC_EXT_CSD_PARTITION_CONFIG] = 0xa8;
    out_data[MMC_EXT_CSD_SEC_FEATURE_SUPPORT] = 0x1 << MMC_EXT_CSD_SEC_FEATURE_SUPPORT_SEC_GB_CL_EN;
    out_data[MMC_EXT_CSD_PARTITION_SWITCH_TIME] = 0;
    out_data[MMC_EXT_CSD_BOOT_SIZE_MULT] = 0x10;
    out_data[MMC_EXT_CSD_GENERIC_CMD6_TIME] = 0;
    out_data[MMC_EXT_CSD_MAX_PACKED_WRITES] = 63;
    out_data[MMC_EXT_CSD_MAX_PACKED_READS] = 63;
  });

  ASSERT_OK(StartDriverForMmc(/*speed_capabilities=*/{}, /*supply_power_framework=*/false));

  driver_test_.runtime().PerformBlockingWork([&] {
    auto client = GetRemoteBlockDeviceForBlockServer("user");
    ASSERT_OK(client);

    fuchsia_storage_block::wire::BlockInfo info;
    EXPECT_OK(client->BlockGetInfo(&info));

    ASSERT_EQ(kMaxTransferSize % info.block_size, 0);
    const uint32_t max_transfer_size_in_blocks = kMaxTransferSize / info.block_size;
    ASSERT_GT(max_transfer_size_in_blocks, 1);

    const int len = 2 * kMaxTransferSize;

    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(len * 2, 0, &vmo));

    storage::Vmoid owned_vmoid;
    EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));

    // It doesn't matter if we leak the ID.
    vmoid_t vmoid = owned_vmoid.TakeId();

    auto buffer = std::make_unique<uint8_t[]>(len);
    uint8_t c = 0;
    for (int i = 0; i < len / 2; ++i) {
      buffer[i] = c;
      c += 7;
    }

    EXPECT_OK(vmo.write(buffer.get(), 0, len));

    const uint32_t blocks1 = max_transfer_size_in_blocks - 10;
    const uint32_t blocks2 = 2 * max_transfer_size_in_blocks - blocks1;

    BlockFifoRequest requests[] = {{
                                       .command =
                                           {
                                               .opcode = BLOCK_OPCODE_WRITE,
                                           },
                                       .vmoid = vmoid,
                                       .length = blocks1,
                                       .vmo_offset = 0,
                                       .dev_offset = 0,
                                   },
                                   {
                                       .command =
                                           {
                                               .opcode = BLOCK_OPCODE_WRITE,
                                           },
                                       .vmoid = vmoid,
                                       .length = blocks2,
                                       .vmo_offset = blocks1,
                                       .dev_offset = blocks1,
                                   }};

    EXPECT_OK(client->FifoTransaction(requests, 2));

    // The above requests should be split by block server into three writes:
    //   1. Write blocks [0, 21]
    //   2. Write blocks [22, 53]
    //   3. Write blocks [54, 63]
    //
    // This will result in three SDMMC requests:
    //   1. Packed write of [0, 21] and [22, 23]
    //   2. Non-packed write of [24, 53]
    //   3. Non-packed write of [54, 63]
    //
    // The packed command header will be written to block 0, and block data intended for [0, 23]
    // will follow it in blocks [1, 24]. Write #2 will then overwrite block 24. To account for this,
    // skip the packed command header when reading from the fake device.
    std::vector<uint8_t> write_data1 = sdmmc_.Read(info.block_size, 23 * info.block_size);
    EXPECT_BYTES_EQ(write_data1.data(), buffer.get(), write_data1.size());

    // Check the last two writes.
    std::vector<uint8_t> write_data2 = sdmmc_.Read(24 * info.block_size, 40 * info.block_size);
    EXPECT_BYTES_EQ(write_data2.data(), buffer.get() + len - write_data2.size(),
                    write_data2.size());

    requests[0].command.opcode = BLOCK_OPCODE_READ;
    requests[0].vmo_offset += max_transfer_size_in_blocks * 2;
    requests[1].command.opcode = BLOCK_OPCODE_READ;
    requests[1].vmo_offset += max_transfer_size_in_blocks * 2;

    EXPECT_OK(client->FifoTransaction(requests, 2));

    auto read_buffer = std::make_unique<uint8_t[]>(len);
    EXPECT_OK(vmo.read(read_buffer.get(), len, len));

    // Similarly, skip the packed command header when checking read data.
    std::span<uint8_t> read_data1{read_buffer.get() + info.block_size, 23 * info.block_size};
    EXPECT_BYTES_EQ(read_data1.data(), buffer.get(), read_data1.size());

    std::span<uint8_t> read_data2{read_buffer.get() + (24 * info.block_size), 40 * info.block_size};
    EXPECT_BYTES_EQ(read_data2.data(), buffer.get() + len - read_data2.size(), read_data2.size());
  });
}

TEST_P(SdmmcBlockDeviceTest, PackedCommandWriteError) {
  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    SetDefaultMmcExtCsd(out_data);
    out_data[MMC_EXT_CSD_MAX_PACKED_WRITES] = 63;
    out_data[MMC_EXT_CSD_MAX_PACKED_READS] = 63;
  });

  ASSERT_OK(StartDriverForMmc(/*speed_capabilities=*/{}, /*supply_power_framework=*/false));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(1024, 0, &vmo));

  driver_test_.runtime().PerformBlockingWork([&] {
    auto client = GetRemoteBlockDeviceForBlockServer("user");
    ASSERT_OK(client);

    storage::Vmoid owned_vmoid;
    EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));
    vmoid_t vmoid = owned_vmoid.TakeId();

    BlockFifoRequest requests[] = {
        {
            .command = {.opcode = BLOCK_OPCODE_WRITE},
            .vmoid = vmoid,
            .length = 1,
            .vmo_offset = 0,
            .dev_offset = 0,
        },
        {
            .command = {.opcode = BLOCK_OPCODE_WRITE},
            .vmoid = vmoid,
            .length = 1,
            .vmo_offset = 1,
            .dev_offset = 1,
        },
    };

    EXPECT_OK(client->FifoTransaction(requests, 2));
  });

  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    *reinterpret_cast<uint32_t*>(&out_data[212]) = htole32(FakeSdmmcDevice::kBlockCount);
    out_data[MMC_EXT_CSD_MAX_PACKED_WRITES] = 63;
    out_data[MMC_EXT_CSD_MAX_PACKED_READS] = 63;
    // Return an error for packed commands.
    out_data[MMC_EXT_CSD_PACKED_COMMAND_STATUS] = 1;
  });

  driver_test_.runtime().PerformBlockingWork([&] {
    auto client = GetRemoteBlockDeviceForBlockServer("user");
    ASSERT_OK(client);

    storage::Vmoid owned_vmoid;
    EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));
    vmoid_t vmoid = owned_vmoid.TakeId();

    BlockFifoRequest requests[] = {
        {
            .command = {.opcode = BLOCK_OPCODE_WRITE},
            .vmoid = vmoid,
            .length = 1,
            .vmo_offset = 0,
            .dev_offset = 0,
        },
        {
            .command = {.opcode = BLOCK_OPCODE_WRITE},
            .vmoid = vmoid,
            .length = 1,
            .vmo_offset = 1,
            .dev_offset = 1,
        },
    };

    // The same packed command should now fail.
    EXPECT_EQ(client->FifoTransaction(requests, 2), ZX_ERR_IO);
  });
}

TEST_P(SdmmcBlockDeviceTest, PackedCommandReadError) {
  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    SetDefaultMmcExtCsd(out_data);
    out_data[MMC_EXT_CSD_MAX_PACKED_WRITES] = 63;
    out_data[MMC_EXT_CSD_MAX_PACKED_READS] = 63;
  });

  ASSERT_OK(StartDriverForMmc(/*speed_capabilities=*/{}, /*supply_power_framework=*/false));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(1024, 0, &vmo));

  driver_test_.runtime().PerformBlockingWork([&] {
    auto client = GetRemoteBlockDeviceForBlockServer("user");
    ASSERT_OK(client);

    storage::Vmoid owned_vmoid;
    EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));
    vmoid_t vmoid = owned_vmoid.TakeId();

    BlockFifoRequest requests[] = {
        {
            .command = {.opcode = BLOCK_OPCODE_READ},
            .vmoid = vmoid,
            .length = 1,
            .vmo_offset = 0,
            .dev_offset = 0,
        },
        {
            .command = {.opcode = BLOCK_OPCODE_READ},
            .vmoid = vmoid,
            .length = 1,
            .vmo_offset = 1,
            .dev_offset = 1,
        },
    };

    EXPECT_OK(client->FifoTransaction(requests, 2));
  });

  sdmmc_.set_command_callback(MMC_SEND_EXT_CSD, [](cpp20::span<uint8_t> out_data) {
    SetDefaultMmcExtCsd(out_data);
    out_data[MMC_EXT_CSD_MAX_PACKED_WRITES] = 63;
    out_data[MMC_EXT_CSD_MAX_PACKED_READS] = 63;
    // Return an error for packed commands.
    out_data[MMC_EXT_CSD_PACKED_COMMAND_STATUS] = 1;
  });

  driver_test_.runtime().PerformBlockingWork([&] {
    auto client = GetRemoteBlockDeviceForBlockServer("user");
    ASSERT_OK(client);

    storage::Vmoid owned_vmoid;
    EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));
    vmoid_t vmoid = owned_vmoid.TakeId();

    BlockFifoRequest requests[] = {
        {
            .command = {.opcode = BLOCK_OPCODE_READ},
            .vmoid = vmoid,
            .length = 1,
            .vmo_offset = 0,
            .dev_offset = 0,
        },
        {
            .command = {.opcode = BLOCK_OPCODE_READ},
            .vmoid = vmoid,
            .length = 1,
            .vmo_offset = 1,
            .dev_offset = 1,
        },
    };

    // The same packed command should now fail.
    EXPECT_EQ(client->FifoTransaction(requests, 2), ZX_ERR_IO);
  });
}

TEST_P(SdmmcBlockDeviceTest, NodeToken) {
  ASSERT_OK(StartDriverForMmc());

  zx::result connect_result =
      driver_test_.Connect<fuchsia_hardware_block_volume::Service::Token>("user");
  ASSERT_OK(connect_result);

  fidl::SyncClient<fuchsia_driver_token::NodeToken> client(std::move(connect_result.value()));
  auto get_result = client->Get();
  ASSERT_TRUE(get_result.is_ok());

  zx_info_handle_basic_t info1, info2;
  ASSERT_EQ(node_token_.get_info(ZX_INFO_HANDLE_BASIC, &info1, sizeof(info1), nullptr, nullptr),
            ZX_OK);
  ASSERT_EQ(
      get_result->token().get_info(ZX_INFO_HANDLE_BASIC, &info2, sizeof(info2), nullptr, nullptr),
      ZX_OK);
  ASSERT_EQ(info1.koid, info2.koid);
}

TEST_P(SdmmcBlockDeviceTest, InlineCryptoDunOutOfRange) {
  ASSERT_OK(StartDriverForMmc());

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(FakeSdmmcDevice::kBlockSize, 0, &vmo));

  driver_test_.runtime().PerformBlockingWork([&] {
    auto client = GetRemoteBlockDeviceForBlockServer("user");
    ASSERT_OK(client);

    storage::Vmoid owned_vmoid;
    EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));
    vmoid_t vmoid = owned_vmoid.TakeId();

    BlockFifoRequest req = {
        .command = {.opcode = BLOCK_OPCODE_READ, .flags = BLOCK_IO_FLAG_INLINE_ENCRYPTION_ENABLED},
        .vmoid = vmoid,
        .length = 1,
        .vmo_offset = 0,
        .dev_offset = 0,
        .dun = uint64_t{1} << 32,
        .slot = 1,
    };
    EXPECT_EQ(client->FifoTransaction(&req, 1), ZX_ERR_OUT_OF_RANGE);
  });
}

INSTANTIATE_TEST_SUITE_P(SdmmcProtocolUsingFidlTest, SdmmcBlockDeviceTest, zxtest::Bool());

}  // namespace sdmmc

FUCHSIA_DRIVER_EXPORT2(sdmmc::TestSdmmcRootDevice);
