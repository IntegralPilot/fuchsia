// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_DWARF_UNIT_IMPL_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_DWARF_UNIT_IMPL_H_

#include <map>

#include "src/developer/debug/zxdb/symbols/address_range_map.h"
#include "src/developer/debug/zxdb/symbols/dwarf_unit.h"
#include "src/developer/debug/zxdb/symbols/line_table_impl.h"
#include "src/lib/fxl/memory/weak_ptr.h"

namespace llvm {
class DWARFDie;
class DWARFUnit;
class DWARFUnitVector;
}  // namespace llvm

namespace zxdb {

class AddressRangeMapBuilder;
class DwarfBinaryImpl;

class DwarfUnitImpl : public DwarfUnit {
 public:
  // Construct with fxl::MakeRefCounted<DwarfUnitImpl>().

  // Possibly null (this class may outlive the DwarfBinary).
  llvm::DWARFUnit* unit() const { return binary_ ? unit_ : nullptr; }

  // DwarfUnit implementation.
  DwarfBinary* GetBinary() const override;
  llvm::DWARFUnit* GetLLVMUnit() const override;
  int GetDwarfVersion() const override;
  llvm::DWARFDie GetUnitDie() const override;
  uint64_t GetOffset() const override;
  LazySymbol FunctionForRelativeAddress(uint64_t relative_address) const override;
  std::string GetCompilationDir() const override;
  const LineTable& GetLineTable() const override;
  const llvm::DWARFDebugLine::LineTable* GetLLVMLineTable() const override;
  uint64_t GetDieCount() const override;
  llvm::DWARFDie GetLLVMDieAtIndex(uint64_t index) const override;
  llvm::DWARFDie GetLLVMDieAtOffset(DwarfDieRef die_ref) const override;
  uint64_t GetIndexForLLVMDie(const llvm::DWARFDie& die) const override;
  DwarfDieRef GetDieRef(uint64_t offset) const override;
  DwarfDieRef GetDieRef(const llvm::DWARFDie& die) const override;

 private:
  FRIEND_REF_COUNTED_THREAD_SAFE(DwarfUnitImpl);
  FRIEND_MAKE_REF_COUNTED(DwarfUnitImpl);

  DwarfUnitImpl(DwarfBinaryImpl* binary, llvm::DWARFUnit* unit);

  // Call every time before using func_addr_to_die_offset_ to make sure it's lazily populated.
  void EnsureFuncAddrMap() const;

  // Recursive helper for EnsureFuncAddrMap(). Takes a pre-decoded non-weak reference to the binary_
  // weak member to avoid checking for weak pointers each time.
  void AddDieToFuncAddr(const DwarfBinary& binary, const llvm::DWARFDie& die,
                        AddressRangeMapBuilder& builder) const;

  // The binary that owns us.
  fxl::WeakPtr<DwarfBinaryImpl> binary_;

  // This pointer is owned by LLVM's DWARFContext object. Integrating LLVM's memory model with ours
  // here is a bit messy. In practice this means that the DwarfBinary outlives all DwarfUnits, and
  // users should check that the binary_ is still valid before dereferencing.
  llvm::DWARFUnit* unit_;

  // Maps the begin address of each code range in this DIE (the key) to the DIE offset of the
  // function covering that address (the value). Computed lazily by EnsureFuncAddrMap().
  //
  // This could be optimized for reading. During population we actually need to decode each symbol
  // to get the address ranges, and then throw everything away except the DIE offset. We could store
  // a LazySymbol here or actually the full symbol information. Currently, we assume that the
  // debugger is more memory-limited than CPU limited and that decoding symbols is relatively
  // inexpensive, so it's better to have the smaller data.
  mutable AddressRangeMap func_addr_to_die_offset_;

  // The line table. Computed lazily.
  mutable std::optional<LineTableImpl> line_table_;
};

llvm::DWARFUnit* GetUnitForOffset(const llvm::DWARFUnitVector& unit_vector, DwarfDieRef die_ref);

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_DWARF_UNIT_IMPL_H_
