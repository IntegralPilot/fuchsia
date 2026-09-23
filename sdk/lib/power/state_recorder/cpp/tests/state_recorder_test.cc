// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/inspect/component/cpp/component.h>
#include <lib/inspect/testing/cpp/inspect.h>
#include <lib/power/state_recorder/cpp/enum_state_recorder.h>
#include <lib/power/state_recorder/cpp/numeric_state_recorder.h>

#include <memory>

#include <gtest/gtest.h>

#include "gmock/gmock.h"
#include "src/lib/testing/loop_fixture/real_loop_fixture.h"
#include "zircon/errors.h"

using ::inspect::testing::ChildrenMatch;
using ::inspect::testing::DoubleArrayIs;
using ::inspect::testing::DoubleIs;
using ::inspect::testing::IntArrayIs;
using ::inspect::testing::IntIs;
using ::inspect::testing::NameMatches;
using ::inspect::testing::NodeMatches;
using ::inspect::testing::PropertyList;
using ::inspect::testing::StringIs;
using ::inspect::testing::UintArrayIs;
using ::inspect::testing::UintIs;
using ::testing::AllOf;
using ::testing::ElementsAre;
using ::testing::UnorderedElementsAre;

namespace power_observability {

// Extracts timestamps from a "history" hierarchy in sharded format.
std::vector<int64_t> GetTimestamps(const inspect::Hierarchy* history_hierarchy, size_t count) {
  std::vector<int64_t> timestamps;
  for (size_t i = 0; i < count; i++) {
    size_t shard_idx = i / internal::kShardCapacity;
    size_t slot_idx = i % internal::kShardCapacity;
    auto h = history_hierarchy->GetByPath({"shards", std::to_string(shard_idx)});
    EXPECT_NE(h, nullptr);
    auto property = h->node().get_property<inspect::IntArrayValue>("times");
    EXPECT_NE(property, nullptr);
    timestamps.push_back(property->value()[slot_idx]);
  }
  return timestamps;
}

// Returns the current timestamp in nanoseconds, clamped to millisecond resolution if `lazy_record`
// is true.
int64_t GetCurrentTimestamp(bool lazy_record) {
  auto timestamp = zx::clock::get_boot();
  if (lazy_record) {
    auto msecs = internal::to_msecs(timestamp);
    return zx::msec(msecs).get();
  } else {
    return timestamp.get();
  }
}

class StateRecorderTest : public gtest::RealLoopFixture {
 protected:
  void SetUp() override {
    gtest::RealLoopFixture::SetUp();
    inspector_ =
        std::make_unique<inspect::ComponentInspector>(dispatcher(), inspect::PublishOptions{});
    manager_ = std::make_unique<StateRecorderManager>(*inspector_);
  }

  inspect::Hierarchy GetHierarchy() {
    fpromise::result<inspect::Hierarchy> result =
        RunPromise(inspect::ReadFromInspector(inspector_->inspector()));
    EXPECT_TRUE(result.is_ok());
    return std::move(result.value());
  }

