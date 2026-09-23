// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "symbol-tests.h"

#include <lib/elfldltl/testing/typed-test.h>

#include <set>

#include <gmock/gmock.h>

namespace {

using namespace std::literals;

using ::testing::UnorderedElementsAreArray;

constexpr std::string_view kEmpty{};
constexpr elfldltl::SymbolName kEmptySymbol(kEmpty);
constexpr uint32_t kEmptyCompatHash = 0;
constexpr uint32_t kEmptyGnuHash = 5381;

constexpr uint32_t kFoobarCompatHash = 0x06d65882;
constexpr uint32_t kFoobarGnuHash = 0xfde460be;

FORMAT_TYPED_TEST_SUITE(ElfldltlSymbolTests);

TEST(ElfldltlSymbolTests, CompatHash) {
  EXPECT_EQ(kEmptyCompatHash, elfldltl::SymbolName(kEmpty).compat_hash());
  EXPECT_EQ(kFoobarCompatHash, elfldltl::SymbolName(kFoobar).compat_hash());
}

static_assert(kEmptySymbol.compat_hash() == kEmptyCompatHash);
static_assert(kFoobarSymbol.compat_hash() == kFoobarCompatHash);

TEST(ElfldltlSymbolTests, GnuHash) {
  EXPECT_EQ(kEmptyGnuHash, elfldltl::SymbolName(kEmpty).gnu_hash());
  EXPECT_EQ(kFoobarGnuHash, elfldltl::SymbolName(kFoobar).gnu_hash());
}

static_assert(kEmptySymbol.gnu_hash() == kEmptyGnuHash);
static_assert(kFoobarSymbol.gnu_hash() == kFoobarGnuHash);

TYPED_TEST(ElfldltlSymbolTests, CompatHashSize) {
  using Elf = typename TestFixture::Elf;

  elfldltl::SymbolInfo<Elf> si;
  kTestSymbols<Elf>.SetInfo(si);
  si.set_compat_hash(kTestCompatHash<typename Elf::Word>);

  EXPECT_EQ(si.safe_symtab().size(), kTestSymbolCount);
}

TYPED_TEST(ElfldltlSymbolTests, GnuHashSize) {
  using Elf = typename TestFixture::Elf;

  elfldltl::SymbolInfo<Elf> si;
  kTestSymbols<Elf>.SetInfo(si);
  si.set_gnu_hash(kTestGnuHash<typename Elf::Addr>);

  EXPECT_EQ(si.safe_symtab().size(), kTestSymbolCount);
}

TYPED_TEST(ElfldltlSymbolTests, LookupCompatHash) {
  using Elf = typename TestFixture::Elf;

  elfldltl::SymbolInfo<Elf> si;
  kTestSymbols<Elf>.SetInfo(si);
  si.set_compat_hash(kTestCompatHash<typename Elf::Word>);

  EXPECT_EQ(kNotFoundSymbol.Lookup(si), nullptr);

  EXPECT_EQ(kQuuxSymbol.Lookup(si), nullptr);  // Undefined should be skipped.

  const auto* foo = kFooSymbol.Lookup(si);
  ASSERT_NE(foo, nullptr);
  EXPECT_EQ(foo->value(), kFooAddress);

  const auto* bar = kBarSymbol.Lookup(si);
  ASSERT_NE(bar, nullptr);
  EXPECT_EQ(bar->value(), kBarAddress);

  const auto* foobar = kFoobarSymbol.Lookup(si);
  ASSERT_NE(foobar, nullptr);
  EXPECT_EQ(foobar->value(), kFoobarAddress);
}

TYPED_TEST(ElfldltlSymbolTests, LookupGnuHash) {
  using Elf = typename TestFixture::Elf;

  elfldltl::SymbolInfo<Elf> si;
  kTestSymbols<Elf>.SetInfo(si);
  si.set_gnu_hash(kTestGnuHash<typename Elf::Addr>);

  EXPECT_EQ(kNotFoundSymbol.Lookup(si), nullptr);

  EXPECT_EQ(kQuuxSymbol.Lookup(si), nullptr);  // Undefined should be skipped.

  const auto* foo = kFooSymbol.Lookup(si);
  ASSERT_NE(foo, nullptr);
  EXPECT_EQ(foo->value(), kFooAddress);

  const auto* bar = kBarSymbol.Lookup(si);
  ASSERT_NE(bar, nullptr);
  EXPECT_EQ(bar->value(), kBarAddress);

  const auto* foobar = kFoobarSymbol.Lookup(si);
  ASSERT_NE(foobar, nullptr);
  EXPECT_EQ(foobar->value(), kFoobarAddress);
}

// The enumeration tests use the same symbol table with both flavors of hash
// table.

template <class Elf>
struct CompatHash {
  using Table = typename elfldltl::CompatHash<Elf>;
  static constexpr void SetInfo(elfldltl::SymbolInfo<Elf>& si) {
    si.set_compat_hash(kTestCompatHash<typename Elf::Word>);
  }
  static constexpr Table Get(const elfldltl::SymbolInfo<Elf>& si) { return *si.compat_hash(); }
  static constexpr std::string_view kNames[] = {
      "bar",
      "foo",
      "foobar",
      "quux",
  };
};

template <class Elf>
struct GnuHash {
  using Table = typename elfldltl::GnuHash<Elf>;
  static constexpr void SetInfo(elfldltl::SymbolInfo<Elf>& si) {
    si.set_gnu_hash(kTestGnuHash<typename Elf::Addr>);
  }
  static constexpr Table Get(const elfldltl::SymbolInfo<Elf>& si) { return *si.gnu_hash(); }
  static constexpr std::string_view kNames[] = {
      // The DT_GNU_HASH table omits the undefined symbols.
      "bar",
      "foo",
      "foobar",
  };
};

template <class Elf, template <class ElfLayout> class HashTable>
void EnumerateHashTable() {
  using Sym = Elf::Sym;

  elfldltl::SymbolInfo<Elf> si;
  kTestSymbols<Elf>.SetInfo(si);
  HashTable<Elf>::SetInfo(si);
  const auto hash_table = HashTable<Elf>::Get(si);

  // Collect all the symbols in a set that doesn't remove duplicates.
  std::multiset<std::string_view> symbol_names;
  for (const Sym& sym : si.HashedSymbols(hash_table)) {
    std::string_view name = si.string(sym.name);
    ASSERT_FALSE(name.empty());
    symbol_names.insert(name);
  }

  EXPECT_THAT(symbol_names, UnorderedElementsAreArray(HashTable<Elf>::kNames));
}

TYPED_TEST(ElfldltlSymbolTests, EnumerateCompatHash) {
  EnumerateHashTable<typename TestFixture::Elf, CompatHash>();
}

TYPED_TEST(ElfldltlSymbolTests, EnumerateGnuHash) {
  EnumerateHashTable<typename TestFixture::Elf, GnuHash>();
}

TYPED_TEST(ElfldltlSymbolTests, OnSymbols) {
  using Elf = TestFixture::Elf;
  using SymbolInfo = elfldltl::SymbolInfo<Elf>;
  using Sym = Elf::Sym;

  {
    SymbolInfo si;
    kTestSymbols<Elf>.SetInfo(si);
    si.set_compat_hash(kTestCompatHash<typename Elf::Word>);
    std::multiset<std::string_view> symbol_names;
    EXPECT_TRUE(si.OnSymbols([&si, &symbol_names](const Sym& sym) {
      std::string_view name = si.string(sym.name);
      EXPECT_FALSE(name.empty());
      symbol_names.insert(name);
      return true;
    }));
    EXPECT_THAT(symbol_names, UnorderedElementsAreArray(CompatHash<Elf>::kNames))
        << "symbols from DT_HASH";
  }

  {
    SymbolInfo si;
    kTestSymbols<Elf>.SetInfo(si);
    si.set_gnu_hash(kTestGnuHash<typename Elf::Addr>);
    std::multiset<std::string_view> symbol_names;
    EXPECT_TRUE(si.OnSymbols([&si, &symbol_names](const Sym& sym) {
      std::string_view name = si.string(sym.name);
      EXPECT_FALSE(name.empty());
      symbol_names.insert(name);
      return true;
    }));
    EXPECT_THAT(symbol_names, UnorderedElementsAreArray(GnuHash<Elf>::kNames))
        << "symbols from DT_GNU_HASH";
    ;
  }
}

template <class Elf, template <class ElfLayout> class HashTable>
void LookupVaddrTest() {
  using SymbolInfo = elfldltl::SymbolInfo<Elf>;
  using Sym = Elf::Sym;

  SymbolInfo si;
  kTestSymbols<Elf>.SetInfo(si);
  HashTable<Elf>::SetInfo(si);

  auto sym_name = [&si](const Sym* sym) -> std::string_view {
    if (!sym) {
      return "<no symbol found>"sv;
    };
    std::string_view name = si.string(sym->name);
    if (name.empty()) {
      return "<invalid st_name>"sv;
    }
    return name;
  };

  // This is below the lowest symbol address.
  EXPECT_EQ(sym_name(si.LookupVaddr(0x050)), "<no symbol found>"sv);

  // This is exactly the quux symbol, and not in the range of any other symbol.
  // Even though it's an SHN_UNDEF symbol, it's got a nonzero value/size and so
  // could be a PLT entry and thus will be considered--but only in DT_HASH!  In
  // DT_GNU_HASH, _all_ SHN_UNDEF symbols are omitted.  Since the hash table is
  // the only (safe and efficient) means of enumerating the whole symbol table,
  // the PLT entries won't be seen.
  if constexpr (std::same_as<HashTable<Elf>, GnuHash<Elf>>) {
    EXPECT_EQ(sym_name(si.LookupVaddr(kQuuxAddress)), "<no symbol found>"sv);
  } else {
    EXPECT_EQ(sym_name(si.LookupVaddr(kQuuxAddress)), "quux"sv);
  }

  // Somewhere in the middle of the foo symbol's size range.
  EXPECT_EQ(sym_name(si.LookupVaddr(kFooAddress + 0x10)), "foo"sv);

  // The foobar symbol size range is wholly inside the bar symbol's size range,
  // but an exact match will prefer it.
  EXPECT_EQ(sym_name(si.LookupVaddr(kFoobarAddress)), "foobar"sv);
}

TYPED_TEST(ElfldltlSymbolTests, LookupVaddrCompatHash) {
  LookupVaddrTest<typename TestFixture::Elf, CompatHash>();
}

TYPED_TEST(ElfldltlSymbolTests, LookupVaddrGnuHash) {
  LookupVaddrTest<typename TestFixture::Elf, GnuHash>();
}

TYPED_TEST(ElfldltlSymbolTests, SymbolInfoForSingleLookup) {
  using Elf = typename TestFixture::Elf;

  constexpr static elfldltl::SymbolInfoForSingleLookup<Elf> si{"sym"};

  elfldltl::SymbolName name{si, si.symbol()};
  EXPECT_EQ(name, "sym");
}

TYPED_TEST(ElfldltlSymbolTests, Remote) {
  using Elf = typename TestFixture::Elf;

  using RemoteSymbolInfo = elfldltl::SymbolInfo<Elf, elfldltl::RemoteAbiTraits>;

  RemoteSymbolInfo si;
  si = RemoteSymbolInfo(si);
}

#ifdef __APPLE__
#define SECTION_NAME "__DATA,__bss"
#else
#define SECTION_NAME ".bss"
#endif

TYPED_TEST(ElfldltlSymbolTests, ZeroInitialized) {
  using Elf = typename TestFixture::Elf;

  // Test that this object can be zero initialized by putting it in .bss
  [[gnu::section(SECTION_NAME)]] static elfldltl::SymbolInfo<Elf> foo{
      elfldltl::kLinkerZeroInitialized};
  foo.InitLinkerZeroInitialized();

  EXPECT_EQ(foo.strtab(), "\0"sv);
}

}  // namespace
