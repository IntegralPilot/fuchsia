// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "load-tests.h"

#include <lib/elfldltl/container.h>
#include <lib/elfldltl/diagnostics.h>
#include <lib/elfldltl/load.h>
#include <lib/elfldltl/memory.h>
#include <lib/elfldltl/phdr.h>
#include <lib/elfldltl/static-vector.h>
#include <lib/elfldltl/testing/diagnostics.h>
#include <lib/elfldltl/testing/typed-test.h>
#include <lib/symbolizer-markup/writer.h>

#include <source_location>
#include <span>

namespace {

using elfldltl::testing::ConstantPhdr;
using elfldltl::testing::DataPhdr;
using elfldltl::testing::DataWithZeroFillPhdr;
using elfldltl::testing::ExpectedSingleError;
using elfldltl::testing::ExpectOkDiagnostics;
using elfldltl::testing::ZeroFillPhdr;

constexpr size_t kPageSize = 0x1000;

FORMAT_TYPED_TEST_SUITE(ElfldltlLoadTests);

TYPED_TEST(ElfldltlLoadTests, FailToAdd) {
  using Elf = typename TestFixture::Elf;
  using Phdr = typename Elf::Phdr;

  ExpectedSingleError error("too many PT_LOAD segments", ": maximum 0 < requested ", 1);

  elfldltl::LoadInfo<Elf, elfldltl::StaticVector<0>::Container> load_info;

  Phdr phdr{.memsz = 1};
  EXPECT_FALSE(load_info.AddSegment(error, kPageSize, phdr));
}

TYPED_TEST(ElfldltlLoadTests, AddEmptyPhdr) {
  using Elf = typename TestFixture::Elf;
  using Phdr = typename Elf::Phdr;

  auto diag = ExpectOkDiagnostics();

  elfldltl::LoadInfo<Elf, elfldltl::StaticVector<0>::Container> load_info;

  Phdr phdr{};
  EXPECT_TRUE(load_info.AddSegment(diag, kPageSize, phdr));
}

TYPED_TEST(ElfldltlLoadTests, CreateConstantSegment) {
  using Elf = typename TestFixture::Elf;
  using Phdr = typename Elf::Phdr;

  auto diag = ExpectOkDiagnostics();

  elfldltl::LoadInfo<Elf, elfldltl::StaticVector<1>::Container> load_info;
  using ConstantSegment = typename decltype(load_info)::ConstantSegment;

  Phdr phdr{.memsz = kPageSize * 10};
  EXPECT_TRUE(load_info.AddSegment(diag, kPageSize, phdr));

  const auto& segments = load_info.segments();
  ASSERT_EQ(segments.size(), 1u);
  const auto& variant = segments[0];
  ASSERT_TRUE(std::holds_alternative<ConstantSegment>(variant));
  EXPECT_EQ(std::get<ConstantSegment>(variant).memsz(), phdr.memsz);
}

TYPED_TEST(ElfldltlLoadTests, CreateZeroFillSegment) {
  using Elf = typename TestFixture::Elf;
  using Phdr = typename Elf::Phdr;

  auto diag = ExpectOkDiagnostics();

  elfldltl::LoadInfo<Elf, elfldltl::StaticVector<1>::Container> load_info;
  using ZeroFillSegment = typename decltype(load_info)::ZeroFillSegment;

  Phdr phdr{.memsz = kPageSize * 5};
  phdr.flags = Phdr::kRead | Phdr::kWrite;
  EXPECT_TRUE(load_info.AddSegment(diag, kPageSize, phdr));

  const auto& segments = load_info.segments();
  ASSERT_EQ(segments.size(), 1u);
  const auto& variant = segments[0];
  ASSERT_TRUE(std::holds_alternative<ZeroFillSegment>(variant));
  EXPECT_EQ(std::get<ZeroFillSegment>(variant).memsz(), phdr.memsz);
}

TYPED_TEST(ElfldltlLoadTests, CreateDataWithZeroFillSegment) {
  using Elf = typename TestFixture::Elf;
  using Phdr = typename Elf::Phdr;

  auto diag = ExpectOkDiagnostics();

  elfldltl::LoadInfo<Elf, elfldltl::StaticVector<1>::Container> load_info;
  using DataWithZeroFillSegment = typename decltype(load_info)::DataWithZeroFillSegment;

  Phdr phdr{.filesz = kPageSize, .memsz = kPageSize * 5};
  phdr.flags = Phdr::kRead | Phdr::kWrite;
  EXPECT_TRUE(load_info.AddSegment(diag, kPageSize, phdr));

  const auto& segments = load_info.segments();
  ASSERT_EQ(segments.size(), 1u);
  const auto& variant = segments[0];
  ASSERT_TRUE(std::holds_alternative<DataWithZeroFillSegment>(variant));
  EXPECT_EQ(std::get<DataWithZeroFillSegment>(variant).memsz(), phdr.memsz());
}

TYPED_TEST(ElfldltlLoadTests, CreateDataSegment) {
  using Elf = typename TestFixture::Elf;
  using Phdr = typename Elf::Phdr;

  auto diag = ExpectOkDiagnostics();

  elfldltl::LoadInfo<Elf, elfldltl::StaticVector<1>::Container> load_info;
  using DataSegment = typename decltype(load_info)::DataSegment;

  Phdr phdr{.filesz = kPageSize, .memsz = kPageSize};
  phdr.flags = Phdr::kRead | Phdr::kWrite;
  EXPECT_TRUE(load_info.AddSegment(diag, kPageSize, phdr));

  const auto& segments = load_info.segments();
  ASSERT_EQ(segments.size(), 1u);
  const auto& variant = segments[0];
  ASSERT_TRUE(std::holds_alternative<DataSegment>(variant));
  EXPECT_EQ(std::get<DataSegment>(variant).memsz(), phdr.memsz());
}

template <class Elf, bool Merged, template <class ElfLayout> typename Segment1,
          template <class ElfLayout> typename Segment2,
          template <class ElfLayout> typename GetPhdr1,
          template <class ElfLayout> typename GetPhdr2>
void DoMergeTest() {
  using Segment1T = Segment1<Elf>;
  using Segment2T = Segment2<Elf>;
  using size_type = typename Elf::size_type;
  constexpr unsigned totalSegments = Merged ? 1 : 2;

  auto diag = ExpectOkDiagnostics();

  elfldltl::LoadInfo<Elf, elfldltl::StaticVector<2>::Container> load_info;
  const auto& segments = load_info.segments();

  size_type offset = 0;
  auto phdr1 = GetPhdr1<Elf>{}(offset);
  auto phdr2 = GetPhdr2<Elf>{}(offset);
  auto expectedSize = Merged ? phdr1.memsz() + phdr2.memsz() : phdr2.memsz();

  load_info.AddSegment(diag, kPageSize, phdr1);
  ASSERT_EQ(segments.size(), 1u);
  ASSERT_TRUE(std::holds_alternative<Segment1T>(segments.back()));
  EXPECT_EQ(std::get<Segment1T>(segments.back()).memsz(), phdr1.memsz());
  load_info.AddSegment(diag, kPageSize, phdr2);
  ASSERT_EQ(segments.size(), totalSegments);
  ASSERT_TRUE(std::holds_alternative<Segment2T>(segments.back()));
  EXPECT_EQ(std::get<Segment2T>(segments.back()).memsz(), expectedSize);
}

template <class Elf, template <class ElfLayout> typename Segment1,
          template <class ElfLayout> typename Segment2,
          template <class ElfLayout> typename GetPhdr1,
          template <class ElfLayout> typename GetPhdr2>
void MergeTest() {
  DoMergeTest<Elf, true, Segment1, Segment2, GetPhdr1, GetPhdr2>();
}

template <class Elf, template <class ElfLayout> typename Segment1,
          template <class ElfLayout> typename Segment2,
          template <class ElfLayout> typename GetPhdr1,
          template <class ElfLayout> typename GetPhdr2>
void NotMergedTest() {
  DoMergeTest<Elf, false, Segment1, Segment2, GetPhdr1, GetPhdr2>();
}

template <class Elf, template <class ElfLayout> typename Segment,
          template <class ElfLayout> typename GetPhdr>
void MergeSameTest() {
  MergeTest<Elf, Segment, Segment, GetPhdr, GetPhdr>();
}
template <typename Elf>
using ConstantSegment =
    typename elfldltl::LoadInfo<Elf, elfldltl::StaticVector<0>::Container>::ConstantSegment;

template <typename Elf>
using ZeroFillSegment =
    typename elfldltl::LoadInfo<Elf, elfldltl::StaticVector<0>::Container>::ZeroFillSegment;

template <typename Elf>
using DataWithZeroFillSegment =
    typename elfldltl::LoadInfo<Elf, elfldltl::StaticVector<0>::Container>::DataWithZeroFillSegment;

template <typename Elf>
using DataSegment =
    typename elfldltl::LoadInfo<Elf, elfldltl::StaticVector<0>::Container>::DataSegment;

TYPED_TEST(ElfldltlLoadTests, MergeSameConstantSegment) {
  MergeSameTest<typename TestFixture::Elf, ConstantSegment, ConstantPhdr>();
}

TYPED_TEST(ElfldltlLoadTests, MergeSameDataSegment) {
  MergeSameTest<typename TestFixture::Elf, DataSegment, DataPhdr>();
}

TYPED_TEST(ElfldltlLoadTests, MergeDataAndZeroFill) {
  MergeTest<typename TestFixture::Elf, DataSegment, DataWithZeroFillSegment, DataPhdr,
            ZeroFillPhdr>();
}

TYPED_TEST(ElfldltlLoadTests, MergeDataAndDataWithZeroFill) {
  MergeTest<typename TestFixture::Elf, DataSegment, DataWithZeroFillSegment, DataPhdr,
            DataWithZeroFillPhdr>();
}

TYPED_TEST(ElfldltlLoadTests, CantMergeConstant) {
  NotMergedTest<typename TestFixture::Elf, ConstantSegment, ZeroFillSegment, ConstantPhdr,
                ZeroFillPhdr>();
  NotMergedTest<typename TestFixture::Elf, ConstantSegment, DataWithZeroFillSegment, ConstantPhdr,
                DataWithZeroFillPhdr>();
  NotMergedTest<typename TestFixture::Elf, ConstantSegment, DataSegment, ConstantPhdr, DataPhdr>();
}

TYPED_TEST(ElfldltlLoadTests, CantMergeZeroFill) {
  NotMergedTest<typename TestFixture::Elf, ZeroFillSegment, ConstantSegment, ZeroFillPhdr,
                ConstantPhdr>();
  // Logically two ZeroFillSegment's could be merged but we don't currently do
  // this because these are unlikely to exist in the wild.
  NotMergedTest<typename TestFixture::Elf, ZeroFillSegment, ZeroFillSegment, ZeroFillPhdr,
                ZeroFillPhdr>();
  NotMergedTest<typename TestFixture::Elf, ZeroFillSegment, DataWithZeroFillSegment, ZeroFillPhdr,
                DataWithZeroFillPhdr>();
  NotMergedTest<typename TestFixture::Elf, ZeroFillSegment, DataSegment, ZeroFillPhdr, DataPhdr>();
}

TYPED_TEST(ElfldltlLoadTests, CantMergeDataAndZeroFill) {
  NotMergedTest<typename TestFixture::Elf, DataWithZeroFillSegment, ConstantSegment,
                DataWithZeroFillPhdr, ConstantPhdr>();
  NotMergedTest<typename TestFixture::Elf, DataWithZeroFillSegment, DataWithZeroFillSegment,
                DataWithZeroFillPhdr, DataWithZeroFillPhdr>();
  NotMergedTest<typename TestFixture::Elf, DataWithZeroFillSegment, DataSegment,
                DataWithZeroFillPhdr, DataPhdr>();
}

TYPED_TEST(ElfldltlLoadTests, CantMergeData) {
  NotMergedTest<typename TestFixture::Elf, DataSegment, ConstantSegment, DataPhdr, ConstantPhdr>();
}

TYPED_TEST(ElfldltlLoadTests, GetPhdrObserver) {
  using Elf = typename TestFixture::Elf;
  using Phdr = typename Elf::Phdr;
  using size_type = typename Elf::size_type;

  auto diag = ExpectOkDiagnostics();

  elfldltl::LoadInfo<Elf, elfldltl::StdContainer<std::vector>::Container> load_info;
  using ConstantSegment = typename decltype(load_info)::ConstantSegment;
  using DataWithZeroFillSegment = typename decltype(load_info)::DataWithZeroFillSegment;

  size_type offset = 0;
  const Phdr kPhdrs[] = {
      ConstantPhdr<Elf>{}(offset), ConstantPhdr<Elf>{}(offset), DataPhdr<Elf>{}(offset),
      DataPhdr<Elf>{}(offset),     ZeroFillPhdr<Elf>{}(offset),
  };

  EXPECT_TRUE(elfldltl::DecodePhdrs(diag, std::span(kPhdrs), load_info.GetPhdrObserver(kPageSize)));
  const auto& segments = load_info.segments();
  EXPECT_EQ(segments.size(), 2u);
  ASSERT_TRUE(std::holds_alternative<ConstantSegment>(segments[0]));
  EXPECT_EQ(std::get<ConstantSegment>(segments[0]).memsz(), kPhdrs[0].memsz + kPhdrs[1].memsz);
  ASSERT_TRUE(std::holds_alternative<DataWithZeroFillSegment>(segments[1]));
  EXPECT_EQ(std::get<DataWithZeroFillSegment>(segments[1]).memsz(),
            kPhdrs[2].memsz + kPhdrs[3].memsz + kPhdrs[4].memsz);
}

TYPED_TEST(ElfldltlLoadTests, VisitSegments) {
  using Elf = typename TestFixture::Elf;
  using Phdr = typename Elf::Phdr;
  using size_type = typename Elf::size_type;

  auto diag = ExpectOkDiagnostics();

  elfldltl::LoadInfo<Elf, elfldltl::StdContainer<std::vector>::Container> load_info;

  ASSERT_EQ(load_info.segments().size(), 0u);
  EXPECT_TRUE(load_info.VisitSegments([](auto&& segment) {
    ADD_FAILURE();
    return true;
  }));

  size_type offset = 0;
  const Phdr kPhdrs[] = {
      ConstantPhdr<Elf>{}(offset),
      DataPhdr<Elf>{}(offset),
  };

  EXPECT_TRUE(elfldltl::DecodePhdrs(diag, std::span(kPhdrs), load_info.GetPhdrObserver(kPageSize)));
  ASSERT_EQ(load_info.segments().size(), 2u);

  int currentIndex = 0;
  EXPECT_TRUE(load_info.VisitSegments([&](auto&& segment) {
    EXPECT_EQ(segment.offset(), kPhdrs[currentIndex++].offset);
    return true;
  }));

  currentIndex = 0;
  EXPECT_FALSE(load_info.VisitSegments([&](auto&& segment) {
    EXPECT_EQ(currentIndex++, 0);
    return false;
  }));
}

TYPED_TEST(ElfldltlLoadTests, RemoveLastSegment) {
  using Elf = typename TestFixture::Elf;
  using Phdr = typename Elf::Phdr;
  using size_type = typename Elf::size_type;
  using LoadInfo = elfldltl::LoadInfo<Elf, elfldltl::StdContainer<std::vector>::Container>;

  auto diag = ExpectOkDiagnostics();

  LoadInfo load_info;

  size_type offset = 0;
  const Phdr kPhdrs[] = {
      ConstantPhdr<Elf>{}(offset),
      DataPhdr<Elf>{}(offset),
  };
  EXPECT_TRUE(elfldltl::DecodePhdrs(diag, std::span(kPhdrs), load_info.GetPhdrObserver(kPageSize)));
  ASSERT_EQ(load_info.segments().size(), 2u);

  EXPECT_EQ(load_info.vaddr_size(), 2 * kPageSize);

  auto segment = load_info.RemoveLastSegment();
  static_assert(std::is_same_v<decltype(segment), typename LoadInfo::Segment>);

  EXPECT_EQ(load_info.segments().size(), 1u);
  EXPECT_TRUE(
      std::holds_alternative<typename LoadInfo::ConstantSegment>(load_info.segments().front()));
  EXPECT_EQ(load_info.vaddr_size(), kPageSize);
}

TYPED_TEST(ElfldltlLoadTests, AddSegmentUpdatesVaddrSize) {
  using Elf = typename TestFixture::Elf;
  using LoadInfo = elfldltl::LoadInfo<Elf, elfldltl::StdContainer<std::vector>::Container>;

  auto diag = ExpectOkDiagnostics();

  LoadInfo load_info;
  EXPECT_EQ(load_info.vaddr_size(), 0u);

  EXPECT_TRUE(load_info.AddSegment(diag, typename LoadInfo::ZeroFillSegment(0, kPageSize)));
  ASSERT_EQ(load_info.segments().size(), 1u);
  EXPECT_EQ(load_info.vaddr_size(), kPageSize);
}

TYPED_TEST(ElfldltlLoadTests, RelroBounds) {
  using Elf = typename TestFixture::Elf;
  using Phdr = typename Elf::Phdr;

  elfldltl::LoadInfo<Elf, elfldltl::StdContainer<std::vector>::Container> load_info;
  using Region = typename decltype(load_info)::Region;

  {
    Region r = load_info.RelroBounds({}, kPageSize);
    EXPECT_EQ(r.start, 0u);
    EXPECT_EQ(r.end, 0u);
    EXPECT_TRUE(r.empty());
  }
  {
    Phdr phdr{.memsz = kPageSize - 1};
    Region r = load_info.RelroBounds(phdr, kPageSize);
    EXPECT_EQ(r.start, 0u);
    EXPECT_EQ(r.end, 0u);
    EXPECT_TRUE(r.empty());
  }
  {
    Phdr phdr{.memsz = kPageSize};
    Region r = load_info.RelroBounds(phdr, kPageSize);
    EXPECT_EQ(r.start, 0u);
    EXPECT_EQ(r.end, kPageSize);
  }
  {
    Phdr phdr{.memsz = kPageSize + 1};
    Region r = load_info.RelroBounds(phdr, kPageSize);
    EXPECT_EQ(r.start, 0u);
    EXPECT_EQ(r.end, kPageSize);
  }
}

TYPED_TEST(ElfldltlLoadTests, ApplyRelroMissing) {
  using Elf = typename TestFixture::Elf;
  using Phdr = typename Elf::Phdr;
  using size_type = typename Elf::size_type;

  auto diag = ExpectOkDiagnostics();

  elfldltl::LoadInfo<Elf, elfldltl::StdContainer<std::vector>::Container> load_info;

  size_type offset = kPageSize;
  Phdr phdrs[] = {
      DataPhdr<Elf>{}(offset),
      {.type = elfldltl::ElfPhdrType::kRelro, .memsz = kPageSize},
  };

  ASSERT_FALSE(load_info.RelroBounds(phdrs[1], kPageSize).empty());

  {
    ASSERT_EQ(load_info.segments().size(), 0u);
    ExpectedSingleError expected("PT_GNU_RELRO not in any data segment");
    EXPECT_TRUE(load_info.ApplyRelro(expected, phdrs[1], kPageSize, false));
  }

  EXPECT_TRUE(elfldltl::DecodePhdrs(diag, std::span<const Phdr>(phdrs),
                                    load_info.GetPhdrObserver(kPageSize)));

  {
    ASSERT_EQ(load_info.segments().size(), 1u);
    ExpectedSingleError expected("PT_GNU_RELRO not in any data segment");
    EXPECT_TRUE(load_info.ApplyRelro(expected, phdrs[1], kPageSize, false));
  }
}

TYPED_TEST(ElfldltlLoadTests, ApplyRelroBadStart) {
  using Elf = typename TestFixture::Elf;
  using Phdr = typename Elf::Phdr;

  auto diag = ExpectOkDiagnostics();

  elfldltl::LoadInfo<Elf, elfldltl::StdContainer<std::vector>::Container> load_info;

  Phdr phdrs[] = {
      {.type = elfldltl::ElfPhdrType::kLoad, .filesz = 2 * kPageSize, .memsz = 2 * kPageSize},
      {.type = elfldltl::ElfPhdrType::kRelro, .vaddr = kPageSize, .memsz = kPageSize},
  };

  phdrs[0].flags = elfldltl::PhdrBase::kRead | elfldltl::PhdrBase::kWrite;

  ASSERT_EQ(load_info.RelroBounds(phdrs[1], kPageSize).start, kPageSize);
  ASSERT_EQ(load_info.RelroBounds(phdrs[1], kPageSize).end, kPageSize * 2);

  EXPECT_TRUE(elfldltl::DecodePhdrs(diag, std::span<const Phdr>(phdrs),
                                    load_info.GetPhdrObserver(kPageSize)));

  ExpectedSingleError expected("PT_GNU_RELRO not at segment start");
  EXPECT_TRUE(load_info.ApplyRelro(expected, phdrs[1], kPageSize, false));
}

TYPED_TEST(ElfldltlLoadTests, ApplyRelroTooManyLoads) {
  using Elf = typename TestFixture::Elf;
  using Phdr = typename Elf::Phdr;

  auto diag = ExpectOkDiagnostics();

  elfldltl::LoadInfo<Elf, elfldltl::StaticVector<1>::Container> load_info;

  Phdr phdrs[] = {
      {.type = elfldltl::ElfPhdrType::kLoad, .filesz = 2 * kPageSize, .memsz = 2 * kPageSize},
      {.type = elfldltl::ElfPhdrType::kRelro, .memsz = kPageSize},
  };
  phdrs[0].flags = elfldltl::PhdrBase::kRead | elfldltl::PhdrBase::kWrite;

  EXPECT_TRUE(elfldltl::DecodePhdrs(diag, std::span<const Phdr>(phdrs),
                                    load_info.GetPhdrObserver(kPageSize)));

  ASSERT_EQ(load_info.segments().size(), 1u);

  auto expected = ExpectedSingleError("too many PT_LOAD segments", ": maximum 1 < requested ", 2);
  load_info.ApplyRelro(expected, phdrs[1], kPageSize, false);
}

using SomeLI = elfldltl::LoadInfo<elfldltl::Elf<>, elfldltl::StdContainer<std::vector>::Container>;
enum SegmentType {
  C = SomeLI::Segment(SomeLI::ConstantSegment(0, 0, 0, 0)).index(),
  D = SomeLI::Segment(SomeLI::DataSegment(0, 0, 0, 0)).index(),
  DWZF = SomeLI::Segment(SomeLI::DataWithZeroFillSegment(0, 0, 0, 0)).index(),
  ZF = SomeLI::Segment(SomeLI::ZeroFillSegment(0, 0)).index(),
  RO,  // DataSegment that should overlaps with the relro region
};

// Can't be {RO} or {C}
using SplitStrategy = std::optional<SegmentType>;

// This class creates adjacent segments based on segment type.
// All segments except for 'RO' will have a memsz of `kPageSize`, the flags and filesz are changed
// depending on the `SegmentType`. The SplitStrategy defines how a 'RO' segment should be created
// such that it will be split into a ConstSegment and a segment defined by the strategy.
// For example:
// {C, RO, D} with a ZF split strategy will create the following Phdrs
// | Type         |  |     C     ||         RO(ZF)        ||      D      |
// | flags        |  |     R     ||           RW          ||      RW     |
// | offset       |  |     0     ||       kPagesize       || kPagesize*3 |
// | {mem,file}sz |  | kPagesize || kPagesize*2,kPagesize ||  kPagesize  |
// get_relro_phdr will return a phdr that overlaps with the RO segment like:
//                                |    RO     |
//                                |   ~RWX    |
//                                | kPagesize |
//                                | kPagesize |
// Such that after ApplyRelro is called the 'RO(ZF)' segment will be split into a ConstantSegment
// and a ZeroFillSegment. The expected result then would be {C, C, ZF, D} with merge_ro false or
// {C, ZF, D} with merge_ro true.
template <typename Elf>
struct PhdrCreator {
  using Phdr = typename Elf::Phdr;
  using size_type = typename Elf::size_type;

