// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/ffx/lib/symbolize/sys/wrapper.h"

#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/zxdb/common/curl.h"
#include "src/developer/debug/zxdb/format/format_name.h"
#include "tools/symbolizer/command_line_options.h"
#include "tools/symbolizer/symbolizer.h"
#include "tools/symbolizer/symbolizer_impl.h"

extern "C" {
void symbolizer_global_init() { zxdb::Curl::GlobalInit(); }
void symbolizer_global_cleanup() { zxdb::Curl::GlobalCleanup(); }

symbolizer::SymbolizerImpl* symbolizer_new() {
  symbolizer::CommandLineOptions options;
  options.SetupDefaultsFromEnvironment();
  return new symbolizer::SymbolizerImpl(options);
}
void symbolizer_free(symbolizer::SymbolizerImpl* symbolizer) { delete symbolizer; }

void symbolizer_reset(symbolizer::SymbolizerImpl* symbolizer) {
  symbolizer::Symbolizer::ResetType type_t = symbolizer::Symbolizer::ResetType::kUnknown;
  symbolizer->Reset(false, type_t);
}
void symbolizer_add_module(symbolizer::SymbolizerImpl* symbolizer, uint64_t id, const char* name,
                           size_t name_len, const char* build_id, size_t build_id_len) {
  symbolizer->Module(id, std::string_view(name, name_len),
                     std::string_view(build_id, build_id_len));
}

MappingStatus symbolizer_add_mapping(symbolizer::SymbolizerImpl* symbolizer, uint64_t module_id,
                                     uint64_t start_addr, uint64_t size, uint64_t module_offset,
                                     const char* flags, size_t flags_len) {
  auto status = symbolizer->MMap(start_addr, size, module_id, std::string_view(flags, flags_len),
                                 module_offset);
  switch (status) {
    case symbolizer::SymbolizerImpl::MMapStatus::kOk:
      return MappingStatus::Ok;
    case symbolizer::SymbolizerImpl::MMapStatus::kInconsistentBaseAddress:
      return MappingStatus::InconsistentBaseAddress;
    case symbolizer::SymbolizerImpl::MMapStatus::kInvalidModuleId:
      return MappingStatus::InvalidModuleId;
  };
}

ResolveAddressStatus symbolizer_resolve_address(symbolizer::SymbolizerImpl* symbolizer,
                                                uint64_t address, location_callback output,
                                                void* output_context) {
  symbolizer::SymbolizerImpl::BacktraceStatus status = symbolizer->Backtrace(
      address, debug_ipc::StackFrame::AddressType::kUnknown,
      [address, output, output_context](auto inline_index, auto& location, auto& module) {
        symbolizer_location_t output_location;

        // Store the variable at this scope so it lives as long as the callback invocation.
        std::string function_name;
        if (location.symbol().is_valid()) {
          auto symbol = location.symbol().Get();
          auto function = symbol->template As<zxdb::Function>();
          // There are some functions that are not considered zxdb::Function, e.g., functions in
          // vDSO and functions in crt1.S.
          // TODO(https://fxbug.dev/404336927): Investigate why cast doesn't work here for some
          // functions
          if (function) {
            function_name = zxdb::FormatFunctionName(function, {}).AsString();
          } else {
            function_name = symbol->GetFullName();
          }
          output_location.function = function_name.c_str();
          output_location.function_len = function_name.size();
        }

        if (location.file_line().is_valid()) {
          output_location.file = location.file_line().file().c_str();
          output_location.file_len = location.file_line().file().size();
          output_location.line = location.file_line().line();
        }

        if (module.name.size() > 0) {
          output_location.library = module.name.c_str();
          output_location.library_len = module.name.size();
        }
        output_location.library_offset = address - module.base + module.negative_base;

        output(&output_location, output_context);
      });
  switch (status) {
    case symbolizer::SymbolizerImpl::BacktraceStatus::kOk:
      return ResolveAddressStatus::Ok;
    case symbolizer::SymbolizerImpl::BacktraceStatus::kSymbolFileUnavailable:
      return ResolveAddressStatus::SymbolFileUnavailable;
    case symbolizer::SymbolizerImpl::BacktraceStatus::kNoOverlappingModule:
      return ResolveAddressStatus::NoOverlappingModule;
  }
}
}
