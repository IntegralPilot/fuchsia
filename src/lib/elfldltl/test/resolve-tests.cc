// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/elfldltl/dynamic.h>
#include <lib/elfldltl/layout.h>
#include <lib/elfldltl/load.h>
#include <lib/elfldltl/mapped-fd-file.h>
#include <lib/elfldltl/resolve.h>
#include <lib/elfldltl/testing/diagnostics.h>
#include <lib/elfldltl/testing/get-test-data.h>
#include <lib/elfldltl/testing/typed-test.h>

#include <filesystem>
#include <string>
#include <string_view>

#include "symbol-tests.h"

namespace {

using elfldltl::testing::ExpectedSingleError;
using elfldltl::testing::ExpectOkDiagnostics;

template <class ElfLayout>
class ElfldltlResolveTests : public testing::Test {
 public:
  using Elf = ElfLayout;

  class TestModule {
   public:
    using size_type = typename Elf::size_type;

    explicit TestModule(std::string_view prefix) { create(prefix); }

    explicit TestModule(const elfldltl::SymbolInfo<Elf>& si) : symbol_info_{si} {}

    constexpr const auto& symbol_info() const { return symbol_info_; }
    constexpr auto& symbol_info() { return symbol_info_; }

    constexpr size_type load_bias() const { return 0; }

    constexpr size_type tls_module_id() const { return 0; }

    constexpr size_type static_tls_bias() const { return 0; }

    constexpr bool uses_static_tls() const { return false; }

    constexpr auto& file() { return file_; }

    constexpr fit::result<bool, const typename Elf::Sym*> Lookup(  //
        auto& diag, elfldltl::SymbolName& name) const {
      return fit::ok(name.Lookup(symbol_info_));
    }

   private:
    // This is in it's own function that returns void so that ASSERT_* macros
    // can be used. Evidently they don't work in constructors. This method
    // should only be used by the constructor.
    void create(std::string_view prefix) {
      using Phdr = typename Elf::Phdr;
      using Dyn = typename Elf::Dyn;

      auto diag = ExpectOkDiagnostics();

      std::filesystem::path path = elfldltl::testing::GetTestDataPath(GetFileName(prefix));
      fbl::unique_fd fd(open(path.c_str(), O_RDONLY));
      ASSERT_TRUE(fd) << path;

      ASSERT_TRUE(file_.Init(fd.get()).is_ok());

      auto headers =
          elfldltl::LoadHeadersFromFile<Elf>(diag, file_, elfldltl::NoArrayFromFile<Phdr>(), {});
      ASSERT_TRUE(headers);

      std::optional<Phdr> ph;
      elfldltl::DecodePhdrs(diag, headers->second, elfldltl::PhdrDynamicObserver<Elf>(ph));
      ASSERT_TRUE(ph);

      // Note: We read in from offset here and not vaddr so we get the file
      // contents because we are not loading this file. For this reason, the
      // checked in files must have a flat dynamic segment entirely in RODATA.
      auto dyn = file_.template ReadArray<Dyn>(ph->offset(), ph->filesz() / sizeof(Dyn));
      ASSERT_TRUE(dyn);

      ASSERT_TRUE(elfldltl::DecodeDynamic(diag, file_, *dyn,
                                          elfldltl::DynamicSymbolInfoObserver(symbol_info_)));
    }

    elfldltl::MappedFdFile file_;
    elfldltl::SymbolInfo<Elf> symbol_info_;
  };

