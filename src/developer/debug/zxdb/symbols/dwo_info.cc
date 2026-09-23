// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/symbols/dwo_info.h"

#include "src/developer/debug/zxdb/symbols/compile_unit.h"
#include "src/developer/debug/zxdb/symbols/dwarf_die_ref.h"
#include "src/developer/debug/zxdb/symbols/module_symbols.h"
#include "src/developer/debug/zxdb/symbols/symbol.h"
#include "src/developer/debug/zxdb/symbols/symbol_factory.h"

namespace zxdb {

Err DwoInfo::Load(const std::string& name, const std::string& binary_name) {
  binary_ = std::make_unique<DwarfBinaryImpl>(name, binary_name, std::string());
  if (Err err = binary_->Load(weak_factory_.GetWeakPtr(), DwarfSymbolFactory::kDWO);
      err.has_error()) {
    binary_.reset();
    return err;
  }
  return Err();
}

CompileUnit* DwoInfo::GetSkeletonCompileUnit() {
  if (!skeleton_unit_) {
    // The skeleton DIE offset refers into the MAIN BINARY so we must use the main binary's symbol
    // factory to create it, not our symbol_factory_ (for the .dwo file).
    //
    // This must not be a temporary because it will own the pointers for the duration of this fn.
    LazySymbol lazy_skeleton = module_symbols_->GetSymbolFactory()->MakeLazy(
        DwarfDieRef::Main(skeleton_.skeleton_die_offset));
    if (const CompileUnit* unit = lazy_skeleton.Get()->As<CompileUnit>()) {
      skeleton_unit_ = RefPtrTo<CompileUnit>(unit);
    }
  }
  // Returns a pointer to our cached value. This assumes the cache is not cleared.
  return skeleton_unit_.get();
}

}  // namespace zxdb
