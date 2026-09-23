// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-sdmmc-with-banjo.h"

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.device/cpp/fidl.h>
#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <fidl/fuchsia.power.system/cpp/fidl.h>
#include <fidl/fuchsia.power.system/cpp/test_base.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/fake-bti/cpp/fake-bti.h>
#include <lib/driver/fake-clock/cpp/fake-clock.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/driver/mmio/testing/cpp/test-helper.h>
#include <lib/driver/power/cpp/testing/fake_element_control.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/driver/testing/cpp/test_node.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/inspect/testing/cpp/zxtest/inspect.h>
#include <lib/mmio-ptr/fake.h>
#include <lib/sdio/hw.h>
#include <lib/sdmmc/hw.h>
#include <threads.h>
#include <zircon/types.h>

#include <memory>
#include <vector>

#include <soc/aml-s912/s912-hw.h>
#include <zxtest/zxtest.h>

#include "aml-sdmmc-regs.h"

namespace aml_sdmmc {

using fdf_power::testing::FakeElementControl;

class TestAmlSdmmcWithBanjo : public AmlSdmmcWithBanjo {
 public:
  static constexpr char kInstance[] = "-mmc@ff000000";

  explicit TestAmlSdmmcWithBanjo() : AmlSdmmcWithBanjo() {}

  void* SetTestHooks() {
    view_.emplace(mmio().View(0));
    return descs_buffer();
  }

  const inspect::Hierarchy* GetInspectRoot(const std::string& suffix) {
    inspector_.ReadInspect(component_inspector_->inspector());
    return inspector_.hierarchy().GetByPath({"aml-sdmmc-port" + suffix});
  }

  void ExpectInspectBoolPropertyValue(const std::string& name, bool value) {
    const auto* root = GetInspectRoot(kInstance);
    ASSERT_NOT_NULL(root);

    const auto* property = root->node().get_property<inspect::BoolPropertyValue>(name);
    ASSERT_NOT_NULL(property);
    EXPECT_EQ(property->value(), value);
  }

  void ExpectInspectPropertyValue(const std::string& name, uint64_t value) {
    const auto* root = GetInspectRoot(kInstance);
    ASSERT_NOT_NULL(root);

    const auto* property = root->node().get_property<inspect::UintPropertyValue>(name);
    ASSERT_NOT_NULL(property);
    EXPECT_EQ(property->value(), value);
  }

  void ExpectInspectPropertyValue(const std::string& path, const std::string& name,
                                  std::string_view value) {
    const auto* root = GetInspectRoot(kInstance);
    ASSERT_NOT_NULL(root);

    const auto* property =
        root->GetByPath({path})->node().get_property<inspect::StringPropertyValue>(name);
    ASSERT_NOT_NULL(property);
    EXPECT_STREQ(property->value(), value);
  }

  zx_status_t WaitForInterruptImpl() override {
    zx::result result = fake_bti::GetPinnedVmo(zx::unowned_bti(bti()));
    if (result.is_error()) {
      return result.status_value();
    }

    // In the tuning case there are exactly two VMOs pinned: one to hold the DMA descriptors, and
    // one to hold the received tuning block. Write the expected tuning data to the second pinned
    // VMO so that the tuning check always passes.
    constexpr size_t kTuningVmoSize = 2;
    std::vector<fake_bti::FakeBtiPinnedVmoInfo> pinned_vmos = std::move(result.value());
    if (pinned_vmos.size() == kTuningVmoSize &&
        pinned_vmos[0].size >= sizeof(aml_sdmmc_tuning_blk_pattern_4bit)) {
      zx_vmo_write(pinned_vmos[1].vmo.get(), aml_sdmmc_tuning_blk_pattern_4bit,
                   pinned_vmos[1].offset, sizeof(aml_sdmmc_tuning_blk_pattern_4bit));
    }
    for (auto& pinned_vmo : pinned_vmos) {
      zx_handle_close(pinned_vmo.vmo.get());
    }

    if (request_index_ < request_results_.size() && request_results_[request_index_] == 0) {
      // Indicate a receive CRC error.
      view_->Write32(1, kAmlSdmmcStatusOffset);

      successful_transfers_ = 0;
      request_index_++;
    } else if (interrupt_status_.has_value()) {
      view_->Write32(interrupt_status_.value(), kAmlSdmmcStatusOffset);
    } else {
      // Indicate that the request completed successfully.
      view_->Write32(1 << 13, kAmlSdmmcStatusOffset);

      // Each tuning transfer is attempted five times with a short-circuit if one fails.
      // Report every successful transfer five times to make the results arrays easier to
      // follow.
      if (++successful_transfers_ % AML_SDMMC_TUNING_TEST_ATTEMPTS == 0) {
        successful_transfers_ = 0;
        request_index_++;
      }
    }
    return ZX_OK;
  }

  void WaitForBus() const override { /* Do nothing, bus is always ready in tests */ }

  void SetRequestResults(const char* request_results) {
    request_results_.clear();
    const size_t results_size = strlen(request_results);
    request_results_.reserve(results_size);

    for (size_t i = 0; i < results_size; i++) {
      ASSERT_TRUE((request_results[i] == '|') || (request_results[i] == '-'));
      request_results_.push_back(request_results[i] == '|' ? 1 : 0);
    }

    request_index_ = 0;
  }

  void SetRequestInterruptStatus(uint32_t status) { interrupt_status_ = status; }

 private:
  std::vector<uint8_t> request_results_;
  size_t request_index_ = 0;
  uint32_t successful_transfers_ = 0;
  // The optional interrupt status to set after a request is completed.
  std::optional<uint32_t> interrupt_status_;
  inspect::InspectTestHelper inspector_;
  std::optional<fdf::MmioView> view_;
};

class FakeLessor : public fidl::Server<fuchsia_power_broker::Lessor> {
 public:
  void AddSideEffect(fit::function<void()> side_effect) { side_effect_ = std::move(side_effect); }

  void Lease(LeaseRequest& req, LeaseCompleter::Sync& completer) override {
    if (side_effect_) {
      side_effect_();
    }

    auto [lease_control_client_end, lease_control_server_end] =
        fidl::Endpoints<fuchsia_power_broker::LeaseControl>::Create();
    completer.Reply(fit::success(std::move(lease_control_client_end)));
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_power_broker::Lessor> md,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}

 private:
  fit::function<void()> side_effect_;
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

class FakePowerBroker : public fidl::Server<fuchsia_power_broker::Topology> {
 public:
  fidl::ProtocolHandler<fuchsia_power_broker::Topology> CreateHandler() {
    return bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                   fidl::kIgnoreBindingClosure);
  }

  void AddElement(fuchsia_power_broker::ElementSchema& req,
                  AddElementCompleter::Sync& completer) override {
    completer.Reply(fit::success());
  }

  void Lease(LeaseRequest& req, LeaseCompleter::Sync& completer) override {
    completer.Reply(fit::success());
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_power_broker::Topology> md,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}

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

  FakeLessor* hardware_power_lessor_ = nullptr;
  fidl::Client<fuchsia_power_broker::ElementRunner> hardware_power_element_runner_client_;
  FakeElementControl* hardware_power_element_control_ = nullptr;

 private:
  fidl::ServerBindingGroup<fuchsia_power_broker::Topology> bindings_;

  std::vector<PowerElement> servers_;
};

class AmlSdmmcTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    fdf_fake::FakePDev::Config config{.use_fake_irq = true, .device_info = fdf::PDev::DeviceInfo{}};
    zx::vmo dup;
    zx_status_t status = mmio_buffer_.get_vmo()->duplicate(ZX_RIGHT_SAME_RIGHTS, &dup);
    if (status != ZX_OK) {
      return zx::error(status);
    }
    config.mmios[0] = fdf::PDev::MmioInfo{
        .offset = mmio_buffer_.get_offset(),
        .size = mmio_buffer_.get_size(),
        .vmo = std::move(dup),
    };
    config.btis[0] = std::move(bti_);
    pdev_server_.SetConfig(std::move(config));

    zx_status_t metadata_status =
        pdev_server_.AddFidlMetadata(fuchsia_hardware_sdmmc::SdmmcMetadata::kSerializableName,
                                     fuchsia_hardware_sdmmc::SdmmcMetadata{});
    if (metadata_status != ZX_OK) {
      return zx::error(metadata_status);
    }

    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();
    zx::result<> result = to_driver_vfs.AddService<fuchsia_hardware_platform_device::Service>(
        pdev_server_.GetInstanceHandler(dispatcher), "default");
    if (result.is_error()) {
      return result.take_error();
    }

    result = to_driver_vfs.AddService<fuchsia_hardware_clock::Service>(
        clock_server_.CreateInstanceHandler(dispatcher), "clock-gate");
    if (result.is_error()) {
      return result.take_error();
    }

    result = to_driver_vfs.component().AddUnmanagedProtocol<fuchsia_power_broker::Topology>(
        power_broker_.CreateHandler());
    if (result.is_error()) {
      return result.take_error();
    }

    return zx::ok();
  }

  void SetBti(zx::bti bti) { bti_ = std::move(bti); }

  fdf_fake::FakePDev& pdev_server() { return pdev_server_; }
  fdf_fake::FakeClock& clock_server() { return clock_server_; }
  FakePowerBroker& power_broker() { return power_broker_; }
  fdf::MmioBuffer& mmio_buffer() { return mmio_buffer_; }

 private:
  fdf_fake::FakePDev pdev_server_;
  fdf_fake::FakeClock clock_server_;
  FakePowerBroker power_broker_;
  zx::bti bti_;
  fdf::MmioBuffer mmio_buffer_ =
      fdf_testing::CreateMmioBuffer(S912_SD_EMMC_B_LENGTH, ZX_CACHE_POLICY_UNCACHED_DEVICE);
};

struct TestConfig {
  using DriverType = TestAmlSdmmcWithBanjo;
  using EnvironmentType = AmlSdmmcTestEnvironment;
};

class AmlSdmmcWithBanjoTest : public zxtest::Test {
 public:
  AmlSdmmcWithBanjoTest() {}

  void StartDriver(bool create_fake_bti_with_paddrs = false, bool supply_power_framework = false) {
    // This is used by AmlSdmmc::Init() to create the descriptor buffer -- can be any nonzero paddr.
    const zx_paddr_t paddrs[] = {zx_system_get_page_size()};
    zx::result result = create_fake_bti_with_paddrs ? fake_bti::CreateFakeBtiWithPaddrs(paddrs)
                                                    : fake_bti::CreateFakeBti();
    ASSERT_TRUE(result.is_ok());
    zx::bti bti = std::move(result.value());

    ASSERT_OK(bti.duplicate(ZX_RIGHT_SAME_RIGHTS, &bti_owned_));
    bti_ = bti_owned_.borrow();

    driver_test().RunInEnvironmentTypeContext(
        [bti = std::move(bti)](AmlSdmmcTestEnvironment& env) mutable {
          env.SetBti(std::move(bti));
        });

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
      local_power_args.token() = std::move(element_token_copy);

      power_args = std::move(local_power_args);

      driver_test().RunInEnvironmentTypeContext(
          [control_server = std::move(element_control_server),
           runner_client = std::move(element_runner_client),
           lessor_server = std::move(lessor_server)](AmlSdmmcTestEnvironment& env) mutable {
            env.power_broker().AddHardwarePowerElement(
                std::move(control_server), std::move(runner_client), std::move(lessor_server));
          });
    }

