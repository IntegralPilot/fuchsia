// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <assert.h>
#include <stdio.h>

#include <climits>
#include <iterator>
#include <limits>
#include <utility>

#include <hwreg/bitfields.h>
#include <hwreg/mmio.h>
#include <hwreg/mock.h>
#include <hwreg/pio.h>
#include <zxtest/zxtest.h>

class CompilationTestReg32 : public hwreg::RegisterBase<CompilationTestReg32, uint32_t> {
 public:
  static hwreg::RegisterAddr<CompilationTestReg32> Get() { return {0}; }

  DEF_FIELD(30, 12, field1);
  DEF_BIT(11, field2);
  DEF_RSVDZ_FIELD(10, 5);
  DEF_FIELD(4, 3, field3);
  DEF_RSVDZ_BIT(2);
  DEF_RSVDZ_BIT(1);
  DEF_FIELD(0, 0, field4);
};

// This function exists so that the resulting code can be inspected easily in the
// object file.
void compilation_test() {
  volatile uint32_t fake_reg = 1ul << 31;
  hwreg::RegisterMmio mmio(&fake_reg);

  auto reg = CompilationTestReg32::Get().ReadFrom(&mmio);
  reg.set_field1(0x31234);
  reg.set_field2(1);
  reg.set_field3(2);
  reg.set_field4(0);
  reg.WriteTo(&mmio);
}

namespace {

template <typename IntType>
struct LastBit {
  static constexpr const unsigned int value = sizeof(IntType) * CHAR_BIT - 1;
};

template <typename IntType>
struct StructSubBitTestReg {
  IntType field;

  DEF_SUBBIT(field, 0, first_bit);
  DEF_SUBBIT(field, 1, mid_bit);
  DEF_SUBBIT(field, LastBit<IntType>::value, last_bit);
};

template <typename IntType>
void struct_sub_bit_test() {
  StructSubBitTestReg<IntType> val = {};
  EXPECT_EQ(0u, val.first_bit());
  EXPECT_EQ(0u, val.mid_bit());
  EXPECT_EQ(0u, val.last_bit());

  val.set_first_bit(1);
  EXPECT_EQ(1u, val.field);
  EXPECT_EQ(1u, val.first_bit());
  EXPECT_EQ(0u, val.mid_bit());
  EXPECT_EQ(0u, val.last_bit());
  val.set_first_bit(0);

  val.set_mid_bit(1);
  EXPECT_EQ(2u, val.field);
  EXPECT_EQ(0u, val.first_bit());
  EXPECT_EQ(1u, val.mid_bit());
  EXPECT_EQ(0u, val.last_bit());
  val.set_mid_bit(0);

  val.set_last_bit(1);
  EXPECT_EQ(1ull << LastBit<IntType>::value, val.field);
  EXPECT_EQ(0u, val.first_bit());
  EXPECT_EQ(0u, val.mid_bit());
  EXPECT_EQ(1u, val.last_bit());
  val.set_last_bit(0);
}

TEST(StructSubBitTestCase, Uint8) { ASSERT_NO_FAILURES(struct_sub_bit_test<uint8_t>()); }
TEST(StructSubBitTestCase, Uint16) { ASSERT_NO_FAILURES(struct_sub_bit_test<uint16_t>()); }
TEST(StructSubBitTestCase, Uint32) { ASSERT_NO_FAILURES(struct_sub_bit_test<uint32_t>()); }
TEST(StructSubBitTestCase, Uint64) { ASSERT_NO_FAILURES(struct_sub_bit_test<uint64_t>()); }

template <typename IntType>
struct StructSubFieldTestReg {
  IntType field1;
  DEF_SUBFIELD(field1, LastBit<IntType>::value, 0, whole_length);

  IntType field2;
  DEF_SUBFIELD(field2, 2, 2, single_bit);

  IntType field3;
  DEF_SUBFIELD(field3, 2, 1, range1);
  DEF_SUBFIELD(field3, 5, 3, range2);
};

template <typename IntType>
void struct_sub_field_test() {
  StructSubFieldTestReg<IntType> val = {};

  // Ensure writing to a whole length field affects all bits
  constexpr IntType kMax = std::numeric_limits<IntType>::max();
  EXPECT_EQ(0u, val.whole_length());
  val.set_whole_length(kMax);
  EXPECT_EQ(kMax, val.whole_length());
  EXPECT_EQ(kMax, val.field1);
  val.set_whole_length(0);
  EXPECT_EQ(0, val.whole_length());
  EXPECT_EQ(0, val.field1);

  // Ensure writing to a single bit only affects that bit
  EXPECT_EQ(0u, val.single_bit());
  val.set_single_bit(1);
  EXPECT_EQ(1u, val.single_bit());
  EXPECT_EQ(4u, val.field2);
  val.set_single_bit(0);
  EXPECT_EQ(0u, val.single_bit());
  EXPECT_EQ(0u, val.field2);

  // Ensure writing to adjacent fields does not bleed across
  EXPECT_EQ(0u, val.range1());
  EXPECT_EQ(0u, val.range2());
  val.set_range1(3);
  EXPECT_EQ(3u, val.range1());
  EXPECT_EQ(0u, val.range2());
  EXPECT_EQ(3u << 1, val.field3);
  val.set_range2(1);
  EXPECT_EQ(3u, val.range1());
  EXPECT_EQ(1u, val.range2());
  EXPECT_EQ((3u << 1) | (1u << 3), val.field3);
  val.set_range2(2);
  EXPECT_EQ(3u, val.range1());
  EXPECT_EQ(2u, val.range2());
  EXPECT_EQ((3u << 1) | (2u << 3), val.field3);
  val.set_range1(0);
  EXPECT_EQ(0u, val.range1());
  EXPECT_EQ(2u, val.range2());
  EXPECT_EQ((2u << 3), val.field3);
}

TEST(StructSubFieldTestCase, Uint8) { ASSERT_NO_FAILURES(struct_sub_field_test<uint8_t>()); }
TEST(StructSubFieldTestCase, Uint16) { ASSERT_NO_FAILURES(struct_sub_field_test<uint16_t>()); }
TEST(StructSubFieldTestCase, Uint32) { ASSERT_NO_FAILURES(struct_sub_field_test<uint32_t>()); }
TEST(StructSubFieldTestCase, Uint64) { ASSERT_NO_FAILURES(struct_sub_field_test<uint64_t>()); }

template <typename IntType>
struct StructEnumSubFieldTestReg {
  enum class EnumWholeRange : IntType {
    kZero = 0,
    kOne = 1,
    kMax = std::numeric_limits<IntType>::max(),
  };
  enum class EnumBit : uint8_t {
    kZero = 0,
    kOne = 1,
  };
  enum class EnumRange : uint64_t {
    kZero = 0,
    kOne = 1,
    kTwo = 2,
    kThree = 3,
  };

  IntType field1;
  DEF_ENUM_SUBFIELD(field1, EnumWholeRange, LastBit<IntType>::value, 0, whole_length);

  IntType field2;
  DEF_ENUM_SUBFIELD(field2, EnumBit, 2, 2, single_bit);

