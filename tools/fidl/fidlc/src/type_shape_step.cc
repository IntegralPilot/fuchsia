// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/type_shape_step.h"

#include <zircon/assert.h>

#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/src/type_shape.h"

namespace fidlc {

namespace {

// A helper for doing checked math. On overflow, reports an error and returns 0.
// Returning 0 localizes the error rather than propagating it everywhere.
class CheckedMath {
 public:
  CheckedMath(Reporter* reporter, std::optional<SourceSpan> span)
      : reporter_(reporter), span_(span) {}

  uint32_t Add(uint32_t a, uint32_t b) {
    uint32_t sum;
    return __builtin_add_overflow(a, b, &sum) ? Fail(a, '+', b) : sum;
  }

  uint32_t Mul(uint32_t a, uint32_t b) {
    uint32_t prod;
    return __builtin_mul_overflow(a, b, &prod) ? Fail(a, '*', b) : prod;
  }

 private:
  uint32_t Fail(uint32_t a, char op, uint32_t b) {
    // TODO(https://fxbug.dev/323940291): Remove this. See Calculate(Type*).
    ZX_ASSERT_MSG(
        span_.has_value(),
        "TypeShapeStep had integer overflow, but can't report it because there is no source span");
    reporter_->Fail(ErrTypeShapeIntegerOverflow, span_.value(), a, op, b);
    return 0;
  }