    zx::result<> start_result = driver_test().StartDriverWithCustomStartArgs(
        [supply_power_framework,
         &power_args](fuchsia_driver_framework::DriverStartArgs& start_args) {
          aml_sdmmc_config::Config fake_config;
          fake_config.enable_suspend() = supply_power_framework;
          start_args.config(fake_config.ToVmo());

          if (supply_power_framework) {
            start_args.power_element_args(std::move(power_args.value()));
          }
        });
    ASSERT_OK(start_result.status_value());

    driver_test().RunInEnvironmentTypeContext(
        [&](AmlSdmmcTestEnvironment& env) { mmio_.emplace(env.mmio_buffer().View(0)); });

    dut_ = driver_test().driver();
    descs_ = dut_->SetTestHooks();

    mmio_->Write32(0xff, kAmlSdmmcDelay1Offset);
    mmio_->Write32(0xff, kAmlSdmmcDelay2Offset);
    mmio_->Write32(0xff, kAmlSdmmcAdjustOffset);

    ASSERT_OK(dut_->SdmmcHwReset());

    EXPECT_EQ(mmio_->Read32(kAmlSdmmcDelay1Offset), 0);
    EXPECT_EQ(mmio_->Read32(kAmlSdmmcDelay2Offset), 0);
    EXPECT_EQ(mmio_->Read32(kAmlSdmmcAdjustOffset), 0);

    mmio_->Write32(1, kAmlSdmmcCfgOffset);  // Set bus width 4.
  }

  void TearDown() override {
    zx::result<> stop_result = driver_test().StopDriver();
    EXPECT_OK(stop_result.status_value());
    dut_ = nullptr;
  }

  bool IsClockEnabled() {
    return driver_test().RunInEnvironmentTypeContext<bool>(
        [](AmlSdmmcTestEnvironment& env) { return env.clock_server().enabled(); });
  }

  fidl::ClientEnd<fuchsia_io::Directory> CreateDriverSvcClient() {
    return driver_test().ConnectToDriverSvcDir();
  }

  fdf::WireSyncClient<fuchsia_hardware_sdmmc::Sdmmc> GetClient() {
    fdf::WireSyncClient<fuchsia_hardware_sdmmc::Sdmmc> client;

    [&]() {
      zx::result sdmmc_client_end =
          fdf::internal::DriverTransportConnect<fuchsia_hardware_sdmmc::SdmmcService::Sdmmc>(
              CreateDriverSvcClient(), component::kDefaultInstance);
      ASSERT_TRUE(sdmmc_client_end.is_ok());

      client.Bind(std::move(*sdmmc_client_end));
    }();

    return client;
  }

  fdf_testing::ForegroundDriverTest<TestConfig>& driver_test() { return driver_test_; }

 protected:
  static zx_koid_t GetVmoKoid(const zx::vmo& vmo) {
    zx_info_handle_basic_t info = {};
    size_t actual = 0;
    size_t available = 0;
    zx_status_t status =
        vmo.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), &actual, &available);
    if (status != ZX_OK || actual < 1) {
      return ZX_KOID_INVALID;
    }
    return info.koid;
  }

  void InitializeContiguousPaddrs(const size_t vmos) {
    std::vector<zx_paddr_t> paddrs;
    for (size_t i = 0; i < vmos; i++) {
      paddrs.push_back((i << 24) | zx_system_get_page_size());
    }
    ASSERT_OK(fake_bti::SetPaddrs(zx::unowned_bti(bti_), paddrs));
  }

  void InitializeSingleVmoPaddrs(const size_t pages) {
    std::vector<zx_paddr_t> paddrs;
    for (size_t i = 0; i < pages; i++) {
      paddrs.push_back(zx_system_get_page_size() * (i + 1));
    }
    ASSERT_OK(fake_bti::SetPaddrs(zx::unowned_bti(bti_), paddrs));
  }

  void InitializeNonContiguousPaddrs(const size_t vmos) {
    std::vector<zx_paddr_t> paddrs;
    for (size_t i = 0; i < vmos; i++) {
      paddrs.push_back(zx_system_get_page_size() * (i + 1) * 2);
    }
    ASSERT_OK(fake_bti::SetPaddrs(zx::unowned_bti(bti_), paddrs));
  }

  aml_sdmmc_desc_t* descriptors() const { return reinterpret_cast<aml_sdmmc_desc_t*>(descs_); }

  zx::unowned_bti bti_;
  zx::bti bti_owned_;

  std::optional<fdf::MmioView> mmio_;
  TestAmlSdmmcWithBanjo* dut_ = nullptr;

 private:
  fdf_testing::ForegroundDriverTest<TestConfig> driver_test_;
  void* descs_ = nullptr;
};

TEST_F(AmlSdmmcWithBanjoTest, Init) {
  StartDriver();

  AmlSdmmcClock::Get().FromValue(0).WriteTo(&*mmio_);

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  EXPECT_EQ(AmlSdmmcClock::Get().ReadFrom(&*mmio_).reg_value(), AmlSdmmcClock::Get()
                                                                    .FromValue(0)
                                                                    .set_cfg_div(60)
                                                                    .set_cfg_src(0)
                                                                    .set_cfg_co_phase(2)
                                                                    .set_cfg_tx_phase(0)
                                                                    .set_cfg_rx_phase(0)
                                                                    .set_cfg_always_on(1)
                                                                    .reg_value());
}

TEST_F(AmlSdmmcWithBanjoTest, InitTimeoutDescBusy) {
  StartDriver();

  AmlSdmmcStatus::Get().FromValue(0).set_desc_busy(1).WriteTo(&*mmio_);
  EXPECT_EQ(dut_->Init(TestAmlSdmmcWithBanjo::kInstance), ZX_ERR_TIMED_OUT);
}

TEST_F(AmlSdmmcWithBanjoTest, InitTimeoutCoreBusy) {
  StartDriver();

  AmlSdmmcStatus::Get().FromValue(0).set_core_busy(1).WriteTo(&*mmio_);
  EXPECT_EQ(dut_->Init(TestAmlSdmmcWithBanjo::kInstance), ZX_ERR_TIMED_OUT);
}

TEST_F(AmlSdmmcWithBanjoTest, Tuning) {
  StartDriver();

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  AmlSdmmcClock::Get().FromValue(0).set_cfg_div(10).WriteTo(&*mmio_);
  AmlSdmmcCfg::Get().ReadFrom(&*mmio_).set_bus_width(AmlSdmmcCfg::kBusWidth4Bit).WriteTo(&*mmio_);

  auto adjust = AmlSdmmcAdjust::Get().FromValue(0);

  adjust.set_adj_fixed(0).set_adj_delay(0x3f).WriteTo(&*mmio_);

  EXPECT_OK(dut_->SdmmcPerformTuning(SD_SEND_TUNING_BLOCK));

  adjust.ReadFrom(&*mmio_);

  EXPECT_EQ(adjust.adj_fixed(), 1);
  EXPECT_EQ(adjust.adj_delay(), 0);
}

TEST_F(AmlSdmmcWithBanjoTest, DelayLineTuningAllPass) {
  StartDriver();

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  AmlSdmmcClock::Get().FromValue(0).set_cfg_div(10).WriteTo(&*mmio_);
  AmlSdmmcCfg::Get().ReadFrom(&*mmio_).set_bus_width(AmlSdmmcCfg::kBusWidth4Bit).WriteTo(&*mmio_);

  auto adjust = AmlSdmmcAdjust::Get().FromValue(0).set_adj_delay(0x3f).WriteTo(&*mmio_);
  auto delay1 = AmlSdmmcDelay1::Get().FromValue(0).WriteTo(&*mmio_);
  auto delay2 = AmlSdmmcDelay2::Get().FromValue(0).WriteTo(&*mmio_);

  EXPECT_OK(dut_->SdmmcPerformTuning(SD_SEND_TUNING_BLOCK));

  adjust.ReadFrom(&*mmio_);
  delay1.ReadFrom(&*mmio_);
  delay2.ReadFrom(&*mmio_);

  // No failing window was found, so the default settings should be used.
  EXPECT_EQ(adjust.adj_delay(), 0);
  EXPECT_EQ(delay1.dly_0(), 0);
  EXPECT_EQ(delay1.dly_1(), 0);
  EXPECT_EQ(delay1.dly_2(), 0);
  EXPECT_EQ(delay1.dly_3(), 0);
  EXPECT_EQ(delay1.dly_4(), 0);
  EXPECT_EQ(delay2.dly_5(), 0);
  EXPECT_EQ(delay2.dly_6(), 0);
  EXPECT_EQ(delay2.dly_7(), 0);
  EXPECT_EQ(delay2.dly_8(), 0);
  EXPECT_EQ(delay2.dly_9(), 0);

  dut_->ExpectInspectPropertyValue("adj_delay", 0);

  dut_->ExpectInspectPropertyValue("delay_lines", 0);

  dut_->ExpectInspectPropertyValue(
      "tuning_results_adj_delay_0", "tuning_results",
      "||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||");

  dut_->ExpectInspectPropertyValue("distance_to_failing_point", 63);
}

TEST_F(AmlSdmmcWithBanjoTest, DelayLineTuningFailingPoint) {
  StartDriver();

  dut_->SetRequestResults(
      "-----------|||||||||||||||||||||||||||||||||||||||||||||||||----"
      "-------------------------------|||||||||||||||||||||||||||||||||"
      "||||||||||-----------------------------------------|||||||||||||"
      "||||||||||||||||||||||||||||||----------------------------------"
      "||||||||||||||||||||||||||||||||||||||||||||||||||--------------"
      "||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||"
      "-----------|||||||||||||||||||||||||||||||||||||||||||||||||||||"
      "-------------------------------|||||||||||||||||||||||||||||||||"
      "||||||||||||||||||||-------------------------------|||||||||||||"
      "||||||||||||||||||||||||||||||||||||||||------------------------");

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  AmlSdmmcClock::Get().FromValue(0).set_cfg_div(10).WriteTo(&*mmio_);
  AmlSdmmcCfg::Get().ReadFrom(&*mmio_).set_bus_width(AmlSdmmcCfg::kBusWidth4Bit).WriteTo(&*mmio_);

  auto adjust = AmlSdmmcAdjust::Get().FromValue(0).set_adj_delay(0x3f).WriteTo(&*mmio_);
  auto delay1 = AmlSdmmcDelay1::Get().FromValue(0).WriteTo(&*mmio_);
  auto delay2 = AmlSdmmcDelay2::Get().FromValue(0).WriteTo(&*mmio_);

  EXPECT_OK(dut_->SdmmcPerformTuning(SD_SEND_TUNING_BLOCK));

  adjust.ReadFrom(&*mmio_);
  delay1.ReadFrom(&*mmio_);
  delay2.ReadFrom(&*mmio_);

  EXPECT_EQ(adjust.adj_delay(), 7);
  EXPECT_EQ(delay1.dly_0(), 30);
  EXPECT_EQ(delay1.dly_1(), 30);
  EXPECT_EQ(delay1.dly_2(), 30);
  EXPECT_EQ(delay1.dly_3(), 30);
  EXPECT_EQ(delay1.dly_4(), 30);
  EXPECT_EQ(delay2.dly_5(), 30);
  EXPECT_EQ(delay2.dly_6(), 30);
  EXPECT_EQ(delay2.dly_7(), 30);
  EXPECT_EQ(delay2.dly_8(), 30);
  EXPECT_EQ(delay2.dly_9(), 30);

  dut_->ExpectInspectPropertyValue("adj_delay", 7);

  dut_->ExpectInspectPropertyValue("delay_lines", 30);

  dut_->ExpectInspectPropertyValue(
      "tuning_results_adj_delay_7", "tuning_results",
      "-------------------------------|||||||||||||||||||||||||||||||||");

  dut_->ExpectInspectPropertyValue("distance_to_failing_point", 0);
}