  IntType field3;
  DEF_ENUM_SUBFIELD(field3, EnumRange, 2, 1, range1);
  DEF_ENUM_SUBFIELD(field3, EnumRange, 5, 3, range2);
};

template <typename IntType>
void struct_enum_sub_field_test() {
  using Reg = StructEnumSubFieldTestReg<IntType>;
  using EnumWholeRange = typename Reg::EnumWholeRange;
  using EnumRange = typename Reg::EnumRange;
  using EnumBit = typename Reg::EnumBit;

  Reg val = {};

  // Ensure writing to a whole length field affects all bits
  constexpr IntType kMax = std::numeric_limits<IntType>::max();
  EXPECT_EQ(EnumWholeRange::kZero, val.whole_length());
  val.set_whole_length(EnumWholeRange::kMax);
  EXPECT_EQ(EnumWholeRange::kMax, val.whole_length());
  EXPECT_EQ(kMax, val.field1);
  val.set_whole_length(EnumWholeRange::kZero);
  EXPECT_EQ(EnumWholeRange::kZero, val.whole_length());
  EXPECT_EQ(0, val.field1);

  // Ensure writing to a single bit only affects that bit
  EXPECT_EQ(EnumBit::kZero, val.single_bit());
  val.set_single_bit(EnumBit::kOne);
  EXPECT_EQ(EnumBit::kOne, val.single_bit());
  EXPECT_EQ(4u, val.field2);
  val.set_single_bit(EnumBit::kZero);
  EXPECT_EQ(EnumBit::kZero, val.single_bit());
  EXPECT_EQ(0u, val.field2);

  // Ensure writing to adjacent fields does not bleed across
  EXPECT_EQ(EnumRange::kZero, val.range1());
  EXPECT_EQ(EnumRange::kZero, val.range2());
  val.set_range1(EnumRange::kThree);
  EXPECT_EQ(EnumRange::kThree, val.range1());
  EXPECT_EQ(EnumRange::kZero, val.range2());
  EXPECT_EQ(3u << 1, val.field3);
  val.set_range2(EnumRange::kOne);
  EXPECT_EQ(EnumRange::kThree, val.range1());
  EXPECT_EQ(EnumRange::kOne, val.range2());
  EXPECT_EQ((3u << 1) | (1u << 3), val.field3);
  val.set_range2(EnumRange::kTwo);
  EXPECT_EQ(EnumRange::kThree, val.range1());
  EXPECT_EQ(EnumRange::kTwo, val.range2());
  EXPECT_EQ((3u << 1) | (2u << 3), val.field3);
  val.set_range1(EnumRange::kZero);
  EXPECT_EQ(EnumRange::kZero, val.range1());
  EXPECT_EQ(EnumRange::kTwo, val.range2());
  EXPECT_EQ((2u << 3), val.field3);
}

TEST(StructEnumSubFieldTestCase, Uint8) {
  ASSERT_NO_FAILURES(struct_enum_sub_field_test<uint8_t>());
}
TEST(StructEnumSubFieldTestCase, Uint16) {
  ASSERT_NO_FAILURES(struct_enum_sub_field_test<uint16_t>());
}
TEST(StructEnumSubFieldTestCase, Uint32) {
  ASSERT_NO_FAILURES(struct_enum_sub_field_test<uint32_t>());
}
TEST(StructEnumSubFieldTestCase, Uint64) {
  ASSERT_NO_FAILURES(struct_enum_sub_field_test<uint64_t>());
}

enum class ConditionalSubfieldTestRegEnum {
  kA,
  kB,
  kC,
};

template <ConditionalSubfieldTestRegEnum Condition>
struct ConditionalSubfieldTestReg {
  using SelfType = ConditionalSubfieldTestReg<Condition>;

  static constexpr bool kA = Condition == ConditionalSubfieldTestRegEnum::kA;
  static constexpr bool kB = Condition == ConditionalSubfieldTestRegEnum::kB;
  static constexpr bool kC = Condition == ConditionalSubfieldTestRegEnum::kC;

  uint16_t field;

  // Conditional kA fields.
  DEF_COND_SUBFIELD(field, 15, 8, same, kA);
  DEF_COND_SUBBIT(field, 7, a_7, kA);
  DEF_COND_SUBFIELD(field, 6, 4, a_6_4, kA);
  DEF_COND_SUBBIT(field, 3, a_3, kA);

  // Conditional kB fields.
  DEF_COND_SUBFIELD(field, 12, 10, same, kB);
  DEF_COND_UNSHIFTED_SUBFIELD(field, 7, 5, b_7_5, kB);
  DEF_COND_SUBFIELD(field, 4, 3, b_4_3, kB);

  // Conditional kC fields.
  enum class Rsvp { kNo = 0, kYes = 0b1111 };
  DEF_COND_SUBFIELD(field, 13, 12, same, kC);
  DEF_COND_ENUM_SUBFIELD(field, Rsvp, 7, 4, c_7_4, kC);
  DEF_COND_SUBBIT(field, 3, c_3, kC);

  // Unconditional, common field.
  DEF_SUBFIELD(field, 2, 0, common);
};

TEST(StructConditionalSubfieldsTestCase, ConditionalSubfields) {
  using Enum = ConditionalSubfieldTestRegEnum;
  {
    using RegA = ConditionalSubfieldTestReg<Enum::kA>;

    RegA reg{.field = 0xffff};
    EXPECT_EQ(0xff, reg.same());
    reg.set_same(0);
    EXPECT_EQ(0b1, reg.a_7());
    reg.set_a_7(0);
    EXPECT_EQ(0b111, reg.a_6_4());
    reg.set_a_6_4(0);
    EXPECT_EQ(0b1, reg.a_3());
    reg.set_a_3(0);
    EXPECT_EQ(0b111, reg.common());
    reg.set_common(0);
    EXPECT_EQ(0, reg.field);
  }

  {
    using RegB = ConditionalSubfieldTestReg<Enum::kB>;

    RegB reg{.field = 0xffff};
    EXPECT_EQ(0b111, reg.same());
    reg.set_same(0);
    EXPECT_EQ(0b11100000, reg.b_7_5());
    reg.set_b_7_5(0);
    EXPECT_EQ(0b11, reg.b_4_3());
    reg.set_b_4_3(0);
    EXPECT_EQ(0b111, reg.common());
    reg.set_common(0);
    EXPECT_EQ(0xe300, reg.field);
  }

  {
    using RegC = ConditionalSubfieldTestReg<Enum::kC>;
    using Rsvp = typename RegC::Rsvp;

    RegC reg{.field = 0xffff};
    EXPECT_EQ(0b11, reg.same());
    reg.set_same(0);
    EXPECT_EQ(Rsvp::kYes, reg.c_7_4());
    reg.set_c_7_4(Rsvp::kNo);
    EXPECT_EQ(0b1, reg.c_3());
    reg.set_c_3(0);
    EXPECT_EQ(0b111, reg.common());
    reg.set_common(0);
    EXPECT_EQ(0xcf00, reg.field);
  }
}

// This definition amounts to a compilation test.
template <bool Condition>
struct ConditionalSubfieldsWithSameNameTestReg {
  using SelfType = ConditionalSubfieldsWithSameNameTestReg<Condition>;

  uint16_t field;

