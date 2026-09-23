// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/symbols/dwarf_binary_impl.h"

#include <lib/syslog/cpp/macros.h>

#include <algorithm>
#include <cstdint>
#include <limits>
#include <memory>
#include <unordered_set>

#include <llvm/DebugInfo/DIContext.h>
#include <llvm/DebugInfo/DWARF/DWARFCompileUnit.h>
#include <llvm/DebugInfo/DWARF/DWARFContext.h>
#include <llvm/DebugInfo/DWARF/DWARFDebugArangeSet.h>
#include <llvm/DebugInfo/DWARF/DWARFTypeUnit.h>
#include <llvm/DebugInfo/DWARF/DWARFUnit.h>
#include <llvm/Object/Binary.h>
#include <llvm/Object/ELFObjectFile.h>
#include <llvm/Object/ObjectFile.h>
#include <llvm/Support/Error.h>

#include "src/developer/debug/shared/logging/logging.h"
#include "src/developer/debug/zxdb/common/file_util.h"
#include "src/developer/debug/zxdb/symbols/dwarf_unit_impl.h"
#include "src/lib/elflib/elflib.h"

namespace zxdb {

namespace {

uint64_t ComputeMappedLength(elflib::ElfLib& elf) {
  uint64_t max = 0;
  for (const elflib::Elf64_Phdr& header : elf.GetSegmentHeaders()) {
    // Only check segments that are loaded. Some segments contain things like DWARF symbols that
    // won't be loaded. Here we only want the size in-memory to resolve addresses in the program's
    // address space.
    if (header.p_type == elflib::PT_LOAD)
      max = std::max(max, header.p_vaddr + header.p_memsz);
  }
  return max;
}

// Merges the "symbols" and "dynamic symbols" into a single map. Returns an empty map if the symbols
// couldn't be loaded.
//
// The ".dynsym" table is normally described as containing a subset of the information (just the
// global symbols) in the ".symtab" section. But in a stripped binary, there will be only a
// ".dynsym" section. To handle all the cases, this merges both tables. If a name is the same, this
// assumes the symbols are the same. The non-dynamic one will be used in the case of duplicates.
std::map<std::string, elflib::Elf64_Sym> GetMergedElfSymbols(elflib::ElfLib& elf) {
  std::map<std::string, elflib::Elf64_Sym> result;

  if (auto dyn = elf.GetAllDynamicSymbols())
    result = std::move(*dyn);

  // Merge in the ".symtab" section, overwriting any definitions that are duplicates.
  if (auto sym = elf.GetAllSymbols()) {
    for (const auto& pair : *sym) {
      result.insert(pair);
    }
  }

  return result;
}

// This function exists to filter out specific errors when decompressing
// .debug_gdb_scripts sections from compressed debuginfo files which fail in
// LLVM.
void LLVMErrorHandler(llvm::Error error) {
  llvm::handleAllErrors(std::move(error), [](llvm::ErrorInfoBase& info) {
    if (info.message().find("gdb_scripts")) {
      // Suppress errors for decompressing the "debug_gdb_scripts" section in rust binaries.
      return;
    }

    // Otherwise just pass through to the console like the defaut error handler.
    LOGS(Error) << info.message();
  });
}

}  // namespace

// To work around https://github.com/llvm/llvm-project/issues/58641.
class DwarfBinaryImpl::DebugAranges {
 public:
  explicit DebugAranges(llvm::DWARFContext* context) {
    std::unordered_set<uint64_t> parsed_units;

    // Extract aranges from .debug_aranges section first.
    llvm::DWARFDataExtractor aranges(context->getDWARFObj().getArangesSection(),
                                     context->isLittleEndian(), 0);
    uint64_t aranges_offset = 0;

    while (aranges.isValidOffset(aranges_offset)) {
      llvm::DWARFDebugArangeSet set;
      if (auto err = set.extract(aranges, &aranges_offset, OnWarning)) {
        LOGS(Error) << llvm::toString(std::move(err));
        break;
      }
      uint64_t offset = set.getCompileUnitDIEOffset();
      for (const auto& desc : set.descriptors()) {
        // getEndAddress() might overflow.
        if (desc.Address && desc.getEndAddress() > desc.Address)
          ranges_.push_back({desc.Address, desc.getEndAddress(), offset});
      }
      parsed_units.insert(offset);
    }

    // Generate aranges from compile units because .debug_aranges could be incomplete.
    // But we can skip those units that we have parsed.
    for (const auto& compile_unit : context->compile_units()) {
      uint64_t offset = compile_unit->getOffset();
      if (parsed_units.insert(offset).second) {
        if (auto ranges = compile_unit->collectAddressRanges()) {
          for (const auto& range : *ranges) {
            if (range.LowPC && range.HighPC > range.LowPC)
              ranges_.push_back({range.LowPC, range.HighPC, offset});
          }
        } else {
          LOGS(Error) << llvm::toString(ranges.takeError());
        }
      }
    }

    std::sort(ranges_.begin(), ranges_.end(),
              [](const Range& first, const Range& second) { return first.begin < second.begin; });
    // Check
    Range* prev = nullptr;
    for (Range& range : ranges_) {
      if (prev && prev->end > range.begin) {
        // This could happen when the linker resolves the same symbol in different CUs to the same
        // address in the final executable.
        prev->end = range.begin;
      }
      prev = &range;
    }
  }

