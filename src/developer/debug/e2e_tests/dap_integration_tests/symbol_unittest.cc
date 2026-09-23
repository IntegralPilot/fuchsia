// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <filesystem>
#include <vector>

#include <gtest/gtest.h>

#include "src/developer/debug/zxdb/common/err.h"
#include "src/developer/debug/zxdb/common/host_util.h"
#include "src/developer/debug/zxdb/symbols/file_line.h"
#include "src/developer/debug/zxdb/symbols/input_location.h"
#include "src/developer/debug/zxdb/symbols/location.h"
#include "src/developer/debug/zxdb/symbols/module_symbols.h"
#include "src/developer/debug/zxdb/symbols/resolve_options.h"
#include "src/developer/debug/zxdb/symbols/symbol_context.h"
#include "src/developer/debug/zxdb/symbols/system_symbols.h"

namespace {

bool ResolveLocationInSymbolDir(zxdb::SystemSymbols& system_symbols,
                                const std::filesystem::path& symbol_dir,
                                const zxdb::InputLocation& input_location) {
  for (const auto& entry : std::filesystem::recursive_directory_iterator(symbol_dir)) {
    if (!entry.is_regular_file() || entry.path().extension() != ".debug") {
      continue;
    }

    // As seen in BuildIDIndex::SearchBuildIdDirs, lookup splits build_id via:
    //   substr(0, 2) + "/" + substr(2)
    // Here we perform the inverse to reconstruct the Build ID string from the path.
    // Example: a file at ".build-id/a1/b2c3d4.debug" transforms to Build ID "a1b2c3d4".
    std::string build_id =
        entry.path().parent_path().filename().string() + entry.path().stem().string();

    fxl::RefPtr<zxdb::ModuleSymbols> debug_module;
    if (system_symbols.GetModule("", build_id, false, &debug_module).has_error() || !debug_module) {
      continue;
    }

    zxdb::SymbolContext symbol_context(0x10000000);
    if (!debug_module->ResolveInputLocation(symbol_context, input_location, zxdb::ResolveOptions())
             .empty()) {
      return true;
    }
  }
  return false;
}

TEST(DapSymbolTest, CrasherSymbolAvailable) {
  std::filesystem::path symbol_dir =
      std::filesystem::path(zxdb::GetSelfPath()).parent_path() / DAP_E2E_TESTS_SYMBOL_DIR;

  ASSERT_TRUE(std::filesystem::exists(symbol_dir))
      << "Symbol directory does not exist: " << symbol_dir;

  zxdb::SystemSymbols system_symbols([](const std::string&, zxdb::DebugSymbolFileType) {});
  system_symbols.build_id_index().AddBuildIdDir(symbol_dir.string());

  zxdb::InputLocation input_loc(zxdb::Identifier(zxdb::IdentifierQualification::kGlobal,
                                                 zxdb::IdentifierComponent("blind_write")));
  EXPECT_TRUE(ResolveLocationInSymbolDir(system_symbols, symbol_dir, input_loc))
      << "Failed to resolve 'blind_write' symbol in any module under " << symbol_dir;
}

TEST(DapSymbolTest, CrasherFileLineAvailable) {
  std::filesystem::path symbol_dir =
      std::filesystem::path(zxdb::GetSelfPath()).parent_path() / DAP_E2E_TESTS_SYMBOL_DIR;

  ASSERT_TRUE(std::filesystem::exists(symbol_dir))
      << "Symbol directory does not exist: " << symbol_dir;

  std::filesystem::path build_root =
      std::filesystem::path(zxdb::GetSelfPath()).parent_path().parent_path();
  zxdb::SystemSymbols system_symbols([](const std::string&, zxdb::DebugSymbolFileType) {});

  // Here we also add the build_dir as the build_root to perform the relative path operation.
  // The resulting relative path will be ../../src/developer/forensics/crasher/cpp/crasher.c,
  // which perfectly matches the DWARF file path pattern.
  system_symbols.build_id_index().AddBuildIdDir(symbol_dir.string(), build_root.string());

  std::string crasher_path = (build_root / "../../src/developer/forensics/crasher/cpp/crasher.c")
                                 .lexically_normal()
                                 .string();

  const int lineNumber = 25;
  EXPECT_TRUE(ResolveLocationInSymbolDir(
      system_symbols, symbol_dir, zxdb::InputLocation(zxdb::FileLine(crasher_path, lineNumber))))
      << "Failed to resolve '" << crasher_path << ":" << "lineNumber' location in any module under "
      << symbol_dir;
}

}  // namespace
