// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <bind/fuchsia/cpp/bind.h>
#include <gtest/gtest.h>

#include "src/devices/i2c/drivers/i2c/i2c-test-env.h"
#include "src/devices/i2c/drivers/i2c/i2c_config.h"

namespace i2c {

namespace {

fuchsia_hardware_i2c_businfo::I2CChannel CreateChannel(uint32_t address, uint32_t i2c_class,
                                                       uint32_t vid, uint32_t pid, uint32_t did) {
  return fuchsia_hardware_i2c_businfo::I2CChannel{{
      .address = address,
      .i2c_class = i2c_class,
      .vid = vid,
      .pid = pid,
      .did = did,
  }};
}

class I2cDriverTest : public ::testing::Test {
 public:
  void Init(const fuchsia_hardware_i2c_businfo::I2CBusMetadata& metadata) {
    test_runner.RunInEnvironmentTypeContext(
        [metadata](TestEnvironment& env) { env.AddMetadata(metadata); });
    EXPECT_TRUE(test_runner
                    .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                      i2c_config::Config config{{.enable_suspend = true}};
                      args.config(config.ToVmo());
                    })
                    .is_ok());
  }

  void InitGeneric(fuchsia_hardware_i2c_businfo::I2CBusMetadata metadata) {
    test_runner.RunInEnvironmentTypeContext(
        [metadata = std::move(metadata)](TestEnvironment& env) { env.InitGeneric(metadata); });
    EXPECT_TRUE(test_runner
                    .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                      i2c_config::Config config{{.enable_suspend = true}};
                      args.config(config.ToVmo());
                    })
                    .is_ok());
  }

  void TearDown() override {
    zx::result<> result = test_runner.StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

 protected:
  fdf_testing::ForegroundDriverTest<TestConfig> test_runner;
};

TEST_F(I2cDriverTest, OneChannel) {
  const fuchsia_hardware_i2c_businfo::I2CChannel kChannel = CreateChannel(5, 10, 2, 4, 6);
  const uint32_t kBusId = 32;

  Init(fuchsia_hardware_i2c_businfo::I2CBusMetadata{{
      .channels{{kChannel}},
      .bus_id = kBusId,
  }});

  test_runner.RunInNodeContext([](fdf_testing::TestNode& node) {
    ASSERT_EQ(1u, node.children().count("i2c"));
    fdf_testing::TestNode& i2c_node = node.children().at("i2c");

    EXPECT_EQ(1u, i2c_node.children().size());
    EXPECT_TRUE(i2c_node.children().count("i2c-32-5"));
  });
}

TEST_F(I2cDriverTest, NoMetadata) {
  EXPECT_TRUE(test_runner
                  .StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
                    i2c_config::Config config{{.enable_suspend = true}};
                    args.config(config.ToVmo());
                  })
                  .is_error());
}

TEST_F(I2cDriverTest, MultipleChannels) {
  const fuchsia_hardware_i2c_businfo::I2CChannel kChannel1 = CreateChannel(5, 10, 2, 4, 6);
  const fuchsia_hardware_i2c_businfo::I2CChannel kChannel2 = CreateChannel(15, 30, 3, 6, 9);
  const uint32_t kBusId = 32;

  Init(fuchsia_hardware_i2c_businfo::I2CBusMetadata{{
      .channels{{kChannel1, kChannel2}},
      .bus_id = kBusId,
  }});

  test_runner.RunInNodeContext([](fdf_testing::TestNode& node) {
    ASSERT_EQ(1u, node.children().count("i2c"));
    fdf_testing::TestNode& i2c_node = node.children().at("i2c");

    EXPECT_EQ(2u, i2c_node.children().size());
    EXPECT_TRUE(i2c_node.children().count("i2c-32-5"));
    EXPECT_TRUE(i2c_node.children().count("i2c-32-15"));
  });
}

