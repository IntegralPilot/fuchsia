// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/test.types/cpp/natural_types.h>
#include <lib/zx/event.h>
#include <zircon/fidl.h>

#include <iostream>
#include <string>
#include <utility>
#include <vector>

#include <zxtest/zxtest.h>

static std::vector<uint8_t> GetWireStructBytes() {
  // clang-format off
  std::vector<uint8_t> bytes = {
      // Wire format metadata.
      0, kFidlWireFormatMagicNumberInitial, FIDL_MESSAGE_HEADER_AT_REST_FLAGS_0_USE_VERSION_V2, 0,
      0, 0, 0, 0,
      // Payload, a single uint32_t.
      42, 0, 0, 0, 0, 0, 0, 0,
  };
  // clang-format on
  return bytes;
}

TEST(Unpersist, TooFewBytesError) {
  std::vector<uint8_t> bytes = {1, 2, 3};
  fit::result result = ::fidl::Unpersist<test_types::CopyableStruct>(cpp20::span(bytes));
  ASSERT_FALSE(result.is_ok());
  ASSERT_EQ(fidl::Reason::kDecodeError, result.error_value().reason());
}

TEST(Unpersist, TooManyBytesError) {
  for (size_t n : {1, 8}) {
    std::vector<uint8_t> bytes = GetWireStructBytes();
    for (size_t i = 0; i < n; i++) {
      bytes.push_back(0);
    }
    fit::result result = ::fidl::Unpersist<test_types::CopyableStruct>(cpp20::span(bytes));
    ASSERT_FALSE(result.is_ok(), "Should fail with %zu extra bytes", n);
    ASSERT_EQ(fidl::Reason::kDecodeError, result.error_value().reason());
  }
}

TEST(Unpersist, NaturalStruct) {
  std::vector<uint8_t> bytes = GetWireStructBytes();
  EXPECT_EQ(bytes.size(), 16U);

  fit::result result = ::fidl::Unpersist<test_types::CopyableStruct>(cpp20::span(bytes));
  ASSERT_TRUE(result.is_ok(), "Error during unpersist: %s",
              result.error_value().FormatDescription().c_str());
  test_types::CopyableStruct& obj = result.value();

  // Check decoded value.
  EXPECT_EQ(42, obj.x());
}

TEST(Persist, NaturalStruct) {
  const test_types::CopyableStruct obj{{.x = 42}};

  fit::result result = ::fidl::Persist(obj);
  ASSERT_TRUE(result.is_ok(), "Error during persist: %s",
              result.error_value().FormatDescription().c_str());

  // clang-format off
  const std::vector<uint8_t> golden_bytes = {
      // Wire format metadata.
      0, kFidlWireFormatMagicNumberInitial, FIDL_MESSAGE_HEADER_AT_REST_FLAGS_0_USE_VERSION_V2, 0,
      0, 0, 0, 0,
      // Payload, a single uint32_t.
      42, 0, 0, 0, 0, 0, 0, 0,
  };
  // clang-format on

  ASSERT_EQ(result.value().size(), golden_bytes.size());
  EXPECT_BYTES_EQ(result.value().data(), golden_bytes.data(), golden_bytes.size());
}

TEST(Unpersist, NaturalUnion) {
  // clang-format off
  const std::vector<uint8_t> bytes = {
      // Wire format metadata.
      0, kFidlWireFormatMagicNumberInitial, FIDL_MESSAGE_HEADER_AT_REST_FLAGS_0_USE_VERSION_V2, 0,
      0, 0, 0, 0,
      // Payload, a union with ordinal 1, and inlined int32.
      1, 0, 0, 0, 0, 0, 0, 0,
      42, 0, 0, 0, 0, 0, 1, 0,
  };
  // clang-format on
  EXPECT_EQ(bytes.size(), 24U);

  fit::result result = ::fidl::Unpersist<test_types::TestStrictXUnion>(cpp20::span(bytes));
  ASSERT_TRUE(result.is_ok(), "Error during unpersist: %s",
              result.error_value().FormatDescription().c_str());
  test_types::TestStrictXUnion& obj = result.value();

  // Check decoded value.
  EXPECT_TRUE(obj.primitive().has_value());
  EXPECT_EQ(42, obj.primitive().value());
}

TEST(Persist, NaturalUnion) {
  const auto obj = test_types::TestStrictXUnion::WithPrimitive(42);

  fit::result result = ::fidl::Persist(obj);
  ASSERT_TRUE(result.is_ok(), "Error during persist: %s",
              result.error_value().FormatDescription().c_str());

  // clang-format off
  const std::vector<uint8_t> golden_bytes = {
      // Wire format metadata.
      0, kFidlWireFormatMagicNumberInitial, FIDL_MESSAGE_HEADER_AT_REST_FLAGS_0_USE_VERSION_V2, 0,
      0, 0, 0, 0,
      // Payload, a union with ordinal 1, and inlined int32.
      1, 0, 0, 0, 0, 0, 0, 0,
      42, 0, 0, 0, 0, 0, 1, 0,
  };
  // clang-format on

  ASSERT_EQ(result.value().size(), golden_bytes.size());
  EXPECT_BYTES_EQ(result.value().data(), golden_bytes.data(), golden_bytes.size());
}

TEST(Unpersist, NaturalTable) {
  // clang-format off
  const std::vector<uint8_t> bytes = {
      // Wire format metadata.
      0, kFidlWireFormatMagicNumberInitial, FIDL_MESSAGE_HEADER_AT_REST_FLAGS_0_USE_VERSION_V2, 0,
      0, 0, 0, 0,
      // Payload, an empty table.
      0, 0, 0, 0, 0, 0, 0, 0,
      0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
  };
  // clang-format on
  EXPECT_EQ(bytes.size(), 24U);

  fit::result result = ::fidl::Unpersist<test_types::SampleEmptyTable>(cpp20::span(bytes));
  ASSERT_TRUE(result.is_ok(), "Error during unpersist: %s",
              result.error_value().FormatDescription().c_str());
  test_types::SampleEmptyTable& obj = result.value();

  // Check decoded value.
  EXPECT_TRUE(obj.IsEmpty());
}

TEST(Persist, NaturalTable) {
  const test_types::SampleEmptyTable obj;

  fit::result result = ::fidl::Persist(obj);
  ASSERT_TRUE(result.is_ok(), "Error during persist: %s",
              result.error_value().FormatDescription().c_str());

  // clang-format off
  const std::vector<uint8_t> golden_bytes = {
      // Wire format metadata.
      0, kFidlWireFormatMagicNumberInitial, FIDL_MESSAGE_HEADER_AT_REST_FLAGS_0_USE_VERSION_V2, 0,
      0, 0, 0, 0,
      // Payload, an empty table.
      0, 0, 0, 0, 0, 0, 0, 0,
      0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
  };
  // clang-format on

  ASSERT_EQ(result.value().size(), golden_bytes.size());
  EXPECT_BYTES_EQ(result.value().data(), golden_bytes.data(), golden_bytes.size());
}
