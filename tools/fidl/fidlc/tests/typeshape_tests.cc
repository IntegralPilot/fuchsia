// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/src/type_shape.h"
#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

#define EXPECT_TYPE_SHAPE(decl, expected)     \
  {                                           \
    SCOPED_TRACE("EXPECT_TYPE_SHAPE failed"); \
    ExpectTypeShape((decl), (expected));      \
  }

void ExpectTypeShape(const TypeDecl* decl, TypeShape expected) {
  ASSERT_NE(decl, nullptr);
  auto& actual = decl->type_shape.value();
  EXPECT_EQ(expected.inline_size, actual.inline_size);
  EXPECT_EQ(expected.alignment, actual.alignment);
  EXPECT_EQ(expected.max_out_of_line, actual.max_out_of_line);
  EXPECT_EQ(expected.max_handles, actual.max_handles);
  EXPECT_EQ(expected.depth, actual.depth);
  EXPECT_EQ(expected.has_padding, actual.has_padding);
  EXPECT_EQ(expected.has_flexible_envelope, actual.has_flexible_envelope);
}

#define EXPECT_FIELD_SHAPES(decl, expected)     \
  {                                             \
    SCOPED_TRACE("EXPECT_FIELD_SHAPES failed"); \
    ExpectFieldShapes((decl), (expected));      \
  }

void ExpectFieldShapes(const Struct* decl, const std::vector<FieldShape>& expected) {
  ASSERT_NE(decl, nullptr);
  ASSERT_EQ(decl->members.size(), expected.size());
  for (size_t i = 0; i < expected.size(); i++) {
    auto& actual = decl->members[i].field_shape;
    EXPECT_EQ(expected[i].offset, actual.offset) << "struct member at index=" << i;
    EXPECT_EQ(expected[i].padding, actual.padding) << "struct member at index=" << i;
  }
}

TEST(TypeshapeTests, GoodEmptyStruct) {
  TestLibrary library(R"FIDL(
library example;

type Empty = struct {};
)FIDL");
  ASSERT_COMPILED(library);

  auto empty = library.LookupStruct("Empty");
  EXPECT_TYPE_SHAPE(empty, (TypeShape{
                               .inline_size = 1,
                               .alignment = 1,
                           }));
  EXPECT_FIELD_SHAPES(empty, (std::vector<FieldShape>{}));
}

