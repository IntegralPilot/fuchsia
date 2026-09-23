// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-power.h"

#include <fidl/fuchsia.hardware.pwm/cpp/wire_test_base.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/fake-vreg/cpp/fake-vreg.h>
#include <lib/driver/testing/cpp/driver_test.h>

#include <memory>
#include <optional>
#include <utility>

#include <bind/fuchsia/amlogic/platform/cpp/bind.h>
#include <gtest/gtest.h>
#include <soc/aml-common/aml-pwm-regs.h>

#include "src/lib/testing/predicates/status.h"

namespace {

bool operator==(const fuchsia_hardware_pwm::wire::PwmConfig& lhs,
                const fuchsia_hardware_pwm::wire::PwmConfig& rhs) {
  return (lhs.polarity == rhs.polarity) && (lhs.period_ns == rhs.period_ns) &&
         (lhs.duty_cycle == rhs.duty_cycle) && (lhs.mode_config.size() == rhs.mode_config.size()) &&
         (reinterpret_cast<aml_pwm::mode_config*>(lhs.mode_config.data())->mode ==
          reinterpret_cast<aml_pwm::mode_config*>(rhs.mode_config.data())->mode);
}

}  // namespace

namespace power::test {

class FakePwmServer final : public fidl::testing::WireTestBase<fuchsia_hardware_pwm::Pwm> {
 public:
  void SetConfig(SetConfigRequestView request, SetConfigCompleter::Sync& completer) override {
    ASSERT_TRUE(expect_configs_.size() > 0);
    const auto& expect_config = expect_configs_.front();

    ASSERT_TRUE(request->config == expect_config);

    expect_configs_.pop_front();
    mode_config_buffers_.pop_front();
    completer.ReplySuccess();
  }
  void Enable(EnableCompleter::Sync& completer) override { completer.ReplySuccess(); }

  void ExpectSetConfig(fuchsia_hardware_pwm::wire::PwmConfig config) {
    std::unique_ptr<uint8_t[]> mode_config = std::make_unique<uint8_t[]>(config.mode_config.size());
    memcpy(mode_config.get(), config.mode_config.data(), config.mode_config.size());
    auto copy = config;
    copy.mode_config =
        fidl::VectorView<uint8_t>::FromExternal(mode_config.get(), config.mode_config.size());
    expect_configs_.push_back(std::move(copy));
    mode_config_buffers_.push_back(std::move(mode_config));
  }

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  fuchsia_hardware_pwm::Service::InstanceHandler CreateInstanceHandler() {
    return fuchsia_hardware_pwm::Service::InstanceHandler{{
        .pwm = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                       fidl::kIgnoreBindingClosure),
    }};
  }

  void VerifyAndClear() {
    ASSERT_TRUE(expect_configs_.size() == 0);
    ASSERT_TRUE(mode_config_buffers_.size() == 0);
  }

 private:
  std::list<fuchsia_hardware_pwm::wire::PwmConfig> expect_configs_;
  std::list<std::unique_ptr<uint8_t[]>> mode_config_buffers_;
  fidl::ServerBindingGroup<fuchsia_hardware_pwm::Pwm> bindings_;
};

class AmlPowerTestEnvironment : public fdf_testing::Environment {
 public:
  enum class Mode : uint8_t {
    kAstro,
    kVim3,
  };

  void InitAstro(const fuchsia_hardware_amlogic_metadata::PowerMetadata& metadata) {
    pdev_.AddFidlMetadata(fuchsia_hardware_amlogic_metadata::PowerMetadata::kSerializableName,
                          metadata);
    mode_ = Mode::kAstro;
  }

  void InitVim3() { mode_ = Mode::kVim3; }

  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    auto* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();

    device_server_.Initialize(component::kDefaultInstance, std::nullopt);
    zx_status_t status = device_server_.Serve(dispatcher, &to_driver_vfs);
    if (status != ZX_OK) {
      return zx::error(status);
    }

    {
      zx::result result = to_driver_vfs.AddService<fuchsia_hardware_platform_device::Service>(
          pdev_.GetInstanceHandler(dispatcher));
      if (result.is_error()) {
        return result.take_error();
      }
    }

