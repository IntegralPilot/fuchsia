// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_DRIVER_SYMBOLS_SYMBOLS_H_
#define SRC_LIB_DRIVER_SYMBOLS_SYMBOLS_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

#ifdef __cplusplus

#include <lib/zx/result.h>
#include <lib/zx/vmo.h>

#include <string_view>
#include <vector>

namespace driver_symbols {

// Checks the |driver_vmo| for usage of restricted symbols.
// Returns a vector of any restricted symbols, or an error if the vmo is not an ELF file.
zx::result<std::vector<std::string>> FindRestrictedSymbols(zx::unowned_vmo driver_vmo,
                                                           std::string_view driver_url);

}  // namespace driver_symbols

#endif

__BEGIN_CDECLS

// LINT.IfChange
typedef struct restricted_symbols restricted_symbols_t;

zx_status_t restricted_symbols_find(zx_handle_t driver_vmo, const char* driver_url,
                                    restricted_symbols_t** out_symbols, size_t* out_symbols_found);

const char* restricted_symbols_get(restricted_symbols_t* symbols, size_t index);

void restricted_symbols_free(restricted_symbols_t* symbols);
// LINT.ThenChange(/src/lib/driver_symbols/src/bindings.rs)

__END_CDECLS

#endif  // SRC_LIB_DRIVER_SYMBOLS_SYMBOLS_H_
