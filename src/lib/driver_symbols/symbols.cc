// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "symbols.h"

#include <elf.h>
#include <lib/zircon-internal/align.h>
#include <lib/zx/vmar.h>
#include <zircon/status.h>

#include <algorithm>
#include <string>
#include <string_view>

#include "restricted_symbols.h"
#include "zircon/system/public/zircon/errors.h"

namespace {

const uint8_t kElfMagic[] = {0x7f, 'E', 'L', 'F'};

// Holds a string table that has been mapped into memory.
class MappedStringsTable {
 public:
  // Maps the table located at |offset| of |len| in |driver_vmo|.
  static zx::result<MappedStringsTable> Create(const zx::unowned_vmo& driver_vmo, uint64_t offset,
                                               uint64_t len) {
    const size_t kPageSize = zx_system_get_page_size();
    // Mapping only works on page-aligned values.
    auto mapped_offset = ZX_ROUNDDOWN(offset, kPageSize);
    auto mapped_size = ZX_ROUNDUP(len + (offset - mapped_offset), kPageSize);
    zx_vaddr_t mapped_addr;
    zx_status_t status = zx::vmar::root_self()->map(ZX_VM_PERM_READ, 0, *driver_vmo, mapped_offset,
                                                    mapped_size, &mapped_addr);
    if (status != ZX_OK) {
      return zx::error(status);
    }
    const char* table = reinterpret_cast<char*>(mapped_addr + (offset - mapped_offset));
    return zx::ok(MappedStringsTable(table, mapped_addr, mapped_size));
  }

  ~MappedStringsTable() {
    if (!ptr_) {
      return;
    }
    [[maybe_unused]] zx_status_t status = zx::vmar::root_self()->unmap(mapped_addr_, mapped_size_);
    ptr_ = nullptr;
    mapped_addr_ = 0;
    mapped_size_ = 0;
  }

  // Disallow copying.
  MappedStringsTable(const MappedStringsTable&) = delete;
  MappedStringsTable& operator=(const MappedStringsTable&) = delete;

  MappedStringsTable(MappedStringsTable&& other)
      : MappedStringsTable(other.ptr_, other.mapped_addr_, other.mapped_size_) {
    other.ptr_ = nullptr;
    other.mapped_addr_ = 0;
    other.mapped_size_ = 0;
  }

  MappedStringsTable& operator=(MappedStringsTable&& other) {
    ptr_ = other.ptr_;
    mapped_addr_ = other.mapped_addr_;
    mapped_size_ = other.mapped_size_;
    other.ptr_ = nullptr;
    other.mapped_addr_ = 0;
    other.mapped_size_ = 0;
    return *this;
  }

  // Returns the pointer to the mapped table.
  const char* ptr() { return ptr_; }

 private:
  MappedStringsTable(const char* ptr, uint64_t mapped_addr, uint64_t mapped_size)
      : ptr_(ptr), mapped_addr_(mapped_addr), mapped_size_(mapped_size) {}