TEST_F(AmlSdmmcWithBanjoTest, DelayLineTuningEvenDivider) {
  StartDriver();

  dut_->SetRequestResults(
      // Largest failing window: adj_delay 8, middle delay 25
      "||||||||||||||||||||||||||||||||||||||||||||||||||--------------"
      "||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||"
      "||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||"
      "||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||"
      "||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||"
      "||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||"
      "-|||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||"
      "---------------------|||||||||||||||||||||||||||||||||||||||||||"
      "||||||||||-------------------------------|||||||||||||||||||||||"
      "||||||||||||||||||||||||||||||-------------------------------|||");

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  AmlSdmmcClock::Get().FromValue(0).set_cfg_div(10).WriteTo(&*mmio_);
  AmlSdmmcCfg::Get().ReadFrom(&*mmio_).set_bus_width(AmlSdmmcCfg::kBusWidth4Bit).WriteTo(&*mmio_);

  auto adjust = AmlSdmmcAdjust::Get().FromValue(0).set_adj_delay(0x3f).WriteTo(&*mmio_);
  auto delay1 = AmlSdmmcDelay1::Get().FromValue(0).WriteTo(&*mmio_);
  auto delay2 = AmlSdmmcDelay2::Get().FromValue(0).WriteTo(&*mmio_);

  EXPECT_OK(dut_->SdmmcPerformTuning(SD_SEND_TUNING_BLOCK));

  adjust.ReadFrom(&*mmio_);
  delay1.ReadFrom(&*mmio_);
  delay2.ReadFrom(&*mmio_);

  EXPECT_EQ(adjust.adj_delay(), 3);
  EXPECT_EQ(delay1.dly_0(), 25);
  EXPECT_EQ(delay1.dly_1(), 25);
  EXPECT_EQ(delay1.dly_2(), 25);
  EXPECT_EQ(delay1.dly_3(), 25);
  EXPECT_EQ(delay1.dly_4(), 25);
  EXPECT_EQ(delay2.dly_5(), 25);
  EXPECT_EQ(delay2.dly_6(), 25);
  EXPECT_EQ(delay2.dly_7(), 25);
  EXPECT_EQ(delay2.dly_8(), 25);
  EXPECT_EQ(delay2.dly_9(), 25);

  dut_->ExpectInspectPropertyValue("adj_delay", 3);

  dut_->ExpectInspectPropertyValue("delay_lines", 25);

  dut_->ExpectInspectPropertyValue(
      "tuning_results_adj_delay_4", "tuning_results",
      "||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||");

  dut_->ExpectInspectPropertyValue("distance_to_failing_point", 63);
}

TEST_F(AmlSdmmcWithBanjoTest, DelayLineTuningOddDivider) {
  StartDriver();

  dut_->SetRequestResults(
      // Largest failing window: adj_delay 3, first delay 0
      "||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||"
      "||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||"
      "-----------|||||||||||||||||||||||||||||||||||||||||||||||||||||"
      "-------------------------------|||||||||||||||||||||||||||||||||"
      "||||||||||||||||||||-------------------------------|||||||||||||"
      "||||||||||||||||||||||||||||||||||||||||------------------------"
      "||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||----"
      "||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||"
      "||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||");

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  AmlSdmmcClock::Get().FromValue(0).set_cfg_div(9).WriteTo(&*mmio_);
  AmlSdmmcCfg::Get().ReadFrom(&*mmio_).set_bus_width(AmlSdmmcCfg::kBusWidth4Bit).WriteTo(&*mmio_);

  auto adjust = AmlSdmmcAdjust::Get().FromValue(0).set_adj_delay(0x3f).WriteTo(&*mmio_);
  auto delay1 = AmlSdmmcDelay1::Get().FromValue(0).WriteTo(&*mmio_);
  auto delay2 = AmlSdmmcDelay2::Get().FromValue(0).WriteTo(&*mmio_);

  EXPECT_OK(dut_->SdmmcPerformTuning(SD_SEND_TUNING_BLOCK));

  adjust.ReadFrom(&*mmio_);
  delay1.ReadFrom(&*mmio_);
  delay2.ReadFrom(&*mmio_);

  EXPECT_EQ(adjust.adj_delay(), 7);
  EXPECT_EQ(delay1.dly_0(), 0);
  EXPECT_EQ(delay1.dly_1(), 0);
  EXPECT_EQ(delay1.dly_2(), 0);
  EXPECT_EQ(delay1.dly_3(), 0);
  EXPECT_EQ(delay1.dly_4(), 0);
  EXPECT_EQ(delay2.dly_5(), 0);
  EXPECT_EQ(delay2.dly_6(), 0);
  EXPECT_EQ(delay2.dly_7(), 0);
  EXPECT_EQ(delay2.dly_8(), 0);
  EXPECT_EQ(delay2.dly_9(), 0);

  dut_->ExpectInspectPropertyValue("adj_delay", 7);

  dut_->ExpectInspectPropertyValue("delay_lines", 0);

  dut_->ExpectInspectPropertyValue(
      "tuning_results_adj_delay_7", "tuning_results",
      "||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||");

  dut_->ExpectInspectPropertyValue("distance_to_failing_point", 63);
}

TEST_F(AmlSdmmcWithBanjoTest, DelayLineTuningCorrectFailingWindowIfLastOne) {
  StartDriver();

  dut_->SetRequestResults(
      "||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||----"
      "||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||"
      "||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||"
      "||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||"
      "||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||");

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  AmlSdmmcClock::Get().FromValue(0).set_cfg_div(5).WriteTo(&*mmio_);
  AmlSdmmcCfg::Get().ReadFrom(&*mmio_).set_bus_width(AmlSdmmcCfg::kBusWidth4Bit).WriteTo(&*mmio_);

  auto adjust = AmlSdmmcAdjust::Get().FromValue(0).set_adj_delay(0x3f).WriteTo(&*mmio_);
  auto delay1 = AmlSdmmcDelay1::Get().FromValue(0).WriteTo(&*mmio_);
  auto delay2 = AmlSdmmcDelay2::Get().FromValue(0).WriteTo(&*mmio_);

  EXPECT_OK(dut_->SdmmcPerformTuning(SD_SEND_TUNING_BLOCK));

  adjust.ReadFrom(&*mmio_);
  delay1.ReadFrom(&*mmio_);
  delay2.ReadFrom(&*mmio_);

  EXPECT_EQ(adjust.adj_delay(), 2);
  EXPECT_EQ(delay1.dly_0(), 60);
  EXPECT_EQ(delay1.dly_1(), 60);
  EXPECT_EQ(delay1.dly_2(), 60);
  EXPECT_EQ(delay1.dly_3(), 60);
  EXPECT_EQ(delay1.dly_4(), 60);
  EXPECT_EQ(delay2.dly_5(), 60);
  EXPECT_EQ(delay2.dly_6(), 60);
  EXPECT_EQ(delay2.dly_7(), 60);
  EXPECT_EQ(delay2.dly_8(), 60);
  EXPECT_EQ(delay2.dly_9(), 60);

  dut_->ExpectInspectPropertyValue("adj_delay", 2);

  dut_->ExpectInspectPropertyValue("delay_lines", 60);
}