  enum class Enum { kA = 0b00, kB = 0b11 };

  DEF_COND_SUBBIT(field, 15, a, Condition);
  DEF_COND_SUBFIELD(field, 14, 12, b, Condition);
  DEF_COND_UNSHIFTED_SUBFIELD(field, 11, 10, c, Condition);
  DEF_COND_ENUM_SUBFIELD(field, Enum, 9, 8, d, Condition);

  DEF_COND_ENUM_SUBFIELD(field, Enum, 7, 6, d, !Condition);
  DEF_COND_UNSHIFTED_SUBFIELD(field, 5, 4, c, !Condition);
  DEF_COND_SUBFIELD(field, 3, 1, b, !Condition);
  DEF_COND_SUBBIT(field, 0, a, !Condition);
};

struct UnshiftedFieldTestReg {
  uint16_t data;

  DEF_UNSHIFTED_SUBFIELD(data, 15, 12, field1);
  DEF_UNSHIFTED_SUBFIELD(data, 11, 8, field2);
  DEF_UNSHIFTED_SUBFIELD(data, 7, 4, field3);
  DEF_UNSHIFTED_SUBFIELD(data, 3, 0, field4);
};

TEST(SubFieldTestCase, UnshifedFields) {
  // Tests simple field isolation
  {
    auto test_reg = UnshiftedFieldTestReg{.data = 0xffff};
    EXPECT_EQ(0xf000, test_reg.field1());
    EXPECT_EQ(0x0f00, test_reg.field2());
    EXPECT_EQ(0x00f0, test_reg.field3());
    EXPECT_EQ(0x000f, test_reg.field4());
  }

  // Test assignment
  {
    auto test_reg = UnshiftedFieldTestReg{.data = 0x0};
    EXPECT_EQ(test_reg.field1(), 0u);
    EXPECT_EQ(test_reg.field2(), 0u);
    EXPECT_EQ(test_reg.field3(), 0u);
    EXPECT_EQ(test_reg.field4(), 0u);

    test_reg.set_field1(0xf000);
    EXPECT_EQ(test_reg.field1(), 0xf000);
    EXPECT_EQ(test_reg.field2(), 0u);
    EXPECT_EQ(test_reg.field3(), 0u);
    EXPECT_EQ(test_reg.field4(), 0u);

    test_reg.set_field2(0xf00);
    EXPECT_EQ(test_reg.field1(), 0xf000);
    EXPECT_EQ(test_reg.field2(), 0xf00);
    EXPECT_EQ(test_reg.field3(), 0u);
    EXPECT_EQ(test_reg.field4(), 0u);

    test_reg.set_field3(0xf0);
    EXPECT_EQ(test_reg.field1(), 0xf000);
    EXPECT_EQ(test_reg.field2(), 0xf00);
    EXPECT_EQ(test_reg.field3(), 0xf0);
    EXPECT_EQ(test_reg.field4(), 0u);

    test_reg.set_field4(0xf);
    EXPECT_EQ(test_reg.field1(), 0xf000);
    EXPECT_EQ(test_reg.field2(), 0xf00);
    EXPECT_EQ(test_reg.field3(), 0xf0);
    EXPECT_EQ(test_reg.field4(), 0xf);
  }
}

class RsvdZPartialTestReg8 : public hwreg::RegisterBase<RsvdZPartialTestReg8, uint8_t> {
 public:
  static hwreg::RegisterAddr<RsvdZPartialTestReg8> Get() { return {0}; }

  DEF_RSVDZ_FIELD(7, 3);
};
class RsvdZPartialTestReg16 : public hwreg::RegisterBase<RsvdZPartialTestReg16, uint16_t> {
 public:
  static hwreg::RegisterAddr<RsvdZPartialTestReg16> Get() { return {0}; }

  DEF_RSVDZ_FIELD(14, 1);
};
class RsvdZPartialTestReg32 : public hwreg::RegisterBase<RsvdZPartialTestReg32, uint32_t> {
 public:
  static hwreg::RegisterAddr<RsvdZPartialTestReg32> Get() { return {0}; }

  DEF_RSVDZ_FIELD(31, 12);
  DEF_RSVDZ_FIELD(10, 5);
  DEF_RSVDZ_BIT(3);
};
class RsvdZPartialTestReg64 : public hwreg::RegisterBase<RsvdZPartialTestReg64, uint64_t> {
 public:
  static hwreg::RegisterAddr<RsvdZPartialTestReg64> Get() { return {0}; }

  DEF_RSVDZ_FIELD(63, 18);
  DEF_RSVDZ_FIELD(10, 0);
};

TEST(RegisterTestCase, RsvdzPartial) {
  volatile uint64_t fake_reg;
  hwreg::RegisterMmio mmio(&fake_reg);

  // Ensure we mask off the RsvdZ bits when we write them back, regardless of
  // what we read them as.
  {
    constexpr auto allones = std::numeric_limits<uint8_t>::max();
    hwreg::Mock mock;
    mock.ExpectRead(allones, 0).ExpectWrite(uint8_t{0x7}, 0);
    auto reg = RsvdZPartialTestReg8::Get().ReadFrom(mock.io());
    EXPECT_EQ(allones, reg.reg_value());
    reg.WriteTo(mock.io());
    mock.VerifyAndClear();
  }
  {
    fake_reg = std::numeric_limits<uint16_t>::max();
    auto reg = RsvdZPartialTestReg16::Get().ReadFrom(&mmio);
    EXPECT_EQ(std::numeric_limits<uint16_t>::max(), reg.reg_value());
    reg.WriteTo(&mmio);
    uint64_t reg_value = fake_reg;
    EXPECT_EQ(0x8001u, reg_value);
  }
  {
    fake_reg = std::numeric_limits<uint32_t>::max();
    auto reg = RsvdZPartialTestReg32::Get().ReadFrom(&mmio);
    EXPECT_EQ(std::numeric_limits<uint32_t>::max(), reg.reg_value());
    reg.WriteTo(&mmio);
    uint64_t reg_value = fake_reg;
    EXPECT_EQ((1ull << 11) | 0x17ull, reg_value);
  }
  {
    fake_reg = std::numeric_limits<uint64_t>::max();
    auto reg = RsvdZPartialTestReg64::Get().ReadFrom(&mmio);
    EXPECT_EQ(std::numeric_limits<uint64_t>::max(), reg.reg_value());
    reg.WriteTo(&mmio);
    uint64_t reg_value = fake_reg;
    EXPECT_EQ(0x7full << 11, reg_value);
  }
}

class RsvdZFullTestReg8 : public hwreg::RegisterBase<RsvdZFullTestReg8, uint8_t> {
 public:
  static hwreg::RegisterAddr<RsvdZFullTestReg8> Get() { return {0}; }

  DEF_RSVDZ_FIELD(7, 0);
};
class RsvdZFullTestReg16 : public hwreg::RegisterBase<RsvdZFullTestReg16, uint16_t> {
 public:
  static hwreg::RegisterAddr<RsvdZFullTestReg16> Get() { return {0}; }

  DEF_RSVDZ_FIELD(15, 0);
};
class RsvdZFullTestReg32 : public hwreg::RegisterBase<RsvdZFullTestReg32, uint32_t> {
 public:
  static hwreg::RegisterAddr<RsvdZFullTestReg32> Get() { return {0}; }

