// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/expr/resolve_base.h"

#include "src/developer/debug/shared/string_util.h"
#include "src/developer/debug/zxdb/common/ref_ptr_to.h"
#include "src/developer/debug/zxdb/expr/cast.h"
#include "src/developer/debug/zxdb/expr/eval_context.h"
#include "src/developer/debug/zxdb/expr/expr_language.h"
#include "src/developer/debug/zxdb/expr/expr_parser.h"
#include "src/developer/debug/zxdb/expr/expr_value.h"
#include "src/developer/debug/zxdb/expr/find_name.h"
#include "src/developer/debug/zxdb/expr/resolve_collection.h"
#include "src/developer/debug/zxdb/expr/resolve_ptr_ref.h"
#include "src/developer/debug/zxdb/expr/resolve_type.h"
#include "src/developer/debug/zxdb/symbols/arch.h"
#include "src/developer/debug/zxdb/symbols/collection.h"
#include "src/developer/debug/zxdb/symbols/data_member.h"
#include "src/developer/debug/zxdb/symbols/elf_symbol.h"
#include "src/developer/debug/zxdb/symbols/lazy_symbol.h"
#include "src/developer/debug/zxdb/symbols/location.h"
#include "src/developer/debug/zxdb/symbols/modified_type.h"
#include "src/developer/debug/zxdb/symbols/symbol.h"
#include "src/developer/debug/zxdb/symbols/symbol_utils.h"
#include "src/developer/debug/zxdb/symbols/template_parameter.h"