  std::unique_ptr<inspect::ComponentInspector> inspector_;
  std::unique_ptr<StateRecorderManager> manager_;
};

enum class SwitchState : uint8_t {
  kOff = 0,
  kOn = 1,
};

static const std::map<SwitchState, std::string> kOffOn = {
    {SwitchState::kOff, "OFF"},
    {SwitchState::kOn, "ON"},
};

class OffOnTest : public StateRecorderTest, public ::testing::WithParamInterface<bool> {};

TEST_P(OffOnTest, OffOn) {
  bool lazy_record = GetParam();
  EnumStateMetadata metadata = {
      .name = "my_switch",
      .states = kOffOn,
      .trace_category_literal = "power_test",
  };

  auto result = EnumStateRecorder<SwitchState>::Create(
      metadata, {.capacity = 10, .lazy_record = lazy_record}, *manager_);
  ASSERT_TRUE(result.is_ok());
  EnumStateRecorder recorder(std::move(result.value()));

  auto start_ns = GetCurrentTimestamp(lazy_record);
  recorder.Record(SwitchState::kOff);
  recorder.Record(SwitchState::kOn);
  recorder.Record(SwitchState::kOff);
  recorder.Record(SwitchState::kOn);
  auto end_ns = GetCurrentTimestamp(lazy_record);

  auto hierarchy = GetHierarchy();
  EXPECT_THAT(hierarchy, AllOf(NodeMatches(NameMatches("root")),
                               ChildrenMatch(ElementsAre(NodeMatches(
                                   AllOf(NameMatches("power_observability_state_recorders")))))));

  auto recorders_root_hierarchy = hierarchy.GetByPath({"power_observability_state_recorders"});
  ASSERT_NE(recorders_root_hierarchy, nullptr);
  EXPECT_THAT(*recorders_root_hierarchy,
              ChildrenMatch(ElementsAre(NodeMatches(NameMatches("my_switch")))));

  auto recorder_hierarchy = recorders_root_hierarchy->GetByPath({"my_switch"});
  ASSERT_NE(recorder_hierarchy, nullptr);
  EXPECT_THAT(*recorder_hierarchy,
              AllOf(NodeMatches(NameMatches("my_switch")),
                    ChildrenMatch(UnorderedElementsAre(NodeMatches(NameMatches("metadata")),
                                                       NodeMatches(NameMatches("history")),
                                                       NodeMatches(NameMatches("reset_info"))))));

  auto metadata_hierarchy = recorder_hierarchy->GetByPath({"metadata"});
  ASSERT_NE(metadata_hierarchy, nullptr);
  EXPECT_THAT(*metadata_hierarchy,
              AllOf(NodeMatches(PropertyList(UnorderedElementsAre(StringIs("format_version", "2.0"),
                                                                  StringIs("name", "my_switch"),
                                                                  StringIs("type", "enum")))),
                    ChildrenMatch(UnorderedElementsAre(NodeMatches(NameMatches("states"))))));
  auto states_hierarchy = metadata_hierarchy->GetByPath({"states"});
  ASSERT_NE(states_hierarchy, nullptr);
  EXPECT_THAT(*states_hierarchy,
              NodeMatches(PropertyList(UnorderedElementsAre(UintIs("OFF", 0), UintIs("ON", 1)))));

  auto history = recorder_hierarchy->GetByPath({"history"});
  ASSERT_NE(history, nullptr);

  EXPECT_THAT(*history,
              AllOf(NodeMatches(PropertyList(UnorderedElementsAre(
                        UintIs("current_index", lazy_record ? 0 : 4), UintIs("current_size", 4)))),
                    ChildrenMatch(ElementsAre(NodeMatches(NameMatches("shards"))))));

  auto shard_0 = history->GetByPath({"shards", "0"});
  ASSERT_NE(shard_0, nullptr);
  size_t expected_shard_size = lazy_record ? 4 : 10;
  std::vector<uint64_t> expected_values = {0, 1, 0, 1};
  if (!lazy_record) {
    expected_values.resize(10, 0);
  }
  EXPECT_THAT(*shard_0, NodeMatches(PropertyList(UnorderedElementsAre(
                            IntArrayIs("times", testing::SizeIs(expected_shard_size)),
                            UintArrayIs("values", expected_values)))));

  // Make sure timestamps are non-decreasing.
  auto timestamps = GetTimestamps(history, 4);
  ASSERT_LE(start_ns, timestamps[0]);
  ASSERT_LE(timestamps[0], timestamps[1]);
  ASSERT_LE(timestamps[1], timestamps[2]);
  ASSERT_LE(timestamps[2], timestamps[3]);
  ASSERT_LE(timestamps[3], end_ns);
}

INSTANTIATE_TEST_SUITE_P(OffOnTest, OffOnTest, ::testing::Bool());

TEST_F(StateRecorderTest, ResetCount) {
  EnumStateMetadata metadata = {
      .name = "my_switch",
      .states = kOffOn,
      .trace_category_literal = "power_test",
  };

  auto result = EnumStateRecorder<SwitchState>::Create(
      metadata, {.capacity = 10, .lazy_record = true}, *manager_);
  ASSERT_TRUE(result.is_ok());
  EnumStateRecorder recorder(std::move(result.value()));

  const zx::time_boot kTime0(0);
  const zx::duration kOverflowDelta =
      zx::msec(static_cast<int64_t>(std::numeric_limits<int32_t>::max()) + 500);
  const zx::time_boot kTime1 = kTime0 + kOverflowDelta;
  const zx::time_boot kTime2 = kTime1 + kOverflowDelta;

  recorder.Record(SwitchState::kOff, kTime0);
  recorder.Record(SwitchState::kOn, kTime1);
  recorder.Record(SwitchState::kOff, kTime2);

  // Each new entry reset the buffer, so we expect only the last entry to be in history, a reset
  // count of 2, and the last reset timestamp equal to the timestamp of the last entry.
  auto hierarchy = GetHierarchy();
  auto recorder_hierarchy =
      hierarchy.GetByPath({"power_observability_state_recorders", "my_switch"});
  ASSERT_NE(recorder_hierarchy, nullptr);

  auto history = recorder_hierarchy->GetByPath({"history"});
  ASSERT_NE(history, nullptr);

  EXPECT_THAT(*history, AllOf(NodeMatches(PropertyList(UnorderedElementsAre(
                                  UintIs("current_index", 0), UintIs("current_size", 1)))),
                              ChildrenMatch(ElementsAre(NodeMatches(NameMatches("shards"))))));

  auto shard_0 = history->GetByPath({"shards", "0"});
  ASSERT_NE(shard_0, nullptr);
  EXPECT_THAT(*shard_0, NodeMatches(PropertyList(UnorderedElementsAre(
                            IntArrayIs("times", std::vector<int64_t>{kTime2.get()}),
                            UintArrayIs("values", std::vector<uint64_t>{0})))));

  auto reset_info =
      hierarchy.GetByPath({"power_observability_state_recorders", "my_switch", "reset_info"});
  ASSERT_NE(reset_info, nullptr);
  EXPECT_THAT(*reset_info, NodeMatches(PropertyList(UnorderedElementsAre(
                               UintIs("count", 2), IntIs("last_reset_ns", kTime2.get())))));
}

TEST_F(StateRecorderTest, CantReuseName) {
  EnumStateMetadata metadata = {
      .name = "my_switch",
      .states = kOffOn,
      .trace_category_literal = "power_test",
  };

  {
    auto result_1 = EnumStateRecorder<SwitchState>::Create(
        metadata, {.capacity = 10, .lazy_record = false}, *manager_);
    ASSERT_TRUE(result_1.is_ok());

    // Reusing a name results in an error.
    auto result_2 = EnumStateRecorder<SwitchState>::Create(
        metadata, {.capacity = 10, .lazy_record = false}, *manager_);
    ASSERT_TRUE(result_2.is_error());
    ASSERT_EQ(result_2.error_value(), ZX_ERR_ALREADY_EXISTS);
  }

  // After result_1 is dropped, the name is available for use again.
  auto result_3 = EnumStateRecorder<SwitchState>::Create(
      metadata, {.capacity = 10, .lazy_record = false}, *manager_);
  ASSERT_TRUE(result_3.is_ok());
}

TEST_F(StateRecorderTest, MultipleRecorders) {
  enum class EnablementState : uint8_t { kDisabled = 0, kEnabled = 1 };

  const std::map<EnablementState, std::string> kDisabledEnabled = {
      {EnablementState::kDisabled, "DISABLED"},
      {EnablementState::kEnabled, "ENABLED"},
  };

  EnumStateMetadata<SwitchState> metadata_0 = {
      .name = "switch_0",
      .states = kOffOn,
      .trace_category_literal = "power_test",
  };
  EnumStateMetadata<EnablementState> metadata_1 = {
      .name = "switch_1",
      .states = kDisabledEnabled,
      .trace_category_literal = "power_test",
  };

  auto result_0 = EnumStateRecorder<SwitchState>::Create(
      metadata_0, {.capacity = 10, .lazy_record = false}, *manager_);
  ASSERT_TRUE(result_0.is_ok());
  EnumStateRecorder recorder_0 = std::move(result_0.value());

  auto result_1 = EnumStateRecorder<EnablementState>::Create(
      metadata_1, {.capacity = 10, .lazy_record = false}, *manager_);
  ASSERT_TRUE(result_1.is_ok());
  EnumStateRecorder recorder_1 = std::move(result_1.value());

  recorder_0.Record(SwitchState::kOff);
  recorder_0.Record(SwitchState::kOn);
  recorder_1.Record(EnablementState::kEnabled);
  recorder_1.Record(EnablementState::kDisabled);

  auto hierarchy = GetHierarchy();
  EXPECT_THAT(hierarchy, AllOf(NodeMatches(NameMatches("root")),
                               ChildrenMatch(ElementsAre(NodeMatches(
                                   AllOf(NameMatches("power_observability_state_recorders")))))));

  auto recorders_root_hierarchy = hierarchy.GetByPath({"power_observability_state_recorders"});
  ASSERT_NE(recorders_root_hierarchy, nullptr);
  EXPECT_THAT(*recorders_root_hierarchy,
              ChildrenMatch(ElementsAre(NodeMatches(NameMatches("switch_0")),
                                        NodeMatches(NameMatches("switch_1")))));

  auto history_0 = recorders_root_hierarchy->GetByPath({"switch_0", "history"});
  ASSERT_NE(history_0, nullptr);
  EXPECT_THAT(*history_0, AllOf(NodeMatches(PropertyList(UnorderedElementsAre(
                                    UintIs("current_index", 2), UintIs("current_size", 2)))),
                                ChildrenMatch(ElementsAre(NodeMatches(NameMatches("shards"))))));
  auto shard_0_0 = history_0->GetByPath({"shards", "0"});
  ASSERT_NE(shard_0_0, nullptr);
  std::vector<uint64_t> expected_values_0 = {0, 1};
  expected_values_0.resize(10, 0);
  EXPECT_THAT(
      *shard_0_0,
      NodeMatches(PropertyList(UnorderedElementsAre(IntArrayIs("times", testing::SizeIs(10)),
                                                    UintArrayIs("values", expected_values_0)))));

  auto history_1 = recorders_root_hierarchy->GetByPath({"switch_1", "history"});
  ASSERT_NE(history_1, nullptr);
  EXPECT_THAT(*history_1, AllOf(NodeMatches(PropertyList(UnorderedElementsAre(
                                    UintIs("current_index", 2), UintIs("current_size", 2)))),
                                ChildrenMatch(ElementsAre(NodeMatches(NameMatches("shards"))))));
  auto shard_1_0 = history_1->GetByPath({"shards", "0"});
  ASSERT_NE(shard_1_0, nullptr);
  std::vector<uint64_t> expected_values_1 = {1, 0};
  expected_values_1.resize(10, 0);
  EXPECT_THAT(
      *shard_1_0,
      NodeMatches(PropertyList(UnorderedElementsAre(IntArrayIs("times", testing::SizeIs(10)),
                                                    UintArrayIs("values", expected_values_1)))));
}

class ThreeStatesTest : public StateRecorderTest, public ::testing::WithParamInterface<bool> {};

TEST_P(ThreeStatesTest, ThreeStates) {
  bool lazy_record = GetParam();
  enum class FanSpeed : uint8_t { kOff = 0, kLow = 1, kHigh = 2 };

  const std::map<FanSpeed, std::string> kFanSpeeds = {
      {FanSpeed::kOff, "OFF"},
      {FanSpeed::kLow, "LOW_SPEED"},
      {FanSpeed::kHigh, "HIGH_SPEED"},
  };
  EnumStateMetadata metadata = {
      .name = "my_fan",
      .states = kFanSpeeds,
      .trace_category_literal = "power_test",
  };

  auto result = EnumStateRecorder<FanSpeed>::Create(
      metadata, {.capacity = 10, .lazy_record = lazy_record}, *manager_);
  ASSERT_TRUE(result.is_ok());
  EnumStateRecorder recorder(std::move(result.value()));

  auto start_ns = GetCurrentTimestamp(lazy_record);
  recorder.Record(FanSpeed::kOff);
  recorder.Record(FanSpeed::kHigh);
  recorder.Record(FanSpeed::kLow);
  auto end_ns = GetCurrentTimestamp(lazy_record);

  auto hierarchy = GetHierarchy();
  EXPECT_THAT(hierarchy, AllOf(NodeMatches(NameMatches("root")),
                               ChildrenMatch(ElementsAre(NodeMatches(
                                   AllOf(NameMatches("power_observability_state_recorders")))))));

  auto recorders_root_hierarchy = hierarchy.GetByPath({"power_observability_state_recorders"});
  ASSERT_NE(recorders_root_hierarchy, nullptr);
  EXPECT_THAT(*recorders_root_hierarchy,
              ChildrenMatch(ElementsAre(NodeMatches(NameMatches("my_fan")))));

  auto recorder_hierarchy = recorders_root_hierarchy->GetByPath({"my_fan"});
  ASSERT_NE(recorder_hierarchy, nullptr);
  EXPECT_THAT(*recorder_hierarchy,
              AllOf(NodeMatches(NameMatches("my_fan")),
                    ChildrenMatch(UnorderedElementsAre(NodeMatches(NameMatches("metadata")),
                                                       NodeMatches(NameMatches("history")),
                                                       NodeMatches(NameMatches("reset_info"))))));

  auto history = recorder_hierarchy->GetByPath({"history"});
  ASSERT_NE(history, nullptr);

  EXPECT_THAT(*history,
              AllOf(NodeMatches(PropertyList(UnorderedElementsAre(
                        UintIs("current_index", lazy_record ? 0 : 3), UintIs("current_size", 3)))),
                    ChildrenMatch(ElementsAre(NodeMatches(NameMatches("shards"))))));

  auto shard_0 = history->GetByPath({"shards", "0"});
  ASSERT_NE(shard_0, nullptr);
  size_t expected_shard_size = lazy_record ? 3 : 10;
  std::vector<uint64_t> expected_values = {0, 2, 1};
  if (!lazy_record) {
    expected_values.resize(10, 0);
  }
  EXPECT_THAT(*shard_0, NodeMatches(PropertyList(UnorderedElementsAre(
                            IntArrayIs("times", testing::SizeIs(expected_shard_size)),
                            UintArrayIs("values", expected_values)))));

  // Make sure timestamps are non-decreasing.
  auto timestamps = GetTimestamps(history, 3);
  ASSERT_LE(start_ns, timestamps[0]);
  ASSERT_LE(timestamps[0], timestamps[1]);
  ASSERT_LE(timestamps[1], timestamps[2]);
  ASSERT_LE(timestamps[2], end_ns);
}

INSTANTIATE_TEST_SUITE_P(ThreeStatesTest, ThreeStatesTest, ::testing::Bool());

class NumericStateRecorderSignedTest : public StateRecorderTest,
                                       public ::testing::WithParamInterface<bool> {};

TEST_P(NumericStateRecorderSignedTest, SignedInt) {
  bool lazy_record = GetParam();

  auto test_body = [&](auto type_val) {
    using T = decltype(type_val);
    std::string name = "my_numeric_int_state" + std::to_string(sizeof(T));
    NumericStateMetadata<T> metadata = {
        .name = name,
        .units = Units::Number(),
        .trace_category_literal = "power_test",
    };

    auto result = NumericStateRecorder<T>::Create(
        metadata, {.capacity = 10, .lazy_record = lazy_record}, *this->manager_);
    ASSERT_TRUE(result.is_ok());
    auto recorder = std::move(result.value());

    auto start_ns = GetCurrentTimestamp(lazy_record);
    recorder.Record(0);
    recorder.Record(1);
    recorder.Record(-1);
    auto end_ns = GetCurrentTimestamp(lazy_record);

    auto hierarchy = this->GetHierarchy();
    auto* recorders_root = hierarchy.GetByPath({"power_observability_state_recorders"});
    ASSERT_NE(recorders_root, nullptr);
    auto* recorder_node = recorders_root->GetByPath({name});
    ASSERT_NE(recorder_node, nullptr);

    EXPECT_THAT(*recorder_node,
                AllOf(NodeMatches(NameMatches(name)),
                      ChildrenMatch(UnorderedElementsAre(NodeMatches(NameMatches("metadata")),
                                                         NodeMatches(NameMatches("history")),
                                                         NodeMatches(NameMatches("reset_info"))))));

    auto* metadata_node = recorder_node->GetByPath({"metadata"});
    ASSERT_NE(metadata_node, nullptr);
    EXPECT_THAT(*metadata_node, NodeMatches(PropertyList(UnorderedElementsAre(
                                    StringIs("format_version", "2.0"), StringIs("name", name),
                                    StringIs("type", "numeric"), StringIs("units", "#")))));

    auto* history_node = recorder_node->GetByPath({"history"});
    ASSERT_NE(history_node, nullptr);
    EXPECT_THAT(
        *history_node,
        AllOf(NodeMatches(PropertyList(UnorderedElementsAre(
                  UintIs("current_index", lazy_record ? 0 : 3), UintIs("current_size", 3)))),
              ChildrenMatch(ElementsAre(NodeMatches(NameMatches("shards"))))));

    auto* shard_0 = history_node->GetByPath({"shards", "0"});
    ASSERT_NE(shard_0, nullptr);
    size_t expected_shard_size = lazy_record ? 3 : 10;
    std::vector<int64_t> expected_values = {0, 1, -1};
    if (!lazy_record) {
      expected_values.resize(10, 0);
    }
    EXPECT_THAT(*shard_0, NodeMatches(PropertyList(UnorderedElementsAre(
                              IntArrayIs("times", testing::SizeIs(expected_shard_size)),
                              IntArrayIs("values", expected_values)))));

    // Make sure timestamps are non-decreasing.
    auto timestamps = GetTimestamps(history_node, 3);
    ASSERT_LE(start_ns, timestamps[0]);
    ASSERT_LE(timestamps[0], timestamps[1]);
    ASSERT_LE(timestamps[1], timestamps[2]);
    ASSERT_LE(timestamps[2], end_ns);
  };

  test_body(int8_t{});
  test_body(int16_t{});
  test_body(int32_t{});
  test_body(int64_t{});
}

INSTANTIATE_TEST_SUITE_P(NumericStateRecorderSignedTest, NumericStateRecorderSignedTest,
                         ::testing::Bool());

class NumericStateRecorderUnsignedTest : public StateRecorderTest,
                                         public ::testing::WithParamInterface<bool> {};

TEST_P(NumericStateRecorderUnsignedTest, UnsignedInt) {
  bool lazy_record = GetParam();

  auto test_body = [&](auto type_val) {
    using T = decltype(type_val);
    std::string name = "my_numeric_uint_state" + std::to_string(sizeof(T));
    NumericStateMetadata<T> metadata = {
        .name = name,
        .units = Units::Percent(),
        .range = {{0, 100}},
        .trace_category_literal = "power_test",
    };

    auto result = NumericStateRecorder<T>::Create(
        metadata, {.capacity = 10, .lazy_record = lazy_record}, *this->manager_);
    ASSERT_TRUE(result.is_ok());
    auto recorder = std::move(result.value());

    auto start_ns = GetCurrentTimestamp(lazy_record);
    recorder.Record(50);
    recorder.Record(100);
    auto end_ns = GetCurrentTimestamp(lazy_record);

    auto hierarchy = this->GetHierarchy();
    auto* recorders_root = hierarchy.GetByPath({"power_observability_state_recorders"});
    ASSERT_NE(recorders_root, nullptr);
    auto* recorder_node = recorders_root->GetByPath({name});
    ASSERT_NE(recorder_node, nullptr);

    auto* metadata_node = recorder_node->GetByPath({"metadata"});
    ASSERT_NE(metadata_node, nullptr);
    EXPECT_THAT(*metadata_node, NodeMatches(PropertyList(UnorderedElementsAre(
                                    StringIs("format_version", "2.0"), StringIs("name", name),
                                    StringIs("type", "numeric"), StringIs("units", "%")))));

    auto* range_node = metadata_node->GetByPath({"range"});
    ASSERT_NE(range_node, nullptr);
    EXPECT_THAT(*range_node, NodeMatches(PropertyList(UnorderedElementsAre(
                                 UintIs("min_inc", 0), UintIs("max_inc", 100)))));

    auto* history_node = recorder_node->GetByPath({"history"});
    ASSERT_NE(history_node, nullptr);
    EXPECT_THAT(
        *history_node,
        AllOf(NodeMatches(PropertyList(UnorderedElementsAre(
                  UintIs("current_index", lazy_record ? 0 : 2), UintIs("current_size", 2)))),
              ChildrenMatch(ElementsAre(NodeMatches(NameMatches("shards"))))));

    auto* shard_0 = history_node->GetByPath({"shards", "0"});
    ASSERT_NE(shard_0, nullptr);
    size_t expected_shard_size = lazy_record ? 2 : 10;
    std::vector<uint64_t> expected_values = {50, 100};
    if (!lazy_record) {
      expected_values.resize(10, 0);
    }
    EXPECT_THAT(*shard_0, NodeMatches(PropertyList(UnorderedElementsAre(
                              IntArrayIs("times", testing::SizeIs(expected_shard_size)),
                              UintArrayIs("values", expected_values)))));

    // Make sure timestamps are non-decreasing.
    auto timestamps = GetTimestamps(history_node, 2);
    ASSERT_LE(start_ns, timestamps[0]);
    ASSERT_LE(timestamps[0], timestamps[1]);
    ASSERT_LE(timestamps[1], end_ns);
  };

  test_body(uint8_t{});
  test_body(uint16_t{});
  test_body(uint32_t{});
  test_body(uint64_t{});
}

INSTANTIATE_TEST_SUITE_P(NumericStateRecorderUnsignedTest, NumericStateRecorderUnsignedTest,
                         ::testing::Bool());

class NumericStateRecorderFloatTest : public StateRecorderTest,
                                      public ::testing::WithParamInterface<bool> {};

TEST_P(NumericStateRecorderFloatTest, FloatingPoint) {
  bool lazy_record = GetParam();

  auto test_body = [&](auto type_val) {
    using T = decltype(type_val);
    std::string name = "my_numeric_float_state" + std::to_string(sizeof(T));
    NumericStateMetadata<T> metadata = {
        .name = name,
        .units = Units::Hertz(DecimalPrefix::Kilo),
        .trace_category_literal = "power_test",
    };

    auto result = NumericStateRecorder<T>::Create(
        metadata, {.capacity = 10, .lazy_record = lazy_record}, *this->manager_);
    ASSERT_TRUE(result.is_ok());
    auto recorder = std::move(result.value());

    auto start_ns = GetCurrentTimestamp(lazy_record);
    recorder.Record(25.5);
    recorder.Record(26.0);
    auto end_ns = GetCurrentTimestamp(lazy_record);

    auto hierarchy = this->GetHierarchy();
    auto* recorders_root = hierarchy.GetByPath({"power_observability_state_recorders"});
    ASSERT_NE(recorders_root, nullptr);

    auto* recorder_node = recorders_root->GetByPath({name});
    ASSERT_NE(recorder_node, nullptr);

    auto* metadata_node = recorder_node->GetByPath({"metadata"});
    ASSERT_NE(metadata_node, nullptr);
    EXPECT_THAT(*metadata_node, NodeMatches(PropertyList(UnorderedElementsAre(
                                    StringIs("format_version", "2.0"), StringIs("name", name),
                                    StringIs("type", "numeric"), StringIs("units", "kHz")))));

    auto* history_node = recorder_node->GetByPath({"history"});
    ASSERT_NE(history_node, nullptr);
    EXPECT_THAT(
        *history_node,
        AllOf(NodeMatches(PropertyList(UnorderedElementsAre(
                  UintIs("current_index", lazy_record ? 0 : 2), UintIs("current_size", 2)))),
              ChildrenMatch(ElementsAre(NodeMatches(NameMatches("shards"))))));

    auto* shard_0 = history_node->GetByPath({"shards", "0"});
    ASSERT_NE(shard_0, nullptr);
    size_t expected_shard_size = lazy_record ? 2 : 10;
    std::vector<double> expected_values = {25.5, 26.0};
    if (!lazy_record) {
      expected_values.resize(10, 0.0);
    }
    EXPECT_THAT(*shard_0, NodeMatches(PropertyList(UnorderedElementsAre(
                              IntArrayIs("times", testing::SizeIs(expected_shard_size)),
                              DoubleArrayIs("values", expected_values)))));

    // Make sure timestamps are non-decreasing.
    auto timestamps = GetTimestamps(history_node, 2);
    ASSERT_LE(start_ns, timestamps[0]);
    ASSERT_LE(timestamps[0], timestamps[1]);
    ASSERT_LE(timestamps[1], end_ns);
  };

  test_body(float{});
  test_body(double{});
}

INSTANTIATE_TEST_SUITE_P(NumericStateRecorderFloatTest, NumericStateRecorderFloatTest,
                         ::testing::Bool());

class MultiShardTest : public StateRecorderTest, public ::testing::WithParamInterface<bool> {};

TEST_P(MultiShardTest, ExceedsSingleShardCapacity) {
  bool lazy_record = GetParam();
  EnumStateMetadata metadata = {
      .name = "multi_shard_switch",
      .states = kOffOn,
      .trace_category_literal = "power_test",
  };

  const size_t capacity = internal::kShardCapacity + 50;  // 250
  auto result = EnumStateRecorder<SwitchState>::Create(
      metadata, {.capacity = capacity, .lazy_record = lazy_record}, *manager_);
  ASSERT_TRUE(result.is_ok());
  EnumStateRecorder recorder(std::move(result.value()));

  // Record 220 items to cross kShardCapacity (200) into shard 1.
  for (size_t i = 0; i < 220; ++i) {
    recorder.Record(i % 2 == 0 ? SwitchState::kOff : SwitchState::kOn);
  }

  auto hierarchy = GetHierarchy();
  auto* recorder_hierarchy =
      hierarchy.GetByPath({"power_observability_state_recorders", "multi_shard_switch"});
  ASSERT_NE(recorder_hierarchy, nullptr);

  auto* history = recorder_hierarchy->GetByPath({"history"});
  ASSERT_NE(history, nullptr);

  EXPECT_THAT(
      *history,
      AllOf(NodeMatches(PropertyList(UnorderedElementsAre(
                UintIs("current_index", lazy_record ? 0 : 220), UintIs("current_size", 220)))),
            ChildrenMatch(ElementsAre(NodeMatches(NameMatches("shards"))))));

  auto* shard_0 = history->GetByPath({"shards", "0"});
  ASSERT_NE(shard_0, nullptr);
  std::vector<uint64_t> expected_shard_0(internal::kShardCapacity);
  for (size_t i = 0; i < internal::kShardCapacity; ++i) {
    expected_shard_0[i] = (i % 2 == 0) ? 0 : 1;
  }
  EXPECT_THAT(*shard_0, NodeMatches(PropertyList(UnorderedElementsAre(
                            IntArrayIs("times", testing::SizeIs(internal::kShardCapacity)),
                            UintArrayIs("values", expected_shard_0)))));

  auto* shard_1 = history->GetByPath({"shards", "1"});
  ASSERT_NE(shard_1, nullptr);
  size_t expected_shard_1_size = lazy_record ? 20 : 50;
  std::vector<uint64_t> expected_shard_1(expected_shard_1_size, 0);
  for (size_t i = 0; i < 20; ++i) {
    size_t global_idx = internal::kShardCapacity + i;
    expected_shard_1[i] = (global_idx % 2 == 0) ? 0 : 1;
  }
  EXPECT_THAT(*shard_1, NodeMatches(PropertyList(UnorderedElementsAre(
                            IntArrayIs("times", testing::SizeIs(expected_shard_1_size)),
                            UintArrayIs("values", expected_shard_1)))));
}

TEST_P(MultiShardTest, ExactShardCapacityBoundary) {
  bool lazy_record = GetParam();
  NumericStateMetadata<int64_t> metadata = {
      .name = "exact_boundary",
      .units = Units::Number(),
      .trace_category_literal = "power_test",
  };

  const size_t capacity = internal::kShardCapacity;  // 200
  auto result = NumericStateRecorder<int64_t>::Create(
      metadata, {.capacity = capacity, .lazy_record = lazy_record}, *manager_);
  ASSERT_TRUE(result.is_ok());
  auto recorder = std::move(result.value());

  for (size_t i = 0; i < capacity; ++i) {
    recorder.Record(static_cast<int64_t>(i));
  }

  auto hierarchy = GetHierarchy();
  auto* recorder_hierarchy =
      hierarchy.GetByPath({"power_observability_state_recorders", "exact_boundary"});
  ASSERT_NE(recorder_hierarchy, nullptr);

  auto* history = recorder_hierarchy->GetByPath({"history"});
  ASSERT_NE(history, nullptr);

  EXPECT_THAT(*history, AllOf(NodeMatches(PropertyList(UnorderedElementsAre(
                                  UintIs("current_index", 0), UintIs("current_size", capacity)))),
                              ChildrenMatch(ElementsAre(NodeMatches(NameMatches("shards"))))));

  auto* shards = history->GetByPath({"shards"});
  ASSERT_NE(shards, nullptr);
  // Verify there is exactly 1 shard: "0" (no empty shard "1" created).
  EXPECT_THAT(*shards, ChildrenMatch(ElementsAre(NodeMatches(NameMatches("0")))));

  auto* shard_0 = shards->GetByPath({"0"});
  ASSERT_NE(shard_0, nullptr);
  std::vector<int64_t> expected_values(capacity);
  for (size_t i = 0; i < capacity; ++i) {
    expected_values[i] = static_cast<int64_t>(i);
  }
  EXPECT_THAT(
      *shard_0,
      NodeMatches(PropertyList(UnorderedElementsAre(IntArrayIs("times", testing::SizeIs(capacity)),
                                                    IntArrayIs("values", expected_values)))));
}

INSTANTIATE_TEST_SUITE_P(MultiShardTest, MultiShardTest, ::testing::Bool());

class CircularBufferWrapAroundTest : public StateRecorderTest,
                                     public ::testing::WithParamInterface<bool> {};

TEST_P(CircularBufferWrapAroundTest, SingleShardWrapAround) {
  bool lazy_record = GetParam();
  NumericStateMetadata<int64_t> metadata = {
      .name = "single_shard_wrap_around",
      .units = Units::Number(),
      .trace_category_literal = "power_test",
  };

  const size_t capacity = 4;
  auto result = NumericStateRecorder<int64_t>::Create(
      metadata, {.capacity = capacity, .lazy_record = lazy_record}, *manager_);
  ASSERT_TRUE(result.is_ok());
  auto recorder = std::move(result.value());

  // Record 6 items: 10, 20, 30, 40, 50, 60 (exceeds capacity of 4)
  for (int64_t val : {10, 20, 30, 40, 50, 60}) {
    recorder.Record(val);
  }

  auto hierarchy = GetHierarchy();
  auto* recorder_hierarchy =
      hierarchy.GetByPath({"power_observability_state_recorders", "single_shard_wrap_around"});
  ASSERT_NE(recorder_hierarchy, nullptr);

  auto* history = recorder_hierarchy->GetByPath({"history"});
  ASSERT_NE(history, nullptr);

  // Size should saturate at capacity (4).
  // Eager index wraps: 6 % 4 = 2. Lazy index is always 0 (linearized).
  EXPECT_THAT(*history,
              AllOf(NodeMatches(PropertyList(UnorderedElementsAre(
                        UintIs("current_index", lazy_record ? 0 : 2), UintIs("current_size", 4)))),
                    ChildrenMatch(ElementsAre(NodeMatches(NameMatches("shards"))))));

  auto* shard_0 = history->GetByPath({"shards", "0"});
  ASSERT_NE(shard_0, nullptr);

  if (lazy_record) {
    // Lazy mode linearizes the 4 most recent items: [30, 40, 50, 60]
    EXPECT_THAT(*shard_0, NodeMatches(PropertyList(UnorderedElementsAre(
                              IntArrayIs("times", testing::SizeIs(4)),
                              IntArrayIs("values", std::vector<int64_t>{30, 40, 50, 60})))));
  } else {
    // Eager mode overwrote slots 0 and 1 with 50 and 60: [50, 60, 30, 40]
    EXPECT_THAT(*shard_0, NodeMatches(PropertyList(UnorderedElementsAre(
                              IntArrayIs("times", testing::SizeIs(4)),
                              IntArrayIs("values", std::vector<int64_t>{50, 60, 30, 40})))));
  }
}

TEST_P(CircularBufferWrapAroundTest, MultiShardWrapAround) {
  bool lazy_record = GetParam();
  NumericStateMetadata<int64_t> metadata = {
      .name = "multi_shard_wrap_around",
      .units = Units::Number(),
      .trace_category_literal = "power_test",
  };

  const size_t capacity = internal::kShardCapacity + 10;  // 210
  auto result = NumericStateRecorder<int64_t>::Create(
      metadata, {.capacity = capacity, .lazy_record = lazy_record}, *manager_);
  ASSERT_TRUE(result.is_ok());
  auto recorder = std::move(result.value());

  // Record 215 items (0..214) to exceed capacity (210) by 5 items.
  for (size_t i = 0; i < 215; ++i) {
    recorder.Record(static_cast<int64_t>(i));
  }

  auto hierarchy = GetHierarchy();
  auto* recorder_hierarchy =
      hierarchy.GetByPath({"power_observability_state_recorders", "multi_shard_wrap_around"});
  ASSERT_NE(recorder_hierarchy, nullptr);

  auto* history = recorder_hierarchy->GetByPath({"history"});
  ASSERT_NE(history, nullptr);

  EXPECT_THAT(
      *history,
      AllOf(NodeMatches(PropertyList(UnorderedElementsAre(
                UintIs("current_index", lazy_record ? 0 : 5), UintIs("current_size", capacity)))),
            ChildrenMatch(ElementsAre(NodeMatches(NameMatches("shards"))))));

  auto* shard_0 = history->GetByPath({"shards", "0"});
  ASSERT_NE(shard_0, nullptr);
  auto* shard_1 = history->GetByPath({"shards", "1"});
  ASSERT_NE(shard_1, nullptr);

  if (lazy_record) {
    // Lazy mode linearizes the last 210 items (5..214) in order across shard 0 and shard 1.
    std::vector<int64_t> expected_shard_0(internal::kShardCapacity);
    for (size_t i = 0; i < internal::kShardCapacity; ++i) {
      expected_shard_0[i] = static_cast<int64_t>(5 + i);
    }
    EXPECT_THAT(*shard_0, NodeMatches(PropertyList(UnorderedElementsAre(
                              IntArrayIs("times", testing::SizeIs(internal::kShardCapacity)),
                              IntArrayIs("values", expected_shard_0)))));

    std::vector<int64_t> expected_shard_1(10);
    for (size_t i = 0; i < 10; ++i) {
      expected_shard_1[i] = static_cast<int64_t>(205 + i);
    }
    EXPECT_THAT(
        *shard_1,
        NodeMatches(PropertyList(UnorderedElementsAre(IntArrayIs("times", testing::SizeIs(10)),
                                                      IntArrayIs("values", expected_shard_1)))));
  } else {
    // Eager mode: items 210..214 wrapped around and overwrote slots 0..4 in shard 0.
    std::vector<int64_t> expected_shard_0(internal::kShardCapacity);
    for (size_t i = 0; i < 5; ++i) {
      expected_shard_0[i] = static_cast<int64_t>(210 + i);
    }
    for (size_t i = 5; i < internal::kShardCapacity; ++i) {
      expected_shard_0[i] = static_cast<int64_t>(i);
    }
    EXPECT_THAT(*shard_0, NodeMatches(PropertyList(UnorderedElementsAre(
                              IntArrayIs("times", testing::SizeIs(internal::kShardCapacity)),
                              IntArrayIs("values", expected_shard_0)))));

    // Shard 1 retains items 200..209.
    std::vector<int64_t> expected_shard_1(10);
    for (size_t i = 0; i < 10; ++i) {
      expected_shard_1[i] = static_cast<int64_t>(200 + i);
    }
    EXPECT_THAT(
        *shard_1,
        NodeMatches(PropertyList(UnorderedElementsAre(IntArrayIs("times", testing::SizeIs(10)),
                                                      IntArrayIs("values", expected_shard_1)))));
  }
}

INSTANTIATE_TEST_SUITE_P(CircularBufferWrapAroundTest, CircularBufferWrapAroundTest,
                         ::testing::Bool());

}  // namespace power_observability
