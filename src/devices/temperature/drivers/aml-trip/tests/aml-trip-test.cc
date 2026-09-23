// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../aml-trip.h"

#include <fidl/fuchsia.hardware.clock/cpp/wire_messaging.h>
#include <fidl/fuchsia.hardware.clock/cpp/wire_test_base.h>
#include <fidl/fuchsia.hardware.trippoint/cpp/common_types.h>
#include <fidl/fuchsia.hardware.trippoint/cpp/wire_types.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/fake-mmio-reg/cpp/fake-mmio-reg.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/mmio/testing/cpp/test-helper.h>
#include <lib/driver/testing/cpp/driver_test.h>

#include <cstdint>
#include <limits>

#include <gtest/gtest.h>
#include <src/devices/temperature/drivers/aml-trip/aml-trip-device.h>

#include "../aml-trip-device.h"
#include "src/lib/testing/predicates/status.h"

namespace temperature::test {

class FakeSensorMmio {
 public:
  FakeSensorMmio() : mmio_(sizeof(uint32_t), kRegCount) {
    mmio_[kTstat0Offset].SetReadCallback([]() {
      // Magic temp code that evaluates to roughly 41.9C with a sensor trim = 0.
      return 0x205c;
    });
    mmio_[kTstat1Offset].SetReadCallback([this]() { return tstat1_; });
  }

  fdf::MmioBuffer mmio() { return mmio_.GetMmioBuffer(); }

  void SetIrqStat(uint32_t index, bool stat) {
    if (stat) {
      tstat1_ |= (1u << index);
    } else {
      tstat1_ &= ~(1u << index);
    }
  }

 private:
  static constexpr size_t kTstat0Offset = 0x40;
  static constexpr size_t kTstat1Offset = 0x44;
  static constexpr size_t kRegCount = 0x80 / sizeof(uint32_t);

  uint8_t tstat1_ = 0x0;

  fake_mmio::FakeMmioRegRegion mmio_;
};

class AmlTripTestEnvironment : public fdf_testing::Environment {
 public:
  void Init(std::string platform_device_name) {
    static constexpr size_t kTrimRegSize = 0x04;

    compat_server_.Initialize("default");

    ASSERT_OK(zx::interrupt::create(zx::resource(), 0, ZX_INTERRUPT_VIRTUAL, &irq_));
    zx::interrupt irq;
    irq_.duplicate(ZX_RIGHT_SAME_RIGHTS, &irq);
    std::map<uint32_t, zx::interrupt> irqs;
    irqs.insert({0, std::move(irq)});

    std::map<uint32_t, fdf_fake::Mmio> mmios;
    mmios.insert({AmlTrip::kSensorMmioIndex, sensor_mmio_.mmio()});
    mmios.insert({AmlTrip::kTrimMmioIndex,
                  fdf_testing::CreateMmioBuffer(kTrimRegSize, ZX_CACHE_POLICY_UNCACHED_DEVICE)});

    pdev_.SetConfig({.mmios = std::move(mmios),
                     .irqs = std::move(irqs),
                     .device_info{{.name{std::move(platform_device_name)}}}});
  }

  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();

    zx_status_t status = compat_server_.Serve(dispatcher, &to_driver_vfs);
    if (status != ZX_OK) {
      return zx::error(status);
    }

    zx::result result = to_driver_vfs.AddService<fuchsia_hardware_platform_device::Service>(
        pdev_.GetInstanceHandler(dispatcher));
    if (result.is_error()) {
      return result.take_error();
    }

    return zx::ok();
  }

  zx::interrupt& irq() { return irq_; }

  FakeSensorMmio& sensor_mmio() { return sensor_mmio_; }

 private:
  fdf_fake::FakePDev pdev_;
  compat::DeviceServer compat_server_;
  zx::interrupt irq_;
  FakeSensorMmio sensor_mmio_;
};

class FixtureConfig final {
 public:
  using DriverType = AmlTripDevice;
  using EnvironmentType = AmlTripTestEnvironment;
};

