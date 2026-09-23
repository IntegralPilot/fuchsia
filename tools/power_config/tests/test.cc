// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.power/cpp/fidl.h>

#include <zxtest/zxtest.h>

#include "tools/power_config/lib/cpp/power_config.h"

TEST(PowerConfigTest, CheckValues) {
  zx::result power_config = power_config::Load("/pkg/data/example_power");
  ASSERT_OK(power_config);
  fuchsia_hardware_power::PowerElementConfiguration& config =
      power_config.value().power_elements()[0];
  ASSERT_EQ(config.element().value().name().value(), "my-awesome-element");

  // Check element information.
  ASSERT_EQ(config.element().value().levels().value().size(), 2);
  fuchsia_hardware_power::PowerLevel& level = config.element().value().levels().value()[0];
  ASSERT_EQ(level.name().value(), "off");
  ASSERT_EQ(level.level().value(), 0);
  ASSERT_EQ(level.transitions().value().size(), 1);
  ASSERT_EQ(level.transitions().value()[0].target_level().value(), 1);
  ASSERT_EQ(level.transitions().value()[0].latency_us().value(), 1000);

  level = config.element().value().levels().value()[1];
  ASSERT_EQ(level.name().value(), "on");
  ASSERT_EQ(level.level().value(), 1);
  ASSERT_EQ(level.transitions().value().size(), 1);
  ASSERT_EQ(level.transitions().value()[0].target_level().value(), 0);
  ASSERT_EQ(level.transitions().value()[0].latency_us().value(), 2000);

  // Check dependency information.
  ASSERT_EQ(config.dependencies().value().size(), 1);
  fuchsia_hardware_power::PowerDependency& dependency = config.dependencies().value()[0];
  ASSERT_EQ(dependency.child().value(), "my-awesome-element");
  ASSERT_EQ(dependency.parent().value().Which(),
            fuchsia_hardware_power::ParentElement::Tag::kInstanceName);
  ASSERT_EQ(dependency.parent().value().instance_name().value(), "my-rad-parent");

  ASSERT_EQ(dependency.level_deps().value().size(), 2);
  fuchsia_hardware_power::LevelTuple& level_tuple = dependency.level_deps().value()[0];
  ASSERT_EQ(level_tuple.child_level().value(), 0);
  ASSERT_EQ(level_tuple.parent_level().value(), 0);

  level_tuple = dependency.level_deps().value()[1];
  ASSERT_EQ(level_tuple.child_level().value(), 0);
  ASSERT_EQ(level_tuple.parent_level().value(), 1);
}
