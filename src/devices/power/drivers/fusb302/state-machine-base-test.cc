// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/power/drivers/fusb302/state-machine-base.h"

#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/fpromise/single_threaded_executor.h>
#include <lib/inspect/cpp/hierarchy.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/cpp/reader.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <zircon/assert.h>

#include <cinttypes>
#include <cstddef>
#include <cstdint>
#include <optional>
#include <utility>

#include <fbl/vector.h>
#include <gtest/gtest.h>

namespace fusb302 {

namespace {

enum class MockState : int32_t {
  kInitial = 1,
  kWaitingForInput = 2,
  kLoopDone = 3,
};

class MockInput {
 public:
  MockInput() = default;

  // Not copyable, to ensure that StateMachineBase doesn't try to copy inputs.
  MockInput(const MockInput&) = delete;
  MockInput& operator=(const MockInput&) = delete;

  ~MockInput() = default;
};

class MockStateMachine : public StateMachineBase<MockStateMachine, MockState, const MockInput&> {
 public:
  explicit MockStateMachine(inspect::Node root_node)
      : StateMachineBase(MockState::kInitial, std::move(root_node), "MockStateMachine") {}

  MockStateMachine(const MockStateMachine&) = delete;
  MockStateMachine& operator=(const MockStateMachine&) = delete;

  ~MockStateMachine() override = default;

  const char* StateToString(MockState state) const override {
    switch (state) {
      case MockState::kInitial:
        return "Initial";
      case MockState::kWaitingForInput:
        return "WaitingForInput";
      case MockState::kLoopDone:
        return "LoopDone";
    }
    ZX_DEBUG_ASSERT_MSG(false, "Invalid MockState: %" PRId32, static_cast<int32_t>(state));
    return "(invalid)";
  }

  void ExpectEnterState(MockState state) {
    expected_calls_.push_back({.call_type = CallType::kEnterState, .state = state});
  }

  void ExpectEvaluateInput(const MockInput& input, MockState current_state, MockState next_state) {
    expected_calls_.push_back({
        .call_type = CallType::kEvaluateInput,
        .state = current_state,
        .mock_input = &input,
        .next_state = next_state,
    });
  }

  void ExpectExitState(MockState state) {
    expected_calls_.push_back({.call_type = CallType::kExitState, .state = state});
  }

  void CheckAllAccessesReplayed() { EXPECT_EQ(expected_calls_.size(), expected_call_index_); }

  void ForceStateTransition(MockState new_state) {
    StateMachineBase::ForceStateTransition(new_state);
  }

 protected:
  void EnterState(MockState state) override {
    CheckCall({.call_type = CallType::kEnterState, .state = state});
  }

  MockState NextState(const MockInput& input, MockState current_state) override {
    // We'll return `current_state` if we don't have a suitable expectation.
    MockState next_state = current_state;
    if (expected_call_index_ < expected_calls_.size()) {
      next_state = expected_calls_[expected_call_index_].next_state.value_or(current_state);
    }

    CheckCall({
        .call_type = CallType::kEvaluateInput,
        .state = current_state,
        .mock_input = &input,
        .next_state = std::nullopt,
    });
    return next_state;
  }

  void ExitState(MockState state) override {
    CheckCall({.call_type = CallType::kExitState, .state = state});
  }

 private:
  enum class CallType : int32_t {
    kEnterState = 1,
    kEvaluateInput = 2,
    kExitState = 3,
  };

  struct CallInfo {
    CallType call_type;
    MockState state;

    // nullptr if `call_type` is not `kEvaluateInput`.
    const MockInput* mock_input = nullptr;

    // nullopt if `call_type` is not `kEvaluateInput` or for actual calls.
    std::optional<MockState> next_state = std::nullopt;
  };

  void CheckCall(const CallInfo& actual_call) {
    ASSERT_LT(expected_call_index_, expected_calls_.size());

    const CallInfo& expected_call = expected_calls_[expected_call_index_];
    ++expected_call_index_;

    EXPECT_EQ(expected_call.call_type, actual_call.call_type);
    EXPECT_EQ(expected_call.state, actual_call.state);
    EXPECT_EQ(expected_call.mock_input, actual_call.mock_input);
  }

