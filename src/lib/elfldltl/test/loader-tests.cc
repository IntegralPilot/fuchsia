// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/container.h>
#include <lib/elfldltl/diagnostics.h>
#include <lib/elfldltl/dynamic.h>
#include <lib/elfldltl/link.h>
#include <lib/elfldltl/load.h>
#include <lib/elfldltl/mmap-loader.h>
#include <lib/elfldltl/testing/diagnostics.h>
#include <lib/elfldltl/testing/get-test-data.h>
#include <lib/elfldltl/testing/loader.h>
#include <lib/elfldltl/testing/typed-test.h>
#include <lib/fit/defer.h>
#include <sys/mman.h>

#include <filesystem>
#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "test-data.h"

namespace {

struct LoadOptions {
  bool commit = true;
  bool persist_state = false;
  bool reloc = true;
};

template <class Traits>
class ElfldltlLoaderTests : public elfldltl::testing::LoadTests<Traits> {
 public:
  using Base = elfldltl::testing::LoadTests<Traits>;
  using typename Base::Loader;
  using typename Base::LoadInfo;
  using Relro = typename Loader::Relro;
  using Region = typename LoadInfo::Region;

  void TearDown() override {
    // We use the Posix APIs for genericism of the test.
    // The destructors of the various loaders use platform
    // specific APIs.
    if (!mem_.image().empty()) {
      munmap(mem_.image().data(), mem_.image().size());
    }
  }

  void Load(std::string_view so_path, LoadOptions options = {}) {
    std::optional<typename Base::LoadResult> result;
    Base::Load(so_path, result);
    ASSERT_TRUE(result);

    elfldltl::DirectMemory mem;
    if constexpr (Traits::kHasMemory) {
      mem.set_base(result->loader.memory().base());
      mem.set_image(result->loader.memory().image());
    } else {
      mem.set_base(result->info.vaddr_start());
      mem.set_image(
          {reinterpret_cast<std::byte*>(result->info.vaddr_start() + result->loader.load_bias()),
           result->info.vaddr_size()});
    }

    EXPECT_EQ(mem.base(), result->info.vaddr_start());
    EXPECT_EQ(mem.image().size_bytes(), result->info.vaddr_size());
    EXPECT_EQ(reinterpret_cast<uintptr_t>(mem.image().data()),
              result->info.vaddr_start() + result->loader.load_bias());

    auto diag = elfldltl::testing::ExpectOkDiagnostics();
    std::span<const Phdr> phdrs = result->phdrs;
    std::optional<Phdr> ph;
    std::optional<Phdr> relro_phdr;
    elfldltl::DecodePhdrs(diag, phdrs, elfldltl::PhdrDynamicObserver<Elf>(ph),
                          elfldltl::PhdrRelroObserver<Elf>(relro_phdr));
    ASSERT_TRUE(ph);

    auto dyn = mem.ReadArray<Dyn>(ph->vaddr(), ph->filesz() / sizeof(Dyn));
    ASSERT_TRUE(dyn);

    elfldltl::RelocationInfo<Elf> reloc_info;
    ASSERT_TRUE(elfldltl::DecodeDynamic(diag, mem, *dyn,
                                        elfldltl::DynamicRelocationInfoObserver(reloc_info),
                                        elfldltl::DynamicSymbolInfoObserver(sym_info_)));

    if (options.reloc) {
      uintptr_t bias = reinterpret_cast<uintptr_t>(mem.image().data()) - mem.base();
      ASSERT_TRUE(RelocateRelative(diag, mem, reloc_info, bias));

      auto resolve = [this, bias](const auto& ref,
                                  elfldltl::RelocateTls tls_type) -> fit::result<bool, Definition> {
        EXPECT_EQ(tls_type, elfldltl::RelocateTls::kNone) << "Should not have any tls relocs";
        elfldltl::SymbolName name{sym_info_, ref};
        if (const Sym* sym = name.Lookup(sym_info_)) {
          return fit::ok(Definition{sym, bias});
        }
        ADD_FAILURE() << name << " not found!";
        return fit::error{false};
      };

      ASSERT_TRUE(elfldltl::RelocateSymbolic(mem, diag, reloc_info, sym_info_, bias, resolve));
    }

    entry_ = mem.GetPointer<std::remove_pointer_t<decltype(entry_)>>(result->entry);
    if (options.commit) {
      mem_ = mem;

      Region relro_bounds{};
      relro_phdr_ = relro_phdr;
      if (relro_phdr_) {
        relro_bounds = LoadInfo::RelroBounds(*relro_phdr_, result->loader.page_size());
      }
      relro_ = std::move(result->loader).Commit(relro_bounds);
    }

    if (options.persist_state) {
      loader_opt_ = std::move(result->loader);
    }

    EXPECT_EQ(mem_.image().empty(), !options.commit);
  }

  template <typename T>
  T* lookup_sym(elfldltl::SymbolName name) {
    auto* sym = name.Lookup(sym_info_);
    EXPECT_NE(sym, nullptr) << name;
    return mem_.GetPointer<T>(sym->value);
  }

  void protect_relro() {
    ASSERT_TRUE(loader_opt_);
    ASSERT_TRUE(relro_phdr_);
    auto diag = elfldltl::testing::ExpectOkDiagnostics();

    ASSERT_TRUE(std::move(relro_).Commit(diag));
  }

  template <typename T>
  T* entry() const {
    return reinterpret_cast<T*>(entry_);
  }

 private:
  using Elf = elfldltl::Elf<>;
  using Phdr = Elf::Phdr;
  using Dyn = Elf::Dyn;
  using Sym = Elf::Sym;
  using size_type = Elf::size_type;

