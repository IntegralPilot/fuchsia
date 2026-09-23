// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_EXPR_REGISTER_UTILS_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_EXPR_REGISTER_UTILS_H_

#include <span>

#include "src/developer/debug/shared/arch.h"
#include "src/developer/debug/zxdb/expr/expr_language.h"
#include "src/developer/debug/zxdb/expr/expr_value.h"
#include "src/developer/debug/zxdb/expr/parsed_identifier.h"
#include "src/developer/debug/zxdb/expr/vector_register_format.h"

namespace zxdb {

// Converts an identifier to a RegisterID. Returns kUnknown if the identifier is
// not a register for the given architecture.
debug::RegisterID GetRegisterID(debug::Arch arch, const ParsedIdentifier& ident);

// Returns a generic error message for register |id|.
Err GetUnavailableRegisterErr(debug::RegisterID id);

// Converts |data| into an ExprValue with the given |id|. The format for
// non-vector registers will be looked up based on |id|. |lang| will determine
// the builtin Type symbol that will represent the register data.
ErrOrValue RegisterDataToValue(ExprLanguage lang, debug::RegisterID id,
                               VectorRegisterFormat vector_fmt, std::span<const uint8_t> data);
}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_EXPR_REGISTER_UTILS_H_
