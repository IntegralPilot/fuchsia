// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/verify_steps.h"

#include <zircon/assert.h>

#include "tools/fidl/fidlc/src/attribute_schema.h"
#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/src/names.h"
#include "tools/fidl/fidlc/src/transport.h"

namespace fidlc {

namespace {

class HandleTransportHelper {
 public:
  HandleTransportHelper(const Protocol* protocol, Reporter* reporter)
      : protocol_(protocol), reporter_(reporter) {
    std::string_view transport_name = "Channel";
    Attribute* transport_attribute = protocol_->attributes->Get("transport");
    if (transport_attribute != nullptr) {
      auto arg = transport_attribute->GetArg(AttributeArg::kDefaultAnonymousName);
      std::string_view quoted_transport =
          static_cast<const LiteralConstant*>(arg->value.get())->literal->span().data();
      // Remove quotes around the transport.
      transport_name = quoted_transport.substr(1, quoted_transport.size() - 2);
    }
    transport_ = Transport::FromTransportName(transport_name);
  }

  void Check() {
    if (!transport_)
      return;
    for (auto& method : protocol_->methods) {
      if (auto& request = method.maybe_request)
        Visit(request->type, method.name);
      if (auto& response = method.maybe_response)
        Visit(response->type, method.name);
    }
  }

 private:
  void Visit(const Type* type, SourceSpan span) {
    switch (type->kind) {
      case Type::Kind::kUntypedNumeric:
      case Type::Kind::kPrimitive:
      case Type::Kind::kString:
      case Type::Kind::kInternal:
        return;
      case Type::Kind::kArray:
        return Visit(static_cast<const ArrayType*>(type)->element_type, span);
      case Type::Kind::kVector:
        return Visit(static_cast<const VectorType*>(type)->element_type, span);
      case Type::Kind::kZxExperimentalPointer:
        return Visit(static_cast<const ZxExperimentalPointerType*>(type)->pointee_type, span);
      case Type::Kind::kBox:
        return Visit(static_cast<const BoxType*>(type)->boxed_type, span);
      case Type::Kind::kHandle: {
        const Resource* resource = static_cast<const HandleType*>(type)->resource_decl;
        std::optional<HandleClass> handle_class = HandleClassFromName(resource->name);
        if (!handle_class.has_value() || !transport_->IsCompatible(handle_class.value())) {
          reporter_->Fail(ErrHandleUsedInIncompatibleTransport, span,
                          FullyQualifiedName(resource->name), transport_->name, protocol_);
        }
        return;
      }
      case Type::Kind::kTransportSide: {
        std::string_view transport_name =
            static_cast<const TransportSideType*>(type)->protocol_transport;
        const Transport* transport_side_transport = Transport::FromTransportName(transport_name);
        ZX_ASSERT(transport_side_transport);
        if (!transport_side_transport->handle_class.has_value() ||
            !transport_->IsCompatible(transport_side_transport->handle_class.value())) {
          reporter_->Fail(ErrTransportEndUsedInIncompatibleTransport, span, transport_name,
                          transport_->name, protocol_);
        }
        return;
      }
      case Type::Kind::kIdentifier:
        // Handled below.
        break;
    }

    auto* decl = static_cast<const IdentifierType*>(type)->type_decl;
    if (auto [it, inserted] = seen_.insert(decl); !inserted)
      return;

    switch (decl->kind) {
      case Decl::Kind::kAlias:
      case Decl::Kind::kBuiltin:
      case Decl::Kind::kConst:
      case Decl::Kind::kProtocol:
      case Decl::Kind::kResource:
      case Decl::Kind::kService:
        ZX_PANIC("unexpected kind");
      case Decl::Kind::kBits:
      case Decl::Kind::kEnum:
        break;
      case Decl::Kind::kNewType:
        return Visit(static_cast<const NewType*>(decl)->type_ctor->type, span);
      case Decl::Kind::kStruct:
        for (auto& member : static_cast<const Struct*>(decl)->members)
          Visit(member.type_ctor->type, span);
        break;
      case Decl::Kind::kTable:
        for (auto& member : static_cast<const Table*>(decl)->members)
          Visit(member.type_ctor->type, span);
        break;
      case Decl::Kind::kUnion:
        for (auto& member : static_cast<const Union*>(decl)->members)
          Visit(member.type_ctor->type, span);
        break;
      case Decl::Kind::kOverlay:
        for (auto& member : static_cast<const Overlay*>(decl)->members)
          Visit(member.type_ctor->type, span);
        break;
    }
  }

  const Protocol* protocol_;
  Reporter* reporter_;
  const Transport* transport_;
  std::set<const Decl*> seen_;
};

}  // namespace

void VerifyResourcenessStep::RunImpl() {
  for (auto& decl : library()->declarations.structs)
    ValidateResourceness(decl.get());
  for (auto& decl : library()->declarations.tables)
    ValidateResourceness(decl.get());
  for (auto& decl : library()->declarations.unions)
    ValidateResourceness(decl.get());
  for (auto& decl : library()->declarations.overlays)
    ValidateResourceness(decl.get());
}

template <typename DeclType>
void VerifyResourcenessStep::ValidateResourceness(const DeclType* decl) {
  if (decl->resourceness.value() == Resourceness::kResource)
    return;
  for (auto& member : decl->members) {
    if (member.type_ctor->type->Resourceness() == Resourceness::kResource) {
      reporter()->Fail(ErrTypeMustBeResource, decl->name.span().value(), decl->kind, decl->name,
                       member.name.data(), member.name);
    }
  }
}

void VerifyHandleTransportStep::RunImpl() {
  for (const auto& protocol : library()->declarations.protocols) {
    HandleTransportHelper(protocol.get(), reporter()).Check();
  }
}

void VerifyAttributesStep::RunImpl() {
  library()->ForEachElement([&](Element* element) {
    for (const auto& attribute : element->attributes->attributes) {
      auto& schema = all_libraries()->RetrieveAttributeSchema(attribute.get());
      schema.Validate(reporter(), experimental_flags(), attribute.get(), element);
    }
  });
}

void VerifyDependenciesStep::RunImpl() {
  library()->dependencies.VerifyAllDependenciesWereUsed(library(), reporter());
}

}  // namespace fidlc
