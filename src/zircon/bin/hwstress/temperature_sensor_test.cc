// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "temperature_sensor.h"

#include <fidl/fuchsia.hardware.thermal/cpp/wire.h>
#include <fidl/fuchsia.hardware.thermal/cpp/wire_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <stdio.h>
#include <zircon/compiler.h>
#include <zircon/status.h>

#include <cmath>
#include <optional>

#include <gtest/gtest.h>

namespace hwstress {
namespace {

class FakeThermalServer : public fidl::testing::WireTestBase<fuchsia_hardware_thermal::Device> {
 public:
  explicit FakeThermalServer(float reported_temperature)
      : reported_temperature_(reported_temperature) {}

  void GetTemperatureCelsius(GetTemperatureCelsiusCompleter::Sync& completer) override {
    completer.Reply(ZX_OK, reported_temperature_);
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

 private:
  float reported_temperature_;
};

TEST(TemperatureSensor, NullSensor) {
  ASSERT_EQ(std::nullopt, CreateNullTemperatureSensor()->ReadCelcius());
}

TEST(TemperatureSensor, SystemTemperatureSensor) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  ASSERT_EQ(loop.StartThread("fake-thermal-server-thread"), ZX_OK);

  // Create a server that always reports a single, static value.
  FakeThermalServer server{42.0};

  // Create endpoints.
  auto endpoints = fidl::Endpoints<fuchsia_hardware_thermal::Device>::Create();

  // Bind the server.
  fidl::BindServer(loop.dispatcher(), std::move(endpoints.server), &server);

  // Ensure we can read from the server.
  auto sensor = CreateSystemTemperatureSensor(endpoints.client.TakeChannel());
  ASSERT_EQ(42.0, sensor->ReadCelcius());
  ASSERT_EQ(42.0, sensor->ReadCelcius());
  ASSERT_EQ(42.0, sensor->ReadCelcius());

  // Close the server by shutting down the loop. Ensure that we get nullopt results.
  loop.Shutdown();
  ASSERT_EQ(std::nullopt, sensor->ReadCelcius());
}

TEST(TemperatureToString, Basic) {
  // Normal values.
  EXPECT_EQ(TemperatureToString(1.0), "1.0°C");
  EXPECT_EQ(TemperatureToString(-1.0), "-1.0°C");
  EXPECT_EQ(TemperatureToString(100.0), "100.0°C");
  EXPECT_EQ(TemperatureToString(3.14159265359), "3.1°C");

  // Unknown value.
  EXPECT_EQ(TemperatureToString(std::nullopt), "unknown");

  // We don't expect these temperatures, but we shouldn't crash.
  EXPECT_EQ(TemperatureToString(std::numeric_limits<double>::infinity()), "inf°C");
  EXPECT_EQ(TemperatureToString(std::nan("")), "nan°C");
}

}  // namespace
}  // namespace hwstress
