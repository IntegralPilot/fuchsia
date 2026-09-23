// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <bringup/lib/restricted-machine/environment.h>
#include <bringup/lib/restricted-machine/machine-type.h>
#include <bringup/lib/restricted-machine/machine.h>
#include <bringup/lib/restricted-machine/testing/machine.h>
#include <gtest/gtest.h>

#include <bringup/lib/restricted-machine/testing/fixture.gtest.h>

namespace {

class Workflow : public restricted_machine::testing::SupportedMachinesTest {
 public:
  static void SetUpTestSuite() {
    restricted_machine::testing::SupportedMachinesTest::SetUpTestSuiteHelper("example-loadable");
  }
};

TEST_P(Workflow, AffineScaleRatio) {
  std::unique_ptr<restricted_machine::Machine> machine = CreateMachine();
  auto a = environment()->MakeArgument<uint32_t>(524626542);
  auto b = environment()->MakeArgument<uint32_t>(123554);
  auto scale = environment()->MakeArgument<uint64_t>(10ULL);
  auto r = machine->Call("scale_ratio", a.get(), b.get(), scale.get());
  if (r.is_error()) {
    RM_LOG(ERROR) << "An error occurred on Call(): " << r.error_value();
    machine->LogState();
  }
  ASSERT_TRUE(r.is_ok());
  EXPECT_EQ(42461UL, r.value());
}

}  // namespace

// Invoke the tests using the supported machines.
INSTANTIATE_TEST_SUITE_P(, Workflow,
                         testing::ValuesIn(::restricted_machine::testing::kSupportedMachines),
                         ::restricted_machine::testing::SupportedMachinesTest::ParamToText);
