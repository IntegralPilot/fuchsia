// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/expr/register_utils.h"

#include "src/developer/debug/shared/register_info.h"
#include "src/developer/debug/zxdb/expr/builtin_types.h"
#include "src/developer/debug/zxdb/expr/find_name.h"
#include "src/developer/debug/zxdb/symbols/modified_type.h"

namespace zxdb {

debug::RegisterID GetRegisterID(debug::Arch arch, const ParsedIdentifier& ident) {
  // Check for explicit register identifier annotation.
  if (ident.components().size() == 1 &&
      ident.components()[0].special() == SpecialIdentifier::kRegister) {
    return debug::StringToRegisterID(arch, ident.components()[0].name());
  }

  // Try to convert the identifier string to a register name.
  auto str = GetSingleComponentIdentifierName(ident);
  if (!str)
    return debug::RegisterID::kUnknown;
  return debug::StringToRegisterID(arch, *str);
}

Err GetUnavailableRegisterErr(debug::RegisterID id) {
  return Err("Register %s unavailable in this context.", debug::RegisterIDToString(id));
}

ErrOrValue RegisterDataToValue(ExprLanguage lang, debug::RegisterID id,
                               VectorRegisterFormat vector_fmt, std::span<const uint8_t> data) {
  const debug::RegisterInfo* info = debug::InfoForRegister(id);
  if (!info)
    return Err("Unknown register");

  ExprValueSource source(id);

  switch (info->format) {
    case debug::RegisterFormat::kGeneral:
    case debug::RegisterFormat::kSpecial: {
      return ExprValue(GetBuiltinUnsignedType(lang, data.size()),
                       std::vector<uint8_t>(data.begin(), data.end()), source);
    }

    case debug::RegisterFormat::kFloat: {
      return ExprValue(GetBuiltinFloatType(lang, data.size()),
                       std::vector<uint8_t>(data.begin(), data.end()), source);
    }

    case debug::RegisterFormat::kVector: {
      return VectorRegisterToValue(id, vector_fmt, std::vector<uint8_t>(data.begin(), data.end()));
    }

    case debug::RegisterFormat::kVoidAddress: {
      // A void* is a pointer to no type.
      return ExprValue(fxl::MakeRefCounted<ModifiedType>(DwarfTag::kPointerType, LazySymbol()),
                       std::vector<uint8_t>(data.begin(), data.end()), source);
    }

    case debug::RegisterFormat::kWordAddress: {
      auto word_ptr_type = fxl::MakeRefCounted<ModifiedType>(DwarfTag::kPointerType,
                                                             GetBuiltinUnsignedType(lang, 8));
      return ExprValue(word_ptr_type, std::vector<uint8_t>(data.begin(), data.end()), source);
    }
  }

  return Err("Unknown register type");
}

}  // namespace zxdb