  Reporter* reporter_;
  std::optional<SourceSpan> span_;
};

// Like std::add_sat from C++26.
uint32_t AddSat(uint32_t a, uint32_t b) {
  uint32_t sum;
  return __builtin_add_overflow(a, b, &sum) ? UINT32_MAX : sum;
}

// Like std::mul_sat from C++26.
uint32_t MulSat(uint32_t a, uint32_t b) {
  uint32_t prod;
  return __builtin_mul_overflow(a, b, &prod) ? UINT32_MAX : prod;
}

// Returns the padding required to align value to the given alignment.
uint32_t Padding(uint32_t value, uint32_t alignment) {
  // https://en.wikipedia.org/wiki/Data_structure_alignment#Computing_padding
  return -value & (alignment - 1);
}

// Returns padding for a FIDL wire format primary/secondary object.
uint32_t ObjectPadding(uint32_t size) { return Padding(size, 8); }

// Calculate the type shape for an envelope wrapping an inner type.
TypeShape Envelope(TypeShape inner) {
  bool inlined = inner.inline_size <= 4;
  auto padding = Padding(inner.inline_size, inlined ? 4 : 8);
  return TypeShape{
      .inline_size = 8,
      .alignment = 8,
      .depth = AddSat(inner.depth, 1),
      .max_handles = inner.max_handles,
      .max_out_of_line =
          AddSat(inner.max_out_of_line, inlined ? 0 : AddSat(inner.inline_size, padding)),
      .has_padding = inner.has_padding || padding != 0,
      .has_flexible_envelope = inner.has_flexible_envelope,
  };
}

}  // namespace

TypeShape TypeShapeStep::Calculate(TypeDecl* decl) {
  CheckedMath checked(reporter(), decl->name.span());
  switch (decl->kind) {
    case Decl::Kind::kBits:
      return Compile(static_cast<Bits*>(decl)->subtype_ctor->type);
    case Decl::Kind::kEnum:
      return Compile(static_cast<Enum*>(decl)->subtype_ctor->type);
    case Decl::Kind::kNewType:
      return Compile(static_cast<NewType*>(decl)->type_ctor->type);
    case Decl::Kind::kStruct: {
      auto struct_decl = static_cast<Struct*>(decl);
      TypeShape acc;
      FieldShape* prev_field = nullptr;
      for (auto& struct_member : struct_decl->members) {
        TypeShape member = Compile(struct_member.type_ctor->type);
        auto padding = Padding(acc.inline_size, member.alignment);
        auto offset = checked.Add(acc.inline_size, padding);
        struct_member.field_shape.offset = offset;
        if (prev_field)
          prev_field->padding = padding;
        acc.inline_size = checked.Add(offset, member.inline_size);
        acc.alignment = std::max(acc.alignment, member.alignment);
        acc.depth = std::max(acc.depth, member.depth);
        acc.max_handles = AddSat(acc.max_handles, member.max_handles);
        acc.max_out_of_line = AddSat(acc.max_out_of_line, member.max_out_of_line);
        acc.has_padding = acc.has_padding || member.has_padding || padding != 0;
        acc.has_flexible_envelope = acc.has_flexible_envelope || member.has_flexible_envelope;
        prev_field = &struct_member.field_shape;
      }
      if (struct_decl->members.empty()) {
        acc.inline_size = 1;
      } else {
        auto padding = Padding(acc.inline_size, acc.alignment);
        prev_field->padding = padding;
        acc.inline_size = checked.Add(acc.inline_size, padding);
        acc.has_padding = acc.has_padding || padding != 0;
      }
      // This limit exists so that coding tables can use uint16_t.
      if (uint32_t limit = UINT16_MAX; acc.inline_size > limit) {
        reporter()->Fail(ErrInlineSizeExceedsLimit, struct_decl->name.span().value(),
                         struct_decl->name, acc.inline_size, limit);
      }
      return acc;
    }
    case Decl::Kind::kTable: {
      auto table_decl = static_cast<Table*>(decl);
      TypeShape acc;
      uint32_t max_ordinal = 0;
      for (auto& table_member : table_decl->members) {
        TypeShape envelope = Envelope(Compile(table_member.type_ctor->type));
        acc.depth = std::max(acc.depth, envelope.depth);
        acc.max_handles = AddSat(acc.max_handles, envelope.max_handles);
        acc.max_out_of_line = AddSat(acc.max_out_of_line, envelope.max_out_of_line);
        acc.has_padding = acc.has_padding || envelope.has_padding;
        max_ordinal = std::max(max_ordinal, static_cast<uint32_t>(table_member.ordinal->value));
      }
      return TypeShape{
          .inline_size = 16,
          .alignment = 8,
          .depth = AddSat(acc.depth, 1),
          .max_handles = acc.max_handles,
          .max_out_of_line = AddSat(acc.max_out_of_line, MulSat(max_ordinal, 8)),
          .has_padding = acc.has_padding,
          .has_flexible_envelope = true,
      };
    }
    case Decl::Kind::kUnion: {
      auto union_decl = static_cast<Union*>(decl);
      TypeShape acc;
      for (auto& union_member : union_decl->members) {
        TypeShape envelope = Envelope(Compile(union_member.type_ctor->type));
        acc.depth = std::max(acc.depth, envelope.depth);
        acc.max_handles = std::max(acc.max_handles, envelope.max_handles);
        acc.max_out_of_line = std::max(acc.max_out_of_line, envelope.max_out_of_line);
        acc.has_padding = acc.has_padding || envelope.has_padding;
        acc.has_flexible_envelope = acc.has_flexible_envelope || envelope.has_flexible_envelope;
      }
      return TypeShape{
          .inline_size = 16,
          .alignment = 8,
          .depth = acc.depth,
          .max_handles = acc.max_handles,
          .max_out_of_line = acc.max_out_of_line,
          .has_padding = acc.has_padding,
          .has_flexible_envelope =
              acc.has_flexible_envelope || union_decl->strictness == Strictness::kFlexible,
      };
    }
    case Decl::Kind::kOverlay: {
      auto overlay_decl = static_cast<Overlay*>(decl);
      TypeShape acc;
      uint32_t smallest_member = UINT32_MAX, largest_member = 0;
      for (auto& overlay_member : overlay_decl->members) {
        TypeShape member = Compile(overlay_member.type_ctor->type);
        acc.depth = std::max(acc.depth, member.depth);
        acc.max_handles = std::max(acc.max_handles, member.max_handles);
        acc.max_out_of_line = std::max(acc.max_out_of_line, member.max_out_of_line);
        acc.has_padding = acc.has_padding || member.has_padding;
        acc.has_flexible_envelope = acc.has_flexible_envelope || member.has_flexible_envelope;
        smallest_member = std::min(smallest_member, member.inline_size);
        largest_member = std::max(largest_member, member.inline_size);
      }
      auto payload = checked.Add(largest_member, Padding(largest_member, 8));
      return TypeShape{
          .inline_size = checked.Add(payload, 8),
          .alignment = 8,
          .depth = acc.depth,
          .max_handles = acc.max_handles,
          .max_out_of_line = acc.max_out_of_line,
          .has_padding = acc.has_padding || smallest_member != payload,
          .has_flexible_envelope = acc.has_flexible_envelope,
      };
    }
    default:
      ZX_PANIC("unexpected kind");
  }
}

TypeShape TypeShapeStep::Calculate(Type* type) {
  // TODO(https://fxbug.dev/323940291): This doesn't work for most types because
  // type->name is a builtin whose span is null, and Reporter::Fail asserts the
  // span is not null. We need to get access to the TypeConstructor's span.
  CheckedMath checked(reporter(), type->name.span());
  switch (type->kind) {
    case Type::Kind::kArray: {
      auto array = static_cast<const ArrayType*>(type);
      auto count = array->element_count->value;
      auto element = Compile(array->element_type);
      return TypeShape{
          .inline_size = checked.Mul(count, element.inline_size),
          .alignment = element.alignment,
          .depth = element.depth,
          .max_handles = MulSat(count, element.max_handles),
          .max_out_of_line = MulSat(count, element.max_out_of_line),
          .has_padding = element.has_padding,
          .has_flexible_envelope = element.has_flexible_envelope,
      };
    }
    case Type::Kind::kVector: {
      auto vector = static_cast<const VectorType*>(type);
      auto count = vector->ElementCount();
      auto element = Compile(vector->element_type);
      auto max_array_size = MulSat(count, element.inline_size);
      return TypeShape{
          .inline_size = 16,
          .alignment = 8,
          .depth = AddSat(element.depth, 1),
          .max_handles = MulSat(count, element.max_handles),
          .max_out_of_line = AddSat(AddSat(max_array_size, ObjectPadding(max_array_size)),
                                    MulSat(count, element.max_out_of_line)),
          // Vector items are packed to natural alignment, not object alignment;
          // but since the count is only an upper bound, the only way for the
          // vector to never have padding is if elements are 8-byte aligned.
          .has_padding = element.has_padding || ObjectPadding(element.inline_size) != 0,
          .has_flexible_envelope = element.has_flexible_envelope,
      };
    }
    case Type::Kind::kString: {
      auto string = static_cast<const StringType*>(type);
      auto max_array_size = string->MaxSize();
      return TypeShape{
          .inline_size = 16,
          .alignment = 8,
          .depth = 1,
          .max_handles = 0,
          .max_out_of_line = AddSat(max_array_size, ObjectPadding(max_array_size)),
          .has_padding = true,
      };
    }
    case Type::Kind::kBox: {
      auto box = static_cast<const BoxType*>(type);
      auto boxed = Compile(box->boxed_type);
      auto padding = ObjectPadding(boxed.inline_size);
      return TypeShape{
          .inline_size = 8,
          .alignment = 8,
          .depth = AddSat(boxed.depth, 1),
          .max_handles = boxed.max_handles,
          .max_out_of_line = AddSat(AddSat(boxed.inline_size, padding), boxed.max_out_of_line),
          .has_padding = boxed.has_padding || padding != 0,
          .has_flexible_envelope = boxed.has_flexible_envelope,
      };
    }
    case Type::Kind::kZxExperimentalPointer: {
      // Treat experimental_pointer<T> like uintptr_t. We could instead treat it
      // like box<T>, but that would be meaningless because syscalls do not use
      // the FIDL wire format and there is no concept of "out of line".
      return TypeShape{.inline_size = 8, .alignment = 8};
    }
    case Type::Kind::kHandle:
    case Type::Kind::kTransportSide: {
      return TypeShape{.inline_size = 4, .alignment = 4, .max_handles = 1};
    }
    case Type::Kind::kPrimitive: {
      auto primitive = static_cast<const PrimitiveType*>(type);
      auto size = PrimitiveType::SubtypeSize(primitive->subtype);
      return TypeShape{.inline_size = size, .alignment = size};
    }
    case Type::Kind::kInternal: {
      auto internal = static_cast<const InternalType*>(type);
      switch (internal->subtype) {
        case InternalSubtype::kFrameworkErr:
          return TypeShape{.inline_size = 4, .alignment = 4};
      }
    }
    case Type::Kind::kIdentifier: {
      auto decl = static_cast<const IdentifierType*>(type)->type_decl;
      return Compile(decl);
    }
    case Type::Kind::kUntypedNumeric:
      ZX_PANIC("untyped numerics should be resolved before this");
  }
}

TypeShape TypeShapeStep::Compile(TypeDecl* decl, bool is_recursive_call) {
  return CompileImpl(decl, is_recursive_call);
}

TypeShape TypeShapeStep::Compile(Type* type, bool is_recursive_call) {
  return CompileImpl(type, is_recursive_call);
}

template <typename TypeDeclOrType>
TypeShape TypeShapeStep::CompileImpl(TypeDeclOrType* target, bool is_recursive_call) {
  static_assert(std::is_same_v<TypeDeclOrType, TypeDecl> || std::is_same_v<TypeDeclOrType, Type>);
  if (target->type_shape.has_value())
    return target->type_shape.value();
  // If we're in a cycle, make depth and max_out_of_line unbounded. Leave the
  // other fields at their default (identity) values. They'll be wrong, but
  // that's ok because we don't store the intermediate typeshapes -- see below.
  if (target->type_shape_compiling)
    return {.depth = UINT32_MAX, .max_out_of_line = UINT32_MAX};
  target->type_shape_compiling = true;
  auto result = Calculate(target);
  target->type_shape_compiling = false;
  // TODO(https://fxbug.dev/323940291): This is too conservative. Just because
  // we can reach a handle and a cycle doesn't mean the handle is in the cycle.
  if (result.depth == UINT32_MAX && result.max_handles != 0)
    result.max_handles = UINT32_MAX;
  // Store the result to avoid redundant calculations. But if there are cycles,
  // fall back to the non-memoized algorithm to ensure each final type shape is
  // influenced by everything reachable from it.
  if (!(is_recursive_call && result.depth == UINT32_MAX))
    target->type_shape = result;
  return result;
}

void TypeShapeStep::RunImpl() {
  auto& decls = library()->declarations;
  for (auto& decl : decls.bits)
    Compile(decl.get(), false);
  for (auto& decl : decls.enums)
    Compile(decl.get(), false);
  for (auto& decl : decls.new_types)
    Compile(decl.get(), false);
  for (auto& decl : decls.structs)
    Compile(decl.get(), false);
  for (auto& decl : decls.tables)
    Compile(decl.get(), false);
  for (auto& decl : decls.unions)
    Compile(decl.get(), false);
  for (auto& decl : decls.overlays)
    Compile(decl.get(), false);
  for (auto& type : created_types())
    Compile(type.get(), false);
}

}  // namespace fidlc