TEST_F(AmlSdmmcWithBanjoTest, SetBusFreq) {
  StartDriver();

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));
  dut_->ExpectInspectPropertyValue("bus_clock_frequency", 400'000);

  AmlSdmmcCfg::Get().ReadFrom(&*mmio_).set_bus_width(AmlSdmmcCfg::kBusWidth4Bit).WriteTo(&*mmio_);

  auto clock = AmlSdmmcClock::Get().FromValue(0).WriteTo(&*mmio_);

  EXPECT_OK(dut_->SdmmcSetBusFreq(100'000'000));
  EXPECT_EQ(clock.ReadFrom(&*mmio_).cfg_div(), 10);
  EXPECT_EQ(clock.cfg_src(), 1);
  dut_->ExpectInspectPropertyValue("bus_clock_frequency", 100'000'000);

  EXPECT_OK(dut_->SdmmcSetBusFreq(0));
  EXPECT_EQ(clock.ReadFrom(&*mmio_).cfg_div(), 0);
  dut_->ExpectInspectPropertyValue("bus_clock_frequency", 0);

  EXPECT_OK(dut_->SdmmcSetBusFreq(54'000'000));
  EXPECT_EQ(clock.ReadFrom(&*mmio_).cfg_div(), 19);
  EXPECT_EQ(clock.cfg_src(), 1);
  dut_->ExpectInspectPropertyValue("bus_clock_frequency", 52'631'578);

  EXPECT_OK(dut_->SdmmcSetBusFreq(400'000));
  EXPECT_EQ(clock.ReadFrom(&*mmio_).cfg_div(), 60);
  EXPECT_EQ(clock.cfg_src(), 0);
  dut_->ExpectInspectPropertyValue("bus_clock_frequency", 400'000);
}

TEST_F(AmlSdmmcWithBanjoTest, ClearStatus) {
  StartDriver();

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  // Set end_of_chain to indicate we're done and to have something to clear
  dut_->SetRequestInterruptStatus(1 << 13);
  sdmmc_req_t request;
  memset(&request, 0, sizeof(request));
  uint32_t unused_response[4];
  EXPECT_OK(dut_->SdmmcRequest(&request, unused_response));

  auto status = AmlSdmmcStatus::Get().FromValue(0);
  EXPECT_EQ(AmlSdmmcStatus::kClearStatus, status.ReadFrom(&*mmio_).reg_value());
}

TEST_F(AmlSdmmcWithBanjoTest, TxCrcError) {
  StartDriver();

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  // Set TX CRC error bit (8) and desc_busy bit (30)
  dut_->SetRequestInterruptStatus(1 << 8 | 1 << 30);
  sdmmc_req_t request;
  memset(&request, 0, sizeof(request));
  uint32_t unused_response[4];
  EXPECT_EQ(ZX_ERR_IO_DATA_INTEGRITY, dut_->SdmmcRequest(&request, unused_response));

  auto start = AmlSdmmcStart::Get().FromValue(0);
  // The desc busy bit should now have been cleared because of the error
  EXPECT_EQ(0, start.ReadFrom(&*mmio_).desc_busy());
}

TEST_F(AmlSdmmcWithBanjoTest, UnownedVmosBlockMode) {
  StartDriver(/*create_fake_bti_with_paddrs=*/true);
  InitializeContiguousPaddrs(10);

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  zx::vmo vmos[10] = {};
  sdmmc_buffer_region_t buffers[10];
  for (uint32_t i = 0; i < std::size(vmos); i++) {
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmos[i]));
    buffers[i] = {
        .buffer =
            {
                .vmo = vmos[i].get(),
            },
        .type = SDMMC_BUFFER_TYPE_VMO_HANDLE,
        .offset = i * 16,
        .size = 32 * (i + 2),
    };
  }

  sdmmc_req_t request = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 32,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers_list = buffers,
      .buffers_count = std::size(buffers),
  };
  uint32_t response[4] = {};
  AmlSdmmcCmdResp::Get().FromValue(0xfedc9876).WriteTo(&*mmio_);
  EXPECT_OK(dut_->SdmmcRequest(&request, response));
  EXPECT_EQ(response[0], 0xfedc9876);

  const aml_sdmmc_desc_t* descs = descriptors();
  auto expected_desc_cfg = AmlSdmmcCmdCfg::Get()
                               .FromValue(0)
                               .set_len(2)
                               .set_block_mode(1)
                               .set_timeout(AmlSdmmcCmdCfg::kDefaultCmdTimeout)
                               .set_data_io(1)
                               .set_data_wr(0)
                               .set_resp_num(1)
                               .set_cmd_idx(SDMMC_READ_MULTIPLE_BLOCK)
                               .set_owner(1);

  EXPECT_EQ(descs[0].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[0].cmd_arg, 0x1234abcd);
  EXPECT_EQ(descs[0].data_addr, zx_system_get_page_size());
  EXPECT_EQ(descs[0].resp_addr, 0);

  for (uint32_t i = 1; i < std::size(vmos); i++) {
    expected_desc_cfg.set_len(i + 2).set_no_resp(1).set_no_cmd(1).set_resp_num(0).set_cmd_idx(0);
    if (i == std::size(vmos) - 1) {
      expected_desc_cfg.set_end_of_chain(1);
    }
    EXPECT_EQ(descs[i].cmd_info, expected_desc_cfg.reg_value());
    EXPECT_EQ(descs[i].cmd_arg, 0);
    EXPECT_EQ(descs[i].data_addr, (i << 24) | (zx_system_get_page_size() + (i * 16)));
    EXPECT_EQ(descs[i].resp_addr, 0);
  }
}

TEST_F(AmlSdmmcWithBanjoTest, UnownedVmosNotBlockSizeMultiple) {
  StartDriver(/*create_fake_bti_with_paddrs=*/true);
  InitializeContiguousPaddrs(10);

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  zx::vmo vmos[10] = {};
  sdmmc_buffer_region_t buffers[10];
  for (uint32_t i = 0; i < std::size(vmos); i++) {
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmos[i]));
    buffers[i] = {
        .buffer =
            {
                .vmo = vmos[i].get(),
            },
        .type = SDMMC_BUFFER_TYPE_VMO_HANDLE,
        .offset = 0,
        .size = 32 * (i + 2),
    };
  }

  buffers[5].size = 25;

  sdmmc_req_t request = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 32,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers_list = buffers,
      .buffers_count = std::size(buffers),
  };
  uint32_t response[4] = {};
  EXPECT_NOT_OK(dut_->SdmmcRequest(&request, response));
}

TEST_F(AmlSdmmcWithBanjoTest, UnownedVmosByteMode) {
  StartDriver(/*create_fake_bti_with_paddrs=*/true);
  InitializeContiguousPaddrs(10);

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  zx::vmo vmos[10] = {};
  sdmmc_buffer_region_t buffers[10];
  for (uint32_t i = 0; i < std::size(vmos); i++) {
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmos[i]));
    buffers[i] = {
        .buffer =
            {
                .vmo = vmos[i].get(),
            },
        .type = SDMMC_BUFFER_TYPE_VMO_HANDLE,
        .offset = i * 4,
        .size = 50,
    };
  }

  sdmmc_req_t request = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 50,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers_list = buffers,
      .buffers_count = std::size(buffers),
  };
  uint32_t response[4] = {};
  AmlSdmmcCmdResp::Get().FromValue(0xfedc9876).WriteTo(&*mmio_);
  EXPECT_OK(dut_->SdmmcRequest(&request, response));
  EXPECT_EQ(response[0], 0xfedc9876);

  const aml_sdmmc_desc_t* descs = descriptors();
  auto expected_desc_cfg = AmlSdmmcCmdCfg::Get()
                               .FromValue(0)
                               .set_len(50)
                               .set_timeout(AmlSdmmcCmdCfg::kDefaultCmdTimeout)
                               .set_data_io(1)
                               .set_data_wr(0)
                               .set_resp_num(1)
                               .set_cmd_idx(SDMMC_READ_MULTIPLE_BLOCK)
                               .set_owner(1);

  EXPECT_EQ(descs[0].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[0].cmd_arg, 0x1234abcd);
  EXPECT_EQ(descs[0].data_addr, zx_system_get_page_size());
  EXPECT_EQ(descs[0].resp_addr, 0);

  for (uint32_t i = 1; i < std::size(vmos); i++) {
    expected_desc_cfg.set_len(50).set_no_resp(1).set_no_cmd(1).set_resp_num(0).set_cmd_idx(0);
    if (i == std::size(vmos) - 1) {
      expected_desc_cfg.set_end_of_chain(1);
    }
    EXPECT_EQ(descs[i].cmd_info, expected_desc_cfg.reg_value());
    EXPECT_EQ(descs[i].cmd_arg, 0);
    EXPECT_EQ(descs[i].data_addr, (i << 24) | (zx_system_get_page_size() + (i * 4)));
    EXPECT_EQ(descs[i].resp_addr, 0);
  }
}

TEST_F(AmlSdmmcWithBanjoTest, UnownedVmoByteModeMultiBlock) {
  StartDriver(/*create_fake_bti_with_paddrs=*/true);
  InitializeContiguousPaddrs(1);

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  sdmmc_buffer_region_t buffer = {
      .buffer =
          {
              .vmo = vmo.get(),
          },
      .type = SDMMC_BUFFER_TYPE_VMO_HANDLE,
      .offset = 0,
      .size = 400,
  };

  sdmmc_req_t request = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 100,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers_list = &buffer,
      .buffers_count = 1,
  };
  uint32_t response[4] = {};
  AmlSdmmcCmdResp::Get().FromValue(0xfedc9876).WriteTo(&*mmio_);
  EXPECT_OK(dut_->SdmmcRequest(&request, response));
  EXPECT_EQ(response[0], 0xfedc9876);

  const aml_sdmmc_desc_t* descs = descriptors();
  auto expected_desc_cfg = AmlSdmmcCmdCfg::Get()
                               .FromValue(0)
                               .set_len(100)
                               .set_timeout(AmlSdmmcCmdCfg::kDefaultCmdTimeout)
                               .set_data_io(1)
                               .set_data_wr(0)
                               .set_resp_num(1)
                               .set_cmd_idx(SDMMC_READ_MULTIPLE_BLOCK)
                               .set_owner(1);

  EXPECT_EQ(descs[0].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[0].cmd_arg, 0x1234abcd);
  EXPECT_EQ(descs[0].data_addr, zx_system_get_page_size());
  EXPECT_EQ(descs[0].resp_addr, 0);

  for (uint32_t i = 1; i < 4; i++) {
    expected_desc_cfg.set_no_resp(1).set_no_cmd(1).set_resp_num(0).set_cmd_idx(0);
    if (i == 3) {
      expected_desc_cfg.set_end_of_chain(1);
    }
    EXPECT_EQ(descs[i].cmd_info, expected_desc_cfg.reg_value());
    EXPECT_EQ(descs[i].cmd_arg, 0);
    EXPECT_EQ(descs[i].data_addr, zx_system_get_page_size() + (i * 100));
    EXPECT_EQ(descs[i].resp_addr, 0);
  }
}

TEST_F(AmlSdmmcWithBanjoTest, UnownedVmoOffsetNotAligned) {
  StartDriver(/*create_fake_bti_with_paddrs=*/true);
  InitializeContiguousPaddrs(1);

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  sdmmc_buffer_region_t buffer = {
      .buffer =
          {
              .vmo = vmo.get(),
          },
      .type = SDMMC_BUFFER_TYPE_VMO_HANDLE,
      .offset = 3,
      .size = 64,
  };

  sdmmc_req_t request = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 32,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers_list = &buffer,
      .buffers_count = 1,
  };
  uint32_t response[4] = {};
  AmlSdmmcCmdResp::Get().FromValue(0xfedc9876).WriteTo(&*mmio_);
  EXPECT_NOT_OK(dut_->SdmmcRequest(&request, response));
}

TEST_F(AmlSdmmcWithBanjoTest, UnownedVmoSingleBufferMultipleDescriptors) {
  StartDriver(/*create_fake_bti_with_paddrs=*/true);
  const size_t pages = ((32 * 514) / zx_system_get_page_size()) + 1;
  InitializeSingleVmoPaddrs(pages);

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(pages * zx_system_get_page_size(), 0, &vmo));

  sdmmc_buffer_region_t buffer = {
      .buffer =
          {
              .vmo = vmo.get(),
          },
      .type = SDMMC_BUFFER_TYPE_VMO_HANDLE,
      .offset = 16,
      .size = 32 * 513,
  };

  sdmmc_req_t request = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 32,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers_list = &buffer,
      .buffers_count = 1,
  };
  uint32_t response[4] = {};
  AmlSdmmcCmdResp::Get().FromValue(0xfedc9876).WriteTo(&*mmio_);
  EXPECT_OK(dut_->SdmmcRequest(&request, response));
  EXPECT_EQ(response[0], 0xfedc9876);

  const aml_sdmmc_desc_t* descs = descriptors();
  auto expected_desc_cfg = AmlSdmmcCmdCfg::Get()
                               .FromValue(0)
                               .set_len(511)
                               .set_block_mode(1)
                               .set_timeout(AmlSdmmcCmdCfg::kDefaultCmdTimeout)
                               .set_data_io(1)
                               .set_data_wr(0)
                               .set_resp_num(1)
                               .set_cmd_idx(SDMMC_READ_MULTIPLE_BLOCK)
                               .set_owner(1);

  EXPECT_EQ(descs[0].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[0].cmd_arg, 0x1234abcd);
  EXPECT_EQ(descs[0].data_addr, zx_system_get_page_size() + 16);
  EXPECT_EQ(descs[0].resp_addr, 0);

  expected_desc_cfg.set_len(2)
      .set_end_of_chain(1)
      .set_no_resp(1)
      .set_no_cmd(1)
      .set_resp_num(0)
      .set_cmd_idx(0);

  EXPECT_EQ(descs[1].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[1].cmd_arg, 0);
  EXPECT_EQ(descs[1].data_addr, zx_system_get_page_size() + (511 * 32) + 16);
  EXPECT_EQ(descs[1].resp_addr, 0);
}

