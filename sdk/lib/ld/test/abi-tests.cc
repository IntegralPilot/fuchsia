// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/testing/typed-test.h>
#include <lib/ld/abi.h>
#include <lib/ld/module.h>
#include <lib/ld/tls.h>
#include <lib/symbolizer-markup/writer.h>

#include <array>
#include <string_view>
#include <type_traits>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

namespace {

using namespace std::literals;

constexpr uint32_t kPageSize = 0x1000;

template <class ElfLayout>
struct LdTests : public elfldltl::testing::FormatTypedTest<ElfLayout> {
  using typename elfldltl::testing::FormatTypedTest<ElfLayout>::Elf;
  using Abi = ld::abi::Abi<Elf>;

  static_assert(std::is_default_constructible_v<Abi>);
  static_assert(std::is_trivially_copy_constructible_v<Abi>);
  static_assert(std::is_trivially_copy_assignable_v<Abi>);
  static_assert(std::is_trivially_destructible_v<Abi>);

  // Since Module is only used by reference in Abi, it has to be separately
  // instantiated and tested.
  using Module = Abi::Module;
  static_assert(std::is_default_constructible_v<Module>);
  static_assert(std::is_trivially_copy_constructible_v<Module>);
  static_assert(std::is_trivially_copy_assignable_v<Module>);
  static_assert(std::is_trivially_destructible_v<Module>);