    switch (mode_) {
      case Mode::kAstro:
        return ServeAstro(to_driver_vfs);
      case Mode::kVim3:
        return ServeVim3(to_driver_vfs);
    }
  }

  void Shutdown() { primary_cluster_pwm_.VerifyAndClear(); }

  FakePwmServer& primary_cluster_pwm() { return primary_cluster_pwm_; }
  fdf_fake::FakeVreg& big_cluster_vreg() { return big_cluster_vreg_; }
  fdf_fake::FakeVreg& little_cluster_vreg() { return little_cluster_vreg_; }

 private:
  zx::result<> ServeAstro(fdf::OutgoingDirectory& to_driver_vfs) {
    zx::result result = to_driver_vfs.AddService<fuchsia_hardware_pwm::Service>(
        primary_cluster_pwm_.CreateInstanceHandler(), AmlPower::kPwmPrimaryParentName);
    if (result.is_error()) {
      return result.take_error();
    }

    return zx::ok();
  }

  zx::result<> ServeVim3(fdf::OutgoingDirectory& to_driver_vfs) {
    zx::result result = to_driver_vfs.AddService<fuchsia_hardware_vreg::Service>(
        big_cluster_vreg_.CreateInstanceHandler(), AmlPower::kVregPwmBigParentName);
    if (result.is_error()) {
      return result.take_error();
    }

    result = to_driver_vfs.AddService<fuchsia_hardware_vreg::Service>(
        little_cluster_vreg_.CreateInstanceHandler(), AmlPower::kVregPwmLittleParentName);
    if (result.is_error()) {
      return result.take_error();
    }

    return zx::ok();
  }

  Mode mode_;

  // Mmio Regs and Regions
  FakePwmServer primary_cluster_pwm_;
  fdf_fake::FakeVreg big_cluster_vreg_;
  fdf_fake::FakeVreg little_cluster_vreg_;

  compat::DeviceServer device_server_;
  fdf_fake::FakePDev pdev_;
};

class FixtureConfig final {
 public:
  using DriverType = AmlPower;
  using EnvironmentType = AmlPowerTestEnvironment;
};

class AmlPowerTest : public ::testing::Test {
 public:
  void TearDown() override {
    ASSERT_OK(driver_test_.StopDriver());
    driver_test_.RunInEnvironmentTypeContext([](auto& env) { env.Shutdown(); });
  }

 protected:
  static const fuchsia_hardware_amlogic_metadata::PowerMetadata kTestMetadata;

  void StartDriverAstro(
      const fuchsia_hardware_amlogic_metadata::PowerMetadata& metadata = kTestMetadata) {
    driver_test_.RunInEnvironmentTypeContext([&](auto& env) { env.InitAstro(metadata); });
    StartDriver(
        std::vector{MakeOffer<fuchsia_hardware_pwm::Service>(AmlPower::kPwmPrimaryParentName)});
  }

  void StartDriverVim3() {
    driver_test_.RunInEnvironmentTypeContext([&](auto& env) { env.InitVim3(); });
    StartDriver(
        std::vector{MakeOffer<fuchsia_hardware_vreg::Service>(AmlPower::kVregPwmBigParentName),
                    MakeOffer<fuchsia_hardware_vreg::Service>(AmlPower::kVregPwmLittleParentName)});
  }

  void PrimaryClusterPwmExpectSetConfig(fuchsia_hardware_pwm::wire::PwmConfig cfg) {
    driver_test_.RunInEnvironmentTypeContext(
        [cfg](auto& env) { env.primary_cluster_pwm().ExpectSetConfig(cfg); });
  }

  template <typename T>
  T WithBigClusterVregContext(fit::callback<T(fdf_fake::FakeVreg&)> callback) {
    return driver_test_.RunInEnvironmentTypeContext<T>(
        [callback = std::move(callback)](auto& env) mutable {
          return callback(env.big_cluster_vreg());
        });
  }

  template <typename T>
  T WithLittleClusterVregContext(fit::callback<T(fdf_fake::FakeVreg&)> callback) {
    return driver_test_.RunInEnvironmentTypeContext<T>(
        [callback = std::move(callback)](auto& env) mutable {
          return callback(env.little_cluster_vreg());
        });
  }

  AmlPower& driver() { return *driver_test_.driver(); }

