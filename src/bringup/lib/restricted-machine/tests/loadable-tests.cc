// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/constants.h>

#include <bringup/lib/restricted-machine/internal/loadable-blob.h>
#include <bringup/lib/restricted-machine/machine.h>
#include <zxtest/zxtest.h>

namespace {

using LoadableBlob = restricted_machine::internal::LoadableBlob;

class LoadableTests : public zxtest::TestWithParam<restricted_machine::MachineType> {
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

  uint64_t GetHighMapAtAddress() {
    switch (GetParam()) {
      case restricted_machine::MachineType::kX86_64:
        // The thread vmar upper limit is normally 0x7fffffffffff.
        return 0x7ffffff00000ULL;
      case restricted_machine::MachineType::kArm:
        // This is high for 32-bit.
        return 0x00000000ffff0000ULL;
      case restricted_machine::MachineType::kAarch64:
        // The thread vmar upper limit is normally 0xffffff000000.
        return 0xfffff0000000ULL;
      case restricted_machine::MachineType::kRiscv64:
        // The thread vmar upper limit is normally 0x4000000000.
        return 0x3fffff0000ULL;
      default:
        return 0;
    }
  }

  constexpr static uint64_t kMaxSixtyFourAddressLimit = 0xffffffffffffffffULL;
  constexpr static uint64_t kMaxThirtyTwoAddressLimit = 0x00000000ffffffffULL;
  constexpr static uint64_t kMaxInvalidAddressLimit = 0x00000000000000ffULL;
  constexpr static const std::vector<std::string_view> kEmptySymbolList{};
  constexpr static bool kExportAllSymbols{true};
  constexpr static bool kDontExportAllSymbols{false};
  constexpr static std::optional<zx_vaddr_t> kNoMapAtAddress = std::nullopt;
  constexpr static std::optional<zx_vaddr_t> kLowMapAtAddress = 0x2200000UL;
  constexpr static std::optional<zx_vaddr_t> kMidMapAtAddress = 0xffff0000UL;
  constexpr static std::optional<zx_vaddr_t> kInvalidMapAtAddress = 0x2000UL;
  constexpr static std::string_view kInvalidLoadable{"i_dont_exist"};
  constexpr static std::string_view kUndefinedSymbol{"undefined_symbol"};
  constexpr static std::string_view kLoadableName{"loadable-tests-loadable"};
  constexpr static std::string_view kPrintfLoadableName{"printf"};
  constexpr static std::string_view kPingWrapper{"call_ping"};
  constexpr static std::string_view kPrintfWrapper{"call_printf"};
  const static std::unordered_map<std::string_view, uint64_t> kEmptySymbolMap;

