// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_DWARF_BINARY_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_DWARF_BINARY_H_

#include "llvm/BinaryFormat/ELF.h"
#include "src/developer/debug/zxdb/symbols/dwarf_die_ref.h"
#include "src/developer/debug/zxdb/symbols/dwarf_unit.h"
#include "src/developer/debug/zxdb/symbols/symbol_context.h"
#include "src/developer/debug/zxdb/symbols/unit_index.h"

namespace llvm {

namespace object {
class Binary;
class ObjectFile;
}  // namespace object

}  // namespace llvm

namespace zxdb {

class DwarfSymbolFactory;
class SymbolContext;

// Represents the low-level DWARF file. It provides a mockable wrapper around a llvm::DWARFContext.
//
// Unlike LLVM, a DwarfBinary represents one DWARF file on disk. In the case of Debug Fission
// (--gsplit-dwarf mode where each compilation unit has its own symbols in a .dwo file), there will
// be a separate DwarfBinary for each .dwo file as well as the main binary. These are linked
// together by ModuleSymbols which represents the symbols for a .so or executable file.
//
// In LLVM there will be one llvm::DWARFContext that covers everything but their support for Debug
// Fission is not complete enough for our use case so we roll our own.
//
// This is currently a very leaky abstraction because a lot of code was written before it was
// created and that coded uses llvm objects directly. As a result, this has some accessors for
// the underlying LLVM objects. Mocks may return null for these. If a new test needs to be written
// for such code, wrappers should be added so that the code no longer needs the llvm objects and
// can use the mockable wrappers.
class DwarfBinary {
 public:
  virtual ~DwarfBinary() = default;

  // Getters for basic binary information.
  virtual std::string GetName() const = 0;
  virtual std::string GetBuildID() const = 0;
  virtual std::time_t GetModificationTime() const = 0;

  virtual const DwarfSymbolFactory* GetSymbolFactory() const = 0;

  // Return whether this module has been given the opportunity to include symbols from the binary
  // itself, such as PLT entries.
  virtual bool HasBinary() const = 0;

  // Returns underlying LLVM objects. May be null in tests since the mock won't have this. See the
  // class comment above.
  virtual llvm::object::ObjectFile* GetLLVMObjectFile() = 0;
  virtual llvm::DWARFContext* GetLLVMContext() = 0;

  // Returns the extent of the mapped segments in memory.
  virtual uint64_t GetMappedLength() const = 0;

  // Returns symbols from the ELF File.
  virtual const std::map<std::string, llvm::ELF::Elf64_Sym>& GetELFSymbols() const = 0;
  virtual const std::map<std::string, uint64_t> GetPLTSymbols() const = 0;

  // Allows access to compile units in this binary. There are two categories, the "normal" units in
  // .debug_info and the DWO units in .debug_info.dwo. Normally there are only one or the other,
  // but this isn't guaranteed.
  virtual uint32_t GetNormalUnitCount() const = 0;
  virtual uint32_t GetDWOUnitCount() const = 0;
  virtual fxl::RefPtr<DwarfUnit> GetUnitAtIndex(UnitIndex i) = 0;

  // Returns the DwarfUnit covering the given absolute address location. Can be null if there's
  // no unit that covers this area.
  fxl::RefPtr<DwarfUnit> UnitForAddress(const SymbolContext& symbol_context,
                                        TargetPointer absolute_address) {
    return UnitForRelativeAddress(symbol_context.AbsoluteToRelative(absolute_address));
  }

  // Like UnitForAddress but takes an address relative to the load address of the binary.
  virtual fxl::RefPtr<DwarfUnit> UnitForRelativeAddress(uint64_t relative_address) = 0;

  // Returns the 64-bit value from the .debug_addr section.
  //
  // The addr_base is an attribute on the compilation unit. This is the byte offset of the address
  // table in the .debug_addr section of that compilation unit's address table.
  //
  // The index is the address index within that compilation unit's table. This is an index into the
  // table (of address-sized entries) rather than a byte index.
  virtual std::optional<uint64_t> GetDebugAddrEntry(uint64_t addr_base, uint64_t index) const = 0;

  // Returns the main unit in a .dwo file. When using DWARF "debug fission" the main binary will
  // have "skeleton units" that reference .dwo files containing the real unit information. These
  // files will normally have exactly one unit in them. This getter returns that unit.
  //
  // It's theoretically possible for a .dwo file to have more than one unit. If that happens, it
  // would be possible to disambiguate them by name. Currently this does not happen so this just
  // returns the first DWO unit.
  fxl::RefPtr<DwarfUnit> GetDwoUnit() { return GetUnitAtIndex(UnitIndex(true, 0)); }

  // Looks up a DIE across units by its DIE reference.
  virtual llvm::DWARFDie GetLLVMDieAtOffset(DwarfDieRef die_ref) const = 0;

  // Looks up a DIE reference by its 64-bit DWARF type signature (DW_FORM_ref_sig8 /
  // DW_AT_signature).
  virtual DwarfDieRef GetDieRefForSignature(uint64_t signature) const = 0;

  virtual void ClearLLVMCache() = 0;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_DWARF_BINARY_H_