TEST_F(I2cDriverTest, GetName) {
  const std::string kTestChildName = "i2c-16-5";
  const std::string kChannelName = "xyz";
  constexpr uint32_t kGlobalId = 42;
  fuchsia_hardware_i2c_businfo::I2CChannel channel = CreateChannel(5, 10, 2, 4, 6);
  channel.name(kChannelName);
  channel.global_id(kGlobalId);

  constexpr uint32_t kBusId = 16;

  Init(fuchsia_hardware_i2c_businfo::I2CBusMetadata{{
      .channels{{channel}},
      .bus_id = kBusId,
  }});

  test_runner.RunInNodeContext([&](fdf_testing::TestNode& node) {
    ASSERT_EQ(1u, node.children().count("i2c"));
    fdf_testing::TestNode& i2c_node = node.children().at("i2c");
    ASSERT_TRUE(i2c_node.children().count(kTestChildName));

    auto properties = i2c_node.children().at(kTestChildName).GetProperties();
    auto name_prop = std::ranges::find_if(
        properties, [](const auto& prop) { return prop.key() == bind_fuchsia::NAME; });
    ASSERT_NE(name_prop, properties.end());
    ASSERT_EQ(name_prop->value().Which(),
              fuchsia_driver_framework::NodePropertyValue::Tag::kStringValue);
    EXPECT_EQ(name_prop->value().string_value().value(), kChannelName);

    auto id_prop = std::ranges::find_if(
        properties, [](const auto& prop) { return prop.key() == bind_fuchsia::ID; });
    ASSERT_NE(id_prop, properties.end());
    ASSERT_EQ(id_prop->value().Which(),
              fuchsia_driver_framework::NodePropertyValue::Tag::kIntValue);
    EXPECT_EQ(id_prop->value().int_value().value(), kGlobalId);
  });

  zx::result connect_result =
      test_runner.Connect<fuchsia_hardware_i2c::Service::Device>(kTestChildName);
  ASSERT_TRUE(connect_result.is_ok());
  zx::result run_result = test_runner.RunOnBackgroundDispatcherSync(
      [client_end = std::move(connect_result.value()), expected_name = kChannelName]() mutable {
        auto name_result = fidl::WireCall(client_end)->GetName();
        ASSERT_OK(name_result.status());
        ASSERT_TRUE(name_result->is_ok());
        ASSERT_EQ(std::string(name_result->value()->name.get()), expected_name);
      });
  ASSERT_OK(run_result.status_value());
}

TEST_F(I2cDriverTest, GenericMetadataTest) {
  const std::string kTestChildName = "i2c-32-5";
  const std::string kChannelName = "xyz";
  constexpr uint32_t kGlobalId = 42;
  fuchsia_hardware_i2c_businfo::I2CChannel channel = CreateChannel(5, 10, 2, 4, 6);
  channel.name(kChannelName);
  channel.global_id(kGlobalId);
  const uint32_t kBusId = 32;

  InitGeneric(fuchsia_hardware_i2c_businfo::I2CBusMetadata{{
      .channels{{channel}},
      .bus_id = kBusId,
  }});

  test_runner.RunInNodeContext([&](fdf_testing::TestNode& node) {
    ASSERT_EQ(1u, node.children().count("i2c"));
    fdf_testing::TestNode& i2c_node = node.children().at("i2c");

    EXPECT_EQ(1u, i2c_node.children().size());
    ASSERT_TRUE(i2c_node.children().count(kTestChildName));

    auto properties = i2c_node.children().at(kTestChildName).GetProperties();
    auto name_prop = std::ranges::find_if(
        properties, [](const auto& prop) { return prop.key() == bind_fuchsia::NAME; });
    ASSERT_NE(name_prop, properties.end());
    ASSERT_EQ(name_prop->value().Which(),
              fuchsia_driver_framework::NodePropertyValue::Tag::kStringValue);
    EXPECT_EQ(name_prop->value().string_value().value(), kChannelName);

    auto id_prop = std::ranges::find_if(
        properties, [](const auto& prop) { return prop.key() == bind_fuchsia::ID; });
    ASSERT_NE(id_prop, properties.end());
    ASSERT_EQ(id_prop->value().Which(),
              fuchsia_driver_framework::NodePropertyValue::Tag::kIntValue);
    EXPECT_EQ(id_prop->value().int_value().value(), kGlobalId);
  });
}

}  // namespace

}  // namespace i2c