namespace zxdb {

namespace {

// When a class has a vtable, the pointer to the vtable is generated as a member of the class' data.
// This member is marked with DW_AT_artificial and named "_vptr.MyClass" by GCC and "_vptr$MyClass"
// by Clang, where "MyClass" is the name of the class. There is no scoping information on the name
// (namespaces, etc.).
const char kVtableMemberPrefix[] = "_vptr";

// The Clang demangler produces this prefix for vtable symbols.
const char kVtableSymbolNamePrefix[] = "vtable for ";

void PromoteRustDynPtrToDerived(const fxl::RefPtr<EvalContext>& context, ExprValue value,
                                EvalCallback cb) {
  // Rust dyn pointer is a struct that looks like
  //
  // 0x000957c3:   DW_TAG_structure_type
  //                 DW_AT_name	("*mut dyn core::future::future::Future<Output=()>")
  //                 DW_AT_byte_size	(0x10)
  //                 DW_AT_alignment	(8)
  //
  // 0x000957ca:     DW_TAG_member
  //                   DW_AT_name	("pointer")
  //                   DW_AT_type	(0x000957e1 "dyn core::future::future::Future<Output=()> *")
  //                   DW_AT_alignment	(8)
  //                   DW_AT_data_member_location	(0x00)
  //
  // 0x000957d5:     DW_TAG_member
  //                   DW_AT_name	("vtable")
  //                   DW_AT_type	(0x00094332 "usize (*)[3]")
  //                   DW_AT_alignment	(8)
  //                   DW_AT_data_member_location	(0x08)
  //
  // 0x000957e0:     NULL

  fxl::RefPtr<Collection> coll_type = context->GetConcreteTypeAs<Collection>(value.type());
  if (!coll_type)
    return cb(std::move(value));

  // The name could be either "*mut dyn ..." for raw pointers, or "alloc::boxed::Box<dyn ...>" for
  // boxed pointers. "dyn ..." could be parenthesized as "(dyn ... + ...)".
  if (!debug::StringStartsWith(coll_type->GetAssignedName(), "*mut ") &&
      !debug::StringStartsWith(coll_type->GetAssignedName(), "alloc::boxed::Box<"))
    return cb(std::move(value));

  // Extract pointer and vtable from the fat pointer.
  ErrOrValue pointer_val = ResolveNonstaticMember(context, value, {"pointer"});
  ErrOrValue vtable_val = ResolveNonstaticMember(context, value, {"vtable"});
  TargetPointer vtable = 0;
  if (pointer_val.has_error() || vtable_val.has_error() ||
      vtable_val.value().PromoteTo64(&vtable).has_error())
    return cb(std::move(value));

  // Derive types from the vtable.
  //
  // One approach is to find the DW_TAG_variable at the address |vtable|, but we don't have such
  // index to resolve addresses to variables. Instead, we read the first member of the vtable which
  // is the address of |core::ptr::drop_in_place<DerivedType>(Arg1)|. Note that the Arg1 is usually
  // DerivedType* but could be different for boxes so we use the template parameter instead.

  // Read the memory directly to bypass type manipulations.
  context->GetDataProvider()->GetMemoryAsync(
      vtable, sizeof(TargetPointer),
      [cb = std::move(cb), value, context, pointer = pointer_val.value()](
          const Err& err, std::vector<uint8_t> data) mutable {
        if (err.has_error() || data.size() != sizeof(TargetPointer))
          return cb(std::move(value));

        // Assume the same endian.
        TargetPointer drop_in_place_addr = *reinterpret_cast<TargetPointer*>(data.data());
        Location loc = context->GetLocationForAddress(drop_in_place_addr);
        if (!loc.symbol())
          return cb(std::move(value));

        const Function* func = loc.symbol().Get()->As<Function>();
        if (!func || func->template_params().empty())
          return cb(std::move(value));

        // Get the templated parameter and create a ModifiedType to that type.
        LazySymbol pointed_to = func->template_params()[0].Get()->As<TemplateParameter>()->type();
        auto derived = fxl::MakeRefCounted<ModifiedType>(DwarfTag::kPointerType, pointed_to);
        return cb(ExprValue(derived, pointer.data(), pointer.source()));
      });
}

}  // namespace

// The code would be a little simpler if we just tried to dereference the pointer/reference and
// then check for the vtable member. But this will be called a lot when evaluating collections,
// usually won't match, and the dereference will require a slow memory fetch. By checking the
// pointed-to/referenced type first, we avoid this overhead.
void PromotePtrRefToDerived(const fxl::RefPtr<EvalContext>& context, PromoteToDerived what,
                            ExprValue value, EvalCallback cb) {
  // Errors in this function should issue the callback with the original value. Errors mean that
  // promotion has failed, but we can still handle the original base class pointer.
  if (!value.type())
    return cb(std::move(value));

  // Rust uses a fat pointer (which is a struct type) that encodes vtable directly.
  if (context->GetLanguage() == ExprLanguage::kRust) {
    return PromoteRustDynPtrToDerived(context, value, std::move(cb));
  }

  // Type must be the right match pointer or a reference.
  fxl::RefPtr<ModifiedType> mod_type = context->GetConcreteTypeAs<ModifiedType>(value.type());
  if (!mod_type)
    return cb(std::move(value));

  bool tag_match = false;
  switch (what) {
    case PromoteToDerived::kPtrOnly:
      tag_match = (mod_type->tag() == DwarfTag::kPointerType);
      break;
    case PromoteToDerived::kRefOnly:
      tag_match = DwarfTagIsEitherReference(mod_type->tag());
      break;
    case PromoteToDerived::kPtrOrRef:
      tag_match = DwarfTagIsPointerOrReference(mod_type->tag());
      break;
  }
  if (!tag_match)
    return cb(std::move(value));

  // Referenced type must be a collection. Save the original non-concrete type for below.
  const Type* original_type = mod_type->modified().Get()->As<Type>();
  if (!original_type)
    return cb(std::move(value));
  fxl::RefPtr<Collection> modified_collection =
      context->GetConcreteTypeAs<Collection>(original_type);
  if (!modified_collection)
    return cb(std::move(value));

  // Referenced collection must have a vtable pointer.
  fxl::RefPtr<DataMember> vtable_member = GetVtableMember(modified_collection.get());
  if (!vtable_member)
    return cb(std::move(value));

  // Type is a pointer or reference to a virtual type. Get the vtable pointer value to see where it
  // goes.
  TargetPointer object_loc = 0;
  if (value.PromoteTo64(&object_loc).has_error())
    return cb(std::move(value));

  // Get the value of the vtable member. We user the original (non-concrete) type so the resulting
  // type is correct, with all C-V qualifiers.
  TargetPointer vtable_member_loc = object_loc + vtable_member->member_location();
  ResolvePointer(
      context, vtable_member_loc, RefPtrTo(vtable_member->type().Get()->As<Type>()),
      [context, original_value = std::move(value), modifier_tag = mod_type->tag(),
       modified_type = RefPtrTo(original_type), cb = std::move(cb)](ErrOrValue result) mutable {
        if (result.has_error())
          return cb(std::move(original_value));

        TargetPointer vtable = 0;
        if (result.value().PromoteTo64(&vtable).has_error())
          return cb(std::move(original_value));

        fxl::RefPtr<Type> derived_type = DerivedTypeForVtable(context, vtable);
        if (!derived_type)
          return cb(std::move(original_value));

        // Cast to the desired destination type. It should have the same type pattern as the
        // original: [ <C-V qualifier> ] + <pointer or reference> + [ <C-V qualifier> ] We did two
        // GetConcreteType() calls on each side of the ptr/ref and those stripped qualifiers need to
        // be put back.
        //
        // This code isn't perfect and will get confused if there are typedefs. Copying the C-V
        // qualifier will stop at typedefs, but the typedef could expand to something with a
        // qualifier like "const Foo" and this code would miss it. This gets very complicated and
        // the debugger doesn't actually follow qualifiers. This seems good enough for now.
        auto dest_type = AddCVQualifiersToMatch(modified_type.get(), std::move(derived_type));
        dest_type = fxl::MakeRefCounted<ModifiedType>(modifier_tag, std::move(dest_type));
        dest_type = AddCVQualifiersToMatch(original_value.type(), std::move(dest_type));
        CastExprValue(context, CastType::kStatic, original_value, dest_type, ExprValueSource(),
                      [original_value, cb = std::move(cb)](ErrOrValue value) mutable {
                        // Discard casting errors to just use the original value.
                        if (value.has_error())
                          cb(std::move(original_value));
                        else
                          cb(value.value());
                      });
      });
}

fxl::RefPtr<DataMember> GetVtableMember(const Collection* coll) {
  for (const auto& lazy_member : coll->data_members()) {
    const DataMember* member = lazy_member.Get()->As<DataMember>();
    if (!member)
      continue;

    if (member->artificial() &&
        debug::StringStartsWith(member->GetAssignedName(), kVtableMemberPrefix))
      return RefPtrTo(member);
  }
  return fxl::RefPtr<DataMember>();
}

std::string TypeNameForVtableSymbolName(const std::string& sym_name) {
  if (!debug::StringStartsWith(sym_name, kVtableSymbolNamePrefix))
    return std::string();
  return sym_name.substr(std::size(kVtableSymbolNamePrefix) - 1);  // Trim the prefix w/o the null.
}

fxl::RefPtr<Type> DerivedTypeForVtable(const fxl::RefPtr<EvalContext>& context, TargetPointer ptr) {
  Location loc = context->GetLocationForAddress(ptr);
  if (!loc.symbol())
    return nullptr;

  // Expect vtable symbols to be ELF ones. There won't be DWARF entries since they don't appear in
  // the program.
  const ElfSymbol* elf_symbol = loc.symbol().Get()->As<ElfSymbol>();
  if (!elf_symbol)
    return nullptr;

  std::string type_name = TypeNameForVtableSymbolName(elf_symbol->GetAssignedName());
  if (type_name.empty())
    return nullptr;  // Not a vtable entry.

  ParsedIdentifier ident;
  if (ExprParser::ParseIdentifier(type_name, &ident).has_error())
    return nullptr;  // Type name not parseable.

  return FindTypeDefinition(context->GetFindNameContext(), std::move(ident));
}

}  // namespace zxdb
