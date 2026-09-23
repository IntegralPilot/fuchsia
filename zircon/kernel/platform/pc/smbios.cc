// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
//
#include <lib/smbios/smbios.h>
#include <stdint.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <kernel/ffi.h>
#include <platform/pc/smbios.h>

#include <ktl/enforce.h>

// FFI Declarations for Rust SMBIOS implementation.
extern "C" {
typedef zx_status_t (*RustSmbiosWalkCallback)(uint8_t major_ver, uint8_t minor_ver,
                                              uint8_t docrev_ver, const smbios::Header* hdr,
                                              const char* str_table_start, size_t str_table_len,
                                              void* ctx);

zx_status_t rust_smbios_walk_structs(RustSmbiosWalkCallback cb, void* ctx);
}  // extern "C"

namespace {

zx_status_t SmbiosWalkCallbackBridge(uint8_t major_ver, uint8_t minor_ver, uint8_t docrev_ver,
                                     const smbios::Header* hdr, const char* str_table_start,
                                     size_t str_table_len, void* ctx) {
  auto& cb = *reinterpret_cast<smbios::StructWalkCallback*>(ctx);
  smbios::SpecVersion version(major_ver, minor_ver, docrev_ver);

  smbios::StringTable st;
  zx_status_t status = st.Init(hdr, hdr->length + str_table_len);
  if (status != ZX_OK) {
    return status;
  }

  return cb(version, hdr, st);
}

}  // namespace

zx_status_t SmbiosWalkStructs(smbios::StructWalkCallback cb) {
  return rust_smbios_walk_structs(SmbiosWalkCallbackBridge, &cb);
}
