// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FFX_LIB_SYMBOLIZE_SYS_WRAPPER_H_
#define SRC_DEVELOPER_FFX_LIB_SYMBOLIZE_SYS_WRAPPER_H_

#include <cstddef>
#include <cstdint>

// Forward declaration to allow for typed pointers. Pointers generated from this header must *not*
// be dereferenced by Rust because the size of this value does not match that of the underlying C++.
namespace symbolizer {
class SymbolizerImpl;
}  // namespace symbolizer

extern "C" {
void symbolizer_global_init();
void symbolizer_global_cleanup();

symbolizer::SymbolizerImpl* symbolizer_new();
void symbolizer_free(symbolizer::SymbolizerImpl* symbolizer);

void symbolizer_reset(symbolizer::SymbolizerImpl* symbolizer);
void symbolizer_add_module(symbolizer::SymbolizerImpl* symbolizer, uint64_t id, const char* name,
                           size_t name_len, const char* build_id, size_t build_id_len);

enum class MappingStatus : uint8_t {
  Ok,
  InconsistentBaseAddress,
  InvalidModuleId,
};
MappingStatus symbolizer_add_mapping(symbolizer::SymbolizerImpl* symbolizer, uint64_t module_id,
                                     uint64_t start_addr, uint64_t size, uint64_t module_offset,
                                     const char* flags, size_t flags_len);

// Does not own the pointed-to data.
struct symbolizer_location_t {
  const char* function = nullptr;
  size_t function_len = 0;

  const char* file = nullptr;
  size_t file_len = 0;

  uint32_t line = 0;

  const char* library = nullptr;
  size_t library_len = 0;

  uint64_t library_offset = 0;
};

typedef void (*location_callback)(const symbolizer_location_t* location, void* context);
enum class ResolveAddressStatus : uint8_t {
  Ok,
  SymbolFileUnavailable,
  NoOverlappingModule,
};
ResolveAddressStatus symbolizer_resolve_address(symbolizer::SymbolizerImpl* symbolizer,
                                                uint64_t address, location_callback output,
                                                void* output_context);
}

#endif /* SRC_DEVELOPER_FFX_LIB_SYMBOLIZE_SYS_WRAPPER_H_ */