  uint64_t FindAddress(uint64_t address) const {
    auto it = std::partition_point(ranges_.begin(), ranges_.end(),
                                   [address](const Range& range) { return range.end <= address; });
    // it is the first range that has end > address.
    if (it != ranges_.end() && it->begin <= address) {
      return it->cu_offset;
    }
    return -1ULL;
  }

 private:
  static void OnWarning(llvm::Error err) { LOGS(Warn) << llvm::toString(std::move(err)); }

  struct Range {
    uint64_t begin;
    uint64_t end;
    uint64_t cu_offset;
  };
  std::vector<Range> ranges_;
};

DwarfBinaryImpl::DwarfBinaryImpl(const std::string& name, const std::string& binary_name,
                                 const std::string& build_id)
    : name_(name), binary_name_(binary_name), build_id_(build_id), weak_factory_(this) {}

DwarfBinaryImpl::~DwarfBinaryImpl() {}

fxl::WeakPtr<DwarfBinaryImpl> DwarfBinaryImpl::GetWeakPtr() { return weak_factory_.GetWeakPtr(); }

std::string DwarfBinaryImpl::GetName() const { return name_; }

std::string DwarfBinaryImpl::GetBuildID() const { return build_id_; }

std::time_t DwarfBinaryImpl::GetModificationTime() const { return modification_time_; }

const DwarfSymbolFactory* DwarfBinaryImpl::GetSymbolFactory() const {
  return symbol_factory_.get();
}

bool DwarfBinaryImpl::HasBinary() const {
  if (!binary_name_.empty())
    return true;

  if (auto debug = elflib::ElfLib::Create(name_))
    return debug->ProbeHasProgramBits();

  return false;
}

Err DwarfBinaryImpl::Load(fxl::WeakPtr<DwarfSymbolFactory::Delegate> delegate,
                          DwarfSymbolFactory::FileType file_type) {
  symbol_factory_ = fxl::MakeRefCounted<DwarfSymbolFactory>(weak_factory_.GetWeakPtr(),
                                                            std::move(delegate), file_type);

  if (auto debug = elflib::ElfLib::Create(name_)) {
    if (debug->ProbeHasProgramBits()) {
      // Found in ".debug" file.
      plt_symbols_ = debug->GetPLTOffsets();
      elf_symbols_ = GetMergedElfSymbols(*debug);
      mapped_length_ = ComputeMappedLength(*debug);
    } else if (auto elf = elflib::ElfLib::Create(binary_name_)) {
      // Found in binary file.
      plt_symbols_ = elf->GetPLTOffsets();
      elf_symbols_ = GetMergedElfSymbols(*elf);
      mapped_length_ = ComputeMappedLength(*elf);
    }
  }

  llvm::Expected<llvm::object::OwningBinary<llvm::object::Binary>> bin_or_err =
      llvm::object::createBinary(name_);
  if (!bin_or_err) {
    auto err_str = llvm::toString(bin_or_err.takeError());
    return Err("Error loading symbols for \"" + name_ + "\": " + err_str);
  }

  modification_time_ = GetFileModificationTime(name_);

  auto binary_pair = bin_or_err->takeBinary();
  binary_buffer_ = std::move(binary_pair.second);
  binary_ = std::move(binary_pair.first);

  // Overwrite the default Error handler object, but leave everything else default.
  context_ = llvm::DWARFContext::create(*GetLLVMObjectFile(),
                                        llvm::DWARFContext::ProcessDebugRelocations::Process,
                                        nullptr, "", &LLVMErrorHandler);

  return Err();
}

llvm::object::ObjectFile* DwarfBinaryImpl::GetLLVMObjectFile() {
  return static_cast<llvm::object::ObjectFile*>(binary_.get());
}

llvm::DWARFContext* DwarfBinaryImpl::GetLLVMContext() { return context_.get(); }

uint64_t DwarfBinaryImpl::GetMappedLength() const { return mapped_length_; }

const std::map<std::string, llvm::ELF::Elf64_Sym>& DwarfBinaryImpl::GetELFSymbols() const {
  return elf_symbols_;
}

const std::map<std::string, uint64_t> DwarfBinaryImpl::GetPLTSymbols() const {
  return plt_symbols_;
}

uint32_t DwarfBinaryImpl::GetNormalUnitCount() const {
  auto unit_range = context_->normal_units();
  return unit_range.end() - unit_range.begin();
}

uint32_t DwarfBinaryImpl::GetDWOUnitCount() const {
  auto unit_range = context_->dwo_units();
  return unit_range.end() - unit_range.begin();
}

fxl::RefPtr<DwarfUnit> DwarfBinaryImpl::GetUnitAtIndex(UnitIndex i) {
  llvm::DWARFUnit* unit = nullptr;
  // LLVM's DWARFContext stores compile units followed by type units in a single unified
  // unit vector. The total unit count across both sections is NumCompileUnits + NumTypeUnits.
  if (i.is_dwo) {
    FX_DCHECK(i.index < context_->getNumDWOCompileUnits() + context_->getNumDWOTypeUnits());
    unit = context_->getDWOUnitAtIndex(i.index);
  } else {
    FX_DCHECK(i.index < context_->getNumCompileUnits() + context_->getNumTypeUnits());
    unit = context_->getUnitAtIndex(i.index);
  }
  return FromLLVMUnit(unit);
}

fxl::RefPtr<DwarfUnit> DwarfBinaryImpl::UnitForRelativeAddress(uint64_t relative_address) {
  if (!debug_aranges_) {
    debug_aranges_ = std::make_unique<DebugAranges>(context_.get());
  }
  return FromLLVMUnit(
      context_->getCompileUnitForOffset(debug_aranges_->FindAddress(relative_address)));
}

fxl::RefPtr<DwarfUnit> DwarfBinaryImpl::FromLLVMUnit(llvm::DWARFUnit* llvm_unit) {
  if (!llvm_unit)
    return fxl::RefPtr<DwarfUnit>();

  auto found = unit_map_.find(llvm_unit);
  if (found == unit_map_.end()) {
    auto unit = fxl::MakeRefCounted<DwarfUnitImpl>(this, llvm_unit);
    unit_map_[llvm_unit] = unit;
    return unit;
  }
  return found->second;
}

std::optional<uint64_t> DwarfBinaryImpl::GetDebugAddrEntry(uint64_t addr_base,
                                                           uint64_t index) const {
  const llvm::DWARFObject& object = context_->getDWARFObj();
  llvm::StringRef string_ref = object.getAddrSection().Data;

  // From the DWARF 5 spec: "The DW_AT_addr_base attribute points to the first entry following the
  // header. The entries are indexed sequentially from this base entry, starting from 0. So the
  // addr_base is a byte offset, but the index is an index into the address table from there.
  //
  // Here we assume the addresses are always 64 bits. The address table header that precedes the
  // array has this size as a field which we need to consult if we support non-64 bit platforms.
  uint64_t offset = addr_base + (index * kTargetPointerSize);

  if (offset > std::numeric_limits<uint64_t>::max() - kTargetPointerSize ||
      string_ref.size() < offset + kTargetPointerSize)
    return std::nullopt;  // No room in table data.

  uint64_t result;
  memcpy(&result, string_ref.bytes_begin() + offset, kTargetPointerSize);
  return result;
}

llvm::DWARFDie DwarfBinaryImpl::GetLLVMDieAtOffset(DwarfDieRef die_ref) const {
  if (!die_ref.is_valid() || !context_)
    return llvm::DWARFDie();

  if (llvm::DWARFUnit* unit = GetUnitForOffset(context_->getNormalUnitsVector(), die_ref))
    return unit->getDIEForOffset(die_ref.offset());
  if (llvm::DWARFUnit* unit = GetUnitForOffset(context_->getDWOUnitsVector(), die_ref))
    return unit->getDIEForOffset(die_ref.offset());
  return llvm::DWARFDie();
}

void DwarfBinaryImpl::EnsureSignatureMap() const {
  if (signature_to_die_.has_value() || !context_)
    return;

  auto& map = signature_to_die_.emplace();
  auto add_units = [&map](const llvm::DWARFUnitVector& units) {
    for (const std::unique_ptr<llvm::DWARFUnit>& unit : units) {
      if (!unit->isTypeUnit())
        continue;
      auto* type_unit = static_cast<llvm::DWARFTypeUnit*>(unit.get());
      uint64_t die_offset = unit->getOffset() + type_unit->getTypeOffset();
      map[type_unit->getTypeHash()] = DwarfDieRef::ForTypeUnit(unit->getVersion(), die_offset);
    }
  };

  add_units(context_->getNormalUnitsVector());
  add_units(context_->getDWOUnitsVector());
}

DwarfDieRef DwarfBinaryImpl::GetDieRefForSignature(uint64_t signature) const {
  EnsureSignatureMap();
  auto found = signature_to_die_->find(signature);
  if (found == signature_to_die_->end())
    return DwarfDieRef();
  return found->second;
}

void DwarfBinaryImpl::ClearLLVMCache() {
  signature_to_die_.reset();
  for (size_t i = 0; i < GetNormalUnitCount(); i++) {
    auto llvm_unit = GetUnitAtIndex(UnitIndex(false, i))->GetLLVMUnit();
    context_->clearLineTableForUnit(llvm_unit);
    llvm_unit->clear();
  }

  for (size_t i = 0; i < GetDWOUnitCount(); i++) {
    auto llvm_unit = GetUnitAtIndex(UnitIndex(true, i))->GetLLVMUnit();
    context_->clearLineTableForUnit(llvm_unit);
    llvm_unit->clear();
  }
}

}  // namespace zxdb