  SplitStrategy strategy;
  size_type offset = 0;
  size_type relro_offset = 0;

  Phdr operator()(SegmentType type) {
    Phdr phdr{.type = elfldltl::ElfPhdrType::kLoad, .offset = offset, .vaddr = offset};
    auto w = {D, DWZF, ZF, RO};
    if (std::any_of(w.begin(), w.end(), [type](auto t) { return type == t; })) {
      phdr.flags = elfldltl::PhdrBase::kRead | elfldltl::PhdrBase::kWrite;
    } else {
      phdr.flags = elfldltl::PhdrBase::kRead;
    }

    size_type memsz = kPageSize;
    size_type filesz = kPageSize;
    if (type == DWZF || (type == RO && strategy && *strategy == DWZF)) {
      filesz /= 2;
    } else if (type == ZF || (type == RO && strategy && *strategy == ZF)) {
      filesz = 0;
    }
    if (type == RO) {
      relro_offset = offset;
      if (strategy) {
        memsz += kPageSize;
        filesz += kPageSize;
      }
    }

    offset += memsz;

    phdr.memsz = memsz;
    phdr.filesz = filesz;
    return phdr;
  }

  Phdr get_relro_phdr() {
    return {.type = elfldltl::ElfPhdrType::kRelro, .vaddr = relro_offset, .memsz = kPageSize};
  }
};

using PhdrsPattern = std::initializer_list<SegmentType>;

template <class Elf, template <class> class SegmentWrapper = elfldltl::NoSegmentWrapper>
using RelroTestLoadInfo = elfldltl::LoadInfo<Elf, elfldltl::StdContainer<std::vector>::Container,
                                             elfldltl::PhdrLoadPolicy::kBasic, SegmentWrapper>;

template <typename Elf, template <class> class SegmentWrapper = elfldltl::NoSegmentWrapper>
void RelroTest(PhdrsPattern input, PhdrsPattern expected, SplitStrategy strategy, bool merge_ro,
               std::source_location loc = std::source_location::current()) {
  using Phdr = typename Elf::Phdr;

  std::vector<Phdr> input_phdrs;
  PhdrCreator<Elf> creator{strategy};
  std::transform(input.begin(), input.end(), std::back_inserter(input_phdrs), std::ref(creator));

  auto diag = ExpectOkDiagnostics();

  RelroTestLoadInfo<Elf, SegmentWrapper> load_info;
  EXPECT_TRUE(elfldltl::DecodePhdrs(diag,
                                    std::span<const Phdr>(input_phdrs.data(), input_phdrs.size()),
                                    load_info.GetPhdrObserver(kPageSize)));
  ASSERT_TRUE(load_info.ApplyRelro(diag, creator.get_relro_phdr(), kPageSize, merge_ro))
      << "line " << loc.line();
  auto& segments = load_info.segments();
  ASSERT_EQ(segments.size(), expected.size()) << "line " << loc.line();

  for (size_t i = 0; i < segments.size(); i++) {
    EXPECT_EQ(segments[i].index(), std::data(expected)[i]) << "line " << loc.line();
  }
}

template <typename Elf, template <class> class SegmentWrapper = elfldltl::NoSegmentWrapper>
void RelroTest(PhdrsPattern input, PhdrsPattern expected, SplitStrategy strategy,
               std::source_location loc = std::source_location::current()) {
  RelroTest<Elf, SegmentWrapper>(input, expected, strategy, true, loc);
  RelroTest<Elf, SegmentWrapper>(input, expected, strategy, false, loc);
}

TYPED_TEST(ElfldltlLoadTests, ApplyRelroBasic) {
  using Elf = typename TestFixture::Elf;
  RelroTest<Elf>({RO}, {C}, {});
  RelroTest<Elf>({RO}, {C, D}, D);
  RelroTest<Elf>({RO}, {C, DWZF}, DWZF);
  RelroTest<Elf>({RO}, {C, ZF}, ZF);
}

template <class Segment>
class MoveOnlySegmentWrapper : public Segment {
 public:
  using Segment::Segment;