  DEF_RSVDZ_FIELD(31, 0);
};
class RsvdZFullTestReg64 : public hwreg::RegisterBase<RsvdZFullTestReg64, uint64_t> {
 public:
  static hwreg::RegisterAddr<RsvdZFullTestReg64> Get() { return {0}; }

  DEF_RSVDZ_FIELD(63, 0);
};

TEST(RegisterTestCase, RsvdzFull) {
  volatile uint64_t fake_reg;
  hwreg::RegisterMmio mmio(&fake_reg);

  // Ensure we mask off the RsvdZ bits when we write them back, regardless of
  // what we read them as.
  {
    fake_reg = std::numeric_limits<uint8_t>::max();
    auto reg = RsvdZFullTestReg8::Get().ReadFrom(&mmio);
    EXPECT_EQ(std::numeric_limits<uint8_t>::max(), reg.reg_value());
    reg.WriteTo(&mmio);
    uint64_t reg_value = fake_reg;
    EXPECT_EQ(0u, reg_value);
  }
  {
    fake_reg = std::numeric_limits<uint16_t>::max();
    auto reg = RsvdZFullTestReg16::Get().ReadFrom(&mmio);
    EXPECT_EQ(std::numeric_limits<uint16_t>::max(), reg.reg_value());
    reg.WriteTo(&mmio);
    uint64_t reg_value = fake_reg;
    EXPECT_EQ(0u, reg_value);
  }
  {
    fake_reg = std::numeric_limits<uint32_t>::max();
    auto reg = RsvdZFullTestReg32::Get().ReadFrom(&mmio);
    EXPECT_EQ(std::numeric_limits<uint32_t>::max(), reg.reg_value());
    reg.WriteTo(&mmio);
    uint64_t reg_value = fake_reg;
    EXPECT_EQ(0u, reg_value);
  }
  {
    fake_reg = std::numeric_limits<uint64_t>::max();
    auto reg = RsvdZFullTestReg64::Get().ReadFrom(&mmio);
    EXPECT_EQ(std::numeric_limits<uint64_t>::max(), reg.reg_value());
    reg.WriteTo(&mmio);
    uint64_t reg_value = fake_reg;
    EXPECT_EQ(0u, reg_value);
  }
}

class FieldTestReg8 : public hwreg::RegisterBase<FieldTestReg8, uint8_t> {
 public:
  static hwreg::RegisterAddr<FieldTestReg8> Get() { return {0}; }

  DEF_FIELD(7, 3, field1);
  DEF_FIELD(2, 0, field2);
};
class FieldTestReg16 : public hwreg::RegisterBase<FieldTestReg16, uint16_t> {
 public:
  static hwreg::RegisterAddr<FieldTestReg16> Get() { return {0}; }

  DEF_FIELD(13, 3, field1);
  DEF_FIELD(2, 1, field2);
  DEF_BIT(0, field3);
};
class FieldTestReg32 : public hwreg::RegisterBase<FieldTestReg32, uint32_t> {
 public:
  static hwreg::RegisterAddr<FieldTestReg32> Get() { return {0}; }

  DEF_FIELD(30, 21, field1);
  DEF_FIELD(20, 12, field2);
  DEF_RSVDZ_FIELD(11, 0);
};
class FieldTestReg64 : public hwreg::RegisterBase<FieldTestReg64, uint64_t> {
 public:
  static hwreg::RegisterAddr<FieldTestReg64> Get() { return {0}; }

  DEF_FIELD(60, 20, field1);
  DEF_FIELD(10, 0, field2);
};

TEST(RegisterTestCase, Field) {
  volatile uint64_t fake_reg;
  hwreg::RegisterMmio mmio(&fake_reg);

  // Ensure modified fields go to the right place, and unspecified bits are
  // preserved.
  {
    constexpr uint8_t kInitVal = 0x42u;
    fake_reg = kInitVal;
    auto reg = FieldTestReg8::Get().ReadFrom(&mmio);
    EXPECT_EQ(kInitVal, reg.reg_value());
    EXPECT_EQ(kInitVal >> 3, reg.field1());
    EXPECT_EQ(0x2u, reg.field2());
    reg.set_field1(0x1fu);
    reg.set_field2(0x1u);
    EXPECT_EQ(0x1fu, reg.field1());
    EXPECT_EQ(0x1u, reg.field2());

    reg.WriteTo(&mmio);
    uint64_t reg_value = fake_reg;
    EXPECT_EQ((0x1fu << 3) | 1, reg_value);
  }
  {
    constexpr uint16_t kInitVal = 0b1010'1111'0101'0000u;
    fake_reg = kInitVal;
    auto reg = FieldTestReg16::Get().ReadFrom(&mmio);
    EXPECT_EQ(kInitVal, reg.reg_value());
    EXPECT_EQ((kInitVal >> 3) & ((1u << 11) - 1), reg.field1());
    EXPECT_EQ((kInitVal >> 1) & 0x3u, reg.field2());
    EXPECT_EQ(kInitVal & 1u, reg.field3());
    reg.set_field1(42);
    reg.set_field2(2);
    reg.set_field3(1);
    EXPECT_EQ(42u, reg.field1());
    EXPECT_EQ(2u, reg.field2());
    EXPECT_EQ(1u, reg.field3());
    reg.WriteTo(&mmio);
    uint64_t reg_value = fake_reg;
    EXPECT_EQ((0b10u << 14) | (42u << 3) | (2u << 1) | 1u, reg_value);
  }
  {
    constexpr uint32_t kInitVal = 0xe987'2fffu;
    fake_reg = kInitVal;
    auto reg = FieldTestReg32::Get().ReadFrom(&mmio);
    EXPECT_EQ(kInitVal, reg.reg_value());
    EXPECT_EQ((kInitVal >> 21) & ((1u << 10) - 1), reg.field1());
    EXPECT_EQ((kInitVal >> 12) & ((1u << 9) - 1), reg.field2());
    reg.set_field1(0x3a7);
    reg.set_field2(0x8f);
    EXPECT_EQ(0x3a7u, reg.field1());
    EXPECT_EQ(0x8fu, reg.field2());
    reg.WriteTo(&mmio);
    uint64_t reg_value = fake_reg;
    EXPECT_EQ((0b1u << 31) | (0x3a7u << 21) | (0x8fu << 12), reg_value);
  }
  {
    constexpr uint64_t kInitVal = 0xfedc'ba98'7654'3210ull;
    fake_reg = kInitVal;
    auto reg = FieldTestReg64::Get().ReadFrom(&mmio);
    EXPECT_EQ(kInitVal, reg.reg_value());
    EXPECT_EQ((kInitVal >> 20) & ((1ull << 41) - 1), reg.field1());
    EXPECT_EQ(kInitVal & ((1ull << 11) - 1), reg.field2());
    reg.set_field1(0x1a2'3456'789aull);
    reg.set_field2(0x78c);
    EXPECT_EQ(0x1a2'3456'789aull, reg.field1());
    EXPECT_EQ(0x78cu, reg.field2());
    reg.WriteTo(&mmio);
    uint64_t reg_value = fake_reg;
    EXPECT_EQ((0b111ull << 61) | (0x1a2'3456'789aull << 20) | (0x86ull << 11) | 0x78cu, reg_value);
  }
}

class EnumFieldTestReg8 : public hwreg::RegisterBase<EnumFieldTestReg8, uint8_t> {
 public:
  enum MyEnum { Test0 = 0, Test1 = 1, Test2 = 2, Test3 = 3 };
  static hwreg::RegisterAddr<EnumFieldTestReg8> Get() { return {0}; }

