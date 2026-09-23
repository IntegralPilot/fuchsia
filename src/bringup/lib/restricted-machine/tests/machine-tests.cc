// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/constants.h>

#include <bringup/lib/restricted-machine/machine.h>
#include <zxtest/zxtest.h>

namespace {

class MachineTests : public zxtest::TestWithParam<restricted_machine::MachineType> {
 public:
  void SetUp() override {
    environment_ = fbl::AdoptRef(new restricted_machine::Environment);
    machine_type_ = GetParam();
    if (!restricted_machine::Environment::HardwareSupported(machine_type_)) {
      ZXTEST_SKIP() << "unsupported machine type: " << machine_type_.AsString();
      return;
    }
    ASSERT_TRUE(environment_->Initialize(machine_type_));
  }

 protected:
  restricted_machine::MachineType machine_type_;
  fbl::RefPtr<restricted_machine::Environment> environment_{};
};

TEST_P(MachineTests, PingValueArgs) {
  restricted_machine::Machine machine(environment_);
  if (environment_->address_limit() != 0) {
    ZXTEST_SKIP() << "this cannot be run on address limited environments";
  }
  ASSERT_TRUE(machine.Initialize());
  auto r = machine.Call(restricted_machine::Environment::kPingFunctionName, 1, 2, 3, 4);
  EXPECT_TRUE(r.is_error());
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, r.error_value());
}

TEST_P(MachineTests, Ping) {
  restricted_machine::Machine machine(environment_);
  ASSERT_TRUE(machine.Initialize());
  auto arg0 = environment_->MakeArgument<uint64_t>(2);
  auto arg1 = environment_->MakeArgument<uint64_t>(3);
  auto arg2 = environment_->MakeArgument<uint64_t>(4);
  auto arg3 = environment_->MakeArgument<uint64_t>(5);
  auto r = machine.Call(restricted_machine::Environment::kPingFunctionName, arg0.get(), arg1.get(),
                        arg2.get(), arg3.get());
  ASSERT_TRUE(r.is_ok());
  EXPECT_EQ(14, r.value());
}

// Derive a machine to test arg prepping
class ArgMachine : public restricted_machine::Machine {
 public:
  ArgMachine(fbl::RefPtr<restricted_machine::Environment> e) : restricted_machine::Machine(e) {}
  ~ArgMachine() = default;
  template <typename... Args>
  zx::result<> doPrepArgs(std::vector<uint64_t> *args, Args... vargs) {
    return prepArgs(args, vargs...);
  }
};

TEST_P(MachineTests, PrepArgsAllocationLimit) {
  auto e = AdoptRef(new restricted_machine::Environment);
  uint64_t limit = 0x4fffffff;
  ASSERT_TRUE(e->Initialize(machine_type_, 4096 * 12, limit));

  ArgMachine machine(e);
  ASSERT_TRUE(machine.Initialize());
  auto arg0 = e->MakeArgument<std::string>("Hello World");
  auto arg1 = e->MakeArgument<uint64_t>(1);
  auto arg2 = e->MakeArgument<uint64_t>(2);
  auto arg3 = e->MakeArgument<uint64_t>(3);
  std::vector<uint64_t> args;
  auto result = machine.doPrepArgs(&args, arg0.get(), arg1.get(), arg2.get(), arg3.get());
  EXPECT_OK(result);
  ASSERT_EQ(args.size(), 4);
  EXPECT_GT(limit, args[0]);
  EXPECT_GT(limit, args[1]);
  EXPECT_GT(limit, args[2]);
  EXPECT_GT(limit, args[3]);
  EXPECT_STREQ(reinterpret_cast<std::string *>(args[0])->c_str(), arg0->c_str());
  EXPECT_GT(limit, reinterpret_cast<uint64_t>(reinterpret_cast<std::string *>(args[0])->data()));
  EXPECT_EQ(*reinterpret_cast<uint64_t *>(args[1]), 1ULL);
  EXPECT_EQ(*reinterpret_cast<uint64_t *>(args[2]), 2ULL);
  EXPECT_EQ(*reinterpret_cast<uint64_t *>(args[3]), 3ULL);
}

TEST_P(MachineTests, PrepArgsOutOfRange) {
  auto e = AdoptRef(new restricted_machine::Environment);
  uint64_t limit = 0x4fffffff;
  ASSERT_TRUE(e->Initialize(machine_type_, 4096 * 12, limit));
  ArgMachine machine(e);
  ASSERT_TRUE(machine.Initialize());
  auto arg = e->MakeArgument<std::string>("Hello World");
  // The stack-based value should be out of range reliably.
  std::vector<uint64_t> args;
  uint64_t stack_arg = 2;
  auto result = machine.doPrepArgs(&args, &stack_arg, arg.get());
  EXPECT_TRUE(result.is_error());
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, result.error_value());
  EXPECT_EQ(args.size(), 0);

  args.clear();
  result = machine.doPrepArgs(&args, arg.get(), &stack_arg);
  EXPECT_TRUE(result.is_error());
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, result.error_value());
  EXPECT_EQ(args.size(), 1);

  args.clear();
  result = machine.doPrepArgs(&args, arg.get(), arg.get(), &stack_arg);
  EXPECT_TRUE(result.is_error());
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, result.error_value());
  EXPECT_EQ(args.size(), 2);

  args.clear();
  result = machine.doPrepArgs(&args, arg.get(), arg.get(), arg.get(), &stack_arg);
  EXPECT_TRUE(result.is_error());
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, result.error_value());
  EXPECT_EQ(args.size(), 3);
}

// This test validates the FPU registers workflow ensuring a value is returned
// in the FPU registers from the restricted environment.
TEST_P(MachineTests, FpuRegisterPreservation) {
  restricted_machine::Machine machine(environment_);
  ASSERT_TRUE(machine.Initialize());
  machine.enable_fpu_registers(true);

  // Make a buffer and fill it with known values
  std::vector<char> expected_fpu_buffer;
  memset(machine.FpuRegisters()->data(), 0xAA, machine.FpuRegisters()->size());
  // Ping will set the first 64-bits of FPU register space to the first
  // argument's value.
  auto arg0 = environment_->MakeArgument<uint64_t>(2);

  // Call ping()
  auto arg1 = environment_->MakeArgument<uint64_t>(5);
  auto arg2 = environment_->MakeArgument<uint64_t>(4);
  auto arg3 = environment_->MakeArgument<uint64_t>(3);
  auto r = machine.Call(restricted_machine::Environment::kPingFunctionName, arg0.get(), arg1.get(),
                        arg2.get(), arg3.get());
  ASSERT_TRUE(r.is_ok());
  EXPECT_EQ(14, r.value());

  // Ping will set the first 64-bits of FPU register space to the first
  // argument's value.
  uint8_t *value = reinterpret_cast<uint8_t *>(arg0.get());
  for (uint16_t i = 0; i < 8; i++) {
    EXPECT_EQ(machine.FpuRegisters()->at(i), value[i]);
  }
  // Now we can see if the other FPU registers survived.
  for (uint16_t i = 8; i < machine.FpuRegisters()->size(); i++) {
    EXPECT_EQ(0xAA, machine.FpuRegisters()->at(i)) << "i: " << i;
  }
}

}  // anonymous namespace

INSTANTIATE_TEST_SUITE_P(, MachineTests, zxtest::ValuesIn(restricted_machine::kSupportedMachines),
                         [](const zxtest::TestParamInfo<MachineTests::ParamType> &info) {
                           return std::string(info.param.AsString());
                         });

int main(int argc, char **argv) { return RUN_ALL_TESTS(argc, argv); }