  MoveOnlySegmentWrapper(const MoveOnlySegmentWrapper&) = delete;
  constexpr MoveOnlySegmentWrapper(MoveOnlySegmentWrapper&&) = default;
  constexpr MoveOnlySegmentWrapper& operator=(MoveOnlySegmentWrapper&&) = default;
};

TYPED_TEST(ElfldltlLoadTests, ApplyRelroMoveOnly) {
  using Elf = typename TestFixture::Elf;

  using DefaultSegment = typename RelroTestLoadInfo<Elf>::LoadInfo::Segment;
  static_assert(std::is_copy_constructible_v<DefaultSegment>);
  static_assert(std::is_copy_assignable_v<DefaultSegment>);

  using MoveOnlySegment =
      typename RelroTestLoadInfo<Elf, MoveOnlySegmentWrapper>::LoadInfo::Segment;
  static_assert(!std::is_copy_constructible_v<MoveOnlySegment>);
  static_assert(!std::is_copy_assignable_v<MoveOnlySegment>);
  static_assert(std::is_move_constructible_v<MoveOnlySegment>);
  static_assert(std::is_move_assignable_v<MoveOnlySegment>);

  RelroTest<Elf, MoveOnlySegmentWrapper>({RO}, {C}, {});
  RelroTest<Elf, MoveOnlySegmentWrapper>({RO}, {C, D}, D);
  RelroTest<Elf, MoveOnlySegmentWrapper>({RO}, {C, DWZF}, DWZF);
  RelroTest<Elf, MoveOnlySegmentWrapper>({RO}, {C, ZF}, ZF);
}

TYPED_TEST(ElfldltlLoadTests, ApplyRelroMergeRight) {
  using Elf = typename TestFixture::Elf;

  RelroTest<Elf>({RO, C}, {C, C}, {}, false);
  RelroTest<Elf>({RO, C}, {C}, {}, true);
  RelroTest<Elf>({RO, C}, {C, D, C}, D);
  RelroTest<Elf>({RO, C}, {C, DWZF, C}, DWZF);
  RelroTest<Elf>({RO, C}, {C, ZF, C}, ZF);

  RelroTest<Elf>({RO, D}, {C, D}, {});
  RelroTest<Elf>({RO, D}, {C, D}, D);
  RelroTest<Elf>({RO, D}, {C, DWZF, D}, DWZF);
  RelroTest<Elf>({RO, D}, {C, ZF, D}, ZF);

  RelroTest<Elf>({RO, DWZF}, {C, DWZF}, {});
  RelroTest<Elf>({RO, DWZF}, {C, DWZF}, D);
  RelroTest<Elf>({RO, DWZF}, {C, DWZF, DWZF}, DWZF);
  RelroTest<Elf>({RO, DWZF}, {C, ZF, DWZF}, ZF);

  RelroTest<Elf>({RO, ZF}, {C, ZF}, {});
  RelroTest<Elf>({RO, ZF}, {C, DWZF}, D);
  // The following could be:
  // RelroTest<Elf>({RO, ZF}, {C, DWZF}, DWZF);
  // RelroTest<Elf>({RO, ZF}, {C, ZF}, ZF);
  // but we don't have Merge overloads for (*, ZF) because these are unlikely to exist in the wild.
  RelroTest<Elf>({RO, ZF}, {C, DWZF, ZF}, DWZF);
  RelroTest<Elf>({RO, ZF}, {C, ZF, ZF}, ZF);
}

TYPED_TEST(ElfldltlLoadTests, ApplyRelroMergeLeft) {
  using Elf = typename TestFixture::Elf;

  RelroTest<Elf>({C, RO}, {C, C}, {}, false);
  RelroTest<Elf>({C, RO}, {C}, {}, true);
}

TYPED_TEST(ElfldltlLoadTests, ApplyRelroMergeBoth) {
  using Elf = typename TestFixture::Elf;

  RelroTest<Elf>({C, RO, C}, {C, C, C}, {}, false);
  RelroTest<Elf>({C, RO, C}, {C}, {}, true);
}

template <class Segment>
class CantMergeSegmentWrapper : public Segment {
 public:
  using Segment::Segment;
  template <class Other>
  constexpr std::false_type CanMergeWith(const Other& other) const {
    return {};
  }
};

TYPED_TEST(ElfldltlLoadTests, ApplyRelroCantMergeSegmentWrapper) {
  using Elf = typename TestFixture::Elf;

  RelroTest<Elf, CantMergeSegmentWrapper>({RO, C}, {C, C}, {});
  RelroTest<Elf, CantMergeSegmentWrapper>({C, RO, C}, {C, C, C}, {});
  RelroTest<Elf, CantMergeSegmentWrapper>({C, RO}, {C, C}, {});
}

TYPED_TEST(ElfldltlLoadTests, ApplyRelroCantMerge) {
  using Elf = typename TestFixture::Elf;
  using Phdr = typename Elf::Phdr;

  auto diag = ExpectOkDiagnostics();

  Phdr phdrs[] = {
      {.type = elfldltl::ElfPhdrType::kLoad, .filesz = kPageSize, .memsz = kPageSize},
      {.type = elfldltl::ElfPhdrType::kLoad,
       .offset = kPageSize,
       .vaddr = kPageSize,
       .filesz = kPageSize,
       .memsz = kPageSize},
  };

  phdrs[0].flags = elfldltl::PhdrBase::kRead | elfldltl::PhdrBase::kExecute;
  phdrs[1].flags = elfldltl::PhdrBase::kRead | elfldltl::PhdrBase::kWrite;
  Phdr relro = {.type = elfldltl::ElfPhdrType::kRelro, .vaddr = kPageSize, .memsz = kPageSize};

  for (bool merge_ro : {true, false}) {
    elfldltl::LoadInfo<Elf, elfldltl::StdContainer<std::vector>::Container> load_info;
    using ConstantSegment = typename decltype(load_info)::ConstantSegment;

    EXPECT_TRUE(elfldltl::DecodePhdrs(diag, std::span<const Phdr>(phdrs),
                                      load_info.GetPhdrObserver(kPageSize)));
    auto& segments = load_info.segments();
    ASSERT_EQ(segments.size(), 2u);
    EXPECT_TRUE(load_info.ApplyRelro(diag, relro, kPageSize, merge_ro));
    ASSERT_EQ(segments.size(), 2u);
    ASSERT_TRUE(std::holds_alternative<ConstantSegment>(segments[0]));
    EXPECT_EQ(std::get<ConstantSegment>(segments[0]).flags(), phdrs[0].flags);
    ASSERT_TRUE(std::holds_alternative<ConstantSegment>(segments[1]));
    auto expected_flags = elfldltl::PhdrBase::kRead | (!merge_ro ? elfldltl::PhdrBase::kWrite : 0);
    EXPECT_EQ(std::get<ConstantSegment>(segments[1]).flags(), expected_flags);
  }
}

template <class Segment>
class CantReplaceSegmentWrapper : public Segment {
 public:
  using Segment::Segment;
  constexpr std::false_type CanReplace() const { return {}; }
};

TYPED_TEST(ElfldltlLoadTests, ApplyRelroCantReplaceSegmentWrapper) {
  using Elf = typename TestFixture::Elf;
  using Phdr = typename Elf::Phdr;

  Phdr phdrs[] = {
      {.type = elfldltl::ElfPhdrType::kLoad, .filesz = kPageSize, .memsz = kPageSize},
      {.type = elfldltl::ElfPhdrType::kLoad,
       .offset = kPageSize,
       .vaddr = kPageSize,
       .filesz = kPageSize,
       .memsz = kPageSize},
  };

  phdrs[0].flags = elfldltl::PhdrBase::kRead | elfldltl::PhdrBase::kExecute;
  phdrs[1].flags = elfldltl::PhdrBase::kRead | elfldltl::PhdrBase::kWrite;
  Phdr relro = {.type = elfldltl::ElfPhdrType::kRelro, .vaddr = kPageSize, .memsz = kPageSize};

  std::string error;
  auto diag = elfldltl::OneStringDiagnostics(error);

  RelroTestLoadInfo<Elf, CantReplaceSegmentWrapper> load_info;
  EXPECT_TRUE(elfldltl::DecodePhdrs(diag, std::span<const Phdr>(phdrs),
                                    load_info.GetPhdrObserver(kPageSize)));

  for (bool merge_ro : {true, false}) {
    EXPECT_FALSE(load_info.ApplyRelro(diag, relro, kPageSize, merge_ro));
    EXPECT_EQ(error, "Cannot split segment to apply PT_GNU_RELRO protections");
  }
}

TYPED_TEST(ElfldltlLoadTests, SymbolizerContext) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;
  using Phdr = typename Elf::Phdr;

