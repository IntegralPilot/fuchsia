// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/layout.h>
#include <lib/elfldltl/machine.h>
#include <lib/elfldltl/relocation.h>
#include <lib/elfldltl/testing/typed-test.h>

#include <limits>

#include <gtest/gtest.h>

namespace {

FORMAT_TYPED_TEST_SUITE(ElfldltlRelocationTests);

TYPED_TEST(ElfldltlRelocationTests, VisitRelativeEmpty) {
  using RelocInfo = elfldltl::RelocationInfo<typename TestFixture::Elf>;
  RelocInfo info;
  size_t count = 0;
  EXPECT_TRUE(info.VisitRelative([&count](auto&& reloc) -> bool {
    ++count;
    return false;
  }));
  EXPECT_EQ(0u, count);
}

template <class RelocInfo>
constexpr typename RelocInfo::size_type RelocOffset(const RelocInfo& info,
                                                    const typename RelocInfo::Rel& reloc) {
  return reloc.offset;
}

template <class RelocInfo>
constexpr typename RelocInfo::size_type RelocOffset(const RelocInfo& info,
                                                    const typename RelocInfo::Rela& reloc) {
  return reloc.offset;
}

template <class RelocInfo>
constexpr typename RelocInfo::size_type RelocOffset(const RelocInfo& info,
                                                    const typename RelocInfo::size_type& reloc) {
  return reloc;
}

template <class RelocInfo>
constexpr typename RelocInfo::Addend RelocAddend(const RelocInfo& info,
                                                 const typename RelocInfo::Rel& reloc) {
  return 0;
}

template <class RelocInfo>
constexpr typename RelocInfo::Addend RelocAddend(const RelocInfo& info,
                                                 const typename RelocInfo::Rela& reloc) {
  return reloc.addend;
}

template <class RelocInfo>
constexpr typename RelocInfo::Addend RelocAddend(const RelocInfo& info,
                                                 const typename RelocInfo::size_type& reloc) {
  return 0;
}

constexpr auto kTestMachine = elfldltl::ElfMachine::kNone;
using TestType = elfldltl::RelocationTraits<kTestMachine>::Type;
constexpr uint32_t kRelativeType = static_cast<uint32_t>(TestType::kRelative);

template <class Elf, bool BadCount = false>
void VisitRelativeRel() {
  using RelocInfo = elfldltl::RelocationInfo<Elf>;
  using Rel = typename RelocInfo::Rel;

  constexpr Rel relocs[] = {
      {8, Rel::MakeInfo(0, kRelativeType)},
      {24, Rel::MakeInfo(0, kRelativeType)},
  };

  RelocInfo info;
  info.set_rel(relocs, BadCount ? 99 : 2);

  EXPECT_TRUE(RelocInfo::template ValidateRelative<kTestMachine>(info.rel_relative()));

  size_t count = 0;
  EXPECT_TRUE(info.VisitRelative([&](auto&& reloc) -> bool {
    auto offset = RelocOffset(info, reloc);
    switch (count++) {
      case 0:
        EXPECT_EQ(8u, offset);
        break;
      case 1:
        EXPECT_EQ(24u, offset);
        break;
    }
    return true;
  }));
  EXPECT_EQ(2u, count);
}

TYPED_TEST(ElfldltlRelocationTests, VisitRelativeRel) {
  VisitRelativeRel<typename TestFixture::Elf>();
}

TYPED_TEST(ElfldltlRelocationTests, VisitRelativeBadRelCount) {
  VisitRelativeRel<typename TestFixture::Elf, true>();
}

template <class Elf, bool BadCount = false>
void VisitRelativeRela() {
  using RelocInfo = elfldltl::RelocationInfo<Elf>;
  using Rela = typename RelocInfo::Rela;

  constexpr Rela relocs[] = {
      {8, Rela::MakeInfo(0, kRelativeType), 0x11111111},
      {24, Rela::MakeInfo(0, kRelativeType), 0x33333333},
  };

  RelocInfo info;
  info.set_rela(relocs, BadCount ? 99 : 2);

  EXPECT_TRUE(RelocInfo::template ValidateRelative<kTestMachine>(info.rela_relative()));

  size_t count = 0;
  EXPECT_TRUE(info.VisitRelative([&](auto&& reloc) -> bool {
    auto offset = RelocOffset(info, reloc);
    auto addend = RelocAddend(info, reloc)();
    switch (count++) {
      case 0:
        EXPECT_EQ(8u, offset);
        EXPECT_EQ(0x11111111, addend);
        break;
      case 1:
        EXPECT_EQ(24u, offset);
        EXPECT_EQ(0x33333333, addend);
        break;
    }
    return true;
  }));
  EXPECT_EQ(2u, count);
}

TYPED_TEST(ElfldltlRelocationTests, VisitRelativeRela) {
  VisitRelativeRela<typename TestFixture::Elf>();
}

TYPED_TEST(ElfldltlRelocationTests, VisitRelativeBadRelaCount) {
  VisitRelativeRela<typename TestFixture::Elf, true>();
}

TYPED_TEST(ElfldltlRelocationTests, VisitRelativeRelrSingle) {
  using RelocInfo = elfldltl::RelocationInfo<typename TestFixture::Elf>;
  using Addr = typename RelocInfo::Addr;

  constexpr Addr relocs[] = {
      8,
  };

  RelocInfo info;
  info.set_relr(relocs);

  EXPECT_TRUE(RelocInfo::ValidateRelative(info.relr()));

  size_t count = 0;
  EXPECT_TRUE(info.VisitRelative([&](auto&& reloc) -> bool {
    auto offset = RelocOffset(info, reloc);
    switch (count++) {
      case 0:
        EXPECT_EQ(8u, offset);
        break;
    }
    return true;
  }));
  EXPECT_EQ(1u, count);
}

TYPED_TEST(ElfldltlRelocationTests, VisitRelativeRelrNoBitmaps) {
  using RelocInfo = elfldltl::RelocationInfo<typename TestFixture::Elf>;
  using Addr = typename RelocInfo::Addr;

  constexpr Addr relocs[] = {
      0x8,
      0x18,
      0x28,
  };

  RelocInfo info;
  info.set_relr(relocs);

  EXPECT_TRUE(RelocInfo::ValidateRelative(info.relr()));

  size_t count = 0;
  EXPECT_TRUE(info.VisitRelative([&](auto&& reloc) -> bool {
    auto offset = RelocOffset(info, reloc);
    switch (count++) {
      case 0:
        EXPECT_EQ(0x8u, offset);
        break;
      case 1:
        EXPECT_EQ(0x18u, offset);
        break;
      case 2:
        EXPECT_EQ(0x28u, offset);
        break;
    }
    return true;
  }));
  EXPECT_EQ(3u, count);
}

TYPED_TEST(ElfldltlRelocationTests, VisitRelativeRelrSingleBitmap) {
  using RelocInfo = elfldltl::RelocationInfo<typename TestFixture::Elf>;
  using Addr = typename RelocInfo::Addr;

  constexpr Addr relocs[] = {
      0x8,
      0b10101,
  };

  RelocInfo info;
  info.set_relr(relocs);

  EXPECT_TRUE(RelocInfo::ValidateRelative(info.relr()));

  size_t count = 0;
  EXPECT_TRUE(info.VisitRelative([&](auto&& reloc) -> bool {
    auto offset = RelocOffset(info, reloc);
    EXPECT_EQ(0x8 + (sizeof(Addr) * 2 * count), offset);
    ++count;
    return true;
  }));
  EXPECT_EQ(3u, count);
}

TYPED_TEST(ElfldltlRelocationTests, VisitRelativeRelrMultipleBitmaps) {
  using RelocInfo = elfldltl::RelocationInfo<typename TestFixture::Elf>;
  using Addr = typename RelocInfo::Addr;
  using size_type = typename RelocInfo::size_type;

  constexpr auto bitmap = [](uint32_t bits) -> size_type {
    if constexpr (sizeof(Addr) == sizeof(uint32_t)) {
      return bits;
    } else {
      return (static_cast<uint64_t>(bits) << 32) | bits;
    }
  };

  constexpr Addr relocs[] = {
      0x8,
      bitmap(0x55555555),
      bitmap(0xaaaaaaaa) | 1,
  };

  RelocInfo info;
  info.set_relr(relocs);

  EXPECT_TRUE(RelocInfo::ValidateRelative(info.relr()));

  size_t count = 0;
  EXPECT_TRUE(info.VisitRelative([&](auto&& reloc) -> bool {
    auto offset = RelocOffset(info, reloc);
    EXPECT_EQ(0x8 + (sizeof(Addr) * 2 * count), offset) << sizeof(Addr) << " * 2 * " << count;
    ++count;
    return true;
  }));

  EXPECT_EQ(static_cast<size_t>(TestFixture::Elf::kAddressBits), count);
}

TYPED_TEST(ElfldltlRelocationTests, VisitSymbolicEmpty) {
  using RelocInfo = elfldltl::RelocationInfo<typename TestFixture::Elf>;
  RelocInfo info;
  size_t count = 0;
  EXPECT_TRUE(info.VisitSymbolic([&count](auto&& reloc) -> bool {
    ++count;
    return false;
  }));
  EXPECT_EQ(0u, count);
}

// TODO(https://fxbug.dev/42151659): real VisitSymbolic tests

template <elfldltl::ElfMachine Machine>
constexpr void CheckMachine() {
  using Traits = elfldltl::RelocationTraits<Machine>;

  // Each machine must provide all these as distinct values, and no others.
  // This is mostly just a compile-time test to elicit errors if a Type::kFoo
  // is missing and to get the compiler warnings if any enum constants are
  // omitted from this switch.  The only runtime test is that kNone is zero.
  uint32_t type = 0;
  switch (static_cast<typename Traits::Type>(type)) {
    case Traits::Type::kNone:  // Has value zero on every machine.
      EXPECT_EQ(0u, type);
      break;

      // All other values are machine-dependent.
    case Traits::Type::kRelative:
    case Traits::Type::kAbsolute:
    case Traits::Type::kPlt:
    case Traits::Type::kTlsAbsolute:
    case Traits::Type::kTlsRelative:
    case Traits::Type::kTlsModule:
      FAIL();
      break;

    default:
      if (type == Traits::kGot) {
        FAIL();
        break;
      }

      if (type == Traits::kTlsDesc) {
        FAIL();
        break;
      }
  }
}

template <elfldltl::ElfMachine... Machines>
struct CheckMachines {
  CheckMachines() { (CheckMachine<Machines>(), ...); }
};

TEST(ElfldltlRelocationTests, Machines) { elfldltl::AllSupportedMachines<CheckMachines>(); }

}  // namespace
