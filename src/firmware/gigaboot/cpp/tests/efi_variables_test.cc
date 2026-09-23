// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "efi_variables.h"

#include <lib/efi/testing/stub_boot_services.h>
#include <lib/efi/testing/stub_runtime_services.h>

#include <span>

#include <efi/variable/variable_id.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <phys/efi/main.h>

using ::efi::MockBootServices;
using ::efi::MockRuntimeServices;
using ::efi::StubRuntimeServices;
using ::testing::_;
using ::testing::ElementsAreArray;
using ::testing::Eq;
using ::testing::NiceMock;
using ::testing::Return;
using ::testing::Test;

const efi_handle kImageHandle = reinterpret_cast<efi_handle>(0x10);

using namespace gigaboot;

namespace {

class EfiVariablesTest : public Test {
 public:
  void SetUp() override {
    // Configure the necessary mocks for memory operation.
    system_table_ = efi_system_table{
        .RuntimeServices = mock_runtime_services_.services(),
        .BootServices = mock_boot_services_.services(),
    };

    // EfiMain(kImageHandle, &system_table_);
    gEfiImageHandle = kImageHandle;
    gEfiSystemTable = &system_table_;

    ON_CALL(mock_runtime_services_, QueryVariableInfo).WillByDefault(Return(EFI_NOT_FOUND));
    ON_CALL(mock_runtime_services_, GetNextVariableName).WillByDefault(Return(EFI_NOT_FOUND));
    ON_CALL(mock_runtime_services_, GetVariable).WillByDefault(Return(EFI_NOT_FOUND));
  }

  void TearDown() override {
    gEfiImageHandle = 0;
    gEfiSystemTable = nullptr;
  }

 protected:
  NiceMock<MockRuntimeServices> mock_runtime_services_;
  NiceMock<MockBootServices> mock_boot_services_;
  efi_system_table system_table_ = {};
  EfiVariables efi_variables_;
};

class EfiVariablesStubTest : public Test {
 public:
  void SetUp() override {
    // Configure the necessary mocks for memory operation.
    system_table_ = efi_system_table{
        .RuntimeServices = stub_runtime_services_.services(),
        .BootServices = mock_boot_services_.services(),
    };

    // EfiMain(kImageHandle, &system_table_);
    gEfiImageHandle = kImageHandle;
    gEfiSystemTable = &system_table_;
  }

  void TearDown() override {
    gEfiImageHandle = 0;
    gEfiSystemTable = nullptr;
  }

  void SetVariables(const std::list<efi::Variable>& vars) {
    stub_runtime_services_.SetVariables(vars);
  }

