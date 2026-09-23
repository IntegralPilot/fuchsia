// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
// This file provides a basic testing fixture which allows users to quickly
// convert a zxtest or googletest-based set of tests to a testsuite which can be
// parameterized to run the tests against all supported architectures for the
// device under test.  Using the fixture below and testing::Machine, included
// code will be run in normal mode (via MachineType::kNone) and under each
// supported restricted mode machine target. For x86-64 and riscv64, this is
// simply 'kNative' -- the only supported machine target. For aarch64, this will
// include kNative, for 64-bit code, and kArm for 32-bit code.
//
// The fixture enables the same test definition to be used for each supported
// machine type, as defined in this file.
#ifndef SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_TESTING_FIXTURE_H_
#define SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_TESTING_FIXTURE_H_

#include <bringup/lib/restricted-machine/internal/arch-helpers.h>
#include <bringup/lib/restricted-machine/internal/common.h>
#include <bringup/lib/restricted-machine/machine.h>
#include <bringup/lib/restricted-machine/testing/machine.h>
#include <bringup/lib/restricted-machine/testing/needs-next.h>

#if !defined(ZXTEST_SKIP) && !defined(GTEST_SKIP)
#error "include fixture.gtest.h or fixture.zxtest.h"
#endif

#include <optional>

namespace restricted_machine {

namespace testing {

// Expand ::restricted_machine::kSupportedMachines to include kNone as a analog
// for use_normal_mode().
constexpr static auto kSupportedMachines = std::to_array<MachineType>({
    ::restricted_machine::MachineType::kNone,
    ::restricted_machine::MachineType::kNative,
#if defined(__aarch64__) && \
    !(__has_feature(address_sanitizer) || __has_feature(hwaddress_sanitizer))
    // Testing infrastructure expects to be able to map a shared VMO somewhere in the
    // root vmar. If the test fixture is for an Arm32 machine, then the VMO will be
    // mapped into the bottom 4GB of the root vmar. However, we can't do this for
    // sanitized builds using shadow because shadow occupies the bottom eighth of the
    // root vmar.
    ::restricted_machine::MachineType::kArm,
#endif
});

// SupportedMachinesTest provides a fixture base class for use with zxtest or
// googletest parameterized testing.
//
// The test fixture should be derived from SupportedMachinesTest and then invoke
// SetUpTestSuiteHelper() from its SetUpTestSuite() definition. To instantiate
// the tests, the user must invoke the zxtest or googletest macro as follows:
//
//  INSTANTIATE_TEST_SUITE_P(, DerivedFixtureClassName,
//                          zxtest::ValuesIn(::restricted_machine::testing::kSupportedMachines),
//                           ::restricted_machine::testing::SupportedMachinesTest::ParamToText);
//
// DerivedFixtureClass should match the derived class and zxtest will need to
// be swapped for testing:: with googletest.
//
// See //src/bringup/lib/restricted-machine/tests/example-tests.cc for usage.
class SupportedMachinesTest : public TestWithParam<restricted_machine::MachineType> {
 public:
  // Derived classes should define |SetUpTestSuiteHelper| to call this method
  // with the name of their loadable blob.
  static void SetUpTestSuiteHelper(
      const std::string_view &loadable_name,
      std::optional<const std::vector<std::string_view> *> symbols = std::nullopt,
      std::optional<size_t> shared_mem_size = std::nullopt,
      std::optional<uint64_t> address_limit = std::nullopt) {
    // Setup an environment per machine supported as there's no reason to reload
    // all the code for each testsuite run.
    for (const auto &machine_type : ::restricted_machine::testing::kSupportedMachines) {
      if (!::restricted_machine::Environment::HardwareSupported(machine_type)) {
        continue;
      }
      auto env = fbl::AdoptRef(new ::restricted_machine::Environment);
      EXPECT_TRUE(env->Initialize(machine_type,
                                  shared_mem_size.value_or(Environment::kDefaultMemoryPoolSize),
                                  address_limit.value_or(0)));
      if (symbols.has_value()) {
        ASSERT_TRUE(env->AddLoadableBlob(loadable_name, *symbols.value()).is_ok());
      } else {
        ASSERT_TRUE(env->AddLoadableBlob(loadable_name).is_ok());
      }
      if (machine_type == ::restricted_machine::MachineType::kNative) {
        // Add a reference to kNative to back normal_mode calls.
        environments_[::restricted_machine::MachineType::kNone] = env;
      }
      environments_[machine_type] = std::move(env);
    }
  }

  // Provide the base behavior for SetUp.
  virtual void SetUp() override {
    if (!has_environment()) {
      GTEST_SKIP() << "unsupported machine: " << machine().AsString();
    }
  }

  // Returns a RefPtr to the correct environment for the current test run.
  virtual fbl::RefPtr<::restricted_machine::Environment> environment() {
    auto env = environment(machine());
    ZX_ASSERT(env.is_ok());
    return std::move(env.value());
  }

  static zx::result<fbl::RefPtr<::restricted_machine::Environment>> environment(
      MachineType machine_type) {
    if (!environments_.contains(machine_type)) {
      return zx::error(ZX_ERR_NOT_FOUND);
    }
    return zx::ok(environments_[machine_type]);
  }

  // Creates a new Machine instance for use by the test against the current
  // environment and current machine test parameter.
  std::unique_ptr<::restricted_machine::Machine> CreateMachine() {
    auto mach = std::make_unique<restricted_machine::testing::Machine>(environment());
    ZX_ASSERT(mach->Initialize());
    // If the parameter is kNone, then the code should run outside of restricted
    // mode.
    ZX_ASSERT(
        mach->set_use_normal_mode(machine() == restricted_machine::MachineType::kNone).is_ok());
    // Provide the normal Machine interface to the caller now that
    // use_normal_mode is set appropriately.
    std::unique_ptr<::restricted_machine::Machine> interface(mach.release());
    return interface;
  }

  bool has_environment() const { return environments_.contains(machine()); }

  static std::string ParamToText(
      const TestParamInfo<::restricted_machine::testing::SupportedMachinesTest::ParamType> &info) {
    return std::string(info.param.AsString());
  }

  virtual ::restricted_machine::MachineType machine() const { return GetParam(); }

 protected:
  static std::unordered_map<::restricted_machine::MachineType,
                            fbl::RefPtr<::restricted_machine::Environment>>
      environments_;
};

std::unordered_map<::restricted_machine::MachineType,
                   fbl::RefPtr<::restricted_machine::Environment>>
    SupportedMachinesTest::environments_{};

}  // namespace testing
}  // namespace restricted_machine

#endif  // SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_TESTING_FIXTURE_H_