TEST_F(AmlSdmmcWithBanjoTest, UnownedVmoSingleBufferNotPageAligned) {
  StartDriver(/*create_fake_bti_with_paddrs=*/true);
  const size_t pages = ((32 * 514) / zx_system_get_page_size()) + 1;
  InitializeNonContiguousPaddrs(pages);

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(pages * zx_system_get_page_size(), 0, &vmo));

  sdmmc_buffer_region_t buffer = {
      .buffer =
          {
              .vmo = vmo.get(),
          },
      .type = SDMMC_BUFFER_TYPE_VMO_HANDLE,
      .offset = 16,
      .size = 32 * 513,
  };

  sdmmc_req_t request = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 32,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers_list = &buffer,
      .buffers_count = 1,
  };
  uint32_t response[4] = {};
  AmlSdmmcCmdResp::Get().FromValue(0xfedc9876).WriteTo(&*mmio_);
  EXPECT_NOT_OK(dut_->SdmmcRequest(&request, response));
}

TEST_F(AmlSdmmcWithBanjoTest, UnownedVmoSingleBufferPageAligned) {
  StartDriver(/*create_fake_bti_with_paddrs=*/true);
  const size_t pages = ((32 * 514) / zx_system_get_page_size()) + 1;
  InitializeNonContiguousPaddrs(pages);

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(pages * zx_system_get_page_size(), 0, &vmo));

  sdmmc_buffer_region_t buffer = {
      .buffer =
          {
              .vmo = vmo.get(),
          },
      .type = SDMMC_BUFFER_TYPE_VMO_HANDLE,
      .offset = 32,
      .size = 32 * 513,
  };

  sdmmc_req_t request = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 32,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers_list = &buffer,
      .buffers_count = 1,
  };
  uint32_t response[4] = {};
  AmlSdmmcCmdResp::Get().FromValue(0xfedc9876).WriteTo(&*mmio_);
  EXPECT_OK(dut_->SdmmcRequest(&request, response));
  EXPECT_EQ(response[0], 0xfedc9876);

  const aml_sdmmc_desc_t* descs = descriptors();
  auto expected_desc_cfg = AmlSdmmcCmdCfg::Get()
                               .FromValue(0)
                               .set_len(127)
                               .set_block_mode(1)
                               .set_timeout(AmlSdmmcCmdCfg::kDefaultCmdTimeout)
                               .set_data_io(1)
                               .set_data_wr(0)
                               .set_resp_num(1)
                               .set_cmd_idx(SDMMC_READ_MULTIPLE_BLOCK)
                               .set_owner(1);

  EXPECT_EQ(descs[0].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[0].cmd_arg, 0x1234abcd);
  EXPECT_EQ(descs[0].data_addr, (zx_system_get_page_size() * 2) + 32);
  EXPECT_EQ(descs[0].resp_addr, 0);

  for (uint32_t i = 1; i < 5; i++) {
    expected_desc_cfg.set_len(128).set_no_resp(1).set_no_cmd(1).set_resp_num(0).set_cmd_idx(0);
    if (i == 4) {
      expected_desc_cfg.set_len(2).set_end_of_chain(1);
    }

    EXPECT_EQ(descs[i].cmd_info, expected_desc_cfg.reg_value());
    EXPECT_EQ(descs[i].cmd_arg, 0);
    EXPECT_EQ(descs[i].data_addr, zx_system_get_page_size() * (i + 1) * 2);
    EXPECT_EQ(descs[i].resp_addr, 0);
  }
}

TEST_F(AmlSdmmcWithBanjoTest, OwnedVmosBlockMode) {
  StartDriver(/*create_fake_bti_with_paddrs=*/true);
  InitializeContiguousPaddrs(10);

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  sdmmc_buffer_region_t buffers[10];
  for (uint32_t i = 0; i < std::size(buffers); i++) {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
    EXPECT_OK(dut_->SdmmcRegisterVmo(i, 0, std::move(vmo), i * 64, 512, SDMMC_VMO_RIGHT_WRITE));
    buffers[i] = {
        .buffer =
            {
                .vmo_id = i,
            },
        .type = SDMMC_BUFFER_TYPE_VMO_ID,
        .offset = i * 16,
        .size = 32 * (i + 2),
    };
  }

  zx::vmo vmo;
  EXPECT_NOT_OK(dut_->SdmmcUnregisterVmo(3, 1, &vmo));

  sdmmc_req_t request = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 32,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers_list = buffers,
      .buffers_count = std::size(buffers),
  };
  uint32_t response[4] = {};
  AmlSdmmcCmdResp::Get().FromValue(0xfedc9876).WriteTo(&*mmio_);
  EXPECT_OK(dut_->SdmmcRequest(&request, response));
  EXPECT_EQ(response[0], 0xfedc9876);

  const aml_sdmmc_desc_t* descs = descriptors();
  auto expected_desc_cfg = AmlSdmmcCmdCfg::Get()
                               .FromValue(0)
                               .set_len(2)
                               .set_block_mode(1)
                               .set_timeout(AmlSdmmcCmdCfg::kDefaultCmdTimeout)
                               .set_data_io(1)
                               .set_data_wr(0)
                               .set_resp_num(1)
                               .set_cmd_idx(SDMMC_READ_MULTIPLE_BLOCK)
                               .set_owner(1);

  EXPECT_EQ(descs[0].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[0].cmd_arg, 0x1234abcd);
  EXPECT_EQ(descs[0].data_addr, zx_system_get_page_size());
  EXPECT_EQ(descs[0].resp_addr, 0);

  for (uint32_t i = 1; i < std::size(buffers); i++) {
    expected_desc_cfg.set_len(i + 2).set_no_resp(1).set_no_cmd(1).set_resp_num(0).set_cmd_idx(0);
    if (i == std::size(buffers) - 1) {
      expected_desc_cfg.set_end_of_chain(1);
    }
    EXPECT_EQ(descs[i].cmd_info, expected_desc_cfg.reg_value());
    EXPECT_EQ(descs[i].cmd_arg, 0);
    EXPECT_EQ(descs[i].data_addr, (i << 24) | (zx_system_get_page_size() + (i * 80)));
    EXPECT_EQ(descs[i].resp_addr, 0);
  }

  request.client_id = 7;
  EXPECT_NOT_OK(dut_->SdmmcRequest(&request, response));

  EXPECT_OK(dut_->SdmmcUnregisterVmo(3, 0, &vmo));
  EXPECT_NOT_OK(dut_->SdmmcRegisterVmo(2, 0, std::move(vmo), 0, 512, SDMMC_VMO_RIGHT_WRITE));

  request.client_id = 0;
  EXPECT_NOT_OK(dut_->SdmmcRequest(&request, response));
}

TEST_F(AmlSdmmcWithBanjoTest, OwnedVmosNotBlockSizeMultiple) {
  StartDriver(/*create_fake_bti_with_paddrs=*/true);
  InitializeContiguousPaddrs(10);

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  sdmmc_buffer_region_t buffers[10];
  for (uint32_t i = 0; i < std::size(buffers); i++) {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
    EXPECT_OK(dut_->SdmmcRegisterVmo(i, 0, std::move(vmo), i * 64, 512, SDMMC_VMO_RIGHT_WRITE));
    buffers[i] = {
        .buffer =
            {
                .vmo_id = i,
            },
        .type = SDMMC_BUFFER_TYPE_VMO_ID,
        .offset = 0,
        .size = 32 * (i + 2),
    };
  }

  buffers[5].size = 25;

  sdmmc_req_t request = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 32,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers_list = buffers,
      .buffers_count = std::size(buffers),
  };
  uint32_t response[4] = {};
  EXPECT_NOT_OK(dut_->SdmmcRequest(&request, response));
}

TEST_F(AmlSdmmcWithBanjoTest, OwnedVmosByteMode) {
  StartDriver(/*create_fake_bti_with_paddrs=*/true);
  InitializeContiguousPaddrs(10);

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  sdmmc_buffer_region_t buffers[10];
  for (uint32_t i = 0; i < std::size(buffers); i++) {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
    EXPECT_OK(dut_->SdmmcRegisterVmo(i, 0, std::move(vmo), i * 64, 512, SDMMC_VMO_RIGHT_WRITE));
    buffers[i] = {
        .buffer =
            {
                .vmo_id = i,
            },
        .type = SDMMC_BUFFER_TYPE_VMO_ID,
        .offset = i * 4,
        .size = 50,
    };
  }

  sdmmc_req_t request = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 50,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers_list = buffers,
      .buffers_count = std::size(buffers),
  };
  uint32_t response[4] = {};
  AmlSdmmcCmdResp::Get().FromValue(0xfedc9876).WriteTo(&*mmio_);
  EXPECT_OK(dut_->SdmmcRequest(&request, response));
  EXPECT_EQ(response[0], 0xfedc9876);

  const aml_sdmmc_desc_t* descs = descriptors();
  auto expected_desc_cfg = AmlSdmmcCmdCfg::Get()
                               .FromValue(0)
                               .set_len(50)
                               .set_timeout(AmlSdmmcCmdCfg::kDefaultCmdTimeout)
                               .set_data_io(1)
                               .set_data_wr(0)
                               .set_resp_num(1)
                               .set_cmd_idx(SDMMC_READ_MULTIPLE_BLOCK)
                               .set_owner(1);

  EXPECT_EQ(descs[0].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[0].cmd_arg, 0x1234abcd);
  EXPECT_EQ(descs[0].data_addr, zx_system_get_page_size());
  EXPECT_EQ(descs[0].resp_addr, 0);

  for (uint32_t i = 1; i < std::size(buffers); i++) {
    expected_desc_cfg.set_len(50).set_no_resp(1).set_no_cmd(1).set_resp_num(0).set_cmd_idx(0);
    if (i == std::size(buffers) - 1) {
      expected_desc_cfg.set_end_of_chain(1);
    }
    EXPECT_EQ(descs[i].cmd_info, expected_desc_cfg.reg_value());
    EXPECT_EQ(descs[i].cmd_arg, 0);
    EXPECT_EQ(descs[i].data_addr, (i << 24) | (zx_system_get_page_size() + (i * 68)));
    EXPECT_EQ(descs[i].resp_addr, 0);
  }
}