  DEF_ENUM_FIELD(MyEnum, 3, 2, TestField);
};
class EnumFieldTestReg8WithEnumClass
    : public hwreg::RegisterBase<EnumFieldTestReg8WithEnumClass, uint8_t> {
 public:
  enum class MyEnum { Test0 = 0, Test1 = 1, Test2 = 2, Test3 = 3 };
  static hwreg::RegisterAddr<EnumFieldTestReg8WithEnumClass> Get() { return {0}; }

  DEF_ENUM_FIELD(MyEnum, 3, 2, TestField);
};

TEST(RegisterTestCase, EnumField) {
  {
    uint8_t result = []() {
      EnumFieldTestReg8WithEnumClass reg = EnumFieldTestReg8WithEnumClass::Get().FromValue(255);
      reg.set_TestField(EnumFieldTestReg8WithEnumClass::MyEnum::Test0);
      return reg.reg_value();
    }();
    int mask = 0xF3;
    ASSERT_TRUE(result == mask);
    ASSERT_TRUE(EnumFieldTestReg8WithEnumClass::Get().FromValue(result).TestField() ==
                EnumFieldTestReg8WithEnumClass::MyEnum::Test0);
  }
  {
    uint8_t result = []() {
      EnumFieldTestReg8 reg = EnumFieldTestReg8::Get().FromValue(255);
      reg.set_TestField(EnumFieldTestReg8::Test1);
      return reg.reg_value();
    }();
    int mask = 0xF3;
    ASSERT_TRUE(result == (mask | (1 << 2)));
    ASSERT_TRUE(EnumFieldTestReg8::Get().FromValue(result).TestField() == EnumFieldTestReg8::Test1);
  }
  {
    uint8_t result = []() {
      EnumFieldTestReg8 reg = EnumFieldTestReg8::Get().FromValue(255);
      reg.set_TestField(EnumFieldTestReg8::Test2);
      return reg.reg_value();
    }();
    int mask = 0xF3;
    ASSERT_TRUE(result == (mask | (2 << 2)));
    ASSERT_TRUE(EnumFieldTestReg8::Get().FromValue(result).TestField() == EnumFieldTestReg8::Test2);
  }
  {
    uint8_t result = []() {
      EnumFieldTestReg8 reg = EnumFieldTestReg8::Get().FromValue(255);
      reg.set_TestField(EnumFieldTestReg8::Test3);
      return reg.reg_value();
    }();
    constexpr int mask = 0xF3;
    ASSERT_TRUE(result == (mask | (3 << 2)));
    ASSERT_TRUE(EnumFieldTestReg8::Get().FromValue(result).TestField() == EnumFieldTestReg8::Test3);
  }
}

class UnshiftedTestReg16 : public hwreg::RegisterBase<UnshiftedTestReg16, uint16_t> {
 public:
  static hwreg::RegisterAddr<UnshiftedTestReg16> Get() { return {0}; }

  DEF_UNSHIFTED_FIELD(15, 12, field1);
  DEF_UNSHIFTED_FIELD(11, 8, field2);
  DEF_UNSHIFTED_FIELD(7, 4, field3);
  DEF_UNSHIFTED_FIELD(3, 0, field4);
};

class TestPciBar32 : public hwreg::RegisterBase<TestPciBar32, uint32_t> {
 public:
  static hwreg::RegisterAddr<TestPciBar32> Get() { return {0}; }