 private:
  static std::string GetFileName(std::string_view prefix) {
    using namespace std::string_literals;

    constexpr std::string_view class_name = [] {
      switch (Elf::kClass) {
        case elfldltl::ElfClass::k32:
          return "32"sv;
        case elfldltl::ElfClass::k64:
          return "64"sv;
      }
    }();

    constexpr std::string_view data_name = [] {
      switch (Elf::kData) {
        case elfldltl::ElfData::k2Lsb:
          return "le"sv;
        case elfldltl::ElfData::k2Msb:
          return "be"sv;
      }
    }();

    return std::string{prefix} + "-" + std::string{class_name} + std::string{data_name} + ".so";
  }
};

TYPED_TEST_SUITE(ElfldltlResolveTests, elfldltl::testing::AllFormatsTypedTest,
                 elfldltl::testing::TestNames);

constexpr elfldltl::SymbolName kASymbol("a"sv);
constexpr elfldltl::SymbolName kBSymbol("b"sv);
constexpr elfldltl::SymbolName kCSymbol("c"sv);
constexpr elfldltl::SymbolName kWeakBothSymbol("weak_both"sv);

template <class Elf>
struct NoTlsdesc {
  using TlsDescGot = Elf::template TlsDescGot<>;

  constexpr TlsDescGot operator()() const { return {}; }