TEST_F(AmlSdmmcWithBanjoTest, OwnedVmoByteModeMultiBlock) {
  StartDriver(/*create_fake_bti_with_paddrs=*/true);
  InitializeContiguousPaddrs(1);

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  EXPECT_OK(dut_->SdmmcRegisterVmo(1, 0, std::move(vmo), 0, 512, SDMMC_VMO_RIGHT_WRITE));

  sdmmc_buffer_region_t buffer = {
      .buffer =
          {
              .vmo_id = 1,
          },
      .type = SDMMC_BUFFER_TYPE_VMO_ID,
      .offset = 0,
      .size = 400,
  };

  sdmmc_req_t request = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 100,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers_list = &buffer,
      .buffers_count = 1,
  };
  uint32_t response[4] = {};
  AmlSdmmcCmdResp::Get().FromValue(0xfedc9876).WriteTo(&*mmio_);
  EXPECT_OK(dut_->SdmmcRequest(&request, response));
  EXPECT_EQ(response[0], 0xfedc9876);

  const aml_sdmmc_desc_t* descs = descriptors();
  auto expected_desc_cfg = AmlSdmmcCmdCfg::Get()
                               .FromValue(0)
                               .set_len(100)
                               .set_timeout(AmlSdmmcCmdCfg::kDefaultCmdTimeout)
                               .set_data_io(1)
                               .set_data_wr(0)
                               .set_resp_num(1)
                               .set_cmd_idx(SDMMC_READ_MULTIPLE_BLOCK)
                               .set_owner(1);

  EXPECT_EQ(descs[0].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[0].cmd_arg, 0x1234abcd);
  EXPECT_EQ(descs[0].data_addr, zx_system_get_page_size());
  EXPECT_EQ(descs[0].resp_addr, 0);

  for (uint32_t i = 1; i < 4; i++) {
    expected_desc_cfg.set_no_resp(1).set_no_cmd(1).set_resp_num(0).set_cmd_idx(0);
    if (i == 3) {
      expected_desc_cfg.set_end_of_chain(1);
    }
    EXPECT_EQ(descs[i].cmd_info, expected_desc_cfg.reg_value());
    EXPECT_EQ(descs[i].cmd_arg, 0);
    EXPECT_EQ(descs[i].data_addr, zx_system_get_page_size() + (i * 100));
    EXPECT_EQ(descs[i].resp_addr, 0);
  }
}

TEST_F(AmlSdmmcWithBanjoTest, OwnedVmoOffsetNotAligned) {
  StartDriver(/*create_fake_bti_with_paddrs=*/true);
  InitializeContiguousPaddrs(1);

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  EXPECT_OK(dut_->SdmmcRegisterVmo(1, 0, std::move(vmo), 2, 512, SDMMC_VMO_RIGHT_WRITE));

  sdmmc_buffer_region_t buffer = {
      .buffer =
          {
              .vmo_id = 1,
          },
      .type = SDMMC_BUFFER_TYPE_VMO_ID,
      .offset = 32,
      .size = 64,
  };

  sdmmc_req_t request = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 32,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers_list = &buffer,
      .buffers_count = 1,
  };
  uint32_t response[4] = {};
  AmlSdmmcCmdResp::Get().FromValue(0xfedc9876).WriteTo(&*mmio_);
  EXPECT_NOT_OK(dut_->SdmmcRequest(&request, response));
}

TEST_F(AmlSdmmcWithBanjoTest, OwnedVmoSingleBufferMultipleDescriptors) {
  StartDriver(/*create_fake_bti_with_paddrs=*/true);
  const size_t pages = ((32 * 514) / zx_system_get_page_size()) + 1;
  InitializeSingleVmoPaddrs(pages);

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(pages * zx_system_get_page_size(), 0, &vmo));
  EXPECT_OK(dut_->SdmmcRegisterVmo(1, 0, std::move(vmo), 8, (pages * zx_system_get_page_size()) - 8,
                                   SDMMC_VMO_RIGHT_WRITE));

  sdmmc_buffer_region_t buffer = {
      .buffer =
          {
              .vmo_id = 1,
          },
      .type = SDMMC_BUFFER_TYPE_VMO_ID,
      .offset = 8,
      .size = 32 * 513,
  };

  sdmmc_req_t request = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 32,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers_list = &buffer,
      .buffers_count = 1,
  };
  uint32_t response[4] = {};
  AmlSdmmcCmdResp::Get().FromValue(0xfedc9876).WriteTo(&*mmio_);
  EXPECT_OK(dut_->SdmmcRequest(&request, response));
  EXPECT_EQ(response[0], 0xfedc9876);

  const aml_sdmmc_desc_t* descs = descriptors();
  auto expected_desc_cfg = AmlSdmmcCmdCfg::Get()
                               .FromValue(0)
                               .set_len(511)
                               .set_block_mode(1)
                               .set_timeout(AmlSdmmcCmdCfg::kDefaultCmdTimeout)
                               .set_data_io(1)
                               .set_data_wr(0)
                               .set_resp_num(1)
                               .set_cmd_idx(SDMMC_READ_MULTIPLE_BLOCK)
                               .set_owner(1);

  EXPECT_EQ(descs[0].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[0].cmd_arg, 0x1234abcd);
  EXPECT_EQ(descs[0].data_addr, zx_system_get_page_size() + 16);
  EXPECT_EQ(descs[0].resp_addr, 0);

  expected_desc_cfg.set_len(1)
      .set_len(2)
      .set_end_of_chain(1)
      .set_no_resp(1)
      .set_no_cmd(1)
      .set_resp_num(0)
      .set_cmd_idx(0);

  EXPECT_EQ(descs[1].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[1].cmd_arg, 0);
  EXPECT_EQ(descs[1].data_addr, zx_system_get_page_size() + (511 * 32) + 16);
  EXPECT_EQ(descs[1].resp_addr, 0);
}

TEST_F(AmlSdmmcWithBanjoTest, OwnedVmoSingleBufferNotPageAligned) {
  StartDriver(/*create_fake_bti_with_paddrs=*/true);
  const size_t pages = ((32 * 514) / zx_system_get_page_size()) + 1;
  InitializeNonContiguousPaddrs(pages);

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(pages * zx_system_get_page_size(), 0, &vmo));
  EXPECT_OK(dut_->SdmmcRegisterVmo(1, 0, std::move(vmo), 8, (pages * zx_system_get_page_size()) - 8,
                                   SDMMC_VMO_RIGHT_WRITE));

  sdmmc_buffer_region_t buffer = {
      .buffer =
          {
              .vmo = 1,
          },
      .type = SDMMC_BUFFER_TYPE_VMO_ID,
      .offset = 8,
      .size = 32 * 513,
  };

  sdmmc_req_t request = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 32,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers_list = &buffer,
      .buffers_count = 1,
  };
  uint32_t response[4] = {};
  AmlSdmmcCmdResp::Get().FromValue(0xfedc9876).WriteTo(&*mmio_);
  EXPECT_NOT_OK(dut_->SdmmcRequest(&request, response));
}

TEST_F(AmlSdmmcWithBanjoTest, OwnedVmoSingleBufferPageAligned) {
  StartDriver(/*create_fake_bti_with_paddrs=*/true);
  const size_t pages = ((32 * 514) / zx_system_get_page_size()) + 1;
  InitializeNonContiguousPaddrs(pages);

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(pages * zx_system_get_page_size(), 0, &vmo));
  EXPECT_OK(dut_->SdmmcRegisterVmo(
      1, 0, std::move(vmo), 16, (pages * zx_system_get_page_size()) - 16, SDMMC_VMO_RIGHT_WRITE));

  sdmmc_buffer_region_t buffer = {
      .buffer =
          {
              .vmo = 1,
          },
      .type = SDMMC_BUFFER_TYPE_VMO_ID,
      .offset = 16,
      .size = 32 * 513,
  };

  sdmmc_req_t request = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 32,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers_list = &buffer,
      .buffers_count = 1,
  };
  uint32_t response[4] = {};
  AmlSdmmcCmdResp::Get().FromValue(0xfedc9876).WriteTo(&*mmio_);
  EXPECT_OK(dut_->SdmmcRequest(&request, response));
  EXPECT_EQ(response[0], 0xfedc9876);

  const aml_sdmmc_desc_t* descs = descriptors();
  auto expected_desc_cfg = AmlSdmmcCmdCfg::Get()
                               .FromValue(0)
                               .set_len(127)
                               .set_block_mode(1)
                               .set_timeout(AmlSdmmcCmdCfg::kDefaultCmdTimeout)
                               .set_data_io(1)
                               .set_data_wr(0)
                               .set_resp_num(1)
                               .set_cmd_idx(SDMMC_READ_MULTIPLE_BLOCK)
                               .set_owner(1);

  EXPECT_EQ(descs[0].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[0].cmd_arg, 0x1234abcd);
  EXPECT_EQ(descs[0].data_addr, (zx_system_get_page_size() * 2) + 32);
  EXPECT_EQ(descs[0].resp_addr, 0);

  for (uint32_t i = 1; i < 5; i++) {
    expected_desc_cfg.set_len(128).set_no_resp(1).set_no_cmd(1).set_resp_num(0).set_cmd_idx(0);
    if (i == 4) {
      expected_desc_cfg.set_len(2).set_end_of_chain(1);
    }

    EXPECT_EQ(descs[i].cmd_info, expected_desc_cfg.reg_value());
    EXPECT_EQ(descs[i].cmd_arg, 0);
    EXPECT_EQ(descs[i].data_addr, zx_system_get_page_size() * (i + 1) * 2);
    EXPECT_EQ(descs[i].resp_addr, 0);
  }
}

TEST_F(AmlSdmmcWithBanjoTest, OwnedVmoWritePastEnd) {
  StartDriver(/*create_fake_bti_with_paddrs=*/true);
  const size_t pages = ((32 * 514) / zx_system_get_page_size()) + 1;
  InitializeNonContiguousPaddrs(pages);

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(pages * zx_system_get_page_size(), 0, &vmo));
  EXPECT_OK(dut_->SdmmcRegisterVmo(1, 0, std::move(vmo), 32, 32 * 384, SDMMC_VMO_RIGHT_WRITE));

  sdmmc_buffer_region_t buffer = {
      .buffer =
          {
              .vmo = 1,
          },
      .type = SDMMC_BUFFER_TYPE_VMO_ID,
      .offset = 32,
      .size = 32 * 383,
  };

  sdmmc_req_t request = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 32,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers_list = &buffer,
      .buffers_count = 1,
  };
  uint32_t response[4] = {};
  AmlSdmmcCmdResp::Get().FromValue(0xfedc9876).WriteTo(&*mmio_);
  EXPECT_OK(dut_->SdmmcRequest(&request, response));
  EXPECT_EQ(response[0], 0xfedc9876);

  const aml_sdmmc_desc_t* descs = descriptors();
  auto expected_desc_cfg = AmlSdmmcCmdCfg::Get()
                               .FromValue(0)
                               .set_len(126)
                               .set_block_mode(1)
                               .set_timeout(AmlSdmmcCmdCfg::kDefaultCmdTimeout)
                               .set_data_io(1)
                               .set_data_wr(0)
                               .set_resp_num(1)
                               .set_cmd_idx(SDMMC_READ_MULTIPLE_BLOCK)
                               .set_owner(1);

  EXPECT_EQ(descs[0].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[0].cmd_arg, 0x1234abcd);
  EXPECT_EQ(descs[0].data_addr, (zx_system_get_page_size() * 2) + 64);
  EXPECT_EQ(descs[0].resp_addr, 0);

  for (uint32_t i = 1; i < 4; i++) {
    expected_desc_cfg.set_len(128).set_no_resp(1).set_no_cmd(1).set_resp_num(0).set_cmd_idx(0);
    if (i == 3) {
      expected_desc_cfg.set_len(1).set_end_of_chain(1);
    }

    EXPECT_EQ(descs[i].cmd_info, expected_desc_cfg.reg_value());
    EXPECT_EQ(descs[i].cmd_arg, 0);
    EXPECT_EQ(descs[i].data_addr, zx_system_get_page_size() * (i + 1) * 2);
    EXPECT_EQ(descs[i].resp_addr, 0);
  }

  buffer.size = 32 * 384;
  EXPECT_NOT_OK(dut_->SdmmcRequest(&request, response));
}

