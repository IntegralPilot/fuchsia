// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/symbols/module_symbols_impl.h"

#include <stdio.h>

#include <algorithm>
#include <memory>
#include <set>

#include "llvm/DebugInfo/DIContext.h"
#include "llvm/Object/Binary.h"
#include "llvm/Object/ObjectFile.h"
#include "src/developer/debug/shared/address_range.h"
#include "src/developer/debug/shared/largest_less_or_equal.h"
#include "src/developer/debug/shared/logging/logging.h"
#include "src/developer/debug/shared/message_loop.h"
#include "src/developer/debug/zxdb/common/file_util.h"
#include "src/developer/debug/zxdb/symbols/compile_unit.h"
#include "src/developer/debug/zxdb/symbols/dwarf_binary_impl.h"
#include "src/developer/debug/zxdb/symbols/dwarf_die_ref.h"
#include "src/developer/debug/zxdb/symbols/dwarf_expr_eval.h"
#include "src/developer/debug/zxdb/symbols/dwarf_symbol_factory.h"
#include "src/developer/debug/zxdb/symbols/dwo_info.h"
#include "src/developer/debug/zxdb/symbols/elf_symbol.h"
#include "src/developer/debug/zxdb/symbols/file_line.h"
#include "src/developer/debug/zxdb/symbols/find_line.h"
#include "src/developer/debug/zxdb/symbols/function.h"
#include "src/developer/debug/zxdb/symbols/input_location.h"
#include "src/developer/debug/zxdb/symbols/line_details.h"
#include "src/developer/debug/zxdb/symbols/line_table.h"
#include "src/developer/debug/zxdb/symbols/location.h"
#include "src/developer/debug/zxdb/symbols/module_indexer.h"
#include "src/developer/debug/zxdb/symbols/resolve_options.h"
#include "src/developer/debug/zxdb/symbols/symbol_context.h"
#include "src/developer/debug/zxdb/symbols/symbol_data_provider.h"
#include "src/developer/debug/zxdb/symbols/unit_symbol_factory.h"
#include "src/developer/debug/zxdb/symbols/variable.h"
#include "src/lib/elflib/elflib.h"

namespace zxdb {

namespace {

// When looking for symbol matches, don't consider any symbol further than this from the looked-up
// location to be a match. We don't want this number to be too large as users can be confused by
// seeing a name for a symbol that's unrelated to the address at hand.
//
// However, when dealing with unsymbolized code, the nearest previous Elf symbol name can be a
// valuable hint about the location of an address.
constexpr uint64_t kMaxElfOffsetForMatch = 4096;

// Implementation of SymbolDataProvider that returns no memory or registers. This is used when
// evaluating global variables' location expressions which normally just declare an address. See
// LocationForVariable().
class GlobalSymbolDataProvider : public SymbolDataProvider {
 public:
  static Err GetContextError() {
    return Err(
        "Global variable requires register or memory data to locate. "
        "Please file a bug with a repro.");
  }

