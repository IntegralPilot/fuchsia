// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/result.h>
#include <lib/symbolizer-markup/writer.h>

#include <limits>

#include <bringup/lib/restricted-machine/internal/common.h>
#include <bringup/lib/restricted-machine/internal/loadable-blob.h>

namespace restricted_machine {

namespace internal {

template <class Elf>
bool LoadableBlobSymbols::Init(elfldltl::LocalVmarLoader& loader,
                               const elfldltl::SymbolInfo<Elf>& symbol_info,
                               const std::vector<std::string_view> symbols, bool export_symbols) {
  // Instead of searching for known symbols, we walk the whole list.
  // For requested symbols, we save the mapping.
  // For unresolved symbols, we remap them if requested.
  // symbol_info is a string_view subclass so we can print them all
  for (const auto& name : symbols) {
    const auto* sym = elfldltl::SymbolName(name).Lookup(symbol_info);
    if (!sym) {
      RM_LOG(ERROR) << "failed to lookup symbol: " << name;
      return false;
    }
    // We use the string_view from the library rather than what was passed in.
    std::string_view sym_name = symbol_info.string(sym->name());
    symbol_addrs_[sym_name] =
        sym->value() + static_cast<typename Elf::size_type>(loader.load_bias());
    RM_LOG(DEBUG) << "adding symbol mapping: " << name << " -> 0x" << std::hex
                  << symbol_addrs_[name];
  }
  // Now we add all the other symbols that are exported.
  if (export_symbols) {
    for (const auto& sym : symbol_info.safe_symtab()) {
      // Export internal symbols only, regardless of visibility.
      if (sym.value() == 0) {
        continue;
      }
      std::string_view sym_name = symbol_info.string(sym.name());
      if (!sym_name.empty()) {
        symbol_addrs_[sym_name] =
            sym.value() + static_cast<typename Elf::size_type>(loader.load_bias());
        RM_LOG(DEBUG) << "..." << sym_name << "@0x" << std::hex << sym.value() << " -> 0x"
                      << symbol_addrs_[sym_name];
      }
    }
  }

  return true;
}

// Lifted from src/lib/elfldltl/testing/get-test-lib-vmo.cc
zx::vmo LibVmoLoader::Get(std::string_view libname) {
  constexpr auto init_ldsvc = []() {
    // The dl_set_loader_service API replaces the handle used by `dlopen` et al
    // and returns the old one, so initialize by borrowing that handle while
    // leaving it intact in the system runtime.
    zx::unowned_channel channel{dl_set_loader_service(ZX_HANDLE_INVALID)};
    if (!channel->is_valid()) {
      RM_LOG(ERROR) << "failed to setup channel to loader service";
      return fidl::UnownedClientEnd<fuchsia_ldsvc::Loader>{channel};
    }
    zx_handle_t reset = dl_set_loader_service(channel->get());
    if (reset != ZX_HANDLE_INVALID) {
      RM_LOG(WARNING) << "dl_set_loader_service() had prior value";
    }
    return fidl::UnownedClientEnd<fuchsia_ldsvc::Loader>{channel};
  };
  static const auto ldsvc_endpoint = init_ldsvc();

  fidl::Arena arena;
  fidl::WireResult result = fidl::WireCall(ldsvc_endpoint)->LoadObject({arena, libname});
  // Expect the FIDL call to succeed.
  if (!result.ok()) {
    RM_LOG(ERROR) << "LdSvc->LoadObject(" << libname
                  << ") failed: " << zx_status_get_string(result.status());
  }
  // If the VMO was not found, return an empty object.
  if (result->rv == ZX_ERR_NOT_FOUND) {
    RM_LOG(ERROR) << "Loader service did not find a matching VMO for " << libname;
    return zx::vmo(ZX_HANDLE_INVALID);
  }
  if (result->rv != ZX_OK) {
    RM_LOG(ERROR) << "error loading vmo for library (" << libname
                  << "): " << zx_status_get_string(result->rv);
  }
  return std::move(result->object);
}

const zx::vmo& LoadableBlob::elf_vmo(const std::string_view& name) {
  // This must only be loaded once because we can only fetch it from bootfs once.
  // This is because bootfs transfers data to callers instead of copying it.
  if (elf_vmos_.contains(name)) {
    return elf_vmos_[name];
  }
  elf_vmos_[name] = LibVmoLoader().Get(name);
  if (!elf_vmos_[name]) {
    RM_LOG(ERROR) << "could not load '" << name << "'";
  }
  return elf_vmos_[name];
}

void LoadableBlob::Log(std::string_view str) {
#ifndef NDEBUG
  fprintf(stderr, "%.*s", static_cast<int>(str.size()), str.data());
#endif
}

namespace helper {
// Adapted from src/lib/elfldltl/test/loader-tests.cc
// Must meet elfldltl/resolve.h::ResolverDefinition
template <class Elf, elfldltl::ElfMachine Machine>
struct Definition {
  using Sym = Elf::Sym;
  using size_type = Elf::size_type;
  using TlsDescGot = Elf::template TlsDescGot<Machine>;