TEST_F(AmlSdmmcWithBanjoTest, SeparateClientVmoSpaces) {
  StartDriver();

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  const zx_koid_t vmo1_koid = GetVmoKoid(vmo);
  EXPECT_NE(vmo1_koid, ZX_KOID_INVALID);
  EXPECT_OK(dut_->SdmmcRegisterVmo(1, 0, std::move(vmo), 0, zx_system_get_page_size(),
                                   SDMMC_VMO_RIGHT_WRITE));

  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  const zx_koid_t vmo2_koid = GetVmoKoid(vmo);
  EXPECT_NE(vmo2_koid, ZX_KOID_INVALID);
  EXPECT_OK(dut_->SdmmcRegisterVmo(2, 0, std::move(vmo), 0, zx_system_get_page_size(),
                                   SDMMC_VMO_RIGHT_WRITE));

  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  EXPECT_NOT_OK(dut_->SdmmcRegisterVmo(1, 0, std::move(vmo), 0, zx_system_get_page_size(),
                                       SDMMC_VMO_RIGHT_WRITE));

  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  EXPECT_NOT_OK(dut_->SdmmcRegisterVmo(1, 8, std::move(vmo), 0, zx_system_get_page_size(),
                                       SDMMC_VMO_RIGHT_WRITE));

  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  const zx_koid_t vmo3_koid = GetVmoKoid(vmo);
  EXPECT_NE(vmo3_koid, ZX_KOID_INVALID);
  EXPECT_OK(dut_->SdmmcRegisterVmo(1, 1, std::move(vmo), 0, zx_system_get_page_size(),
                                   SDMMC_VMO_RIGHT_WRITE));

  EXPECT_OK(dut_->SdmmcUnregisterVmo(1, 0, &vmo));
  EXPECT_EQ(GetVmoKoid(vmo), vmo1_koid);

  EXPECT_OK(dut_->SdmmcUnregisterVmo(2, 0, &vmo));
  EXPECT_EQ(GetVmoKoid(vmo), vmo2_koid);

  EXPECT_OK(dut_->SdmmcUnregisterVmo(1, 1, &vmo));
  EXPECT_EQ(GetVmoKoid(vmo), vmo3_koid);

  EXPECT_NOT_OK(dut_->SdmmcUnregisterVmo(1, 0, &vmo));
  EXPECT_NOT_OK(dut_->SdmmcUnregisterVmo(2, 0, &vmo));
  EXPECT_NOT_OK(dut_->SdmmcUnregisterVmo(1, 1, &vmo));
}

TEST_F(AmlSdmmcWithBanjoTest, RequestWithOwnedAndUnownedVmos) {
  StartDriver(/*create_fake_bti_with_paddrs=*/true);
  InitializeContiguousPaddrs(10);

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  zx::vmo vmos[5] = {};
  sdmmc_buffer_region_t buffers[10];
  for (uint32_t i = 0; i < 5; i++) {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmos[i]));

    EXPECT_OK(dut_->SdmmcRegisterVmo(i, 0, std::move(vmo), i * 64, 512, SDMMC_VMO_RIGHT_WRITE));
    buffers[i * 2] = {
        .buffer =
            {
                .vmo_id = i,
            },
        .type = SDMMC_BUFFER_TYPE_VMO_ID,
        .offset = i * 16,
        .size = 32 * (i + 2),
    };
    buffers[(i * 2) + 1] = {
        .buffer =
            {
                .vmo = vmos[i].get(),
            },
        .type = SDMMC_BUFFER_TYPE_VMO_HANDLE,
        .offset = i * 16,
        .size = 32 * (i + 2),
    };
  }

  zx::vmo vmo;
  EXPECT_NOT_OK(dut_->SdmmcUnregisterVmo(3, 1, &vmo));

  sdmmc_req_t request = {
      .cmd_idx = SDMMC_READ_MULTIPLE_BLOCK,
      .cmd_flags = SDMMC_READ_MULTIPLE_BLOCK_FLAGS,
      .arg = 0x1234abcd,
      .blocksize = 32,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers_list = buffers,
      .buffers_count = std::size(buffers),
  };
  uint32_t response[4] = {};
  AmlSdmmcCmdResp::Get().FromValue(0xfedc9876).WriteTo(&*mmio_);
  EXPECT_OK(dut_->SdmmcRequest(&request, response));
  EXPECT_EQ(response[0], 0xfedc9876);

  const aml_sdmmc_desc_t* descs = descriptors();
  auto expected_desc_cfg = AmlSdmmcCmdCfg::Get()
                               .FromValue(0)
                               .set_len(2)
                               .set_block_mode(1)
                               .set_timeout(AmlSdmmcCmdCfg::kDefaultCmdTimeout)
                               .set_data_io(1)
                               .set_data_wr(0)
                               .set_resp_num(1)
                               .set_cmd_idx(SDMMC_READ_MULTIPLE_BLOCK)
                               .set_owner(1);

  EXPECT_EQ(descs[0].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[0].cmd_arg, 0x1234abcd);
  EXPECT_EQ(descs[0].data_addr, zx_system_get_page_size());
  EXPECT_EQ(descs[0].resp_addr, 0);

  expected_desc_cfg.set_no_resp(1).set_no_cmd(1).set_resp_num(0).set_cmd_idx(0);
  EXPECT_EQ(descs[1].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[1].cmd_arg, 0);
  EXPECT_EQ(descs[1].data_addr, (5 << 24) | zx_system_get_page_size());
  EXPECT_EQ(descs[1].resp_addr, 0);

  expected_desc_cfg.set_len(3);
  EXPECT_EQ(descs[2].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[2].cmd_arg, 0);
  EXPECT_EQ(descs[2].data_addr, (1 << 24) | (zx_system_get_page_size() + 64 + 16));
  EXPECT_EQ(descs[2].resp_addr, 0);

  EXPECT_EQ(descs[3].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[3].cmd_arg, 0);
  EXPECT_EQ(descs[3].data_addr, (6 << 24) | (zx_system_get_page_size() + 16));
  EXPECT_EQ(descs[3].resp_addr, 0);

  expected_desc_cfg.set_len(4);
  EXPECT_EQ(descs[4].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[4].cmd_arg, 0);
  EXPECT_EQ(descs[4].data_addr, (2 << 24) | (zx_system_get_page_size() + 128 + 32));
  EXPECT_EQ(descs[4].resp_addr, 0);

  EXPECT_EQ(descs[5].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[5].cmd_arg, 0);
  EXPECT_EQ(descs[5].data_addr, (7 << 24) | (zx_system_get_page_size() + 32));
  EXPECT_EQ(descs[5].resp_addr, 0);

  expected_desc_cfg.set_len(5);
  EXPECT_EQ(descs[6].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[6].cmd_arg, 0);
  EXPECT_EQ(descs[6].data_addr, (3 << 24) | (zx_system_get_page_size() + 192 + 48));
  EXPECT_EQ(descs[6].resp_addr, 0);

  EXPECT_EQ(descs[7].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[7].cmd_arg, 0);
  EXPECT_EQ(descs[7].data_addr, (8 << 24) | (zx_system_get_page_size() + 48));
  EXPECT_EQ(descs[7].resp_addr, 0);

  expected_desc_cfg.set_len(6);
  EXPECT_EQ(descs[8].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[8].cmd_arg, 0);
  EXPECT_EQ(descs[8].data_addr, (4 << 24) | (zx_system_get_page_size() + 256 + 64));
  EXPECT_EQ(descs[8].resp_addr, 0);

  expected_desc_cfg.set_end_of_chain(1);
  EXPECT_EQ(descs[9].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[9].cmd_arg, 0);
  EXPECT_EQ(descs[9].data_addr, (9 << 24) | (zx_system_get_page_size() + 64));
  EXPECT_EQ(descs[9].resp_addr, 0);
}

