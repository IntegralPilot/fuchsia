// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_NAMES_H_
#define TOOLS_FIDL_FIDLC_SRC_NAMES_H_

#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/src/properties.h"
#include "tools/fidl/fidlc/src/raw_ast.h"
#include "tools/fidl/fidlc/src/types.h"
#include "tools/fidl/fidlc/src/values.h"

namespace fidlc {

std::string NameHandleSubtype(HandleSubtype subtype);
std::string NameRawLiteralKind(RawLiteral::Kind kind);
std::string NameConstantKind(Constant::Kind kind);
std::string NameConstant(const Constant* constant);
std::string NameTypeKind(const Type* type);
std::string NameType(const Type* type);
std::string NameMethodKind(Protocol::Method::Kind kind);
std::string FullyQualifiedName(const Name& name);

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_NAMES_H_
