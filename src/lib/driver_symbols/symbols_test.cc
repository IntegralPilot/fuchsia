// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/driver_symbols/symbols.h"

#include <elf.h>

#include <vector>

#include <zxtest/zxtest.h>

namespace {

void CreateFakeElf(const std::vector<std::string>& symbols, zx::vmo* out_vmo) {
  // We will write the following to the vmo:
  // * Elf header
  // * Dynamic symbols table
  // * Dynamic symbols string table
  // * Section header table
  const uint64_t kDynamicSymbolsTableOffset = 0x0500;
  const uint64_t kStringTableOffset = 0x1ff0;
  const uint64_t kSectionHeaderTableOffset = 0x2500;
  const uint64_t kVmoSize = 0x4000;

  const uint16_t kNumSections = 2;

  Elf64_Ehdr elf_header = {
      .e_ident = {0x7f, 'E', 'L', 'F'},
      .e_shoff = kSectionHeaderTableOffset,
      .e_shentsize = sizeof(Elf64_Shdr),
      .e_shnum = 2,  // Number of entries in the section header table.
  };
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kVmoSize, 0, &vmo));
  ASSERT_OK(vmo.write(&elf_header, 0, sizeof(elf_header)));

  // Write each symbol to the dynamic symbols table and strings table.
  uint64_t symbols_index = 0;
  uint32_t string_table_index = 1;  // 0 is not a valid index for a symbol string.
  for (auto& symbol : symbols) {
    Elf64_Sym symbol_entry = {
        .st_name = string_table_index,  // Index into string table.
    };
    ASSERT_OK(vmo.write(&symbol_entry,
                        kDynamicSymbolsTableOffset + (symbols_index * sizeof(symbol_entry)),
                        sizeof(symbol_entry)));
    ASSERT_OK(
        vmo.write(symbol.c_str(), kStringTableOffset + string_table_index, symbol.length() + 1));
    symbols_index++;
    string_table_index += symbol.length() + 1;
  }

  // Write the section headers for the dynamic symbols table and strings table.
  Elf64_Shdr section_headers[kNumSections] = {
      {
          .sh_type = SHT_DYNSYM,
          .sh_offset = kDynamicSymbolsTableOffset,
          .sh_size = symbols.size() * sizeof(Elf64_Sym),
          .sh_link = 1,
          .sh_entsize = sizeof(Elf64_Sym),
      },
      {
          .sh_type = SHT_STRTAB,
          .sh_offset = kStringTableOffset,
          .sh_size = string_table_index,
      },
  };
  for (uint16_t i = 0; i < kNumSections; i++) {
    ASSERT_OK(vmo.write(&section_headers[i],
                        kSectionHeaderTableOffset + (i * sizeof(section_headers[i])),
                        sizeof(section_headers[i])));
  }

  *out_vmo = std::move(vmo);
}

TEST(SymbolsTest, InvalidFormat) {
  constexpr std::string_view kDriverUrl = "fuchsia-boot:///#meta/driver.cm";
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(4096, 0, &vmo));
  auto result = driver_symbols::FindRestrictedSymbols(zx::unowned(vmo), kDriverUrl);
  ASSERT_TRUE(result.is_error());
}

TEST(SymbolsTest, NoRestrictedSymbols) {
  const std::vector<std::string> kSymbols = {
      "memcmp",
      "printf",
  };
  constexpr std::string_view kDriverUrl = "fuchsia-boot:///#meta/driver.cm";

  zx::vmo vmo;
  ASSERT_NO_FATAL_FAILURE(CreateFakeElf(kSymbols, &vmo));
  auto result = driver_symbols::FindRestrictedSymbols(zx::unowned(vmo), kDriverUrl);
  ASSERT_TRUE(result.is_ok());

  ASSERT_EQ(result->size(), 0);
}

TEST(SymbolsTest, RestrictedSymbols) {
  const std::vector<std::string> kSymbols = {
      "dlopen",
      "strlen",
      "setenv",
  };
  std::vector<std::string> want_symbols = {
      "dlopen",
      "setenv",
  };
  constexpr std::string_view kDriverUrl = "fuchsia-boot:///#meta/driver.cm";

  zx::vmo vmo;
  ASSERT_NO_FATAL_FAILURE(CreateFakeElf(kSymbols, &vmo));
  auto result = driver_symbols::FindRestrictedSymbols(zx::unowned(vmo), kDriverUrl);
  ASSERT_TRUE(result.is_ok());

  std::sort(want_symbols.begin(), want_symbols.end());
  std::sort(result->begin(), result->end());
  ASSERT_EQ(*result, want_symbols);
}

TEST(SymbolsTest, ThreadSymbolsAllowedDriver) {
  const std::vector<std::string> kSymbols = {
      "thrd_create",
  };
  constexpr std::string_view kDriverUrl = "fuchsia-boot:///#meta/platform-bus-x86.cm";

  zx::vmo vmo;
  ASSERT_NO_FATAL_FAILURE(CreateFakeElf(kSymbols, &vmo));
  auto result = driver_symbols::FindRestrictedSymbols(zx::unowned(vmo), kDriverUrl);
  ASSERT_TRUE(result.is_ok());

  ASSERT_EQ(result->size(), 0);
}

TEST(SymbolsTest, ThreadSymbolsRestrictedDriver) {
  const std::vector<std::string> kSymbols = {
      "thrd_create",
  };
  std::vector<std::string> want_symbols = {
      "thrd_create",
  };
  constexpr std::string_view kDriverUrl = "fuchsia-boot:///#meta/not-on-thread-allowlist.cm";

  zx::vmo vmo;
  ASSERT_NO_FATAL_FAILURE(CreateFakeElf(kSymbols, &vmo));
  auto result = driver_symbols::FindRestrictedSymbols(zx::unowned(vmo), kDriverUrl);
  ASSERT_TRUE(result.is_ok());

  std::sort(want_symbols.begin(), want_symbols.end());
  std::sort(result->begin(), result->end());
  ASSERT_EQ(*result, want_symbols);
}

}  // namespace