  DEF_UNSHIFTED_FIELD(31, 4, address);
  DEF_BIT(3, is_prefetchable);
  DEF_RSVDZ_BIT(2);
  DEF_BIT(1, is_64bit);
  DEF_BIT(0, is_io_space);
};

TEST(RegisterTestCase, UnshiftedField) {
  // Tests simple field isolation
  {
    volatile uint16_t fake_reg = 0xffff;
    auto test_reg = UnshiftedTestReg16::Get().FromValue(fake_reg);
    EXPECT_EQ(0xf000, test_reg.field1());
    EXPECT_EQ(0x0f00, test_reg.field2());
    EXPECT_EQ(0x00f0, test_reg.field3());
    EXPECT_EQ(0x000f, test_reg.field4());
  }

  // Test assignment
  {
    volatile uint16_t fake_reg = 0x0000;
    auto test_reg = UnshiftedTestReg16::Get().FromValue(fake_reg);
    EXPECT_EQ(test_reg.field1(), 0u);
    EXPECT_EQ(test_reg.field2(), 0u);
    EXPECT_EQ(test_reg.field3(), 0u);
    EXPECT_EQ(test_reg.field4(), 0u);

    test_reg.set_field1(0xf000);
    EXPECT_EQ(test_reg.field1(), 0xf000);
    EXPECT_EQ(test_reg.field2(), 0u);
    EXPECT_EQ(test_reg.field3(), 0u);
    EXPECT_EQ(test_reg.field4(), 0u);

    test_reg.set_field2(0xf00);
    EXPECT_EQ(test_reg.field1(), 0xf000);
    EXPECT_EQ(test_reg.field2(), 0xf00);
    EXPECT_EQ(test_reg.field3(), 0u);
    EXPECT_EQ(test_reg.field4(), 0u);

    test_reg.set_field3(0xf0);
    EXPECT_EQ(test_reg.field1(), 0xf000);
    EXPECT_EQ(test_reg.field2(), 0xf00);
    EXPECT_EQ(test_reg.field3(), 0xf0);
    EXPECT_EQ(test_reg.field4(), 0u);

    test_reg.set_field4(0xf);
    EXPECT_EQ(test_reg.field1(), 0xf000);
    EXPECT_EQ(test_reg.field2(), 0xf00);
    EXPECT_EQ(test_reg.field3(), 0xf0);
    EXPECT_EQ(test_reg.field4(), 0xf);
  }

  // Test Writing a Bar size to an address field ala PCI
  {
    volatile uint32_t fake_reg = 1 << 20;  // A 1 MB size BARr
    auto test_reg = TestPciBar32::Get().FromValue(fake_reg);

    EXPECT_EQ(test_reg.address(), 1 << 20);
    test_reg.set_is_prefetchable(1);
    test_reg.set_is_64bit(1);
    test_reg.set_is_io_space(1);
    EXPECT_EQ(test_reg.address(), 1 << 20);
    EXPECT_EQ(test_reg.is_prefetchable(), 1);
    EXPECT_EQ(test_reg.is_64bit(), 1);
    EXPECT_EQ(test_reg.is_io_space(), 1);
  }
}

struct ConstexprArithmeticTestReg
    : public hwreg::RegisterBase<ConstexprArithmeticTestReg, uint32_t> {
  static constexpr unsigned int kTen = 10;

  static hwreg::RegisterAddr<ConstexprArithmeticTestReg> Get() { return {0}; }

  DEF_FIELD(kTen + 2 * kTen, kTen, field2);
  DEF_RSVDZ_FIELD(kTen - 1, 2);
  DEF_BIT(2 + 3 - 4, field1);
};

TEST(RegisterTestCase, BitsAsConstexprArithmeticExpressions) {
  volatile uint32_t fake_reg = 1ul << 31;
  hwreg::RegisterMmio mmio(&fake_reg);

  auto reg = ConstexprArithmeticTestReg::Get().ReadFrom(&mmio);
  reg.set_field1(1);
  reg.set_field2(0xabcd);
  reg.WriteTo(&mmio);
}

enum class ConditionalFieldTestRegEnum {
  kA,
  kB,
  kC,
};

template <ConditionalFieldTestRegEnum Condition>
struct ConditionalFieldTestReg
    : public hwreg::RegisterBase<ConditionalFieldTestReg<Condition>, uint8_t> {
  using SelfType = ConditionalFieldTestReg<Condition>;

  static constexpr bool kA = Condition == ConditionalFieldTestRegEnum::kA;
  static constexpr bool kB = Condition == ConditionalFieldTestRegEnum::kB;
  static constexpr bool kC = Condition == ConditionalFieldTestRegEnum::kC;

  static hwreg::RegisterAddr<SelfType> Get() { return {0}; }

  // Conditional kA fields.
  DEF_COND_BIT(7, a_7, kA);
  DEF_COND_FIELD(6, 4, a_6_4, kA);
  DEF_COND_RSVDZ_BIT(3, kA);

  // Conditional kB fields.
  DEF_COND_UNSHIFTED_FIELD(7, 5, b_7_5, kB);
  DEF_COND_RSVDZ_FIELD(4, 3, kB);

  // Conditional kC fields.
  enum class Rsvp { kNo = 0, kYes = 0b1111 };
  DEF_COND_ENUM_FIELD(Rsvp, 7, 4, c_7_4, kC);
  DEF_COND_BIT(3, c_3, kC);

  // Unconditional, common field.
  DEF_FIELD(2, 0, common);
};

// This definition amounts to a compilation test.
template <bool Condition>
struct ConditionalFieldsWithSameNameTestReg
    : hwreg::RegisterBase<ConditionalFieldsWithSameNameTestReg<Condition>, uint16_t,
                          hwreg::EnablePrinter> {
  using SelfType = ConditionalFieldsWithSameNameTestReg<Condition>;

  enum class Enum { kA = 0b00, kB = 0b11 };

  DEF_COND_BIT(15, a, Condition);
  DEF_COND_FIELD(14, 12, b, Condition);
  DEF_COND_UNSHIFTED_FIELD(11, 10, c, Condition);
  DEF_COND_ENUM_FIELD(Enum, 9, 8, d, Condition);

  DEF_COND_ENUM_FIELD(Enum, 7, 6, d, !Condition);
  DEF_COND_UNSHIFTED_FIELD(5, 4, c, !Condition);
  DEF_COND_FIELD(3, 1, b, !Condition);
  DEF_COND_BIT(0, a, !Condition);

  static auto Get() { return hwreg::RegisterAddr<SelfType>(0); }
};

template <int Variant>
class RegisterWithConditional
    : public hwreg::RegisterBase<RegisterWithConditional<Variant>, uint32_t, hwreg::EnablePrinter> {
 public:
  using SelfType = RegisterWithConditional<Variant>;
  static_assert(Variant < 15 && Variant > 10);
  DEF_COND_BIT(14, xmit, Variant == 14);
  DEF_COND_BIT(13, xmit, Variant == 13);
  DEF_COND_BIT(12, xmit, Variant == 12);
  DEF_COND_BIT(11, xmit, Variant == 11);
  DEF_RSVDZ_FIELD(31, Variant + 1);
  DEF_BIT(4, a);
  DEF_BIT(3, b);
  DEF_BIT(2, c);
  DEF_BIT(1, d);
  DEF_BIT(0, e);

  static auto Get() { return hwreg::RegisterAddr<RegisterWithConditional>(0); }
};

TEST(RegisterTestCase, ConditionalFields) {
  {
    using RegA = ConditionalFieldTestReg<ConditionalFieldTestRegEnum::kA>;

    volatile uint8_t fake_reg = 0xff;
    hwreg::RegisterMmio mmio(&fake_reg);

    auto reg = RegA::Get().ReadFrom(&mmio);
    EXPECT_EQ(0b1, reg.a_7());
    EXPECT_EQ(0b111, reg.a_6_4());
    EXPECT_EQ(0b111, reg.common());
    reg = reg.WriteTo(&mmio).ReadFrom(&mmio);
    EXPECT_EQ(0b11110111, reg.reg_value());
  }

  {
    using RegB = ConditionalFieldTestReg<ConditionalFieldTestRegEnum::kB>;

    volatile uint8_t fake_reg = 0xff;
    hwreg::RegisterMmio mmio(&fake_reg);

    auto reg = RegB::Get().ReadFrom(&mmio);
    EXPECT_EQ(0b11100000, reg.b_7_5());
    EXPECT_EQ(0b111, reg.common());
    reg = reg.WriteTo(&mmio).ReadFrom(&mmio);
    EXPECT_EQ(0b11100111, reg.reg_value());
  }

  {
    using RegC = ConditionalFieldTestReg<ConditionalFieldTestRegEnum::kC>;
    using Rsvp = typename RegC::Rsvp;

    volatile uint16_t fake_reg = 0xffff;
    hwreg::RegisterMmio mmio(&fake_reg);

    auto reg = RegC::Get().ReadFrom(&mmio);
    EXPECT_EQ(Rsvp::kYes, reg.c_7_4());
    EXPECT_EQ(0b1, reg.c_3());
    EXPECT_EQ(0b111, reg.common());
    reg = reg.WriteTo(&mmio).ReadFrom(&mmio);
    EXPECT_EQ(0xff, reg.reg_value());
  }

  {
    using Reg = ConditionalFieldsWithSameNameTestReg<true>;

    volatile uint16_t fake_reg = 0xffff;
    hwreg::RegisterMmio mmio(&fake_reg);

    auto reg = Reg::Get().ReadFrom(&mmio);
    reg.set_a(0).set_b(0).set_c(0x0c00).set_d(Reg::Enum::kA).WriteTo(&mmio);
    uint16_t expected_reg = 0x0cff;
    EXPECT_EQ(expected_reg, static_cast<uint16_t>(fake_reg));
  }

  {
    using Reg = ConditionalFieldsWithSameNameTestReg<false>;

    volatile uint16_t fake_reg = 0xffff;
    hwreg::RegisterMmio mmio(&fake_reg);

    auto reg = Reg::Get().ReadFrom(&mmio);
    reg.set_a(0).set_b(0).set_c(0x0030).set_d(Reg::Enum::kA).WriteTo(&mmio);
    uint16_t expected_reg = 0xff30;
    EXPECT_EQ(static_cast<uint16_t>(fake_reg), expected_reg);
  }

  // Check conditionals work as expected, depending on the condition, the right
  // bit should be set.
  // 14 -> false
  // 12 -> true
  {
    volatile uint16_t fake_reg = 0;
    hwreg::RegisterMmio mmio(&fake_reg);
    auto reg = RegisterWithConditional<14>::Get().FromValue(0);
    reg.set_xmit(1).WriteTo(&mmio);
    EXPECT_NE(fake_reg & (1 << 14), 0u);
  }

  {
    volatile uint16_t fake_reg = 0;
    hwreg::RegisterMmio mmio(&fake_reg);
    auto reg = RegisterWithConditional<13>::Get().FromValue(0);
    reg.set_xmit(1).WriteTo(&mmio);
    EXPECT_NE(fake_reg & (1 << 13), 0u);
  }
  {
    volatile uint16_t fake_reg = 0;
    hwreg::RegisterMmio mmio(&fake_reg);
    auto reg = RegisterWithConditional<12>::Get().FromValue(0);
    reg.set_xmit(1).WriteTo(&mmio);
    EXPECT_NE(fake_reg & (1 << 12), 0u);
  }
  {
    volatile uint16_t fake_reg = 0;
    hwreg::RegisterMmio mmio(&fake_reg);
    auto reg = RegisterWithConditional<11>::Get().FromValue(0);
    reg.set_xmit(1).WriteTo(&mmio);
    EXPECT_NE(fake_reg & (1 << 11), 0u);
  }
}

template <unsigned int N>
struct TemplatedReg : public hwreg::RegisterBase<TemplatedReg<N>, uint32_t> {
  using SelfType = TemplatedReg<N>;

