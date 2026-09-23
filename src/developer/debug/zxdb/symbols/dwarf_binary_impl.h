// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_DWARF_BINARY_IMPL_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_DWARF_BINARY_IMPL_H_

#include <map>
#include <optional>

#include "llvm/DebugInfo/DWARF/DWARFUnit.h"
#include "src/developer/debug/zxdb/common/err.h"
#include "src/developer/debug/zxdb/symbols/dwarf_binary.h"
#include "src/developer/debug/zxdb/symbols/dwarf_symbol_factory.h"
#include "src/lib/fxl/memory/weak_ptr.h"

namespace llvm {

class DWARFCompileUnit;
class DWARFContext;
class MemoryBuffer;

namespace object {
class Binary;
}  // namespace object

}  // namespace llvm

namespace zxdb {

class DwarfBinaryImpl final : public DwarfBinary {
 public:
  // Callers must call Load() to complete initialization (which can fail).
  DwarfBinaryImpl(const std::string& name, const std::string& binary_name,
                  const std::string& build_id);
  ~DwarfBinaryImpl();

  fxl::WeakPtr<DwarfBinaryImpl> GetWeakPtr();

  Err Load(fxl::WeakPtr<DwarfSymbolFactory::Delegate> delegate,
           DwarfSymbolFactory::FileType file_type);

  // DwarfBinary implementation.
  std::string GetName() const override;
  std::string GetBuildID() const override;
  std::time_t GetModificationTime() const override;
  const DwarfSymbolFactory* GetSymbolFactory() const override;
  bool HasBinary() const override;
  llvm::object::ObjectFile* GetLLVMObjectFile() override;
  llvm::DWARFContext* GetLLVMContext() override;
  uint64_t GetMappedLength() const override;
  const std::map<std::string, llvm::ELF::Elf64_Sym>& GetELFSymbols() const override;
  const std::map<std::string, uint64_t> GetPLTSymbols() const override;
  uint32_t GetNormalUnitCount() const override;
  uint32_t GetDWOUnitCount() const override;
  fxl::RefPtr<DwarfUnit> GetUnitAtIndex(UnitIndex i) override;
  fxl::RefPtr<DwarfUnit> UnitForRelativeAddress(uint64_t relative_address) override;
  std::optional<uint64_t> GetDebugAddrEntry(uint64_t addr_base, uint64_t index) const override;
  llvm::DWARFDie GetLLVMDieAtOffset(DwarfDieRef die_ref) const override;
  DwarfDieRef GetDieRefForSignature(uint64_t signature) const override;
  void ClearLLVMCache() override;

 private:
  // Lazily creates a unit for us and returns it. This can handle null input pointers, which will
  // result in a null output pointer.
  fxl::RefPtr<DwarfUnit> FromLLVMUnit(llvm::DWARFUnit* llvm_unit);

  // Call every time before using signature_to_die_ to make sure it's lazily populated.
  void EnsureSignatureMap() const;

  const std::string name_;
  const std::string binary_name_;
  const std::string build_id_;

  std::unique_ptr<llvm::MemoryBuffer> binary_buffer_;  // Backing for binary_.
  std::unique_ptr<llvm::object::Binary> binary_;
  std::unique_ptr<llvm::DWARFContext> context_;  // binary_ must outlive this.

  class DebugAranges;
  std::unique_ptr<DebugAranges> debug_aranges_;  // Replaces context_->Aranges.

  std::time_t modification_time_ = 0;  // Set when the file is loaded.

  std::map<std::string, llvm::ELF::Elf64_Sym> elf_symbols_;
  std::map<std::string, uint64_t> plt_symbols_;

  fxl::RefPtr<DwarfSymbolFactory> symbol_factory_;

  // Holds the mapping between LLVM units and our cached unit wrappers that reference them.
  mutable std::map<const llvm::DWARFUnit*, fxl::RefPtr<DwarfUnit>> unit_map_;

  // Maps 64-bit type unit signatures to their DIE references. Computed lazily by
  // EnsureSignatureMap() by scanning all type units in both normal and DWO unit vectors.
  mutable std::optional<std::map<uint64_t, DwarfDieRef>> signature_to_die_;

  uint64_t mapped_length_ = 0;

  fxl::WeakPtrFactory<DwarfBinaryImpl> weak_factory_;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_DWARF_BINARY_IMPL_H_
