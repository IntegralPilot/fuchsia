// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/symbols/symbol_utils.h"

#include <lib/syslog/cpp/macros.h>

#include "src/developer/debug/shared/logging/logging.h"
#include "src/developer/debug/zxdb/symbols/array_type.h"
#include "src/developer/debug/zxdb/symbols/base_type.h"
#include "src/developer/debug/zxdb/symbols/collection.h"
#include "src/developer/debug/zxdb/symbols/compile_unit.h"
#include "src/developer/debug/zxdb/symbols/data_member.h"
#include "src/developer/debug/zxdb/symbols/dwarf_tag.h"
#include "src/developer/debug/zxdb/symbols/modified_type.h"
#include "src/developer/debug/zxdb/symbols/namespace.h"
#include "src/developer/debug/zxdb/symbols/template_parameter.h"
#include "src/developer/debug/zxdb/symbols/type.h"
#include "src/developer/debug/zxdb/symbols/variable.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace zxdb {

namespace {

// Since we can't use the expression system to resolve the actual value from
// DW_TAG_template_value_parameters, we have to do a simple decoding ourselves.
void AddTemplateValueToName(std::string& name, const TemplateParameter* template_value) {
  const Type* type = template_value->type().Get()->As<Type>();
  if (!type)
    return;
  auto base_type = type->StripCVT()->As<BaseType>();

  if (!base_type)
    return;

  // This vector assumes little endian, so it's in LSB -> MSB order.
  std::vector<uint8_t> v = template_value->const_value().GetConstValue(base_type->byte_size());
  std::string s;

  switch (base_type->base_type()) {
    case BaseType::kBaseTypeBoolean: {
      FX_DCHECK(v.size() == 1);
      if (v[0] == 1) {
        s = "true";
      } else {
        s = "false";
      }
      break;
    }
    case BaseType::kBaseTypeSignedChar:
    case BaseType::kBaseTypeUnsignedChar: {
      FX_DCHECK(v.size() == 1);
      s = fxl::StringPrintf("'%c'", static_cast<char>(v[0]));
      break;
    }
    case BaseType::kBaseTypeSigned:
    case BaseType::kBaseTypeUnsigned: {
      FX_DCHECK(v.size() >= 2);
      uint64_t t = 0;
      for (int i = base_type->byte_size() - 1; i >= 0; i--) {
        t |= (static_cast<uint64_t>(v[i]) << (8 * i));
      }
      s = std::to_string(t);
      break;
    }
    default:
      LOGS(Warn) << BaseType::BaseTypeToString(base_type->base_type(), true)
                 << " is not supported.";
      return;
  }

  AddTemplateParameterToName(name, s.c_str());
}
}  // namespace

Identifier GetSymbolScopePrefix(const Symbol* symbol) {
  if (!symbol->parent().is_valid())
    return Identifier(IdentifierQualification::kGlobal);  // No prefix

  fxl::RefPtr<Symbol> parent = symbol->parent().Get();
  if (parent->tag() == DwarfTag::kCompileUnit)
    return Identifier(IdentifierQualification::kGlobal);  // Don't go above compilation units.

  if (parent->As<Namespace>() || parent->As<Collection>() || parent->As<Function>()) {
    // These are the types that get qualified.
    return parent->GetIdentifier();
  }
  // Anything else is skipped and we just return the parent's prefix. This
  // will include things like lexical blocks.
  return GetSymbolScopePrefix(parent.get());
}

fxl::RefPtr<Collection> MakeRustTuple(const std::string& name,
                                      const std::vector<fxl::RefPtr<Type>>& members) {
  auto coll = fxl::MakeRefCounted<Collection>(DwarfTag::kStructureType, name);
  auto unit =
      fxl::MakeRefCounted<CompileUnit>(DwarfTag::kCompileUnit, fxl::WeakPtr<ModuleSymbols>(),
                                       nullptr, DwarfLang::kRust, "<no file>", std::nullopt);
  coll->set_parent(UncachedLazySymbol::MakeUnsafe(unit));

  uint32_t offset = 0;
  std::vector<LazySymbol> data_members;
  for (size_t i = 0; i < members.size(); i++) {
    auto& type = members[i];
    auto data = fxl::MakeRefCounted<DataMember>("__" + std::to_string(i), type, offset);

    data_members.emplace_back(std::move(data));
    offset += type->byte_size();
  }

  coll->set_byte_size(offset);
  coll->set_data_members(std::move(data_members));
  return coll;
}

fxl::RefPtr<Type> MakeStringLiteralType(size_t length) {
  auto char_type = fxl::MakeRefCounted<BaseType>(BaseType::kBaseTypeSignedChar, 1, "char");
  return fxl::MakeRefCounted<ArrayType>(std::move(char_type), length);
}

fxl::RefPtr<Type> AddCVQualifiersToMatch(const Type* reference, fxl::RefPtr<Type> modified) {
  const Type* source = reference;
  while (source) {
    const ModifiedType* mod_source = source->As<ModifiedType>();
    if (!mod_source || !DwarfTagIsCVQualifier(mod_source->tag()))
      break;

    modified = fxl::MakeRefCounted<ModifiedType>(mod_source->tag(), std::move(modified));
    source = mod_source->modified().Get()->As<Type>();
  }

  return modified;
}

bool NameHasTemplate(std::string_view name) { return !name.empty() && name.back() == '>'; }

void AddTemplateParameterToName(std::string& original, const char* type_name) {
  if (!type_name)
    return;

  // This type already has a template type, we need to append this one.
  if (NameHasTemplate(original)) {
    original.pop_back();
    std::string append_str = ", ";
    append_str += type_name;
    original.append(append_str);
  } else {
    // No template type yet, add "<...>".
    original.push_back('<');
    original.append(type_name);
  }

  original.push_back('>');
}

void AddAllTemplateParametersToName(std::string& name,
                                    const std::vector<LazySymbol>& template_params) {
  for (const auto& param : template_params) {
    const TemplateParameter* template_parameter = param.Get()->As<TemplateParameter>();
    if (template_parameter) {
      if (template_parameter->is_value() && template_parameter->const_value().has_value()) {
        AddTemplateValueToName(name, template_parameter);
      } else {
        const Type* type = template_parameter->type().Get()->As<Type>();
        if (type) {
          AddTemplateParameterToName(name, type->GetAssignedName().c_str());
        } else {
          AddTemplateParameterToName(name, "<unknown>");
        }
      }
    }
  }
}

}  // namespace zxdb