  // SymbolDataProvider implementation.
  debug::Arch GetArch() override { return debug::Arch::kUnknown; }
  void GetRegisterAsync(debug::RegisterID, GetRegisterCallback callback) override {
    debug::MessageLoop::Current()->PostTask(
        FROM_HERE, [cb = std::move(callback)]() mutable { cb(GetContextError(), {}); });
  }
  void GetFrameBaseAsync(GetFrameBaseCallback callback) override {
    debug::MessageLoop::Current()->PostTask(
        FROM_HERE, [cb = std::move(callback)]() mutable { cb(GetContextError(), 0); });
  }
  void GetMemoryAsync(uint64_t address, uint32_t size, GetMemoryCallback callback) override {
    debug::MessageLoop::Current()->PostTask(FROM_HERE, [cb = std::move(callback)]() mutable {
      cb(GetContextError(), std::vector<uint8_t>());
    });
  }
  void WriteMemory(uint64_t address, std::vector<uint8_t> data, WriteCallback cb) override {
    debug::MessageLoop::Current()->PostTask(
        FROM_HERE, [cb = std::move(cb)]() mutable { cb(GetContextError()); });
  }
};

// The order of the parameters matters because "line 0" is handled in "greedy" mode only for the
// candidate line. If the caller is asking about an address that matches line 0, we don't want to
// expand that past line boundaries, but we do want to expand other lines actoss line 0 in greedy
// mode.
bool SameFileLine(const llvm::DWARFDebugLine::Row& reference,
                  const llvm::DWARFDebugLine::Row& candidate, bool greedy) {
  // EndSequence entries don't have files or lines associated with them, it's just a marker. The
  // table will report the previous line's file+line as a side-effect fo the way it's encoded, so
  // explicitly fail matching for these.
  if (reference.EndSequence || candidate.EndSequence)
    return false;

  if (greedy && candidate.Line == 0)
    return true;
  return reference.File == candidate.File && reference.Line == candidate.Line;
}

// Determines if the given input location references a special identifier of the given type. If it
// does, returns the name of that symbol. If it does not, returns a null optional.
std::optional<std::string> GetSpecialInputLocation(const InputLocation& loc, SpecialIdentifier si) {
  if (loc.type != InputLocation::Type::kName || loc.name.components().size() != 1)
    return std::nullopt;

  if (loc.name.components()[0].special() == si)
    return loc.name.components()[0].name();

  return std::nullopt;
}

// Returns true if the given input references the special "main" function annotation.
bool ReferencesMainFunction(const InputLocation& loc) {
  if (loc.type != InputLocation::Type::kName || loc.name.components().size() != 1)
    return false;
  return loc.name.components()[0].special() == SpecialIdentifier::kMain;
}

// Returns true if any component of this identifier isn't supported via lookup in the
// ModuleSymbolsImpl.
bool HasOnlySupportedSpecialIdentifierTypes(const Identifier& ident) {
  for (const auto& comp : ident.components()) {
    switch (comp.special()) {
      case SpecialIdentifier::kNone:
      case SpecialIdentifier::kElf:
      case SpecialIdentifier::kPlt:
      case SpecialIdentifier::kAnon:
        break;  // Normal boring component.
      case SpecialIdentifier::kEscaped:
        FX_NOTREACHED();  // "Escaped" annotations shouldn't appear in identifiers.
        break;
      case SpecialIdentifier::kMain:
        // "$main" is supported only when it's the only component ("foo::$main" is invalid).
        return ident.components().size() == 1;
      case SpecialIdentifier::kRegister:
      case SpecialIdentifier::kZxdb:
        return false;  // Can't look up registers in the symbols.
      case SpecialIdentifier::kLast:
        FX_NOTREACHED();  // Not supposed to be a valid value.
        return false;
    }
  }
  return true;
}

}  // namespace

ModuleSymbolsImpl::ModuleSymbolsImpl(std::unique_ptr<DwarfBinaryImpl> binary,
                                     const std::string& build_dir)
    : binary_(std::move(binary)),
      build_dir_(build_dir),
      index_(std::make_unique<Index>()),  // Placeholder empty index so this is never null.
      weak_factory_(this),
      symbol_weak_factory_(this) {}

ModuleSymbolsImpl::~ModuleSymbolsImpl() = default;

fxl::WeakPtr<ModuleSymbolsImpl> ModuleSymbolsImpl::GetWeakPtr() {
  return weak_factory_.GetWeakPtr();
}

Err ModuleSymbolsImpl::Load(bool create_index) {
  if (Err err = binary_->Load(symbol_weak_factory_.GetWeakPtr(), DwarfSymbolFactory::kMainBinary);
      err.has_error()) {
    return err;
  }

  FillElfSymbols();

  if (create_index) {
    CreateIndex();
  }
  return Err();
}

ModuleSymbolStatus ModuleSymbolsImpl::GetStatus() const {
  ModuleSymbolStatus status;
  status.build_id = binary_->GetBuildID();
  status.base = 0;               // We don't know this, only ProcessSymbols does.
  status.symbols_loaded = true;  // Since this instance exists at all.
  status.functions_indexed = index_->CountSymbolsIndexed();
  status.files_indexed = index_->files_indexed();
  status.symbol_file = binary_->GetName();
  return status;
}

std::time_t ModuleSymbolsImpl::GetModificationTime() const {
  return binary_->GetModificationTime();
}

std::string ModuleSymbolsImpl::GetBuildDir() const { return build_dir_; }

uint64_t ModuleSymbolsImpl::GetMappedLength() const { return binary_->GetMappedLength(); }

const SymbolFactory* ModuleSymbolsImpl::GetSymbolFactory() const {
  return binary_->GetSymbolFactory();
}

std::vector<Location> ModuleSymbolsImpl::ResolveInputLocation(const SymbolContext& symbol_context,
                                                              const InputLocation& input_location,
                                                              const ResolveOptions& options) const {
  // Thie skip_function_prologue option requires that symbolize be set.
  FX_DCHECK(!options.skip_function_prologue || options.symbolize);

  switch (input_location.type) {
    case InputLocation::Type::kNone:
      return std::vector<Location>();
    case InputLocation::Type::kLine:
      return ResolveLineInputLocation(symbol_context, input_location, options);
    case InputLocation::Type::kName:
      return ResolveSymbolInputLocation(symbol_context, input_location, options);
    case InputLocation::Type::kAddress:
      return ResolveAddressInputLocation(symbol_context, input_location, options);
  }
}

ModuleSymbols::FoundUnit ModuleSymbolsImpl::GetDwarfUnitForAddress(
    const SymbolContext& symbol_context, uint64_t absolute_address) const {
  FoundUnit result;

  result.main_binary_unit =
      binary_->UnitForRelativeAddress(symbol_context.AbsoluteToRelative(absolute_address));

  // Default to assuming the units are the same (simplifying the early returns later). This will be
  // overwritten if a different details unit is found.
  result.details_unit = result.main_binary_unit;

  // The dwos_.empty() check is an optimization to avoid checking the skeleton units when we know
  // there are none.
  if (!result.main_binary_unit || dwos_.empty()) {
    // The result should either be both null pointers, or both the valid non-DWO unit.
    return result;
  }

  // In the case of debug fission, we want to return the full unit in the .dwo file. This needs to
  // be looked up if the unit that was matched was a skeleton.
  llvm::DWARFDie llvm_die = result.main_binary_unit->GetUnitDie();
  if (static_cast<int>(llvm_die.getTag()) != static_cast<int>(DwarfTag::kSkeletonUnit)) {
    // Not a skeleton unit.
    return result;
  }

  // Look up the DWO for this skeleton.
  uint64_t skeleton_offset = llvm_die.getOffset();
  auto found_skeleton_index = dwo_skeleton_offset_to_index_.find(skeleton_offset);
  if (found_skeleton_index == dwo_skeleton_offset_to_index_.end()) {
    // Some invalid DWO reference, give up and return the skeleton one.
    return result;
  }

  FX_CHECK(found_skeleton_index->second < dwos_.size());
  if (!dwos_[found_skeleton_index->second]) {
    // Invalid DWO reference.
    return result;
  }
  result.details_unit = dwos_[found_skeleton_index->second]->binary()->GetDwoUnit();
  return result;
}

LineDetails ModuleSymbolsImpl::LineDetailsForAddress(const SymbolContext& symbol_context,
                                                     uint64_t absolute_address, bool greedy) const {
  FoundUnit found_unit = GetDwarfUnitForAddress(symbol_context, absolute_address);
  if (!found_unit)
    return LineDetails();

  // TODO(brettw) this should use our LineTable wrapper instead of LLVM's so it can be mocked.
  const llvm::DWARFDebugLine::LineTable* line_table =
      found_unit.main_binary_unit->GetLLVMLineTable();
  if (!line_table || line_table->Rows.empty())
    return LineDetails();

  uint64_t relative_address = symbol_context.AbsoluteToRelative(absolute_address);
  const auto& rows = line_table->Rows;
  uint32_t found_row_index = line_table->lookupAddress({relative_address});

  // The row could be not found or it could be in a "nop" range indicated by an "end sequence"
  // marker. For padding between functions, the compiler will insert a row with this marker to
  // indicate everything until the next address isn't an instruction. With this flag, the other
  // information on the line will be irrelevant (in practice it will be the same as for the previous
  // entry).
  if (found_row_index == line_table->UnknownRowIndex || rows[found_row_index].EndSequence)
    return LineDetails();

  // Adjust the beginning and end ranges to include all matching entries of the same line.
  //
  // Note that this code must not try to hide "line 0" entries (corresponding to compiler-generated
  // code). This function is used by the stepping code which has its own handling for these ranges.
  // Trying to put "line 0" code in with the previous or next entry (what some other code does that
  // tries to hide this from the user) will confuse the stepping code which will always step through
  // these instructions.
  uint32_t first_row_index = found_row_index;
  while (first_row_index > 0 &&
         SameFileLine(rows[found_row_index], rows[first_row_index - 1], greedy)) {
    first_row_index--;
  }
  uint32_t last_row_index = found_row_index;  // Inclusive.
  while (last_row_index < rows.size() - 1 &&
         SameFileLine(rows[found_row_index], rows[last_row_index + 1], greedy)) {
    last_row_index++;
  }

  // Resolve the file name. Skip for "line 0" entries which are compiled-generated code not
  // associated with a line entry, leaving the file name and compilation directory empty. Typically
  // there will be a file if we ask, but that's leftover from the previous row in the table by the
  // state machine and is not relevant.
  std::string file_name;
  std::string compilation_dir;
  if (rows[first_row_index].Line) {
    line_table->getFileNameByIndex(rows[first_row_index].File, "",
                                   llvm::DILineInfoSpecifier::FileLineInfoKind::RelativeFilePath,
                                   file_name);
    if (!build_dir_.empty()) {
      compilation_dir = build_dir_;
    } else {
      compilation_dir = found_unit.details_unit->GetCompilationDir();
    }
  }

  LineDetails result(
      FileLine(NormalizePath(file_name), compilation_dir, rows[first_row_index].Line));

  // Add entries for each row. The last row doesn't count because it should be
  // an end_sequence marker to provide the ending size of the previous entry.
  // So never include that.
  for (uint32_t i = first_row_index; i <= last_row_index && i < rows.size() - 1; i++) {
    // With loop bounds we can always dereference @ i + 1.
    if (rows[i + 1].Address < rows[i].Address)
      break;  // Going backwards, corrupted so give up.

    LineDetails::LineEntry entry;
    entry.column = rows[i].Column;
    entry.range = AddressRange(symbol_context.RelativeToAbsolute(rows[i].Address.Address),
                               symbol_context.RelativeToAbsolute(rows[i + 1].Address.Address));
    result.entries().push_back(entry);
  }

  return result;
}

std::vector<std::string> ModuleSymbolsImpl::FindFileMatches(std::string_view name) const {
  // If both build_dir_ and name are absolute, convert it to a relative path against build_dir_
  // because the paths in the index are relative.
  if (!build_dir_.empty() && build_dir_[0] == '/' && name[0] == '/') {
    return index_->FindFileMatches(PathRelativeTo(name, build_dir_).string());
  }
  return index_->FindFileMatches(name);
}

std::vector<fxl::RefPtr<Function>> ModuleSymbolsImpl::GetMainFunctions() const {
  std::vector<fxl::RefPtr<Function>> result;
  for (const auto& ref : index_->main_functions()) {
    auto symbol_ref = IndexSymbolRefToSymbol(ref);
    const Function* func = symbol_ref.Get()->As<Function>();
    if (func)
      result.emplace_back(RefPtrTo(func));
  }
  return result;
}

const Index& ModuleSymbolsImpl::GetIndex() const { return *index_; }

LazySymbol ModuleSymbolsImpl::IndexSymbolRefToSymbol(const IndexNode::SymbolRef& ref) const {
  // TODO(bug 53091) in the future we may want to add ELF symbol support here.
  switch (ref.kind()) {
    case IndexNode::SymbolRef::kNull:
      break;
    case IndexNode::SymbolRef::kDwarf:
    case IndexNode::SymbolRef::kDwarfDeclaration:
      // Handled by the DWARF symbol factory.
      if (ref.dwo_index() == IndexNode::SymbolRef::kMainBinary) {
        return GetSymbolFactory()->MakeLazy(ref.die_ref());
      } else {
        // The dwo_index is set: it is a reference into our dwos_ array.
        FX_CHECK(ref.dwo_index() >= 0 && ref.dwo_index() < static_cast<int32_t>(dwos_.size()));

        // The pointers in the dwos_ array may be null. But if the .dwo file could not be loaded, we
        // shouldn't have any index references into it. So all input .dwo indices should be valid
        // pointers.
        FX_CHECK(dwos_[ref.dwo_index()]);
        return dwos_[ref.dwo_index()]->symbol_factory()->MakeLazy(ref.die_ref());
      }
  }
  return LazySymbol();
}

bool ModuleSymbolsImpl::HasBinary() const { return binary_->HasBinary(); }

std::optional<uint64_t> ModuleSymbolsImpl::GetDebugAddrEntry(uint64_t addr_base,
                                                             uint64_t index) const {
  return binary_->GetDebugAddrEntry(addr_base, index);
}

void ModuleSymbolsImpl::AppendLocationForFunction(const SymbolContext& symbol_context,
                                                  const ResolveOptions& options,
                                                  const Function* func,
                                                  std::vector<Location>* result) const {
  if (func->code_ranges().empty())
    return;  // No code associated with this.

  // Compute the full file/line information if requested. This recomputes function DIE which is
  // unnecessary but makes the code structure simpler and ensures the results are always the same
  // with regard to how things like inlined functions are handled (if the location maps to both a
  // function and an inlined function inside of it).
  uint64_t abs_addr = symbol_context.RelativeToAbsolute(func->code_ranges()[0].begin());
  if (options.symbolize)
    result->push_back(LocationForAddress(symbol_context, abs_addr, options, func));
  else
    result->emplace_back(Location::State::kAddress, abs_addr);
}

std::vector<Location> ModuleSymbolsImpl::ResolveLineInputLocation(
    const SymbolContext& symbol_context, const InputLocation& input_location,
    const ResolveOptions& options) const {
  std::vector<Location> result;
  for (const std::string& file : FindFileMatches(input_location.line.file())) {
    ResolveLineInputLocationForFile(symbol_context, file, input_location.line.line(), options,
                                    &result);
  }
  return result;
}

std::vector<Location> ModuleSymbolsImpl::ResolveSymbolInputLocation(
    const SymbolContext& symbol_context, const InputLocation& input_location,
    const ResolveOptions& options) const {
  FX_DCHECK(input_location.type == InputLocation::Type::kName);
  if (!HasOnlySupportedSpecialIdentifierTypes(input_location.name))
    return {};  // Unsupported symbol type.

  // Special-case for ELF/PLT functions.
  //
  // Note that this only checks the ELF index when explicitly requested. This is because for a given
  // function, say "pthread_key_create", it will have a .so with the implementation, and each module
  // that references it will have a PLT thunk (a type of ELF symbol). Matching all the ELF symbols
  // is not what the user wants when they ask for information or a breakpoint on this function (the
  // breakpoint will end up meaning 2 breaks per call).
  //
  // At the same time, it would be nice to use ELF symbols when debugging non-symbolized binaries.
  // We can't just ask if the index is empty to detech this case since "unsymbolized" binaries can
  // sometimes have a few trivial DWARF symbols.
  //
  // Just falling back to ELF symbols here when the main lookup matches nothing doesn't work because
  // the calling modules in the pthread example above will have only the PLT match. To make it work,
  // every caller of this function that combines results from more than one module (including
  // FindName and ProcessSymbols) needs to have some filtering or prioritizing and how this should
  // work is non-obvious.
  if (auto plt_name = GetSpecialInputLocation(input_location, SpecialIdentifier::kPlt))
    return ResolvePltName(symbol_context, *plt_name);
  if (auto elf_name = GetSpecialInputLocation(input_location, SpecialIdentifier::kElf))
    return ResolveElfName(symbol_context, *elf_name);

  std::vector<Location> result;

  auto symbol_to_find = input_location.name;

  // Special-case for main functions.
  if (ReferencesMainFunction(input_location)) {
    auto main_functions = GetMainFunctions();
    if (!main_functions.empty()) {
      for (const auto& func : GetMainFunctions())
        AppendLocationForFunction(symbol_context, options, func.get(), &result);
      return result;
    } else {
      // Nothing explicitly marked as the main function, fall back on anything in the toplevel
      // namespace named "main".
      symbol_to_find = Identifier(IdentifierQualification::kGlobal, IdentifierComponent("main"));

      // Fall through to symbol finding on the new name.
    }
  }

  // TODO(bug 37654) it would be nice if this could be deleted and all code go through
  // expr/find_name.h to query the index. As-is this duplicates some of FindName's logic in a less
  // flexible way.
  for (const auto& ref : index_->FindExact(symbol_to_find)) {
    LazySymbol lazy_symbol = IndexSymbolRefToSymbol(ref);
    const Symbol* symbol = lazy_symbol.Get();

    if (const Function* function = symbol->As<Function>()) {
      // Symbol is a function.
      AppendLocationForFunction(symbol_context, options, function, &result);
    } else if (const Variable* variable = symbol->As<Variable>()) {
      // Symbol is a variable. This will be the case for global variables and file- and class-level
      // statics. This always symbolizes since we already computed the symbol.
      result.push_back(LocationForVariable(symbol_context, RefPtrTo(variable)));
    } else {
      // Unknown type of symbol.
      continue;
    }
  }

  return result;
}

std::vector<Location> ModuleSymbolsImpl::ResolveAddressInputLocation(
    const SymbolContext& symbol_context, const InputLocation& input_location,
    const ResolveOptions& options) const {
  std::vector<Location> result;
  if (options.symbolize) {
    DEBUG_LOG(ModuleSymbols) << "Resolving input location for 0x" << std::hex
                             << input_location.address;
    result.push_back(LocationForAddress(symbol_context, input_location.address, options, nullptr));
  } else {
    result.emplace_back(Location::State::kAddress, input_location.address);
  }
  return result;
}

Location ModuleSymbolsImpl::LocationForAddress(const SymbolContext& symbol_context,
                                               uint64_t absolute_address,
                                               const ResolveOptions& options,
                                               const Function* optional_func) const {
  auto dwarf_loc =
      DwarfLocationForAddress(symbol_context, absolute_address, options, optional_func);
  if (dwarf_loc && dwarf_loc->symbol()) {
    DEBUG_LOG(ModuleSymbols) << "Found DWARF location with symbol for 0x" << std::hex
                             << absolute_address;
    return std::move(*dwarf_loc);
  }

  // Fall back to use ELF symbol but keep the file/line info.
  FileLine file_line;
  int column = 0;
  if (dwarf_loc) {
    DEBUG_LOG(ModuleSymbols) << "Found DWARF location without symbol for 0x" << std::hex
                             << absolute_address;
    file_line = dwarf_loc->file_line();
    column = dwarf_loc->column();
  }
  if (auto elf_locs =
          ElfLocationForAddress(symbol_context, absolute_address, options, file_line, column)) {
    DEBUG_LOG(ModuleSymbols) << "Found ELF location for 0x" << std::hex << absolute_address;
    return std::move(*elf_locs);
  }

  // Not symbolizable, return an "address" with no symbol information. Mark it symbolized to record
  // that we tried and failed.
  DEBUG_LOG(ModuleSymbols) << "Couldn't get location for 0x" << std::hex << absolute_address;
  return Location(Location::State::kSymbolized, absolute_address);
}

// This function is similar to llvm::DWARFContext::getLineInfoForAddress.
std::optional<Location> ModuleSymbolsImpl::DwarfLocationForAddress(
    const SymbolContext& symbol_context, uint64_t absolute_address, const ResolveOptions& options,
    const Function* optional_func) const {
  // TODO(bug 5544) handle addresses that aren't code like global variables.
  FoundUnit found_unit = GetDwarfUnitForAddress(symbol_context, absolute_address);
  if (!found_unit) {
    DEBUG_LOG(ModuleSymbols) << "Don't have DWARF unit for 0x" << std::hex << absolute_address;
    return std::nullopt;
  }

  uint64_t relative_address = symbol_context.AbsoluteToRelative(absolute_address);
  FileLine file_line;
  int column = 0;

  // Get the innermost subroutine or inlined function for the address. This may be empty, but still
  // lookup the line info below in case its present. This computes both a LazySymbol which we
  // pass to the result, and a possibly-null containing Function* (not an inlined subroutine) to do
  // later computations on.
  fxl::RefPtr<Function> function;  // For prologue computations.
  LazySymbol lazy_function;
  if (optional_func) {
    // The function was passed in and we want to return that exact one. This will happen if the
    // caller has asked for the location of a named function.
    function = RefPtrTo(optional_func);
    lazy_function = LazySymbol(optional_func);
  } else {
    // Resolve the function for this address.
    if ((lazy_function = found_unit.details_unit->FunctionForRelativeAddress(relative_address))) {
      DEBUG_LOG(ModuleSymbols) << "Got function from DWARF unit for 0x" << std::hex
                               << absolute_address;
      // FunctionForRelativeAddress() will return the most specific inlined function for the
      // address.
      function = RefPtrTo(lazy_function.Get()->As<Function>());

      // The is_inline() check is strictly unnecessary since ambiguous inline computations will
      // work either way. This check allows us to skip the ambiguous inline computations in the
      // common case that we're not in an inline.
      if (function && function->is_inline() &&
          options.ambiguous_inline == ResolveOptions::AmbiguousInline::kOuter) {
        // Adjust the function to be the outermost frame (should be the non-inlined function)
        // for ambiguous locations (at the beginning of one or more inlined functions).
        std::vector<fxl::RefPtr<Function>> inline_chain =
            function->GetAmbiguousInlineChain(symbol_context, absolute_address);
        if (inline_chain.size() > 1) {
          lazy_function = inline_chain.back();

          // Since we picked a non-topmost inline subroutine, we know the file/line because
          // it's the call location of the inline subroutine we skipped. DWARF doesn't encode
          // column information for this type of call.
          const auto& calling_func = inline_chain[inline_chain.size() - 2];
          file_line = calling_func->call_line();
        }
      }
    } else {
      DEBUG_LOG(ModuleSymbols) << "No function info in DWARF unit for 0x" << std::hex
                               << absolute_address;
    }
  }

  // Get the file/line location (may fail). Don't overwrite one computed above if already set above
  // using the ambigous inline call site.
  if (!file_line.is_valid()) {
    DEBUG_LOG(ModuleSymbols) << "Referencing line table for 0x" << std::hex << absolute_address;
    const LineTable& line_table = found_unit.main_binary_unit->GetLineTable();

    // Use the line table to move the address to after the function prologue. Assume if the function
    // is inline there's no prologue. Inlines themselves will have no prologues, and we assume
    // inlines won't appear in the prologue of other functions.
    if (function && !function->is_inline() && options.skip_function_prologue) {
      if (size_t prologue_size = GetFunctionPrologueSize(line_table, function.get())) {
        // The function has a prologue. When it does, we know it has code ranges so don't need to
        // validate it's nonempty before using.
        uint64_t function_begin = function->code_ranges().front().begin();
        if (relative_address >= function_begin &&
            relative_address < function_begin + prologue_size) {
          // Adjust address to the first real instruction.
          relative_address = function_begin + prologue_size;
          absolute_address = symbol_context.RelativeToAbsolute(relative_address);
        }
      }
    }

    // Look up the line info for this address.
    //
    // This re-computes some of what GetFunctionPrologueSize() may have done above. This could be
    // enhanced in the future by having LineTable::GetRowForAddress that include the prologue
    // adjustment as part of one computation.
    LineTable::FoundRow found_row = line_table.GetRowForAddress(symbol_context, absolute_address);
    if (!found_row.empty()) {
      // Line info present. Only set the file name if there's a nonzero line number. "Line 0"
      // entries which are compiled-generated code not associated with a line entry. Typically there
      // will be a file if we ask, but that's leftover from the previous row in the table by the
      // state machine and is not relevant.
      const LineTable::Row* maybe_row;
      if (options.suitable_for_breakpoint) {
        maybe_row = &found_row.get_statement();
      } else {
        maybe_row = &found_row.get_symbolizable();
      }

      if (maybe_row) {
        const LineTable::Row& row = *maybe_row;
        std::optional<std::string> file_name;
        if (row.Line) {
          file_name = line_table.GetFileNameForRow(row);  // Could still return nullopt.
        } else {
          DEBUG_LOG(ModuleSymbols) << "Line table's row did not have a line number for 0x"
                                   << std::hex << absolute_address;
        }
        if (file_name) {
          // It's important this only gets called when row.Line > 0. FileLine will assert for line 0
          // if a file name or build directory is given to ensure that all "no code" locations
          // compare identically. This is guaranteed here because file_name will only be set when
          // the line is nonzero.
          if (build_dir_.empty()) {
            file_line = FileLine(std::move(*file_name),
                                 found_unit.details_unit->GetCompilationDir(), row.Line);
          } else {
            file_line = FileLine(std::move(*file_name), build_dir_, row.Line);
          }
        }
        column = row.Column;
      } else {
        DEBUG_LOG(ModuleSymbols) << "Line table did not have entry for 0x" << std::hex
                                 << absolute_address;
      }
    } else {
      DEBUG_LOG(ModuleSymbols) << "Line table did not have row for 0x" << std::hex
                               << absolute_address;
    }
  }

  if (!lazy_function && !file_line.is_valid()) {
    DEBUG_LOG(ModuleSymbols) << "File/line info is not valid for eager function 0x" << std::hex
                             << absolute_address;
    return std::nullopt;
  }

  return Location(absolute_address, file_line, column, symbol_context, std::move(lazy_function));
}

std::optional<Location> ModuleSymbolsImpl::ElfLocationForAddress(
    const SymbolContext& symbol_context, uint64_t absolute_address, const ResolveOptions& options,
    FileLine file_line, int column) const {
  if (elf_addresses_.empty()) {
    DEBUG_LOG(ModuleSymbols) << "No ELF addresses available to symbolize 0x" << std::hex
                             << absolute_address;
    return std::nullopt;
  }

  uint64_t relative_addr = symbol_context.AbsoluteToRelative(absolute_address);
  auto found = debug::LargestLessOrEqual(
      elf_addresses_.begin(), elf_addresses_.end(), relative_addr,
      [](const ElfSymbolRecord* r, uint64_t addr) { return r->relative_address < addr; },
      [](const ElfSymbolRecord* r, uint64_t addr) { return r->relative_address == addr; });
  if (found == elf_addresses_.end()) {
    DEBUG_LOG(ModuleSymbols) << "ELF addresses don't contain relative address for 0x" << std::hex
                             << absolute_address;
    return std::nullopt;
  }

  // There could theoretically be multiple matches for this address, but we return only the first.
  const ElfSymbolRecord* record = *found;
  if (relative_addr - record->relative_address > kMaxElfOffsetForMatch) {
    DEBUG_LOG(ModuleSymbols) << "Found ELF record is too far away from 0x" << std::hex
                             << absolute_address;
    return std::nullopt;  // Too far away.
  }
  return Location(
      absolute_address, std::move(file_line), column, symbol_context,
      fxl::MakeRefCounted<ElfSymbol>(const_cast<ModuleSymbolsImpl*>(this)->GetWeakPtr(), *record));
}

Location ModuleSymbolsImpl::LocationForVariable(const SymbolContext& symbol_context,
                                                fxl::RefPtr<Variable> variable) const {
  // Evaluate the DWARF expression for the variable. Global and static variables' locations aren't
  // based on CPU state. In some cases like TLS the location may require CPU state or may result in
  // a constant instead of an address. In these cases give up and return an "unlocated variable."
  // These can easily be evaluated by the expression system so we can still print their values.

  // Need one unique location.
  if (variable->location().locations().size() != 1)
    return Location(symbol_context, std::move(variable));

  auto global_data_provider = fxl::MakeRefCounted<GlobalSymbolDataProvider>();
  DwarfExprEval eval(UnitSymbolFactory(variable.get()), global_data_provider, symbol_context);
  eval.Eval(variable->location().locations()[0].expression,
            [](DwarfExprEval* eval, const Err& err) {});

  // Only evaluate synchronous outputs that result in a pointer.
  if (!eval.is_complete() || !eval.is_success() ||
      eval.GetResultType() != DwarfExprEval::ResultType::kPointer)
    return Location(symbol_context, std::move(variable));

  DwarfStackEntry result = eval.GetResult();
  if (!result.TreatAsUnsigned())
    return Location(symbol_context, std::move(variable));

  // TODO(brettw) in all of the return cases we could in the future fill in the file/line of the
  // definition of the variable. Currently Variables don't provide that (even though it's usually in
  // the DWARF symbols).
  return Location(result.unsigned_value(), FileLine(), 0, symbol_context, std::move(variable));
}

std::vector<Location> ModuleSymbolsImpl::ResolvePltName(const SymbolContext& symbol_context,
                                                        const std::string& mangled_name) const {
  // There can theoretically be multiple symbols with the given name, some might be PLT symbols,
  // some might not be. Check all name matches for a PLT one.
  auto cur = mangled_elf_symbols_.lower_bound(mangled_name);
  while (cur != mangled_elf_symbols_.end() && cur->first == mangled_name) {
    if (cur->second.type == ElfSymbolType::kPlt)
      return {MakeElfSymbolLocation(symbol_context, std::nullopt, cur->second)};
    ++cur;
  }

  // No PLT locations found for this name.
  return {};
}

std::vector<Location> ModuleSymbolsImpl::ResolveElfName(const SymbolContext& symbol_context,
                                                        const std::string& mangled_name) const {
  std::vector<Location> result;

  // There can theoretically be multiple symbols with the given name.
  auto cur = mangled_elf_symbols_.lower_bound(mangled_name);
  while (cur != mangled_elf_symbols_.end() && cur->first == mangled_name) {
    result.push_back(MakeElfSymbolLocation(symbol_context, std::nullopt, cur->second));
    ++cur;
  }

  return result;
}

// To a first approximation we just look up the line in the line table for each compilation unit
// that references the file. Complications:
//
// 1. The line might not be an exact match (the user can specify a blank line or something optimized
//    out). In this case, find the next valid line.
//
// 2. Clang doesn't emit line table entries for the source of an inlined function call.
//    See https://bugs.fuchsia.dev/p/fuchsia/issues/detail?id=112203 and
//    https://github.com/llvm/llvm-project/issues/58483.
//
//    To work around this, for every function with a line table match from complication 1, we search
//    for inlined function calls whose call location matches the input location. (Note: neither GDB
//    nor LLDB do this as of this writing, this is more of a workaround for a Clang bug.)
//
// 3. Conversely, rustc emits too many line table entries for await points. This causes the "best"
//    match (see below) to not actually be the lowest address for this particular line. To solve
//    this, we cross reference the match with the LineDetails for each match's address to filter the
//    matches to the minimal set of AddressRanges. From there, we query each match's address again
//    for the most specific CodeBlock at that address. The match with the lowest address for each
//    unique CodeBlock are the locations that we should return.
//
//    Rust await points are usually broken into a few discontiguous ranges where the generated code
//    wraps the actual function call, giving us clear segments to set the breakpoint. Due to
//    https://github.com/rust-lang/rust/issues/123341, the first segment is typically not where you
//    want the breakpoint to be. We also want to avoid setting breakpoint locations on all of these
//    discontiguous ranges, because the breakpoint would be hit multiple times, so we need to also
//    compare the most specific CodeBlock associated with each of these discontiguous ranges for
//    uniqueness. Rust await points generate a CodeBlock for the generated code to handle the state
//    changes of the returned future which is distinct from the CodeBlock of the containing
//    function, and is conveniently the first code that executes as part of the async function call
//    on the line we're interested in.
//
//    GDB and LLDB handle this case by doing one of the above steps - GDB compares code blocks and
//    LLDB collapses contiguous regions - and then install breakpoints in all of the resulting
//    locations. This leads to LLDB installing a breakpoint before the synchronous part of the
//    generated code (which is what the user primarily will want) and then also on the eventual
//    dropping of the future that happens when the asynchronous operation is completed, which is
//    confusing when the breakpoint hits twice. GDB installs a single additional breakpoint before
//    the user's function is called, which is also what we do.
//
//    Synchronous inlined functions can also result in a set of discontiguous address ranges, but
//    unlike Rust's await points, these will typically all refer to the same CodeBlock, namely the
//    function being inlined. This results in the expected behavior of resolving a single location
//    before any inline call chains begin.
//
// 4. The above step can find many different locations. Maybe some code from the file in question is
//    inlined into the compilation unit, but not the function with the line in it. Or different
//    template instantiations can mean that a line of code is in some instantiations but don't apply
//    to others.
//
//    To solve this duplication problem, get the resolved line of each of the addresses found above
//    and find the best one. Keep only those locations matching the best one (there can still be
//    multiple).
//
// 5. Inlining and templates can mean there can be multiple matches of the exact same line. To
//    handle the case where one line is broken into multiple line table entries, we collapse the
//    range of contiguous addresses for a line table entry to just the one with the smallest
//    address. See complication #3 for the reasoning. This means that multiple addresses may match a
//    single function on the line.
void ModuleSymbolsImpl::ResolveLineInputLocationForFile(const SymbolContext& symbol_context,
                                                        const std::string& canonical_file,
                                                        int line_number,
                                                        const ResolveOptions& options,
                                                        std::vector<Location>* output) const {
  const std::vector<UnitIndex>* units = index_->FindFileUnitIndices(canonical_file);
  if (!units)
    return;

  std::vector<LineMatch> matches;
  for (UnitIndex index : *units) {
    fxl::RefPtr<DwarfUnit> unit = binary_->GetUnitAtIndex(index);
    const LineTable& line_table = unit->GetLineTable();

    // Complication 1 above: find all matches for this line in the unit.
    std::vector<LineMatch> unit_matches =
        GetAllLineTableMatchesInUnit(line_table, canonical_file, line_number);
    matches.insert(matches.end(), unit_matches.begin(), unit_matches.end());
  }

  if (matches.empty())
    return;

  // Complication 2 above: Get all unique functions we found from complication 1, and add in
  // inline call site locations. This can be removed if the above-mentioned Clang bug is fixed.
  //
  // The LineMatch contains the function DIE offset which we use for uniquifying functions, but
  // we currently don't have a convenient way to map this back to a Symbol object. Instead, look up
  // the corresponding function based on address of the previously found match.
  //
  // There are some limitations of this approach. But since neither GDB nor LLDB use inline call
  // site information at all, our implementation is already better than the competition.
  //
  //  - TODO #1: This will work for most cases but isn't quite sufficient. If you have an inlined
  //    function that consists only of a call to another inlined function (maybe in another file),
  //    the calling inlined function will never get identified in the previous loop and we'll never
  //    search its inlined call locations in this loop. Having an inline only call another inline
  //    isn't actually that uncommon in places like the STL.
  //
  //  - TODO #2: For optimized code, sometimes the line you ask for doesn't have any code. To
  //    support this, the line table searching code looks for lines after the one you asked for
  //    also. To prevent matching all later lines in the file, it searches for *transitions* between
  //    code before the line being searched for to code equal to or after the line being searched
  //    for. The inline call site searching code doesn't have enough context to search for these
  //    transitions so only searches for exact matches.
  //
  // The correct approach would be brute-force search all functions in the units identified at the
  // top of this function that reference the file in question. We would check these functions for
  // inlined calls and save their call locations. (As of this writing, our current symbol
  // implementation doesn't make it easy to go through all functions in a unit).
  //
  // Then we would take these inline call locations and merge them into the line table in some way
  // (probably we would make up a completely new line table that's processed in all the ways we need
  // and cache it on the unit). This way, GetAllLineTableMatchesInUnit() can take inline call
  // locations into account when looking for code line transitions. Then if you request a breakpoint
  // for an optimized-out line *before* the inline call, it can "slide" down to the inline call
  // even if there was no line table entry for it.
  std::set<LazySymbol> checked_functions;        // LineMatch.functions we've checked.
  size_t original_match_count = matches.size();  // Don't check anything the loop appends.
  for (size_t i = 0; i < original_match_count; i++) {
    const LineMatch& match = matches[i];

    if (checked_functions.find(match.function) != checked_functions.end())
      continue;  // Already checked this function.

    checked_functions.insert(match.function);

    if (auto fn = match.function.Get()->As<Function>()) {
      // Make sure we have the outermost function and not some random code block inside it.
      //
      // If GetContainingFunction() returns a different function that we put in (i.e. we put in
      // an inline) but we've already checked it, give up. Don't do this if it gives us the
      // same thing back since we've inserted our current function's DIE offset in the map but
      // not checked it yet.
      auto containing_fn = fn->GetContainingFunction(Function::kPhysicalOnly);
      if (fn->GetDieRef() != containing_fn->GetDieRef() &&
          checked_functions.find(fn->GetLazySymbol()) != checked_functions.end()) {
        continue;  // Already checked this toplevel function.
      }

      // Append any new matches.
      AppendLineMatchesForInlineCalls(containing_fn.get(), canonical_file, line_number,
                                      containing_fn.get(), &matches);
    }
  }

  // Complication 3: Map the line details extent back to the match for this line, sorted by the
  // beginning of the range. Using the extent from the LineDetails is the first thing we can use to
  // discard unneeded matches. We only want to keep the match with the lowest address.
  std::map<AddressRange, LineMatch, AddressRangeBeginCmp> range_to_match;
  for (auto& match : matches) {
    auto details = LineDetailsForAddress(symbol_context,
                                         symbol_context.RelativeToAbsolute(match.address), true);

    auto found = range_to_match.find(details.GetExtent());
    if (found == range_to_match.end()) {
      range_to_match[details.GetExtent()] = match;
    } else if (match.address < found->second.address) {
      found->second = match;
    }
  }

  // We will repopulate |matches| with only the matches that refer to unique CodeBlocks. If multiple
  // matches refer to the same CodeBlock, only the match with the lowest address is kept.
  matches.clear();

  // This ensures that the CodeBlock corresponding to the PC for each corresponding match is unique,
  // which is important for finding all the locations that a NON-inlined function could have. This
  // is particularly important when dealing with the generated code around Rust await points.
  auto comparator = [&symbol_context](const CodeBlock* lhs, const CodeBlock* rhs) {
    const auto& lhs_range = lhs->GetFullRange(symbol_context);
    const auto& rhs_range = rhs->GetFullRange(symbol_context);
    return debug::AddressRangeEqualityCmp()(lhs_range, rhs_range);
  };

  // If multiple matches resolve to the same actual range of code, only keep the first one. Since
  // |range_to_matches| is sorted, the first match will always be the lowest address for that
  // CodeBlock and therefore is the best location.
  std::set<const CodeBlock*, decltype(comparator)> seen_code_blocks(comparator);
  for (const auto& [_, match] : range_to_match) {
    auto fn = match.function.Get()->As<Function>();

    auto block =
        fn->GetMostSpecificChild(symbol_context, symbol_context.RelativeToAbsolute(match.address));

    // We already know this is a function location, so at worst we should get back |fn| from
    // GetMostSpecificChild.
    FX_DCHECK(block);

    if (seen_code_blocks.find(block) != seen_code_blocks.end()) {
      // This address is covered by another match's corresponding CodeBlock, we can discard it.
      continue;
    }

    // New CodeBlock, keep it.
    seen_code_blocks.insert(block);
    matches.push_back(match);
  }

  // Complications 4 & 5 above: Get all instances of the best match only with a typical max of one
  // per function. The best match is the one with the lowest line number (found matches should all
  // be bigger than the input line, so this will be the closest). Some matches will appear twice
  // here, indicating that the function was split into unique, discontiguous code regions, and we
  // have to include the extra ones to accommodate https://github.com/rust-lang/rust/issues/123341.
  for (const LineMatch& match : GetBestLineMatches(matches)) {
    uint64_t abs_addr = symbol_context.RelativeToAbsolute(match.address);
    if (options.symbolize) {
      output->push_back(LocationForAddress(symbol_context, abs_addr, options, nullptr));
    } else {
      output->push_back(Location(Location::State::kAddress, abs_addr));
    }
  }
}

Location ModuleSymbolsImpl::MakeElfSymbolLocation(const SymbolContext& symbol_context,
                                                  std::optional<uint64_t> relative_address,
                                                  const ElfSymbolRecord& record) const {
  uint64_t absolute_address;
  if (relative_address) {
    // Caller specified a more specific address (normally inside the ELF symbol).
    absolute_address = symbol_context.RelativeToAbsolute(*relative_address);
  } else {
    // Take address from the ELF symbol.
    absolute_address = symbol_context.RelativeToAbsolute(record.relative_address);
  }

  return Location(
      absolute_address, FileLine(), 0, symbol_context,
      fxl::MakeRefCounted<ElfSymbol>(const_cast<ModuleSymbolsImpl*>(this)->GetWeakPtr(), record));
}

void ModuleSymbolsImpl::CreateIndex() {
  // Create the index for the main binary.
  //
  // We could consider creating a new binary/object file just for indexing. The indexing will page
  // all of the binary in, and most of it won't be needed again (it will be paged back in slowly,
  // savings may make such a change worth it for large programs as needed).
  //
  // Although it will be slightly slower to create, the memory savings may make such a change
  // worth it for large programs.
  ModuleIndexerResult result = IndexModule(GetWeakPtr(), *binary_, build_dir_);
  index_ = std::move(result.index);
  dwos_ = std::move(result.dwo_info);

  // Index the DWOs array by skeleton die offset.
  dwo_skeleton_offset_to_index_.reserve(dwos_.size());
  for (size_t i = 0; i < dwos_.size(); i++) {
    if (dwos_[i]) {
      dwo_skeleton_offset_to_index_[dwos_[i]->skeleton().skeleton_die_offset] = i;
    }
  }
}

void ModuleSymbolsImpl::FillElfSymbols() {
  FX_DCHECK(mangled_elf_symbols_.empty());
  FX_DCHECK(elf_addresses_.empty());

  const std::map<std::string, llvm::ELF::Elf64_Sym>& elf_syms = binary_->GetELFSymbols();
  const std::map<std::string, uint64_t>& plt_syms = binary_->GetPLTSymbols();

  // Insert the regular symbols.
  //
  // The |st_value| is the relative virtual address we want to index. Potentially we might want to
  // save more flags and expose them in the ElfSymbol class.
  for (const auto& [name, sym] : elf_syms) {
    // The symbol type is the low 4 bits. The higher bits encode the visibility which we don't
    // care about. We only need to index objects and code, and a couple of special symbols left
    // specifically for Zxdb's usage.
    int symbol_type = sym.st_info & 0xf;
    if (symbol_type != elflib::STT_OBJECT && symbol_type != elflib::STT_FUNC &&
        symbol_type != elflib::STT_TLS && name.substr(0, 5) != "zxdb.")
      continue;

    if (sym.st_value == 0)
      continue;  // No address for this symbol. Probably imported.

    auto inserted = mangled_elf_symbols_.emplace(
        std::piecewise_construct, std::forward_as_tuple(name),
        std::forward_as_tuple(ElfSymbolType::kNormal, sym.st_value, sym.st_size, name));

    // Append all addresses for now, this will be sorted at the bottom.
    elf_addresses_.push_back(&inserted->second);
  }

  // Insert PLT symbols.
  for (const auto& [name, addr] : plt_syms) {
    // TODO(sadmac): Set the symbol size to the size of a PLT entry on this architecture.
    auto inserted =
        mangled_elf_symbols_.emplace(std::piecewise_construct, std::forward_as_tuple(name),
                                     std::forward_as_tuple(ElfSymbolType::kPlt, addr, 0, name));

    // Append all addresses for now, this will be sorted at the bottom.
    elf_addresses_.push_back(&inserted->second);
  }

  std::sort(elf_addresses_.begin(), elf_addresses_.end(),
            [](const ElfSymbolRecord* left, const ElfSymbolRecord* right) {
              return left->relative_address < right->relative_address;
            });
}

}  // namespace zxdb
