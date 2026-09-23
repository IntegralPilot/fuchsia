// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_DWO_INFO_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_DWO_INFO_H_

#include "src/developer/debug/zxdb/common/err.h"
#include "src/developer/debug/zxdb/symbols/dwarf_binary_impl.h"
#include "src/developer/debug/zxdb/symbols/dwarf_symbol_factory.h"
#include "src/developer/debug/zxdb/symbols/skeleton_unit.h"
#include "src/lib/fxl/memory/weak_ptr.h"

namespace zxdb {

class ModuleSymbols;

// Holds the per-DWO file state. Each DWO file has its own binary and associated symbol factory.
// This class glues everything together for the DWO via the DwarfSymbolFactory::Delegate interface.
//
// The ModuleSymbolsImpl class will bring all of the DwoInfo factories together with the factory
// for the main binary to give the full view of the module.
class DwoInfo final : public DwarfSymbolFactory::Delegate {
 public:
  // Must call Load() which must succeed before using.
  DwoInfo(SkeletonUnit skeleton, fxl::WeakPtr<ModuleSymbols> module_symbols)
      : skeleton_(std::move(skeleton)),
        module_symbols_(std::move(module_symbols)),
        weak_factory_(this) {}

  Err Load(const std::string& name, const std::string& binary_name);

  DwarfBinaryImpl* binary() { return binary_.get(); }
  const SkeletonUnit& skeleton() const { return skeleton_; }
  const SymbolFactory* symbol_factory() { return binary_->GetSymbolFactory(); }

  // DwarfSymbolFactory::Delegate implementation.
  std::string GetBuildDirForSymbolFactory() override { return skeleton_.comp_dir; }
  fxl::WeakPtr<ModuleSymbols> GetModuleSymbols() override { return module_symbols_; }
  CompileUnit* GetSkeletonCompileUnit() override;

 private:
  std::unique_ptr<DwarfBinaryImpl> binary_;  // Guaranteed non-null after Load() succeeds.
  SkeletonUnit skeleton_;  // Refers to the skeleton in the main binary corresponding to this DWO.
  fxl::WeakPtr<ModuleSymbols> module_symbols_;

  // Lazily constructed & cached unit corresponding to the skeleton.
  fxl::RefPtr<CompileUnit> skeleton_unit_;

  fxl::WeakPtrFactory<DwarfSymbolFactory::Delegate> weak_factory_;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_DWO_INFO_H_
