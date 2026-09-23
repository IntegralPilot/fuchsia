// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/expr/resolve_type.h"

#include <gtest/gtest.h>

#include "src/developer/debug/zxdb/expr/find_name.h"
#include "src/developer/debug/zxdb/symbols/collection.h"
#include "src/developer/debug/zxdb/symbols/dwarf_die_ref.h"
#include "src/developer/debug/zxdb/symbols/dwarf_tag.h"
#include "src/developer/debug/zxdb/symbols/index_test_support.h"
#include "src/developer/debug/zxdb/symbols/mock_symbol_factory.h"
#include "src/developer/debug/zxdb/symbols/modified_type.h"
#include "src/developer/debug/zxdb/symbols/process_symbols_test_setup.h"
#include "src/developer/debug/zxdb/symbols/type_test_support.h"

namespace zxdb {

// Also tests ResolveForwardDefinition().
TEST(ResolveType, GetConcreteType) {
  ProcessSymbolsTestSetup setup;
  MockModuleSymbols* module_symbols = setup.InjectMockModule();
  SymbolContext symbol_context(ProcessSymbolsTestSetup::kDefaultLoadAddress);

  const char kMyStructName[] = "MyStruct";

  // Make a forward declaration. It has the declaration flag set and no members or size.
  auto forward_decl = fxl::MakeRefCounted<Collection>(DwarfTag::kStructureType);
  forward_decl->set_assigned_name(kMyStructName);
  forward_decl->set_is_declaration(true);

  // A const modification of the forward declaration.
  auto const_forward_decl = fxl::MakeRefCounted<ModifiedType>(DwarfTag::kConstType, forward_decl);

  FindNameContext find_name_context(&setup.process(), symbol_context);

  // Resolving the const forward-defined value gives the non-const version.
  auto result_type = GetConcreteType(find_name_context, const_forward_decl.get());
  EXPECT_EQ(forward_decl.get(), result_type.get());

  // Make a definition for the type. It has one 32-bit data member.
  auto def = MakeCollectionType(DwarfTag::kStructureType, kMyStructName, {{"a", MakeInt32Type()}});

  // Index the declaration of the type.
  TestIndexedSymbol indexed_def(module_symbols, &module_symbols->index().root(), kMyStructName,
                                def);

  // Now that the index exists for the type, both the const and non-const declarations should
  // resolve to the full definition.
  result_type = GetConcreteType(find_name_context, forward_decl.get());
  EXPECT_EQ(def.get(), result_type.get());
  result_type = GetConcreteType(find_name_context, const_forward_decl.get());
  EXPECT_EQ(def.get(), result_type.get());
}

// Given this code:
//   struct Foo;
//   typedef Foo Foo;
// that never provides a concrete definition of Foo:
//
// The struct forward declaration defines a DWARF structure type declaration. When we look that
// up we can find the typedef which in turn references the structure. This test ensures that
// GetConcreteType handles this cycle.
TEST(ResolveType, TypedefCycle) {
  ProcessSymbolsTestSetup setup;
  MockModuleSymbols* module_symbols = setup.InjectMockModule();
  SymbolContext symbol_context(ProcessSymbolsTestSetup::kDefaultLoadAddress);
  MockSymbolFactory factory;

  const char kStructName[] = "Foo";

  // Struct forward declaration.
  auto forward_decl = fxl::MakeRefCounted<Collection>(DwarfTag::kStructureType);
  forward_decl->set_assigned_name(kStructName);
  forward_decl->set_is_declaration(true);
  // We need to set the lazy "this" member on the symbol so it has a valid DIE offset which is used
  // for cycle checking.
  forward_decl->set_lazy_this(UncachedLazySymbol(factory.factory_ref(), DwarfDieRef::Main(1234)));

  // Create the typedef and index it. Also needs a unique DIE offset.
  auto typedef_decl = fxl::MakeRefCounted<ModifiedType>(DwarfTag::kTypedef, forward_decl);
  typedef_decl->set_assigned_name(kStructName);
  typedef_decl->set_lazy_this(UncachedLazySymbol(factory.factory_ref(), DwarfDieRef::Main(5678)));
  TestIndexedSymbol indexed_typedef(module_symbols, &module_symbols->index().root(), kStructName,
                                    typedef_decl);

  FindNameContext find_name_context(&setup.process(), symbol_context);
  auto result_type = GetConcreteType(find_name_context, forward_decl.get());

  // The specific answer is not very important. Since neither the typedef nor the struct have a
  // real definition, either is plausibly valid for the return value for GetConcreteType(). The
  // important thing is that one of them is returned and the above code doesn't infinitely loop.
  EXPECT_TRUE(result_type.get() == forward_decl.get() || result_type.get() == typedef_decl.get());
}

}  // namespace zxdb