  struct Definition {
    constexpr bool undefined_weak() const { return false; }

    constexpr const Sym& symbol() const { return *symbol_; }

    constexpr size_type bias() const { return bias_; }

    // These will never actually be called.
    constexpr size_type tls_module_id() const { return 0; }
    constexpr size_type static_tls_bias() const { return 0; }

    template <class Diagnostics>
    constexpr fit::result<bool, typename Elf::TlsDescGot<>> tls_desc(Diagnostics& diag) const {
      return fit::error{false};
    }

    constexpr Elf::TlsDescGot<> tls_desc_undefined_weak() const { return {}; }

    const Sym* symbol_ = nullptr;
    size_type bias_ = 0;
  };

  void (*entry_)() = nullptr;
  elfldltl::DirectMemory mem_;
  Relro relro_;
  elfldltl::SymbolInfo<Elf> sym_info_;

  std::optional<typename Traits::Loader> loader_opt_;
  std::optional<Phdr> relro_phdr_;
};

TYPED_TEST_SUITE(ElfldltlLoaderTests, elfldltl::testing::LoaderTypes, elfldltl::testing::TestNames);

TYPED_TEST(ElfldltlLoaderTests, Basic) {
  ASSERT_NO_FATAL_FAILURE(this->Load(kRet24, {.reloc = false}));

  EXPECT_EQ(this->template entry<decltype(Return24)>()(), 24);
}

TYPED_TEST(ElfldltlLoaderTests, UnmapCorrectly) {
  ASSERT_NO_FATAL_FAILURE(this->Load(kRet24, {.commit = false, .reloc = false}));

  EXPECT_DEATH(this->template entry<decltype(Return24)>()(), "");
}

TYPED_TEST(ElfldltlLoaderTests, PersistCorrectly) {
  ASSERT_NO_FATAL_FAILURE(
      this->Load(kRet24, {.commit = false, .persist_state = true, .reloc = false}));

  EXPECT_EQ(this->template entry<decltype(Return24)>()(), 24);
}

TYPED_TEST(ElfldltlLoaderTests, DataSegments) {
  ASSERT_NO_FATAL_FAILURE(this->Load(kNoXSegment));

  TestData* data = this->template entry<TestData>();
  EXPECT_EQ(*data->rodata, 5);
  EXPECT_EQ(*data->data, 18);
  EXPECT_EQ(data->data[kSmallDataCount - 1], 1);
  EXPECT_EQ(*data->bss, 0);

  *data->data = 1;
  *data->bss = 2;
  int* rodata = const_cast<int*>(data->rodata);
  EXPECT_DEATH(*rodata = 3, "");
}

TYPED_TEST(ElfldltlLoaderTests, LargeDataSegment) {
  ASSERT_NO_FATAL_FAILURE(this->Load(kNoXSegmentLargeData));

  TestData* data = this->template entry<TestData>();
  EXPECT_EQ(*data->rodata, 5);
  EXPECT_EQ(*data->data, 18);
  EXPECT_EQ(data->data[kLargeDataCount - 1], 1);
  EXPECT_EQ(*data->bss, 0);

  *data->data = 1;
  data->data[kLargeDataCount - 1] = 9;
  *data->bss = 2;
  int* rodata = const_cast<int*>(data->rodata);
  EXPECT_DEATH(*rodata = 3, "");
}

TYPED_TEST(ElfldltlLoaderTests, LargeBssSegment) {
  ASSERT_NO_FATAL_FAILURE(this->Load(kNoXSegmentLargeBss));

  TestData* data = this->template entry<TestData>();
  EXPECT_EQ(*data->rodata, 5);
  EXPECT_EQ(*data->data, 18);
  EXPECT_EQ(data->data[kSmallDataCount - 1], 1);
  std::span bss(data->bss, kLargeBssCount);
  EXPECT_THAT(bss, testing::Each(testing::Eq(0)));

  *data->data = 1;
  *data->bss = 2;
  int* rodata = const_cast<int*>(data->rodata);
  EXPECT_DEATH(*rodata = 3, "");
}

TYPED_TEST(ElfldltlLoaderTests, ProtectRelroTest) {
  ASSERT_NO_FATAL_FAILURE(this->Load(kRelro, {.persist_state = true}));

  RelroData* data = this->template entry<RelroData>();
  ASSERT_NE(data, nullptr);

  int* volatile relrodata = const_cast<int*>(data->relocated);
  ASSERT_NE(relrodata, nullptr);

  data->relocated = nullptr;
  this->protect_relro();

  ASSERT_EQ(data->relocated, nullptr);
  EXPECT_DEATH(data->relocated = relrodata, "");
}

TYPED_TEST(ElfldltlLoaderTests, BasicSymbol) {
  ASSERT_NO_FATAL_FAILURE(this->Load(kSymbolic));

  constexpr elfldltl::SymbolName kFoo("foo");

  auto* foo_ptr = this->template lookup_sym<decltype(foo)>(kFoo);
  ASSERT_NE(foo_ptr, nullptr);
  EXPECT_EQ(*foo_ptr, 17);
}

TYPED_TEST(ElfldltlLoaderTests, ResolveSymbolic) {
  ASSERT_NO_FATAL_FAILURE(this->Load(kSymbolic));

  EXPECT_EQ(this->template lookup_sym<decltype(NeedsPlt)>(elfldltl::SymbolName{"NeedsPlt"})(), 2);
  EXPECT_EQ(this->template lookup_sym<decltype(NeedsGot)>(elfldltl::SymbolName{"NeedsGot"})(), 3);
}

}  // namespace
