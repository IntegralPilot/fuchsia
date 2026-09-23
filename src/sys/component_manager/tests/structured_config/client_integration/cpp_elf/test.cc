// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/cpp/reader.h>

#include <gtest/gtest.h>

#include "lib/inspect/cpp/vmo/types.h"
#include "src/sys/component_manager/tests/structured_config/client_integration/cpp_elf/receiver_config.h"

receiver_config::Config CreateGolden() {
  receiver_config::Config config;
  config.my_flag() = true;
  config.my_uint8() = 255;
  config.my_uint16() = 65535;
  config.my_uint32() = 4000000000;
  config.my_uint64() = 8000000000;
  config.my_int8() = -127;
  config.my_int16() = -32766;
  config.my_int32() = -2000000000;
  config.my_int64() = -4000000000;
  config.my_string() = "hello, world!";
  config.my_vector_of_flag() = {true, false};
  config.my_vector_of_uint8() = {1, 2, 3};
  config.my_vector_of_uint16() = {2, 3, 4};
  config.my_vector_of_uint32() = {3, 4, 5};
  config.my_vector_of_uint64() = {4, 5, 6};
  config.my_vector_of_int8() = {-1, -2, 3};
  config.my_vector_of_int16() = {-2, -3, 4};
  config.my_vector_of_int32() = {-3, -4, 5};
  config.my_vector_of_int64() = {-4, -5, 6};
  config.my_vector_of_string() = {"hello, world!", "hello, again!"};
  return config;
}

void CheckConfig(const receiver_config::Config& c) {
  receiver_config::Config golden = CreateGolden();
  ASSERT_EQ(c.my_flag(), golden.my_flag());
  ASSERT_EQ(c.my_uint8(), golden.my_uint8());
  ASSERT_EQ(c.my_uint16(), golden.my_uint16());
  ASSERT_EQ(c.my_uint32(), golden.my_uint32());
  ASSERT_EQ(c.my_uint64(), golden.my_uint64());
  ASSERT_EQ(c.my_int8(), golden.my_int8());
  ASSERT_EQ(c.my_int16(), golden.my_int16());
  ASSERT_EQ(c.my_int32(), golden.my_int32());
  ASSERT_EQ(c.my_int64(), golden.my_int64());
  ASSERT_EQ(c.my_vector_of_uint8(), golden.my_vector_of_uint8());
  ASSERT_EQ(c.my_vector_of_uint16(), golden.my_vector_of_uint16());
  ASSERT_EQ(c.my_vector_of_uint32(), golden.my_vector_of_uint32());
  ASSERT_EQ(c.my_vector_of_uint64(), golden.my_vector_of_uint64());
  ASSERT_EQ(c.my_vector_of_int8(), golden.my_vector_of_int8());
  ASSERT_EQ(c.my_vector_of_int16(), golden.my_vector_of_int16());
  ASSERT_EQ(c.my_vector_of_int32(), golden.my_vector_of_int32());
  ASSERT_EQ(c.my_vector_of_int64(), golden.my_vector_of_int64());
  ASSERT_EQ(c.my_vector_of_string(), golden.my_vector_of_string());
}

TEST(ClientIntegration, FromStartupHandle) {
  // NOTE: This must be the only place that takes the startup handle.
  receiver_config::Config c = receiver_config::Config::TakeFromStartupHandle();
  ASSERT_NO_FATAL_FAILURE(CheckConfig(c));
}

TEST(ClientIntegration, CreatedVmo) {
  zx::vmo config_vmo = CreateGolden().ToVmo();
  receiver_config::Config c = receiver_config::Config::CreateFromVmo(std::move(config_vmo));
  ASSERT_NO_FATAL_FAILURE(CheckConfig(c));
}

TEST(ClientIntegration, Inspect) {
  receiver_config::Config config = CreateGolden();

  inspect::Inspector inspector;
  inspect::Node inspect_config = inspector.GetRoot().CreateChild("config");
  config.RecordInspect(&inspect_config);

  auto root = inspect::ReadFromVmo(inspector.DuplicateVmo()).take_value();
  auto config_node = root.GetByPath({"config"});

  ASSERT_EQ(config_node->node().get_property<inspect::BoolPropertyValue>("my_flag")->value(),
            config.my_flag());
  ASSERT_EQ(config_node->node().get_property<inspect::UintPropertyValue>("my_uint8")->value(),
            config.my_uint8());
  ASSERT_EQ(config_node->node().get_property<inspect::UintPropertyValue>("my_uint16")->value(),
            config.my_uint16());
  ASSERT_EQ(config_node->node().get_property<inspect::UintPropertyValue>("my_uint32")->value(),
            config.my_uint32());
  ASSERT_EQ(config_node->node().get_property<inspect::UintPropertyValue>("my_uint64")->value(),
            config.my_uint64());
  ASSERT_EQ(config_node->node().get_property<inspect::IntPropertyValue>("my_int8")->value(),
            config.my_int8());
  ASSERT_EQ(config_node->node().get_property<inspect::IntPropertyValue>("my_int16")->value(),
            config.my_int16());
  ASSERT_EQ(config_node->node().get_property<inspect::IntPropertyValue>("my_int32")->value(),
            config.my_int32());
  ASSERT_EQ(config_node->node().get_property<inspect::IntPropertyValue>("my_int64")->value(),
            config.my_int64());
  ASSERT_EQ(config_node->node().get_property<inspect::StringPropertyValue>("my_string")->value(),
            config.my_string());
  ASSERT_EQ(
      config_node->node().get_property<inspect::StringArrayValue>("my_vector_of_string")->value(),
      config.my_vector_of_string());
  ASSERT_EQ(
      config_node->node().get_property<inspect::UintArrayValue>("my_vector_of_uint64")->value(),
      config.my_vector_of_uint64());
  ASSERT_EQ(config_node->node().get_property<inspect::IntArrayValue>("my_vector_of_int64")->value(),
            config.my_vector_of_int64());
}