 private:
  // Different from `fdf::MakeOffer2()`. This function correctly maps the service's instance name so
  // that the driver's service validator works correctly. In a real environment, this mapping
  // correction is performed by the driver framework.
  template <typename Service>
  static fuchsia_driver_framework::Offer MakeOffer(
      std::string_view instance_name = fdf::kDefaultInstance) {
    static_assert(fidl::IsServiceV<Service>, "Service must be a fidl Service");

    auto mapping = fuchsia_component_decl::NameMapping{{
        .source_name = std::string{instance_name},
        // `fdf::MakeOffer2()` would set `target_name` to `fdf::kDefaultInstance`.
        .target_name = std::string{instance_name},
    }};

    // `fdf::MakeOffer2()` would set this to `fdf::kDefaultInstance`.
    auto includes = std::string{instance_name};

    auto service = fuchsia_component_decl::OfferService{{
        .source_name = std::string(Service::Name),
        .target_name = std::string(Service::Name),
        .source_instance_filter = std::vector{std::string(includes)},
        .renamed_instances = std::vector{std::move(mapping)},
    }};

    return fuchsia_driver_framework::Offer::WithZirconTransport(
        fuchsia_component_decl::Offer::WithService(service));
  }

  // `node_offers` are appended to the node offers provided to `AmlPower::Start()`.
  void StartDriver(std::vector<fuchsia_driver_framework::Offer> node_offers) {
    ASSERT_OK(driver_test_.StartDriverWithCustomStartArgs(
        [additional_node_offers = std::move(node_offers)](fdf::DriverStartArgs& start_args) {
          // Enable service-connect validation.
          if (!start_args.program().has_value()) {
            start_args.program().emplace();
          }
          auto& program = start_args.program().value();
          if (!program.entries().has_value()) {
            program.entries().emplace(std::vector<fuchsia_data::DictionaryEntry>{});
          }
          auto& entries = program.entries().value();
          // TOOD(b/413437623): Possibly replace manual creation of program entry for the
          // service-correct validation once driver test framework provides a way to easily enable
          // service-connect validation.
          entries.emplace_back("service_connect_validation",
                               std::make_unique<fuchsia_data::DictionaryValue>(
                                   fuchsia_data::DictionaryValue::WithStr("true")));

          // Add node offers so that service-connect validation performs correctly.
          // TOOD(b/413437623): Don't manually create node offers once the driver test framework
          // correctly creates node offers from the test environment.
          if (!start_args.node_offers().has_value()) {
            start_args.node_offers().emplace(std::vector<fuchsia_driver_framework::Offer>{});
          }
          auto& node_offers = start_args.node_offers().value();
          node_offers.insert(node_offers.end(), additional_node_offers.begin(),
                             additional_node_offers.end());
          node_offers.emplace_back(MakeOffer<fuchsia_driver_compat::Service>());
          node_offers.emplace_back(MakeOffer<fuchsia_hardware_platform_device::Service>());
        }));
  }

  fdf_testing::ForegroundDriverTest<FixtureConfig> driver_test_;
};

