// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_DWARF_SYMBOL_FACTORY_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_DWARF_SYMBOL_FACTORY_H_

#include <llvm/DebugInfo/DWARF/DWARFAddressRange.h>
#include <llvm/Support/Error.h>  // For llvm::Expected.

#include "src/developer/debug/zxdb/common/address_range.h"
#include "src/developer/debug/zxdb/symbols/dwarf_tag.h"
#include "src/developer/debug/zxdb/symbols/symbol_factory.h"
#include "src/lib/fxl/memory/weak_ptr.h"

namespace llvm {
class DWARFContext;
class DWARFDie;
class DWARFFormValue;
}  // namespace llvm

namespace zxdb {

class AddressRanges;
class BaseType;
class CompileUnit;
class DwarfBinaryImpl;
class LazySymbol;
class ModuleSymbols;
class UncachedLazySymbol;

// Implementation of SymbolFactory that reads from the DWARF symbols in the given DwarfBinary.
//
// For Debug Fission (where each compilation unit has its symbols in a separate .dwo file), there
// will be one DwarfSymbolFactory for each binary (executable or .dwo file).
class DwarfSymbolFactory : public SymbolFactory {
 public:
  enum FileType {
    kMainBinary,  // The main executable or shared library binary.
    kDWO          // A separate DWO file.
  };

  // Provides information from the owner of this class without owning pointers to break the
  // reference cycle.
  class Delegate {
   public:
    virtual std::string GetBuildDirForSymbolFactory() = 0;
    virtual fxl::WeakPtr<ModuleSymbols> GetModuleSymbols() = 0;

    // When the current binary is a DWO, this returns the skeleton unit in the main binary
    // corresponding to this DWO. When this is not a DWO, returns null.
    virtual CompileUnit* GetSkeletonCompileUnit() = 0;
  };

  // SymbolFactory implementation.
  fxl::RefPtr<Symbol> CreateSymbol(DwarfDieRef die_ref) const override;

  using SymbolFactory::MakeLazy;
  using SymbolFactory::MakeUncachedLazy;

 private:
  FRIEND_REF_COUNTED_THREAD_SAFE(DwarfSymbolFactory);
  FRIEND_MAKE_REF_COUNTED(DwarfSymbolFactory);

  DwarfSymbolFactory(fxl::WeakPtr<DwarfBinaryImpl> binary, fxl::WeakPtr<Delegate> delegate,
                     FileType file_type);
  ~DwarfSymbolFactory() override;

  LazySymbol MakeLazy(const llvm::DWARFDie& die) const;
  UncachedLazySymbol MakeUncachedLazy(const llvm::DWARFDie& die) const;

  llvm::DWARFContext* GetLLVMContext() const;

  // Internal version that creates a symbol from a Die.
  fxl::RefPtr<Symbol> DecodeSymbol(const llvm::DWARFDie& die) const;

  // As with SymbolFactory::CreateSymbol, these should never return null but rather an empty Symbol
  // implementation on error.
  //
  // is_specification will be set when this function recursively calls itself to parse the
  // specification of a function implementation.
  //
  // The tag (DW_TAG_subprogram or DW_TAG_inlined_subroutine) is passed in because when recursively
  // looking up the definitions, we want the original DIE tag rather than the specification's tag
  // (the original could be an inlined function while the specification will never be).
  fxl::RefPtr<Symbol> DecodeFunction(const llvm::DWARFDie& die, DwarfTag tag,
                                     bool is_specification = false) const;
  fxl::RefPtr<Symbol> DecodeArrayType(const llvm::DWARFDie& die) const;
  fxl::RefPtr<Symbol> DecodeBaseType(const llvm::DWARFDie& die) const;
  fxl::RefPtr<Symbol> DecodeCallSite(const llvm::DWARFDie& die) const;
  fxl::RefPtr<Symbol> DecodeCallSiteParameter(const llvm::DWARFDie& die) const;
  fxl::RefPtr<Symbol> DecodeCollection(const llvm::DWARFDie& die) const;
  fxl::RefPtr<Symbol> DecodeCompileUnit(const llvm::DWARFDie& die, DwarfTag tag) const;
  fxl::RefPtr<Symbol> DecodeDataMember(const llvm::DWARFDie& die) const;
  fxl::RefPtr<Symbol> DecodeEnum(const llvm::DWARFDie& die) const;
  fxl::RefPtr<Symbol> DecodeFunctionType(const llvm::DWARFDie& die) const;
  fxl::RefPtr<Symbol> DecodeImportedDeclaration(const llvm::DWARFDie& die) const;
  fxl::RefPtr<Symbol> DecodeInheritedFrom(const llvm::DWARFDie& die) const;
  fxl::RefPtr<Symbol> DecodeLexicalBlock(const llvm::DWARFDie& die) const;
  fxl::RefPtr<Symbol> DecodeMemberPtr(const llvm::DWARFDie& die) const;
  fxl::RefPtr<Symbol> DecodeModifiedType(const llvm::DWARFDie& die) const;
  fxl::RefPtr<Symbol> DecodeNamespace(const llvm::DWARFDie& die) const;
  fxl::RefPtr<Symbol> DecodeTemplateParameter(const llvm::DWARFDie& die, DwarfTag tag) const;
  fxl::RefPtr<Symbol> DecodeUnspecifiedType(const llvm::DWARFDie& die) const;
  fxl::RefPtr<Symbol> DecodeVariable(const llvm::DWARFDie& die,
                                     bool is_specification = false) const;
  fxl::RefPtr<Symbol> DecodeVariant(const llvm::DWARFDie& die) const;
  fxl::RefPtr<Symbol> DecodeVariantPart(const llvm::DWARFDie& die) const;

  // Generates ranges for a CodeBlock. The attributes may be not present, this function will compute
  // what it can given the information (which may be an empty vector).
  AddressRanges GetCodeRanges(const llvm::DWARFDie& die) const;

  // Returns the single code range if it's encoded with DW_AT_low_pc and DW_AT_high_pc.
  std::optional<AddressRange> GetLowHighEncodedCodeRange(const llvm::DWARFDie& die) const;

  // Decodes the index for a DW_FORM_rnglistx encoding.
  llvm::Expected<llvm::DWARFAddressRangesVector> GetCodeRangesFromRangelistXIndex(
      const llvm::DWARFDie& die, uint64_t index) const;

  // Looks up an address from the address table, taking account the unit for the DIE. This handles
  // lookup it up in the main binary in the case of DWOs.
  std::optional<uint64_t> GetAddrFromFormValue(const llvm::DWARFDie& die,
                                               const llvm::DWARFFormValue& form) const;
  std::optional<uint64_t> GetAddrFromIndex(const llvm::DWARFDie& die, uint64_t index) const;

  // These can be null if the module is unloaded but there are still some dangling type references
  // to them.
  fxl::WeakPtr<DwarfBinaryImpl> binary_;
  fxl::WeakPtr<Delegate> delegate_;

  FileType file_type_;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_DWARF_SYMBOL_FACTORY_H_
