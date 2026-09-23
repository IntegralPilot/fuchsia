// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/symbols/index_node.h"

#include <gtest/gtest.h>

#include "llvm/DebugInfo/DWARF/DWARFDie.h"
#include "src/developer/debug/zxdb/symbols/mock_symbol_factory.h"

namespace zxdb {

using SymbolRef = IndexNode::SymbolRef;
using Kind = IndexNode::Kind;

// Tests de-duplicating type definitions, and upgrading forward declarations to full definitions.
TEST(IndexNode, DeDupeType) {
  IndexNode node(Kind::kType);

  // Type forward declaration should get appended.
  constexpr DwarfDieRef kFwdDecl1Ref = DwarfDieRef::Main(20);
  node.AddDie(SymbolRef(IndexNode::SymbolRef::kDwarfDeclaration, IndexNode::SymbolRef::kMainBinary,
                        kFwdDecl1Ref));
  ASSERT_EQ(1u, node.dies().size());
  EXPECT_EQ(kFwdDecl1Ref, node.dies()[0].die_ref());

  // Another forward declaration should be ignored in favor of the old one.
  constexpr DwarfDieRef kFwdDecl2Ref = DwarfDieRef::Main(30);
  node.AddDie(SymbolRef(IndexNode::SymbolRef::kDwarfDeclaration, IndexNode::SymbolRef::kMainBinary,
                        kFwdDecl2Ref));
  ASSERT_EQ(1u, node.dies().size());
  EXPECT_EQ(kFwdDecl1Ref, node.dies()[0].die_ref());

  // A full type definition should overwrite the forward declaration.
  constexpr DwarfDieRef kType1Ref = DwarfDieRef::Main(40);
  node.AddDie(
      SymbolRef(IndexNode::SymbolRef::kDwarf, IndexNode::SymbolRef::kMainBinary, kType1Ref));
  ASSERT_EQ(1u, node.dies().size());
  EXPECT_EQ(kType1Ref, node.dies()[0].die_ref());

  // A duplicate full type definition should be ignored in favor of the old one.
  constexpr DwarfDieRef kType2Ref = DwarfDieRef::Main(50);
  node.AddDie(
      SymbolRef(IndexNode::SymbolRef::kDwarf, IndexNode::SymbolRef::kMainBinary, kType2Ref));
  ASSERT_EQ(1u, node.dies().size());
  EXPECT_EQ(kType1Ref, node.dies()[0].die_ref());
}

TEST(IndexNode, DeDupeNamespace) {
  IndexNode root(Kind::kRoot);

  const char kName[] = "ns";

  // Add a namespace, it should be appended but no DIE stored (we don't bother storing DIEs for
  // namespaces).
  const uint32_t kNSOffset = 60;
  root.AddChild(Kind::kNamespace, kName,
                SymbolRef(IndexNode::SymbolRef::kDwarf, IndexNode::SymbolRef::kMainBinary,
                          DwarfDieRef::Main(kNSOffset)));
  ASSERT_EQ(1u, root.namespaces().size());
  EXPECT_TRUE(root.namespaces().begin()->second.dies().empty());

  // A duplicate namespace.
  root.AddChild(Kind::kNamespace, kName,
                SymbolRef(IndexNode::SymbolRef::kDwarf, IndexNode::SymbolRef::kMainBinary,
                          DwarfDieRef::Main(kNSOffset)));
  ASSERT_EQ(1u, root.namespaces().size());
  EXPECT_TRUE(root.namespaces().begin()->second.dies().empty());
}

TEST(IndexNode, MergeFrom) {
  IndexNode dest(Kind::kRoot);

  // Dest setup
  // ----------

  // Add a namespace.
  IndexNode* dest_ns =
      dest.AddChild(Kind::kNamespace, "ns",
                    SymbolRef(IndexNode::SymbolRef::kDwarf, IndexNode::SymbolRef::kMainBinary,
                              DwarfDieRef::Main(0x100)));

  // Type inside the namespace
  IndexNode* dest_ns_type =
      dest_ns->AddChild(Kind::kType, "MyClass",
                        SymbolRef(IndexNode::SymbolRef::kDwarf, IndexNode::SymbolRef::kMainBinary,
                                  DwarfDieRef::Main(0x200)));

  // Function inside the type.
  dest_ns_type->AddChild(Kind::kFunction, "Func",
                         SymbolRef(IndexNode::SymbolRef::kDwarf, IndexNode::SymbolRef::kMainBinary,
                                   DwarfDieRef::Main(0x300)));

  // Source setup
  // ------------

  // This will get merged into the dest.
  IndexNode src(Kind::kRoot);

  // Duplicate namespace as in dest.
  IndexNode* src_ns =
      src.AddChild(Kind::kNamespace, "ns",
                   SymbolRef(IndexNode::SymbolRef::kDwarf, IndexNode::SymbolRef::kMainBinary,
                             DwarfDieRef::Main(0x1100)));

  // Duplicate type as in dest.
  IndexNode* src_ns_type =
      src_ns->AddChild(Kind::kType, "MyClass",
                       SymbolRef(IndexNode::SymbolRef::kDwarf, IndexNode::SymbolRef::kMainBinary,
                                 DwarfDieRef::Main(0x1200)));

  // A new and a duplicate function in the type.
  src_ns_type->AddChild(Kind::kFunction, "Func",
                        SymbolRef(IndexNode::SymbolRef::kDwarf, IndexNode::SymbolRef::kMainBinary,
                                  DwarfDieRef::Main(0x1300)));
  src_ns_type->AddChild(Kind::kFunction, "Func2",
                        SymbolRef(IndexNode::SymbolRef::kDwarf, IndexNode::SymbolRef::kMainBinary,
                                  DwarfDieRef::Main(0x1400)));

  // New type and variable the toplevel.
  src.AddChild(Kind::kType, "NewType",
               SymbolRef(IndexNode::SymbolRef::kDwarf, IndexNode::SymbolRef::kMainBinary,
                         DwarfDieRef::Main(0x1500)));
  src.AddChild(Kind::kVar, "Var",
               SymbolRef(IndexNode::SymbolRef::kDwarf, IndexNode::SymbolRef::kMainBinary,
                         DwarfDieRef::Main(0x1600)));

  // Merging
  // -------

  dest.MergeFrom(src);

  MockSymbolFactory mock_factory;
  std::ostringstream out;
  dest.Dump(out, mock_factory.factory(), 0);

  const char kExpected[] = R"(  Namespaces:
    ns
      Types:
        MyClass: 0x200, 0x1200
          Functions:
            Func: 0x300, 0x1300
            Func2: 0x1400
  Types:
    NewType: 0x1500
  Variables:
    Var: 0x1600
)";
  EXPECT_EQ(kExpected, out.str());
}

}  // namespace zxdb
