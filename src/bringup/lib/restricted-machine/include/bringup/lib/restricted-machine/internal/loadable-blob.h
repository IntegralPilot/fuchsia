// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_INTERNAL_LOADABLE_BLOB_H_
#define SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_INTERNAL_LOADABLE_BLOB_H_
#include <lib/elfldltl/container.h>
#include <lib/elfldltl/diagnostics.h>
#include <lib/elfldltl/dynamic.h>
#include <lib/elfldltl/layout.h>
#include <lib/elfldltl/link.h>
#include <lib/elfldltl/load.h>
#include <lib/elfldltl/mapped-vmo-file.h>
#include <lib/elfldltl/memory.h>
#include <lib/elfldltl/note.h>
#include <lib/elfldltl/symbol.h>
#include <lib/elfldltl/vmar-loader.h>

// For VMO loading
#include <fidl/fuchsia.ldsvc/cpp/wire.h>
#include <lib/zx/channel.h>
#include <lib/zx/vmo.h>
#include <zircon/dlfcn.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <string_view>
#include <unordered_map>

namespace restricted_machine {

namespace internal {

class LoadableBlobSymbols {
 public:
  LoadableBlobSymbols() = default;

  template <class Elf>
  bool Init(elfldltl::LocalVmarLoader& loader, const elfldltl::SymbolInfo<Elf>& symbol_info,
            const std::vector<std::string_view> symbols, bool export_symbols);
  uint64_t addr_of(const std::string_view& symbol) const { return symbol_addrs_.at(symbol); }
  const std::unordered_map<std::string_view, uint64_t>& symbol_map() const { return symbol_addrs_; }

 private:
  // The string_view are only valid as long as the mapped libraries persist.
  std::unordered_map<std::string_view, uint64_t> symbol_addrs_;
};

class LibVmoLoader {
 public:
  LibVmoLoader() = default;
  ~LibVmoLoader() = default;
  virtual zx::vmo Get(std::string_view libname);
};

// This class encapsulates loading a single loadable blob.
class LoadableBlob {
 public:
  LoadableBlob() = default;
  virtual ~LoadableBlob() = default;

  // Load the blob VMO found using the prefix |name| with the machine type of
  // |machine| under the provided |address_limit|. If provided, any symbols
  // matching |symbols| will be added to the symbol map. |global_symbols| will
  // be used for resolving any missing symbol definitions.
  // If |export_symbols| is true, all symbols found in the blob will be added
  // to the map.
  // If provided, |map_at| provides the exact (within page alignment) address to
  // place the loadable vmo at.
  zx::result<> Initialize(const std::string_view& name, elfldltl::ElfMachine machine,
                          uint64_t address_limit, const std::vector<std::string_view>& symbols,
                          const std::unordered_map<std::string_view, uint64_t>& global_symbols,
                          bool export_symbols = false,
                          std::optional<zx_vaddr_t> map_at = std::nullopt) {
    return Load(name, machine, address_limit, symbols, global_symbols, export_symbols, map_at);
  }

  const LoadableBlobSymbols& symbols() const { return loadable_blob_symbols_; }

 private:
  template <typename Elf, elfldltl::ElfMachine Machine>
  bool DoSymbolicRelocation(const std::string& error, auto diag,
                            const elfldltl::SymbolInfo<Elf>& symbol_info,
                            const elfldltl::RelocationInfo<Elf>& reloc_info,
                            const std::unordered_map<std::string_view, uint64_t>& global_symbols,
                            typename Elf::size_type bias);

  static const zx::vmo& elf_vmo(const std::string_view& name);
  static void Log(std::string_view str);

  zx::result<> Load(const std::string_view& name, elfldltl::ElfMachine machine,
                    uint64_t address_limit, const std::vector<std::string_view>& symbols,
                    const std::unordered_map<std::string_view, uint64_t>& global_symbols,
                    bool export_symbols, std::optional<zx_vaddr_t> map_at);

  // Stores the addresses of the symbols in the ELF binary that are used in tests.
  LoadableBlobSymbols loadable_blob_symbols_;

  // Loads (and unloads) the ELF binary used in restricted mode. By making this
  // a member variable, we ensure that the ELF binary's lifetime is the
  // same as the symbol table. Note that loader_.Commit() is never called, and this
  // is what ensures the unmapping on destruction.
  elfldltl::LocalVmarLoader loader_;

  // Stores the ELF VMOs that can only be loaded once from bootfs.
  static std::unordered_map<std::string_view, zx::vmo> elf_vmos_;
};

}  // namespace internal

}  // namespace restricted_machine
#endif  // SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_INTERNAL_LOADABLE_BLOB_H_