 protected:
  restricted_machine::MachineType machine_type_;
  fbl::RefPtr<restricted_machine::Environment> environment_{};
};

const std::unordered_map<std::string_view, uint64_t> LoadableTests::kEmptySymbolMap{};

TEST_P(LoadableTests, InvalidLoadAnywhereAndImplicitlyResolve) {
  auto blob = std::make_unique<LoadableBlob>();
  // We use the default blob used by Environment for this test.
  std::string vmo_name = environment_->GetLoadableBlobPath(kInvalidLoadable);

  auto result =
      blob->Initialize(vmo_name, environment_->machine().AsElfMachine(), kMaxSixtyFourAddressLimit,
                       kEmptySymbolList, kEmptySymbolMap, kExportAllSymbols, kNoMapAtAddress);
  ASSERT_TRUE(result.is_error());
  EXPECT_EQ(ZX_ERR_BAD_HANDLE, result.error_value());
}

TEST_P(LoadableTests, LoadAnywhereAndImplicitlyResolve) {
  auto blob = std::make_unique<LoadableBlob>();
  // We use the default blob used by Environment for this test.
  std::string vmo_name =
      environment_->GetLoadableBlobPath(restricted_machine::Environment::kCallerBlobName);

  auto result =
      blob->Initialize(vmo_name, environment_->machine().AsElfMachine(), kMaxSixtyFourAddressLimit,
                       kEmptySymbolList, kEmptySymbolMap, kExportAllSymbols, kNoMapAtAddress);
  EXPECT_TRUE(result.is_ok());
  ASSERT_TRUE(
      blob->symbols().symbol_map().contains(restricted_machine::Environment::kPingFunctionName));
  ASSERT_TRUE(
      blob->symbols().symbol_map().contains(restricted_machine::Environment::kThunkFunctionName));
}

TEST_P(LoadableTests, DoubleLoadAnywhereAndImplicitlyResolve) {
  auto blob = std::make_unique<LoadableBlob>();
  // We use the default blob used by Environment for this test.
  std::string vmo_name =
      environment_->GetLoadableBlobPath(restricted_machine::Environment::kCallerBlobName);

  auto result =
      blob->Initialize(vmo_name, environment_->machine().AsElfMachine(), kMaxSixtyFourAddressLimit,
                       kEmptySymbolList, kEmptySymbolMap, kExportAllSymbols, kNoMapAtAddress);
  EXPECT_TRUE(result.is_ok());
  auto ping = blob->symbols().symbol_map().find(restricted_machine::Environment::kPingFunctionName);
  ASSERT_NE(blob->symbols().symbol_map().end(), ping);
  auto thunk =
      blob->symbols().symbol_map().find(restricted_machine::Environment::kThunkFunctionName);
  ASSERT_NE(blob->symbols().symbol_map().end(), thunk);

  // Should the same blob may be reloaded and mapped at new addresses.
  auto blob2 = std::make_unique<LoadableBlob>();
  result =
      blob2->Initialize(vmo_name, environment_->machine().AsElfMachine(), kMaxSixtyFourAddressLimit,
                        kEmptySymbolList, kEmptySymbolMap, kExportAllSymbols, kNoMapAtAddress);
  EXPECT_TRUE(result.is_ok());
  auto ping2 =
      blob2->symbols().symbol_map().find(restricted_machine::Environment::kPingFunctionName);
  ASSERT_NE(blob->symbols().symbol_map().end(), ping2);
  EXPECT_NE(ping2->second, ping->second);
  auto thunk2 =
      blob2->symbols().symbol_map().find(restricted_machine::Environment::kThunkFunctionName);
  ASSERT_NE(blob->symbols().symbol_map().end(), thunk2);
  EXPECT_NE(thunk2->second, thunk->second);
}

TEST_P(LoadableTests, LoadUnder4GbAndImplicitlyResolve) {
  auto blob = std::make_unique<LoadableBlob>();
  // We use the default blob used by Environment for this test.
  std::string vmo_name =
      environment_->GetLoadableBlobPath(restricted_machine::Environment::kCallerBlobName);

  auto result =
      blob->Initialize(vmo_name, environment_->machine().AsElfMachine(), kMaxThirtyTwoAddressLimit,
                       kEmptySymbolList, kEmptySymbolMap, kExportAllSymbols, kNoMapAtAddress);
  EXPECT_TRUE(result.is_ok());
  auto it = blob->symbols().symbol_map().find(restricted_machine::Environment::kPingFunctionName);
  ASSERT_NE(blob->symbols().symbol_map().end(), it);
  EXPECT_GT(0x00000000ffffffffULL, it->second);
  it = blob->symbols().symbol_map().find(restricted_machine::Environment::kThunkFunctionName);
  ASSERT_NE(blob->symbols().symbol_map().end(), it);
  EXPECT_GT(0x00000000ffffffffULL, it->second);
}

TEST_P(LoadableTests, LoadUnderInvalidLimitAndImplicitlyResolve) {
  auto blob = std::make_unique<LoadableBlob>();
  // We use the default blob used by Environment for this test.
  std::string vmo_name =
      environment_->GetLoadableBlobPath(restricted_machine::Environment::kCallerBlobName);

  auto result =
      blob->Initialize(vmo_name, environment_->machine().AsElfMachine(), kMaxInvalidAddressLimit,
                       kEmptySymbolList, kEmptySymbolMap, kExportAllSymbols, kNoMapAtAddress);
  ASSERT_TRUE(result.is_error());
  EXPECT_EQ(ZX_ERR_NO_MEMORY, result.error_value());
}

TEST_P(LoadableTests, MapAtLowAddressAndImplicitlyResolve) {
  auto blob = std::make_unique<LoadableBlob>();
  // We use the default blob used by Environment for this test.
  std::string vmo_name =
      environment_->GetLoadableBlobPath(restricted_machine::Environment::kCallerBlobName);

  auto result =
      blob->Initialize(vmo_name, environment_->machine().AsElfMachine(), kMaxSixtyFourAddressLimit,
                       kEmptySymbolList, kEmptySymbolMap, kExportAllSymbols, kLowMapAtAddress);
  EXPECT_TRUE(result.is_ok());
  ASSERT_TRUE(
      blob->symbols().symbol_map().contains(restricted_machine::Environment::kPingFunctionName));
  ASSERT_TRUE(
      blob->symbols().symbol_map().contains(restricted_machine::Environment::kThunkFunctionName));
}

TEST_P(LoadableTests, MapAtMidAddressAndImplicitlyResolve) {
  auto blob = std::make_unique<LoadableBlob>();
  // We use the default blob used by Environment for this test.
  std::string vmo_name =
      environment_->GetLoadableBlobPath(restricted_machine::Environment::kCallerBlobName);

  auto result =
      blob->Initialize(vmo_name, environment_->machine().AsElfMachine(), kMaxSixtyFourAddressLimit,
                       kEmptySymbolList, kEmptySymbolMap, kExportAllSymbols, kMidMapAtAddress);
  EXPECT_TRUE(result.is_ok());
  ASSERT_TRUE(
      blob->symbols().symbol_map().contains(restricted_machine::Environment::kPingFunctionName));
  ASSERT_TRUE(
      blob->symbols().symbol_map().contains(restricted_machine::Environment::kThunkFunctionName));
}

TEST_P(LoadableTests, MapAtHighAddressAndImplicitlyResolve) {
  auto blob = std::make_unique<LoadableBlob>();
  // We use the default blob used by Environment for this test.
  std::string vmo_name =
      environment_->GetLoadableBlobPath(restricted_machine::Environment::kCallerBlobName);

  auto map_at = GetHighMapAtAddress();
  auto result =
      blob->Initialize(vmo_name, environment_->machine().AsElfMachine(), kMaxSixtyFourAddressLimit,
                       kEmptySymbolList, kEmptySymbolMap, kExportAllSymbols, map_at);
  ASSERT_TRUE(result.is_ok());
  ASSERT_TRUE(
      blob->symbols().symbol_map().contains(restricted_machine::Environment::kPingFunctionName));
  ASSERT_TRUE(
      blob->symbols().symbol_map().contains(restricted_machine::Environment::kThunkFunctionName));
}

TEST_P(LoadableTests, MapAtInvalidAddressAndImplicitlyResolve) {
  auto blob = std::make_unique<LoadableBlob>();
  // We use the default blob used by Environment for this test.
  std::string vmo_name =
      environment_->GetLoadableBlobPath(restricted_machine::Environment::kCallerBlobName);

  auto result =
      blob->Initialize(vmo_name, environment_->machine().AsElfMachine(), kMaxSixtyFourAddressLimit,
                       kEmptySymbolList, kEmptySymbolMap, kExportAllSymbols, kInvalidMapAtAddress);
  ASSERT_TRUE(result.is_error());
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, result.error_value());
}

