// Copyright 2020 The Fuchsia Authors. All rights reserved.  Use of
// this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.adc/cpp/test_base.h>
#include <fidl/fuchsia.hardware.temperature/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>

#include <zxtest/zxtest.h>

#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/devices/thermal/drivers/aml-thermistor/thermistor-channel.h"

namespace {

bool FloatNear(float a, float b) { return std::abs(a - b) < 0.1f; }

}  // namespace

namespace thermal {

NtcInfo ntc_info[] = {
    {.part = "ncpXXwf104",
     .profile =
         {
             {.temperature_c = -40, .resistance_ohm = 4397119},  // 0
             {.temperature_c = -35, .resistance_ohm = 3088599},
             {.temperature_c = -30, .resistance_ohm = 2197225},
             {.temperature_c = -25, .resistance_ohm = 1581881},
             {.temperature_c = -20, .resistance_ohm = 1151037},
             {.temperature_c = -15, .resistance_ohm = 846579},
             {.temperature_c = -10, .resistance_ohm = 628988},
             {.temperature_c = -5, .resistance_ohm = 471632},
             {.temperature_c = 0, .resistance_ohm = 357012},
             {.temperature_c = 5, .resistance_ohm = 272500},
             {.temperature_c = 10, .resistance_ohm = 209710},  // 10
             {.temperature_c = 15, .resistance_ohm = 162651},
             {.temperature_c = 20, .resistance_ohm = 127080},
             {.temperature_c = 25, .resistance_ohm = 100000},
             {.temperature_c = 30, .resistance_ohm = 79222},
             {.temperature_c = 35, .resistance_ohm = 63167},
             {.temperature_c = 40, .resistance_ohm = 50677},
             {.temperature_c = 45, .resistance_ohm = 40904},
             {.temperature_c = 50, .resistance_ohm = 33195},
             {.temperature_c = 55, .resistance_ohm = 27091},
             {.temperature_c = 60, .resistance_ohm = 22224},  // 20
             {.temperature_c = 65, .resistance_ohm = 18323},
             {.temperature_c = 70, .resistance_ohm = 15184},
             {.temperature_c = 75, .resistance_ohm = 12635},
             {.temperature_c = 80, .resistance_ohm = 10566},
             {.temperature_c = 85, .resistance_ohm = 8873},
             {.temperature_c = 90, .resistance_ohm = 7481},
             {.temperature_c = 95, .resistance_ohm = 6337},
             {.temperature_c = 100, .resistance_ohm = 5384},
             {.temperature_c = 105, .resistance_ohm = 4594},
             {.temperature_c = 110, .resistance_ohm = 3934},  // 30
             {.temperature_c = 115, .resistance_ohm = 3380},
             {.temperature_c = 120, .resistance_ohm = 2916},
             {.temperature_c = 125, .resistance_ohm = 2522},  // 33
         }},
};

class TestSarAdc : public fidl::testing::TestBase<fuchsia_hardware_adc::Device> {
 public:
  TestSarAdc(async_dispatcher_t* dispatcher,
             fidl::ServerEnd<fuchsia_hardware_adc::Device> server_end)
      : binding_(dispatcher, std::move(server_end), this, [](fidl::UnbindInfo) {}) {}

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void GetNormalizedSample(GetNormalizedSampleCompleter::Sync& completer) override {
    completer.Reply(fit::ok(normalized_sample_));
  }
  void set_normalized_sample(float normalized_sample) { normalized_sample_ = normalized_sample; }

 private:
  fidl::ServerBinding<fuchsia_hardware_adc::Device> binding_;

  float normalized_sample_ = 0;
};

class ThermistorDeviceTest : public zxtest::Test {
 public:
  float CalcSampleValue(NtcInfo info, uint32_t idx, uint32_t pullup) {
    uint32_t ntc_resistance = info.profile[idx].resistance_ohm;
    return static_cast<float>(ntc_resistance) / static_cast<float>(ntc_resistance + pullup);
  }

  void SetUp() override {
    {
      EXPECT_OK(incoming_loop_.StartThread("aml-thermistor-test-adc-thread"));
      // Create devices.
      auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_adc::Device>();
      adc_.SyncCall([&](std::unique_ptr<TestSarAdc>* adc) {
        *adc =
            std::make_unique<TestSarAdc>(incoming_loop_.dispatcher(), std::move(endpoints->server));
      });
      thermistor_ = std::make_unique<ThermistorChannel>(root_.get(), std::move(endpoints->client),
                                                        ntc_info[0], kPullupValue, "channel");
    }

    loop_.StartThread();
    auto endpoints = fidl::Endpoints<fuchsia_hardware_temperature::Device>::Create();
    fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), thermistor_.get());
    client_.Bind(std::move(endpoints.client));
  }

 protected:
  static constexpr uint32_t kPullupValue = 47000;

  std::unique_ptr<ThermistorChannel> thermistor_;
  async::Loop incoming_loop_{&kAsyncLoopConfigNeverAttachToThread};
  async_patterns::TestDispatcherBound<std::unique_ptr<TestSarAdc>> adc_{incoming_loop_.dispatcher(),
                                                                        std::in_place};
  std::shared_ptr<MockDevice> root_ = MockDevice::FakeRootParent();
  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
  fidl::SyncClient<fuchsia_hardware_temperature::Device> client_;
};

TEST_F(ThermistorDeviceTest, GetTemperatureCelsius) {
  {
    uint32_t ntc_idx = 10;
    adc_.SyncCall([&](std::unique_ptr<TestSarAdc>* adc) {
      (*adc)->set_normalized_sample(CalcSampleValue(ntc_info[0], ntc_idx, kPullupValue));
    });
    auto result = client_->GetTemperatureCelsius();
    EXPECT_TRUE(result.is_ok());
    EXPECT_OK(result->status());
    EXPECT_TRUE(FloatNear(result->temp(), ntc_info[0].profile[ntc_idx].temperature_c));
  }

  {  // set read value to 0, which should be out of range of the ntc table. Cap at last index.
    adc_.SyncCall([&](std::unique_ptr<TestSarAdc>* adc) { (*adc)->set_normalized_sample(0); });
    auto result = client_->GetTemperatureCelsius();
    EXPECT_TRUE(result.is_ok());
    EXPECT_OK(result->status());
    EXPECT_TRUE(FloatNear(result->temp(), ntc_info[0].profile[33].temperature_c));
  }

  {  // set read value to 0.99999, which should be out of range of the ntc table. Cap at first
     // index.
    adc_.SyncCall(
        [&](std::unique_ptr<TestSarAdc>* adc) { (*adc)->set_normalized_sample(0.99999f); });
    auto result = client_->GetTemperatureCelsius();
    EXPECT_TRUE(result.is_ok());
    EXPECT_OK(result->status());
    EXPECT_TRUE(FloatNear(result->temp(), ntc_info[0].profile[0].temperature_c));
  }

  {  // set read value to max, which should be out of range of ntc table
    adc_.SyncCall([&](std::unique_ptr<TestSarAdc>* adc) { (*adc)->set_normalized_sample(1); });
    auto result = client_->GetTemperatureCelsius();
    EXPECT_TRUE(result.is_ok());
    EXPECT_NOT_OK(result->status());
  }
}

TEST_F(ThermistorDeviceTest, GetSensorName) {
  constexpr char kExpectedSensorName[] = "channel";

  auto result = client_->GetSensorName();
  ASSERT_TRUE(result.is_ok());
  EXPECT_STREQ(result->name(), kExpectedSensorName);
}

}  //  namespace thermal