const fuchsia_hardware_amlogic_metadata::PowerMetadata AmlPowerTest::kTestMetadata{
    {.voltage_table =
         std::vector<fuchsia_hardware_amlogic_metadata::VoltageTableEntry>{
             {1'050'000, 0},  {1'040'000, 3}, {1'030'000, 6}, {1'020'000, 8}, {1'010'000, 11},
             {1'000'000, 14}, {990'000, 17},  {980'000, 20},  {970'000, 23},  {960'000, 26},
             {950'000, 29},   {940'000, 31},  {930'000, 34},  {920'000, 37},  {910'000, 40},
             {900'000, 43},   {890'000, 45},  {880'000, 48},  {870'000, 51},  {860'000, 54},
             {850'000, 56},   {840'000, 59},  {830'000, 62},  {820'000, 65},  {810'000, 68},
             {800'000, 70},   {790'000, 73},  {780'000, 76},  {770'000, 79},  {760'000, 81},
             {750'000, 84},   {740'000, 87},  {730'000, 89},  {720'000, 92},  {710'000, 95},
             {700'000, 98},   {690'000, 100},
         },
     .voltage_pwm_period = zx::nsec(1250).get()}};

TEST_F(AmlPowerTest, SetVoltage) {
  StartDriverAstro();
  zx_status_t st;
  constexpr uint32_t kTestVoltageInitial = 690'000;
  constexpr uint32_t kTestVoltageFinal = 1'040'000;

  aml_pwm::mode_config on = {aml_pwm::Mode::kOn, {}};

  // Initialize to 0.69V
  fuchsia_hardware_pwm::wire::PwmConfig cfg{.polarity = false,
                                            .period_ns = 1250,
                                            .duty_cycle = 100,
                                            .mode_config = fidl::VectorView<uint8_t>::FromExternal(
                                                reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);

  // Scale up to 1.05V
  cfg = {.polarity = false,
         .period_ns = 1250,
         .duty_cycle = 92,
         .mode_config =
             fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);
  cfg = {.polarity = false,
         .period_ns = 1250,
         .duty_cycle = 84,
         .mode_config =
             fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);
  cfg = {.polarity = false,
         .period_ns = 1250,
         .duty_cycle = 76,
         .mode_config =
             fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);
  cfg = {.polarity = false,
         .period_ns = 1250,
         .duty_cycle = 68,
         .mode_config =
             fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);
  cfg = {.polarity = false,
         .period_ns = 1250,
         .duty_cycle = 59,
         .mode_config =
             fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);
  cfg = {.polarity = false,
         .period_ns = 1250,
         .duty_cycle = 51,
         .mode_config =
             fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);
  cfg = {.polarity = false,
         .period_ns = 1250,
         .duty_cycle = 43,
         .mode_config =
             fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);
  cfg = {.polarity = false,
         .period_ns = 1250,
         .duty_cycle = 34,
         .mode_config =
             fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);
  cfg = {.polarity = false,
         .period_ns = 1250,
         .duty_cycle = 26,
         .mode_config =
             fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);
  cfg = {.polarity = false,
         .period_ns = 1250,
         .duty_cycle = 17,
         .mode_config =
             fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);
  cfg = {.polarity = false,
         .period_ns = 1250,
         .duty_cycle = 8,
         .mode_config =
             fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);
  cfg = {.polarity = false,
         .period_ns = 1250,
         .duty_cycle = 3,
         .mode_config =
             fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);

  uint32_t actual;
  st = driver().PowerImplRequestVoltage(bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_BIG,
                                        kTestVoltageInitial, &actual);
  EXPECT_EQ(kTestVoltageInitial, actual);
  EXPECT_OK(st);

  st = driver().PowerImplRequestVoltage(bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_BIG,
                                        kTestVoltageFinal, &actual);
  EXPECT_EQ(kTestVoltageFinal, actual);
  EXPECT_OK(st);
}

TEST_F(AmlPowerTest, ClusterIndexOutOfRange) {
  constexpr uint32_t kTestVoltage = 690'000;

  StartDriverAstro();

  uint32_t actual;
  zx_status_t st = driver().PowerImplRequestVoltage(
      bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_LITTLE, kTestVoltage, &actual);
  EXPECT_NE(st, ZX_OK);
}

TEST_F(AmlPowerTest, GetVoltageUnset) {
  // Get the voltage before it's been set. Should return ZX_ERR_BAD_STATE.
  StartDriverAstro();

  uint32_t voltage;
  zx_status_t st = driver().PowerImplGetCurrentVoltage(
      bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_LITTLE, &voltage);
  EXPECT_NE(st, ZX_OK);
}

TEST_F(AmlPowerTest, GetVoltage) {
  // Get the voltage before it's been set. Should return ZX_ERR_BAD_STATE.
  constexpr uint32_t kTestVoltage = 690'000;
  zx_status_t st;

  StartDriverAstro();

  // Initialize to 0.69V
  aml_pwm::mode_config on = {aml_pwm::Mode::kOn, {}};
  fuchsia_hardware_pwm::wire::PwmConfig cfg = {
      false, 1250, 100,
      fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);

  uint32_t requested_voltage, actual_voltage;
  st = driver().PowerImplRequestVoltage(bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_BIG,
                                        kTestVoltage, &requested_voltage);
  EXPECT_OK(st);
  EXPECT_EQ(requested_voltage, kTestVoltage);

  st = driver().PowerImplGetCurrentVoltage(bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_BIG,
                                           &actual_voltage);
  EXPECT_OK(st);
  EXPECT_EQ(requested_voltage, actual_voltage);
}

TEST_F(AmlPowerTest, GetVoltageOutOfRange) {
  StartDriverAstro();

  uint32_t voltage;
  zx_status_t st = driver().PowerImplGetCurrentVoltage(
      bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_BIG, &voltage);
  EXPECT_NE(st, ZX_OK);
}

TEST_F(AmlPowerTest, SetVoltageRoundDown) {
  // Set a voltage that's not exactly supported and let the driver round down to the nearest
  // voltage.
  StartDriverAstro();
  constexpr uint32_t kTestVoltageInitial = 830'000;

  // We expect the driver to give us the highest voltage that does not exceed the requested voltage.
  constexpr uint32_t kTestVoltageFinalRequest = 935'000;
  constexpr uint32_t kTestVoltageFinalActual = 930'000;

  aml_pwm::mode_config on = {aml_pwm::Mode::kOn, {}};
  fuchsia_hardware_pwm::wire::PwmConfig cfg{.polarity = false,
                                            .period_ns = 1250,
                                            .duty_cycle = 62,
                                            .mode_config = fidl::VectorView<uint8_t>::FromExternal(
                                                reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);
  cfg = {.polarity = false,
         .period_ns = 1250,
         .duty_cycle = 54,
         .mode_config =
             fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);
  cfg = {.polarity = false,
         .period_ns = 1250,
         .duty_cycle = 45,
         .mode_config =
             fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);
  cfg = {.polarity = false,
         .period_ns = 1250,
         .duty_cycle = 37,
         .mode_config =
             fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);
  cfg = {.polarity = false,
         .period_ns = 1250,
         .duty_cycle = 34,
         .mode_config =
             fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);

  uint32_t actual;
  zx_status_t st;

  st = driver().PowerImplRequestVoltage(bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_BIG,
                                        kTestVoltageInitial, &actual);
  EXPECT_OK(st);
  EXPECT_EQ(actual, kTestVoltageInitial);

  st = driver().PowerImplRequestVoltage(bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_BIG,
                                        kTestVoltageFinalRequest, &actual);
  EXPECT_OK(st);
  EXPECT_EQ(actual, kTestVoltageFinalActual);
}

TEST_F(AmlPowerTest, SetVoltageLittleCluster) {
  // Set a voltage that's not exactly supported and let the driver round down to the nearest
  // voltage.
  StartDriverAstro();
  constexpr uint32_t kTestVoltageInitial = 730'000;
  constexpr uint32_t kTestVoltageFinal = 930'000;

  aml_pwm::mode_config on = {aml_pwm::Mode::kOn, {}};
  fuchsia_hardware_pwm::wire::PwmConfig cfg{.polarity = false,
                                            .period_ns = 1250,
                                            .duty_cycle = 89,
                                            .mode_config = fidl::VectorView<uint8_t>::FromExternal(
                                                reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);
  cfg = {.polarity = false,
         .period_ns = 1250,
         .duty_cycle = 81,
         .mode_config =
             fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);
  cfg = {.polarity = false,
         .period_ns = 1250,
         .duty_cycle = 73,
         .mode_config =
             fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);
  cfg = {.polarity = false,
         .period_ns = 1250,
         .duty_cycle = 65,
         .mode_config =
             fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);
  cfg = {.polarity = false,
         .period_ns = 1250,
         .duty_cycle = 56,
         .mode_config =
             fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);
  cfg = {.polarity = false,
         .period_ns = 1250,
         .duty_cycle = 48,
         .mode_config =
             fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);
  cfg = {.polarity = false,
         .period_ns = 1250,
         .duty_cycle = 40,
         .mode_config =
             fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);
  cfg = {.polarity = false,
         .period_ns = 1250,
         .duty_cycle = 34,
         .mode_config =
             fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  PrimaryClusterPwmExpectSetConfig(cfg);

  uint32_t actual;
  zx_status_t st;

  st = driver().PowerImplRequestVoltage(bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_BIG,
                                        kTestVoltageInitial, &actual);
  EXPECT_OK(st);
  EXPECT_EQ(actual, kTestVoltageInitial);

  st = driver().PowerImplRequestVoltage(bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_BIG,
                                        kTestVoltageFinal, &actual);
  EXPECT_OK(st);
  EXPECT_EQ(actual, kTestVoltageFinal);
}

TEST_F(AmlPowerTest, DomainEnableDisable) {
  StartDriverVim3();

  // Enable.
  EXPECT_OK(driver().PowerImplEnablePowerDomain(
      bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_LITTLE));
  EXPECT_OK(driver().PowerImplEnablePowerDomain(
      bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_BIG));

  // Disable.
  EXPECT_OK(driver().PowerImplDisablePowerDomain(
      bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_LITTLE));
  EXPECT_OK(driver().PowerImplDisablePowerDomain(
      bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_BIG));

  // Out of bounds.
  EXPECT_NE(driver().PowerImplDisablePowerDomain(
                bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_LITTLE + 1),
            ZX_OK);
  EXPECT_NE(driver().PowerImplEnablePowerDomain(
                bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_LITTLE + 1),
            ZX_OK);
}

TEST_F(AmlPowerTest, GetDomainStatus) {
  StartDriverAstro();

  // Happy case.
  power_domain_status_t result;
  EXPECT_OK(driver().PowerImplGetPowerDomainStatus(
      bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_BIG, &result));
  EXPECT_EQ(result, POWER_DOMAIN_STATUS_ENABLED);

  // Out of bounds.
  EXPECT_NE(driver().PowerImplGetPowerDomainStatus(
                bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_LITTLE, &result),
            ZX_OK);
}

TEST_F(AmlPowerTest, Vim3SetBigCluster) {
  StartDriverVim3();

  WithBigClusterVregContext<void>(
      [](fdf_fake::FakeVreg& cluster_vreg) { cluster_vreg.SetRegulatorParams(100, 10, 10); });
  const uint32_t kTestVoltage = 155;
  uint32_t actual;
  EXPECT_OK(driver().PowerImplRequestVoltage(
      bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_BIG, kTestVoltage, &actual));
  EXPECT_EQ(actual, 150u);
  uint32_t voltage_step = WithBigClusterVregContext<uint32_t>(
      [](fdf_fake::FakeVreg& cluster_vreg) { return cluster_vreg.voltage_step(); });
  EXPECT_EQ(voltage_step, 5u);

  // Voltage is too low.
  EXPECT_NE(driver().PowerImplRequestVoltage(
                bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_BIG, 99, &actual),
            ZX_OK);

  // Set voltage to the threshold.
  EXPECT_OK(driver().PowerImplRequestVoltage(
      bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_BIG, 200, &actual));
  EXPECT_EQ(actual, 200u);
  voltage_step = WithBigClusterVregContext<uint32_t>(
      [](fdf_fake::FakeVreg& cluster_vreg) { return cluster_vreg.voltage_step(); });
  EXPECT_EQ(voltage_step, 10u);

  // Set voltage beyond the threshold.
  EXPECT_NE(driver().PowerImplRequestVoltage(
                bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_BIG, 300, &actual),
            ZX_OK);
}

TEST_F(AmlPowerTest, Vim3SetLittleCluster) {
  StartDriverVim3();

  WithLittleClusterVregContext<void>(
      [](fdf_fake::FakeVreg& cluster_vreg) { cluster_vreg.SetRegulatorParams(100, 10, 10); });
  const uint32_t kTestVoltage = 155;
  uint32_t actual;
  EXPECT_OK(driver().PowerImplRequestVoltage(
      bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_LITTLE, kTestVoltage, &actual));
  EXPECT_EQ(actual, 150u);
  uint32_t voltage_step = WithLittleClusterVregContext<uint32_t>(
      [](fdf_fake::FakeVreg& cluster_vreg) { return cluster_vreg.voltage_step(); });
  EXPECT_EQ(voltage_step, 5u);

  // Voltage is too low.
  EXPECT_NE(driver().PowerImplRequestVoltage(
                bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_LITTLE, 99, &actual),
            ZX_OK);

  // Set voltage to the threshold.
  EXPECT_OK(driver().PowerImplRequestVoltage(
      bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_LITTLE, 200, &actual));
  EXPECT_EQ(actual, 200u);
  voltage_step = WithLittleClusterVregContext<uint32_t>(
      [](fdf_fake::FakeVreg& cluster_vreg) { return cluster_vreg.voltage_step(); });
  EXPECT_EQ(voltage_step, 10u);

  // Set voltage beyond the threshold.
  EXPECT_NE(driver().PowerImplRequestVoltage(
                bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_LITTLE, 300, &actual),
            ZX_OK);
}

TEST_F(AmlPowerTest, Vim3GetSupportedVoltageRange) {
  StartDriverVim3();

  WithBigClusterVregContext<void>(
      [](fdf_fake::FakeVreg& cluster_vreg) { cluster_vreg.SetRegulatorParams(100, 10, 10); });

  uint32_t max, min;
  EXPECT_OK(driver().PowerImplGetSupportedVoltageRange(
      bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_BIG, &min, &max));
  EXPECT_EQ(max, 200u);
  EXPECT_EQ(min, 100u);

  WithLittleClusterVregContext<void>(
      [](fdf_fake::FakeVreg& cluster_vreg) { cluster_vreg.SetRegulatorParams(100, 20, 5); });

  EXPECT_OK(driver().PowerImplGetSupportedVoltageRange(
      bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_LITTLE, &min, &max));
  EXPECT_EQ(max, 200u);
  EXPECT_EQ(min, 100u);
}

}  // namespace power::test