TEST_P(LoadableTests, LoadAnywhereAndExplicitlyResolve) {
  auto blob = std::make_unique<LoadableBlob>();
  // We use the default blob used by Environment for this test.
  std::string vmo_name =
      environment_->GetLoadableBlobPath(restricted_machine::Environment::kCallerBlobName);

  auto result =
      blob->Initialize(vmo_name, environment_->machine().AsElfMachine(), kMaxSixtyFourAddressLimit,
                       {restricted_machine::Environment::kPingFunctionName}, kEmptySymbolMap,
                       kDontExportAllSymbols, kNoMapAtAddress);
  EXPECT_TRUE(result.is_ok());
  ASSERT_TRUE(
      blob->symbols().symbol_map().contains(restricted_machine::Environment::kPingFunctionName));
  ASSERT_FALSE(
      blob->symbols().symbol_map().contains(restricted_machine::Environment::kThunkFunctionName));
}

TEST_P(LoadableTests, LoadAnywhereAndExplicitlyResolveUndefinedSymbol) {
  auto blob = std::make_unique<LoadableBlob>();
  // We use the default blob used by Environment for this test.
  std::string vmo_name =
      environment_->GetLoadableBlobPath(restricted_machine::Environment::kCallerBlobName);

  auto result =
      blob->Initialize(vmo_name, environment_->machine().AsElfMachine(), kMaxSixtyFourAddressLimit,
                       {kUndefinedSymbol}, kEmptySymbolMap, kDontExportAllSymbols, kNoMapAtAddress);
  EXPECT_TRUE(result.is_error());
  EXPECT_EQ(ZX_ERR_NOT_FOUND, result.error_value());
}