  auto diag = ExpectOkDiagnostics();

  elfldltl::LoadInfo<Elf, elfldltl::StdContainer<std::vector>::Container> info;

  constexpr std::array kBuildId{std::byte{0x12}, std::byte{0x34}, std::byte{0xab}, std::byte{0xcd}};

  size_type offset = 0;
  for (uint32_t flags : (const uint32_t[]){
           Phdr::kRead,
           Phdr::kExecute,
           Phdr::kRead | Phdr::kWrite,
       }) {
    Phdr phdr = {
        .type = elfldltl::ElfPhdrType::kLoad,
        .offset = offset,
        .vaddr = offset,
        .filesz = kPageSize,
        .memsz = kPageSize,
    };
    phdr.flags = flags;
    offset += kPageSize;
    ASSERT_TRUE(info.AddSegment(diag, kPageSize, phdr));
  };

  constexpr char kExpectedContext[] = R"""(foo: {{{module:17:foo:elf:1234abcd}}}
foo: {{{mmap:0x12340000:0x1000:load:17:r:0x0}}}
foo: {{{mmap:0x12341000:0x1000:load:17:x:0x1000}}}
foo: {{{mmap:0x12342000:0x1000:load:17:rw:0x2000}}}
)""";

  std::string markup;
  symbolizer_markup::Writer writer([&markup](std::string_view str) { markup += str; });
  EXPECT_EQ(&writer,
            &(info.SymbolizerContext(writer, 17, "foo", std::span(kBuildId), 0x12340000, "foo")));

