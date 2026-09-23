// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/annotations/battery_info_provider.h"

#include <lib/vfs/cpp/service.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/feedback/annotations/constants.h"
#include "src/developer/forensics/feedback/annotations/types.h"
#include "src/developer/forensics/testing/backoff.h"
#include "src/developer/forensics/testing/gpretty_printers.h"  // IWYU pragma: keep
#include "src/developer/forensics/testing/stubs/battery_info_provider.h"
#include "src/developer/forensics/testing/unit_test_fixture.h"

namespace forensics::feedback {
namespace {

using ::forensics::stubs::StubBatteryInfoProvider;
using ::fuchsia_power_battery::BatteryManager;
using ::fuchsia_power_battery::BatteryStatus;
using ::fuchsia_power_battery::ChargeSource;
using ::fuchsia_power_battery::ChargeStatus;
using ::testing::Contains;
using ::testing::Pair;
using ::testing::UnorderedElementsAreArray;

class BatteryInfoProviderTest : public UnitTestFixture {
 protected:
  void SetUpProvider(bool inject_stub = true) {
    if (inject_stub) {
      auto service = std::make_unique<vfs::Service>(
          [this](zx::channel channel, async_dispatcher_t* /*unused*/) {
            binding_ = fidl::BindServer(
                dispatcher(), fidl::ServerEnd<BatteryManager>(std::move(channel)), &stub_);
          });
      InjectServiceProvider(std::move(service),
                            ::fuchsia_power_battery::BatteryManager::kDiscoverableName);
    }

    provider_ = std::make_unique<BatteryInfoProvider>(dispatcher(), services(),
                                                      std::make_unique<MonotonicBackoff>());
  }

