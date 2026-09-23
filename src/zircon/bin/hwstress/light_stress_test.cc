// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "light_stress.h"

#include <fuchsia/hardware/light/cpp/fidl_test_base.h>

#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "testing_util.h"

using ::testing::ElementsAre;

namespace hwstress {
namespace {

class FakeLightServer : public fuchsia::hardware::light::testing::Light_TestBase {
 public:
  struct Light {
    std::string name;
    fuchsia::hardware::light::Capability capability;
    double brightness;
  };

  explicit FakeLightServer(std::vector<Light> lights) : lights_(std::move(lights)) {}

  // Return internal list of lights.
  const std::vector<Light>& lights() { return lights_; }

  // Implementation of |Light| methods.
  void GetNumLights(GetNumLightsCallback callback) override {
    return callback(static_cast<uint32_t>(lights_.size()));
  }
  void GetInfo(uint32_t index, GetInfoCallback callback) override {
    fuchsia::hardware::light::Light_GetInfo_Response response;
    response.info.capability = lights_.at(index).capability;
    response.info.name = lights_.at(index).name;
    callback(fuchsia::hardware::light::Light_GetInfo_Result::WithResponse(std::move(response)));
  }
  void SetBrightnessValue(uint32_t index, double value,
                          SetBrightnessValueCallback callback) override {
    lights_.at(index).brightness = value;
    callback(fuchsia::hardware::light::Light_SetBrightnessValue_Result::WithResponse(
        fuchsia::hardware::light::Light_SetBrightnessValue_Response()));
  }
  void SetSimpleValue(uint32_t index, bool value, SetSimpleValueCallback callback) override {
    lights_.at(index).brightness = value ? 1.0 : 0.0;
    callback(fuchsia::hardware::light::Light_SetSimpleValue_Result::WithResponse(
        fuchsia::hardware::light::Light_SetSimpleValue_Response()));
  }
  void SetRgbValue(uint32_t index, fuchsia::hardware::light::Rgb value,
                   SetRgbValueCallback callback) override {
    lights_.at(index).brightness =
        (value.red > 0.0 || value.green > 0.0 || value.blue > 0.0) ? 1.0 : 0.0;
    callback(fuchsia::hardware::light::Light_SetRgbValue_Result::WithResponse(
        fuchsia::hardware::light::Light_SetRgbValue_Response()));
  }

  // Callback when a unimplemented FIDL method is called.
  void NotImplemented_(const std::string& name) override {
    ZX_PANIC("Unimplemented: %s", name.c_str());
  }

 private:
  std::vector<Light> lights_;
};

TEST(LightStress, GetLights) {
  // Create a light server exposing three lights.
  FakeLightServer server{{
      FakeLightServer::Light{
          .name = "A",
          .capability = fuchsia::hardware::light::Capability::BRIGHTNESS,
      },
      FakeLightServer::Light{
          .name = "B",
          .capability = fuchsia::hardware::light::Capability::SIMPLE,
      },
      FakeLightServer::Light{
          .name = "C",
          .capability = fuchsia::hardware::light::Capability::RGB,
      },
  }};

  // Create a connection to the server.
  auto factory = std::make_unique<testing::LoopbackConnectionFactory>();
  auto client = factory->CreateSyncPtrTo<fuchsia::hardware::light::Light>(&server);

  // Query light server information.
  std::vector<LightInfo> lights = GetLights(client).value();

  // Ensure we detected the supported lights, and the index of each is correct.
  EXPECT_THAT(lights,
              ElementsAre(LightInfo{"A", 0, fuchsia::hardware::light::Capability::BRIGHTNESS},
                          LightInfo{"B", 1, fuchsia::hardware::light::Capability::SIMPLE},
                          LightInfo{"C", 2, fuchsia::hardware::light::Capability::RGB}));
}

TEST(LightStress, TurnLightOnOff) {
  // Create a light server exposing three lights of different capabilities.
  FakeLightServer server{{
      FakeLightServer::Light{
          .name = "A",
          .capability = fuchsia::hardware::light::Capability::BRIGHTNESS,
      },
      FakeLightServer::Light{
          .name = "B",
          .capability = fuchsia::hardware::light::Capability::SIMPLE,
      },
      FakeLightServer::Light{
          .name = "C",
          .capability = fuchsia::hardware::light::Capability::RGB,
      },
  }};

  // Create a connection to the server.
  auto factory = std::make_unique<testing::LoopbackConnectionFactory>();
  auto client = factory->CreateSyncPtrTo<fuchsia::hardware::light::Light>(&server);

  // Test BRIGHTNESS light.
  {
    LightInfo info{
        .name = "A", .index = 0, .capability = fuchsia::hardware::light::Capability::BRIGHTNESS};
    ASSERT_TRUE(TurnOnLight(client, info).is_ok());
    EXPECT_EQ(server.lights().at(0).brightness, 1.0);
    ASSERT_TRUE(TurnOffLight(client, info).is_ok());
    EXPECT_EQ(server.lights().at(0).brightness, 0.0);
  }

  // Test SIMPLE light.
  {
    LightInfo info{
        .name = "B", .index = 1, .capability = fuchsia::hardware::light::Capability::SIMPLE};
    ASSERT_TRUE(TurnOnLight(client, info).is_ok());
    EXPECT_EQ(server.lights().at(1).brightness, 1.0);
    ASSERT_TRUE(TurnOffLight(client, info).is_ok());
    EXPECT_EQ(server.lights().at(1).brightness, 0.0);
  }

  // Test RGB light.
  {
    LightInfo info{
        .name = "C", .index = 2, .capability = fuchsia::hardware::light::Capability::RGB};
    ASSERT_TRUE(TurnOnLight(client, info).is_ok());
    EXPECT_EQ(server.lights().at(2).brightness, 1.0);
    ASSERT_TRUE(TurnOffLight(client, info).is_ok());
    EXPECT_EQ(server.lights().at(2).brightness, 0.0);
  }
}

}  // namespace
}  // namespace hwstress