  template <class Diagnostics, class Definition>
  constexpr std::optional<TlsDescGot> operator()(Diagnostics& diag, const Definition& defn) const {
    return {};
  }
};

template <class Elf>
constexpr NoTlsdesc<Elf> kNoTlsdesc{};

TYPED_TEST(ElfldltlResolveTests, SingleModule) {
  using Elf = typename TestFixture::Elf;
  using Sym = typename Elf::Sym;
  using TestModule = typename ElfldltlResolveTests<Elf>::TestModule;

  auto diag = ExpectOkDiagnostics();

  TestModule module("first");
  if (this->HasFatalFailure()) {
    return;
  }

  auto& si = module.symbol_info();

  const Sym* a = kASymbol.Lookup(si);
  ASSERT_NE(a, nullptr);

  std::span modules{&module, 1};
  auto resolve = elfldltl::MakeSymbolResolver(module, modules, diag, kNoTlsdesc<Elf>);
  auto found = resolve(*a, elfldltl::RelocateTls::kNone);
  ASSERT_TRUE(found.is_ok()) << found.error_value();
  ASSERT_FALSE(found->undefined_weak());
  EXPECT_EQ(&found->symbol(), a);
  EXPECT_EQ(found->symbol().value, 1ul);
}

TYPED_TEST(ElfldltlResolveTests, DefBothFoundFirst) {
  using Elf = typename TestFixture::Elf;
  using Sym = typename Elf::Sym;
  using TestModule = typename ElfldltlResolveTests<Elf>::TestModule;

  auto diag = ExpectOkDiagnostics();

  std::array modules{TestModule("first"), TestModule("second")};
  if (this->HasFatalFailure()) {
    return;
  }

  auto& si = modules[0].symbol_info();
  auto& si2 = modules[1].symbol_info();

  const Sym* a = kASymbol.Lookup(si);
  ASSERT_NE(a, nullptr);
  const Sym* a2 = kASymbol.Lookup(si2);
  ASSERT_NE(a2, nullptr);

  auto resolve = elfldltl::MakeSymbolResolver(modules[1], modules, diag, kNoTlsdesc<Elf>);
  auto found = resolve(*a2, elfldltl::RelocateTls::kNone);
  ASSERT_TRUE(found.is_ok()) << found.error_value();
  ASSERT_FALSE(found->undefined_weak());
  EXPECT_EQ(&found->symbol(), a);
  EXPECT_EQ(found->symbol().value, 1ul);
}

TYPED_TEST(ElfldltlResolveTests, UndefFirstFoundSecond) {
  using Elf = typename TestFixture::Elf;
  using Sym = typename Elf::Sym;
  using TestModule = typename ElfldltlResolveTests<Elf>::TestModule;

  auto diag = ExpectOkDiagnostics();

  std::array modules{TestModule("first"), TestModule("second")};
  if (this->HasFatalFailure()) {
    return;
  }

  auto& si2 = modules[1].symbol_info();

  elfldltl::SymbolInfoForSingleLookup<Elf> si{"b"};
  TestModule lookup_module{si};

  const Sym* b_def = kBSymbol.Lookup(si2);
  ASSERT_NE(b_def, nullptr);

  auto resolve = elfldltl::MakeSymbolResolver(lookup_module, modules, diag, kNoTlsdesc<Elf>);
  auto found = resolve(si.symbol(), elfldltl::RelocateTls::kNone);
  ASSERT_TRUE(found.is_ok()) << found.error_value();
  ASSERT_FALSE(found->undefined_weak());
  EXPECT_EQ(&found->symbol(), b_def);
  EXPECT_EQ(found->symbol().value, 2ul);
}

TYPED_TEST(ElfldltlResolveTests, UndefinedWeak) {
  using Elf = typename TestFixture::Elf;
  using TestModule = typename ElfldltlResolveTests<Elf>::TestModule;

  auto diag = ExpectOkDiagnostics();

  std::array modules{TestModule("first"), TestModule("second")};
  if (this->HasFatalFailure()) {
    return;
  }

  elfldltl::SymbolInfoForSingleLookup<Elf> si{"noexist", elfldltl::ElfSymType::kNoType,
                                              elfldltl::ElfSymBind::kWeak};
  auto check_resolver = [&](auto&& resolve, std::string_view identifier) {
    auto found = resolve(si.symbol(), elfldltl::RelocateTls::kNone);
    ASSERT_TRUE(found.is_ok()) << identifier;
    EXPECT_TRUE(found->undefined_weak()) << identifier;
  };

  TestModule lookup_module{si};
  check_resolver(elfldltl::MakeSymbolResolver(lookup_module, modules, diag, kNoTlsdesc<Elf>,
                                              elfldltl::ResolverPolicy::kStrictLinkOrder),
                 "kStrictLinkOrder");
  check_resolver(elfldltl::MakeSymbolResolver(lookup_module, modules, diag, kNoTlsdesc<Elf>,
                                              elfldltl::ResolverPolicy::kStrongOverWeak),
                 "kStrongOverWeak");
}

TYPED_TEST(ElfldltlResolveTests, DefaultWeakPolicy) {
  using Elf = typename TestFixture::Elf;
  using Sym = typename Elf::Sym;
  using TestModule = typename ElfldltlResolveTests<Elf>::TestModule;

  auto diag = ExpectOkDiagnostics();

  std::array modules{TestModule("first"), TestModule("second")};
  if (this->HasFatalFailure()) {
    return;
  }

  auto& si1 = modules[0].symbol_info();
  auto& si2 = modules[1].symbol_info();

  const Sym* c_weak = kCSymbol.Lookup(si1);
  ASSERT_NE(c_weak, nullptr);
  const Sym* c_strong = kCSymbol.Lookup(si2);
  ASSERT_NE(c_strong, nullptr);

  elfldltl::SymbolInfoForSingleLookup<Elf> si{"c"};

  auto check_resolver = [&](auto&& resolve, std::string_view identifier) {
    auto found = resolve(si.symbol(), elfldltl::RelocateTls::kNone);
    ASSERT_TRUE(found.is_ok()) << identifier;
    ASSERT_FALSE(found->undefined_weak()) << identifier;
    EXPECT_EQ(&found->symbol(), c_weak) << identifier;
    EXPECT_EQ(found->symbol().value, 1ul) << identifier;
  };

  TestModule lookup_module{si};
  check_resolver(elfldltl::MakeSymbolResolver(lookup_module, modules, diag, kNoTlsdesc<Elf>),
                 "default");
  check_resolver(elfldltl::MakeSymbolResolver(lookup_module, modules, diag, kNoTlsdesc<Elf>,
                                              elfldltl::ResolverPolicy::kStrictLinkOrder),
                 "specified");
}

TYPED_TEST(ElfldltlResolveTests, DynamicWeakPolicy) {
  using Elf = typename TestFixture::Elf;
  using Sym = typename Elf::Sym;
  using TestModule = typename ElfldltlResolveTests<Elf>::TestModule;

  auto diag = ExpectOkDiagnostics();

  std::array modules{TestModule("first"), TestModule("second")};
  if (this->HasFatalFailure()) {
    return;
  }

  auto& si1 = modules[0].symbol_info();
  auto& si2 = modules[1].symbol_info();

  {
    const Sym* c_weak = kCSymbol.Lookup(si1);
    ASSERT_NE(c_weak, nullptr);
    const Sym* c_strong = kCSymbol.Lookup(si2);
    ASSERT_NE(c_strong, nullptr);

    elfldltl::SymbolInfoForSingleLookup<Elf> si{"c"};

    TestModule lookup_module{si};
    auto resolve = elfldltl::MakeSymbolResolver(lookup_module, modules, diag, kNoTlsdesc<Elf>,
                                                elfldltl::ResolverPolicy::kStrongOverWeak);
    auto found = resolve(si.symbol(), elfldltl::RelocateTls::kNone);
    ASSERT_TRUE(found.is_ok());
    ASSERT_FALSE(found->undefined_weak());
    EXPECT_EQ(&found->symbol(), c_strong);
    EXPECT_EQ(found->symbol().value, 2ul);
  }
  {
    const Sym* weak_both1 = kWeakBothSymbol.Lookup(si1);
    ASSERT_TRUE(weak_both1);
    const Sym* weak_both2 = kWeakBothSymbol.Lookup(si2);
    ASSERT_TRUE(weak_both2);

    elfldltl::SymbolInfoForSingleLookup<Elf> si{"weak_both"};

    TestModule lookup_module{si};
    auto resolve = elfldltl::MakeSymbolResolver(lookup_module, modules, diag, kNoTlsdesc<Elf>,
                                                elfldltl::ResolverPolicy::kStrongOverWeak);
    auto found = resolve(si.symbol(), elfldltl::RelocateTls::kNone);
    ASSERT_TRUE(found.is_ok());
    ASSERT_FALSE(found->undefined_weak());
    EXPECT_EQ(&found->symbol(), weak_both1);
    EXPECT_EQ(found->symbol().value, 1ul);
  }
}

TYPED_TEST(ElfldltlResolveTests, GnuUniqueError) {
  using Elf = typename TestFixture::Elf;
  using Sym = typename Elf::Sym;
  using TestModule = typename ElfldltlResolveTests<Elf>::TestModule;

  ExpectedSingleError expected("STB_GNU_UNIQUE not supported");

  TestModule module("gnu_unique");
  if (this->HasFatalFailure()) {
    return;
  }

  auto& si = module.symbol_info();

  const Sym* a = kASymbol.Lookup(si);
  ASSERT_NE(a, nullptr);

  std::span modules{&module, 1};
  auto resolve = elfldltl::MakeSymbolResolver(module, modules, expected, kNoTlsdesc<Elf>);
  auto found = resolve(*a, elfldltl::RelocateTls::kNone);
  EXPECT_TRUE(found.is_error());
}

TYPED_TEST(ElfldltlResolveTests, Undefined) {
  using Elf = typename TestFixture::Elf;
  using TestModule = typename ElfldltlResolveTests<Elf>::TestModule;

  ExpectedSingleError expected("undefined symbol: ", "noexist");

  std::array modules{TestModule("first"), TestModule("second")};
  if (this->HasFatalFailure()) {
    return;
  }

  elfldltl::SymbolInfoForSingleLookup<Elf> si{"noexist"};
  TestModule lookup_module{si};
  auto resolve = elfldltl::MakeSymbolResolver(lookup_module, modules, expected, kNoTlsdesc<Elf>);
  auto found = resolve(si.symbol(), elfldltl::RelocateTls::kNone);
  EXPECT_TRUE(found.is_error());
}

}  // namespace