class AmlTripTest : public ::testing::Test {
 public:
  static constexpr std::string_view kTestName = "test-trip-device";

  void SetUp() override {
    driver_test_.RunInEnvironmentTypeContext([](auto& env) { env.Init(std::string{kTestName}); });
    ASSERT_OK(driver_test_.StartDriver());
  }

  void TearDown() override { ASSERT_OK(driver_test_.StopDriver()); }

 protected:
  fidl::ClientEnd<fuchsia_hardware_trippoint::TripPoint> trippoint_client() {
    zx::result client = driver_test_.Connect<fuchsia_hardware_temperature::Service::Trippoint>(
        AmlTrip::kChildNodeName);
    EXPECT_TRUE(client.is_ok());
    return std::move(client.value());
  }

  fidl::ClientEnd<fuchsia_hardware_temperature::Device> temperature_client() {
    zx::result client = driver_test_.Connect<fuchsia_hardware_temperature::Service::Device>(
        AmlTrip::kChildNodeName);
    EXPECT_TRUE(client.is_ok());
    return std::move(client.value());
  }

  void TriggerIrq() {
    driver_test_.RunInEnvironmentTypeContext(
        [](auto& env) { env.irq().trigger(0, zx::clock::get_boot()); });
  }

  void SetIrqStat(uint32_t index, bool stat) {
    driver_test_.RunInEnvironmentTypeContext(
        [index, stat](auto& env) { env.sensor_mmio().SetIrqStat(index, stat); });
  }

 private:
  fdf_testing::BackgroundDriverTest<FixtureConfig> driver_test_;
};

TEST_F(AmlTripTest, TestReadTemperature) {
  auto result = fidl::WireCall(temperature_client())->GetTemperatureCelsius();

  ASSERT_TRUE(result.ok());

  // Expect the measured temperature to be within 0.01C of 41.9 for floating
  // point error.
  EXPECT_LT(abs(result->temp - 41.9), 0.01);
}

TEST_F(AmlTripTest, TestGetSensorName) {
  auto result = fidl::WireCall(temperature_client())->GetSensorName();

  ASSERT_TRUE(result.ok());

  const std::string name(result->name.begin(), result->name.end());

  EXPECT_EQ(name, kTestName);
}

TEST_F(AmlTripTest, TestGetTripPointDescriptors) {
  auto result = fidl::WireCall(trippoint_client())->GetTripPointDescriptors();

  ASSERT_TRUE(result.ok());

  for (const auto& desc : result.value()->descriptors) {
    if (desc.index < 4) {
      EXPECT_EQ(desc.type, fuchsia_hardware_trippoint::TripPointType::kOneshotTempAbove);
    } else {
      EXPECT_EQ(desc.type, fuchsia_hardware_trippoint::TripPointType::kOneshotTempBelow);
    }
    EXPECT_TRUE(desc.configuration.is_cleared_trip_point());
  }
}

TEST_F(AmlTripTest, TestSetTripBadIndex) {
  fuchsia_hardware_trippoint::wire::OneshotTempAboveTripPoint tp;
  tp.critical_temperature_celsius = 30.0;

  fuchsia_hardware_trippoint::wire::TripPointDescriptor desc;
  desc.index = 8;  // This is out of range!
  desc.type = fuchsia_hardware_trippoint::wire::TripPointType::kOneshotTempAbove;
  desc.configuration =
      fuchsia_hardware_trippoint::wire::TripPointValue::WithOneshotTempAboveTripPoint(tp);

  std::vector<fuchsia_hardware_trippoint::wire::TripPointDescriptor> descs;
  descs.push_back(desc);

  auto descs_view =
      fidl::VectorView<fuchsia_hardware_trippoint::wire::TripPointDescriptor>::FromExternal(descs);

  auto set_result = fidl::WireCall(trippoint_client())->SetTripPoints(descs_view);
  EXPECT_TRUE(set_result.ok());
  EXPECT_TRUE(set_result->is_error());
}