 protected:
  StubRuntimeServices stub_runtime_services_;
  NiceMock<MockBootServices> mock_boot_services_;
  efi_system_table system_table_ = {};
  EfiVariables efi_variables_;
};

class EfiVariableInfoFixture
    : public ::testing::TestWithParam<std::list<EfiVariables::EfiVariableInfo>> {};

TEST_P(EfiVariableInfoFixture, Eq) {
  std::list<EfiVariables::EfiVariableInfo> input = GetParam();
  for (const auto& a : input) {
    EXPECT_EQ(a, a);
  }
}

TEST_P(EfiVariableInfoFixture, Ne) {
  std::list<EfiVariables::EfiVariableInfo> input = GetParam();
  for (auto a = input.begin(); a != input.end(); a++) {
    for (auto b = std::next(a); b != input.end(); b++) {
      EXPECT_NE(*a, *b);
    }
  }
}

INSTANTIATE_TEST_SUITE_P(EfiVariableInfoTest, EfiVariableInfoFixture,
                         ::testing::Values(std::list<EfiVariables::EfiVariableInfo>{
                             {0, 0, 0},
                             {1, 2, 3},
                             {3, 2, 1},
                             {std::numeric_limits<uint64_t>::max(), 2, 1},
                             {3, std::numeric_limits<uint64_t>::max(), 1},
                             {3, 2, std::numeric_limits<uint64_t>::max()},
                             {std::numeric_limits<uint64_t>::max(),
                              std::numeric_limits<uint64_t>::max(),
                              std::numeric_limits<uint64_t>::max()},
                         }));

TEST_F(EfiVariablesTest, EfiQueryVariableInfoError) {
  auto res = efi_variables_.EfiQueryVariableInfo();
  ASSERT_TRUE(res.is_error());
}

TEST_F(EfiVariablesTest, DumpVarInfoSuccess) {
  constexpr EfiVariables::EfiVariableInfo expected = {
      .max_var_storage_size = 1,
      .remaining_var_storage_size = 2,
      .max_var_size = 3,
  };
  EXPECT_CALL(mock_runtime_services_, QueryVariableInfo)
      .WillOnce([](uint32_t attributes, uint64_t* max_var_storage_size,
                   uint64_t* remaining_var_storage_size, uint64_t* max_var_size) {
        *max_var_storage_size = expected.max_var_storage_size;
        *remaining_var_storage_size = expected.remaining_var_storage_size;
        *max_var_size = expected.max_var_size;
        return EFI_SUCCESS;
      });

  auto res = efi_variables_.EfiQueryVariableInfo();

  ASSERT_TRUE(res.is_ok());
  EXPECT_EQ(res.value(), expected);
}

TEST_F(EfiVariablesTest, DumpVarInfoMaxValues) {
  constexpr EfiVariables::EfiVariableInfo expected = {
      .max_var_storage_size = std::numeric_limits<uint64_t>::max(),
      .remaining_var_storage_size = std::numeric_limits<uint64_t>::max(),
      .max_var_size = std::numeric_limits<uint64_t>::max(),
  };
  EXPECT_CALL(mock_runtime_services_, QueryVariableInfo)
      .WillOnce([](uint32_t attributes, uint64_t* max_var_storage_size,
                   uint64_t* remaining_var_storage_size, uint64_t* max_var_size) {
        *max_var_storage_size = expected.max_var_storage_size;
        *remaining_var_storage_size = expected.remaining_var_storage_size;
        *max_var_size = expected.max_var_size;
        return EFI_SUCCESS;
      });

  auto res = efi_variables_.EfiQueryVariableInfo();

  ASSERT_TRUE(res.is_ok());
  EXPECT_EQ(res.value(), expected);
}

constexpr efi_guid kGuid[] = {
    {0x0, 0x0, 0x0, {0x0}},
    {0x1, 0x1, 0x1, {0x1}},
    {0x2, 0x2, 0x2, {0x2}},
    {0x3, 0x3, 0x3, {0x3}},
};
static const efi::VariableId kVariableId[] = {
    {efi::String(u"var0"), kGuid[0]},
    {efi::String(u"var1"), kGuid[1]},
    {efi::String(u"var2"), kGuid[2]},
    {efi::String(u"var3"), kGuid[3]},
};
static const efi::VariableValue kValue[] = {
    {0x00},
    {0x01, 0x02},
    {0x01, 0x02, 0x03},
    {0x01, 0x02, 0x03, 0x04},
};

TEST_F(EfiVariablesStubTest, GetVariableNotThere) {
  auto res = efi_variables_.EfiGetVariable(kVariableId[0]);
  ASSERT_TRUE(res.is_error());
}

// Helper function to work around matching `fbl::Vector`
// Alternative is to add `using value_type = T;` to `fbl::Vector` class;
std::span<const uint8_t> ToSpan(const efi::VariableValue& value) {
  return std::span<const uint8_t>(value.begin(), value.end());
}

TEST_F(EfiVariablesStubTest, GetVariableGood) {
  SetVariables({{kVariableId[0], kValue[0]}});

  auto res = efi_variables_.EfiGetVariable(kVariableId[0]);
  ASSERT_TRUE(res.is_ok());
  EXPECT_THAT(ToSpan(kValue[0]), ElementsAreArray(res.value()));
}

TEST_F(EfiVariablesStubTest, GetVariableMultipleGood) {
  SetVariables({
      {kVariableId[0], kValue[0]},
      {kVariableId[1], kValue[1]},
      {kVariableId[2], kValue[2]},
      {kVariableId[3], kValue[3]},
  });

  for (size_t i = 0; i < std::size(kVariableId); i++) {
    auto res = efi_variables_.EfiGetVariable(kVariableId[i]);
    ASSERT_TRUE(res.is_ok());
    EXPECT_THAT(ToSpan(kValue[i]), ElementsAreArray(res.value()));
  }
}

TEST_F(EfiVariablesStubTest, GetVariableBadGuid) {
  SetVariables({{{kVariableId[0].name, kGuid[1]}, kValue[0]}});

  auto res = efi_variables_.EfiGetVariable(kVariableId[0]);
  ASSERT_TRUE(res.is_error());
}

TEST_F(EfiVariablesStubTest, GetVariableBadName) {
  SetVariables({{{kVariableId[1].name, kGuid[0]}, kValue[0]}});

  auto res = efi_variables_.EfiGetVariable(kVariableId[0]);
  ASSERT_TRUE(res.is_error());
}

TEST_F(EfiVariablesStubTest, GetVariableGuidDiffers) {
  SetVariables({
      {{kVariableId[1].name, kGuid[0]}, kValue[1]},
      {kVariableId[0], kValue[0]},
      {{kVariableId[2].name, kGuid[0]}, kValue[2]},
  });

  auto res = efi_variables_.EfiGetVariable(kVariableId[0]);
  ASSERT_TRUE(res.is_ok());
  EXPECT_THAT(ToSpan(kValue[0]), ElementsAreArray(res.value()));
}

TEST_F(EfiVariablesTest, GetVariableNotThere) {
  auto res = efi_variables_.EfiGetVariable(kVariableId[0]);
  ASSERT_TRUE(res.is_error());
}

TEST_F(EfiVariablesStubTest, GetGuidFromEmptyList) {
  SetVariables({});
  auto res = efi_variables_.GetGuid(std::u16string_view(kVariableId[0].name));
  ASSERT_TRUE(res.is_error());
}

TEST_F(EfiVariablesStubTest, GetGuidOneExist) {
  SetVariables({
      {kVariableId[0], kValue[0]},
      {kVariableId[1], kValue[1]},
  });

  auto res = efi_variables_.GetGuid(std::u16string_view(kVariableId[0].name));

  ASSERT_TRUE(res.is_ok());
  EXPECT_EQ(res.value(), kVariableId[0].vendor_guid);
}

TEST_F(EfiVariablesStubTest, GetGuidMultipleExist) {
  SetVariables({
      {kVariableId[0], kValue[0]},
      {kVariableId[0], kValue[1]},
  });

  auto res = efi_variables_.GetGuid(std::u16string_view(kVariableId[0].name));

  ASSERT_TRUE(res.is_error());
}

TEST_F(EfiVariablesStubTest, GetGuidMissing) {
  SetVariables({
      {kVariableId[0], kValue[0]},
      {kVariableId[1], kValue[1]},
  });

  auto res = efi_variables_.GetGuid(std::u16string_view(kVariableId[2].name));

  ASSERT_TRUE(res.is_error());
}

TEST_F(EfiVariablesStubTest, GetGuidBadInput) {
  SetVariables({
      {kVariableId[0], kValue[0]},
      {kVariableId[1], kValue[1]},
  });

  auto res = efi_variables_.GetGuid(u"");

  ASSERT_TRUE(res.is_error());
}

TEST_F(EfiVariablesStubTest, IterateOverEmptyList) {
  SetVariables({});

  EfiVariables efi_variables;
  EXPECT_EQ(std::distance(efi_variables.begin(), efi_variables.end()), 0UL);
  EXPECT_EQ(efi_variables.begin(), efi_variables.end());
}

TEST_F(EfiVariablesStubTest, IterateOverList) {
  SetVariables({
      {kVariableId[0], kValue[0]},
      {kVariableId[1], kValue[1]},
      {kVariableId[2], kValue[2]},
      {kVariableId[3], kValue[3]},
  });

  EfiVariables efi_variables;

  size_t i = 0;
  for (const auto& v_id : efi_variables) {
    ASSERT_TRUE(v_id.IsValid());
    EXPECT_EQ(v_id.name, efi::String(kVariableId[i].name));
    EXPECT_EQ(v_id.vendor_guid, kVariableId[i].vendor_guid);
    ++i;
  }
}

}  // unnamed namespace
