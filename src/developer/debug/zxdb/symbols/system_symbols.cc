// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/symbols/system_symbols.h"

#include <memory>

#include "src/developer/debug/shared/logging/logging.h"
#include "src/developer/debug/zxdb/common/file_util.h"
#include "src/developer/debug/zxdb/common/host_util.h"
#include "src/developer/debug/zxdb/common/ref_ptr_to.h"
#include "src/developer/debug/zxdb/symbols/dwarf_binary_impl.h"
#include "src/developer/debug/zxdb/symbols/module_symbols_impl.h"
#include "src/lib/elflib/elflib.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace zxdb {

namespace {

// Checks for a file with the given name on the local system that has the given build ID. If
// it exists, it will return nonempty paths in the Entry, identical to
// BuildIDIndex::EntryForBuildID().
BuildIDIndex::Entry LoadLocalModuleSymbols(const std::string& name, const std::string& build_id) {
  BuildIDIndex::Entry result;

  if (name.empty())
    return result;

  auto file = fopen(name.c_str(), "r");
  if (!file) {
    DEBUG_LOG(BuildIDIndex) << "Couldn't open " << name << ": " << strerror(errno);
    return result;
  }
  auto elf = elflib::ElfLib::Create(file, elflib::ElfLib::Ownership::kTakeOwnership);
  if (!elf) {
    DEBUG_LOG(SystemSymbols) << name << " is not an ELF file.";
    return result;
  }

  std::string file_build_id = elf->GetGNUBuildID();
  if (file_build_id == build_id) {
    // Matches, declare this local file contains both code and symbols.
    result.debug_info = name;
    result.binary = name;
  } else {
    DEBUG_LOG(SystemSymbols) << name << "'s build ID does not match " << build_id;
  }
  return result;
}

}  // namespace

SystemSymbols::SystemSymbols(RequestDownloadFunction f)
    : request_download_(std::move(f)), weak_factory_(this) {}

SystemSymbols::~SystemSymbols() = default;

void SystemSymbols::InjectModuleForTesting(const std::string& build_id, ModuleSymbols* module) {
  SaveModule(build_id, module);
}

Err SystemSymbols::GetModule(const std::string& name, const std::string& build_id,
                             bool force_reload_symbols, fxl::RefPtr<ModuleSymbols>* module,
                             SystemSymbols::DownloadType download_type) {
  *module = fxl::RefPtr<ModuleSymbols>();

  auto found_existing = modules_.find(build_id);
  if (found_existing != modules_.end()) {
    if (force_reload_symbols) {
      // Clear any cached symbols. Processes with existing references to the old symbols will keep
      // their existing reference to the old symbol file. This will only affect new symbol loads.
      modules_.erase(found_existing);
    } else {
      // Use cached.
      DEBUG_LOG(SystemSymbols) << "Found cached symbols for " << build_id;
      *module = RefPtrTo(found_existing->second);
      return Err();
    }
  }

  BuildIDIndex::Entry entry = build_id_index_.EntryForBuildID(build_id);

  if (enable_local_fallback_ && entry.debug_info.empty()) {
    // Local fallback is enabled and the name could be an absolute local path. See if the binary
    // matches and has symbols (this will leave entry.debug_info empty if still not found).
    entry = LoadLocalModuleSymbols(name, build_id);
  }

  if (entry.debug_info.empty() && download_type == SystemSymbols::DownloadType::kSymbols &&
      request_download_) {
    DEBUG_LOG(SystemSymbols) << "Requesting debuginfo download for " << build_id;
    request_download_(build_id, DebugSymbolFileType::kDebugInfo);
  }

  if (auto debug = elflib::ElfLib::Create(entry.debug_info)) {
    if (!debug->ProbeHasProgramBits() && entry.binary.empty() &&
        download_type == SystemSymbols::DownloadType::kBinary && request_download_) {
      // File doesn't exist or has no symbols, schedule a download.
      DEBUG_LOG(SystemSymbols) << "Requesting binary download for " << build_id;
      request_download_(build_id, DebugSymbolFileType::kBinary);
    }
  }

  if (entry.debug_info.empty()) {
    DEBUG_LOG(SystemSymbols) << "Symbols not synchronously available for " << build_id;
    return Err();  // No symbols synchronously available.
  }

  auto binary = std::make_unique<DwarfBinaryImpl>(entry.debug_info, entry.binary, build_id);
  auto module_impl = fxl::MakeRefCounted<ModuleSymbolsImpl>(std::move(binary), entry.build_dir);
  if (Err err = module_impl->Load(create_index_); err.has_error()) {
    return err;
  }

  *module = module_impl;                // Save to output parameter (as base class pointer).
  SaveModule(build_id, module->get());  // Save in cache for future use.
  return Err();
}

void SystemSymbols::SaveModule(const std::string& build_id, ModuleSymbols* module) {
  // Can't save a module that already exists.
  FX_DCHECK(modules_.find(build_id) == modules_.end());

  module->set_deletion_cb(
      [weak_system = weak_factory_.GetWeakPtr(), build_id](ModuleSymbols* module) {
        if (!weak_system)
          return;
        SystemSymbols* system = weak_system.get();

        // Only clear our reference if it's the module reporting the delete. This can get
        // out-of-sync when symbols are force-updated: new module loads will get the new symbols but
        // old processes can still have references to the old one.
        auto found = system->modules_.find(build_id);
        if (found != system->modules_.end() && module == found->second)
          system->modules_.erase(found);
      });
  modules_[build_id] = module;
}

}  // namespace zxdb