  StubBatteryInfoProvider stub_;
  std::optional<fidl::ServerBindingRef<BatteryManager>> binding_;
  std::unique_ptr<BatteryInfoProvider> provider_;
};

TEST_F(BatteryInfoProviderTest, AnnotationsMissingValueOnMissingStatus) {
  fuchsia_power_battery::BatteryInfo info;
  info.level_percent(50.0f);
  info.charge_source(ChargeSource::kAcAdapter);
  info.charge_status(ChargeStatus::kCharging);
  stub_.set_battery_info(std::move(info));

  SetUpProvider();

  std::optional<Annotations> annotations;
  provider_->Get([&annotations](Annotations a) { annotations = std::move(a); });

  RunLoopUntilIdle();
  ASSERT_TRUE(annotations.has_value());
  EXPECT_THAT(*annotations,
              UnorderedElementsAreArray({
                  Pair(kDeviceBatteryLevelKey, ErrorOrString(Error::kMissingValue)),
                  Pair(kDeviceBatteryStateKey, ErrorOrString(Error::kMissingValue)),
                  Pair(kDeviceBatteryOnChargerKey, ErrorOrString(Error::kMissingValue)),
              }));
}

TEST_F(BatteryInfoProviderTest, AnnotationsLogicErrorOnNotPresentStatus) {
  fuchsia_power_battery::BatteryInfo info;
  info.status(BatteryStatus::kNotPresent);
  info.level_percent(50.0f);
  info.charge_source(ChargeSource::kAcAdapter);
  info.charge_status(ChargeStatus::kCharging);
  stub_.set_battery_info(std::move(info));

  SetUpProvider();

  std::optional<Annotations> annotations;
  provider_->Get([&annotations](Annotations a) { annotations = std::move(a); });

  RunLoopUntilIdle();
  ASSERT_TRUE(annotations.has_value());
  EXPECT_THAT(*annotations, UnorderedElementsAreArray({
                                Pair(kDeviceBatteryLevelKey, ErrorOrString(Error::kLogicError)),
                                Pair(kDeviceBatteryStateKey, ErrorOrString(Error::kLogicError)),
                                Pair(kDeviceBatteryOnChargerKey, ErrorOrString(Error::kLogicError)),
                            }));
}

TEST_F(BatteryInfoProviderTest, ErrorMissingLevel) {
  fuchsia_power_battery::BatteryInfo info;
  info.status(BatteryStatus::kOk);
  info.charge_source(ChargeSource::kAcAdapter);
  info.charge_status(ChargeStatus::kCharging);
  stub_.set_battery_info(std::move(info));

  SetUpProvider();

  std::optional<Annotations> annotations;
  provider_->Get([&annotations](Annotations a) { annotations = std::move(a); });

  RunLoopUntilIdle();
  ASSERT_TRUE(annotations.has_value());
  EXPECT_THAT(*annotations,
              Contains(Pair(kDeviceBatteryLevelKey, ErrorOrString(Error::kMissingValue))));
}

struct ChargeSourceParam {
  std::string test_name;
  std::optional<ChargeSource> source;
  ErrorOrString expected_value;
};

class BatteryInfoProviderChargeSourceTest
    : public BatteryInfoProviderTest,
      public ::testing::WithParamInterface<ChargeSourceParam> {};

TEST_P(BatteryInfoProviderChargeSourceTest, Success) {
  const ChargeSourceParam& param = GetParam();

  fuchsia_power_battery::BatteryInfo info;
  info.status(BatteryStatus::kOk);
  if (param.source.has_value()) {
    info.charge_source(*param.source);
  }

  stub_.set_battery_info(std::move(info));
  SetUpProvider();

  std::optional<Annotations> annotations;
  provider_->Get([&annotations](Annotations a) { annotations = std::move(a); });

  RunLoopUntilIdle();
  ASSERT_TRUE(annotations.has_value());
  EXPECT_THAT(*annotations, Contains(Pair(kDeviceBatteryOnChargerKey, param.expected_value)));
}

INSTANTIATE_TEST_SUITE_P(BatteryInfoProviderTests, BatteryInfoProviderChargeSourceTest,
                         ::testing::ValuesIn(std::vector<ChargeSourceParam>({
                             {
                                 "AcAdapter",
                                 ChargeSource::kAcAdapter,
                                 ErrorOrString("true"),
                             },
                             {
                                 "Usb",
                                 ChargeSource::kUsb,
                                 ErrorOrString("true"),
                             },
                             {
                                 "Wireless",
                                 ChargeSource::kWireless,
                                 ErrorOrString("true"),
                             },
                             {
                                 "None",
                                 ChargeSource::kNone,
                                 ErrorOrString("false"),
                             },
                             {
                                 "Unknown",
                                 ChargeSource::kUnknown,
                                 ErrorOrString("false"),
                             },
                             {
                                 "NotSet",
                                 std::nullopt,
                                 ErrorOrString(Error::kMissingValue),
                             },
                         })),
                         [](const testing::TestParamInfo<ChargeSourceParam>& info) {
                           return info.param.test_name;
                         });

struct ChargeStatusParam {
  std::string test_name;
  std::optional<ChargeStatus> status;
  ErrorOrString expected_state;
};

class BatteryInfoProviderChargeStatusTest
    : public BatteryInfoProviderTest,
      public ::testing::WithParamInterface<ChargeStatusParam> {};

TEST_P(BatteryInfoProviderChargeStatusTest, Success) {
  const ChargeStatusParam& param = GetParam();
  fuchsia_power_battery::BatteryInfo info;
  info.status(BatteryStatus::kOk);
  if (param.status.has_value()) {
    info.charge_status(*param.status);
  }

  stub_.set_battery_info(std::move(info));
  SetUpProvider();

  std::optional<Annotations> annotations;
  provider_->Get([&annotations](Annotations a) { annotations = std::move(a); });

  RunLoopUntilIdle();
  ASSERT_TRUE(annotations.has_value());
  EXPECT_THAT(*annotations, Contains(Pair(kDeviceBatteryStateKey, param.expected_state)));
}

INSTANTIATE_TEST_SUITE_P(BatteryInfoProviderTests, BatteryInfoProviderChargeStatusTest,
                         ::testing::ValuesIn(std::vector<ChargeStatusParam>({
                             {
                                 "Charging",
                                 ChargeStatus::kCharging,
                                 ErrorOrString("charging"),
                             },
                             {
                                 "NotCharging",
                                 ChargeStatus::kNotCharging,
                                 ErrorOrString("not charging"),
                             },
                             {
                                 "Discharging",
                                 ChargeStatus::kDischarging,
                                 ErrorOrString("discharging"),
                             },
                             {
                                 "Full",
                                 ChargeStatus::kFull,
                                 ErrorOrString("full"),
                             },
                             {
                                 "Unknown",
                                 ChargeStatus::kUnknown,
                                 ErrorOrString("unknown"),
                             },
                             {
                                 "NotSet",
                                 std::nullopt,
                                 ErrorOrString(Error::kMissingValue),
                             },
                         })),
                         [](const testing::TestParamInfo<ChargeStatusParam>& info) {
                           return info.param.test_name;
                         });

TEST_F(BatteryInfoProviderTest, Keys) {
  SetUpProvider();

  EXPECT_THAT(provider_->GetKeys(), UnorderedElementsAreArray({
                                        kDeviceBatteryLevelKey,
                                        kDeviceBatteryStateKey,
                                        kDeviceBatteryOnChargerKey,
                                    }));

  EXPECT_THAT(provider_->GetKeys(), BatteryInfoProvider::GetAnnotationKeys());
}

TEST_F(BatteryInfoProviderTest, InitialConnectionFailure) {
  SetUpProvider(/*inject_stub=*/false);

  std::optional<Annotations> annotations;
  provider_->Get([&annotations](Annotations a) { annotations = std::move(a); });

  RunLoopUntilIdle();
  ASSERT_TRUE(annotations.has_value());
  EXPECT_THAT(*annotations,
              UnorderedElementsAreArray({
                  Pair(kDeviceBatteryLevelKey, ErrorOrString(Error::kNotAvailableInProduct)),
                  Pair(kDeviceBatteryStateKey, ErrorOrString(Error::kNotAvailableInProduct)),
                  Pair(kDeviceBatteryOnChargerKey, ErrorOrString(Error::kNotAvailableInProduct)),
              }));
}

TEST_F(BatteryInfoProviderTest, Reconnects) {
  fuchsia_power_battery::BatteryInfo info;
  info.status(BatteryStatus::kOk);
  info.level_percent(42.0f);
  info.charge_source(ChargeSource::kNone);
  info.charge_status(ChargeStatus::kDischarging);
  stub_.set_battery_info(std::move(info));

  SetUpProvider();
  RunLoopUntilIdle();
  ASSERT_TRUE(binding_.has_value());

  binding_->Unbind();
  RunLoopUntilIdle();

  std::optional<Annotations> annotations;
  provider_->Get([&annotations](Annotations a) { annotations = std::move(a); });

  RunLoopUntilIdle();
  ASSERT_TRUE(annotations.has_value());
  EXPECT_THAT(*annotations,
              UnorderedElementsAreArray({
                  Pair(kDeviceBatteryLevelKey, ErrorOrString(Error::kConnectionError)),
                  Pair(kDeviceBatteryStateKey, ErrorOrString(Error::kConnectionError)),
                  Pair(kDeviceBatteryOnChargerKey, ErrorOrString(Error::kConnectionError)),
              }));

  // Wait for backoff.
  RunLoopFor(zx::sec(1));

  annotations = std::nullopt;
  provider_->Get([&annotations](Annotations a) { annotations = std::move(a); });

  RunLoopUntilIdle();
  ASSERT_TRUE(annotations.has_value());
  EXPECT_THAT(*annotations, UnorderedElementsAreArray({
                                Pair(kDeviceBatteryLevelKey, ErrorOrString("42")),
                                Pair(kDeviceBatteryStateKey, ErrorOrString("discharging")),
                                Pair(kDeviceBatteryOnChargerKey, ErrorOrString("false")),
                            }));
}

TEST_F(BatteryInfoProviderTest, ResetsBackoffOnSuccess) {
  fuchsia_power_battery::BatteryInfo info;
  info.status(BatteryStatus::kOk);
  info.level_percent(42.0f);
  info.charge_source(ChargeSource::kNone);
  info.charge_status(ChargeStatus::kDischarging);
  stub_.set_battery_info(std::move(info));

  SetUpProvider();
  RunLoopUntilIdle();
  ASSERT_TRUE(binding_.has_value());

  binding_->Unbind();
  RunLoopUntilIdle();

  std::optional<Annotations> annotations;
  provider_->Get([&annotations](Annotations a) { annotations = std::move(a); });

  RunLoopUntilIdle();
  ASSERT_TRUE(annotations.has_value());
  EXPECT_THAT(*annotations,
              UnorderedElementsAreArray({
                  Pair(kDeviceBatteryLevelKey, ErrorOrString(Error::kConnectionError)),
                  Pair(kDeviceBatteryStateKey, ErrorOrString(Error::kConnectionError)),
                  Pair(kDeviceBatteryOnChargerKey, ErrorOrString(Error::kConnectionError)),
              }));

  // Wait for backoff.
  RunLoopFor(zx::sec(1));

  annotations = std::nullopt;
  provider_->Get([&annotations](Annotations a) { annotations = std::move(a); });

  RunLoopUntilIdle();
  ASSERT_TRUE(annotations.has_value());
  EXPECT_THAT(*annotations, UnorderedElementsAreArray({
                                Pair(kDeviceBatteryLevelKey, ErrorOrString("42")),
                                Pair(kDeviceBatteryStateKey, ErrorOrString("discharging")),
                                Pair(kDeviceBatteryOnChargerKey, ErrorOrString("false")),
                            }));

  binding_->Unbind();
  RunLoopUntilIdle();

  annotations = std::nullopt;
  provider_->Get([&annotations](Annotations a) { annotations = std::move(a); });

  RunLoopUntilIdle();
  ASSERT_TRUE(annotations.has_value());
  EXPECT_THAT(*annotations,
              UnorderedElementsAreArray({
                  Pair(kDeviceBatteryLevelKey, ErrorOrString(Error::kConnectionError)),
                  Pair(kDeviceBatteryStateKey, ErrorOrString(Error::kConnectionError)),
                  Pair(kDeviceBatteryOnChargerKey, ErrorOrString(Error::kConnectionError)),
              }));

  // Wait for backoff again. It should be zx::sec(1), not zx::sec(2) because of the intermediate
  // success.
  RunLoopFor(zx::sec(1));

  annotations = std::nullopt;
  provider_->Get([&annotations](Annotations a) { annotations = std::move(a); });

  RunLoopUntilIdle();
  ASSERT_TRUE(annotations.has_value());
  EXPECT_THAT(*annotations, UnorderedElementsAreArray({
                                Pair(kDeviceBatteryLevelKey, ErrorOrString("42")),
                                Pair(kDeviceBatteryStateKey, ErrorOrString("discharging")),
                                Pair(kDeviceBatteryOnChargerKey, ErrorOrString("false")),
                            }));
}

TEST_F(BatteryInfoProviderTest, DoesNotReconnectOnNotFound) {
  fuchsia_power_battery::BatteryInfo info;
  info.status(BatteryStatus::kOk);
  info.level_percent(42.0f);
  info.charge_source(ChargeSource::kNone);
  info.charge_status(ChargeStatus::kDischarging);
  stub_.set_battery_info(std::move(info));

  SetUpProvider();
  RunLoopUntilIdle();
  ASSERT_TRUE(binding_.has_value());

  binding_->Close(ZX_ERR_NOT_FOUND);
  RunLoopUntilIdle();

  std::optional<Annotations> annotations;
  provider_->Get([&annotations](Annotations a) { annotations = std::move(a); });

  RunLoopUntilIdle();
  ASSERT_TRUE(annotations.has_value());
  EXPECT_THAT(*annotations,
              UnorderedElementsAreArray({
                  Pair(kDeviceBatteryLevelKey, ErrorOrString(Error::kNotAvailableInProduct)),
                  Pair(kDeviceBatteryStateKey, ErrorOrString(Error::kNotAvailableInProduct)),
                  Pair(kDeviceBatteryOnChargerKey, ErrorOrString(Error::kNotAvailableInProduct)),
              }));

  // Wait for backoff.
  RunLoopFor(zx::sec(1));

  annotations = std::nullopt;
  provider_->Get([&annotations](Annotations a) { annotations = std::move(a); });

  RunLoopUntilIdle();
  ASSERT_TRUE(annotations.has_value());
  EXPECT_THAT(*annotations,
              UnorderedElementsAreArray({
                  Pair(kDeviceBatteryLevelKey, ErrorOrString(Error::kNotAvailableInProduct)),
                  Pair(kDeviceBatteryStateKey, ErrorOrString(Error::kNotAvailableInProduct)),
                  Pair(kDeviceBatteryOnChargerKey, ErrorOrString(Error::kNotAvailableInProduct)),
              }));
}

TEST_F(BatteryInfoProviderTest, GetTimedOut) {
  SetUpProvider();
  RunLoopUntilIdle();
  ASSERT_TRUE(binding_.has_value());

  std::optional<Annotations> annotations;
  provider_->Get([&annotations](Annotations a) { annotations = std::move(a); });

  binding_->Close(ZX_ERR_TIMED_OUT);
  RunLoopUntilIdle();

  ASSERT_TRUE(annotations.has_value());
  EXPECT_THAT(*annotations, UnorderedElementsAreArray({
                                Pair(kDeviceBatteryLevelKey, ErrorOrString(Error::kTimeout)),
                                Pair(kDeviceBatteryStateKey, ErrorOrString(Error::kTimeout)),
                                Pair(kDeviceBatteryOnChargerKey, ErrorOrString(Error::kTimeout)),
                            }));
}

TEST_F(BatteryInfoProviderTest, GetSuccess) {
  fuchsia_power_battery::BatteryInfo info;
  info.status(BatteryStatus::kOk);
  info.level_percent(42.5f);
  info.charge_source(ChargeSource::kAcAdapter);
  info.charge_status(ChargeStatus::kCharging);
  stub_.set_battery_info(std::move(info));

  SetUpProvider();

  std::optional<Annotations> annotations;
  provider_->Get([&annotations](Annotations a) { annotations = std::move(a); });

  RunLoopUntilIdle();
  ASSERT_TRUE(annotations.has_value());
  EXPECT_THAT(*annotations, UnorderedElementsAreArray({
                                Pair(kDeviceBatteryLevelKey, ErrorOrString("42")),
                                Pair(kDeviceBatteryStateKey, ErrorOrString("charging")),
                                Pair(kDeviceBatteryOnChargerKey, ErrorOrString("true")),
                            }));
}

}  // namespace
}  // namespace forensics::feedback