  using RDebug = Abi::RDebug;
  static_assert(std::is_default_constructible_v<RDebug>);
  static_assert(std::is_trivially_copy_constructible_v<RDebug>);
  static_assert(std::is_trivially_copy_assignable_v<RDebug>);
  static_assert(std::is_trivially_destructible_v<RDebug>);
};

TYPED_TEST_SUITE(LdTests, elfldltl::testing::AllFormatsTypedTest, elfldltl::testing::TestNames);

TYPED_TEST(LdTests, AbiTypes) {
  using Abi = typename TestFixture::Abi;
  using Module = typename TestFixture::Module;
  using RDebug = typename TestFixture::RDebug;

  Abi abi;
  abi = Abi{abi};

  Module module;
  module = Module{module};

  RDebug r_debug;
  r_debug = RDebug{r_debug};

  // Test that this object is zero initialized so it can be put in bss.
  alignas(Module) std::array<std::byte, sizeof(Module)> storage{};
  new (&storage) Module(Module::LinkerZeroInitialized());
  EXPECT_THAT(storage, testing::Each(testing::Eq(std::byte(0))));
}

template <class Abi>
constexpr Abi::Module MakeModule(uint32_t modid, const char* name,
                                 std::span<const std::byte> build_id, typename Abi::Addr load_addr,
                                 std::span<const typename Abi::Phdr> phdrs, bool symbols_visible,
                                 const typename Abi::Module* next,
                                 const typename Abi::Module* prev) {
  typename Abi::Module result;
  result.symbolizer_modid = modid;
  result.link_map.name = name;
  result.build_id = build_id;
  result.link_map.addr = load_addr;
  result.vaddr_start = load_addr + phdrs.front().vaddr;
  result.vaddr_end = (load_addr + phdrs.back().vaddr + phdrs.back().memsz + kPageSize - 1) &
                     -typename Abi::Addr{kPageSize};
  result.symbols_visible = symbols_visible;
  result.phdrs = phdrs;
  if (next) {
    result.link_map.next = &const_cast<Abi::Module*>(next)->link_map;
  }
  if (prev) {
    result.link_map.prev = &const_cast<Abi::Module*>(prev)->link_map;
  }
  return result;
}

constexpr const char* kName1 = "first";
constexpr const char* kName2 = "second";
constexpr const char* kName3 = "third";

constexpr std::array kBuildId1 = {std::byte{0x12}, std::byte{0x34}};
constexpr std::array kBuildId2 = {std::byte{0x56}, std::byte{0x78}};
constexpr std::array kBuildId3 = {std::byte{0xaa}, std::byte{0xbb}, std::byte{0xcc}};

// This is actually the same in all instantiations.
using PhdrFlags = elfldltl::Elf<>::Phdr::Flags;

// Different field order in Elf32 vs Elf64 makes designated initializers
// annoying for Phdr.
template <class Elf>
consteval typename Elf::Phdr MakeLoad(typename Elf::Word flags, typename Elf::Addr vaddr,
                                      typename Elf::Addr memsz) {
  typename Elf::Phdr phdr = {.type = elfldltl::ElfPhdrType::kLoad};
  phdr.flags = flags;
  phdr.vaddr = vaddr;
  phdr.memsz = memsz;
  return phdr;
}

template <class Elf>
constexpr Elf::Phdr kPhdrs1[] = {
    MakeLoad<Elf>(PhdrFlags::kRead, 0, 0x1000),
    MakeLoad<Elf>(PhdrFlags::kRead | PhdrFlags::kExecute, 0x1000, 0x2000),
};

template <class Elf>
constexpr Elf::Phdr kPhdrs2[] = {
    MakeLoad<Elf>(PhdrFlags::kRead | PhdrFlags::kExecute, 0, 0x1234),
    MakeLoad<Elf>(PhdrFlags::kRead | PhdrFlags::kWrite, 0x2234, 0x1000),
};

template <class Elf>
constexpr ld::abi::Abi<Elf>::Phdr kPhdrs3[] = {
    MakeLoad<Elf>(PhdrFlags::kRead, 0, 0x3000),
    MakeLoad<Elf>(PhdrFlags::kExecute, 0x3000, 0x4000),
    MakeLoad<Elf>(PhdrFlags::kRead | PhdrFlags::kWrite, 0x7000, 0x8000),
};

template <class Elf>
constexpr ld::abi::Abi<Elf>::Module kModules[3] = {
    MakeModule<ld::abi::Abi<Elf>>(0, kName1, kBuildId1, 0x1000, kPhdrs1<Elf>, true,
                                  &kModules<Elf>[1], nullptr),
    MakeModule<ld::abi::Abi<Elf>>(1, kName2, kBuildId2, 0x5000, kPhdrs2<Elf>, false,
                                  &kModules<Elf>[2], &kModules<Elf>[0]),
    MakeModule<ld::abi::Abi<Elf>>(2, kName3, kBuildId3, 0xf000, kPhdrs3<Elf>, true, nullptr,
                                  &kModules<Elf>[1]),
};

TYPED_TEST(LdTests, AbiModuleList) {
  using Abi = typename TestFixture::Abi;
  using Elf = typename TestFixture::Elf;

  constexpr Abi abi{.loaded_modules{&kModules<Elf>[0]}};
  const auto modules = ld::AbiLoadedModules(abi);
  auto it = modules.begin();
  ASSERT_NE(it, modules.end());
  ASSERT_EQ(it++->link_map.name.get(), kName1);
  ASSERT_NE(it, modules.end());
  ASSERT_EQ(it++->link_map.name.get(), kName2);
  ASSERT_NE(it, modules.end());
  ASSERT_EQ(it++->link_map.name.get(), kName3);
  ASSERT_EQ(it, modules.end());
}

TYPED_TEST(LdTests, AbiSymbolicModuleList) {
  using Abi = typename TestFixture::Abi;
  using Elf = typename TestFixture::Elf;

  constexpr Abi abi{.loaded_modules{&kModules<Elf>[0]}};
  auto modules = ld::AbiLoadedSymbolModules(abi);
  auto it = modules.begin();
  ASSERT_NE(it, modules.end());
  const char* c = it++->link_map.name.get();
  ASSERT_EQ(c, kName1);
  ASSERT_NE(it, modules.end());
  ASSERT_EQ(it++->link_map.name.get(), kName3);
  ASSERT_EQ(it, modules.end());
  ASSERT_EQ((--it)->link_map.name.get(), kName3);
  ASSERT_EQ((--it)->link_map.name.get(), kName1);
}

TYPED_TEST(LdTests, ModuleSymbolizerContext) {
  using Abi = typename TestFixture::Abi;
  using Elf = typename TestFixture::Elf;

  auto module_context = [](const Abi::Module& module) -> std::string {
    std::string markup_text;
    symbolizer_markup::Writer writer{
        [&markup_text](std::string_view str) { markup_text += str; },
    };
    auto& return_ref = ld::ModuleSymbolizerContext(writer, module, kPageSize);
    EXPECT_EQ(&return_ref, &writer);
    return markup_text;
  };

  EXPECT_EQ(module_context(kModules<Elf>[0]),
            "{{{module:0:first:elf:1234}}}\n"
            "{{{mmap:0x1000:0x1000:load:0:r:0x0}}}\n"
            "{{{mmap:0x2000:0x3000:load:0:rx:0x1000}}}\n");
  EXPECT_EQ(module_context(kModules<Elf>[1]),
            "{{{module:1:second:elf:5678}}}\n"
            "{{{mmap:0x5000:0x2000:load:1:rx:0x0}}}\n"
            "{{{mmap:0x7000:0x3000:load:1:rw:0x2000}}}\n");
  EXPECT_EQ(module_context(kModules<Elf>[2]),
            "{{{module:2:third:elf:aabbcc}}}\n"
            "{{{mmap:0xf000:0x3000:load:2:r:0x0}}}\n"
            "{{{mmap:0x12000:0x7000:load:2:x:0x3000}}}\n"
            "{{{mmap:0x16000:0xf000:load:2:rw:0x7000}}}\n");
}

TYPED_TEST(LdTests, ModuleContainsVaddr) {
  using Elf = typename TestFixture::Elf;

  EXPECT_FALSE(ld::ModuleContainsVaddr(kModules<Elf>[0], 0x0100));
  EXPECT_TRUE(ld::ModuleContainsVaddr(kModules<Elf>[0], 0x1000));
  EXPECT_FALSE(ld::ModuleContainsVaddr(kModules<Elf>[0], 0x4000));
  EXPECT_TRUE(ld::ModuleContainsVaddr(kModules<Elf>[0], 0x2345));
}

TYPED_TEST(LdTests, FindModuleByVaddr) {
  using Abi = typename TestFixture::Abi;
  using Elf = typename TestFixture::Elf;

  constexpr Abi abi{.loaded_modules{&kModules<Elf>[0]}};
  const auto modules = ld::AbiLoadedModules(abi);

  decltype(modules.begin()) it;
  auto it_name = [&it, &modules]() -> std::string_view {
    if (it == modules.end()) {
      return "<not found>"sv;
    }
    return it->link_map.name.get();
  };

  it = ld::FindModuleByVaddr(modules, 0x0100);  // Below any module.
  EXPECT_EQ(it, modules.end()) << it_name();

  it = ld::FindModuleByVaddr(modules, 0x123000);  // Above any module.
  EXPECT_EQ(it, modules.end()) << it_name();

  it = ld::FindModuleByVaddr(modules, 0x2000);  // Inside first.
  EXPECT_EQ(it, modules.begin()) << it_name();

  it = ld::FindModuleByVaddr(modules, 0x5000);  // At start of second.
  EXPECT_EQ(it_name(), "second"sv);

  it = ld::FindModuleByVaddr(modules, 0xa000);  // Just past end of second.
  EXPECT_EQ(it, modules.end()) << it_name();

  it = ld::FindModuleByVaddr(modules, 0xbad1);  // In the large gap before third.
  EXPECT_EQ(it, modules.end()) << it_name();

  it = ld::FindModuleByVaddr(modules, 0x10000);  // Inside third.
  EXPECT_EQ(it_name(), "third"sv);
}

}  // namespace