  fbl::Vector<CallInfo> expected_calls_;
  mutable size_t expected_call_index_ = 0;
};

class StateMachineBaseTest : public ::testing::Test {
 public:
  void TearDown() override { state_machine_.CheckAllAccessesReplayed(); }

  void ExpectInspectStateEquals(MockState state) {
    fpromise::result<inspect::Hierarchy> hierarchy_result =
        fpromise::run_single_threaded(inspect::ReadFromInspector(inspect_));
    ASSERT_TRUE(hierarchy_result.is_ok());

    inspect::Hierarchy hierarchy = std::move(hierarchy_result.value());
    const inspect::Hierarchy* state_machine_root = hierarchy.GetByPath({"MockStateMachine"});
    ASSERT_TRUE(state_machine_root);

    const inspect::IntPropertyValue* actual_state_value =
        state_machine_root->node().get_property<inspect::IntPropertyValue>("State");
    ASSERT_TRUE(actual_state_value);

    EXPECT_EQ(actual_state_value->value(), static_cast<int32_t>(state));
  }

 protected:
  fdf_testing::ScopedGlobalLogger logger_;

  inspect::Inspector inspect_;
  MockStateMachine state_machine_{inspect_.GetRoot().CreateChild("MockStateMachine")};
};

TEST_F(StateMachineBaseTest, ConstructorSetsInitialState) {
  EXPECT_EQ(MockState::kInitial, state_machine_.current_state());
  ExpectInspectStateEquals(MockState::kInitial);
}

TEST_F(StateMachineBaseTest, EvaluateInputEntry) {
  MockInput input1;

  state_machine_.ExpectEnterState(MockState::kInitial);
  state_machine_.ExpectEvaluateInput(input1, MockState::kInitial, MockState::kInitial);

  state_machine_.Run(input1);
  EXPECT_EQ(MockState::kInitial, state_machine_.current_state());
}

TEST_F(StateMachineBaseTest, EvaluateInputNoChange) {
  MockInput input1, input2;

  state_machine_.ExpectEnterState(MockState::kInitial);
  state_machine_.ExpectEvaluateInput(input1, MockState::kInitial, MockState::kInitial);
  state_machine_.ExpectEvaluateInput(input2, MockState::kInitial, MockState::kInitial);

  state_machine_.Run(input1);
  state_machine_.Run(input2);
  EXPECT_EQ(MockState::kInitial, state_machine_.current_state());
}

TEST_F(StateMachineBaseTest, EvaluateInputEntryTransition) {
  MockInput input1;

  state_machine_.ExpectEnterState(MockState::kInitial);
  state_machine_.ExpectEvaluateInput(input1, MockState::kInitial, MockState::kWaitingForInput);
  state_machine_.ExpectExitState(MockState::kInitial);
  state_machine_.ExpectEnterState(MockState::kWaitingForInput);
  state_machine_.ExpectEvaluateInput(input1, MockState::kWaitingForInput,
                                     MockState::kWaitingForInput);

  state_machine_.Run(input1);
  EXPECT_EQ(MockState::kWaitingForInput, state_machine_.current_state());
  ExpectInspectStateEquals(MockState::kWaitingForInput);
}

TEST_F(StateMachineBaseTest, EvaluateInputLoop) {
  MockInput input1, input2, input3, input4;

  state_machine_.ExpectEnterState(MockState::kInitial);
  state_machine_.ExpectEvaluateInput(input1, MockState::kInitial, MockState::kWaitingForInput);
  state_machine_.ExpectExitState(MockState::kInitial);
  state_machine_.ExpectEnterState(MockState::kWaitingForInput);
  state_machine_.ExpectEvaluateInput(input1, MockState::kWaitingForInput,
                                     MockState::kWaitingForInput);
  state_machine_.ExpectEvaluateInput(input2, MockState::kWaitingForInput,
                                     MockState::kWaitingForInput);
  state_machine_.ExpectEvaluateInput(input3, MockState::kWaitingForInput,
                                     MockState::kWaitingForInput);
  state_machine_.ExpectEvaluateInput(input4, MockState::kWaitingForInput, MockState::kLoopDone);
  state_machine_.ExpectExitState(MockState::kWaitingForInput);
  state_machine_.ExpectEnterState(MockState::kLoopDone);
  state_machine_.ExpectEvaluateInput(input4, MockState::kLoopDone, MockState::kLoopDone);

  state_machine_.Run(input1);

  state_machine_.Run(input2);
  EXPECT_EQ(MockState::kWaitingForInput, state_machine_.current_state());

  state_machine_.Run(input3);
  EXPECT_EQ(MockState::kWaitingForInput, state_machine_.current_state());

  state_machine_.Run(input4);
  EXPECT_EQ(MockState::kLoopDone, state_machine_.current_state());
  ExpectInspectStateEquals(MockState::kLoopDone);
}

TEST_F(StateMachineBaseTest, ForceStateTransitionNoTransitionBeforeEntry) {
  MockInput input;

  state_machine_.ExpectEnterState(MockState::kInitial);
  state_machine_.ExpectEvaluateInput(input, MockState::kInitial, MockState::kInitial);

  state_machine_.ForceStateTransition(MockState::kInitial);
  EXPECT_EQ(MockState::kInitial, state_machine_.current_state());
  state_machine_.Run(input);
  EXPECT_EQ(MockState::kInitial, state_machine_.current_state());
}

TEST_F(StateMachineBaseTest, ForceStateTransitionNoTransitionAfterEntry) {
  MockInput input1, input2;

  state_machine_.ExpectEnterState(MockState::kInitial);
  state_machine_.ExpectEvaluateInput(input1, MockState::kInitial, MockState::kInitial);
  state_machine_.ExpectEvaluateInput(input2, MockState::kInitial, MockState::kInitial);

  state_machine_.Run(input1);
  state_machine_.ForceStateTransition(MockState::kInitial);
  EXPECT_EQ(MockState::kInitial, state_machine_.current_state());
  state_machine_.Run(input2);
}

TEST_F(StateMachineBaseTest, ForceStateTransitionStateChangeCausesExit) {
  MockInput input;

  state_machine_.ExpectEnterState(MockState::kInitial);
  state_machine_.ExpectEvaluateInput(input, MockState::kInitial, MockState::kInitial);
  state_machine_.ExpectExitState(MockState::kInitial);

  state_machine_.Run(input);
  state_machine_.ForceStateTransition(MockState::kLoopDone);
  EXPECT_EQ(MockState::kLoopDone, state_machine_.current_state());
}

TEST_F(StateMachineBaseTest, ForceStateTransitionStateChangeAfterConstructorNoExit) {
  // No transitions occur.
  state_machine_.ForceStateTransition(MockState::kLoopDone);
  EXPECT_EQ(MockState::kLoopDone, state_machine_.current_state());
}

TEST_F(StateMachineBaseTest, ForceStateTransitionStateChangeCausesDeferedEnter) {
  MockInput input1, input2;

  state_machine_.ExpectEnterState(MockState::kInitial);
  state_machine_.ExpectEvaluateInput(input1, MockState::kInitial, MockState::kInitial);
  state_machine_.ExpectExitState(MockState::kInitial);
  state_machine_.ExpectEnterState(MockState::kLoopDone);
  state_machine_.ExpectEvaluateInput(input2, MockState::kLoopDone, MockState::kLoopDone);

  state_machine_.Run(input1);
  state_machine_.ForceStateTransition(MockState::kLoopDone);
  EXPECT_EQ(MockState::kLoopDone, state_machine_.current_state());
  state_machine_.Run(input2);
}

TEST_F(StateMachineBaseTest, ForceStateTransitionStateChangeAfterConstructorNoExitDeferedEnter) {
  MockInput input;

  state_machine_.ExpectEnterState(MockState::kLoopDone);
  state_machine_.ExpectEvaluateInput(input, MockState::kLoopDone, MockState::kLoopDone);

  state_machine_.ForceStateTransition(MockState::kLoopDone);
  EXPECT_EQ(MockState::kLoopDone, state_machine_.current_state());
  state_machine_.Run(input);
}

}  // namespace

}  // namespace fusb302