  static_assert(N < 32);

  static hwreg::RegisterAddr<TemplatedReg<N>> Get() { return {0}; }

  DEF_FIELD(N, 0, head);
};

TEST(RegisterTestCase, Templated) {
  volatile uint32_t fake_reg = 0xffff'ffff;
  hwreg::RegisterMmio mmio(&fake_reg);

  {
    auto reg = TemplatedReg<0>::Get().ReadFrom(&mmio);
    EXPECT_EQ(reg.head(), 0b1);
  }

  {
    auto reg = TemplatedReg<4>::Get().ReadFrom(&mmio);
    EXPECT_EQ(reg.head(), 0b11111);
  }

  {
    auto reg = TemplatedReg<31>::Get().ReadFrom(&mmio);
    EXPECT_EQ(reg.head(), 0xffff'ffff);
  }

  {
    auto reg = TemplatedReg<1>::Get().ReadFrom(&mmio);
    reg.set_head(0);
    reg.WriteTo(&mmio);
    EXPECT_EQ(reg.head(), 0);
  }
}

class PrintableTestReg
    : public hwreg::RegisterBase<PrintableTestReg, uint32_t, hwreg::EnablePrinter> {
 public:
  static hwreg::RegisterAddr<PrintableTestReg> Get() { return {0}; }

  DEF_RSVDZ_BIT(31);
  DEF_FIELD(30, 21, field1);
  DEF_FIELD(20, 12, field2);
  DEF_RSVDZ_FIELD(11, 0);
};

class PrintableTestReg2
    : public hwreg::RegisterBase<PrintableTestReg2, uint32_t, hwreg::EnablePrinter> {
 public:
  static hwreg::RegisterAddr<PrintableTestReg2> Get() { return {0}; }

  DEF_FIELD(30, 21, field1);
  DEF_FIELD(20, 12, field2);
};

TEST(RegisterTestCase, Print) {
  volatile uint64_t fake_reg;
  hwreg::RegisterMmio mmio(&fake_reg);

  constexpr uint32_t kInitVal = 0xe987'2fffu;
  fake_reg = kInitVal;
  {
    auto reg = PrintableTestReg::Get().ReadFrom(&mmio);
    unsigned call_count = 0;
    const char* expected[] = {
        "RsvdZ[31:31]: 0x1 (1)",
        "field1[30:21]: 0x34c (844)",
        "field2[20:12]: 0x072 (114)",
        "RsvdZ[11:0]: 0xfff (4095)",
    };
    reg.Print([&](const char* buf) {
      EXPECT_STREQ(expected[call_count], buf, "mismatch");
      call_count++;
    });
    EXPECT_EQ(std::size(expected), call_count);
  }

  {
    auto reg = PrintableTestReg2::Get().ReadFrom(&mmio);
    unsigned call_count = 0;
    const char* expected[] = {
        "field1[30:21]: 0x34c (844)",
        "field2[20:12]: 0x072 (114)",
        "unknown set bits: 0x80000fff",
    };
    reg.Print([&](const char* buf) {
      EXPECT_STREQ(expected[call_count], buf, "mismatch");
      call_count++;
    });
    EXPECT_EQ(std::size(expected), call_count);
  }
}

class IterableTestReg
    : public hwreg::RegisterBase<IterableTestReg, uint32_t, hwreg::EnablePrinter> {
 public:
  DEF_RSVDZ_BIT(31);
  DEF_FIELD(30, 21, field1);
  DEF_FIELD(20, 12, field2);
  DEF_RSVDZ_FIELD(11, 0);
};

TEST(RegisterTestCase, ForEachField) {
  auto reg = IterableTestReg{}.set_field1(0b1001111001).set_field2(0b101010101);
  const struct {
    const char* name;
    uint32_t value;
    uint32_t high_bit;
    uint32_t low_bit;
  } expected_calls[] = {
      {nullptr, 0, 31, 31},
      {"field1", 0b1001111001, 30, 21},
      {"field2", 0b101010101, 20, 12},
      {nullptr, 0, 11, 0},
  };

  size_t call_idx = 0;
  auto cb = [&call_idx, expected_calls](const char* name, uint32_t value, uint32_t high_bit,
                                        uint32_t low_bit) {
    ASSERT_LT(call_idx, std::size(expected_calls));
    auto& expected = expected_calls[call_idx++];
    EXPECT_STREQ(name, expected.name);
    EXPECT_EQ(value, expected.value);
    EXPECT_EQ(high_bit, expected.high_bit);
    EXPECT_EQ(low_bit, expected.low_bit);
  };
  reg.ForEachField(cb);
  EXPECT_EQ(std::size(expected_calls), call_idx);
}

class ChainingTestReg : public hwreg::RegisterBase<ChainingTestReg, uint32_t> {
 public:
  static hwreg::RegisterAddr<ChainingTestReg> Get() { return {0}; }

  DEF_RSVDZ_BIT(31);
  DEF_FIELD(30, 21, field1);
  DEF_FIELD(20, 12, field2);
  DEF_RSVDZ_FIELD(11, 0);
};

// Test using the "fluent" style of chaining calls, like:
// ChainingTestReg::Get().ReadFrom(&mmio).set_field1(0x234).set_field2(0x123).WriteTo(&mmio);
TEST(RegisterTestCase, SetChaining) {
  volatile uint32_t fake_reg;
  hwreg::RegisterMmio mmio(&fake_reg);

  // With ReadFrom from a RegAddr
  fake_reg = ~0u;
  ChainingTestReg::Get().ReadFrom(&mmio).set_field1(0x234).set_field2(0x123).WriteTo(&mmio);
  uint32_t reg_value = fake_reg;
  EXPECT_EQ((0x234u << 21) | (0x123u << 12), reg_value);

  // With ReadFrom from ChainingTestReg
  fake_reg = ~0u;
  auto reg = ChainingTestReg::Get().FromValue(0);
  reg.ReadFrom(&mmio).set_field1(0x234).set_field2(0x123).WriteTo(&mmio);
  reg_value = fake_reg;
  EXPECT_EQ((0x234u << 21) | (0x123u << 12), reg_value);
}

class TestRegWithPrinter
    : public hwreg::RegisterBase<TestRegWithPrinter, uint64_t, hwreg::EnablePrinter> {};
class TestRegWithoutPrinter : public hwreg::RegisterBase<TestRegWithoutPrinter, uint64_t> {};

// Compile-time test that not enabling printing functions provides a size reduction
[[maybe_unused]] void printer_size_reduction() {
  static_assert(sizeof(TestRegWithPrinter) > sizeof(TestRegWithoutPrinter), "");
}

class TestRegForSizing8 : public hwreg::RegisterBase<TestRegForSizing8, uint8_t> {
 public:
  static hwreg::RegisterAddr<TestRegForSizing8> Get() { return {0}; }

  DEF_RSVDZ_BIT(7);
  DEF_RSVDZ_BIT(6);
  DEF_RSVDZ_BIT(5);
  DEF_RSVDZ_BIT(4);
  DEF_RSVDZ_BIT(3);
  DEF_RSVDZ_BIT(2);
  DEF_RSVDZ_BIT(1);
  DEF_RSVDZ_BIT(0);
};
class TestRegForSizing16 : public hwreg::RegisterBase<TestRegForSizing16, uint16_t> {
 public:
  static hwreg::RegisterAddr<TestRegForSizing16> Get() { return {0}; }

  DEF_RSVDZ_BIT(15);
  DEF_RSVDZ_BIT(14);
  DEF_RSVDZ_BIT(13);
  DEF_RSVDZ_BIT(12);
  DEF_RSVDZ_BIT(11);
  DEF_RSVDZ_BIT(10);
  DEF_RSVDZ_BIT(9);
  DEF_RSVDZ_BIT(8);
  DEF_RSVDZ_BIT(7);
  DEF_RSVDZ_BIT(6);
  DEF_RSVDZ_BIT(5);
  DEF_RSVDZ_BIT(4);
  DEF_RSVDZ_BIT(3);
  DEF_RSVDZ_BIT(2);
  DEF_RSVDZ_BIT(1);
  DEF_RSVDZ_BIT(0);
};
class TestRegForSizing32 : public hwreg::RegisterBase<TestRegForSizing32, uint32_t> {
 public:
  static hwreg::RegisterAddr<TestRegForSizing32> Get() { return {0}; }

  DEF_RSVDZ_BIT(15);
  DEF_RSVDZ_BIT(14);
  DEF_RSVDZ_BIT(13);
  DEF_RSVDZ_BIT(12);
  DEF_RSVDZ_BIT(11);
  DEF_RSVDZ_BIT(10);
  DEF_RSVDZ_BIT(9);
  DEF_RSVDZ_BIT(8);
  DEF_RSVDZ_BIT(7);
  DEF_RSVDZ_BIT(6);
  DEF_RSVDZ_BIT(5);
  DEF_RSVDZ_BIT(4);
  DEF_RSVDZ_BIT(3);
  DEF_RSVDZ_BIT(2);
  DEF_RSVDZ_BIT(1);
  DEF_RSVDZ_BIT(0);
};
class TestRegForSizing64 : public hwreg::RegisterBase<TestRegForSizing64, uint64_t> {
 public:
  static hwreg::RegisterAddr<TestRegForSizing64> Get() { return {0}; }

  DEF_RSVDZ_BIT(15);
  DEF_RSVDZ_BIT(14);
  DEF_RSVDZ_BIT(13);
  DEF_RSVDZ_BIT(12);
  DEF_RSVDZ_BIT(11);
  DEF_RSVDZ_BIT(10);
  DEF_RSVDZ_BIT(9);
  DEF_RSVDZ_BIT(8);
  DEF_RSVDZ_BIT(7);
  DEF_RSVDZ_BIT(6);
  DEF_RSVDZ_BIT(5);
  DEF_RSVDZ_BIT(4);
  DEF_RSVDZ_BIT(3);
  DEF_RSVDZ_BIT(2);
  DEF_RSVDZ_BIT(1);
  DEF_RSVDZ_BIT(0);
};

[[maybe_unused]] void type_size() {
  // This C++ feature allows us to reduce the storage requirements.  Without
  // this, instances of derivatives of RegisterBase that escape the compiler's
  // analysis will pay a cost of 1 byte per internal field (so 1 for each
  // BIT/FIELD declaration and 2 for each RSVDZ_BIT/FIELD declaration).
#if __has_cpp_attribute(no_unique_address)
  static_assert(sizeof(TestRegForSizing8) == 8);
  static_assert(sizeof(TestRegForSizing16) == 12);
  static_assert(sizeof(TestRegForSizing32) == 16);
  static_assert(sizeof(TestRegForSizing64) == 32);
#else
  static_assert(sizeof(TestRegForSizing8) == 24);
  static_assert(sizeof(TestRegForSizing16) == 44);
  static_assert(sizeof(TestRegForSizing32) == 52);
  static_assert(sizeof(TestRegForSizing64) == 72);
#endif
}

struct FakeIo {
  template <typename IntType>
  void Write(IntType value, uint32_t) const {
    ZX_ASSERT(value == 17);
  }

  template <typename IntType>
  IntType Read(uint32_t offset) const {
    return 23;
  }
};

class TestRegForVariantIo : public hwreg::RegisterBase<TestRegForVariantIo, uint64_t> {
 public:
  static hwreg::RegisterAddr<TestRegForVariantIo> Get() { return {0}; }

  DEF_FIELD(63, 0, value);
};

TEST(RegisterTestCase, Variant) {
  using MyIo = std::variant<std::variant<FakeIo, hwreg::RegisterMmio>, hwreg::Mock::RegisterIo>;

  MyIo io;

  {
    auto reg = TestRegForVariantIo::Get().ReadFrom(&io);
    EXPECT_EQ(23, reg.value());
    reg.set_value(17);
    reg.WriteTo(&io);
  }

  {
    volatile uint64_t fake_reg = 17;
    io.emplace<0>(&fake_reg);
    auto reg = TestRegForVariantIo::Get().ReadFrom(&io);
    EXPECT_EQ(17, reg.value());
    reg.set_value(23);
    reg.WriteTo(&io);
    EXPECT_EQ(23, fake_reg);
  }

  {
    hwreg::Mock mock;
    io.emplace<1>(*mock.io());
    mock.ExpectRead(uint64_t{17}, 0).ExpectWrite(uint64_t{23}, 1);
    auto reg = TestRegForVariantIo::Get().ReadFrom(&io);
    EXPECT_EQ(17, reg.value());
    reg.set_reg_addr(1);
    reg.set_value(23);
    reg.WriteTo(&io);
    mock.VerifyAndClear();
  }
}

}  // namespace
