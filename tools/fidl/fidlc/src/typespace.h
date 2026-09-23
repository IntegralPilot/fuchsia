// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_TYPESPACE_H_
#define TOOLS_FIDL_FIDLC_SRC_TYPESPACE_H_

#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/src/name.h"
#include "tools/fidl/fidlc/src/reporter.h"
#include "tools/fidl/fidlc/src/types.h"

namespace fidlc {

constexpr uint32_t kHandleSameRights = 0x80000000;  // ZX_HANDLE_SAME_RIGHTS

class TypeResolver;
struct LayoutInvocation;
struct LayoutParameterList;
struct TypeConstraints;

// Typespace owns all types. It provides methods for building types which
// store the object in the typespace and return a Type*.
class Typespace final {
 public:
  // Initializes the typespace with builtin types from the root library.
  Typespace(const Library* root_library, Reporter* reporter);
  Typespace(const Typespace&) = delete;
  Typespace(Typespace&&) = default;

  Reporter* reporter() { return reporter_; }
  const std::vector<std::unique_ptr<Type>>& types() { return types_; }

  Type* Create(TypeResolver* resolver, const Reference& layout,
               const LayoutParameterList& parameters, const TypeConstraints& constraints,
               bool compile_decls, LayoutInvocation* out_params);

  PrimitiveType* GetPrimitiveType(PrimitiveSubtype subtype);
  InternalType* GetInternalType(InternalSubtype subtype);
  Type* GetUnboundedStringType();
  Type* GetStringType(size_t max_size);
  Type* GetUntypedNumericType();

 private:
  class Creator;

  Type* Intern(std::unique_ptr<Type> type);

  Reporter* reporter_;

  std::vector<std::unique_ptr<Type>> types_;
  std::map<PrimitiveSubtype, std::unique_ptr<PrimitiveType>> primitive_types_;
  std::map<InternalSubtype, std::unique_ptr<InternalType>> internal_types_;
  std::unique_ptr<StringType> unbounded_string_type_;
  std::unique_ptr<UntypedNumericType> untyped_numeric_type_;
  std::vector<std::unique_ptr<SizeValue>> sizes_;
  std::optional<Name> vector_layout_name_;
  std::optional<Name> pointer_type_name_;
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_TYPESPACE_H_