  const char* ptr_ = nullptr;
  uint64_t mapped_addr_ = 0;
  uint64_t mapped_size_ = 0;
};

zx::result<Elf64_Ehdr> GetElfHeader(const zx::unowned_vmo& driver_vmo) {
  Elf64_Ehdr elf_header;
  zx_status_t status = driver_vmo->read(&elf_header, 0, sizeof(elf_header));
  if (status != ZX_OK) {
    return zx::error(status);
  }
  if (memcmp(elf_header.e_ident, kElfMagic, sizeof(kElfMagic)) != 0) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (elf_header.e_shoff == 0) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  return zx::ok(elf_header);
}

// Returns the section header located at |section_index| in the section header table.
zx::result<Elf64_Shdr> GetSectionHeader(const zx::unowned_vmo& driver_vmo,
                                        const Elf64_Ehdr& elf_header, uint32_t section_index) {
  Elf64_Shdr section_header;
  // The start of the section header table is located at |e_shoff| in the driver vmo.
  uint64_t vmo_offset = elf_header.e_shoff + (section_index * sizeof(section_header));
  zx_status_t status = driver_vmo->read(&section_header, vmo_offset, sizeof(section_header));
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(section_header);
}

// Returns the section header matching |header_type|, or ZX_ERR_NOT_FOUND if no match is found.
zx::result<Elf64_Shdr> FindSectionHeader(const zx::unowned_vmo& driver_vmo,
                                         const Elf64_Ehdr& elf_header, Elf32_Word header_type) {
  // |e_shnum| holds the number of section header table entries.
  for (uint16_t i = 0; i < elf_header.e_shnum; i++) {
    auto section_header = GetSectionHeader(driver_vmo, elf_header, i);
    if (section_header->sh_type == SHT_DYNSYM) {
      return section_header;
    }
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

zx::result<std::string> GetRelativeUrl(std::string_view url) {
  constexpr std::string_view kPrefix = "#meta/";

  size_t prefix_start_idx = url.find(kPrefix);
  if (prefix_start_idx == std::string::npos) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  return zx::ok(url.substr(prefix_start_idx));
}

}  // namespace

namespace driver_symbols {

zx::result<std::vector<std::string>> FindRestrictedSymbols(zx::unowned_vmo driver_vmo,
                                                           std::string_view driver_url) {
  auto relative_url = GetRelativeUrl(driver_url);
  if (relative_url.is_error()) {
    return relative_url.take_error();
  }

  zx::result<Elf64_Ehdr> elf_header = GetElfHeader(driver_vmo);
  if (elf_header.is_error()) {
    return elf_header.take_error();
  }

  // Find the section header for the dynamic symbols table.
  zx::result<Elf64_Shdr> dynsym_header = FindSectionHeader(driver_vmo, *elf_header, SHT_DYNSYM);
  if (dynsym_header.is_error()) {
    return dynsym_header.take_error();
  }

  // Find the section header for the dynamic symbols string table.
  if (dynsym_header->sh_link == 0) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  auto dynsym_strings_header = GetSectionHeader(driver_vmo, *elf_header, dynsym_header->sh_link);
  if (dynsym_strings_header.is_error()) {
    return dynsym_strings_header.take_error();
  }

  // Map in the dynamic symbols string table.
  auto dynsym_strings_table = MappedStringsTable::Create(
      driver_vmo, dynsym_strings_header->sh_offset, dynsym_strings_header->sh_size);
  if (dynsym_strings_table.is_error()) {
    return dynsym_strings_table.take_error();
  }

  // Iterate through the dynamic symbols table and look for restricted symbols.
  std::vector<std::string> matches;
  uint64_t symbols_start = dynsym_header->sh_offset;
  uint64_t symbols_end = symbols_start + dynsym_header->sh_size;
  for (uint64_t offset = symbols_start; offset < symbols_end; offset += sizeof(Elf64_Sym)) {
    Elf64_Sym symbol;
    zx_status_t status = driver_vmo->read(&symbol, offset, sizeof(symbol));
    if (status != ZX_OK) {
      return zx::error(status);
    }
    if (symbol.st_name == 0) {
      // It's valid for a symbol to not have a name.
      continue;
    }
    // |st_name| is the index into the dynamic symbols string table.
    const char* name = dynsym_strings_table->ptr() + symbol.st_name;
    if (std::ranges::binary_search(kRestrictedLibcSymbols, name)) {
      matches.push_back(name);
    } else if (std::ranges::binary_search(kRestrictedDriverRuntimeSymbols, name)) {
      if (!std::ranges::binary_search(kRestrictedDriverRuntimeSymbolsDriversAllowlist,
                                      *relative_url)) {
        matches.push_back(name);
      }
    } else if (std::ranges::binary_search(kCreateThreadSymbols, name)) {
      if (!std::ranges::binary_search(kCreateThreadSymbolsDriversAllowlist, *relative_url)) {
        matches.push_back(name);
      }
    }
  }
  return zx::ok(matches);
}

}  // namespace driver_symbols

extern "C" {

struct restricted_symbols {
  std::vector<std::string> symbols;
};

zx_status_t restricted_symbols_find(zx_handle_t driver_vmo, const char* driver_url,
                                    restricted_symbols_t** out_symbols, size_t* out_symbols_found) {
  auto result = driver_symbols::FindRestrictedSymbols(zx::unowned_vmo(driver_vmo), driver_url);
  if (result.is_error()) {
    return result.status_value();
  }

  *out_symbols_found = result.value().size();
  *out_symbols = new restricted_symbols_t(std::move(result.value()));

  return ZX_OK;
}

const char* restricted_symbols_get(restricted_symbols_t* symbols, size_t index) {
  return symbols->symbols[index].c_str();
}

void restricted_symbols_free(restricted_symbols_t* symbols) { delete symbols; }
}