  constexpr bool undefined_weak() const { return false; }

  constexpr const Sym& symbol() const { return *symbol_; }

  constexpr size_type bias() const { return bias_; }

  // These will never actually be called.
  constexpr size_type tls_module_id() const { return 0; }
  constexpr size_type static_tls_bias() const { return 0; }

  template <class Diagnostics>
  constexpr fit::result<bool, TlsDescGot> tls_desc(Diagnostics& diag) const {
    return fit::error{false};
  }

  constexpr TlsDescGot tls_desc_undefined_weak() const { return {}; }
  const Sym* symbol_ = nullptr;
  size_type bias_ = 0;
};
}  // namespace helper

template <typename Elf, elfldltl::ElfMachine Machine>
bool LoadableBlob::DoSymbolicRelocation(
    const std::string& error, auto diag, const elfldltl::SymbolInfo<Elf>& symbol_info,
    const elfldltl::RelocationInfo<Elf>& reloc_info,
    const std::unordered_map<std::string_view, uint64_t>& global_symbols,
    typename Elf::size_type bias) {
  using Definition = helper::Definition<Elf, Machine>;
  // The callback resolves against the provided list of symbols. It resolves
  // unknown symbols to 0 but causes the caller to fail via a captured value.
  // This allows for user to see the full list of missing symbols in one
  // execution cycle rather than needing to re-run repeatedly or use objdump
  // on the loadable.
  bool unresolved_symbols = false;
  auto resolve = [&global_symbols, &unresolved_symbols, symbol_info, bias](
                     const auto& ref,
                     elfldltl::RelocateTls tls_type) -> fit::result<bool, Definition> {
    if (tls_type != elfldltl::RelocateTls::kNone) {
      RM_LOG(WARNING) << "RelocateTls found where none should exist";
    }
    elfldltl::SymbolName name{symbol_info, ref};
    if (const typename Definition::Sym* sym = name.Lookup(symbol_info)) {
      RM_LOG(DEBUG) << "resolving locally: " << std::string(name);
      return fit::ok(Definition{sym, bias});
    }
    auto addr = global_symbols.find(name);
    if (addr == global_symbols.end()) {
      RM_LOG(ERROR) << "address for symbol not provided in already loaded blobs: "
                    << std::string(name);
      unresolved_symbols = true;
      return fit::ok(Definition{&ref, 0});
    }
    RM_LOG(DEBUG) << "Resolving " << name << "  -> 0x" << std::hex << addr->second;
    return fit::ok(Definition{&ref, static_cast<typename Elf::size_type>(addr->second)});
  };
  if (!elfldltl::RelocateSymbolic<Machine>(loader_.memory(), diag, reloc_info, symbol_info, bias,
                                           resolve)) {
    // Only log extra if a message was written to diag since there won't
    // necessarily be other insight.
    if (!error.empty()) {
      RM_LOG(ERROR) << "RelocateSymbolic() failed with elfldltl error: " << error.c_str();
    }
    return false;
  }
  return !unresolved_symbols;
}

zx::result<> LoadableBlob::Load(
    const std::string_view& name, elfldltl::ElfMachine machine, uint64_t address_limit,
    const std::vector<std::string_view>& symbols,
    const std::unordered_map<std::string_view, uint64_t>& global_symbols, bool export_symbols,
    std::optional<zx_vaddr_t> map_at) {
  // Map the VMO into the test harness's address space for convenience of extracting the build ID.
  elfldltl::MappedVmoFile file;
  {
    auto result = file.Init(elf_vmo(name).borrow());
    if (!result.is_ok()) {
      RM_LOG(ERROR) << "unable to open " << name << ": " << result.status_string();
      return result.take_error();
    }
  }

  // Use a simple incrementing counter for differentiating loaded blobs.
  static std::atomic<int> counter{0};
  unsigned int id = counter.fetch_add(1);

  // Ensure early return on failure during the search.
  std::string error{};
  auto diag = elfldltl::OneStringDiagnostics(error);

  // We pass an error code into the load callback to propagate relevant errors
  // outside of the diag mechanism.
  zx::result<> load_status = zx::ok();
  // This lambda actually loads the ELF binary into the restricted address space.
  auto load = [this, &load_status, &global_symbols, &name, &address_limit, &map_at, &symbols, id,
               &file, &diag, &error, &export_symbols]<class Ehdr, class Phdr>(
                  const Ehdr& ehdr, std::span<const Phdr> phdrs) -> bool {
    using Elf = typename Ehdr::ElfLayout;
    using size_type = typename Elf::size_type;
    using Dyn = typename Elf::Dyn;
    using LoadInfo = elfldltl::LoadInfo<Elf, elfldltl::StdContainer<std::vector>::Container>;

    // Perform basic header checks. Since we may be loading for an architecture that is not the
    // same as the host architecture, we have to pass nullopt to the machine argument.
    if (!ehdr.Loadable(diag, std::nullopt)) {
      RM_LOG(ERROR) << name << " not loadable";
      load_status = zx::error(ZX_ERR_IO_INVALID);
      return false;
    }

    // This will collect the build ID from the file, for the symbolizer markup.
    std::span<const std::byte> build_id;
    auto build_id_observer = [&build_id](const auto& note) -> fit::result<fit::failed, bool> {
      if (!note.IsBuildId()) {
        // This is a different note, so keep looking.
        return fit::ok(true);
      }
      build_id = note.desc;
      // Tell the caller not to call again for another note.
      return fit::ok(false);
    };

    // Get all the essentials from the phdrs: load info, the build ID note, and
    // the PT_DYNAMIC phdr.
    LoadInfo load_info;
    std::optional<Phdr> dyn_phdr;
    if (!elfldltl::DecodePhdrs(
            diag, phdrs, load_info.GetPhdrObserver(static_cast<size_type>(loader_.page_size())),
            elfldltl::PhdrFileNoteObserver(Elf{}, file, elfldltl::NoArrayFromFile<std::byte>{},
                                           build_id_observer),
            elfldltl::PhdrDynamicObserver<Elf>(dyn_phdr))) {
      RM_LOG(ERROR) << "Failed to decode Phdrs!";
      load_status = zx::error(ZX_ERR_IO_INVALID);
      return false;
    }

    // Based on the size_type, determine if there is a hard address limit.
    static constexpr uint64_t kMaxSixtyFourValue = std::numeric_limits<uint64_t>::max();
    uint64_t hard_address_limit = kMaxSixtyFourValue;
    if constexpr (sizeof(size_type) < sizeof(uint64_t)) {
      hard_address_limit = static_cast<uint64_t>(std::numeric_limits<size_type>::max());
    }
    if (address_limit > hard_address_limit) {
      RM_LOG(INFO) << "A hard address limit has been applied: 0x" << std::hex << hard_address_limit;
      address_limit = hard_address_limit;
    }

    // loader_ uses the root vmar, so we must account for the base vmar
    // address in order to map at a specific address or enforce an address
    // limit.
    zx_info_vmar_t vmar_info = {};
    if (auto status = zx::vmar::root_self()->get_info(ZX_INFO_VMAR, &vmar_info, sizeof(vmar_info),
                                                      nullptr, nullptr);
        status != ZX_OK) {
      RM_LOG(ERROR) << "Failed to load root VMAR info.";
      load_status = zx::error(status);
      return false;
    }

    // Attempt to map the blob at a specific address
    if (map_at.has_value()) {
      uint64_t adjusted = map_at.value();
      if (adjusted < static_cast<uint64_t>(vmar_info.base)) {
        RM_LOG(ERROR) << "Requested mapping address is before the thread's VMAR.";
        load_status = zx::error(ZX_ERR_INVALID_ARGS);
        return false;
      }
      if (adjusted + load_info.vaddr_size() >
          static_cast<uint64_t>(vmar_info.base) + vmar_info.len) {
        RM_LOG(ERROR) << "Requested mapping address is after the thread's VMAR @0x" << std::hex
                      << static_cast<uint64_t>(vmar_info.base) + vmar_info.len;
        load_status = zx::error(ZX_ERR_INVALID_ARGS);
        return false;
      }
      // Subtract the base from the absolute offset to get the relative offset
      adjusted -= vmar_info.base;
      // Align to the closest page boundary.
      adjusted -= adjusted % zx_system_get_page_size();
      // Ensure the mapping will fit.
      if (adjusted + load_info.vaddr_size() > address_limit) {
        RM_LOG(ERROR) << "Requested mapping exceeds the address limit.";
        load_status = zx::error(ZX_ERR_INVALID_ARGS);
        return false;
      }
      if (!loader_.Allocate(diag, load_info, adjusted)) {
        RM_LOG(ERROR) << "failed to allocate memory at the given address 0x" << std::hex
                      << map_at.value() << " (" << adjusted << ") for " << name;
        load_status = zx::error(ZX_ERR_NO_MEMORY);
        return false;
      }
      // Attempt to allocate the ELF vmo below any machine-required maximum.
    } else if (address_limit != 0) {
      // It's worth noting that there is nothing magic about incrementing
      // by 0x20000.  The alternative would be refactoring Allocate() to take an upper
      // limit or using a much smaller offset to search for an available
      // restricted_blob-sized space.
      constexpr static size_t kMappingIncrement = 0x20000;
      bool allocated = false;
      for (size_t vmar_offset = 0x0;  // from vmar.base
           vmar_info.base + vmar_offset + load_info.vaddr_size() <= address_limit;
           vmar_offset += kMappingIncrement) {
        RM_LOG(DEBUG) << "searching for valid ELF allocation @0x" << std::hex
                      << vmar_info.base + vmar_offset;
        if (loader_.Allocate(diag, load_info, vmar_offset)) {
          allocated = true;
          break;
        }
      }
      if (!allocated) {
        RM_LOG(ERROR) << "failed to allocate addressable memory for loadable module: " << name;
        load_status = zx::error(ZX_ERR_NO_MEMORY);
        return false;
      }
    }

    if (!loader_.Load(diag, load_info, elf_vmo(name).borrow())) {
      RM_LOG(ERROR) << "cannot load " << name;
      load_status = zx::error(ZX_ERR_IO);
      return false;
    }

    // Log symbolizer markup context for the test module to ease debugging.
    symbolizer_markup::Writer markup_writer{Log};
    load_info.SymbolizerContext(
        markup_writer, id, name, build_id,
        static_cast<size_type>(load_info.vaddr_start() + loader_.load_bias()));

    // Read the PT_DYNAMIC, which leads to symbol information.
    cpp20::span<const Dyn> dyn;
    {
      if (!dyn_phdr) {
        RM_LOG(ERROR) << "no PT_DYNAMIC found in " << name;
        load_status = zx::error(ZX_ERR_IO_INVALID);
        return false;
      }
      auto read_dyn =
          loader_.memory().ReadArray<Dyn>(dyn_phdr->vaddr, dyn_phdr->filesz / sizeof(Dyn));
      if (read_dyn) {
        dyn = *read_dyn;
      } else {
        RM_LOG(WARNING) << "PT_DYNAMIC not read for " << name;
      }
    }

    // Decode PT_DYNAMIC just enough to get the symbols and relocation info
    elfldltl::SymbolInfo<Elf> symbol_info;
    elfldltl::RelocationInfo<Elf> reloc_info;
    if (!elfldltl::DecodeDynamic(diag, loader_.memory(), dyn,
                                 elfldltl::DynamicRelocationInfoObserver(reloc_info),
                                 elfldltl::DynamicSymbolInfoObserver(symbol_info))) {
      RM_LOG(ERROR) << "elfldltl::DecodeDynamic failed: " << error;
      load_status = zx::error(ZX_ERR_IO);
      return false;
    }

    uintptr_t mem_bias =
        reinterpret_cast<uintptr_t>(loader_.memory().image().data()) - loader_.memory().base();
    typename Elf::size_type bias = static_cast<typename Elf::size_type>(mem_bias);
    if (!RelocateRelative(diag, loader_.memory(), reloc_info, bias)) {
      RM_LOG(ERROR) << "elfldltl::RelocateRelative() failed: " << error;
      load_status = zx::error(ZX_ERR_IO);
      return false;
    }

    // Avoid accidentally exploring compile-time validation for unsupported
    // 32-bit configurations by guarding at compile-time here.
    // Any additional 32-bit targets will need to be added here.
    bool relocated = false;
    error = "";
    if constexpr (Elf::kClass == elfldltl::ElfClass::k64) {
      relocated = DoSymbolicRelocation<Elf, elfldltl::ElfMachine::kNative>(
          error, diag, symbol_info, reloc_info, global_symbols, bias);
      // Add supported 32-bit machine targets below.
    } else if constexpr (elfldltl::ElfMachine::kNative == elfldltl::ElfMachine::kAarch64) {
      relocated = DoSymbolicRelocation<Elf, elfldltl::ElfMachine::kArm>(
          error, diag, symbol_info, reloc_info, global_symbols, bias);
    } else {
      RM_LOG(WARNING) << "symbolic relocation not supported for the targeted machine";
    }
    if (!relocated) {
      RM_LOG(ERROR) << "failed to perform symbolic relocation for loadable blob";
      load_status = zx::error(ZX_ERR_NOT_FOUND);
      return false;
    }

    // Load the symbols we need for restricted mode tests.
    if (!loadable_blob_symbols_.Init(loader_, symbol_info, symbols, export_symbols)) {
      load_status = zx::error(ZX_ERR_NOT_FOUND);
      return false;
    }
    return true;
  };

  // This reads the ELF header just enough to dispatch to an instantiation of
  // the lambda for the specific ELF format found (accepting all four formats).
  auto phdr_allocator = [&diag]<typename T>(size_t count) {
    return elfldltl::ContainerArrayFromFile<elfldltl::StdContainer<std::vector>::Container<T>>(
        diag, "impossible")(count);
  };

  if (elfldltl::WithLoadHeadersFromFile(diag, file, phdr_allocator, load, elfldltl::ElfData::k2Lsb,
                                        machine) == false) {
    // We propagated error via load_status. If it didn't capture the error, we
    // default to ZX_ERR_INTERNAL and emit the diag message.
    if (load_status.is_ok()) {
      load_status = zx::error(ZX_ERR_INTERNAL);
      RM_LOG(ERROR) << "elfldltl::WithLoadHeadersFromFile() failed for " << name << ": " << error;
    }
    return load_status;
  }
  return zx::ok();
}

std::unordered_map<std::string_view, zx::vmo> LoadableBlob::elf_vmos_{};

}  // namespace internal

}  // namespace restricted_machine