TEST_P(LoadableTests, SymbolicRelocation) {
  // We want to validate the relocation, so we test via machine.
  restricted_machine::Machine machine(environment_);
  ASSERT_TRUE(machine.Initialize());

  ASSERT_TRUE(
      environment_->AddLoadableBlob(restricted_machine::Environment::kCallerBlobName).is_ok());
  ASSERT_TRUE(environment_->AddLoadableBlob(kPrintfLoadableName).is_ok());
  ASSERT_TRUE(environment_->AddLoadableBlob(kLoadableName).is_ok());

  auto arg0 = environment_->MakeArgument<uint64_t>(2);
  auto result = machine.Call(kPingWrapper, arg0.get());
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(8, result.value());
}

TEST_P(LoadableTests, MissingSymbolicRelocation) {
  restricted_machine::Machine machine(environment_);
  ASSERT_TRUE(machine.Initialize());

  // Only printf should be missing.
  ASSERT_TRUE(
      environment_->AddLoadableBlob(restricted_machine::Environment::kCallerBlobName).is_ok());
  auto result = environment_->AddLoadableBlob(kLoadableName);
  ASSERT_TRUE(result.is_error());
  EXPECT_EQ(ZX_ERR_NOT_FOUND, result.error_value());
}

TEST_P(LoadableTests, MultipleMissingSymbolicRelocation) {
  restricted_machine::Machine machine(environment_);
  ASSERT_TRUE(machine.Initialize());

  auto result = environment_->AddLoadableBlob(kLoadableName);
  ASSERT_TRUE(result.is_error());
  EXPECT_EQ(ZX_ERR_NOT_FOUND, result.error_value());
}

// This could land in either loadable or machine tests.
TEST_P(LoadableTests, SymbolicRelocationWithSyscallContinuation) {
  // We want to validate the relocation, so we test via machine.
  restricted_machine::Machine machine(environment_);
  ASSERT_TRUE(machine.Initialize());

  ASSERT_TRUE(
      environment_->AddLoadableBlob(restricted_machine::Environment::kCallerBlobName).is_ok());
  ASSERT_TRUE(environment_->AddLoadableBlob(kPrintfLoadableName).is_ok());
  ASSERT_TRUE(environment_->AddLoadableBlob(kLoadableName).is_ok());

  auto arg0 = environment_->MakeArgument<uint64_t>(2);
  auto result = machine.Call(kPrintfWrapper, arg0.get());
  ASSERT_TRUE(result.is_error());
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, result.error_value());
  machine.LogState(ZX_RESTRICTED_REASON_SYSCALL);
  EXPECT_EQ(ZX_RESTRICTED_REASON_SYSCALL, machine.last_reason());
  // Ensure the __NR_write syscall was invoked.
  switch (GetParam()) {
    case restricted_machine::MachineType::kX86_64:
      static constexpr uint64_t kLinuxX64WriteNr = 1;
      EXPECT_EQ(kLinuxX64WriteNr, machine.registers()->syscall_number());
      break;
    case restricted_machine::MachineType::kArm:
      static constexpr uint64_t kLinuxArmWriteNr = 4;
      EXPECT_EQ(kLinuxArmWriteNr, machine.registers()->syscall_number());
      break;
    case restricted_machine::MachineType::kAarch64:
    case restricted_machine::MachineType::kRiscv64:
      static constexpr uint64_t kLinuxAarch64WriteNr = 64;
      EXPECT_EQ(kLinuxAarch64WriteNr, machine.registers()->syscall_number());
      break;
    default:
      ASSERT_TRUE(false) << "Unsupported parameter";
  }
  int fd = static_cast<int>(machine.registers()->syscall_arg(0));
  const char *buf = reinterpret_cast<const char *>(machine.registers()->syscall_arg(1));
  size_t count = static_cast<size_t>(machine.registers()->syscall_arg(2));
  // Confirm the syscall arguments.
  ASSERT_EQ(1, fd);
  ASSERT_NE(nullptr, buf);
  ASSERT_LT(0, count);
  // Ensure the correct buffer was sent and return the bytes "written".
  EXPECT_EQ(std::string("A number: 2\n"), std::string(buf, count));
  machine.registers()->set_syscall_return(count);
  ASSERT_TRUE(machine.CommitState().is_ok());
  result = machine.Enter();
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(0, result.value());
}

}  // anonymous namespace

INSTANTIATE_TEST_SUITE_P(, LoadableTests, zxtest::ValuesIn(restricted_machine::kSupportedMachines),
                         [](const zxtest::TestParamInfo<LoadableTests::ParamType> &info) {
                           return std::string(info.param.AsString());
                         });

int main(int argc, char **argv) { return RUN_ALL_TESTS(argc, argv); }
