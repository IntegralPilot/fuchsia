// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/container.h>
#include <lib/elfldltl/diagnostics.h>
#include <lib/elfldltl/dynamic.h>
#include <lib/elfldltl/machine.h>
#include <lib/elfldltl/memory.h>
#include <lib/elfldltl/testing/diagnostics.h>
#include <lib/elfldltl/testing/typed-test.h>

#include <vector>

#include "symbol-tests.h"

namespace {

using elfldltl::testing::ExpectedSingleError;
using elfldltl::testing::ExpectOkDiagnostics;

FORMAT_TYPED_TEST_SUITE(ElfldltlDynamicTests);

TYPED_TEST(ElfldltlDynamicTests, Empty) {
  using Elf = typename TestFixture::Elf;

  ExpectOkDiagnostics diag;

  elfldltl::DirectMemory memory({}, 0);

  // Nothing but the terminator.
  constexpr typename Elf::Dyn dyn[] = {
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  // No matchers and nothing to match.
  EXPECT_TRUE(elfldltl::DecodeDynamic(diag, memory, std::span(dyn)));
}

TYPED_TEST(ElfldltlDynamicTests, MissingTerminator) {
  using Elf = typename TestFixture::Elf;

  ExpectedSingleError diag{"missing DT_NULL terminator in PT_DYNAMIC"};

  elfldltl::DirectMemory memory({}, 0);

  // Empty span has no terminator.
  std::span<const typename Elf::Dyn> dyn;

  EXPECT_TRUE(elfldltl::DecodeDynamic(diag, memory, dyn));
}

TYPED_TEST(ElfldltlDynamicTests, RejectTextrel) {
  using Elf = typename TestFixture::Elf;

  elfldltl::DirectMemory memory({}, 0);

  {
    // PT_DYNAMIC without DT_TEXTREL.
    constexpr typename Elf::Dyn dyn_notextrel[] = {
        {.tag = elfldltl::ElfDynTag::kNull},
    };

    ExpectOkDiagnostics diag;
    EXPECT_TRUE(elfldltl::DecodeDynamic(diag, memory, std::span(dyn_notextrel),
                                        elfldltl::DynamicTextrelRejectObserver{}));
  }

  {
    // PT_DYNAMIC with DT_TEXTREL.
    constexpr typename Elf::Dyn dyn_textrel[] = {
        {.tag = elfldltl::ElfDynTag::kTextRel},
        {.tag = elfldltl::ElfDynTag::kNull},
    };

    elfldltl::testing::ExpectedSingleError expected{
        elfldltl::DynamicTextrelRejectObserver::kMessage,
    };

    EXPECT_TRUE(elfldltl::DecodeDynamic(expected, memory, std::span(dyn_textrel),
                                        elfldltl::DynamicTextrelRejectObserver{}));
  }

  {
    // PT_DYNAMIC with DF_TEXTREL.
    constexpr typename Elf::Dyn dyn_flags_textrel[] = {
        {
            .tag = elfldltl::ElfDynTag::kFlags,
            .val = elfldltl::ElfDynFlags::kTextRel | elfldltl::ElfDynFlags::kBindNow,
        },
        {.tag = elfldltl::ElfDynTag::kNull},
    };

    auto expected = elfldltl::testing::ExpectedSingleError{
        elfldltl::DynamicTextrelRejectObserver::kMessage,
    };
    EXPECT_TRUE(elfldltl::DecodeDynamic(expected, memory, std::span(dyn_flags_textrel),
                                        elfldltl::DynamicTextrelRejectObserver{}));
  }
}

TYPED_TEST(ElfldltlDynamicTests, RelocationInfoObserverEmpty) {
  using Elf = typename TestFixture::Elf;
  using Dyn = typename Elf::Dyn;

  ExpectOkDiagnostics diag;
  elfldltl::DirectMemory empty_memory({}, 0);

  // PT_DYNAMIC with no reloc info.
  constexpr Dyn dyn_noreloc[] = {
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  elfldltl::RelocationInfo<Elf> info;
  EXPECT_TRUE(elfldltl::DecodeDynamic(diag, empty_memory, std::span(dyn_noreloc),
                                      elfldltl::DynamicRelocationInfoObserver(info)));

  EXPECT_TRUE(info.rel_relative().empty());
  EXPECT_TRUE(info.rel_symbolic().empty());
  EXPECT_TRUE(info.rela_relative().empty());
  EXPECT_TRUE(info.rela_symbolic().empty());
  EXPECT_TRUE(info.relr().empty());
  std::visit([](const auto& table) { EXPECT_TRUE(table.empty()); }, info.jmprel());
}

// This synthesizes a memory image of relocation test data with known
// offsets and addresses that can be referenced in dynamic section entries in
// the specific test data.  The same image contents are used for several tests
// below with different dynamic section data.  Because the Memory API admits
// mutation of the image, the same image buffer shouldn't be reused for
// multiple tests just in case a test mutates the buffer (though they are meant
// not to).  So this helper object is created in each test case to reconstruct
// the same data afresh.
template <typename Elf>
class RelocInfoTestImage {
 public:
  using size_type = typename Elf::size_type;
  using Addr = typename Elf::Addr;
  using Dyn = typename Elf::Dyn;
  using Rel = typename Elf::Rel;
  using Rela = typename Elf::Rela;
  using Sym = typename Elf::Sym;

  static size_type size_bytes() { return sizeof(image_); }

  static size_type image_addr() { return kImageAddr; }

  static size_type rel_size_bytes() { return sizeof(image_.rel); }

  static size_type relent_size_bytes() { return sizeof(image_.rel[0]); }

  static size_type rela_size_bytes() { return sizeof(image_.rela); }

  static size_type relaent_size_bytes() { return sizeof(image_.rela[0]); }

  static size_type relr_size_bytes() { return sizeof(image_.relr); }

  static size_type relrent_size_bytes() { return sizeof(image_.relr[0]); }

  size_type rel_addr() const { return ImageAddr(image_.rel); }

  size_type rela_addr() const { return ImageAddr(image_.rela); }

  size_type relr_addr() const { return ImageAddr(image_.relr); }

  elfldltl::DirectMemory memory() { return elfldltl::DirectMemory(image_bytes(), kImageAddr); }

 private:
  // Build up some good relocation data in a memory image.

  static constexpr size_type kImageAddr = 0x123400;
  static constexpr auto kTestMachine = elfldltl::ElfMachine::kNone;
  using TestType = elfldltl::RelocationTraits<kTestMachine>::Type;
  static constexpr uint32_t kRelativeType = static_cast<uint32_t>(TestType::kRelative);
  static constexpr uint32_t kAbsoluteType = static_cast<uint32_t>(TestType::kAbsolute);

  template <typename T>
  size_type ImageAddr(const T& data) const {
    return static_cast<size_type>(reinterpret_cast<const std::byte*>(&data) -
                                  reinterpret_cast<const std::byte*>(&image_)) +
           kImageAddr;
  }

  struct ImageData {
    Rel rel[3] = {
        {8, kRelativeType},
        {24, kRelativeType},
        {4096, kAbsoluteType},
    };

    Rela rela[3] = {
        {8, kRelativeType, 0x11111111},
        {24, kRelativeType, 0x33333333},
        {4096, kAbsoluteType, 0x1234},
    };

    Addr relr[3] = {
        32,
        0x55555555,
        0xaaaaaaaa | 1,
    };
  } image_;

  std::span<std::byte> image_bytes() { return std::as_writable_bytes(std::span(&image_, 1)); }
};

TYPED_TEST(ElfldltlDynamicTests, RelocationInfoObserverFullValid) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;
  using Dyn = typename Elf::Dyn;

  ExpectOkDiagnostics diag;
  RelocInfoTestImage<Elf> test_image;

  // PT_DYNAMIC with full valid reloc info.

  const Dyn dyn_goodreloc[] = {
      {
          .tag = elfldltl::ElfDynTag::kRel,
          .val = static_cast<size_type>(test_image.rel_addr()),
      },
      {.tag = elfldltl::ElfDynTag::kRelSz, .val = test_image.rel_size_bytes()},
      {
          .tag = elfldltl::ElfDynTag::kRelEnt,
          .val = test_image.relent_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kRelCount, .val = 2},
      {
          .tag = elfldltl::ElfDynTag::kRela,
          .val = static_cast<size_type>(test_image.rela_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelaSz,
          .val = test_image.rela_size_bytes(),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelaEnt,
          .val = test_image.relaent_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kRelaCount, .val = 2},
      {
          .tag = elfldltl::ElfDynTag::kJmpRel,
          .val = static_cast<size_type>(test_image.rel_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kPltRelSz,
          .val = test_image.rel_size_bytes(),
      },
      {
          .tag = elfldltl::ElfDynTag::kPltRel,
          .val = static_cast<size_type>(elfldltl::ElfDynTag::kRel),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelr,
          .val = static_cast<size_type>(test_image.relr_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelrSz,
          .val = test_image.relr_size_bytes(),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelrEnt,
          .val = test_image.relrent_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  elfldltl::RelocationInfo<Elf> info;
  EXPECT_TRUE(elfldltl::DecodeDynamic(diag, test_image.memory(), std::span(dyn_goodreloc),
                                      elfldltl::DynamicRelocationInfoObserver(info)));

  EXPECT_EQ(2u, info.rel_relative().size());
  EXPECT_EQ(1u, info.rel_symbolic().size());
  EXPECT_EQ(2u, info.rela_relative().size());
  EXPECT_EQ(1u, info.rela_symbolic().size());
  EXPECT_EQ(3u, info.relr().size());
  std::visit([](const auto& table) { EXPECT_EQ(3u, table.size()); }, info.jmprel());
}

// We'll reuse that same image for the various error case tests.
// These cases only differ in their PT_DYNAMIC contents.

TYPED_TEST(ElfldltlDynamicTests, RelocationInfoObserverBadRelent) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;
  using Dyn = typename Elf::Dyn;

  ExpectedSingleError diag{"incorrect DT_RELENT value"};
  RelocInfoTestImage<Elf> test_image;

  const Dyn dyn_bad_relent[] = {
      {
          .tag = elfldltl::ElfDynTag::kRel,
          .val = static_cast<size_type>(test_image.rel_addr()),
      },
      {.tag = elfldltl::ElfDynTag::kRelSz, .val = test_image.rel_size_bytes()},
      {.tag = elfldltl::ElfDynTag::kRelEnt, .val = 17},  // Wrong size.
      {.tag = elfldltl::ElfDynTag::kRelCount, .val = 2},

      {
          .tag = elfldltl::ElfDynTag::kRela,
          .val = static_cast<size_type>(test_image.rela_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelaSz,
          .val = test_image.rela_size_bytes(),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelaEnt,
          .val = test_image.relaent_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kRelaCount, .val = 2},

      {
          .tag = elfldltl::ElfDynTag::kJmpRel,
          .val = static_cast<size_type>(test_image.rel_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kPltRelSz,
          .val = test_image.rel_size_bytes(),
      },
      {
          .tag = elfldltl::ElfDynTag::kPltRel,
          .val = static_cast<size_type>(elfldltl::ElfDynTag::kRel),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelr,
          .val = static_cast<size_type>(test_image.relr_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelrSz,
          .val = test_image.relr_size_bytes(),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelrEnt,
          .val = test_image.relrent_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  elfldltl::RelocationInfo<Elf> info;
  EXPECT_TRUE(elfldltl::DecodeDynamic(diag, test_image.memory(), std::span(dyn_bad_relent),
                                      elfldltl::DynamicRelocationInfoObserver(info)));

  // With keep-going, the data is delivered anyway.
  EXPECT_EQ(2u, info.rel_relative().size());
  EXPECT_EQ(1u, info.rel_symbolic().size());
  EXPECT_EQ(2u, info.rela_relative().size());
  EXPECT_EQ(1u, info.rela_symbolic().size());
  EXPECT_EQ(3u, info.relr().size());
  std::visit([](const auto& table) { EXPECT_EQ(3u, table.size()); }, info.jmprel());
}

TYPED_TEST(ElfldltlDynamicTests, RelocationInfoObserverBadRelaent) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;
  using Dyn = typename Elf::Dyn;

  ExpectedSingleError diag{"incorrect DT_RELAENT value"};
  RelocInfoTestImage<Elf> test_image;

  const Dyn dyn_bad_relaent[] = {
      {
          .tag = elfldltl::ElfDynTag::kRel,
          .val = static_cast<size_type>(test_image.rel_addr()),
      },
      {.tag = elfldltl::ElfDynTag::kRelSz, .val = test_image.rel_size_bytes()},
      {
          .tag = elfldltl::ElfDynTag::kRelEnt,
          .val = test_image.relent_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kRelCount, .val = 2},

      {
          .tag = elfldltl::ElfDynTag::kRela,
          .val = static_cast<size_type>(test_image.rela_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelaSz,
          .val = test_image.rela_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kRelaEnt, .val = 17},  // Wrong size.
      {.tag = elfldltl::ElfDynTag::kRelaCount, .val = 2},

      {
          .tag = elfldltl::ElfDynTag::kJmpRel,
          .val = static_cast<size_type>(test_image.rel_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kPltRelSz,
          .val = test_image.rel_size_bytes(),
      },
      {
          .tag = elfldltl::ElfDynTag::kPltRel,
          .val = static_cast<size_type>(elfldltl::ElfDynTag::kRel),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelr,
          .val = static_cast<size_type>(test_image.relr_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelrSz,
          .val = test_image.relr_size_bytes(),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelrEnt,
          .val = test_image.relrent_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  elfldltl::RelocationInfo<Elf> info;
  EXPECT_TRUE(elfldltl::DecodeDynamic(diag, test_image.memory(), std::span(dyn_bad_relaent),
                                      elfldltl::DynamicRelocationInfoObserver(info)));

  // With keep-going, the data is delivered anyway.
  EXPECT_EQ(2u, info.rel_relative().size());
  EXPECT_EQ(1u, info.rel_symbolic().size());
  EXPECT_EQ(2u, info.rela_relative().size());
  EXPECT_EQ(1u, info.rela_symbolic().size());
  EXPECT_EQ(3u, info.relr().size());
  std::visit([](const auto& table) { EXPECT_EQ(3u, table.size()); }, info.jmprel());
}

TYPED_TEST(ElfldltlDynamicTests, RelocationInfoObserverBadRelrent) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;
  using Dyn = typename Elf::Dyn;

  ExpectedSingleError diag{"incorrect DT_RELRENT value"};
  RelocInfoTestImage<Elf> test_image;

  const Dyn dyn_bad_relrent[] = {
      {
          .tag = elfldltl::ElfDynTag::kRel,
          .val = static_cast<size_type>(test_image.rel_addr()),
      },
      {.tag = elfldltl::ElfDynTag::kRelSz, .val = test_image.rel_size_bytes()},
      {
          .tag = elfldltl::ElfDynTag::kRelEnt,
          .val = test_image.relent_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kRelCount, .val = 2},

      {
          .tag = elfldltl::ElfDynTag::kRela,
          .val = static_cast<size_type>(test_image.rela_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelaSz,
          .val = test_image.rela_size_bytes(),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelaEnt,
          .val = test_image.relaent_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kRelaCount, .val = 2},

      {
          .tag = elfldltl::ElfDynTag::kJmpRel,
          .val = static_cast<size_type>(test_image.rel_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kPltRelSz,
          .val = test_image.rel_size_bytes(),
      },
      {
          .tag = elfldltl::ElfDynTag::kPltRel,
          .val = static_cast<size_type>(elfldltl::ElfDynTag::kRel),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelr,
          .val = static_cast<size_type>(test_image.relr_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelrSz,
          .val = test_image.relr_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kRelrEnt, .val = 3},  // Wrong size.
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  elfldltl::RelocationInfo<Elf> info;
  EXPECT_TRUE(elfldltl::DecodeDynamic(diag, test_image.memory(), std::span(dyn_bad_relrent),
                                      elfldltl::DynamicRelocationInfoObserver(info)));

  // With keep-going, the data is delivered anyway.
  EXPECT_EQ(2u, info.rel_relative().size());
  EXPECT_EQ(1u, info.rel_symbolic().size());
  EXPECT_EQ(2u, info.rela_relative().size());
  EXPECT_EQ(1u, info.rela_symbolic().size());
  EXPECT_EQ(3u, info.relr().size());
  std::visit([](const auto& table) { EXPECT_EQ(3u, table.size()); }, info.jmprel());
}

TYPED_TEST(ElfldltlDynamicTests, RelocationInfoObserverMissingPltrel) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;
  using Dyn = typename Elf::Dyn;

  ExpectedSingleError diag{"invalid DT_PLTREL entry"};
  RelocInfoTestImage<Elf> test_image;

  const Dyn dyn_missing_pltrel[] = {
      {
          .tag = elfldltl::ElfDynTag::kRel,
          .val = static_cast<size_type>(test_image.rel_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelSz,
          .val = test_image.rel_size_bytes(),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelEnt,
          .val = test_image.relent_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kRelCount, .val = 2},
      {
          .tag = elfldltl::ElfDynTag::kRela,
          .val = static_cast<size_type>(test_image.rela_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelaSz,
          .val = test_image.rela_size_bytes(),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelaEnt,
          .val = test_image.relaent_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kRelaCount, .val = 2},
      {
          .tag = elfldltl::ElfDynTag::kJmpRel,
          .val = static_cast<size_type>(test_image.rel_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kPltRelSz,
          .val = test_image.rel_size_bytes(),
      },
      // Missing DT_PLTREL.
      {
          .tag = elfldltl::ElfDynTag::kRelr,
          .val = static_cast<size_type>(test_image.relr_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelrSz,
          .val = test_image.relr_size_bytes(),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelrEnt,
          .val = test_image.relrent_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  elfldltl::RelocationInfo<Elf> info;
  EXPECT_TRUE(elfldltl::DecodeDynamic(diag, test_image.memory(), std::span(dyn_missing_pltrel),
                                      elfldltl::DynamicRelocationInfoObserver(info)));

  // DT_JMPREL was ignored but the rest is normal.
  EXPECT_EQ(2u, info.rel_relative().size());
  EXPECT_EQ(1u, info.rel_symbolic().size());
  EXPECT_EQ(2u, info.rela_relative().size());
  EXPECT_EQ(1u, info.rela_symbolic().size());
  EXPECT_EQ(3u, info.relr().size());
  std::visit([](const auto& table) { EXPECT_EQ(0u, table.size()); }, info.jmprel());
}

TYPED_TEST(ElfldltlDynamicTests, RelocationInfoObserverBadPltrel) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;
  using Dyn = typename Elf::Dyn;

  ExpectedSingleError diag{"missing DT_PLTREL entry"};
  RelocInfoTestImage<Elf> test_image;

  const Dyn dyn_bad_pltrel[] = {
      {
          .tag = elfldltl::ElfDynTag::kRel,
          .val = static_cast<size_type>(test_image.rel_addr()),
      },
      {.tag = elfldltl::ElfDynTag::kRelSz, .val = test_image.rel_size_bytes()},
      {
          .tag = elfldltl::ElfDynTag::kRelEnt,
          .val = test_image.relent_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kRelCount, .val = 2},
      {
          .tag = elfldltl::ElfDynTag::kRela,
          .val = static_cast<size_type>(test_image.rela_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelaSz,
          .val = test_image.rela_size_bytes(),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelaEnt,
          .val = test_image.relaent_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kRelaCount, .val = 2},
      {
          .tag = elfldltl::ElfDynTag::kJmpRel,
          .val = static_cast<size_type>(test_image.rel_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kPltRelSz,
          .val = test_image.rel_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kPltRel, .val = 0},  // Invalid value.
      {
          .tag = elfldltl::ElfDynTag::kRelr,
          .val = static_cast<size_type>(test_image.relr_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelrSz,
          .val = test_image.relr_size_bytes(),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelrEnt,
          .val = test_image.relrent_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  elfldltl::RelocationInfo<Elf> info;
  EXPECT_TRUE(elfldltl::DecodeDynamic(diag, test_image.memory(), std::span(dyn_bad_pltrel),
                                      elfldltl::DynamicRelocationInfoObserver(info)));

  // DT_JMPREL was ignored but the rest is normal.
  EXPECT_EQ(2u, info.rel_relative().size());
  EXPECT_EQ(1u, info.rel_symbolic().size());
  EXPECT_EQ(2u, info.rela_relative().size());
  EXPECT_EQ(1u, info.rela_symbolic().size());
  EXPECT_EQ(3u, info.relr().size());
  std::visit([](const auto& table) { EXPECT_EQ(0u, table.size()); }, info.jmprel());
}

// The bad address, size, and alignment cases are all the same template code
// paths for each table so we only test DT_REL to stand in for the rest.

TYPED_TEST(ElfldltlDynamicTests, RelocationInfoObserverBadRelAddr) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;
  using Dyn = typename Elf::Dyn;

  ExpectedSingleError diag{"DT_REL has misaligned address"};
  RelocInfoTestImage<Elf> test_image;

  const Dyn dyn_bad_rel_addr[] = {
      {
          .tag = elfldltl::ElfDynTag::kRel,
          // This is an invalid address, before the image starts.
          .val = test_image.image_addr() - 1,
      },
      {.tag = elfldltl::ElfDynTag::kRelSz, .val = test_image.rel_size_bytes()},
      {
          .tag = elfldltl::ElfDynTag::kRelEnt,
          .val = test_image.relent_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kRelCount, .val = 2},
      {
          .tag = elfldltl::ElfDynTag::kRela,
          .val = static_cast<size_type>(test_image.rela_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelaSz,
          .val = test_image.rela_size_bytes(),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelaEnt,
          .val = test_image.relaent_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kRelaCount, .val = 2},
      {
          .tag = elfldltl::ElfDynTag::kJmpRel,
          .val = static_cast<size_type>(test_image.rel_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kPltRelSz,
          .val = test_image.rel_size_bytes(),
      },
      {
          .tag = elfldltl::ElfDynTag::kPltRel,
          .val = static_cast<size_type>(elfldltl::ElfDynTag::kRel),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelr,
          .val = static_cast<size_type>(test_image.relr_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelrSz,
          .val = test_image.relr_size_bytes(),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelrEnt,
          .val = test_image.relrent_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  elfldltl::RelocationInfo<Elf> info;
  EXPECT_TRUE(elfldltl::DecodeDynamic(diag, test_image.memory(), std::span(dyn_bad_rel_addr),
                                      elfldltl::DynamicRelocationInfoObserver(info)));

  // DT_REL was ignored but the rest is normal.
  EXPECT_EQ(0u, info.rel_relative().size());
  EXPECT_EQ(0u, info.rel_symbolic().size());
  EXPECT_EQ(2u, info.rela_relative().size());
  EXPECT_EQ(1u, info.rela_symbolic().size());
  EXPECT_EQ(3u, info.relr().size());
  std::visit([](const auto& table) { EXPECT_EQ(3u, table.size()); }, info.jmprel());
}

TYPED_TEST(ElfldltlDynamicTests, RelocationInfoObserverBadRelSz) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;
  using Dyn = typename Elf::Dyn;

  ExpectedSingleError diag{"DT_RELSZ not a multiple of DT_REL entry size"};
  RelocInfoTestImage<Elf> test_image;

  const Dyn dyn_bad_relsz[] = {
      {.tag = elfldltl::ElfDynTag::kRel, .val = test_image.rel_addr()},
      {
          .tag = elfldltl::ElfDynTag::kRelSz,
          // This is an invalid size, bigger than the whole image.
          .val = test_image.size_bytes() + 1,
      },
      {
          .tag = elfldltl::ElfDynTag::kRelEnt,
          .val = test_image.relent_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kRelCount, .val = 2},
      {
          .tag = elfldltl::ElfDynTag::kRela,
          .val = static_cast<size_type>(test_image.rela_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelaSz,
          .val = test_image.rela_size_bytes(),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelaEnt,
          .val = test_image.relaent_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kRelaCount, .val = 2},
      {
          .tag = elfldltl::ElfDynTag::kJmpRel,
          .val = static_cast<size_type>(test_image.rel_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kPltRelSz,
          .val = test_image.rel_size_bytes(),
      },
      {
          .tag = elfldltl::ElfDynTag::kPltRel,
          .val = static_cast<size_type>(elfldltl::ElfDynTag::kRel),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelr,
          .val = static_cast<size_type>(test_image.relr_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelrSz,
          .val = test_image.relr_size_bytes(),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelrEnt,
          .val = test_image.relrent_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  elfldltl::RelocationInfo<Elf> info;
  EXPECT_TRUE(elfldltl::DecodeDynamic(diag, test_image.memory(), std::span(dyn_bad_relsz),
                                      elfldltl::DynamicRelocationInfoObserver(info)));

  // DT_REL was ignored but the rest is normal.
  EXPECT_EQ(0u, info.rel_relative().size());
  EXPECT_EQ(0u, info.rel_symbolic().size());
  EXPECT_EQ(2u, info.rela_relative().size());
  EXPECT_EQ(1u, info.rela_symbolic().size());
  EXPECT_EQ(3u, info.relr().size());
  std::visit([](const auto& table) { EXPECT_EQ(3u, table.size()); }, info.jmprel());
}

TYPED_TEST(ElfldltlDynamicTests, RelocationInfoObserverBadRelSzAlign) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;
  using Dyn = typename Elf::Dyn;

  ExpectedSingleError diag{"DT_RELSZ not a multiple of DT_REL entry size"};
  RelocInfoTestImage<Elf> test_image;

  const Dyn dyn_bad_relsz_align[] = {
      {.tag = elfldltl::ElfDynTag::kRel, .val = test_image.rel_addr()},
      {
          .tag = elfldltl::ElfDynTag::kRelSz,
          // This size is not a multiple of the entry size.
          .val = test_image.rel_size_bytes() - 3,
      },
      {
          .tag = elfldltl::ElfDynTag::kRelEnt,
          .val = test_image.relent_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kRelCount, .val = 2},
      {
          .tag = elfldltl::ElfDynTag::kRela,
          .val = static_cast<size_type>(test_image.rela_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelaSz,
          .val = test_image.rela_size_bytes(),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelaEnt,
          .val = test_image.relaent_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kRelaCount, .val = 2},
      {
          .tag = elfldltl::ElfDynTag::kJmpRel,
          .val = static_cast<size_type>(test_image.rel_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kPltRelSz,
          .val = test_image.rel_size_bytes(),
      },
      {
          .tag = elfldltl::ElfDynTag::kPltRel,
          .val = static_cast<size_type>(elfldltl::ElfDynTag::kRel),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelr,
          .val = static_cast<size_type>(test_image.relr_addr()),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelrSz,
          .val = test_image.relr_size_bytes(),
      },
      {
          .tag = elfldltl::ElfDynTag::kRelrEnt,
          .val = test_image.relrent_size_bytes(),
      },
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  elfldltl::RelocationInfo<Elf> info;
  EXPECT_TRUE(elfldltl::DecodeDynamic(diag, test_image.memory(), std::span(dyn_bad_relsz_align),
                                      elfldltl::DynamicRelocationInfoObserver(info)));

  // DT_REL was ignored but the rest is normal.
  EXPECT_EQ(0u, info.rel_relative().size());
  EXPECT_EQ(0u, info.rel_symbolic().size());
  EXPECT_EQ(2u, info.rela_relative().size());
  EXPECT_EQ(1u, info.rela_symbolic().size());
  EXPECT_EQ(3u, info.relr().size());
  std::visit([](const auto& table) { EXPECT_EQ(3u, table.size()); }, info.jmprel());
}

// This synthesizes a memory image of symbol-related test data with known
// offsets and addresses that can be referenced in dynamic section entries in
// the specific test data.  The same image contents are used for several tests
// below with different dynamic section data.  Because the Memory API admits
// mutation of the image, the same image buffer shouldn't be reused for
// multiple tests just in case a test mutates the buffer (though they are meant
// not to).  So this helper object is created in each test case to reconstruct
// the same data afresh.
template <typename Elf>
class SymbolInfoTestImage {
 public:
  using size_type = typename Elf::size_type;
  using Dyn = typename Elf::Dyn;
  using Sym = typename Elf::Sym;

  SymbolInfoTestImage() {
    // Build up some good symbol data in a memory image.
    soname_offset_ = test_syms_.AddString("libfoo.so");

    auto symtab_bytes = std::as_bytes(test_syms_.symtab());
    std::span<const std::byte> strtab_bytes{
        reinterpret_cast<const std::byte*>(test_syms_.strtab().data()),
        test_syms_.strtab().size(),
    };

    image_ = std::vector<std::byte>(symtab_bytes.begin(), symtab_bytes.end());
    auto next_addr = [this]() -> size_type {
      size_t align_pad = sizeof(size_type) - (image_.size() % sizeof(size_type));
      image_.insert(image_.end(), align_pad, std::byte{});
      return kSymtabAddr + static_cast<size_type>(image_.size());
    };

    strtab_addr_ = next_addr();
    image_.insert(image_.end(), strtab_bytes.begin(), strtab_bytes.end());

    gnu_hash_addr_ = next_addr();
    auto gnu_hash_data = std::span(kTestGnuHash<typename Elf::Addr>);
    auto gnu_hash_bytes = std::as_bytes(gnu_hash_data);
    image_.insert(image_.end(), gnu_hash_bytes.begin(), gnu_hash_bytes.end());

    hash_addr_ = next_addr();
    auto hash_data = std::span(kTestCompatHash<typename Elf::Word>);
    auto hash_bytes = std::as_bytes(hash_data);
    image_.insert(image_.end(), hash_bytes.begin(), hash_bytes.end());
  }

  size_type soname_offset() const { return soname_offset_; }

  size_type strtab_addr() const { return strtab_addr_; }

  size_t strtab_size_bytes() const { return test_syms_.strtab().size(); }

  size_type symtab_addr() { return kSymtabAddr; }

  size_type hash_addr() const { return hash_addr_; }

  size_type gnu_hash_addr() const { return gnu_hash_addr_; }

  const TestSymtab<Elf>& test_syms() const { return test_syms_; }

  size_t size_bytes() const { return image_.size(); }

  elfldltl::DirectMemory memory() { return elfldltl::DirectMemory(image_, kSymtabAddr); }

 private:
  static constexpr size_type kSymtabAddr = 0x1000;

  std::vector<std::byte> image_;
  TestSymtab<Elf> test_syms_ = kTestSymbols<Elf>;
  size_type soname_offset_ = 0;
  size_type strtab_addr_ = 0;
  size_type hash_addr_ = 0;
  size_type gnu_hash_addr_ = 0;
};

TYPED_TEST(ElfldltlDynamicTests, SymbolInfoObserverEmpty) {
  using Elf = typename TestFixture::Elf;
  using Dyn = typename Elf::Dyn;

  ExpectOkDiagnostics diag;
  elfldltl::DirectMemory empty_memory({}, 0);

  // PT_DYNAMIC with no symbol info.
  constexpr Dyn dyn_nosyms[] = {
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  elfldltl::SymbolInfo<Elf> info;
  EXPECT_TRUE(elfldltl::DecodeDynamic(diag, empty_memory, std::span(dyn_nosyms),
                                      elfldltl::DynamicSymbolInfoObserver(info)));

  EXPECT_EQ(info.strtab().size(), 1u);
  EXPECT_TRUE(info.symtab().empty());
  EXPECT_TRUE(info.soname().empty());
  EXPECT_FALSE(info.compat_hash());
  EXPECT_FALSE(info.gnu_hash());
}

TYPED_TEST(ElfldltlDynamicTests, SymbolInfoObserverFullValid) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;
  using Dyn = typename Elf::Dyn;
  using Sym = typename Elf::Sym;

  ExpectOkDiagnostics diag;
  SymbolInfoTestImage<Elf> test_image;

  constexpr uint32_t kDynFlags =
      elfldltl::ElfDynFlags::kBindNow | elfldltl::ElfDynFlags::kStaticTls;
  constexpr uint32_t kDynFlags1 = 0x3;

  // PT_DYNAMIC with full valid symbol info.
  const Dyn dyn_goodsyms[] = {
      {.tag = elfldltl::ElfDynTag::kSoname, .val = test_image.soname_offset()},
      {.tag = elfldltl::ElfDynTag::kSymTab, .val = test_image.symtab_addr()},
      {.tag = elfldltl::ElfDynTag::kSymEnt, .val = sizeof(Sym)},
      {.tag = elfldltl::ElfDynTag::kStrTab, .val = test_image.strtab_addr()},
      {
          .tag = elfldltl::ElfDynTag::kStrSz,
          .val = static_cast<size_type>(test_image.strtab_size_bytes()),
      },
      {.tag = elfldltl::ElfDynTag::kHash, .val = test_image.hash_addr()},
      {
          .tag = elfldltl::ElfDynTag::kGnuHash,
          .val = test_image.gnu_hash_addr(),
      },
      {.tag = elfldltl::ElfDynTag::kFlags, .val = kDynFlags},
      {.tag = elfldltl::ElfDynTag::kFlags1, .val = kDynFlags1},
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  elfldltl::SymbolInfo<Elf> info;
  EXPECT_TRUE(elfldltl::DecodeDynamic(diag, test_image.memory(), std::span(dyn_goodsyms),
                                      elfldltl::DynamicSymbolInfoObserver(info)));

  EXPECT_EQ(info.strtab().size(), test_image.test_syms().strtab().size());
  EXPECT_EQ(info.strtab(), test_image.test_syms().strtab());
  EXPECT_EQ(info.safe_symtab().size(), test_image.test_syms().symtab().size());
  EXPECT_EQ(info.soname(), "libfoo.so");
  EXPECT_TRUE(info.compat_hash());
  EXPECT_TRUE(info.gnu_hash());
  EXPECT_EQ(info.flags(), kDynFlags);
  EXPECT_EQ(info.flags1(), kDynFlags1);
}

// We'll reuse that same image for the various error case tests.
// These cases only differ in their PT_DYNAMIC contents.

TYPED_TEST(ElfldltlDynamicTests, SymbolInfoObserverBadSonameOffset) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;
  using Dyn = typename Elf::Dyn;
  using Sym = typename Elf::Sym;

  ExpectedSingleError diag{"DT_SONAME does not fit in DT_STRTAB"};
  SymbolInfoTestImage<Elf> test_image;
  elfldltl::DirectMemory image_memory = test_image.memory();

  const Dyn dyn_bad_soname_offset[] = {
      {
          .tag = elfldltl::ElfDynTag::kSoname,
          // This is an invalid string table offset.
          .val = static_cast<size_type>(test_image.test_syms().strtab().size()),
      },
      {.tag = elfldltl::ElfDynTag::kSymTab, .val = test_image.symtab_addr()},
      {.tag = elfldltl::ElfDynTag::kSymEnt, .val = sizeof(Sym)},
      {.tag = elfldltl::ElfDynTag::kStrTab, .val = test_image.strtab_addr()},
      {
          .tag = elfldltl::ElfDynTag::kStrSz,
          .val = static_cast<size_type>(test_image.strtab_size_bytes()),
      },
      {.tag = elfldltl::ElfDynTag::kHash, .val = test_image.hash_addr()},
      {.tag = elfldltl::ElfDynTag::kGnuHash, .val = test_image.gnu_hash_addr()},
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  elfldltl::SymbolInfo<Elf> info;
  EXPECT_TRUE(elfldltl::DecodeDynamic(diag, image_memory, std::span(dyn_bad_soname_offset),
                                      elfldltl::DynamicSymbolInfoObserver(info)));
}

TYPED_TEST(ElfldltlDynamicTests, SymbolInfoObserverBadSyment) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;
  using Dyn = typename Elf::Dyn;

  ExpectedSingleError diag{"incorrect DT_SYMENT value ", 17};
  SymbolInfoTestImage<Elf> test_image;
  elfldltl::DirectMemory image_memory = test_image.memory();

  const Dyn dyn_bad_syment[] = {
      {.tag = elfldltl::ElfDynTag::kSoname, .val = test_image.soname_offset()},
      {.tag = elfldltl::ElfDynTag::kSymTab, .val = test_image.symtab_addr()},
      {.tag = elfldltl::ElfDynTag::kSymEnt, .val = 17},  // Wrong size.
      {.tag = elfldltl::ElfDynTag::kStrTab, .val = test_image.strtab_addr()},
      {
          .tag = elfldltl::ElfDynTag::kStrSz,
          .val = static_cast<size_type>(test_image.strtab_size_bytes()),
      },
      {.tag = elfldltl::ElfDynTag::kHash, .val = test_image.hash_addr()},
      {
          .tag = elfldltl::ElfDynTag::kGnuHash,
          .val = test_image.gnu_hash_addr(),
      },
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  elfldltl::SymbolInfo<Elf> info;
  EXPECT_TRUE(elfldltl::DecodeDynamic(diag, image_memory, std::span(dyn_bad_syment),
                                      elfldltl::DynamicSymbolInfoObserver(info)));
}

TYPED_TEST(ElfldltlDynamicTests, SymbolInfoObserverMissingStrsz) {
  using Elf = typename TestFixture::Elf;
  using Dyn = typename Elf::Dyn;
  using Sym = typename Elf::Sym;

  ExpectedSingleError diag{"DT_STRTAB without DT_STRSZ"};
  SymbolInfoTestImage<Elf> test_image;
  elfldltl::DirectMemory image_memory = test_image.memory();

  const Dyn dyn_missing_strsz[] = {
      {.tag = elfldltl::ElfDynTag::kSymTab, .val = test_image.symtab_addr()},
      {.tag = elfldltl::ElfDynTag::kSymEnt, .val = sizeof(Sym)},
      {.tag = elfldltl::ElfDynTag::kStrTab, .val = test_image.strtab_addr()},
      // DT_STRSZ omitted with DT_STRTAB present.
      {.tag = elfldltl::ElfDynTag::kHash, .val = test_image.hash_addr()},
      {
          .tag = elfldltl::ElfDynTag::kGnuHash,
          .val = test_image.gnu_hash_addr(),
      },
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  elfldltl::SymbolInfo<Elf> info;
  EXPECT_TRUE(elfldltl::DecodeDynamic(diag, image_memory, std::span(dyn_missing_strsz),
                                      elfldltl::DynamicSymbolInfoObserver(info)));
}

TYPED_TEST(ElfldltlDynamicTests, SymbolInfoObserverMissingStrtab) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;
  using Dyn = typename Elf::Dyn;
  using Sym = typename Elf::Sym;

  ExpectedSingleError diag{"DT_STRSZ without DT_STRTAB"};
  SymbolInfoTestImage<Elf> test_image;
  elfldltl::DirectMemory image_memory = test_image.memory();

  const Dyn dyn_missing_strtab[] = {
      {.tag = elfldltl::ElfDynTag::kSymTab, .val = test_image.symtab_addr()},
      // DT_STRTAB omitted with DT_STRSZ present.
      {
          .tag = elfldltl::ElfDynTag::kStrSz,
          .val = static_cast<size_type>(test_image.strtab_size_bytes()),
      },
      {.tag = elfldltl::ElfDynTag::kSymEnt, .val = sizeof(Sym)},
      {.tag = elfldltl::ElfDynTag::kHash, .val = test_image.hash_addr()},
      {
          .tag = elfldltl::ElfDynTag::kGnuHash,
          .val = test_image.gnu_hash_addr(),
      },
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  elfldltl::SymbolInfo<Elf> info;
  EXPECT_TRUE(elfldltl::DecodeDynamic(diag, image_memory, std::span(dyn_missing_strtab),
                                      elfldltl::DynamicSymbolInfoObserver(info)));
}

TYPED_TEST(ElfldltlDynamicTests, SymbolInfoObserverBadStrtabAddr) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;
  using Dyn = typename Elf::Dyn;
  using Sym = typename Elf::Sym;

  ExpectedSingleError diag{
      "invalid address in DT_STRTAB or invalid size in DT_STRSZ",
  };
  SymbolInfoTestImage<Elf> test_image;
  elfldltl::DirectMemory image_memory = test_image.memory();

  const Dyn dyn_bad_strtab_addr[] = {
      {.tag = elfldltl::ElfDynTag::kSymTab, .val = test_image.symtab_addr()},
      {.tag = elfldltl::ElfDynTag::kSymEnt, .val = sizeof(Sym)},
      // This is an invalid address, before the image start.
      {
          .tag = elfldltl::ElfDynTag::kStrTab,
          .val = test_image.symtab_addr() - 1,
      },
      {
          .tag = elfldltl::ElfDynTag::kStrSz,
          .val = static_cast<size_type>(test_image.strtab_size_bytes()),
      },
      {.tag = elfldltl::ElfDynTag::kHash, .val = test_image.hash_addr()},
      {
          .tag = elfldltl::ElfDynTag::kGnuHash,
          .val = test_image.gnu_hash_addr(),
      },
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  elfldltl::SymbolInfo<Elf> info;
  EXPECT_TRUE(elfldltl::DecodeDynamic(diag, image_memory, std::span(dyn_bad_strtab_addr),
                                      elfldltl::DynamicSymbolInfoObserver(info)));
}

TYPED_TEST(ElfldltlDynamicTests, SymbolInfoObserverBadSymtabAddr) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;
  using Dyn = typename Elf::Dyn;
  using Sym = typename Elf::Sym;

  ExpectOkDiagnostics diag;
  SymbolInfoTestImage<Elf> test_image;
  elfldltl::DirectMemory image_memory = test_image.memory();

  // Since the symtab has no known bounds, bad addresses are only diagnosed via
  // the memory object and cause hard failure, not via the diag object where
  // keep_going causes success return.
  const Dyn dyn_bad_symtab_addr[] = {
      {.tag = elfldltl::ElfDynTag::kSoname, .val = test_image.soname_offset()},
      {
          .tag = elfldltl::ElfDynTag::kSymTab,
          // This is an invalid address, past the image end.
          .val = static_cast<size_type>(test_image.symtab_addr() + test_image.size_bytes()),
      },
      {.tag = elfldltl::ElfDynTag::kSymEnt, .val = sizeof(Sym)},
      {.tag = elfldltl::ElfDynTag::kStrTab, .val = test_image.strtab_addr()},
      {
          .tag = elfldltl::ElfDynTag::kStrSz,
          .val = static_cast<size_type>(test_image.strtab_size_bytes()),
      },
      {.tag = elfldltl::ElfDynTag::kHash, .val = test_image.hash_addr()},
      {
          .tag = elfldltl::ElfDynTag::kGnuHash,
          .val = test_image.gnu_hash_addr(),
      },
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  elfldltl::SymbolInfo<Elf> info;
  EXPECT_FALSE(elfldltl::DecodeDynamic(diag, image_memory, std::span(dyn_bad_symtab_addr),
                                       elfldltl::DynamicSymbolInfoObserver(info)));
}

TYPED_TEST(ElfldltlDynamicTests, SymbolInfoObserverBadSymtabAlign) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;
  using Dyn = typename Elf::Dyn;
  using Sym = typename Elf::Sym;

  ExpectedSingleError diag{"DT_SYMTAB has misaligned address"};
  SymbolInfoTestImage<Elf> test_image;
  elfldltl::DirectMemory image_memory = test_image.memory();

  // A misaligned symtab becomes a hard failure after diagnosis because it's
  // treated like a memory failure in addition to the diagnosed error.
  const Dyn dyn_bad_symtab_align[] = {
      {.tag = elfldltl::ElfDynTag::kSoname, .val = test_image.soname_offset()},
      {
          .tag = elfldltl::ElfDynTag::kSymTab,
          // This is misaligned vs alignof(Sym).
          .val = test_image.symtab_addr() + 2,
      },
      {.tag = elfldltl::ElfDynTag::kSymEnt, .val = sizeof(Sym)},
      {.tag = elfldltl::ElfDynTag::kStrTab, .val = test_image.strtab_addr()},
      {
          .tag = elfldltl::ElfDynTag::kStrSz,
          .val = static_cast<size_type>(test_image.strtab_size_bytes()),
      },
      {.tag = elfldltl::ElfDynTag::kHash, .val = test_image.hash_addr()},
      {
          .tag = elfldltl::ElfDynTag::kGnuHash,
          .val = test_image.gnu_hash_addr(),
      },
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  elfldltl::SymbolInfo<Elf> info;
  EXPECT_FALSE(elfldltl::DecodeDynamic(diag, image_memory, std::span(dyn_bad_symtab_align),
                                       elfldltl::DynamicSymbolInfoObserver(info)));
}

TYPED_TEST(ElfldltlDynamicTests, SymbolInfoObserverBadHashAddr) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;
  using Dyn = typename Elf::Dyn;
  using Sym = typename Elf::Sym;

  ExpectOkDiagnostics diag;
  SymbolInfoTestImage<Elf> test_image;
  elfldltl::DirectMemory image_memory = test_image.memory();

  // Since DT_HASH has no known bounds, bad addresses are only diagnosed via
  // the memory object and cause hard failure, not via the diag object where
  // keep_going causes success return.
  const Dyn dyn_bad_hash_addr[] = {
      {.tag = elfldltl::ElfDynTag::kSoname, .val = test_image.soname_offset()},
      {.tag = elfldltl::ElfDynTag::kSymTab, .val = test_image.symtab_addr()},
      {.tag = elfldltl::ElfDynTag::kSymEnt, .val = sizeof(Sym)},
      {.tag = elfldltl::ElfDynTag::kStrTab, .val = test_image.strtab_addr()},
      {
          .tag = elfldltl::ElfDynTag::kStrSz,
          .val = static_cast<size_type>(test_image.strtab_size_bytes()),
      },
      {
          .tag = elfldltl::ElfDynTag::kHash,
          // This is an invalid address, past the image end.
          .val = static_cast<size_type>(test_image.symtab_addr() + test_image.size_bytes()),
      },
      {
          .tag = elfldltl::ElfDynTag::kGnuHash,
          .val = test_image.gnu_hash_addr(),
      },
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  elfldltl::SymbolInfo<Elf> info;
  EXPECT_FALSE(elfldltl::DecodeDynamic(diag, image_memory, std::span(dyn_bad_hash_addr),
                                       elfldltl::DynamicSymbolInfoObserver(info)));
}

TYPED_TEST(ElfldltlDynamicTests, SymbolInfoObserverBadHashAlign) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;
  using Dyn = typename Elf::Dyn;
  using Sym = typename Elf::Sym;

  ExpectedSingleError diag{"DT_HASH has misaligned address"};
  SymbolInfoTestImage<Elf> test_image;
  elfldltl::DirectMemory image_memory = test_image.memory();

  const Dyn dyn_bad_hash_align[] = {
      {.tag = elfldltl::ElfDynTag::kSoname, .val = test_image.soname_offset()},
      {.tag = elfldltl::ElfDynTag::kSymTab, .val = test_image.symtab_addr()},
      {.tag = elfldltl::ElfDynTag::kSymEnt, .val = sizeof(Sym)},
      {.tag = elfldltl::ElfDynTag::kStrTab, .val = test_image.strtab_addr()},
      {
          .tag = elfldltl::ElfDynTag::kStrSz,
          .val = static_cast<size_type>(test_image.strtab_size_bytes()),
      },
      {
          .tag = elfldltl::ElfDynTag::kHash,
          // This is misaligned vs alignof(Word).
          .val = test_image.hash_addr() + 2,
      },
      {
          .tag = elfldltl::ElfDynTag::kGnuHash,
          .val = test_image.gnu_hash_addr(),
      },
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  elfldltl::SymbolInfo<Elf> info;
  EXPECT_TRUE(elfldltl::DecodeDynamic(diag, image_memory, std::span(dyn_bad_hash_align),
                                      elfldltl::DynamicSymbolInfoObserver(info)));
}

TYPED_TEST(ElfldltlDynamicTests, SymbolInfoObserverBadGnuHashAddr) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;
  using Dyn = typename Elf::Dyn;
  using Sym = typename Elf::Sym;

  ExpectOkDiagnostics diag;
  SymbolInfoTestImage<Elf> test_image;
  elfldltl::DirectMemory image_memory = test_image.memory();

  // Since DT_GNU_HASH has no known bounds, bad addresses are only diagnosed
  // via the memory object and cause hard failure, not via the diag object
  // where keep_going causes success return.
  const Dyn dyn_bad_gnu_hash_addr[] = {
      {.tag = elfldltl::ElfDynTag::kSoname, .val = test_image.soname_offset()},
      {.tag = elfldltl::ElfDynTag::kSymTab, .val = test_image.symtab_addr()},
      {.tag = elfldltl::ElfDynTag::kSymEnt, .val = sizeof(Sym)},
      {.tag = elfldltl::ElfDynTag::kStrTab, .val = test_image.strtab_addr()},
      {
          .tag = elfldltl::ElfDynTag::kStrSz,
          .val = static_cast<size_type>(test_image.strtab_size_bytes()),
      },
      {.tag = elfldltl::ElfDynTag::kHash, .val = test_image.hash_addr()},
      {
          .tag = elfldltl::ElfDynTag::kGnuHash,
          // This is an invalid address, past the image end.
          .val = static_cast<size_type>(test_image.symtab_addr() + test_image.size_bytes()),
      },
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  elfldltl::SymbolInfo<Elf> info;
  EXPECT_FALSE(elfldltl::DecodeDynamic(diag, image_memory, std::span(dyn_bad_gnu_hash_addr),
                                       elfldltl::DynamicSymbolInfoObserver(info)));
}

TYPED_TEST(ElfldltlDynamicTests, SymbolInfoObserverBadGnuHashAlign) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;
  using Dyn = typename Elf::Dyn;
  using Sym = typename Elf::Sym;

  ExpectedSingleError diag{"DT_GNU_HASH has misaligned address"};
  SymbolInfoTestImage<Elf> test_image;
  elfldltl::DirectMemory image_memory = test_image.memory();

  const Dyn dyn_bad_gnu_hash_align[] = {
      {.tag = elfldltl::ElfDynTag::kSoname, .val = test_image.soname_offset()},
      {.tag = elfldltl::ElfDynTag::kSymTab, .val = test_image.symtab_addr()},
      {.tag = elfldltl::ElfDynTag::kSymEnt, .val = sizeof(Sym)},
      {.tag = elfldltl::ElfDynTag::kStrTab, .val = test_image.strtab_addr()},
      {
          .tag = elfldltl::ElfDynTag::kStrSz,
          .val = static_cast<size_type>(test_image.strtab_size_bytes()),
      },
      {.tag = elfldltl::ElfDynTag::kHash, .val = test_image.hash_addr()},
      {
          .tag = elfldltl::ElfDynTag::kGnuHash,
          // This is misaligned vs alignof(size_type).
          .val = test_image.hash_addr() + sizeof(size_type) - 1,
      },
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  elfldltl::SymbolInfo<Elf> info;
  EXPECT_TRUE(elfldltl::DecodeDynamic(diag, image_memory, std::span(dyn_bad_gnu_hash_align),
                                      elfldltl::DynamicSymbolInfoObserver(info)));
}

template <class Elf, class AbiTraits = elfldltl::LocalAbiTraits>
struct NotCalledSymbolInfo {
  std::string_view string(typename Elf::size_type) const {
    ADD_FAILURE();
    return {};
  }
};

TYPED_TEST(ElfldltlDynamicTests, ObserveNeededEmpty) {
  using Elf = typename TestFixture::Elf;

  auto diag = ExpectOkDiagnostics();

  elfldltl::DirectMemory memory({}, 0);

  NotCalledSymbolInfo<Elf> si;

  constexpr typename Elf::Dyn dyn[] = {
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  EXPECT_TRUE(
      elfldltl::DecodeDynamic(diag, memory, std::span(dyn),
                              elfldltl::DynamicNeededObserver(si, [](std::string_view needed) {
                                ADD_FAILURE() << "Unexpected needed entry: " << needed.data();
                                return false;
                              })));
}

TYPED_TEST(ElfldltlDynamicTests, ObserveNeeded) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;

  auto diag = ExpectOkDiagnostics();

  elfldltl::DirectMemory memory({}, 0);

  elfldltl::SymbolInfo<Elf> si;

  constexpr std::string_view kNeededStrings[] = {"zero.so", "one.so", "two.so", "3.so"};
  TestSymtab<Elf> symtab;

  const typename Elf::Dyn dyn[] = {
      {.tag = elfldltl::ElfDynTag::kNeeded, .val = symtab.AddString(kNeededStrings[0])},
      {.tag = elfldltl::ElfDynTag::kNeeded, .val = symtab.AddString(kNeededStrings[1])},
      {.tag = elfldltl::ElfDynTag::kNeeded, .val = symtab.AddString(kNeededStrings[2])},
      {.tag = elfldltl::ElfDynTag::kNeeded, .val = symtab.AddString(kNeededStrings[3])},
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  symtab.SetInfo(si);

  size_type current_index = 0;
  auto expect_next = [&](std::string_view needed) {
    EXPECT_EQ(kNeededStrings[current_index++], needed);
    return true;
  };

  EXPECT_TRUE(elfldltl::DecodeDynamic(diag, memory, std::span(dyn),
                                      elfldltl::DynamicNeededObserver(si, expect_next)));
}

TYPED_TEST(ElfldltlDynamicTests, ObserveValueCollection) {
  using Elf = typename TestFixture::Elf;
  using size_type = typename Elf::size_type;

  auto diag = ExpectOkDiagnostics();

  elfldltl::DirectMemory memory({}, 0);

  TestSymtab<Elf> symtab;

  auto val0 = symtab.AddString("zero.so");
  auto val1 = symtab.AddString("one.so");
  auto val2 = symtab.AddString("two.so");
  auto val3 = symtab.AddString("three.so");

  const typename Elf::Dyn dyn[] = {
      {.tag = elfldltl::ElfDynTag::kNeeded, .val = val0},
      {.tag = elfldltl::ElfDynTag::kNeeded, .val = val1},
      {.tag = elfldltl::ElfDynTag::kNeeded, .val = val2},
      {.tag = elfldltl::ElfDynTag::kNeeded, .val = val3},
      // These tags should not be matched or collected by the observer.
      {.tag = elfldltl::ElfDynTag::kSoname, .val = 0x1},
      {.tag = elfldltl::ElfDynTag::kSymTab, .val = 0x2},
      {.tag = elfldltl::ElfDynTag::kSymEnt, .val = 0x3},
      {.tag = elfldltl::ElfDynTag::kNull},
  };

  static const constexpr std::string_view kCollectionError = "Failed to push value to collection.";
  elfldltl::StdContainer<std::vector>::Container<size_type> values;
  EXPECT_TRUE(elfldltl::DecodeDynamic(
      diag, memory, std::span(dyn),
      elfldltl::DynamicValueCollectionObserver<
          Elf, elfldltl::ElfDynTag::kNeeded,
          elfldltl::StdContainer<std::vector>::Container<size_type>, kCollectionError>(values)));

  EXPECT_EQ(values.size(), 4u);
  EXPECT_EQ(values[0], val0);
  EXPECT_EQ(values[1], val1);
  EXPECT_EQ(values[2], val2);
  EXPECT_EQ(values[3], val3);
}

}  // namespace
