// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/names.h"

#include <sstream>

#include "tools/fidl/fidlc/src/flat_ast.h"

namespace fidlc {

std::string NameHandleSubtype(HandleSubtype subtype) {
  switch (subtype) {
    case HandleSubtype::kHandle:
      return "handle";
    case HandleSubtype::kBti:
      return "bti";
    case HandleSubtype::kChannel:
      return "channel";
    case HandleSubtype::kClock:
      return "clock";
    case HandleSubtype::kCounter:
      return "counter";
    case HandleSubtype::kEvent:
      return "event";
    case HandleSubtype::kEventpair:
      return "eventpair";
    case HandleSubtype::kException:
      return "exception";
    case HandleSubtype::kFifo:
      return "fifo";
    case HandleSubtype::kGuest:
      return "guest";
    case HandleSubtype::kInterrupt:
      return "interrupt";
    case HandleSubtype::kIob:
      return "iob";
    case HandleSubtype::kIommu:
      return "iommu";
    case HandleSubtype::kJob:
      return "job";
    case HandleSubtype::kDebugLog:
      return "debuglog";
    case HandleSubtype::kMsi:
      return "msi";
    case HandleSubtype::kPager:
      return "pager";
    case HandleSubtype::kPciDevice:
      return "pcidevice";
    case HandleSubtype::kPmt:
      return "pmt";
    case HandleSubtype::kPort:
      return "port";
    case HandleSubtype::kProcess:
      return "process";
    case HandleSubtype::kProfile:
      return "profile";
    case HandleSubtype::kResource:
      return "resource";
    case HandleSubtype::kSocket:
      return "socket";
    case HandleSubtype::kStream:
      return "stream";
    case HandleSubtype::kSuspendToken:
      return "suspendtoken";
    case HandleSubtype::kThread:
      return "thread";
    case HandleSubtype::kTimer:
      return "timer";
    case HandleSubtype::kVcpu:
      return "vcpu";
    case HandleSubtype::kVmar:
      return "vmar";
    case HandleSubtype::kVmo:
      return "vmo";
  }
}

std::string NameRawLiteralKind(RawLiteral::Kind kind) {
  switch (kind) {
    case RawLiteral::Kind::kDocComment:
    case RawLiteral::Kind::kString:
      return "string";
    case RawLiteral::Kind::kNumeric:
      return "numeric";
    case RawLiteral::Kind::kBool:
      return "bool";
  }
}

std::string NameTypeKind(const Type* type) {
  switch (type->kind) {
    case Type::Kind::kArray:
      if (static_cast<const ArrayType*>(type)->IsStringArray()) {
        return "string_array";
      }
      return "array";
    case Type::Kind::kVector:
      return "vector";
    case Type::Kind::kZxExperimentalPointer:
      return "experimental_pointer";
    case Type::Kind::kString:
      return "string";
    case Type::Kind::kHandle:
      return "handle";
    case Type::Kind::kTransportSide:
      return "endpoint";
    case Type::Kind::kPrimitive:
      return "primitive";
    case Type::Kind::kInternal:
      return "internal";
    // TODO(https://fxbug.dev/42149402): transition the JSON and other backends to using box
    case Type::Kind::kBox:
    case Type::Kind::kIdentifier:
      return "identifier";
    case Type::Kind::kUntypedNumeric:
      ZX_PANIC("should not have untyped numeric here");
  }
}

std::string NameConstantKind(Constant::Kind kind) {
  switch (kind) {
    case Constant::Kind::kIdentifier:
      return "identifier";
    case Constant::Kind::kLiteral:
      return "literal";
    case Constant::Kind::kBinaryOperator:
      return "binary_operator";
  }
}

std::string NameConstant(const Constant* constant) {
  switch (constant->kind) {
    case Constant::Kind::kLiteral: {
      auto literal_constant = static_cast<const LiteralConstant*>(constant);
      return std::string(literal_constant->literal->span().data());
    }
    case Constant::Kind::kIdentifier: {
      auto identifier_constant = static_cast<const IdentifierConstant*>(constant);
      return FullyQualifiedName(identifier_constant->reference.resolved().name());
    }
    case Constant::Kind::kBinaryOperator: {
      return std::string("binary operator");
    }
  }
}

void NameTypeHelper(std::ostringstream& buf, const Type* type) {
  buf << FullyQualifiedName(type->name);
  switch (type->kind) {
    case Type::Kind::kArray: {
      const auto* array_type = static_cast<const ArrayType*>(type);
      buf << '<';
      NameTypeHelper(buf, array_type->element_type);
      if (array_type->element_count->value != kMaxSize) {
        buf << ", ";
        buf << array_type->element_count->value;
      }
      buf << '>';
      break;
    }
    case Type::Kind::kVector: {
      const auto* vector_type = static_cast<const VectorType*>(type);
      buf << '<';
      NameTypeHelper(buf, vector_type->element_type);
      buf << '>';
      if (vector_type->ElementCount() != kMaxSize) {
        buf << ':';
        buf << vector_type->ElementCount();
      }
      break;
    }
    case Type::Kind::kString: {
      const auto* string_type = static_cast<const StringType*>(type);
      if (string_type->MaxSize() != kMaxSize) {
        buf << ':';
        buf << string_type->MaxSize();
      }
      break;
    }
    case Type::Kind::kZxExperimentalPointer: {
      const auto* pointer_type = static_cast<const ZxExperimentalPointerType*>(type);
      buf << '<';
      NameTypeHelper(buf, pointer_type->pointee_type);
      buf << '>';
      break;
    }
    case Type::Kind::kHandle: {
      const auto* handle_type = static_cast<const HandleType*>(type);
      if (handle_type->subtype != HandleSubtype::kHandle) {
        buf << ':';
        buf << NameHandleSubtype(handle_type->subtype);
      }
      break;
    }
    case Type::Kind::kTransportSide: {
      const auto* transport_side = static_cast<const TransportSideType*>(type);
      buf << (transport_side->end == TransportSide::kClient ? "client" : "server");
      buf << ':';
      buf << FullyQualifiedName(transport_side->protocol_decl->name);
      break;
    }
    case Type::Kind::kBox: {
      const auto* box_type = static_cast<const BoxType*>(type);
      buf << '<';
      buf << FullyQualifiedName(box_type->boxed_type->name);
      buf << '>';
      break;
    }
    case Type::Kind::kPrimitive:
    case Type::Kind::kInternal:
    case Type::Kind::kIdentifier:
    case Type::Kind::kUntypedNumeric:
      // Like Stars, they are known by name.
      break;
  }  // switch
  // TODO(https://fxbug.dev/42175844): Use the new syntax, `:optional`.
  if (type->IsNullable()) {
    buf << '?';
  }
}

std::string NameType(const Type* type) {
  std::ostringstream buf;
  NameTypeHelper(buf, type);
  return buf.str();
}

std::string NameMethodKind(Protocol::Method::Kind kind) {
  switch (kind) {
    case Protocol::Method::Kind::kOneWay:
      return "oneway";
    case Protocol::Method::Kind::kTwoWay:
      return "twoway";
    case Protocol::Method::Kind::kEvent:
      return "event";
  }
}

std::string FullyQualifiedName(const Name& name) {
  if (name.is_intrinsic())
    return name.full_name();
  return name.library()->name + "/" + name.full_name();
}

}  // namespace fidlc