TEST(TypeshapeTests, GoodEmptyStructWithinAnotherStruct) {
  TestLibrary library(R"FIDL(
library example;

type Empty = struct {};

// Size = 1 byte for |bool a|
//      + 1 byte for |Empty b|
//      + 2 bytes for |int16 c|
//      + 1 bytes for |Empty d|
//      + 3 bytes padding
//      + 4 bytes for |int32 e|
//      + 2 bytes for |int16 f|
//      + 1 byte for |Empty g|
//      + 1 byte for |Empty h|
//      = 16 bytes
//
// Alignment = 4 bytes stemming from largest member (int32).
//
type EmptyWithOtherThings = struct {
    a bool;
    // no padding
    b Empty;
    // no padding
    c int16;
    // no padding
    d Empty;
    // 3 bytes padding
    e int32;
    // no padding
    f int16;
    // no padding
    g Empty;
    // no padding
    h Empty;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto empty_with_other_things = library.LookupStruct("EmptyWithOtherThings");
  EXPECT_TYPE_SHAPE(empty_with_other_things, (TypeShape{
                                                 .inline_size = 16,
                                                 .alignment = 4,
                                                 .has_padding = true,
                                             }));
  EXPECT_FIELD_SHAPES(empty_with_other_things, (std::vector<FieldShape>{
                                                   {.offset = 0},                // bool a;
                                                   {.offset = 1},                // Empty b;
                                                   {.offset = 2},                // int16 c;
                                                   {.offset = 4, .padding = 3},  // Empty d;
                                                   {.offset = 8},                // int32 e;
                                                   {.offset = 12},               // int16 f;
                                                   {.offset = 14},               // Empty g;
                                                   {.offset = 15},               // Empty h;
                                               }));
}

TEST(TypeshapeTests, GoodSimpleNewTypes) {
  TestLibrary library(R"FIDL(
library example;

type BoolAndU32 = struct {
    b bool;
    u uint32;
};
type NewBoolAndU32 = BoolAndU32;

type BitsImplicit = strict bits {
    VALUE = 1;
};
type NewBitsImplicit = BitsImplicit;


type TableWithBoolAndU32 = table {
    1: b bool;
    2: u uint32;
};
type NewTableWithBoolAndU32 = TableWithBoolAndU32;

type BoolAndU64 = struct {
    b bool;
    u uint64;
};
type UnionOfThings = strict union {
    1: ob bool;
    2: bu BoolAndU64;
};
type NewUnionOfThings = UnionOfThings;
)FIDL");
  library.EnableFlag(ExperimentalFlag::kAllowNewTypes);
  ASSERT_COMPILED(library);

  auto new_bool_and_u32_struct = library.LookupNewType("NewBoolAndU32");
  EXPECT_TYPE_SHAPE(new_bool_and_u32_struct, (TypeShape{
                                                 .inline_size = 8,
                                                 .alignment = 4,
                                                 .has_padding = true,
                                             }));

  auto new_bits_implicit = library.LookupNewType("NewBitsImplicit");
  EXPECT_TYPE_SHAPE(new_bits_implicit, (TypeShape{
                                           .inline_size = 4,
                                           .alignment = 4,
                                       }));

  auto new_bool_and_u32_table = library.LookupNewType("NewTableWithBoolAndU32");
  EXPECT_TYPE_SHAPE(new_bool_and_u32_table, (TypeShape{
                                                .inline_size = 16,
                                                .alignment = 8,
                                                .depth = 2,
                                                .max_out_of_line = 16,
                                                .has_padding = true,
                                                .has_flexible_envelope = true,
                                            }));

  auto new_union = library.LookupNewType("NewUnionOfThings");
  EXPECT_TYPE_SHAPE(new_union, (TypeShape{
                                   .inline_size = 16,
                                   .alignment = 8,
                                   .depth = 1,
                                   .max_out_of_line = 16,
                                   .has_padding = true,
                               }));
}

TEST(TypeshapeTests, GoodSimpleStructs) {
  TestLibrary library(R"FIDL(
library example;

type OneBool = struct {
    b bool;
};

type TwoBools = struct {
    a bool;
    b bool;
};

type BoolAndU32 = struct {
    b bool;
    u uint32;
};

type BoolAndU64 = struct {
    b bool;
    u uint64;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto one_bool = library.LookupStruct("OneBool");
  EXPECT_TYPE_SHAPE(one_bool, (TypeShape{
                                  .inline_size = 1,
                                  .alignment = 1,
                              }));
  EXPECT_FIELD_SHAPES(one_bool, (std::vector<FieldShape>{{.offset = 0}}));

  auto two_bools = library.LookupStruct("TwoBools");
  EXPECT_TYPE_SHAPE(two_bools, (TypeShape{
                                   .inline_size = 2,
                                   .alignment = 1,
                               }));
  EXPECT_FIELD_SHAPES(two_bools, (std::vector<FieldShape>{
                                     {.offset = 0},
                                     {.offset = 1},
                                 }));

  auto bool_and_u32 = library.LookupStruct("BoolAndU32");
  EXPECT_TYPE_SHAPE(bool_and_u32, (TypeShape{
                                      .inline_size = 8,
                                      .alignment = 4,
                                      .has_padding = true,
                                  }));
  EXPECT_FIELD_SHAPES(bool_and_u32, (std::vector<FieldShape>{
                                        {.offset = 0, .padding = 3},
                                        {.offset = 4},
                                    }));

  auto bool_and_u64 = library.LookupStruct("BoolAndU64");
  EXPECT_TYPE_SHAPE(bool_and_u64, (TypeShape{
                                      .inline_size = 16,
                                      .alignment = 8,
                                      .has_padding = true,
                                  }));
  EXPECT_FIELD_SHAPES(bool_and_u64, (std::vector<FieldShape>{
                                        {.offset = 0, .padding = 7},
                                        {.offset = 8},
                                    }));
}

TEST(TypeshapeTests, GoodSimpleStructsWithHandles) {
  TestLibrary library(R"FIDL(
library example;
using zx;

type OneHandle = resource struct {
  h zx.Handle;
};

type TwoHandles = resource struct {
  h1 zx.Handle:CHANNEL;
  h2 zx.Handle:PORT;
};

type ThreeHandlesOneOptional = resource struct {
  h1 zx.Handle:CHANNEL;
  h2 zx.Handle:PORT;
  opt_h3 zx.Handle:<VMO, optional>;
};
)FIDL");
  library.UseLibraryZx();
  ASSERT_COMPILED(library);

  auto one_handle = library.LookupStruct("OneHandle");
  EXPECT_TYPE_SHAPE(one_handle, (TypeShape{
                                    .inline_size = 4,
                                    .alignment = 4,
                                    .max_handles = 1,
                                }));
  EXPECT_FIELD_SHAPES(one_handle, (std::vector<FieldShape>{{.offset = 0}}));

  auto two_handles = library.LookupStruct("TwoHandles");
  EXPECT_TYPE_SHAPE(two_handles, (TypeShape{
                                     .inline_size = 8,
                                     .alignment = 4,
                                     .max_handles = 2,
                                 }));
  EXPECT_FIELD_SHAPES(two_handles, (std::vector<FieldShape>{
                                       {.offset = 0},
                                       {.offset = 4},
                                   }));

  auto three_handles_one_optional = library.LookupStruct("ThreeHandlesOneOptional");
  EXPECT_TYPE_SHAPE(three_handles_one_optional, (TypeShape{
                                                    .inline_size = 12,
                                                    .alignment = 4,
                                                    .max_handles = 3,
                                                }));
  EXPECT_FIELD_SHAPES(three_handles_one_optional, (std::vector<FieldShape>{
                                                      {.offset = 0},
                                                      {.offset = 4},
                                                      {.offset = 8},
                                                  }));
}

TEST(TypeshapeTests, GoodBits) {
  TestLibrary library(R"FIDL(
library example;

type Bits16 = strict bits : uint16 {
    VALUE = 1;
};

type BitsImplicit = strict bits {
    VALUE = 1;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto bits16 = library.LookupBits("Bits16");
  EXPECT_TYPE_SHAPE(bits16, (TypeShape{
                                .inline_size = 2,
                                .alignment = 2,
                            }));

  auto bits_implicit = library.LookupBits("BitsImplicit");
  EXPECT_TYPE_SHAPE(bits_implicit, (TypeShape{
                                       .inline_size = 4,
                                       .alignment = 4,
                                   }));
}

TEST(TypeshapeTests, GoodSimpleTables) {
  TestLibrary library(R"FIDL(
library example;

type TableWithNoMembers = table {};

type TableWithOneBool = table {
    1: b bool;
};

type TableWithTwoBools = table {
    1: a bool;
    2: b bool;
};

type TableWithBoolAndU32 = table {
    1: b bool;
    2: u uint32;
};

type TableWithBoolAndU64 = table {
    1: b bool;
    2: u uint64;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto no_members = library.LookupTable("TableWithNoMembers");
  EXPECT_TYPE_SHAPE(no_members, (TypeShape{
                                    .inline_size = 16,
                                    .alignment = 8,
                                    .depth = 1,
                                    .has_flexible_envelope = true,
                                }));

  auto one_bool = library.LookupTable("TableWithOneBool");
  EXPECT_TYPE_SHAPE(one_bool, (TypeShape{
                                  .inline_size = 16,
                                  .alignment = 8,
                                  .depth = 2,
                                  .max_out_of_line = 8,
                                  .has_padding = true,
                                  .has_flexible_envelope = true,
                              }));

  auto two_bools = library.LookupTable("TableWithTwoBools");
  EXPECT_TYPE_SHAPE(two_bools, (TypeShape{
                                   .inline_size = 16,
                                   .alignment = 8,
                                   .depth = 2,
                                   .max_out_of_line = 16,
                                   .has_padding = true,
                                   .has_flexible_envelope = true,
                               }));

  auto bool_and_u32 = library.LookupTable("TableWithBoolAndU32");
  EXPECT_TYPE_SHAPE(bool_and_u32, (TypeShape{
                                      .inline_size = 16,
                                      .alignment = 8,
                                      .depth = 2,
                                      .max_out_of_line = 16,
                                      .has_padding = true,
                                      .has_flexible_envelope = true,
                                  }));

  auto bool_and_u64 = library.LookupTable("TableWithBoolAndU64");
  EXPECT_TYPE_SHAPE(bool_and_u64, (TypeShape{
                                      .inline_size = 16,
                                      .alignment = 8,
                                      .depth = 2,
                                      .max_out_of_line = 24,
                                      .has_padding = true,
                                      .has_flexible_envelope = true,
                                  }));
}

TEST(TypeshapeTests, GoodTablesWithOrdinalGaps) {
  TestLibrary library(R"FIDL(
library example;

type GapInMiddle = table {
    1: b bool;
    3: b2 bool;
};

type GapAtStart = table {
    3: b bool;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto some_reserved = library.LookupTable("GapInMiddle");
  EXPECT_TYPE_SHAPE(some_reserved, (TypeShape{
                                       .inline_size = 16,
                                       .alignment = 8,
                                       .depth = 2,
                                       .max_out_of_line = 24,
                                       .has_padding = true,
                                       .has_flexible_envelope = true,
                                   }));

  auto last_non_reserved = library.LookupTable("GapAtStart");
  EXPECT_TYPE_SHAPE(last_non_reserved, (TypeShape{
                                           .inline_size = 16,
                                           .alignment = 8,
                                           .depth = 2,
                                           .max_out_of_line = 24,
                                           .has_padding = true,
                                           .has_flexible_envelope = true,
                                       }));
}

TEST(TypeshapeTests, GoodSimpleTablesWithHandles) {
  TestLibrary library(R"FIDL(
library example;
using zx;

type TableWithOneHandle = resource table {
  1: h zx.Handle;
};
)FIDL");
  library.UseLibraryZx();
  ASSERT_COMPILED(library);

  auto one_handle = library.LookupTable("TableWithOneHandle");
  EXPECT_TYPE_SHAPE(one_handle, (TypeShape{
                                    .inline_size = 16,
                                    .alignment = 8,
                                    .depth = 2,
                                    .max_handles = 1,
                                    .max_out_of_line = 8,
                                    .has_flexible_envelope = true,
                                }));
}

TEST(TypeshapeTests, GoodOptionalStructs) {
  TestLibrary library(R"FIDL(
library example;

type OneBool = struct {
    b bool;
};

type OptionalOneBool = struct {
    s box<OneBool>;
};

type TwoBools = struct {
    a bool;
    b bool;
};

type OptionalTwoBools = struct {
    s box<TwoBools>;
};

type BoolAndU32 = struct {
    b bool;
    u uint32;
};

type OptionalBoolAndU32 = struct {
    s box<BoolAndU32>;
};

type BoolAndU64 = struct {
    b bool;
    u uint64;
};

type OptionalBoolAndU64 = struct {
    s box<BoolAndU64>;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto one_bool = library.LookupStruct("OptionalOneBool");
  EXPECT_TYPE_SHAPE(one_bool, (TypeShape{
                                  .inline_size = 8,
                                  .alignment = 8,
                                  .depth = 1,
                                  .max_out_of_line = 8,
                                  .has_padding = true,
                              }));

  auto two_bools = library.LookupStruct("OptionalTwoBools");
  EXPECT_TYPE_SHAPE(two_bools, (TypeShape{
                                   .inline_size = 8,
                                   .alignment = 8,
                                   .depth = 1,
                                   .max_out_of_line = 8,
                                   .has_padding = true,
                               }));

  auto bool_and_u32 = library.LookupStruct("OptionalBoolAndU32");
  EXPECT_TYPE_SHAPE(bool_and_u32, (TypeShape{
                                      .inline_size = 8,
                                      .alignment = 8,
                                      .depth = 1,
                                      .max_out_of_line = 8,
                                      .has_padding = true,  // because |BoolAndU32| has padding
                                  }));

  auto bool_and_u64 = library.LookupStruct("OptionalBoolAndU64");
  EXPECT_TYPE_SHAPE(bool_and_u64, (TypeShape{
                                      .inline_size = 8,
                                      .alignment = 8,
                                      .depth = 1,
                                      .max_out_of_line = 16,
                                      .has_padding = true,  // because |BoolAndU64| has padding
                                  }));
}

TEST(TypeshapeTests, GoodOptionalTables) {
  TestLibrary library(R"FIDL(
library example;

type OneBool = struct {
    b bool;
};

type TableWithOptionalOneBool = table {
    1: s OneBool;
};

type TableWithOneBool = table {
    1: b bool;
};

type TableWithOptionalTableWithOneBool = table {
    1: s TableWithOneBool;
};

type TwoBools = struct {
    a bool;
    b bool;
};

type TableWithOptionalTwoBools = table {
    1: s TwoBools;
};

type TableWithTwoBools = table {
    1: a bool;
    2: b bool;
};

type TableWithOptionalTableWithTwoBools = table {
    1: s TableWithTwoBools;
};

type BoolAndU32 = struct {
    b bool;
    u uint32;
};

type TableWithOptionalBoolAndU32 = table {
    1: s BoolAndU32;
};

type TableWithBoolAndU32 = table {
    1: b bool;
    2: u uint32;
};

type TableWithOptionalTableWithBoolAndU32 = table {
    1: s TableWithBoolAndU32;
};

type BoolAndU64 = struct {
    b bool;
    u uint64;
};

type TableWithOptionalBoolAndU64 = table {
    1: s BoolAndU64;
};

type TableWithBoolAndU64 = table {
    1: b bool;
    2: u uint64;
};

type TableWithOptionalTableWithBoolAndU64 = table {
    1: s TableWithBoolAndU64;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto one_bool = library.LookupTable("TableWithOptionalOneBool");
  EXPECT_TYPE_SHAPE(one_bool, (TypeShape{
                                  .inline_size = 16,
                                  .alignment = 8,
                                  .depth = 2,
                                  .max_out_of_line = 8,
                                  .has_padding = true,
                                  .has_flexible_envelope = true,
                              }));

  auto table_with_one_bool = library.LookupTable("TableWithOptionalTableWithOneBool");
  EXPECT_TYPE_SHAPE(table_with_one_bool, (TypeShape{
                                             .inline_size = 16,
                                             .alignment = 8,
                                             .depth = 4,
                                             .max_out_of_line = 32,
                                             .has_padding = true,
                                             .has_flexible_envelope = true,
                                         }));

  auto two_bools = library.LookupTable("TableWithOptionalTwoBools");
  EXPECT_TYPE_SHAPE(two_bools, (TypeShape{
                                   .inline_size = 16,
                                   .alignment = 8,
                                   .depth = 2,
                                   .max_out_of_line = 8,
                                   .has_padding = true,
                                   .has_flexible_envelope = true,
                               }));

  auto table_with_two_bools = library.LookupTable("TableWithOptionalTableWithTwoBools");
  EXPECT_TYPE_SHAPE(table_with_two_bools, (TypeShape{
                                              .inline_size = 16,
                                              .alignment = 8,
                                              .depth = 4,
                                              .max_out_of_line = 40,
                                              .has_padding = true,
                                              .has_flexible_envelope = true,
                                          }));

  auto bool_and_u32 = library.LookupTable("TableWithOptionalBoolAndU32");
  EXPECT_TYPE_SHAPE(bool_and_u32, (TypeShape{
                                      .inline_size = 16,
                                      .alignment = 8,
                                      .depth = 2,
                                      .max_out_of_line = 16,
                                      .has_padding = true,
                                      .has_flexible_envelope = true,
                                  }));

  auto table_with_bool_and_u32 = library.LookupTable("TableWithOptionalTableWithBoolAndU32");
  EXPECT_TYPE_SHAPE(table_with_bool_and_u32, (TypeShape{
                                                 .inline_size = 16,
                                                 .alignment = 8,
                                                 .depth = 4,
                                                 .max_out_of_line = 40,
                                                 .has_padding = true,
                                                 .has_flexible_envelope = true,
                                             }));

  auto bool_and_u64 = library.LookupTable("TableWithOptionalBoolAndU64");
  EXPECT_TYPE_SHAPE(bool_and_u64, (TypeShape{
                                      .inline_size = 16,
                                      .alignment = 8,
                                      .depth = 2,
                                      .max_out_of_line = 24,
                                      .has_padding = true,
                                      .has_flexible_envelope = true,
                                  }));

  auto table_with_bool_and_u64 = library.LookupTable("TableWithOptionalTableWithBoolAndU64");
  EXPECT_TYPE_SHAPE(table_with_bool_and_u64, (TypeShape{
                                                 .inline_size = 16,
                                                 .alignment = 8,
                                                 .depth = 4,
                                                 .max_out_of_line = 48,
                                                 .has_padding = true,
                                                 .has_flexible_envelope = true,
                                             }));
}

TEST(TypeshapeTests, GoodUnions) {
  TestLibrary library(R"FIDL(
library example;

type BoolAndU64 = struct {
    b bool;
    u uint64;
};

type UnionOfThings = strict union {
    1: ob bool;
    2: bu BoolAndU64;
};

type Bool = struct {
    b bool;
};

type OptBool = struct {
    opt_b box<Bool>;
};

type UnionWithOutOfLine = strict union {
    1: opt_bool OptBool;
};

type OptionalUnion = struct {
    u UnionOfThings:optional;
};

type TableWithOptionalUnion = table {
    1: u UnionOfThings;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto union_with_out_of_line = library.LookupUnion("UnionWithOutOfLine");
  EXPECT_TYPE_SHAPE(union_with_out_of_line, (TypeShape{
                                                .inline_size = 16,
                                                .alignment = 8,
                                                .depth = 2,
                                                .max_out_of_line = 16,
                                                .has_padding = true,
                                            }));

  auto a_union = library.LookupUnion("UnionOfThings");
  EXPECT_TYPE_SHAPE(a_union, (TypeShape{
                                 .inline_size = 16,
                                 .alignment = 8,
                                 .depth = 1,
                                 .max_out_of_line = 16,
                                 .has_padding = true,
                             }));

  auto optional_union = library.LookupStruct("OptionalUnion");
  EXPECT_TYPE_SHAPE(optional_union, (TypeShape{
                                        // because |UnionOfThings| union header is inline
                                        .inline_size = 16,
                                        .alignment = 8,
                                        .depth = 1,
                                        .max_out_of_line = 16,
                                        .has_padding = true,
                                    }));

  auto table_with_optional_union = library.LookupTable("TableWithOptionalUnion");
  EXPECT_TYPE_SHAPE(table_with_optional_union, (TypeShape{
                                                   .inline_size = 16,
                                                   .alignment = 8,
                                                   .depth = 3,
                                                   .max_out_of_line = 40,
                                                   .has_padding = true,
                                                   .has_flexible_envelope = true,
                                               }));
}

TEST(TypeshapeTests, GoodUnionsWithHandles) {
  TestLibrary library(R"FIDL(
library example;
using zx;

type OneHandleUnion = strict resource union {
  1: one_handle zx.Handle;
  2: one_bool bool;
  3: one_int uint32;
};

type ManyHandleUnion = strict resource union {
  1: one_handle zx.Handle;
  2: handle_array array<zx.Handle, 8>;
  3: handle_vector vector<zx.Handle>:8;
};
)FIDL");
  library.UseLibraryZx();
  ASSERT_COMPILED(library);

  auto one_handle_union = library.LookupUnion("OneHandleUnion");
  EXPECT_TYPE_SHAPE(one_handle_union, (TypeShape{
                                          .inline_size = 16,
                                          .alignment = 8,
                                          .depth = 1,
                                          .max_handles = 1,
                                          .has_padding = true,
                                      }));

  auto many_handle_union = library.LookupUnion("ManyHandleUnion");
  EXPECT_TYPE_SHAPE(many_handle_union, (TypeShape{
                                           .inline_size = 16,
                                           .alignment = 8,
                                           .depth = 2,
                                           .max_handles = 8,
                                           .max_out_of_line = 48,
                                           .has_padding = true,
                                       }));
}

TEST(TypeshapeTests, GoodOverlays) {
  TestLibrary library(R"FIDL(
library example;

type BoolOrStringOrU64 = strict overlay {
    1: b bool;
    2: s string:255;
    3: u uint64;
};

type BoolOverlay = strict overlay {
    1: b bool;
};

type U64BoolStruct = struct {
    u uint64;
    b bool;
};

type BoolOverlayStruct = struct {
    bo BoolOverlay;
};

type BoolOverlayAndUint8Struct = struct {
    bo BoolOverlay;
    x uint8;
};
)FIDL");
  library.EnableFlag(ExperimentalFlag::kZxCTypes);

  ASSERT_COMPILED(library);
  auto bool_or_string_or_u64 = library.LookupOverlay("BoolOrStringOrU64");
  EXPECT_TYPE_SHAPE(bool_or_string_or_u64, (TypeShape{
                                               .inline_size = 24,
                                               .alignment = 8,
                                               .depth = 1,
                                               .max_out_of_line = 256,
                                               .has_padding = true,
                                           }));

  // BoolOverlay and U64BoolStruct should have basically the same typeshape.
  auto bool_overlay = library.LookupOverlay("BoolOverlay");
  auto u64_bool_struct = library.LookupStruct("U64BoolStruct");
  EXPECT_TYPE_SHAPE(bool_overlay, (TypeShape{
                                      .inline_size = 16,
                                      .alignment = 8,
                                      .has_padding = true,
                                  }));
  EXPECT_TYPE_SHAPE(u64_bool_struct, (TypeShape{
                                         .inline_size = 16,
                                         .alignment = 8,
                                         .has_padding = true,
                                     }));

  auto bool_overlay_struct = library.LookupStruct("BoolOverlayStruct");
  EXPECT_FIELD_SHAPES(bool_overlay_struct, (std::vector<FieldShape>{{.offset = 0}}));

  auto bool_overlay_and_uint8_struct = library.LookupStruct("BoolOverlayAndUint8Struct");
  EXPECT_FIELD_SHAPES(bool_overlay_and_uint8_struct, (std::vector<FieldShape>{
                                                         {.offset = 0},
                                                         {.offset = 16, .padding = 7},
                                                     }));
}

TEST(TypeshapeTests, GoodVectors) {
  TestLibrary library(R"FIDL(
library example;

type PaddedVector = struct {
    pv vector<int32>:3;
};

type NoPaddingVector = struct {
    npv vector<uint64>:3;
};

type UnboundedVector = struct {
    uv vector<int32>;
};

type UnboundedVectors = struct {
    uv1 vector<int32>;
    uv2 vector<int32>;
};

type TableWithPaddedVector = table {
    1: pv vector<int32>:3;
};

type TableWithUnboundedVector = table {
    1: uv vector<int32>;
};

type TableWithUnboundedVectors = table {
    1: uv1 vector<int32>;
    2: uv2 vector<int32>;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto padded_vector = library.LookupStruct("PaddedVector");
  EXPECT_TYPE_SHAPE(padded_vector, (TypeShape{
                                       .inline_size = 16,
                                       .alignment = 8,
                                       .depth = 1,
                                       .max_out_of_line = 16,
                                       .has_padding = true,
                                   }));

  auto no_padding_vector = library.LookupStruct("NoPaddingVector");
  EXPECT_TYPE_SHAPE(no_padding_vector, (TypeShape{
                                           .inline_size = 16,
                                           .alignment = 8,
                                           .depth = 1,
                                           .max_out_of_line = 24,
                                       }));

  auto unbounded_vector = library.LookupStruct("UnboundedVector");
  EXPECT_TYPE_SHAPE(unbounded_vector, (TypeShape{
                                          .inline_size = 16,
                                          .alignment = 8,
                                          .depth = 1,
                                          .max_out_of_line = UINT32_MAX,
                                          .has_padding = true,
                                      }));

  auto unbounded_vectors = library.LookupStruct("UnboundedVectors");
  EXPECT_TYPE_SHAPE(unbounded_vectors, (TypeShape{
                                           .inline_size = 32,
                                           .alignment = 8,
                                           .depth = 1,
                                           .max_out_of_line = UINT32_MAX,
                                           .has_padding = true,
                                       }));

  auto table_with_padded_vector = library.LookupTable("TableWithPaddedVector");
  EXPECT_TYPE_SHAPE(table_with_padded_vector, (TypeShape{
                                                  .inline_size = 16,
                                                  .alignment = 8,
                                                  .depth = 3,
                                                  .max_out_of_line = 40,
                                                  .has_padding = true,
                                                  .has_flexible_envelope = true,
                                              }));

  auto table_with_unbounded_vector = library.LookupTable("TableWithUnboundedVector");
  EXPECT_TYPE_SHAPE(table_with_unbounded_vector, (TypeShape{
                                                     .inline_size = 16,
                                                     .alignment = 8,
                                                     .depth = 3,
                                                     .max_out_of_line = UINT32_MAX,
                                                     .has_padding = true,
                                                     .has_flexible_envelope = true,
                                                 }));

  auto table_with_unbounded_vectors = library.LookupTable("TableWithUnboundedVectors");
  EXPECT_TYPE_SHAPE(table_with_unbounded_vectors, (TypeShape{
                                                      .inline_size = 16,
                                                      .alignment = 8,
                                                      .depth = 3,
                                                      .max_out_of_line = UINT32_MAX,
                                                      .has_padding = true,
                                                      .has_flexible_envelope = true,
                                                  }));
}

TEST(TypeshapeTests, GoodVectorsWithHandles) {
  TestLibrary library(R"FIDL(
library example;
using zx;

type HandleVector = resource struct {
  hv vector<zx.Handle>:8;
};

type HandleNullableVector = resource struct {
  hv vector<zx.Handle>:<8, optional>;
};

type TableWithHandleVector = resource table {
  1: hv vector<zx.Handle>:8;
};

type UnboundedHandleVector = resource struct {
  hv vector<zx.Handle>;
};

type TableWithUnboundedHandleVector = resource table {
  1: hv vector<zx.Handle>;
};

type OneHandle = resource struct {
  h zx.Handle;
};

type HandleStructVector = resource struct {
  sv vector<OneHandle>:8;
};

type TableWithOneHandle = resource table {
  1: h zx.Handle;
};

type HandleTableVector = resource struct {
  sv vector<TableWithOneHandle>:8;
};

type TableWithHandleStructVector = resource table {
  1: sv vector<OneHandle>:8;
};
)FIDL");
  library.UseLibraryZx();
  ASSERT_COMPILED(library);

  auto handle_vector = library.LookupStruct("HandleVector");
  EXPECT_TYPE_SHAPE(handle_vector, (TypeShape{
                                       .inline_size = 16,
                                       .alignment = 8,
                                       .depth = 1,
                                       .max_handles = 8,
                                       .max_out_of_line = 32,
                                       .has_padding = true,
                                   }));

  auto handle_nullable_vector = library.LookupStruct("HandleNullableVector");
  EXPECT_TYPE_SHAPE(handle_nullable_vector, (TypeShape{
                                                .inline_size = 16,
                                                .alignment = 8,
                                                .depth = 1,
                                                .max_handles = 8,
                                                .max_out_of_line = 32,
                                                .has_padding = true,
                                            }));

  auto unbounded_handle_vector = library.LookupStruct("UnboundedHandleVector");
  EXPECT_TYPE_SHAPE(unbounded_handle_vector, (TypeShape{
                                                 .inline_size = 16,
                                                 .alignment = 8,
                                                 .depth = 1,
                                                 .max_handles = UINT32_MAX,
                                                 .max_out_of_line = UINT32_MAX,
                                                 .has_padding = true,
                                             }));

  auto table_with_unbounded_handle_vector = library.LookupTable("TableWithUnboundedHandleVector");
  EXPECT_TYPE_SHAPE(table_with_unbounded_handle_vector, (TypeShape{
                                                            .inline_size = 16,
                                                            .alignment = 8,
                                                            .depth = 3,
                                                            .max_handles = UINT32_MAX,
                                                            .max_out_of_line = UINT32_MAX,
                                                            .has_padding = true,
                                                            .has_flexible_envelope = true,
                                                        }));

  auto handle_struct_vector = library.LookupStruct("HandleStructVector");
  EXPECT_TYPE_SHAPE(handle_struct_vector, (TypeShape{
                                              .inline_size = 16,
                                              .alignment = 8,
                                              .depth = 1,
                                              .max_handles = 8,
                                              .max_out_of_line = 32,
                                              .has_padding = true,
                                          }));

  auto handle_table_vector = library.LookupStruct("HandleTableVector");
  EXPECT_TYPE_SHAPE(handle_table_vector, (TypeShape{
                                             .inline_size = 16,
                                             .alignment = 8,
                                             .depth = 3,
                                             .max_handles = 8,
                                             .max_out_of_line = 192,
                                             .has_flexible_envelope = true,
                                         }));

  auto table_with_handle_struct_vector = library.LookupTable("TableWithHandleStructVector");
  EXPECT_TYPE_SHAPE(table_with_handle_struct_vector, (TypeShape{
                                                         .inline_size = 16,
                                                         .alignment = 8,
                                                         .depth = 3,
                                                         .max_handles = 8,
                                                         .max_out_of_line = 56,
                                                         .has_padding = true,
                                                         .has_flexible_envelope = true,
                                                     }));
}

TEST(TypeshapeTests, GoodStrings) {
  TestLibrary library(R"FIDL(
library example;

type ShortString = struct {
    s string:5;
};

type UnboundedString = struct {
    s string;
};

type TableWithShortString = table {
    1: s string:5;
};

type TableWithUnboundedString = table {
    1: s string;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto short_string = library.LookupStruct("ShortString");
  EXPECT_TYPE_SHAPE(short_string, (TypeShape{
                                      .inline_size = 16,
                                      .alignment = 8,
                                      .depth = 1,
                                      .max_out_of_line = 8,
                                      .has_padding = true,
                                  }));

  auto unbounded_string = library.LookupStruct("UnboundedString");
  EXPECT_TYPE_SHAPE(unbounded_string, (TypeShape{
                                          .inline_size = 16,
                                          .alignment = 8,
                                          .depth = 1,
                                          .max_out_of_line = UINT32_MAX,
                                          .has_padding = true,
                                      }));

  auto table_with_short_string = library.LookupTable("TableWithShortString");
  EXPECT_TYPE_SHAPE(table_with_short_string, (TypeShape{
                                                 .inline_size = 16,
                                                 .alignment = 8,
                                                 .depth = 3,
                                                 .max_out_of_line = 32,
                                                 .has_padding = true,
                                                 .has_flexible_envelope = true,
                                             }));

  auto table_with_unbounded_string = library.LookupTable("TableWithUnboundedString");
  EXPECT_TYPE_SHAPE(table_with_unbounded_string, (TypeShape{
                                                     .inline_size = 16,
                                                     .alignment = 8,
                                                     .depth = 3,
                                                     .max_out_of_line = UINT32_MAX,
                                                     .has_padding = true,
                                                     .has_flexible_envelope = true,
                                                 }));
}

TEST(TypeshapeTests, GoodStringArrays) {
  TestLibrary library(R"FIDL(
library example;

type StringArray = struct {
    s string_array<5>;
};
)FIDL");
  library.EnableFlag(ExperimentalFlag::kZxCTypes);

  ASSERT_COMPILED(library);

  auto string_array = library.LookupStruct("StringArray");
  EXPECT_TYPE_SHAPE(string_array, (TypeShape{
                                      .inline_size = 5,
                                      .alignment = 1,
                                  }));
}

TEST(TypeshapeTests, GoodArrays) {
  TestLibrary library(R"FIDL(
library example;

type AnArray = struct {
    a array<int64, 5>;
};

type TableWithAnArray = table {
    1: a array<int64, 5>;
};

type TableWithAnInt32ArrayWithPadding = table {
    1: a array<int32, 3>;
};

type TableWithAnInt32ArrayNoPadding = table {
    1: a array<int32, 4>;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto an_array = library.LookupStruct("AnArray");
  EXPECT_TYPE_SHAPE(an_array, (TypeShape{
                                  .inline_size = 40,
                                  .alignment = 8,
                              }));

  auto table_with_an_array = library.LookupTable("TableWithAnArray");
  EXPECT_TYPE_SHAPE(table_with_an_array, (TypeShape{
                                             .inline_size = 16,
                                             .alignment = 8,
                                             .depth = 2,
                                             .max_out_of_line = 48,
                                             .has_flexible_envelope = true,
                                         }));

  auto table_with_an_int32_array_with_padding =
      library.LookupTable("TableWithAnInt32ArrayWithPadding");
  EXPECT_TYPE_SHAPE(table_with_an_int32_array_with_padding, (TypeShape{
                                                                .inline_size = 16,
                                                                .alignment = 8,
                                                                .depth = 2,
                                                                .max_out_of_line = 24,
                                                                .has_padding = true,
                                                                .has_flexible_envelope = true,
                                                            }));

  auto table_with_an_int32_array_no_padding = library.LookupTable("TableWithAnInt32ArrayNoPadding");
  EXPECT_TYPE_SHAPE(table_with_an_int32_array_no_padding, (TypeShape{
                                                              .inline_size = 16,
                                                              .alignment = 8,
                                                              .depth = 2,
                                                              .max_out_of_line = 24,
                                                              .has_flexible_envelope = true,
                                                          }));
}

TEST(TypeshapeTests, GoodArraysWithHandles) {
  TestLibrary library(R"FIDL(
library example;
using zx;

type HandleArray = resource struct {
  h1 array<zx.Handle, 8>;
};

type TableWithHandleArray = resource table {
  1: ha array<zx.Handle, 8>;
};

type NullableHandleArray = resource struct {
  ha array<zx.Handle:optional, 8>;
};

type TableWithNullableHandleArray = resource table {
  1: ha array<zx.Handle:optional, 8>;
};
)FIDL");
  library.UseLibraryZx();
  ASSERT_COMPILED(library);

  auto handle_array = library.LookupStruct("HandleArray");
  EXPECT_TYPE_SHAPE(handle_array, (TypeShape{
                                      .inline_size = 32,
                                      .alignment = 4,
                                      .max_handles = 8,
                                  }));

  auto table_with_handle_array = library.LookupTable("TableWithHandleArray");
  EXPECT_TYPE_SHAPE(table_with_handle_array, (TypeShape{
                                                 .inline_size = 16,
                                                 .alignment = 8,
                                                 .depth = 2,
                                                 .max_handles = 8,
                                                 .max_out_of_line = 40,
                                                 .has_flexible_envelope = true,
                                             }));

  auto nullable_handle_array = library.LookupStruct("NullableHandleArray");
  EXPECT_TYPE_SHAPE(nullable_handle_array, (TypeShape{
                                               .inline_size = 32,
                                               .alignment = 4,
                                               .max_handles = 8,
                                           }));

  auto table_with_nullable_handle_array = library.LookupTable("TableWithNullableHandleArray");
  EXPECT_TYPE_SHAPE(table_with_nullable_handle_array, (TypeShape{
                                                          .inline_size = 16,
                                                          .alignment = 8,
                                                          .depth = 2,
                                                          .max_handles = 8,
                                                          .max_out_of_line = 40,
                                                          .has_flexible_envelope = true,
                                                      }));
}

// TODO(https://fxbug.dev/42069446): write a "unions_with_handles" test case.

TEST(TypeshapeTests, GoodFlexibleUnions) {
  TestLibrary library(R"FIDL(
library example;

type UnionWithOneBool = flexible union {
    1: b bool;
};

type StructWithOptionalUnionWithOneBool = struct {
    opt_union_with_bool UnionWithOneBool:optional;
};

type UnionWithBoundedOutOfLineObject = flexible union {
    // smaller than |v| below, so will not be selected for max-out-of-line
    // calculation.
    1: b bool;

    // 1. vector<int32>:5 = 8 bytes for vector element count
    //                    + 8 bytes for data pointer
    //                    + 24 bytes out-of-line (20 bytes contents +
    //                                            4 bytes for 8-byte alignment)
    //                    = 40 bytes total
    // 1. vector<vector<int32>:5>:6 = vector of up to six of vector<int32>:5
    //                              = 8 bytes for vector element count
    //                              + 8 bytes for data pointer
    //                              + 240 bytes out-of-line (40 bytes contents * 6)
    //                              = 256 bytes total
    2: v vector<vector<int32>:5>:6;
};

type UnionWithUnboundedOutOfLineObject = flexible union {
    1: s string;
};

type UnionWithoutPayloadPadding = flexible union {
    1: a array<uint64, 7>;
};

type PaddingCheck = flexible union {
    1: three array<uint8, 3>;
    2: five array<uint8, 5>;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto one_bool = library.LookupUnion("UnionWithOneBool");
  EXPECT_TYPE_SHAPE(one_bool, (TypeShape{
                                  .inline_size = 16,
                                  .alignment = 8,
                                  .depth = 1,
                                  .has_padding = true,
                                  .has_flexible_envelope = true,
                              }));

  auto opt_one_bool = library.LookupStruct("StructWithOptionalUnionWithOneBool");
  EXPECT_TYPE_SHAPE(opt_one_bool, (TypeShape{
                                      .inline_size = 16,
                                      .alignment = 8,
                                      .depth = 1,
                                      .has_padding = true,
                                      .has_flexible_envelope = true,
                                  }));

  auto xu = library.LookupUnion("UnionWithBoundedOutOfLineObject");
  EXPECT_TYPE_SHAPE(xu, (TypeShape{
                            .inline_size = 16,
                            .alignment = 8,
                            .depth = 3,
                            .max_out_of_line = 256,
                            .has_padding = true,
                            .has_flexible_envelope = true,
                        }));

  auto unbounded = library.LookupUnion("UnionWithUnboundedOutOfLineObject");
  EXPECT_TYPE_SHAPE(unbounded, (TypeShape{
                                   .inline_size = 16,
                                   .alignment = 8,
                                   .depth = 2,
                                   .max_out_of_line = UINT32_MAX,
                                   .has_padding = true,
                                   .has_flexible_envelope = true,
                               }));

  auto xu_no_payload_padding = library.LookupUnion("UnionWithoutPayloadPadding");
  EXPECT_TYPE_SHAPE(xu_no_payload_padding, (TypeShape{
                                               .inline_size = 16,
                                               .alignment = 8,
                                               .depth = 1,
                                               .max_out_of_line = 56,
                                               .has_flexible_envelope = true,
                                           }));

  auto padding_check = library.LookupUnion("PaddingCheck");
  EXPECT_TYPE_SHAPE(padding_check, (TypeShape{
                                       .inline_size = 16,
                                       .alignment = 8,
                                       .depth = 1,
                                       .max_out_of_line = 8,
                                       .has_padding = true,
                                       .has_flexible_envelope = true,
                                   }));
}

TEST(TypeshapeTests, GoodEnvelopeStrictness) {
  TestLibrary library(R"FIDL(
library example;

type StrictLeafUnion = strict union {
    1: a int64;
};

type FlexibleLeafUnion = flexible union {
    1: a int64;
};

type FlexibleUnionOfStrictUnion = flexible union {
    1: xu StrictLeafUnion;
};

type FlexibleUnionOfFlexibleUnion = flexible union {
    1: xu FlexibleLeafUnion;
};

type StrictUnionOfStrictUnion = strict union {
    1: xu StrictLeafUnion;
};

type StrictUnionOfFlexibleUnion = strict union {
    1: xu FlexibleLeafUnion;
};

type FlexibleLeafTable = table {};

type StrictUnionOfFlexibleTable = strict union {
    1: ft FlexibleLeafTable;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto strict_union = library.LookupUnion("StrictLeafUnion");
  EXPECT_TYPE_SHAPE(strict_union, (TypeShape{
                                      .inline_size = 16,
                                      .alignment = 8,
                                      .depth = 1,
                                      .max_out_of_line = 8,
                                  }));

  auto flexible_union = library.LookupUnion("FlexibleLeafUnion");
  EXPECT_TYPE_SHAPE(flexible_union, (TypeShape{
                                        .inline_size = 16,
                                        .alignment = 8,
                                        .depth = 1,
                                        .max_out_of_line = 8,
                                        .has_flexible_envelope = true,
                                    }));

  auto flexible_of_strict = library.LookupUnion("FlexibleUnionOfStrictUnion");
  EXPECT_TYPE_SHAPE(flexible_of_strict, (TypeShape{
                                            .inline_size = 16,
                                            .alignment = 8,
                                            .depth = 2,
                                            .max_out_of_line = 24,
                                            .has_flexible_envelope = true,
                                        }));

  auto flexible_of_flexible = library.LookupUnion("FlexibleUnionOfFlexibleUnion");
  EXPECT_TYPE_SHAPE(flexible_of_flexible, (TypeShape{
                                              .inline_size = 16,
                                              .alignment = 8,
                                              .depth = 2,
                                              .max_out_of_line = 24,
                                              .has_flexible_envelope = true,
                                          }));

  auto strict_of_strict = library.LookupUnion("StrictUnionOfStrictUnion");
  EXPECT_TYPE_SHAPE(strict_of_strict, (TypeShape{
                                          .inline_size = 16,
                                          .alignment = 8,
                                          .depth = 2,
                                          .max_out_of_line = 24,
                                      }));

  auto strict_of_flexible = library.LookupUnion("StrictUnionOfFlexibleUnion");
  EXPECT_TYPE_SHAPE(strict_of_flexible, (TypeShape{
                                            .inline_size = 16,
                                            .alignment = 8,
                                            .depth = 2,
                                            .max_out_of_line = 24,
                                            .has_flexible_envelope = true,
                                        }));

  auto flexible_table = library.LookupTable("FlexibleLeafTable");
  EXPECT_TYPE_SHAPE(flexible_table, (TypeShape{
                                        .inline_size = 16,
                                        .alignment = 8,
                                        .depth = 1,
                                        .has_flexible_envelope = true,
                                    }));

  auto strict_union_of_flexible_table = library.LookupUnion("StrictUnionOfFlexibleTable");
  EXPECT_TYPE_SHAPE(strict_union_of_flexible_table, (TypeShape{
                                                        .inline_size = 16,
                                                        .alignment = 8,
                                                        .depth = 2,
                                                        .max_out_of_line = 16,
                                                        .has_flexible_envelope = true,
                                                    }));
}

TEST(TypeshapeTests, GoodProtocolsAndRequestOfProtocols) {
  TestLibrary library(R"FIDL(
library example;

protocol SomeProtocol {};

type UsingSomeProtocol = resource struct {
    value client_end:SomeProtocol;
};

type UsingOptSomeProtocol = resource struct {
    value client_end:<SomeProtocol, optional>;
};

type UsingRequestSomeProtocol = resource struct {
    value server_end:SomeProtocol;
};

type UsingOptRequestSomeProtocol = resource struct {
    value server_end:<SomeProtocol, optional>;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto using_some_protocol = library.LookupStruct("UsingSomeProtocol");
  EXPECT_TYPE_SHAPE(using_some_protocol, (TypeShape{
                                             .inline_size = 4,
                                             .alignment = 4,
                                             .max_handles = 1,
                                         }));

  auto using_opt_some_protocol = library.LookupStruct("UsingOptSomeProtocol");
  EXPECT_TYPE_SHAPE(using_opt_some_protocol, (TypeShape{
                                                 .inline_size = 4,
                                                 .alignment = 4,
                                                 .max_handles = 1,
                                             }));

  auto using_request_some_protocol = library.LookupStruct("UsingRequestSomeProtocol");
  EXPECT_TYPE_SHAPE(using_request_some_protocol, (TypeShape{
                                                     .inline_size = 4,
                                                     .alignment = 4,
                                                     .max_handles = 1,
                                                 }));

  auto using_opt_request_some_protocol = library.LookupStruct("UsingOptRequestSomeProtocol");
  EXPECT_TYPE_SHAPE(using_opt_request_some_protocol, (TypeShape{
                                                         .inline_size = 4,
                                                         .alignment = 4,
                                                         .max_handles = 1,
                                                     }));
}

TEST(TypeshapeTests, GoodExternalDefinitions) {
  TestLibrary library;
  library.UseLibraryZx();
  library.AddSource("example.fidl", R"FIDL(
library example;

using zx;

type ExternalArrayStruct = struct {
    a array<ExternalSimpleStruct, EXTERNAL_SIZE_DEF>;
};

type ExternalStringSizeStruct = struct {
    a string:EXTERNAL_SIZE_DEF;
};

type ExternalVectorSizeStruct = resource struct {
    a vector<zx.Handle>:EXTERNAL_SIZE_DEF;
};
)FIDL");
  library.AddSource("extern_defs.fidl", R"FIDL(
library example;

const EXTERNAL_SIZE_DEF uint32 = ANOTHER_INDIRECTION;
const ANOTHER_INDIRECTION uint32 = 32;

type ExternalSimpleStruct = struct {
    a uint32;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto ext_struct = library.LookupStruct("ExternalSimpleStruct");
  EXPECT_TYPE_SHAPE(ext_struct, (TypeShape{
                                    .inline_size = 4,
                                    .alignment = 4,
                                }));

  auto ext_arr_struct = library.LookupStruct("ExternalArrayStruct");
  EXPECT_TYPE_SHAPE(ext_arr_struct, (TypeShape{
                                        .inline_size = 4 * 32,
                                        .alignment = 4,
                                    }));

  auto ext_str_struct = library.LookupStruct("ExternalStringSizeStruct");
  EXPECT_TYPE_SHAPE(ext_str_struct, (TypeShape{
                                        .inline_size = 16,
                                        .alignment = 8,
                                        .depth = 1,
                                        .max_out_of_line = 32,
                                        .has_padding = true,
                                    }));

  auto ext_vec_struct = library.LookupStruct("ExternalVectorSizeStruct");
  EXPECT_TYPE_SHAPE(ext_vec_struct, (TypeShape{
                                        .inline_size = 16,
                                        .alignment = 8,
                                        .depth = 1,
                                        .max_handles = 32,
                                        .max_out_of_line = 32 * 4,
                                        .has_padding = true,
                                    }));
}

TEST(TypeshapeTests, GoodSimpleRequest) {
  TestLibrary library(R"FIDL(
library example;

protocol Test {
    Method(struct { a int16; b int16; });
};
)FIDL");
  ASSERT_COMPILED(library);

  auto protocol = library.LookupProtocol("Test");
  ASSERT_NE(protocol, nullptr);
  ASSERT_EQ(protocol->methods.size(), 1u);
  auto& method = protocol->methods[0];
  auto method_request = method.maybe_request.get();
  EXPECT_EQ(method.kind, Protocol::Method::Kind::kOneWay);
  ASSERT_NE(method_request, nullptr);

  auto id = static_cast<const IdentifierType*>(method_request->type);
  auto as_struct = static_cast<const Struct*>(id->type_decl);

  EXPECT_TYPE_SHAPE(as_struct, (TypeShape{
                                   .inline_size = 4,
                                   .alignment = 2,
                               }));
  EXPECT_FIELD_SHAPES(as_struct, (std::vector<FieldShape>{
                                     {.offset = 0},
                                     {.offset = 2},
                                 }));
}

TEST(TypeshapeTests, GoodSimpleResponse) {
  TestLibrary library(R"FIDL(
library example;

protocol Test {
    strict Method() -> (struct { a int16; b int16; });
};
)FIDL");
  ASSERT_COMPILED(library);

  auto protocol = library.LookupProtocol("Test");
  ASSERT_NE(protocol, nullptr);
  ASSERT_EQ(protocol->methods.size(), 1u);
  auto& method = protocol->methods[0];
  EXPECT_EQ(method.kind, Protocol::Method::Kind::kTwoWay);
  auto method_response = method.maybe_response.get();
  ASSERT_NE(method_response, nullptr);

  auto id = static_cast<const IdentifierType*>(method_response->type);
  auto as_struct = static_cast<const Struct*>(id->type_decl);

  EXPECT_TYPE_SHAPE(as_struct, (TypeShape{
                                   .inline_size = 4,
                                   .alignment = 2,
                               }));
  EXPECT_FIELD_SHAPES(as_struct, (std::vector<FieldShape>{
                                     {.offset = 0},
                                     {.offset = 2},
                                 }));
}

TEST(TypeshapeTests, GoodRecursiveRequest) {
  TestLibrary library(R"FIDL(
library example;

type WebMessage = resource struct {
    message_port_req server_end:MessagePort;
};

protocol MessagePort {
    PostMessage(resource struct {
        message WebMessage;
    }) -> (struct {
        success bool;
    });
};
)FIDL");
  ASSERT_COMPILED(library);

  auto web_message = library.LookupStruct("WebMessage");
  EXPECT_TYPE_SHAPE(web_message, (TypeShape{
                                     .inline_size = 4,
                                     .alignment = 4,
                                     .max_handles = 1,
                                 }));
  EXPECT_FIELD_SHAPES(web_message, (std::vector<FieldShape>{{.offset = 0}}));

  auto message_port = library.LookupProtocol("MessagePort");
  ASSERT_EQ(message_port->methods.size(), 1u);
  auto& post_message = message_port->methods[0];
  EXPECT_EQ(post_message.kind, Protocol::Method::Kind::kTwoWay);
  auto post_message_request = post_message.maybe_request.get();

  auto id = static_cast<const IdentifierType*>(post_message_request->type);
  auto as_struct = static_cast<const Struct*>(id->type_decl);

  EXPECT_TYPE_SHAPE(as_struct, (TypeShape{
                                   .inline_size = 4,
                                   .alignment = 4,
                                   .max_handles = 1,
                               }));
  EXPECT_FIELD_SHAPES(as_struct, (std::vector<FieldShape>{{.offset = 0}}));
}

TEST(TypeshapeTests, GoodRecursiveOptRequest) {
  TestLibrary library(R"FIDL(
library example;

type WebMessage = resource struct {
    opt_message_port_req server_end:<MessagePort, optional>;
};

protocol MessagePort {
    PostMessage(resource struct {
        message WebMessage;
    }) -> (struct {
        success bool;
    });
};
)FIDL");
  ASSERT_COMPILED(library);

  auto web_message = library.LookupStruct("WebMessage");
  EXPECT_TYPE_SHAPE(web_message, (TypeShape{
                                     .inline_size = 4,
                                     .alignment = 4,
                                     .max_handles = 1,
                                 }));

  auto message_port = library.LookupProtocol("MessagePort");
  ASSERT_EQ(message_port->methods.size(), 1u);
  auto& post_message = message_port->methods[0];
  EXPECT_EQ(post_message.kind, Protocol::Method::Kind::kTwoWay);
  auto post_message_request = post_message.maybe_request.get();

  auto id = static_cast<const IdentifierType*>(post_message_request->type);
  auto as_struct = static_cast<const Struct*>(id->type_decl);

  EXPECT_TYPE_SHAPE(as_struct, (TypeShape{
                                   .inline_size = 4,
                                   .alignment = 4,
                                   .max_handles = 1,
                               }));
  EXPECT_FIELD_SHAPES(as_struct, (std::vector<FieldShape>{{.offset = 0}}));
}

TEST(TypeshapeTests, GoodRecursiveProtocol) {
  TestLibrary library(R"FIDL(
library example;

type WebMessage = resource struct {
    message_port client_end:MessagePort;
};

protocol MessagePort {
    PostMessage(resource struct {
        message WebMessage;
    }) -> (struct {
        success bool;
    });
};
)FIDL");
  ASSERT_COMPILED(library);

  auto web_message = library.LookupStruct("WebMessage");
  EXPECT_TYPE_SHAPE(web_message, (TypeShape{
                                     .inline_size = 4,
                                     .alignment = 4,
                                     .max_handles = 1,
                                 }));

  auto message_port = library.LookupProtocol("MessagePort");
  ASSERT_EQ(message_port->methods.size(), 1u);
  auto& post_message = message_port->methods[0];
  EXPECT_EQ(post_message.kind, Protocol::Method::Kind::kTwoWay);
  auto post_message_request = post_message.maybe_request.get();

  auto id = static_cast<const IdentifierType*>(post_message_request->type);
  auto as_struct = static_cast<const Struct*>(id->type_decl);

  EXPECT_TYPE_SHAPE(as_struct, (TypeShape{
                                   .inline_size = 4,
                                   .alignment = 4,
                                   .max_handles = 1,
                               }));
  EXPECT_FIELD_SHAPES(as_struct, (std::vector<FieldShape>{{.offset = 0}}));
}

TEST(TypeshapeTests, GoodRecursiveOptProtocol) {
  TestLibrary library(R"FIDL(
library example;

type WebMessage = resource struct {
    opt_message_port client_end:<MessagePort, optional>;
};

protocol MessagePort {
    PostMessage(resource struct {
        message WebMessage;
    }) -> (struct {
        success bool;
    });
};
)FIDL");
  ASSERT_COMPILED(library);

  auto web_message = library.LookupStruct("WebMessage");
  EXPECT_TYPE_SHAPE(web_message, (TypeShape{
                                     .inline_size = 4,
                                     .alignment = 4,
                                     .max_handles = 1,
                                 }));

  auto message_port = library.LookupProtocol("MessagePort");
  ASSERT_EQ(message_port->methods.size(), 1u);
  auto& post_message = message_port->methods[0];
  EXPECT_EQ(post_message.kind, Protocol::Method::Kind::kTwoWay);
  auto post_message_request = post_message.maybe_request.get();

  auto id = static_cast<const IdentifierType*>(post_message_request->type);
  auto as_struct = static_cast<const Struct*>(id->type_decl);

  EXPECT_TYPE_SHAPE(as_struct, (TypeShape{
                                   .inline_size = 4,
                                   .alignment = 4,
                                   .max_handles = 1,
                               }));
  EXPECT_FIELD_SHAPES(as_struct, (std::vector<FieldShape>{{.offset = 0}}));
}

TEST(TypeshapeTests, GoodRecursiveStruct) {
  TestLibrary library(R"FIDL(
library example;

type TheStruct = struct {
    opt_one_more box<TheStruct>;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto the_struct = library.LookupStruct("TheStruct");
  EXPECT_TYPE_SHAPE(the_struct, (TypeShape{
                                    .inline_size = 8,
                                    .alignment = 8,
                                    .depth = UINT32_MAX,
                                    .max_out_of_line = UINT32_MAX,
                                }));
  EXPECT_FIELD_SHAPES(the_struct, (std::vector<FieldShape>{{.offset = 0}}));
}

TEST(TypeshapeTests, GoodRecursiveStructWithHandles) {
  TestLibrary library(R"FIDL(
library example;
using zx;

type TheStruct = resource struct {
  some_handle zx.Handle:VMO;
  opt_one_more box<TheStruct>;
};
)FIDL");
  library.UseLibraryZx();
  ASSERT_COMPILED(library);

  auto the_struct = library.LookupStruct("TheStruct");
  EXPECT_TYPE_SHAPE(the_struct, (TypeShape{
                                    .inline_size = 16,
                                    .alignment = 8,
                                    .depth = UINT32_MAX,
                                    .max_handles = UINT32_MAX,
                                    .max_out_of_line = UINT32_MAX,
                                    .has_padding = true,
                                }));
  EXPECT_FIELD_SHAPES(the_struct, (std::vector<FieldShape>{
                                      {.offset = 0, .padding = 4},
                                      {.offset = 8},
                                  }));
}

TEST(TypeshapeTests, GoodCoRecursiveStruct) {
  TestLibrary library(R"FIDL(
library example;

type A = struct {
    foo box<B>;
};

type B = struct {
    bar box<A>;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto struct_a = library.LookupStruct("A");
  EXPECT_TYPE_SHAPE(struct_a, (TypeShape{
                                  .inline_size = 8,
                                  .alignment = 8,
                                  .depth = UINT32_MAX,
                                  .max_out_of_line = UINT32_MAX,
                              }));

  auto struct_b = library.LookupStruct("B");
  EXPECT_TYPE_SHAPE(struct_b, (TypeShape{
                                  .inline_size = 8,
                                  .alignment = 8,
                                  .depth = UINT32_MAX,
                                  .max_out_of_line = UINT32_MAX,
                              }));
}

TEST(TypeshapeTests, GoodCoRecursiveStructWithHandles) {
  TestLibrary library(R"FIDL(
library example;
using zx;

type A = resource struct {
    a zx.Handle;
    foo box<B>;
};

type B = resource struct {
    b zx.Handle;
    bar box<A>;
};
)FIDL");
  library.UseLibraryZx();
  ASSERT_COMPILED(library);

  auto struct_a = library.LookupStruct("A");
  EXPECT_TYPE_SHAPE(struct_a, (TypeShape{
                                  .inline_size = 16,
                                  .alignment = 8,
                                  .depth = UINT32_MAX,
                                  .max_handles = UINT32_MAX,
                                  .max_out_of_line = UINT32_MAX,
                                  .has_padding = true,
                              }));

  auto struct_b = library.LookupStruct("B");
  EXPECT_TYPE_SHAPE(struct_b, (TypeShape{
                                  .inline_size = 16,
                                  .alignment = 8,
                                  .depth = UINT32_MAX,
                                  .max_handles = UINT32_MAX,
                                  .max_out_of_line = UINT32_MAX,
                                  .has_padding = true,
                              }));
}

TEST(TypeshapeTests, GoodCoRecursiveStruct2) {
  TestLibrary library(R"FIDL(
library example;

type Foo = struct {
    b Bar;
};

type Bar = struct {
    f box<Foo>;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto struct_foo = library.LookupStruct("Foo");
  EXPECT_TYPE_SHAPE(struct_foo, (TypeShape{
                                    .inline_size = 8,
                                    .alignment = 8,
                                    .depth = UINT32_MAX,
                                    .max_out_of_line = UINT32_MAX,
                                }));

  auto struct_bar = library.LookupStruct("Bar");
  EXPECT_TYPE_SHAPE(struct_bar, (TypeShape{
                                    .inline_size = 8,
                                    .alignment = 8,
                                    .depth = UINT32_MAX,
                                    .max_out_of_line = UINT32_MAX,
                                }));
}

TEST(TypeshapeTests, GoodRecursiveUnionAndStruct) {
  TestLibrary library(R"FIDL(
library example;

type Foo = union {
    1: bar struct { f Foo:optional; };
};
)FIDL");
  ASSERT_COMPILED(library);

  auto union_foo = library.LookupUnion("Foo");
  EXPECT_TYPE_SHAPE(union_foo, (TypeShape{
                                   .inline_size = 16,
                                   .alignment = 8,
                                   .depth = UINT32_MAX,
                                   .max_out_of_line = UINT32_MAX,
                                   .has_flexible_envelope = true,
                               }));

  auto struct_bar = library.LookupStruct("Bar");
  EXPECT_TYPE_SHAPE(struct_bar, (TypeShape{
                                    .inline_size = 16,
                                    .alignment = 8,
                                    .depth = UINT32_MAX,
                                    .max_out_of_line = UINT32_MAX,
                                    .has_flexible_envelope = true,
                                }));
}

TEST(TypeshapeTests, GoodRecursiveUnionAndVector) {
  TestLibrary library(R"FIDL(
library example;

type Foo = union {
    1: bar vector<Foo:optional>;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto union_foo = library.LookupUnion("Foo");
  EXPECT_TYPE_SHAPE(union_foo, (TypeShape{
                                   .inline_size = 16,
                                   .alignment = 8,
                                   .depth = UINT32_MAX,
                                   .max_out_of_line = UINT32_MAX,
                                   .has_flexible_envelope = true,
                               }));
}

TEST(TypeshapeTests, GoodRecursionHandlesBounded) {
  TestLibrary library(R"FIDL(
library example;
using zx;

type Foo = resource union {
    1: bar array<Foo:optional, 1>;
    2: h zx.Handle;
};
)FIDL");
  library.UseLibraryZx();
  ASSERT_COMPILED(library);

  auto union_foo = library.LookupUnion("Foo");
  EXPECT_TYPE_SHAPE(union_foo, (TypeShape{
                                   .inline_size = 16,
                                   .alignment = 8,
                                   .depth = UINT32_MAX,
                                   // TODO(https://fxbug.dev/323940291): max_handles should be 1.
                                   .max_handles = UINT32_MAX,
                                   .max_out_of_line = UINT32_MAX,
                                   .has_flexible_envelope = true,
                               }));
}

TEST(TypeshapeTests, GoodRecursionHandlesUnboundedBranching) {
  TestLibrary library(R"FIDL(
library example;
using zx;

type Foo = resource union {
    1: bar array<Foo:optional, 2>;
    2: h zx.Handle;
};
)FIDL");
  library.UseLibraryZx();
  ASSERT_COMPILED(library);

  auto union_foo = library.LookupUnion("Foo");
  EXPECT_TYPE_SHAPE(union_foo, (TypeShape{
                                   .inline_size = 16,
                                   .alignment = 8,
                                   .depth = UINT32_MAX,
                                   .max_handles = UINT32_MAX,
                                   .max_out_of_line = UINT32_MAX,
                                   .has_flexible_envelope = true,
                               }));
}

TEST(TypeshapeTests, GoodRecursionHandlesUnboundedInCycle) {
  TestLibrary library(R"FIDL(
library example;
using zx;

type Foo = resource union {
    1: bar resource table {
        1: baz resource struct { f Foo:optional; };
        2: h zx.Handle;
    };
};
)FIDL");
  library.UseLibraryZx();
  ASSERT_COMPILED(library);

  auto union_foo = library.LookupUnion("Foo");
  EXPECT_TYPE_SHAPE(union_foo, (TypeShape{
                                   .inline_size = 16,
                                   .alignment = 8,
                                   .depth = UINT32_MAX,
                                   .max_handles = UINT32_MAX,
                                   .max_out_of_line = UINT32_MAX,
                                   .has_flexible_envelope = true,
                               }));

  auto table_bar = library.LookupTable("Bar");
  EXPECT_TYPE_SHAPE(table_bar, (TypeShape{
                                   .inline_size = 16,
                                   .alignment = 8,
                                   .depth = UINT32_MAX,
                                   .max_handles = UINT32_MAX,
                                   .max_out_of_line = UINT32_MAX,
                                   .has_flexible_envelope = true,
                               }));

  auto struct_baz = library.LookupStruct("Baz");
  EXPECT_TYPE_SHAPE(struct_baz, (TypeShape{
                                    .inline_size = 16,
                                    .alignment = 8,
                                    .depth = UINT32_MAX,
                                    .max_handles = UINT32_MAX,
                                    .max_out_of_line = UINT32_MAX,
                                    .has_flexible_envelope = true,
                                }));
}

TEST(TypeshapeTests, GoodStructTwoDeep) {
  TestLibrary library(R"FIDL(
library example;
using zx;

type DiffEntry = resource struct {
    key vector<uint8>:256;

    base box<Value>;
    left box<Value>;
    right box<Value>;
};

type Value = resource struct {
    value box<Buffer>;
    priority Priority;
};

type Buffer = resource struct {
    vmo zx.Handle:VMO;
    size uint64;
};

type Priority = enum {
    EAGER = 0;
    LAZY = 1;
};
)FIDL");
  library.UseLibraryZx();
  ASSERT_COMPILED(library);

  auto buffer = library.LookupStruct("Buffer");
  EXPECT_TYPE_SHAPE(buffer, (TypeShape{
                                .inline_size = 16,
                                .alignment = 8,
                                .max_handles = 1,
                                .has_padding = true,
                            }));

  auto value = library.LookupStruct("Value");
  EXPECT_TYPE_SHAPE(value,
                    (TypeShape{
                        .inline_size = 16,
                        .alignment = 8,
                        .depth = 1,
                        .max_handles = 1,
                        .max_out_of_line = 16,
                        .has_padding = true,  // because the size of |Priority| defaults to uint32
                    }));

  auto diff_entry = library.LookupStruct("DiffEntry");
  EXPECT_TYPE_SHAPE(diff_entry, (TypeShape{
                                    .inline_size = 40,
                                    .alignment = 8,
                                    .depth = 2,
                                    .max_handles = 3,
                                    .max_out_of_line = 352,
                                    .has_padding = true,  // because |Value| has padding
                                }));
}

TEST(TypeshapeTests, GoodProtocolChildAndParent) {
  SharedAmongstLibraries shared;
  TestLibrary parent_library(&shared, "parent.fidl", R"FIDL(
library parent;

protocol Parent {
    Sync() -> ();
};
)FIDL");
  ASSERT_COMPILED(parent_library);

  TestLibrary child_library(&shared, "child.fidl", R"FIDL(
library child;

using parent;

protocol Child {
  compose parent.Parent;
};
)FIDL");
  ASSERT_COMPILED(child_library);

  auto child = child_library.LookupProtocol("Child");
  ASSERT_EQ(child->all_methods.size(), 1u);
  auto& sync_with_info = child->all_methods[0];
  EXPECT_EQ(sync_with_info.method->kind, Protocol::Method::Kind::kTwoWay);
  EXPECT_EQ(sync_with_info.method->maybe_request, nullptr);
}

TEST(TypeshapeTests, GoodUnionSize8Alignment4Sandwich) {
  TestLibrary library(R"FIDL(
library example;

type UnionSize8Alignment4 = strict union {
    1: variant uint32;
};

type Sandwich = struct {
    before uint32;
    union UnionSize8Alignment4;
    after uint32;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto sandwich = library.LookupStruct("Sandwich");
  EXPECT_TYPE_SHAPE(sandwich, (TypeShape{
                                  .inline_size = 32,
                                  .alignment = 8,
                                  .depth = 1,
                                  .has_padding = true,
                              }));
  EXPECT_FIELD_SHAPES(sandwich, (std::vector<FieldShape>{
                                    {.offset = 0, .padding = 4},   // before
                                    {.offset = 8},                 // union
                                    {.offset = 24, .padding = 4},  // after
                                }));
}

TEST(TypeshapeTests, GoodUnionSize12Alignment4Sandwich) {
  TestLibrary library(R"FIDL(
library example;

type UnionSize12Alignment4 = strict union {
    1: variant array<uint8, 6>;
};

type Sandwich = struct {
    before uint32;
    union UnionSize12Alignment4;
    after int32;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto sandwich = library.LookupStruct("Sandwich");
  EXPECT_TYPE_SHAPE(sandwich, (TypeShape{
                                  .inline_size = 32,
                                  .alignment = 8,
                                  .depth = 1,
                                  .max_out_of_line = 8,
                                  .has_padding = true,
                              }));
  EXPECT_FIELD_SHAPES(sandwich, (std::vector<FieldShape>{
                                    {.offset = 0, .padding = 4},   // before
                                    {.offset = 8},                 // union
                                    {.offset = 24, .padding = 4},  // after
                                }));
}

TEST(TypeshapeTests, GoodUnionSize24Alignment8Sandwich) {
  TestLibrary library(R"FIDL(
library example;

type StructSize16Alignment8 = struct {
    f1 uint64;
    f2 uint64;
};

type UnionSize24Alignment8 = strict union {
    1: variant StructSize16Alignment8;
};

type Sandwich = struct {
    before uint32;
    union UnionSize24Alignment8;
    after uint32;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto sandwich = library.LookupStruct("Sandwich");
  EXPECT_TYPE_SHAPE(sandwich, (TypeShape{
                                  .inline_size = 32,
                                  .alignment = 8,
                                  .depth = 1,
                                  .max_out_of_line = 16,
                                  .has_padding = true,
                              }));
  EXPECT_FIELD_SHAPES(sandwich, (std::vector<FieldShape>{
                                    {.offset = 0, .padding = 4},   // before
                                    {.offset = 8},                 // union
                                    {.offset = 24, .padding = 4},  // after
                                }));
}

TEST(TypeshapeTests, GoodUnionSize36Alignment4Sandwich) {
  TestLibrary library(R"FIDL(
library example;

type UnionSize36Alignment4 = strict union {
    1: variant array<uint8, 32>;
};

type Sandwich = struct {
    before uint32;
    union UnionSize36Alignment4;
    after uint32;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto sandwich = library.LookupStruct("Sandwich");
  EXPECT_TYPE_SHAPE(sandwich, (TypeShape{
                                  .inline_size = 32,
                                  .alignment = 8,
                                  .depth = 1,
                                  .max_out_of_line = 32,
                                  .has_padding = true,
                              }));
  EXPECT_FIELD_SHAPES(sandwich, (std::vector<FieldShape>{
                                    {.offset = 0, .padding = 4},   // before
                                    {.offset = 8},                 // union
                                    {.offset = 24, .padding = 4},  // after
                                }));
}

TEST(TypeshapeTests, GoodZeroSizeVector) {
  TestLibrary library(R"FIDL(
library example;
using zx;

type A = resource struct {
    zero_size vector<zx.Handle>:0;
};
)FIDL");
  library.UseLibraryZx();
  ASSERT_COMPILED(library);

  auto struct_a = library.LookupStruct("A");
  EXPECT_TYPE_SHAPE(struct_a, (TypeShape{
                                  .inline_size = 16,
                                  .alignment = 8,
                                  .depth = 1,
                                  .has_padding = true,
                              }));
}

// TODO(https://fxbug.dev/323940291): Enable this. Currently can't report the
// error because there is no SourceSpan to use.
TEST(TypeshapeTests, DISABLED_BadIntegerOverflowArray) {
  TestLibrary library;
  library.AddFile("bad/fi-0207.test.fidl");
  library.ExpectFail(ErrTypeShapeIntegerOverflow, 536870912, '*', 8);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TypeshapeTests, BadIntegerOverflowStruct) {
  TestLibrary library(R"FIDL(
library example;
type Foo = struct {
    f1 array<uint8, 2147483648>; // 2^31
    f2 array<uint8, 2147483648>; // 2^31
};
)FIDL");
  library.ExpectFail(ErrTypeShapeIntegerOverflow, 1 << 31, '+', 1 << 31);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TypeshapeTests, BadInlineSizeExceedsLimit) {
  TestLibrary library(R"FIDL(
library example;
type Foo = struct {
    big array<uint8, 65536>; // 2^16
};
)FIDL");
  library.ExpectFail(ErrInlineSizeExceedsLimit, "Foo", UINT16_MAX + 1, UINT16_MAX);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

}  // namespace
}  // namespace fidlc
