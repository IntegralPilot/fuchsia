// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/nop.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

namespace {

using ::testing::ElementsAreArray;


TEST(NopFillTests, Arm64) {
  constexpr size_t kInsnSize = 4;

  // 1 instruction.
  {
    constexpr size_t kSize = kInsnSize;
    constexpr uint8_t kExpected[kSize] = {0x1f, 0x20, 0x03, 0xd5};
    uint8_t actual[kSize];
    arch::NopFill<arch::Arm64NopTraits>(std::as_writable_bytes(std::span{actual}));
    EXPECT_THAT(actual, ElementsAreArray(kExpected));
  }

  // 2 instructions.
  {
    constexpr size_t kSize = 2 * kInsnSize;
    constexpr uint8_t kExpected[kSize] = {
        0x1f, 0x20, 0x03, 0xd5,  //
        0x1f, 0x20, 0x03, 0xd5,
    };
    uint8_t actual[kSize];
    arch::NopFill<arch::Arm64NopTraits>(std::as_writable_bytes(std::span{actual}));
    EXPECT_THAT(actual, ElementsAreArray(kExpected));
  }

  // 5 instructions.
  {
    constexpr size_t kSize = 5 * kInsnSize;
    constexpr uint8_t kExpected[kSize] = {
        0x1f, 0x20, 0x03, 0xd5,  //
        0x1f, 0x20, 0x03, 0xd5,  //
        0x1f, 0x20, 0x03, 0xd5,  //
        0x1f, 0x20, 0x03, 0xd5,  //
        0x1f, 0x20, 0x03, 0xd5,
    };
    uint8_t actual[kSize];
    arch::NopFill<arch::Arm64NopTraits>(std::as_writable_bytes(std::span{actual}));
    EXPECT_THAT(actual, ElementsAreArray(kExpected));
  }

  // 10 instructions.
  {
    constexpr size_t kSize = 10 * kInsnSize;
    constexpr uint8_t kExpected[kSize] = {
        0x1f, 0x20, 0x03, 0xd5,  //
        0x1f, 0x20, 0x03, 0xd5,  //
        0x1f, 0x20, 0x03, 0xd5,  //
        0x1f, 0x20, 0x03, 0xd5,  //
        0x1f, 0x20, 0x03, 0xd5,  //
        0x1f, 0x20, 0x03, 0xd5,  //
        0x1f, 0x20, 0x03, 0xd5,  //
        0x1f, 0x20, 0x03, 0xd5,  //
        0x1f, 0x20, 0x03, 0xd5,  //
        0x1f, 0x20, 0x03, 0xd5,
    };
    uint8_t actual[kSize];
    arch::NopFill<arch::Arm64NopTraits>(std::as_writable_bytes(std::span{actual}));
    EXPECT_THAT(actual, ElementsAreArray(kExpected));
  }
}

TEST(NopFillTests, X86) {
  constexpr size_t kInsnSize = 1;

  // 1 instruction.
  {
    constexpr size_t kSize = kInsnSize;
    constexpr uint8_t kExpected[kSize] = {0x90};
    uint8_t actual[kSize];
    arch::NopFill<arch::X86NopTraits>(std::as_writable_bytes(std::span{actual}));
    EXPECT_THAT(actual, ElementsAreArray(kExpected));
  }

  // 2 instructions.
  {
    constexpr size_t kSize = 2 * kInsnSize;
    constexpr uint8_t kExpected[kSize] = {0x66, 0x90};
    uint8_t actual[kSize];
    arch::NopFill<arch::X86NopTraits>(std::as_writable_bytes(std::span{actual}));
    EXPECT_THAT(actual, ElementsAreArray(kExpected));
  }

  // 3 instructions.
  {
    constexpr size_t kSize = 3 * kInsnSize;
    constexpr uint8_t kExpected[kSize] = {0x0f, 0x1f, 0x00};
    uint8_t actual[kSize];
    arch::NopFill<arch::X86NopTraits>(std::as_writable_bytes(std::span{actual}));
    EXPECT_THAT(actual, ElementsAreArray(kExpected));
  }

  // 4 instructions.
  {
    constexpr size_t kSize = 4 * kInsnSize;
    constexpr uint8_t kExpected[kSize] = {0x0f, 0x1f, 0x40, 0x00};
    uint8_t actual[kSize];
    arch::NopFill<arch::X86NopTraits>(std::as_writable_bytes(std::span{actual}));
    EXPECT_THAT(actual, ElementsAreArray(kExpected));
  }

  // 5 instructions.
  {
    constexpr size_t kSize = 5 * kInsnSize;
    constexpr uint8_t kExpected[kSize] = {0x0f, 0x1f, 0x44, 0x00, 0x00};
    uint8_t actual[kSize];
    arch::NopFill<arch::X86NopTraits>(std::as_writable_bytes(std::span{actual}));
    EXPECT_THAT(actual, ElementsAreArray(kExpected));
  }

  // 6 instructions.
  {
    constexpr size_t kSize = 6 * kInsnSize;
    constexpr uint8_t kExpected[kSize] = {0x66, 0x0f, 0x1f, 0x44, 0x00, 0x00};
    uint8_t actual[kSize];
    arch::NopFill<arch::X86NopTraits>(std::as_writable_bytes(std::span{actual}));
    EXPECT_THAT(actual, ElementsAreArray(kExpected));
  }

  // 7 instructions.
  {
    constexpr size_t kSize = 7 * kInsnSize;
    constexpr uint8_t kExpected[kSize] = {0x0f, 0x1f, 0x80, 0x00, 0x00, 0x00, 0x00};
    uint8_t actual[kSize];
    arch::NopFill<arch::X86NopTraits>(std::as_writable_bytes(std::span{actual}));
    EXPECT_THAT(actual, ElementsAreArray(kExpected));
  }

  // 8 instructions.
  {
    constexpr size_t kSize = 8 * kInsnSize;
    constexpr uint8_t kExpected[kSize] = {0x0f, 0x1f, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00};
    uint8_t actual[kSize];
    arch::NopFill<arch::X86NopTraits>(std::as_writable_bytes(std::span{actual}));
    EXPECT_THAT(actual, ElementsAreArray(kExpected));
  }

  // 9 instructions.
  {
    constexpr size_t kSize = 9 * kInsnSize;
    constexpr uint8_t kExpected[kSize] = {0x66, 0x0f, 0x1f, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00};
    uint8_t actual[kSize];
    arch::NopFill<arch::X86NopTraits>(std::as_writable_bytes(std::span{actual}));
    EXPECT_THAT(actual, ElementsAreArray(kExpected));
  }

  // 10 instructions.
  {
    constexpr size_t kSize = 10 * kInsnSize;
    constexpr uint8_t kExpected[kSize] = {0x66, 0x66, 0x0f, 0x1f, 0x84,
                                          0x00, 0x00, 0x00, 0x00, 0x00};
    uint8_t actual[kSize];
    arch::NopFill<arch::X86NopTraits>(std::as_writable_bytes(std::span{actual}));
    EXPECT_THAT(actual, ElementsAreArray(kExpected));
  }

  // 11 instructions.
  {
    constexpr size_t kSize = 11 * kInsnSize;
    constexpr uint8_t kExpected[kSize] = {0x66, 0x66, 0x66, 0x0f, 0x1f, 0x84,
                                          0x00, 0x00, 0x00, 0x00, 0x00};
    uint8_t actual[kSize];
    arch::NopFill<arch::X86NopTraits>(std::as_writable_bytes(std::span{actual}));
    EXPECT_THAT(actual, ElementsAreArray(kExpected));
  }

  // 12 instructions.
  {
    constexpr size_t kSize = 12 * kInsnSize;
    constexpr uint8_t kExpected[kSize] = {0x66, 0x66, 0x66, 0x66, 0x0f, 0x1f,
                                          0x84, 0x00, 0x00, 0x00, 0x00, 0x00};
    uint8_t actual[kSize];
    arch::NopFill<arch::X86NopTraits>(std::as_writable_bytes(std::span{actual}));
    EXPECT_THAT(actual, ElementsAreArray(kExpected));
  }

  // 13 instructions.
  {
    constexpr size_t kSize = 13 * kInsnSize;
    constexpr uint8_t kExpected[kSize] = {0x66, 0x66, 0x66, 0x66, 0x66, 0x0f, 0x1f,
                                          0x84, 0x00, 0x00, 0x00, 0x00, 0x00};
    uint8_t actual[kSize];
    arch::NopFill<arch::X86NopTraits>(std::as_writable_bytes(std::span{actual}));
    EXPECT_THAT(actual, ElementsAreArray(kExpected));
  }

  // 14 instructions.
  {
    constexpr size_t kSize = 14 * kInsnSize;
    constexpr uint8_t kExpected[kSize] = {0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x0f,
                                          0x1f, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00};
    uint8_t actual[kSize];
    arch::NopFill<arch::X86NopTraits>(std::as_writable_bytes(std::span{actual}));
    EXPECT_THAT(actual, ElementsAreArray(kExpected));
  }

  // 15 instructions.
  {
    constexpr size_t kSize = 15 * kInsnSize;
    constexpr uint8_t kExpected[kSize] = {0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x0f,
                                          0x1f, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00};
    uint8_t actual[kSize];
    arch::NopFill<arch::X86NopTraits>(std::as_writable_bytes(std::span{actual}));
    EXPECT_THAT(actual, ElementsAreArray(kExpected));
  }

  // 50 instructions.
  {
    constexpr size_t kSize = 50 * kInsnSize;
    constexpr uint8_t kExpected[kSize] = {
        0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x0f,
        0x1f, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00,  // Size-15 nop.
        0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x0f,
        0x1f, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00,  // Size-15 nop.
        0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x0f,
        0x1f, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00,  // Size-15 nop.
        0x0f, 0x1f, 0x44, 0x00, 0x00,              // Size-5 nop.
    };
    uint8_t actual[kSize];
    arch::NopFill<arch::X86NopTraits>(std::as_writable_bytes(std::span{actual}));
    EXPECT_THAT(actual, ElementsAreArray(kExpected));
  }
}

}  // namespace