TEST_F(AmlSdmmcWithBanjoTest, ResetCmdInfoBits) {
  StartDriver(/*create_fake_bti_with_paddrs=*/true);

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  const std::array<zx_paddr_t, 3> paddrs = {
      0x1897'7000,
      0x1997'8000,
      0x1997'e000,
  };
  ASSERT_OK(fake_bti::SetPaddrs(zx::unowned_bti(bti_), paddrs));

  // Make sure the appropriate cmd_info bits get cleared.
  descriptors()[0].cmd_info = 0xffff'ffff;
  descriptors()[1].cmd_info = 0xffff'ffff;
  descriptors()[2].cmd_info = 0xffff'ffff;

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size() * 3, 0, &vmo));
  EXPECT_OK(dut_->SdmmcRegisterVmo(1, 2, std::move(vmo), 0, zx_system_get_page_size() * 3,
                                   SDMMC_VMO_RIGHT_WRITE));

  sdmmc_buffer_region_t buffer = {
      .buffer = {.vmo_id = 1},
      .type = SDMMC_BUFFER_TYPE_VMO_ID,
      .offset = 0,
      .size = 10752,
  };

  sdmmc_req_t request = {
      .cmd_idx = SDIO_IO_RW_DIRECT_EXTENDED,
      .cmd_flags = SDIO_IO_RW_DIRECT_EXTENDED_FLAGS | SDMMC_CMD_READ,
      .arg = 0x29000015,
      .blocksize = 512,
      .suppress_error_messages = false,
      .client_id = 2,
      .buffers_list = &buffer,
      .buffers_count = 1,
  };
  uint32_t response[4] = {};
  AmlSdmmcCfg::Get().ReadFrom(&*mmio_).set_blk_len(0).WriteTo(&*mmio_);
  EXPECT_OK(dut_->SdmmcRequest(&request, response));
  EXPECT_EQ(AmlSdmmcCfg::Get().ReadFrom(&*mmio_).blk_len(), 9);

  const aml_sdmmc_desc_t* descs = descriptors();
  auto expected_desc_cfg = AmlSdmmcCmdCfg::Get()
                               .FromValue(0)
                               .set_len(8)
                               .set_block_mode(1)
                               .set_timeout(AmlSdmmcCmdCfg::kDefaultCmdTimeout)
                               .set_data_io(1)
                               .set_data_wr(0)
                               .set_resp_num(1)
                               .set_cmd_idx(SDIO_IO_RW_DIRECT_EXTENDED)
                               .set_owner(1);

  EXPECT_EQ(descs[0].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[0].cmd_arg, 0x29000015);
  EXPECT_EQ(descs[0].data_addr, 0x1897'7000);
  EXPECT_EQ(descs[0].resp_addr, 0);

  expected_desc_cfg.set_no_resp(1).set_no_cmd(1).set_resp_num(0).set_cmd_idx(0);
  EXPECT_EQ(descs[1].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[1].cmd_arg, 0);
  EXPECT_EQ(descs[1].data_addr, 0x1997'8000);
  EXPECT_EQ(descs[1].resp_addr, 0);

  expected_desc_cfg.set_len(5).set_end_of_chain(1);
  EXPECT_EQ(descs[2].cmd_info, expected_desc_cfg.reg_value());
  EXPECT_EQ(descs[2].cmd_arg, 0);
  EXPECT_EQ(descs[2].data_addr, 0x1997'e000);
  EXPECT_EQ(descs[2].resp_addr, 0);
}

TEST_F(AmlSdmmcWithBanjoTest, WriteToReadOnlyVmo) {
  StartDriver(/*create_fake_bti_with_paddrs=*/true);
  InitializeContiguousPaddrs(10);

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  sdmmc_buffer_region_t buffers[10];
  for (uint32_t i = 0; i < std::size(buffers); i++) {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
    const uint32_t vmo_rights = SDMMC_VMO_RIGHT_READ | (i == 5 ? 0 : SDMMC_VMO_RIGHT_WRITE);
    EXPECT_OK(dut_->SdmmcRegisterVmo(i, 0, std::move(vmo), i * 64, 512, vmo_rights));
    buffers[i] = {
        .buffer =
            {
                .vmo_id = i,
            },
        .type = SDMMC_BUFFER_TYPE_VMO_ID,
        .offset = 0,
        .size = 32 * (i + 2),
    };
  }

  sdmmc_req_t request = {
      .cmd_idx = SDIO_IO_RW_DIRECT_EXTENDED,
      .cmd_flags = SDIO_IO_RW_DIRECT_EXTENDED_FLAGS | SDMMC_CMD_READ,
      .arg = 0x29000015,
      .blocksize = 32,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers_list = buffers,
      .buffers_count = std::size(buffers),
  };
  uint32_t response[4] = {};
  EXPECT_NOT_OK(dut_->SdmmcRequest(&request, response));
}

TEST_F(AmlSdmmcWithBanjoTest, ReadFromWriteOnlyVmo) {
  StartDriver(/*create_fake_bti_with_paddrs=*/true);
  InitializeContiguousPaddrs(10);

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  sdmmc_buffer_region_t buffers[10];
  for (uint32_t i = 0; i < std::size(buffers); i++) {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
    const uint32_t vmo_rights = SDMMC_VMO_RIGHT_WRITE | (i == 5 ? 0 : SDMMC_VMO_RIGHT_READ);
    EXPECT_OK(dut_->SdmmcRegisterVmo(i, 0, std::move(vmo), i * 64, 512, vmo_rights));
    buffers[i] = {
        .buffer =
            {
                .vmo_id = i,
            },
        .type = SDMMC_BUFFER_TYPE_VMO_ID,
        .offset = 0,
        .size = 32 * (i + 2),
    };
  }

  sdmmc_req_t request = {
      .cmd_idx = SDIO_IO_RW_DIRECT_EXTENDED,
      .cmd_flags = SDIO_IO_RW_DIRECT_EXTENDED_FLAGS,
      .arg = 0x29000015,
      .blocksize = 32,
      .suppress_error_messages = false,
      .client_id = 0,
      .buffers_list = buffers,
      .buffers_count = std::size(buffers),
  };
  uint32_t response[4] = {};
  EXPECT_NOT_OK(dut_->SdmmcRequest(&request, response));
}

TEST_F(AmlSdmmcWithBanjoTest, ConsecutiveErrorLogging) {
  StartDriver();

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  // First data error.
  dut_->SetRequestInterruptStatus(1 << 8);
  sdmmc_req_t request;
  memset(&request, 0, sizeof(request));
  uint32_t unused_response[4];
  EXPECT_EQ(ZX_ERR_IO_DATA_INTEGRITY, dut_->SdmmcRequest(&request, unused_response));

  // First cmd error.
  dut_->SetRequestInterruptStatus(1 << 11);
  memset(&request, 0, sizeof(request));
  EXPECT_EQ(ZX_ERR_TIMED_OUT, dut_->SdmmcRequest(&request, unused_response));

  // Second data error.
  dut_->SetRequestInterruptStatus(1 << 7);
  memset(&request, 0, sizeof(request));
  EXPECT_EQ(ZX_ERR_IO_DATA_INTEGRITY, dut_->SdmmcRequest(&request, unused_response));

  // Second cmd error.
  dut_->SetRequestInterruptStatus(1 << 11);
  memset(&request, 0, sizeof(request));
  EXPECT_EQ(ZX_ERR_TIMED_OUT, dut_->SdmmcRequest(&request, unused_response));

  zx::vmo vmo;
  EXPECT_OK(zx::vmo::create(32, 0, &vmo));

  // cmd/data goes through.
  const sdmmc_buffer_region_t region{
      .buffer = {.vmo = vmo.get()},
      .type = SDMMC_BUFFER_TYPE_VMO_HANDLE,
      .offset = 0,
      .size = 32,
  };
  dut_->SetRequestInterruptStatus(1 << 13);
  memset(&request, 0, sizeof(request));
  request.cmd_flags = SDMMC_RESP_DATA_PRESENT;  // Must be set to clear the data error count.
  request.blocksize = 32;
  request.buffers_list = &region;
  request.buffers_count = 1;
  EXPECT_OK(dut_->SdmmcRequest(&request, unused_response));

  // Third data error.
  dut_->SetRequestInterruptStatus(1 << 7);
  memset(&request, 0, sizeof(request));
  EXPECT_EQ(ZX_ERR_IO_DATA_INTEGRITY, dut_->SdmmcRequest(&request, unused_response));

  // Third cmd error.
  dut_->SetRequestInterruptStatus(1 << 11);
  memset(&request, 0, sizeof(request));
  EXPECT_EQ(ZX_ERR_TIMED_OUT, dut_->SdmmcRequest(&request, unused_response));
}

TEST_F(AmlSdmmcWithBanjoTest, PowerSuspendResume) {
  StartDriver(/*create_fake_bti_with_paddrs=*/false, /*supply_power_framework=*/true);

  auto clock = AmlSdmmcClock::Get().FromValue(0).WriteTo(&*mmio_);

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  // Set power level to kPowerLevelOn first to satisfy Suspendable's first_activation_occurred_.
  driver_test().RunInEnvironmentTypeContext([](AmlSdmmcTestEnvironment& env) {
    env.power_broker()
        .hardware_power_element_runner_client_->SetLevel(AmlSdmmc::kPowerLevelOn)
        .ThenExactlyOnce([&](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
          EXPECT_TRUE(result.is_ok());
        });
  });
  driver_test().runtime().PerformBlockingWork([&] {
    bool clock_enabled;
    do {
      clock_enabled = IsClockEnabled();
    } while (!clock_enabled);
  });

  // Transition element to off to set up our initial state.
  driver_test().RunInEnvironmentTypeContext([](AmlSdmmcTestEnvironment& env) {
    env.power_broker()
        .hardware_power_element_runner_client_->SetLevel(AmlSdmmc::kPowerLevelOff)
        .ThenExactlyOnce([&](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
          EXPECT_TRUE(result.is_ok());
        });
  });
  driver_test().runtime().PerformBlockingWork([&] {
    bool clock_enabled;
    do {
      clock_enabled = IsClockEnabled();
    } while (clock_enabled);
  });

  dut_->ExpectInspectBoolPropertyValue("power_suspended", true);
  EXPECT_EQ(clock.ReadFrom(&*mmio_).cfg_div(), 0);
  EXPECT_FALSE(IsClockEnabled());

  // Trigger power level change to kPowerLevelOn.
  driver_test().RunInEnvironmentTypeContext([](AmlSdmmcTestEnvironment& env) {
    env.power_broker()
        .hardware_power_element_runner_client_->SetLevel(AmlSdmmc::kPowerLevelOn)
        .ThenExactlyOnce([&](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
          EXPECT_TRUE(result.is_ok());
        });
  });
  driver_test().runtime().PerformBlockingWork([&] {
    bool clock_enabled;
    do {
      clock_enabled = IsClockEnabled();
    } while (!clock_enabled);
  });

  dut_->ExpectInspectBoolPropertyValue("power_suspended", false);
  EXPECT_NE(clock.ReadFrom(&*mmio_).cfg_div(), 0);
  EXPECT_TRUE(IsClockEnabled());

  // Trigger power level change to kPowerLevelOff.
  driver_test().RunInEnvironmentTypeContext([](AmlSdmmcTestEnvironment& env) {
    env.power_broker()
        .hardware_power_element_runner_client_->SetLevel(AmlSdmmc::kPowerLevelOff)
        .ThenExactlyOnce([&](fidl::Result<fuchsia_power_broker::ElementRunner::SetLevel> result) {
          EXPECT_TRUE(result.is_ok());
        });
  });
  driver_test().runtime().PerformBlockingWork([&] {
    bool clock_enabled;
    do {
      clock_enabled = IsClockEnabled();
    } while (clock_enabled);
  });

  dut_->ExpectInspectBoolPropertyValue("power_suspended", true);
  EXPECT_EQ(clock.ReadFrom(&*mmio_).cfg_div(), 0);
  EXPECT_FALSE(IsClockEnabled());
}

TEST_F(AmlSdmmcWithBanjoTest, PowerTokenProvider) {
  StartDriver(/*create_fake_bti_with_paddrs=*/false, /*supply_power_framework=*/true);

  ASSERT_OK(dut_->Init(TestAmlSdmmcWithBanjo::kInstance));

  auto client_end =
      component::ConnectAtMember<fuchsia_hardware_power::PowerTokenService::TokenProvider>(
          CreateDriverSvcClient(), component::kDefaultInstance);
  ASSERT_OK(client_end);
  ASSERT_TRUE(client_end.value().is_valid());

  driver_test().runtime().PerformBlockingWork([&] {
    auto get_token = fidl::WireCall(client_end.value())->GetToken();
    ASSERT_OK(get_token);
    ASSERT_TRUE(get_token->is_ok());
    EXPECT_TRUE(get_token.value()->handle.is_valid());
  });
}

}  // namespace aml_sdmmc

FUCHSIA_DRIVER_EXPORT2(aml_sdmmc::TestAmlSdmmcWithBanjo);