  EXPECT_EQ(kExpectedContext, markup);
}

TYPED_TEST(ElfldltlLoadTests, FindSegment) {
  using Elf = typename TestFixture::Elf;
  using Phdr = typename Elf::Phdr;
  using size_type = typename Elf::size_type;

  auto diag = ExpectOkDiagnostics();

  elfldltl::LoadInfo<Elf, elfldltl::StdContainer<std::vector>::Container> load_info;

  // Expect the first lookup to an empty segment list to return not found.
  ASSERT_TRUE(load_info.segments().empty());
  ASSERT_EQ(load_info.FindSegment(0u), load_info.segments().end());

  size_type offset = kPageSize;
  const std::array kPhdrs = {ConstantPhdr<Elf>{}(offset), DataPhdr<Elf>{}(offset),
                             ConstantPhdr<Elf>{}(offset)};
  // Load all segments first so we can search the segments container with
  // multiple entries.
  for (const Phdr& phdr : kPhdrs) {
    ASSERT_TRUE(load_info.AddSegment(diag, kPageSize, phdr));
  }

  ASSERT_EQ(load_info.segments().size(), 3u);

  // Test finding a segment from its starting vaddr.
  for (const Phdr& phdr : kPhdrs) {
    size_type vaddr = phdr.vaddr;
    const auto found = load_info.FindSegment(vaddr);
    ASSERT_NE(found, load_info.segments().end());
    ASSERT_TRUE(
        load_info.VisitSegment([vaddr](const auto& s) { return s.vaddr() == vaddr; }, *found));
  };

  // Test finding a segment from a vaddr in its vaddr range.
  for (const Phdr& phdr : kPhdrs) {
    size_type vaddr = phdr.vaddr + (phdr.memsz / 2);
    const auto found = load_info.FindSegment(vaddr);
    ASSERT_NE(found, load_info.segments().end());
    ASSERT_TRUE(load_info.VisitSegment(
        [vaddr](const auto& s) { return s.vaddr() < vaddr && vaddr < s.vaddr() + s.memsz(); },
        *found));
  };

  // Test finding a segment out of bounds of the first and last segments
  {
    size_type under_bounds_vaddr = kPhdrs[0].vaddr / 2;
    const auto found = load_info.FindSegment(under_bounds_vaddr);
    ASSERT_EQ(found, load_info.segments().end());
  }
  {
    size_type over_bounds_vaddr = kPhdrs[2].vaddr * 2;
    const auto found = load_info.FindSegment(over_bounds_vaddr);
    ASSERT_EQ(found, load_info.segments().end());
  }
}

}  // namespace