TEST_F(AmlTripTest, TestSetTripBadType) {
  fidl::WireSyncClient<fuchsia_hardware_trippoint::TripPoint> client(trippoint_client());
  auto result = client->GetTripPointDescriptors();

  ASSERT_TRUE(result.ok());

  auto descs_view = result.value()->descriptors;

  std::vector<fuchsia_hardware_trippoint::wire::TripPointDescriptor> descs;

  auto rise_desc = std::find_if(
      descs_view.begin(), descs_view.end(),
      [](fuchsia_hardware_trippoint::wire::TripPointDescriptor& d) {
        return d.type == fuchsia_hardware_trippoint::wire::TripPointType::kOneshotTempAbove;
      });

  // Make sure at least one descriptor is found.
  ASSERT_NE(rise_desc, descs_view.end());

  // The hardware told us that this was a rise trip point but we're going to try to configure
  // it as a fall trip point. This should be an error.
  fuchsia_hardware_trippoint::wire::OneshotTempBelowTripPoint tp;
  tp.critical_temperature_celsius = 30.0;
  rise_desc->type = fuchsia_hardware_trippoint::wire::TripPointType::kOneshotTempBelow;
  rise_desc->configuration =
      fuchsia_hardware_trippoint::wire::TripPointValue::WithOneshotTempBelowTripPoint(tp);

  descs.push_back(*rise_desc);
  auto set_desc_view =
      fidl::VectorView<fuchsia_hardware_trippoint::wire::TripPointDescriptor>::FromExternal(descs);

  auto set_result = client->SetTripPoints(set_desc_view);
  EXPECT_TRUE(set_result.ok());
  EXPECT_TRUE(set_result->is_error());
}

TEST_F(AmlTripTest, TestSetCriticalTempInfinity) {
  fidl::WireSyncClient<fuchsia_hardware_trippoint::TripPoint> client(trippoint_client());
  auto result = client->GetTripPointDescriptors();

  ASSERT_TRUE(result.ok());

  auto descs_view = result.value()->descriptors;

  std::vector<fuchsia_hardware_trippoint::wire::TripPointDescriptor> descs;

  auto rise_desc = std::find_if(
      descs_view.begin(), descs_view.end(),
      [](fuchsia_hardware_trippoint::wire::TripPointDescriptor& d) {
        return d.type == fuchsia_hardware_trippoint::wire::TripPointType::kOneshotTempAbove;
      });

  // Make sure at least one descriptor is found.
  ASSERT_NE(rise_desc, descs_view.end());

  // It's illegal to configure the trip point temperature as Infinity.
  fuchsia_hardware_trippoint::wire::OneshotTempAboveTripPoint tp;
  tp.critical_temperature_celsius = std::numeric_limits<float>::infinity();
  rise_desc->type = fuchsia_hardware_trippoint::wire::TripPointType::kOneshotTempAbove;
  rise_desc->configuration =
      fuchsia_hardware_trippoint::wire::TripPointValue::WithOneshotTempAboveTripPoint(tp);

  descs.push_back(*rise_desc);
  auto set_desc_view =
      fidl::VectorView<fuchsia_hardware_trippoint::wire::TripPointDescriptor>::FromExternal(descs);

  auto set_result = client->SetTripPoints(set_desc_view);
  EXPECT_TRUE(set_result.ok());
  EXPECT_TRUE(set_result->is_error());
}

TEST_F(AmlTripTest, TestTypeConfigMismatch) {
  fidl::WireSyncClient<fuchsia_hardware_trippoint::TripPoint> client(trippoint_client());
  auto result = client->GetTripPointDescriptors();

  ASSERT_TRUE(result.ok());

  auto descs_view = result.value()->descriptors;

  std::vector<fuchsia_hardware_trippoint::wire::TripPointDescriptor> descs;

  auto rise_desc = std::find_if(
      descs_view.begin(), descs_view.end(),
      [](fuchsia_hardware_trippoint::wire::TripPointDescriptor& d) {
        return d.type == fuchsia_hardware_trippoint::wire::TripPointType::kOneshotTempAbove;
      });

  // Make sure at least one descriptor is found.
  ASSERT_NE(rise_desc, descs_view.end());

  // The hardware told us that this was a rise trip point but we're going to try to configure
  // it as a fall trip point. Unlike the test above, we're not going to flip the
  // trip point type field.
  fuchsia_hardware_trippoint::wire::OneshotTempBelowTripPoint tp;
  tp.critical_temperature_celsius = 30.0;
  rise_desc->configuration =
      fuchsia_hardware_trippoint::wire::TripPointValue::WithOneshotTempBelowTripPoint(tp);

  descs.push_back(*rise_desc);
  auto set_desc_view =
      fidl::VectorView<fuchsia_hardware_trippoint::wire::TripPointDescriptor>::FromExternal(descs);

  auto set_result = client->SetTripPoints(set_desc_view);
  EXPECT_TRUE(set_result.ok());
  EXPECT_TRUE(set_result->is_error());
}

