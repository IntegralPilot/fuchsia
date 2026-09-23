// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/symbols/mock_module_symbols.h"

#include <algorithm>

#include "src/developer/debug/shared/string_util.h"
#include "src/developer/debug/zxdb/symbols/function.h"
#include "src/developer/debug/zxdb/symbols/input_location.h"
#include "src/developer/debug/zxdb/symbols/lazy_symbol.h"
#include "src/developer/debug/zxdb/symbols/line_details.h"
#include "src/developer/debug/zxdb/symbols/location.h"
#include "src/developer/debug/zxdb/symbols/test_symbol_module.h"

namespace zxdb {

MockModuleSymbols::MockModuleSymbols(const std::string& local_file_name)
    : local_file_name_(local_file_name) {}
MockModuleSymbols::MockModuleSymbols(const std::string& local_file_name,
                                     const std::string& build_id, bool loaded)
    : local_file_name_(local_file_name), build_id_(build_id), loaded_(loaded) {}
MockModuleSymbols::~MockModuleSymbols() = default;

void MockModuleSymbols::AddSymbolLocations(const Identifier& name, std::vector<Location> locs) {
  named_symbols_[name] = std::move(locs);
}

void MockModuleSymbols::AddSymbolLocations(const std::string& name, std::vector<Location> locs) {
  named_symbols_[TestSymbolModule::SplitName(name)] = std::move(locs);
}

void MockModuleSymbols::AddSymbolLocations(uint64_t address, std::vector<Location> locs) {
  addr_symbols_[address] = std::move(locs);
}

void MockModuleSymbols::AddLineDetails(uint64_t address, LineDetails details) {
  lines_[address] = std::move(details);
}

void MockModuleSymbols::AddSymbolRef(const IndexNode::SymbolRef& die, fxl::RefPtr<Symbol> symbol) {
  die_refs_[die.die_ref()] = std::move(symbol);
}

void MockModuleSymbols::AddFileName(const std::string& file_name) { files_.push_back(file_name); }

void MockModuleSymbols::AddDebugAddrEntry(uint64_t addr_base, uint64_t index, uint64_t value) {
  debug_addr_entries_[std::make_pair(addr_base, index)] = value;
}

ModuleSymbolStatus MockModuleSymbols::GetStatus() const {
  ModuleSymbolStatus status;
  status.name = local_file_name_;
  status.functions_indexed = named_symbols_.size();
  status.symbols_loaded = loaded_;
  status.build_id = build_id_;
  return status;
}

std::vector<Location> MockModuleSymbols::ResolveInputLocation(const SymbolContext& symbol_context,
                                                              const InputLocation& input_location,
                                                              const ResolveOptions& options) const {
  std::vector<Location> result;
  switch (input_location.type) {
    case InputLocation::Type::kAddress:
      std::ignore =
          std::upper_bound(addr_symbols_.begin(), addr_symbols_.end(), input_location.address,
                           [](uint64_t addr, const auto& sym) mutable { return addr < sym.first; });
      if (auto found = addr_symbols_.find(input_location.address); found != addr_symbols_.end()) {
        result = found->second;
      } else {
        // Return identity for all non-found addresses.
        result.emplace_back(
            options.symbolize ? Location::State::kSymbolized : Location::State::kAddress,
            input_location.address);
      }
      break;
    case InputLocation::Type::kName: {
      // The input may be qualified or unqualified globally. Allow either to match an unqualified
      // expected value.
      if (auto found = named_symbols_.find(input_location.name); found != named_symbols_.end()) {
        result = found->second;
      } else if (input_location.name.qualification() == IdentifierQualification::kGlobal) {
        // Try looking up after removing the global qualifier.
        Identifier unqualified = input_location.name;
        unqualified.set_qualification(IdentifierQualification::kRelative);
        if (auto found = named_symbols_.find(unqualified); found != named_symbols_.end())
          result = found->second;
      }
      break;
    }
    default:
      // More complex stuff is not yet supported by this mock.
      break;
  }

  if (!options.symbolize) {
    // The caller did not request symbols so convert each result to an unsymbolized answer. This
    // will match the type of output from the non-mock version.
    for (size_t i = 0; i < result.size(); i++)
      result[i] = Location(Location::State::kAddress, result[i].address());
  }
  return result;
}

ModuleSymbols::FoundUnit MockModuleSymbols::GetDwarfUnitForAddress(
    const SymbolContext& symbol_context, uint64_t absolute_address) const {
  return FoundUnit{nullptr, nullptr};
}

LineDetails MockModuleSymbols::LineDetailsForAddress(const SymbolContext& symbol_context,
                                                     uint64_t absolute_address, bool greedy) const {
  // This mock assumes all addresses are absolute so the symbol context is not used.
  auto found = lines_.find(absolute_address);
  if (found == lines_.end())
    return LineDetails();
  return found->second;
}

std::vector<std::string> MockModuleSymbols::FindFileMatches(std::string_view name) const {
  std::vector<std::string> result;
  for (const std::string& cur : files_) {
    std::string with_slash("/");
    with_slash += std::string(name);
    if (cur == name || debug::StringEndsWith(cur, with_slash))
      result.push_back(cur);
  }
  return result;
}

std::vector<fxl::RefPtr<Function>> MockModuleSymbols::GetMainFunctions() const {
  return std::vector<fxl::RefPtr<Function>>();
}

const Index& MockModuleSymbols::GetIndex() const { return index_; }

LazySymbol MockModuleSymbols::IndexSymbolRefToSymbol(const IndexNode::SymbolRef& die_ref) const {
  auto found = die_refs_.find(die_ref.die_ref());
  if (found == die_refs_.end())
    return LazySymbol();
  return found->second;
}

bool MockModuleSymbols::HasBinary() const { return false; }

std::optional<uint64_t> MockModuleSymbols::GetDebugAddrEntry(uint64_t addr_base,
                                                             uint64_t index) const {
  if (auto found = debug_addr_entries_.find(std::make_pair(addr_base, index));
      found != debug_addr_entries_.end())
    return found->second;
  return std::nullopt;
}

}  // namespace zxdb
