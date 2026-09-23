// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/symbols/dwarf_unit_impl.h"

#include <lib/syslog/cpp/macros.h>

#include <algorithm>

#include "llvm/DebugInfo/DWARF/DWARFContext.h"
#include "src/developer/debug/zxdb/common/ref_ptr_to.h"
#include "src/developer/debug/zxdb/symbols/address_range_map_builder.h"
#include "src/developer/debug/zxdb/symbols/dwarf_binary_impl.h"
#include "src/developer/debug/zxdb/symbols/function.h"
#include "src/developer/debug/zxdb/symbols/lazy_symbol.h"
#include "src/developer/debug/zxdb/symbols/line_table_impl.h"

namespace zxdb {

DwarfUnitImpl::DwarfUnitImpl(DwarfBinaryImpl* binary, llvm::DWARFUnit* unit)
    : binary_(binary->GetWeakPtr()), unit_(unit) {}

DwarfBinary* DwarfUnitImpl::GetBinary() const { return binary_.get(); }

llvm::DWARFUnit* DwarfUnitImpl::GetLLVMUnit() const { return unit_; }

int DwarfUnitImpl::GetDwarfVersion() const { return unit_->getVersion(); }

llvm::DWARFDie DwarfUnitImpl::GetUnitDie() const {
  // Request that the unit DIEs be extracted (false parameter means "not unit die only"). Some
  // operations that use this will iterate through the unit's DIEs which requires the cache to
  // be populated (LLVM doesn't do this lazily and will just report "no children").
  return unit_->getUnitDIE(false);
}

LazySymbol DwarfUnitImpl::FunctionForRelativeAddress(uint64_t relative_address) const {
  if (!binary_) {
    return LazySymbol();
  }

  // Only create the FuncAddrMap when we're actually dealing with DWO files. This operation is quite
  // costly in addition to creating the LLVM DIE index. Note the FuncAddrMap will work in all cases,
  // it is purely an optimization to skip creating it for non-DWO binaries. The overhead may be
  // avoided by either not indexing the LLVM cache, or by skipping the creation of this mapping. We
  // are currently choosing to take the second of those options to support DWO while keeping the
  // non-DWO case fast.
  if (unit_->isDWOUnit()) {
    EnsureFuncAddrMap();
    uint64_t offset = func_addr_to_die_offset_.Lookup(relative_address);
    if (offset)
      return binary_->GetSymbolFactory()->MakeLazy(GetDieRef(offset));
  } else {
    auto die = unit_->getSubroutineForAddress(relative_address);
    if (die.isValid())
      return binary_->GetSymbolFactory()->MakeLazy(GetDieRef(die.getOffset()));
  }

  return LazySymbol();
}

uint64_t DwarfUnitImpl::GetOffset() const {
  if (!binary_)
    return 0;
  return unit_->getOffset();
}

std::string DwarfUnitImpl::GetCompilationDir() const {
  if (!binary_)
    return std::string();

  // getCompilationDir() can return null if unset so be careful not to crash.
  const char* comp_dir = unit_->getCompilationDir();
  if (!comp_dir)
    return std::string();
  return std::string(comp_dir);
}

const LineTable& DwarfUnitImpl::GetLineTable() const {
  if (!line_table_) {
    if (binary_) {
      line_table_.emplace(GetWeakPtr(), GetLLVMLineTable());
    } else {
      line_table_.emplace();
    }
  }
  return *line_table_;
}

const llvm::DWARFDebugLine::LineTable* DwarfUnitImpl::GetLLVMLineTable() const {
  if (!binary_)
    return nullptr;
  return binary_->GetLLVMContext()->getLineTableForUnit(unit_);
}

uint64_t DwarfUnitImpl::GetDieCount() const { return unit_->getNumDIEs(); }

llvm::DWARFDie DwarfUnitImpl::GetLLVMDieAtIndex(uint64_t index) const {
  return unit_->getDIEAtIndex(index);
}

llvm::DWARFDie DwarfUnitImpl::GetLLVMDieAtOffset(DwarfDieRef die_ref) const {
  if (!die_ref.is_valid())
    return llvm::DWARFDie();
  return unit_->getDIEForOffset(die_ref.offset());
}

uint64_t DwarfUnitImpl::GetIndexForLLVMDie(const llvm::DWARFDie& die) const {
  return unit_->getDIEIndex(die);
}

DwarfDieRef DwarfUnitImpl::GetDieRef(uint64_t offset) const {
  if (unit_ && unit_->isTypeUnit())
    return DwarfDieRef::ForTypeUnit(unit_->getVersion(), offset);
  return DwarfDieRef::Main(offset);
}

DwarfDieRef DwarfUnitImpl::GetDieRef(const llvm::DWARFDie& die) const {
  return GetDieRef(die.getOffset());
}

llvm::DWARFUnit* GetUnitForOffset(const llvm::DWARFUnitVector& unit_vector, DwarfDieRef die_ref) {
  if (die_ref.section() == DwarfDieRef::Section::kMain)
    return unit_vector.getUnitForOffset(die_ref.offset());

  auto begin = unit_vector.begin() + unit_vector.getNumInfoUnits();
  auto end = unit_vector.end();
  auto it = std::upper_bound(begin, end, die_ref.offset(),
                             [](uint64_t offset, const std::unique_ptr<llvm::DWARFUnit>& unit) {
                               return offset < unit->getNextUnitOffset();
                             });
  if (it != end && (*it)->getOffset() <= die_ref.offset())
    return it->get();
  return nullptr;
}

void DwarfUnitImpl::EnsureFuncAddrMap() const {
  if (binary_ && func_addr_to_die_offset_.empty()) {
    // Recursively collect all DIEs and their begin ranges.
    AddressRangeMapBuilder builder;
    AddDieToFuncAddr(*binary_, GetUnitDie(), builder);
    func_addr_to_die_offset_ = builder.GetMap();
  }
}

void DwarfUnitImpl::AddDieToFuncAddr(const DwarfBinary& binary, const llvm::DWARFDie& die,
                                     AddressRangeMapBuilder& builder) const {
  // Add all functions and inlines to the map.
  llvm::dwarf::Tag tag = die.getTag();
  if (tag == llvm::dwarf::DW_TAG_subprogram || tag == llvm::dwarf::DW_TAG_inlined_subroutine) {
    auto lazy_func = binary.GetSymbolFactory()->MakeLazy(GetDieRef(die.getOffset()));
    if (const Function* func = lazy_func.Get()->As<Function>()) {
      builder.AddRanges(func->code_ranges(), die.getOffset());
    }
  }

  // Recurse into children.
  llvm::DWARFDie cur = die.getFirstChild();
  while (cur.isValid()) {
    AddDieToFuncAddr(binary, cur, builder);
    cur = cur.getSibling();
  }
}

}  // namespace zxdb