TEST_F(AmlTripTest, TestTripPointSuccess) {
  fidl::WireSyncClient<fuchsia_hardware_trippoint::TripPoint> client(trippoint_client());
  auto result = client->GetTripPointDescriptors();

  ASSERT_TRUE(result.ok());

  auto descs_view = result.value()->descriptors;

  std::vector<fuchsia_hardware_trippoint::wire::TripPointDescriptor> descs;

  auto rise_desc = std::find_if(
      descs_view.begin(), descs_view.end(),
      [](fuchsia_hardware_trippoint::wire::TripPointDescriptor& d) {
        return d.type == fuchsia_hardware_trippoint::wire::TripPointType::kOneshotTempAbove;
      });

  // Make sure at least one descriptor is found.
  ASSERT_NE(rise_desc, descs_view.end());

  fuchsia_hardware_trippoint::wire::OneshotTempAboveTripPoint tp;
  tp.critical_temperature_celsius = 30.0;
  rise_desc->configuration =
      fuchsia_hardware_trippoint::wire::TripPointValue::WithOneshotTempAboveTripPoint(tp);

  descs.push_back(*rise_desc);
  auto set_desc_view =
      fidl::VectorView<fuchsia_hardware_trippoint::wire::TripPointDescriptor>::FromExternal(descs);

  auto set_result = client->SetTripPoints(set_desc_view);
  EXPECT_TRUE(set_result.ok());
  EXPECT_TRUE(set_result->is_ok());

  auto get_result = client->GetTripPointDescriptors();
  ASSERT_TRUE(get_result.ok());

  // Trigger an interrupt and set the registers to make it look like an
  // interrupt is pending.
  SetIrqStat(rise_desc->index, true);

  TriggerIrq();

  auto wait_result = client->WaitForAnyTripPoint();

  EXPECT_TRUE(wait_result.ok());
  EXPECT_TRUE(wait_result->is_ok());
}

