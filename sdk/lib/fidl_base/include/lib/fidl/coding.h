// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FIDL_CODING_H_
#define LIB_FIDL_CODING_H_

#include <zircon/compiler.h>
#include <zircon/fidl.h>
#include <zircon/types.h>

__BEGIN_CDECLS

// The maximum recursion depth the fidl encoder or decoder will
// perform. Each nested aggregate type (structs, unions, arrays,
// vectors, or tables) counts as one step in the recursion depth.
#define FIDL_RECURSION_DEPTH 32

// Perform a fidl_decode_etc as input for HLCPP (leave unknown handles in flexible resource types
// intact instead of closing them, add offsets to unknown envelopes).
// IT MAY BREAK AT ANY TIME OR BE REMOVED WITHOUT NOTICE.
zx_status_t internal__fidl_decode_etc_hlcpp__v2__may_break(const fidl_type_t* type, void* bytes,
                                                           uint32_t num_bytes,
                                                           const zx_handle_info_t* handle_infos,
                                                           uint32_t num_handle_infos,
                                                           const char** error_msg_out);

// Validates an encoded message against the given |type|.
//
// The |bytes| are not modified.
//
// This is a version of the FIDL validator that validates against the v2 wire format.
// IT MAY BREAK AT ANY TIME OR BE REMOVED WITHOUT NOTICE.
zx_status_t internal__fidl_validate__v2__may_break(const fidl_type_t* type, const void* bytes,
                                                   uint32_t num_bytes, uint32_t num_handles,
                                                   const char** out_error_msg);

// Stores the name of a fidl type into the provided buffer.
// Truncates the name if it is too long to fit into the buffer.
// Returns the number of characters written into the buffer.
//
// Note: This function does not write a trailing NUL.
size_t fidl_format_type_name(const fidl_type_t* type, char* buffer, size_t capacity);

__END_CDECLS

#endif  // LIB_FIDL_CODING_H_
