// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/power/cpp/types.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

namespace fdf_power::test {

// Transition tests.

// Verify that `Transition::FromFidl()` can convert a fuchsia_hardware_power::Transition value into
// a Transition value.
TEST(TypesTest, TransitionFromNatural) {
  fuchsia_hardware_power::Transition fidl{{
      .target_level = 2,
      .latency_us = 30045,
  }};
  zx::result transition = Transition::FromFidl(fidl);
  ASSERT_TRUE(transition.is_ok()) << transition.status_string();
  ASSERT_EQ(transition->target_level, 2u);
  ASSERT_EQ(transition->latency_us, 30045u);
}

// Verify that `Transition::FromFidl()` can convert a fuchsia_hardware_power::wire::Transition value
// into a Transition value.
TEST(TypesTest, TransitionFromWire) {
  fidl::Arena arena;
  auto fidl = fuchsia_hardware_power::wire::Transition::Builder(arena)
                  .target_level(2)
                  .latency_us(30045)
                  .Build();

  zx::result transition = Transition::FromFidl(fidl);
  ASSERT_TRUE(transition.is_ok()) << transition.status_string();
  ASSERT_EQ(transition->target_level, 2u);
  ASSERT_EQ(transition->latency_us, 30045u);
}

// Verify `Transition::FromFidl()` will fail if target level is missing.
TEST(ConvertPowerConfigTest, TransitionFromNaturalMissingTargetLevel) {
  fuchsia_hardware_power::Transition fidl{{
      .latency_us = 30045,
  }};
  zx::result transition = Transition::FromFidl(fidl);
  ASSERT_EQ(transition.status_value(), ZX_ERR_INVALID_ARGS);
}

// Verify `Transition::FromFidl()` will fail if target level is missing.
TEST(ConvertPowerConfigTest, TransitionFromWireMissingTargetLevel) {
  fidl::Arena arena;
  auto fidl = fuchsia_hardware_power::wire::Transition::Builder(arena).latency_us(30045).Build();

  zx::result transition = Transition::FromFidl(fidl);
  ASSERT_EQ(transition.status_value(), ZX_ERR_INVALID_ARGS);
}

// Verify `Transition::FromFidl()` will fail if latency is missing.
TEST(ConvertPowerConfigTest, TransitionFromNaturalMissingLatency) {
  fuchsia_hardware_power::Transition fidl{{
      .target_level = 2,
  }};
  zx::result transition = Transition::FromFidl(fidl);
  ASSERT_EQ(transition.status_value(), ZX_ERR_INVALID_ARGS);
}

// Verify `Transition::FromFidl()` will fail if latency is missing.
TEST(ConvertPowerConfigTest, TransitionFromWireMissingLatency) {
  fidl::Arena arena;
  auto fidl = fuchsia_hardware_power::wire::Transition::Builder(arena).target_level(2).Build();

  zx::result transition = Transition::FromFidl(fidl);
  ASSERT_EQ(transition.status_value(), ZX_ERR_INVALID_ARGS);
}

// Level tuple tests.

// Verify that `LevelTuple::FromFidl()` can convert a fuchsia_hardware_power::LevelTuple value into
// a LevelTuple value.
TEST(TypesTest, LevelTupleFromNatural) {
  fuchsia_hardware_power::LevelTuple fidl{{
      .child_level = 56,
      .parent_level = 254,
  }};
  zx::result level_tuple = LevelTuple::FromFidl(fidl);
  ASSERT_TRUE(level_tuple.is_ok()) << level_tuple.status_string();
  ASSERT_EQ(level_tuple->child_level, 56u);
  ASSERT_EQ(level_tuple->parent_level, 254u);
}

// Verify that `LevelTuple::FromFidl()` can convert a fuchsia_hardware_power::wire::LevelTuple value
// into a LevelTuple value.
TEST(TypesTest, LevelTupleFromWire) {
  fidl::Arena arena;
  auto fidl = fuchsia_hardware_power::wire::LevelTuple::Builder(arena)
                  .child_level(56)
                  .parent_level(254)
                  .Build();

  zx::result level_tuple = LevelTuple::FromFidl(fidl);
  ASSERT_TRUE(level_tuple.is_ok()) << level_tuple.status_string();
  ASSERT_EQ(level_tuple->child_level, 56u);
  ASSERT_EQ(level_tuple->parent_level, 254u);
}

// Verify `LevelTuple::FromFidl()` will fail if child level is missing.
TEST(TypesTest, LevelTupleFromNaturalMissingChildLevel) {
  fuchsia_hardware_power::LevelTuple fidl{{
      .parent_level = 254,
  }};
  zx::result level_tuple = LevelTuple::FromFidl(fidl);
  ASSERT_EQ(level_tuple.status_value(), ZX_ERR_INVALID_ARGS);
}

// Verify `LevelTuple::FromFidl()` will fail if child level is missing.
TEST(TypesTest, LevelTupleFromWireMissingChildLevel) {
  fidl::Arena arena;
  auto fidl = fuchsia_hardware_power::wire::LevelTuple::Builder(arena).parent_level(254).Build();

  zx::result level_tuple = LevelTuple::FromFidl(fidl);
  ASSERT_EQ(level_tuple.status_value(), ZX_ERR_INVALID_ARGS);
}

// Verify `LevelTuple::FromFidl()` will fail if parent level is missing.
TEST(TypesTest, LevelTupleFromNaturalMissingParentLevel) {
  fuchsia_hardware_power::LevelTuple fidl{{
      .child_level = 56,
  }};
  zx::result level_tuple = LevelTuple::FromFidl(fidl);
  ASSERT_EQ(level_tuple.status_value(), ZX_ERR_INVALID_ARGS);
}

// Verify `LevelTuple::FromFidl()` will fail if parent level is missing.
TEST(TypesTest, LevelTupleFromWireMissingParentLevel) {
  fidl::Arena arena;
  auto fidl = fuchsia_hardware_power::wire::LevelTuple::Builder(arena).child_level(56).Build();

  zx::result level_tuple = LevelTuple::FromFidl(fidl);
  ASSERT_EQ(level_tuple.status_value(), ZX_ERR_INVALID_ARGS);
}

// Power level tests.

// Verify that `PowerLevel::FromFidl()` can convert a fuchsia_hardware_power::PowerLevel value into
// a PowerLevel value.
TEST(TypesTest, PowerLevelFromNatural) {
  fuchsia_hardware_power::PowerLevel fidl{{
      .level = 4,
      .name = "test power level",
      .transitions = {{
          {{
              .target_level = 5,
              .latency_us = 48392,
          }},
          {{
              .target_level = 90,
              .latency_us = 12334,
          }},
      }},
  }};
  zx::result power_level = PowerLevel::FromFidl(fidl);
  ASSERT_TRUE(power_level.is_ok()) << power_level.status_string();
  ASSERT_EQ(power_level->level, 4u);
  ASSERT_EQ(power_level->name, "test power level");
  ASSERT_EQ(power_level->transitions.size(), 2u);

  ASSERT_EQ(power_level->transitions[0].target_level, 5u);
  ASSERT_EQ(power_level->transitions[0].latency_us, 48392u);

  ASSERT_EQ(power_level->transitions[1].target_level, 90u);
  ASSERT_EQ(power_level->transitions[1].latency_us, 12334u);
}

// Verify that `PowerLevel::FromFidl()` can convert a fuchsia_hardware_power::wire::PowerLevel value
// into a PowerLevel value.
TEST(TypesTest, PowerLevelFromWire) {
  fidl::Arena arena;
  std::vector<fuchsia_hardware_power::wire::Transition> transitions{
      fuchsia_hardware_power::wire::Transition::Builder(arena)
          .target_level(5)
          .latency_us(48392)
          .Build(),
      fuchsia_hardware_power::wire::Transition::Builder(arena)
          .target_level(90)
          .latency_us(12334)
          .Build()};
  auto fidl =
      fuchsia_hardware_power::wire::PowerLevel::Builder(arena)
          .level(4)
          .name("test power level")
          .transitions(
              fidl::VectorView<fuchsia_hardware_power::wire::Transition>::FromExternal(transitions))
          .Build();

  zx::result power_level = PowerLevel::FromFidl(fidl);
  ASSERT_TRUE(power_level.is_ok()) << power_level.status_string();
  ASSERT_EQ(power_level->level, 4u);
  ASSERT_EQ(power_level->name, "test power level");
  ASSERT_EQ(power_level->transitions.size(), 2u);

  ASSERT_EQ(power_level->transitions[0].target_level, 5u);
  ASSERT_EQ(power_level->transitions[0].latency_us, 48392u);

  ASSERT_EQ(power_level->transitions[1].target_level, 90u);
  ASSERT_EQ(power_level->transitions[1].latency_us, 12334u);
}

// Verify `PowerLevel::FromFidl()` will fail if level is missing.
TEST(TypesTest, PowerLevelFromNaturalMissingLevel) {
  fuchsia_hardware_power::PowerLevel fidl{{
      .name = "test power level",
      .transitions = {{}},
  }};
  zx::result power_level = PowerLevel::FromFidl(fidl);
  ASSERT_EQ(power_level.status_value(), ZX_ERR_INVALID_ARGS);
}

// Verify `PowerLevel::FromFidl()` will fail if level is missing.
TEST(TypesTest, PowerLevelFromWireMissingLevel) {
  fidl::Arena arena;
  std::vector<fuchsia_hardware_power::wire::Transition> transitions;
  auto fidl =
      fuchsia_hardware_power::wire::PowerLevel::Builder(arena)
          .name("test power level")
          .transitions(
              fidl::VectorView<fuchsia_hardware_power::wire::Transition>::FromExternal(transitions))
          .Build();

  zx::result power_level = PowerLevel::FromFidl(fidl);
  ASSERT_EQ(power_level.status_value(), ZX_ERR_INVALID_ARGS);
}

// Verify `PowerLevel::FromFidl()` will fail if name is missing.
TEST(TypesTest, PowerLevelFromNaturalMissingName) {
  fuchsia_hardware_power::PowerLevel fidl{{
      .level = 4,
      .transitions = {{}},
  }};
  zx::result power_level = PowerLevel::FromFidl(fidl);
  ASSERT_EQ(power_level.status_value(), ZX_ERR_INVALID_ARGS);
}

// Verify `PowerLevel::FromFidl()` will fail if name is missing.
TEST(TypesTest, PowerLevelFromWireMissingName) {
  fidl::Arena arena;
  std::vector<fuchsia_hardware_power::wire::Transition> transitions;
  auto fidl =
      fuchsia_hardware_power::wire::PowerLevel::Builder(arena)
          .level(4)
          .transitions(
              fidl::VectorView<fuchsia_hardware_power::wire::Transition>::FromExternal(transitions))
          .Build();

  zx::result power_level = PowerLevel::FromFidl(fidl);
  ASSERT_EQ(power_level.status_value(), ZX_ERR_INVALID_ARGS);
}

// Verify `PowerLevel::FromFidl()` will succeed if transitions are missing.
TEST(TypesTest, PowerLevelFromNaturalMissingTransitions) {
  fuchsia_hardware_power::PowerLevel fidl{{
      .level = 4,
      .name = "test power level",
  }};
  zx::result power_level = PowerLevel::FromFidl(fidl);
  ASSERT_TRUE(power_level.is_ok()) << power_level.status_string();
  ASSERT_TRUE(power_level->transitions.empty());
}

// Verify `PowerLevel::FromFidl()` will succeed if transitions are missing.
TEST(TypesTest, PowerLevelFromWireMissingTransitions) {
  fidl::Arena arena;
  auto fidl = fuchsia_hardware_power::wire::PowerLevel::Builder(arena)
                  .level(4)
                  .name("test power level")
                  .Build();

  zx::result power_level = PowerLevel::FromFidl(fidl);
  ASSERT_TRUE(power_level.is_ok()) << power_level.status_string();
  ASSERT_TRUE(power_level->transitions.empty());
}

// Power element tests.

// Verify that `PowerElement::FromFidl()` can convert a fuchsia_hardware_power::PowerElement value
// into a PowerElement value.
TEST(TypesTest, PowerElementFromNatural) {
  fuchsia_hardware_power::PowerElement fidl{
      {.name = "test power element",
       .levels = {{
           {{.level = 200, .name = "test level 1", .transitions = {{}}}},
           {{.level = 134, .name = "test level 2", .transitions = {{}}}},
       }}}};
  zx::result power_element = PowerElement::FromFidl(fidl);
  ASSERT_TRUE(power_element.is_ok()) << power_element.status_string();
  ASSERT_EQ(power_element->name, "test power element");
  ASSERT_EQ(power_element->levels.size(), 2u);

  ASSERT_EQ(power_element->levels[0].level, 200u);
  ASSERT_EQ(power_element->levels[0].name, "test level 1");
  ASSERT_TRUE(power_element->levels[0].transitions.empty());

  ASSERT_EQ(power_element->levels[1].level, 134u);
  ASSERT_EQ(power_element->levels[1].name, "test level 2");
  ASSERT_TRUE(power_element->levels[1].transitions.empty());
}

// Verify that `PowerElement::FromFidl()` can convert a fuchsia_hardware_power::wire::PowerElement
// value into a PowerElement value.
TEST(TypesTest, PowerElementFromWire) {
  fidl::Arena arena;
  std::vector<fuchsia_hardware_power::wire::PowerLevel> levels{
      fuchsia_hardware_power::wire::PowerLevel::Builder(arena)
          .level(200)
          .name("test level 1")
          .Build(),
      fuchsia_hardware_power::wire::PowerLevel::Builder(arena)
          .level(134)
          .name("test level 2")
          .Build(),

  };
  auto fidl =
      fuchsia_hardware_power::wire::PowerElement::Builder(arena)
          .name("test power element")
          .levels(fidl::VectorView<fuchsia_hardware_power::wire::PowerLevel>::FromExternal(levels))
          .Build();

  zx::result power_element = PowerElement::FromFidl(fidl);
  ASSERT_TRUE(power_element.is_ok()) << power_element.status_string();
  ASSERT_EQ(power_element->name, "test power element");
  ASSERT_EQ(power_element->levels.size(), 2u);

  ASSERT_EQ(power_element->levels[0].level, 200u);
  ASSERT_EQ(power_element->levels[0].name, "test level 1");
  ASSERT_TRUE(power_element->levels[0].transitions.empty());

  ASSERT_EQ(power_element->levels[1].level, 134u);
  ASSERT_EQ(power_element->levels[1].name, "test level 2");
  ASSERT_TRUE(power_element->levels[1].transitions.empty());
}

// Verify `PowerElement::FromFidl()` will fail if name is missing.
TEST(TypesTest, PowerElementFromNaturalMissingName) {
  fuchsia_hardware_power::PowerElement fidl{{.levels = {{}}}};
  zx::result power_element = PowerElement::FromFidl(fidl);
  ASSERT_EQ(power_element.status_value(), ZX_ERR_INVALID_ARGS);
}

// Verify `PowerElement::FromFidl()` will fail if name is missing.
TEST(TypesTest, PowerElementFromWireMissingName) {
  fidl::Arena arena;
  std::vector<fuchsia_hardware_power::wire::PowerLevel> levels;
  auto fidl =
      fuchsia_hardware_power::wire::PowerElement::Builder(arena)
          .levels(fidl::VectorView<fuchsia_hardware_power::wire::PowerLevel>::FromExternal(levels))
          .Build();

  zx::result power_element = PowerElement::FromFidl(fidl);
  ASSERT_EQ(power_element.status_value(), ZX_ERR_INVALID_ARGS);
}

// Verify `PowerElement::FromFidl()` will succeed if levels are missing.
TEST(TypesTest, PowerElementFromNaturalMissingLevels) {
  fuchsia_hardware_power::PowerElement fidl{{
      .name = "test power element",
  }};
  zx::result power_element = PowerElement::FromFidl(fidl);
  ASSERT_TRUE(power_element.is_ok()) << power_element.status_string();
  ASSERT_TRUE(power_element->levels.empty());
}

// Verify `PowerElement::FromFidl()` will succeed if levels are missing.
TEST(TypesTest, PowerElementFromWireMissingLevels) {
  fidl::Arena arena;
  auto fidl =
      fuchsia_hardware_power::wire::PowerElement::Builder(arena).name("test power element").Build();

  zx::result power_element = PowerElement::FromFidl(fidl);
  ASSERT_TRUE(power_element.is_ok()) << power_element.status_string();
  ASSERT_TRUE(power_element->levels.empty());
}

// Parent element tests.

// Verify that `ParentElement::FromFidl()` can convert a fuchsia_hardware_power::ParentElement value
// with type "sag" into a ParentElement value.
TEST(TypesTest, ParentElementFromNaturalWithSag) {
  fuchsia_hardware_power::ParentElement fidl = fuchsia_hardware_power::ParentElement::WithSag(
      fuchsia_hardware_power::SagElement::kApplicationActivity);
  zx::result parent_element = ParentElement::FromFidl(fidl);
  ASSERT_TRUE(parent_element.is_ok()) << parent_element.status_string();
  ASSERT_EQ(parent_element->type(), ParentElement::Type::kSag);
  ASSERT_EQ(parent_element->GetSag(), std::optional<SagElement>{SagElement::kApplicationActivity});
}

// Verify that `ParentElement::FromFidl()` can convert a fuchsia_hardware_power::wire::ParentElement
// value with type "sag" into a ParentElement value.
TEST(TypesTest, ParentElementFromWireWithSag) {
  auto fidl = fuchsia_hardware_power::wire::ParentElement::WithSag(
      fuchsia_hardware_power::wire::SagElement::kApplicationActivity);

  zx::result parent_element = ParentElement::FromFidl(fidl);
  ASSERT_TRUE(parent_element.is_ok()) << parent_element.status_string();
  ASSERT_EQ(parent_element->type(), ParentElement::Type::kSag);
  ASSERT_EQ(parent_element->GetSag(), std::optional<SagElement>{SagElement::kApplicationActivity});
}

// Verify that `ParentElement::FromFidl()` can convert a fuchsia_hardware_power::ParentElement value
// with type "instance name" into a ParentElement value.
TEST(TypesTest, ParentElementFromNaturalWithInstanceName) {
  fuchsia_hardware_power::ParentElement fidl =
      fuchsia_hardware_power::ParentElement::WithInstanceName("test parent element");
  zx::result parent_element = ParentElement::FromFidl(fidl);
  ASSERT_TRUE(parent_element.is_ok()) << parent_element.status_string();
  ASSERT_EQ(parent_element->type(), ParentElement::Type::kInstanceName);
  ASSERT_EQ(parent_element->GetInstanceName(), std::optional<std::string>{"test parent element"});
}

// Verify that `ParentElement::FromFidl()` can convert a fuchsia_hardware_power::wire::ParentElement
// value with type "instance name" into a ParentElement value.
TEST(TypesTest, ParentElementFromWireWithInstanceName) {
  fidl::Arena arena;
  auto fidl =
      fuchsia_hardware_power::wire::ParentElement::WithInstanceName(arena, "test parent element");

  zx::result parent_element = ParentElement::FromFidl(fidl);
  ASSERT_TRUE(parent_element.is_ok()) << parent_element.status_string();
  ASSERT_EQ(parent_element->type(), ParentElement::Type::kInstanceName);
  ASSERT_EQ(parent_element->GetInstanceName(), std::optional<std::string>{"test parent element"});
}

TEST(TypesTest, ParentElementFromNaturalWithCpu) {
  fuchsia_hardware_power::ParentElement fidl =
      fuchsia_hardware_power::ParentElement::WithCpuControl(
          fuchsia_hardware_power::CpuPowerElement::kCpu);

  zx::result parent_element = ParentElement::FromFidl(fidl);
  ASSERT_TRUE(parent_element.is_ok()) << parent_element.status_string();
  ASSERT_EQ(parent_element->type(), ParentElement::Type::kCpu);
  ASSERT_EQ(parent_element->GetCpu(), std::optional<CpuElement>(CpuElement::kCpu));
}

TEST(TypesTest, ParentElementFromWireWithCpu) {
  fuchsia_hardware_power::wire::ParentElement fidl =
      fuchsia_hardware_power::wire::ParentElement::WithCpuControl(
          fuchsia_hardware_power::CpuPowerElement::kCpu);

  zx::result parent_element = ParentElement::FromFidl(fidl);
  ASSERT_TRUE(parent_element.is_ok()) << parent_element.status_string();
  ASSERT_EQ(parent_element->type(), ParentElement::Type::kCpu);
  ASSERT_EQ(parent_element->GetCpu(), std::optional<CpuElement>(CpuElement::kCpu));
}

// Power dependency tests.

// Verify that `PowerDependency::FromFidl()` can convert a fuchsia_hardware_power::PowerDependency
// value into a PowerDependency value.
TEST(TypesTest, PowerDependencyFromNatural) {
  fuchsia_hardware_power::PowerDependency fidl{
      {.child = "test child",
       .parent = fuchsia_hardware_power::ParentElement::WithInstanceName("test parent element"),
       .level_deps = {{{{.child_level = 111, .parent_level = 0}},
                       {{.child_level = 222, .parent_level = 1}}}}}};
  zx::result power_dependency = PowerDependency::FromFidl(fidl);
  ASSERT_TRUE(power_dependency.is_ok()) << power_dependency.status_string();
  ASSERT_EQ(power_dependency->child, "test child");
  ASSERT_EQ(power_dependency->parent, ParentElement::WithInstanceName("test parent element"));
  ASSERT_EQ(power_dependency->level_deps.size(), 2u);

  ASSERT_EQ(power_dependency->level_deps[0].child_level, 111u);
  ASSERT_EQ(power_dependency->level_deps[0].parent_level, 0u);

  ASSERT_EQ(power_dependency->level_deps[1].child_level, 222u);
  ASSERT_EQ(power_dependency->level_deps[1].parent_level, 1u);
}

// Verify that `PowerDependency::FromFidl()` can convert a
// fuchsia_hardware_power::wire::PowerDependency value into a PowerDependency value.
TEST(TypesTest, PowerDependencyFromWire) {
  fidl::Arena arena;
  std::vector<fuchsia_hardware_power::wire::LevelTuple> level_deps{
      fuchsia_hardware_power::wire::LevelTuple::Builder(arena)
          .child_level(111)
          .parent_level(0)
          .Build(),
      fuchsia_hardware_power::wire::LevelTuple::Builder(arena)
          .child_level(222)
          .parent_level(1)
          .Build(),
  };
  auto fidl =
      fuchsia_hardware_power::wire::PowerDependency::Builder(arena)
          .child("test child")
          .parent(fuchsia_hardware_power::wire::ParentElement::WithInstanceName(
              arena, "test parent element"))
          .level_deps(
              fidl::VectorView<fuchsia_hardware_power::wire::LevelTuple>::FromExternal(level_deps))
          .Build();

  zx::result power_dependency = PowerDependency::FromFidl(fidl);
  ASSERT_TRUE(power_dependency.is_ok()) << power_dependency.status_string();
  ASSERT_EQ(power_dependency->child, "test child");
  ASSERT_EQ(power_dependency->parent, ParentElement::WithInstanceName("test parent element"));
  ASSERT_EQ(power_dependency->level_deps.size(), 2u);

  ASSERT_EQ(power_dependency->level_deps[0].child_level, 111u);
  ASSERT_EQ(power_dependency->level_deps[0].parent_level, 0u);

  ASSERT_EQ(power_dependency->level_deps[1].child_level, 222u);
  ASSERT_EQ(power_dependency->level_deps[1].parent_level, 1u);
}

// Verify `PowerDependency::FromFidl()` will fail if child is missing.
TEST(TypesTest, PowerDependencyFromNaturalMissingChild) {
  fuchsia_hardware_power::PowerDependency fidl{
      {.parent = fuchsia_hardware_power::ParentElement::WithInstanceName(""), .level_deps = {{}}}};
  zx::result power_dependency = PowerDependency::FromFidl(fidl);
  ASSERT_EQ(power_dependency.status_value(), ZX_ERR_INVALID_ARGS);
}

// Verify `PowerDependency::FromFidl()` will fail if child is missing.
TEST(TypesTest, PowerDependencyFromWireMissingChild) {
  fidl::Arena arena;
  std::vector<fuchsia_hardware_power::wire::LevelTuple> level_deps;
  auto fidl =
      fuchsia_hardware_power::wire::PowerDependency::Builder(arena)
          .parent(fuchsia_hardware_power::wire::ParentElement::WithInstanceName(
              arena, "test parent element"))
          .level_deps(
              fidl::VectorView<fuchsia_hardware_power::wire::LevelTuple>::FromExternal(level_deps))
          .Build();

  zx::result power_dependency = PowerDependency::FromFidl(fidl);
  ASSERT_EQ(power_dependency.status_value(), ZX_ERR_INVALID_ARGS);
}

// Verify `PowerDependency::FromFidl()` will fail if parent is missing.
TEST(TypesTest, PowerDependencyFromNaturalMissingParent) {
  fuchsia_hardware_power::PowerDependency fidl{{.child = "", .level_deps = {{}}}};
  zx::result power_dependency = PowerDependency::FromFidl(fidl);
  ASSERT_EQ(power_dependency.status_value(), ZX_ERR_INVALID_ARGS);
}

// Verify `PowerDependency::FromFidl()` will fail if parent is missing.
TEST(TypesTest, PowerDependencyFromWireMissingParent) {
  fidl::Arena arena;
  std::vector<fuchsia_hardware_power::wire::LevelTuple> level_deps;
  auto fidl =
      fuchsia_hardware_power::wire::PowerDependency::Builder(arena)
          .child("test child")
          .level_deps(
              fidl::VectorView<fuchsia_hardware_power::wire::LevelTuple>::FromExternal(level_deps))
          .Build();

  zx::result power_dependency = PowerDependency::FromFidl(fidl);
  ASSERT_EQ(power_dependency.status_value(), ZX_ERR_INVALID_ARGS);
}

// Verify `PowerDependency::FromFidl()` will succeed if level dependencies are missing.
TEST(TypesTest, PowerDependencyFromNaturalMissingLevelDeps) {
  fuchsia_hardware_power::PowerDependency fidl{
      {.child = "", .parent = fuchsia_hardware_power::ParentElement::WithInstanceName("")}};
  zx::result power_dependency = PowerDependency::FromFidl(fidl);
  ASSERT_TRUE(power_dependency.is_ok()) << power_dependency.status_string();
  ASSERT_TRUE(power_dependency->level_deps.empty());
}

// Verify `PowerDependency::FromFidl()` will succeed if level dependencies are missing.
TEST(TypesTest, PowerDependencyFromWireMissingLevelDeps) {
  fidl::Arena arena;
  auto fidl = fuchsia_hardware_power::wire::PowerDependency::Builder(arena)
                  .child("test child")
                  .parent(fuchsia_hardware_power::wire::ParentElement::WithInstanceName(
                      arena, "test parent element"))
                  .Build();

  zx::result power_dependency = PowerDependency::FromFidl(fidl);
  ASSERT_TRUE(power_dependency.is_ok()) << power_dependency.status_string();
  ASSERT_TRUE(power_dependency->level_deps.empty());
}

// Power element configuration tests.

// Verify that `PowerElementConfiguration::FromFidl()` can convert a
// fuchsia_hardware_power::PowerElementConfiguration value into a PowerElementConfiguration value.
TEST(TypesTest, PowerElementConfigurationFromNatural) {
  fuchsia_hardware_power::PowerElement element{{.name = "test power element", .levels = {{}}}};

  fuchsia_hardware_power::PowerDependency dependency1{
      {.child = "test child 1",
       .parent = fuchsia_hardware_power::ParentElement::WithInstanceName("test parent element 1"),
       .level_deps = {{}}}};

  fuchsia_hardware_power::PowerDependency dependency2{
      {.child = "test child 2",
       .parent = fuchsia_hardware_power::ParentElement::WithInstanceName("test parent element 2"),
       .level_deps = {{}}}};

  fuchsia_hardware_power::PowerElementConfiguration fidl{
      {.element = std::move(element),
       .dependencies = {{std::move(dependency1), std::move(dependency2)}}}};

  zx::result config = PowerElementConfiguration::FromFidl(fidl);
  ASSERT_TRUE(config.is_ok()) << config.status_string();
  ASSERT_EQ(config->element.name, "test power element");
  ASSERT_TRUE(config->element.levels.empty());
  ASSERT_EQ(config->dependencies.size(), 2u);

  ASSERT_EQ(config->dependencies[0].child, "test child 1");
  ASSERT_EQ(config->dependencies[0].parent,
            ParentElement::WithInstanceName("test parent element 1"));
  ASSERT_TRUE(config->dependencies[0].level_deps.empty());

  ASSERT_EQ(config->dependencies[1].child, "test child 2");
  ASSERT_EQ(config->dependencies[1].parent,
            ParentElement::WithInstanceName("test parent element 2"));
  ASSERT_TRUE(config->dependencies[1].level_deps.empty());
}

// Verify that `PowerElementConfiguration::FromFidl()` can convert a
// fuchsia_hardware_power::wire::PowerElementConfiguration value into a PowerElementConfiguration
// value.
TEST(TypesTest, PowerElementConfigurationFromWire) {
  fidl::Arena arena;
  auto element =
      fuchsia_hardware_power::wire::PowerElement::Builder(arena).name("test power element").Build();
  std::vector<fuchsia_hardware_power::wire::PowerDependency> dependencies{
      fuchsia_hardware_power::wire::PowerDependency::Builder(arena)
          .child("test child 1")
          .parent(fuchsia_hardware_power::wire::ParentElement::WithInstanceName(
              arena, "test parent element 1"))
          .Build(),
      fuchsia_hardware_power::wire::PowerDependency::Builder(arena)
          .child("test child 2")
          .parent(fuchsia_hardware_power::wire::ParentElement::WithInstanceName(
              arena, "test parent element 2"))
          .Build()};
  auto fidl = fuchsia_hardware_power::wire::PowerElementConfiguration::Builder(arena)
                  .element(element)
                  .dependencies(
                      fidl::VectorView<fuchsia_hardware_power::wire::PowerDependency>::FromExternal(
                          dependencies))
                  .Build();

  zx::result config = PowerElementConfiguration::FromFidl(fidl);
  ASSERT_TRUE(config.is_ok()) << config.status_string();
  ASSERT_EQ(config->element.name, "test power element");
  ASSERT_TRUE(config->element.levels.empty());
  ASSERT_EQ(config->dependencies.size(), 2u);

  ASSERT_EQ(config->dependencies[0].child, "test child 1");
  ASSERT_EQ(config->dependencies[0].parent,
            ParentElement::WithInstanceName("test parent element 1"));
  ASSERT_TRUE(config->dependencies[0].level_deps.empty());

  ASSERT_EQ(config->dependencies[1].child, "test child 2");
  ASSERT_EQ(config->dependencies[1].parent,
            ParentElement::WithInstanceName("test parent element 2"));
  ASSERT_TRUE(config->dependencies[1].level_deps.empty());
}

// Verify `PowerElementConfiguration::FromFidl()` will fail if element is missing.
TEST(TypesTest, PowerElementConfigurationFromNaturalMissingElement) {
  fuchsia_hardware_power::PowerElement element{{.name = "test power element", .levels = {{}}}};

  fuchsia_hardware_power::PowerElementConfiguration fidl{{.dependencies = {{}}}};
  zx::result config = PowerElementConfiguration::FromFidl(fidl);
  ASSERT_EQ(config.status_value(), ZX_ERR_INVALID_ARGS);
}

// Verify `PowerElementConfiguration::FromFidl()` will fail if element is missing.
TEST(TypesTest, PowerElementConfigurationFromWireMissingElement) {
  fidl::Arena arena;
  std::vector<fuchsia_hardware_power::wire::PowerDependency> dependencies;
  auto fidl = fuchsia_hardware_power::wire::PowerElementConfiguration::Builder(arena)
                  .dependencies(
                      fidl::VectorView<fuchsia_hardware_power::wire::PowerDependency>::FromExternal(
                          dependencies))
                  .Build();

  zx::result config = PowerElementConfiguration::FromFidl(fidl);
  ASSERT_EQ(config.status_value(), ZX_ERR_INVALID_ARGS);
}

// Verify `PowerElementConfiguration::FromFidl()` will succeed if dependencies are missing.
TEST(TypesTest, PowerElementConfigurationFromNaturalMissingDependencies) {
  fuchsia_hardware_power::PowerElement element{{.name = "test power element", .levels = {{}}}};

  fuchsia_hardware_power::PowerElementConfiguration fidl{{.element = std::move(element)}};
  zx::result config = PowerElementConfiguration::FromFidl(fidl);
  ASSERT_TRUE(config.is_ok()) << config.status_string();
  ASSERT_TRUE(config->dependencies.empty());
}

// Verify `PowerElementConfiguration::FromFidl()` will succeed if dependencies are missing.
TEST(TypesTest, PowerElementConfigurationFromWireMissingDependencies) {
  fidl::Arena arena;
  auto element =
      fuchsia_hardware_power::wire::PowerElement::Builder(arena).name("test power element").Build();
  auto fidl = fuchsia_hardware_power::wire::PowerElementConfiguration::Builder(arena)
                  .element(element)
                  .Build();

  zx::result config = PowerElementConfiguration::FromFidl(fidl);
  ASSERT_TRUE(config.is_ok()) << config.status_string();
  ASSERT_TRUE(config->dependencies.empty());
}

}  // namespace fdf_power::test