TEST_F(AmlTripTest, TestTwoTripPointSuccess) {
  fidl::WireSyncClient<fuchsia_hardware_trippoint::TripPoint> client(trippoint_client());
  auto result = client->GetTripPointDescriptors();

  ASSERT_TRUE(result.ok());

  auto descs_view = result.value()->descriptors;

  std::vector<fuchsia_hardware_trippoint::wire::TripPointDescriptor> descs;

  auto rise_desc = std::find_if(
      descs_view.begin(), descs_view.end(),
      [](fuchsia_hardware_trippoint::wire::TripPointDescriptor& d) {
        return d.type == fuchsia_hardware_trippoint::wire::TripPointType::kOneshotTempAbove;
      });

  auto fall_desc = std::find_if(
      descs_view.begin(), descs_view.end(),
      [](fuchsia_hardware_trippoint::wire::TripPointDescriptor& d) {
        return d.type == fuchsia_hardware_trippoint::wire::TripPointType::kOneshotTempBelow;
      });

  // Make sure at least one descriptor is found.
  ASSERT_NE(rise_desc, descs_view.end());
  ASSERT_NE(fall_desc, descs_view.end());

  fuchsia_hardware_trippoint::wire::OneshotTempAboveTripPoint rise_tp;
  rise_tp.critical_temperature_celsius = 30.0;
  rise_desc->configuration =
      fuchsia_hardware_trippoint::wire::TripPointValue::WithOneshotTempAboveTripPoint(rise_tp);

  fuchsia_hardware_trippoint::wire::OneshotTempBelowTripPoint fall_tp;
  fall_tp.critical_temperature_celsius = 50.0;
  fall_desc->configuration =
      fuchsia_hardware_trippoint::wire::TripPointValue::WithOneshotTempBelowTripPoint(fall_tp);

  descs.push_back(*rise_desc);
  descs.push_back(*fall_desc);

  auto set_desc_view =
      fidl::VectorView<fuchsia_hardware_trippoint::wire::TripPointDescriptor>::FromExternal(descs);

  auto set_result = client->SetTripPoints(set_desc_view);
  EXPECT_TRUE(set_result.ok());
  EXPECT_TRUE(set_result->is_ok());

  // Trigger an interrupt and set the registers to make it look like an
  // interrupt is pending.
  SetIrqStat(rise_desc->index, true);
  SetIrqStat(fall_desc->index, true);

  TriggerIrq();

  auto first_result = client->WaitForAnyTripPoint();
  EXPECT_TRUE(first_result.ok());
  EXPECT_TRUE(first_result->is_ok());

  auto second_result = client->WaitForAnyTripPoint();
  EXPECT_TRUE(second_result.ok());
  EXPECT_TRUE(second_result->is_ok());
}

TEST_F(AmlTripTest, TestWaitNoConfigured) {
  auto wait_result = fidl::WireCall(trippoint_client())->WaitForAnyTripPoint();

  // Waiting without configuring any trips is an error.
  EXPECT_TRUE(wait_result.ok());
  EXPECT_FALSE(wait_result->is_ok());
}

TEST_F(AmlTripTest, TestClearTripPointAfterFire) {
  fidl::WireSyncClient<fuchsia_hardware_trippoint::TripPoint> client(trippoint_client());
  auto result = client->GetTripPointDescriptors();

  ASSERT_TRUE(result.ok());

  auto descs_view = result.value()->descriptors;

  std::vector<fuchsia_hardware_trippoint::wire::TripPointDescriptor> descs;

  auto rise_desc = std::find_if(
      descs_view.begin(), descs_view.end(),
      [](fuchsia_hardware_trippoint::wire::TripPointDescriptor& d) {
        return d.type == fuchsia_hardware_trippoint::wire::TripPointType::kOneshotTempAbove;
      });

  // Make sure at least one descriptor is found.
  ASSERT_NE(rise_desc, descs_view.end());

  fuchsia_hardware_trippoint::wire::OneshotTempAboveTripPoint tp;
  tp.critical_temperature_celsius = 30.0;
  rise_desc->configuration =
      fuchsia_hardware_trippoint::wire::TripPointValue::WithOneshotTempAboveTripPoint(tp);

  descs.push_back(*rise_desc);
  auto set_desc_view =
      fidl::VectorView<fuchsia_hardware_trippoint::wire::TripPointDescriptor>::FromExternal(descs);

  auto set_result = client->SetTripPoints(set_desc_view);
  EXPECT_TRUE(set_result.ok());
  EXPECT_TRUE(set_result->is_ok());

  // Trigger an interrupt and set the registers to make it look like an
  // interrupt is pending.
  SetIrqStat(rise_desc->index, true);

  TriggerIrq();

  // Clear the trip point that just fired.
  fuchsia_hardware_trippoint::wire::ClearedTripPoint cleared;
  descs.at(0).configuration =
      fuchsia_hardware_trippoint::wire::TripPointValue::WithClearedTripPoint(cleared);
  auto set_clear_result = client->SetTripPoints(set_desc_view);
  EXPECT_TRUE(set_clear_result.ok());
  EXPECT_TRUE(set_clear_result->is_ok());

  auto wait_result = client->WaitForAnyTripPoint();

  EXPECT_TRUE(wait_result.ok());
  EXPECT_FALSE(wait_result->is_ok());  // No trip points should be configured.
}

}  // namespace temperature::test
