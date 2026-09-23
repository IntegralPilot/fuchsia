// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/compat-hash.h>
#include <lib/elfldltl/gnu-hash.h>
#include <lib/elfldltl/layout.h>
#include <lib/elfldltl/symbol.h>
#include <lib/elfldltl/testing/fuzzer.h>
#include <zircon/assert.h>

#include <algorithm>

namespace {

constexpr size_t kMaxBuckets = 1024;

template <typename Hasher>
void HashFuzzer(Hasher&& hasher, uint32_t no, FuzzedDataProvider& provider) {
  auto name = provider.ConsumeRandomLengthString();
  uint32_t hash = std::forward<Hasher>(hasher)(name);
  ZX_ASSERT(hash != no);
}

template <class Elf>
struct SymbolFuzzer {
  using Sym = Elf::Sym;
  using SymbolInfo = elfldltl::SymbolInfo<Elf>;

  using FuzzerInputs = elfldltl::testing::FuzzerInput<
      // There are five separate inputs.  Only DT_SYMTAB and DT_GNU_HASH really
      // need the Addr alignment.  DT_STRTAB needs no alignment at all and
      // DT_HASH needs only Word alignment.
      sizeof(typename SymbolInfo::Addr),
      typename SymbolInfo::Sym,   // 1. DT_SYMTAB
      typename SymbolInfo::Addr,  // 2. DT_GNU_HASH
      typename SymbolInfo::Word,  // 3. DT_HASH
      char,                       // 4. DT_STRTAB
      uint8_t>;                   // 5. Provider for further fuzzing.

  int operator()(FuzzedDataProvider& provider) const {
    FuzzerInputs inputs(provider);
    auto [symtab, gnu_hash, compat_hash, strtab, blob] = inputs.inputs();

    // Use the inputs to populate a SymbolInfo.
    SymbolInfo info;
    info.set_symtab(symtab)
        .set_strtab({strtab.data(), strtab.size()})
        .set_compat_hash(compat_hash)
        .set_gnu_hash(gnu_hash);

    if (info.compat_hash() && info.compat_hash()->size() > kMaxBuckets) {
      info.set_compat_hash({});
    }
    if (info.gnu_hash() && info.gnu_hash()->size() > kMaxBuckets) {
      info.set_gnu_hash({});
    }
    if (!info.compat_hash() && !info.gnu_hash()) {
      return -1;
    }

    // Do some exhaustive iteration tests.
    auto fuzz_hash_table = [&info](const auto& table) {
      if (table) {
        auto fuzz_hash_table_entry = [&info](const Sym& sym) {
          std::string_view name = info.string(sym.name);
          for (char c : name) {
            // This doesn't do anything but ensures the compiler has to
            // generate a dereference of each character in case the pointer
            // or size is bad.
            __asm__ volatile("" : : "r"(c));
          }
        };
        std::ranges::for_each(info.HashedSymbols(*table), fuzz_hash_table_entry);
      }
    };
    fuzz_hash_table(info.compat_hash());
    fuzz_hash_table(info.gnu_hash());

    // The last input drives the rest of the operation of the fuzzer.
    // Do random lookups until the provider is out of data.
    FuzzedDataProvider blob_provider(blob.data(), blob.size());
    while (blob_provider.remaining_bytes() > 0) {
      const std::string name = blob_provider.ConsumeRandomLengthString();
      if (const auto* sym = elfldltl::SymbolName(name).Lookup(info)) {
        ZX_ASSERT(info.string(sym->name) == name);
      }
    }

    return 0;
  }
};

using Fuzzer = elfldltl::testing::ElfFuzzer<SymbolFuzzer>;

}  // namespace

extern "C" int LLVMFuzzerTestOneInput(const uint8_t* data, size_t size) {
  FuzzedDataProvider provider(data, size);
  HashFuzzer(elfldltl::CompatHashString, elfldltl::kCompatNoHash, provider);
  HashFuzzer(elfldltl::GnuHashString, elfldltl::kGnuNoHash, provider);
  return Fuzzer{}(provider);
}
