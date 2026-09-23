// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/types.h>

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/src/properties.h"
#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

std::optional<PrimitiveSubtype> ConstPrimitiveSubtype(TestLibrary& library,
                                                      const char* const_name) {
  auto type = library.LookupConstant(const_name)->value->type;
  if (type->kind == Type::Kind::kPrimitive) {
    return static_cast<const PrimitiveType*>(type)->subtype;
  }
  return std::nullopt;
}

TEST(TypesTests, GoodRootTypesUnqualified) {
  TestLibrary library(R"FIDL(
library example;

const b bool = false;
const i8 int8 = 0;
const i16 int16 = 0;
const i32 int32 = 0;
const i64 int64 = 0;
const u8 uint8 = 0;
const u16 uint16 = 0;
const u32 uint32 = 0;
const u64 uint64 = 0;
const us usize64 = 0;
const up uintptr64 = 0;
const uc uchar = 0;
const f32 float32 = 0;
const f64 float64 = 0;
)FIDL");

  // For the use of usize64, uintptr64, and uchar.
  library.EnableFlag(ExperimentalFlag::kZxCTypes);

  ASSERT_COMPILED(library);

  EXPECT_EQ(ConstPrimitiveSubtype(library, "b"), PrimitiveSubtype::kBool);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "i8"), PrimitiveSubtype::kInt8);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "i16"), PrimitiveSubtype::kInt16);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "i32"), PrimitiveSubtype::kInt32);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "i64"), PrimitiveSubtype::kInt64);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "u8"), PrimitiveSubtype::kUint8);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "u16"), PrimitiveSubtype::kUint16);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "u32"), PrimitiveSubtype::kUint32);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "u64"), PrimitiveSubtype::kUint64);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "us"), PrimitiveSubtype::kZxUsize64);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "up"), PrimitiveSubtype::kZxUintptr64);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "uc"), PrimitiveSubtype::kZxUchar);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "f32"), PrimitiveSubtype::kFloat32);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "f64"), PrimitiveSubtype::kFloat64);
}

TEST(TypesTests, GoodRootTypesQualified) {
  TestLibrary library(R"FIDL(
library example;

const bool fidl.bool = false;
const int8 fidl.int8 = 0;
const int16 fidl.int16 = 0;
const int32 fidl.int32 = 0;
const int64 fidl.int64 = 0;
const uint8 fidl.uint8 = 0;
const uint16 fidl.uint16 = 0;
const uint32 fidl.uint32 = 0;
const uint64 fidl.uint64 = 0;
const usize64 fidl.usize64 = 0;
const uintptr64 fidl.uintptr64 = 0;
const uchar fidl.uchar = 0;
const float32 fidl.float32 = 0;
const float64 fidl.float64 = 0;
)FIDL");

  // For the use of usize64, uintptr64, and uchar.
  library.EnableFlag(ExperimentalFlag::kZxCTypes);

  ASSERT_COMPILED(library);

  EXPECT_EQ(ConstPrimitiveSubtype(library, "bool"), PrimitiveSubtype::kBool);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "int8"), PrimitiveSubtype::kInt8);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "int16"), PrimitiveSubtype::kInt16);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "int32"), PrimitiveSubtype::kInt32);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "int64"), PrimitiveSubtype::kInt64);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "uint8"), PrimitiveSubtype::kUint8);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "uint16"), PrimitiveSubtype::kUint16);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "uint32"), PrimitiveSubtype::kUint32);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "uint64"), PrimitiveSubtype::kUint64);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "usize64"), PrimitiveSubtype::kZxUsize64);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "uintptr64"), PrimitiveSubtype::kZxUintptr64);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "uchar"), PrimitiveSubtype::kZxUchar);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "float32"), PrimitiveSubtype::kFloat32);
  EXPECT_EQ(ConstPrimitiveSubtype(library, "float64"), PrimitiveSubtype::kFloat64);
}

// Check that fidl's types.h and zircon/types.h's handle subtype
// values stay in sync, until the latter is generated.
TEST(TypesTests, GoodHandleSubtype) {
  static_assert(sizeof(HandleSubtype) == sizeof(zx_obj_type_t));

  static_assert(HandleSubtype::kHandle == static_cast<HandleSubtype>(ZX_OBJ_TYPE_NONE));

  static_assert(HandleSubtype::kBti == static_cast<HandleSubtype>(ZX_OBJ_TYPE_BTI));
  static_assert(HandleSubtype::kChannel == static_cast<HandleSubtype>(ZX_OBJ_TYPE_CHANNEL));
  static_assert(HandleSubtype::kClock == static_cast<HandleSubtype>(ZX_OBJ_TYPE_CLOCK));
  static_assert(HandleSubtype::kCounter == static_cast<HandleSubtype>(ZX_OBJ_TYPE_COUNTER));
  static_assert(HandleSubtype::kEvent == static_cast<HandleSubtype>(ZX_OBJ_TYPE_EVENT));
  static_assert(HandleSubtype::kEventpair == static_cast<HandleSubtype>(ZX_OBJ_TYPE_EVENTPAIR));
  static_assert(HandleSubtype::kException == static_cast<HandleSubtype>(ZX_OBJ_TYPE_EXCEPTION));
  static_assert(HandleSubtype::kFifo == static_cast<HandleSubtype>(ZX_OBJ_TYPE_FIFO));
  static_assert(HandleSubtype::kGuest == static_cast<HandleSubtype>(ZX_OBJ_TYPE_GUEST));
  static_assert(HandleSubtype::kInterrupt == static_cast<HandleSubtype>(ZX_OBJ_TYPE_INTERRUPT));
  static_assert(HandleSubtype::kIob == static_cast<HandleSubtype>(ZX_OBJ_TYPE_IOB));
  static_assert(HandleSubtype::kIommu == static_cast<HandleSubtype>(ZX_OBJ_TYPE_IOMMU));
  static_assert(HandleSubtype::kJob == static_cast<HandleSubtype>(ZX_OBJ_TYPE_JOB));
  static_assert(HandleSubtype::kDebugLog == static_cast<HandleSubtype>(ZX_OBJ_TYPE_DEBUGLOG));
  static_assert(HandleSubtype::kPager == static_cast<HandleSubtype>(ZX_OBJ_TYPE_PAGER));
  static_assert(HandleSubtype::kPciDevice == static_cast<HandleSubtype>(ZX_OBJ_TYPE_PCI_DEVICE));
  static_assert(HandleSubtype::kPmt == static_cast<HandleSubtype>(ZX_OBJ_TYPE_PMT));
  static_assert(HandleSubtype::kPort == static_cast<HandleSubtype>(ZX_OBJ_TYPE_PORT));
  static_assert(HandleSubtype::kProcess == static_cast<HandleSubtype>(ZX_OBJ_TYPE_PROCESS));
  static_assert(HandleSubtype::kProfile == static_cast<HandleSubtype>(ZX_OBJ_TYPE_PROFILE));
  static_assert(HandleSubtype::kResource == static_cast<HandleSubtype>(ZX_OBJ_TYPE_RESOURCE));
  static_assert(HandleSubtype::kSocket == static_cast<HandleSubtype>(ZX_OBJ_TYPE_SOCKET));
  static_assert(HandleSubtype::kStream == static_cast<HandleSubtype>(ZX_OBJ_TYPE_STREAM));
  static_assert(HandleSubtype::kSuspendToken ==
                static_cast<HandleSubtype>(ZX_OBJ_TYPE_SUSPEND_TOKEN));
  static_assert(HandleSubtype::kThread == static_cast<HandleSubtype>(ZX_OBJ_TYPE_THREAD));
  static_assert(HandleSubtype::kTimer == static_cast<HandleSubtype>(ZX_OBJ_TYPE_TIMER));
  static_assert(HandleSubtype::kVcpu == static_cast<HandleSubtype>(ZX_OBJ_TYPE_VCPU));
  static_assert(HandleSubtype::kVmar == static_cast<HandleSubtype>(ZX_OBJ_TYPE_VMAR));
  static_assert(HandleSubtype::kVmo == static_cast<HandleSubtype>(ZX_OBJ_TYPE_VMO));
}

// Check that fidl's types.h and zircon/types.h's rights types stay in
// sync, until the latter is generated.
TEST(TypesTests, GoodRights) { static_assert(sizeof(RightsWrappedType) == sizeof(zx_rights_t)); }

TEST(TypesTests, GoodTypeDeclOfAnonymousLayouts) {
  TestLibrary library(R"FIDL(
library example;
type TypeDecl = struct {
    f0 bits {
      FOO = 1;
    };
    f1 enum {
      BAR = 1;
    };
    f2 struct {
      i0 vector<uint8>;
      @allow_deprecated_struct_defaults
      i1 string = "foo";
    };
    f3 table {
      1: i0 bool;
    };
    f4 union {
      1: i0 bool;
    };
};
)FIDL");
  ASSERT_COMPILED(library);
  auto type_decl = library.LookupStruct("TypeDecl");
  ASSERT_NE(type_decl, nullptr);
  EXPECT_EQ(type_decl->members.size(), 5u);
  auto type_decl_f0 = library.LookupBits("F0");
  ASSERT_NE(type_decl_f0, nullptr);
  EXPECT_EQ(type_decl_f0->members.size(), 1u);
  auto type_decl_f1 = library.LookupEnum("F1");
  ASSERT_NE(type_decl_f1, nullptr);
  EXPECT_EQ(type_decl_f1->members.size(), 1u);
  auto type_decl_f2 = library.LookupStruct("F2");
  ASSERT_NE(type_decl_f2, nullptr);
  EXPECT_EQ(type_decl_f2->members.size(), 2u);
  auto type_decl_f3 = library.LookupTable("F3");
  ASSERT_NE(type_decl_f3, nullptr);
  EXPECT_EQ(type_decl_f3->members.size(), 1u);
  auto type_decl_f4 = library.LookupUnion("F4");
  ASSERT_NE(type_decl_f4, nullptr);
  EXPECT_EQ(type_decl_f4->members.size(), 1u);
}

TEST(TypesTests, BadTypeDeclOfNewTypeErrors) {
  TestLibrary library;
  library.AddFile("bad/fi-0062.test.fidl");
  library.ExpectFail(ErrNewTypesNotAllowed, "Matrix", "array");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TypesTests, GoodTypeParameters) {
  TestLibrary library(R"FIDL(
library example;
type Inner = struct{};
alias Alias = Inner;

type TypeDecl = struct {
  // vector of primitive
  v0 vector<uint8>;
  // vector of sourced
  v1 vector<Inner>;
  // vector of alias
  v2 vector<Alias>;
  // vector of anonymous layout
  v3 vector<struct{
       i0 struct{};
       i1 vector<struct{}>;
     }>;
  // array of primitive
  a0 array<uint8,5>;
  // array of sourced
  a1 array<Inner,5>;
  // array of alias
  a2 array<Alias,5>;
  // array of anonymous layout
  a3 array<struct{
       i2 struct{};
       i3 array<struct{},5>;
     },5>;
};
)FIDL");

  ASSERT_COMPILED(library);
  auto type_decl = library.LookupStruct("TypeDecl");
  ASSERT_NE(type_decl, nullptr);
  EXPECT_EQ(type_decl->members.size(), 8u);
  auto type_decl_vector_anon = library.LookupStruct("V3");
  ASSERT_NE(type_decl_vector_anon, nullptr);
  EXPECT_EQ(type_decl_vector_anon->members.size(), 2u);
  ASSERT_TRUE(library.HasStruct("I0"));
  ASSERT_TRUE(library.HasStruct("I1"));
  auto type_decl_array_anon = library.LookupStruct("A3");
  ASSERT_NE(type_decl_array_anon, nullptr);
  EXPECT_EQ(type_decl_array_anon->members.size(), 2u);
  ASSERT_TRUE(library.HasStruct("I2"));
  ASSERT_TRUE(library.HasStruct("I3"));
}

TEST(TypesTests, GoodLayoutMemberConstraints) {
  TestLibrary library(R"FIDL(
library example;

alias Alias = vector<uint8>;
type t1 = resource struct {
  u0 union { 1: b bool; };
  u1 union { 1: b bool; }:optional;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto type_decl = library.LookupStruct("t1");
  ASSERT_NE(type_decl, nullptr);
  EXPECT_EQ(type_decl->members.size(), 2u);

  size_t i = 0;

  auto u0_type_base = type_decl->members[i++].type_ctor->type;
  ASSERT_EQ(u0_type_base->kind, Type::Kind::kIdentifier);
  auto u0_type = static_cast<const IdentifierType*>(u0_type_base);
  EXPECT_EQ(u0_type->nullability, Nullability::kNonnullable);
  EXPECT_EQ(u0_type->type_decl->kind, Decl::Kind::kUnion);

  auto u1_type_base = type_decl->members[i++].type_ctor->type;
  ASSERT_EQ(u1_type_base->kind, Type::Kind::kIdentifier);
  auto u1_type = static_cast<const IdentifierType*>(u1_type_base);
  EXPECT_EQ(u1_type->nullability, Nullability::kNullable);
  EXPECT_EQ(u1_type->type_decl->kind, Decl::Kind::kUnion);
}

TEST(TypesTests, GoodConstraintsOnVectors) {
  TestLibrary library(R"FIDL(
library example;

alias Alias = vector<uint8>;
type TypeDecl= struct {
  v0 vector<bool>;
  v1 vector<bool>:16;
  v2 vector<bool>:optional;
  v3 vector<bool>:<16,optional>;
  b4 vector<uint8>;
  b5 vector<uint8>:16;
  b6 vector<uint8>:optional;
  b7 vector<uint8>:<16,optional>;
  s8 string;
  s9 string:16;
  s10 string:optional;
  s11 string:<16,optional>;
  a12 Alias;
  a13 Alias:16;
  a14 Alias:optional;
  a15 Alias:<16,optional>;
};
)FIDL");

  ASSERT_COMPILED(library);
  auto type_decl = library.LookupStruct("TypeDecl");
  ASSERT_NE(type_decl, nullptr);
  ASSERT_EQ(type_decl->members.size(), 16u);

  size_t i = 0;

  auto v0_type_base = type_decl->members[i++].type_ctor->type;
  ASSERT_EQ(v0_type_base->kind, Type::Kind::kVector);
  auto v0_type = static_cast<const VectorType*>(v0_type_base);
  EXPECT_EQ(v0_type->nullability, Nullability::kNonnullable);
  EXPECT_EQ(v0_type->element_type->kind, Type::Kind::kPrimitive);
  EXPECT_EQ(v0_type->ElementCount(), kMaxSize);

  auto v1_type_base = type_decl->members[i++].type_ctor->type;
  ASSERT_EQ(v1_type_base->kind, Type::Kind::kVector);
  auto v1_type = static_cast<const VectorType*>(v1_type_base);
  EXPECT_EQ(v1_type->nullability, Nullability::kNonnullable);
  EXPECT_EQ(v1_type->element_type->kind, Type::Kind::kPrimitive);
  EXPECT_EQ(v1_type->ElementCount(), 16u);

  auto v2_type_base = type_decl->members[i++].type_ctor->type;
  ASSERT_EQ(v2_type_base->kind, Type::Kind::kVector);
  auto v2_type = static_cast<const VectorType*>(v2_type_base);
  EXPECT_EQ(v2_type->nullability, Nullability::kNullable);
  EXPECT_EQ(v2_type->element_type->kind, Type::Kind::kPrimitive);
  EXPECT_EQ(v2_type->ElementCount(), kMaxSize);

  auto v3_type_base = type_decl->members[i++].type_ctor->type;
  ASSERT_EQ(v3_type_base->kind, Type::Kind::kVector);
  auto v3_type = static_cast<const VectorType*>(v3_type_base);
  EXPECT_EQ(v3_type->nullability, Nullability::kNullable);
  EXPECT_EQ(v3_type->ElementCount(), 16u);

  auto b4_type_base = type_decl->members[i++].type_ctor->type;
  ASSERT_EQ(b4_type_base->kind, Type::Kind::kVector);
  auto b4_type = static_cast<const VectorType*>(b4_type_base);
  EXPECT_EQ(b4_type->nullability, Nullability::kNonnullable);
  EXPECT_EQ(b4_type->ElementCount(), kMaxSize);

  auto b5_type_base = type_decl->members[i++].type_ctor->type;
  ASSERT_EQ(b5_type_base->kind, Type::Kind::kVector);
  auto b5_type = static_cast<const VectorType*>(b5_type_base);
  EXPECT_EQ(b5_type->nullability, Nullability::kNonnullable);
  EXPECT_EQ(b5_type->ElementCount(), 16u);

  auto b6_type_base = type_decl->members[i++].type_ctor->type;
  ASSERT_EQ(b6_type_base->kind, Type::Kind::kVector);
  auto b6_type = static_cast<const VectorType*>(b6_type_base);
  EXPECT_EQ(b6_type->nullability, Nullability::kNullable);
  EXPECT_EQ(b6_type->ElementCount(), kMaxSize);

  auto b7_type_base = type_decl->members[i++].type_ctor->type;
  ASSERT_EQ(b7_type_base->kind, Type::Kind::kVector);
  auto b7_type = static_cast<const VectorType*>(b7_type_base);
  EXPECT_EQ(b7_type->nullability, Nullability::kNullable);
  EXPECT_EQ(b7_type->ElementCount(), 16u);

  auto s8_type_base = type_decl->members[i++].type_ctor->type;
  ASSERT_EQ(s8_type_base->kind, Type::Kind::kString);
  auto s8_type = static_cast<const StringType*>(s8_type_base);
  EXPECT_EQ(s8_type->nullability, Nullability::kNonnullable);
  EXPECT_EQ(s8_type->MaxSize(), kMaxSize);

  auto s9_type_base = type_decl->members[i++].type_ctor->type;
  ASSERT_EQ(s9_type_base->kind, Type::Kind::kString);
  auto s9_type = static_cast<const StringType*>(s9_type_base);
  EXPECT_EQ(s9_type->nullability, Nullability::kNonnullable);
  EXPECT_EQ(s9_type->MaxSize(), 16u);

  auto s10_type_base = type_decl->members[i++].type_ctor->type;
  ASSERT_EQ(s10_type_base->kind, Type::Kind::kString);
  auto s10_type = static_cast<const StringType*>(s10_type_base);
  EXPECT_EQ(s10_type->nullability, Nullability::kNullable);
  EXPECT_EQ(s10_type->MaxSize(), kMaxSize);

  auto s11_type_base = type_decl->members[i++].type_ctor->type;
  ASSERT_EQ(s11_type_base->kind, Type::Kind::kString);
  auto s11_type = static_cast<const StringType*>(s11_type_base);
  EXPECT_EQ(s11_type->nullability, Nullability::kNullable);
  EXPECT_EQ(s11_type->MaxSize(), 16u);

  auto a12_invocation = type_decl->members[i].type_ctor->resolved_params;
  EXPECT_EQ(a12_invocation.element_type_resolved, nullptr);
  EXPECT_EQ(a12_invocation.nullability, Nullability::kNonnullable);
  auto a12_type_base = type_decl->members[i++].type_ctor->type;
  ASSERT_EQ(a12_type_base->kind, Type::Kind::kVector);
  auto a12_type = static_cast<const VectorType*>(a12_type_base);
  EXPECT_EQ(a12_type->nullability, Nullability::kNonnullable);
  EXPECT_EQ(a12_type->element_type->kind, Type::Kind::kPrimitive);
  EXPECT_EQ(a12_type->ElementCount(), kMaxSize);
  EXPECT_EQ(a12_invocation.size_resolved, nullptr);

  auto a13_invocation = type_decl->members[i].type_ctor->resolved_params;
  EXPECT_EQ(a13_invocation.element_type_resolved, nullptr);
  EXPECT_EQ(a13_invocation.nullability, Nullability::kNonnullable);
  auto a13_type_base = type_decl->members[i++].type_ctor->type;
  ASSERT_EQ(a13_type_base->kind, Type::Kind::kVector);
  auto a13_type = static_cast<const VectorType*>(a13_type_base);
  EXPECT_EQ(a13_type->nullability, Nullability::kNonnullable);
  EXPECT_EQ(a13_type->element_type->kind, Type::Kind::kPrimitive);
  EXPECT_EQ(a13_type->ElementCount(), 16u);
  EXPECT_EQ(a13_type->ElementCount(), a13_invocation.size_resolved->value);

  auto a14_invocation = type_decl->members[i].type_ctor->resolved_params;
  EXPECT_EQ(a14_invocation.element_type_resolved, nullptr);
  EXPECT_EQ(a14_invocation.nullability, Nullability::kNullable);
  auto a14_type_base = type_decl->members[i++].type_ctor->type;
  ASSERT_EQ(a14_type_base->kind, Type::Kind::kVector);
  auto a14_type = static_cast<const VectorType*>(a14_type_base);
  EXPECT_EQ(a14_type->nullability, Nullability::kNullable);
  EXPECT_EQ(a14_type->element_type->kind, Type::Kind::kPrimitive);
  EXPECT_EQ(a14_type->ElementCount(), kMaxSize);
  // EXPECT_EQ(a14_type->ElementCount(), a14_invocation->maybe_size);
  EXPECT_EQ(a14_invocation.size_resolved, nullptr);

  auto a15_invocation = type_decl->members[i].type_ctor->resolved_params;
  EXPECT_EQ(a15_invocation.element_type_resolved, nullptr);
  EXPECT_EQ(a15_invocation.nullability, Nullability::kNullable);
  auto a15_type_base = type_decl->members[i++].type_ctor->type;
  ASSERT_EQ(a15_type_base->kind, Type::Kind::kVector);
  auto a15_type = static_cast<const VectorType*>(a15_type_base);
  EXPECT_EQ(a15_type->nullability, Nullability::kNullable);
  EXPECT_EQ(a15_type->ElementCount(), 16u);
  EXPECT_EQ(a15_type->ElementCount(), a15_invocation.size_resolved->value);
}

TEST(TypesTests, GoodConstraintsOnUnions) {
  TestLibrary library(R"FIDL(
library example;

type UnionDecl = union{1: foo bool;};
alias UnionAlias = UnionDecl;
type TypeDecl= struct {
  u0 union{1: bar bool;};
  u1 union{1: baz bool;}:optional;
  u2 UnionDecl;
  u3 UnionDecl:optional;
  u4 UnionAlias;
  u5 UnionAlias:optional;
};
)FIDL");

  ASSERT_COMPILED(library);
  auto type_decl = library.LookupStruct("TypeDecl");
  ASSERT_NE(type_decl, nullptr);
  ASSERT_EQ(type_decl->members.size(), 6u);
  size_t i = 0;

  auto& u0 = type_decl->members[i++];
  auto u0_type = static_cast<const IdentifierType*>(u0.type_ctor->type);
  EXPECT_EQ(u0_type->nullability, Nullability::kNonnullable);

  auto& u1 = type_decl->members[i++];
  auto u1_type = static_cast<const IdentifierType*>(u1.type_ctor->type);
  EXPECT_EQ(u1_type->nullability, Nullability::kNullable);

  auto& u2 = type_decl->members[i++];
  auto u2_type = static_cast<const IdentifierType*>(u2.type_ctor->type);
  EXPECT_EQ(u2_type->nullability, Nullability::kNonnullable);

  auto& u3 = type_decl->members[i++];
  auto u3_type = static_cast<const IdentifierType*>(u3.type_ctor->type);
  EXPECT_EQ(u3_type->nullability, Nullability::kNullable);

  auto& u4 = type_decl->members[i++];
  auto u4_type = static_cast<const IdentifierType*>(u4.type_ctor->type);
  EXPECT_EQ(u4_type->nullability, Nullability::kNonnullable);

  auto& u5 = type_decl->members[i++];
  auto u5_type = static_cast<const IdentifierType*>(u5.type_ctor->type);
  EXPECT_EQ(u5_type->nullability, Nullability::kNullable);
}

TEST(TypesTests, GoodConstraintsOnHandles) {
  TestLibrary library(R"FIDL(
library example;
using zx;

type TypeDecl = resource struct {
  h0 zx.Handle;
  h1 zx.Handle:VMO;
  h2 zx.Handle:optional;
  h3 zx.Handle:<VMO,optional>;
  h4 zx.Handle:<VMO,zx.Rights.TRANSFER>;
  h5 zx.Handle:<VMO,zx.Rights.TRANSFER,optional>;
};
)FIDL");
  library.UseLibraryZx();

  ASSERT_COMPILED(library);
  auto type_decl = library.LookupStruct("TypeDecl");
  ASSERT_NE(type_decl, nullptr);
  ASSERT_EQ(type_decl->members.size(), 6u);

  auto& h0 = type_decl->members[0];
  auto h0_type = static_cast<const HandleType*>(h0.type_ctor->type);
  EXPECT_EQ(h0_type->subtype, HandleSubtype::kHandle);
  EXPECT_EQ(h0_type->rights, &HandleType::kSameRights);
  EXPECT_EQ(h0_type->nullability, Nullability::kNonnullable);

  auto& h1 = type_decl->members[1];
  auto h1_type = static_cast<const HandleType*>(h1.type_ctor->type);
  EXPECT_NE(h1_type->subtype, HandleSubtype::kHandle);
  EXPECT_EQ(h1_type->rights, &HandleType::kSameRights);
  EXPECT_EQ(h1_type->nullability, Nullability::kNonnullable);

  auto& h2 = type_decl->members[2];
  auto h2_type = static_cast<const HandleType*>(h2.type_ctor->type);
  EXPECT_EQ(h2_type->subtype, HandleSubtype::kHandle);
  EXPECT_EQ(h2_type->rights, &HandleType::kSameRights);
  EXPECT_EQ(h2_type->nullability, Nullability::kNullable);

  auto& h3 = type_decl->members[3];
  auto h3_type = static_cast<const HandleType*>(h3.type_ctor->type);
  EXPECT_EQ(h3_type->subtype, HandleSubtype::kVmo);  // VMO
  EXPECT_EQ(h3_type->rights, &HandleType::kSameRights);
  EXPECT_EQ(h3_type->nullability, Nullability::kNullable);

  auto& h4 = type_decl->members[4];
  auto h4_type = static_cast<const HandleType*>(h4.type_ctor->type);
  EXPECT_EQ(h4_type->subtype, HandleSubtype::kVmo);  // VMO
  EXPECT_EQ(h4_type->rights->value, 0x02u);          // TRANSFER
  EXPECT_EQ(h4_type->nullability, Nullability::kNonnullable);

  auto& h5 = type_decl->members[5];
  auto h5_type = static_cast<const HandleType*>(h5.type_ctor->type);
  EXPECT_EQ(h5_type->subtype, HandleSubtype::kVmo);  // VMO
  EXPECT_EQ(h5_type->rights->value, 0x02u);          // TRANSFER
  EXPECT_EQ(h5_type->nullability, Nullability::kNullable);
}

TEST(TypesTests, BadTooManyLayoutParameters) {
  TestLibrary library;
  library.AddFile("bad/fi-0162-b.test.fidl");
  library.ExpectFail(ErrWrongNumberOfLayoutParameters, "uint8", 0, 1);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TypesTests, BadZeroParameters) {
  TestLibrary library(R"FIDL(
library example;

type Foo = struct {
  foo array;
};
)FIDL");

  library.ExpectFail(ErrWrongNumberOfLayoutParameters, "array", 2, 0);
  ASSERT_COMPILER_DIAGNOSTICS(library);
  EXPECT_EQ(library.errors()[0]->span.data(), "array");
}

TEST(TypesTests, BadNotEnoughParameters) {
  TestLibrary library;
  library.AddFile("bad/fi-0162-a.test.fidl");
  library.ExpectFail(ErrWrongNumberOfLayoutParameters, "array", 2, 1);
  ASSERT_COMPILER_DIAGNOSTICS(library);
  EXPECT_EQ(library.errors()[0]->span.data(), "<8>");
}

TEST(TypesTests, BadTooManyConstraints) {
  TestLibrary library;
  library.AddFile("bad/fi-0164.test.fidl");
  library.ExpectFail(ErrTooManyConstraints, "string", 2, 3);
  ASSERT_COMPILER_DIAGNOSTICS(library);
  EXPECT_EQ(library.errors()[0]->span.data(), "<0, optional, 20>");
}

TEST(TypesTests, BadParameterizedAnonymousLayout) {
  TestLibrary library(R"FIDL(
library example;

type Foo = struct {
  bar struct {}<1>;
};
)FIDL");

  library.ExpectFail(ErrWrongNumberOfLayoutParameters, "Bar", 0, 1);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TypesTests, BadConstrainTwice) {
  TestLibrary library(R"FIDL(
library example;

using zx;

alias MyVmo = zx.Handle:VMO;

type Foo = struct {
    foo MyVmo:zx.ObjType.CHANNEL;
};
)FIDL");
  library.UseLibraryZx();

  // TODO(https://fxbug.dev/42153849): We plan to disallow constraints on aliases, so this
  // error message should change to that. For now, to test this we have to use
  // `zx.ObjType` above because contextual lookup is not done through aliases.
  library.ExpectFail(ErrCannotConstrainTwice, "MyVmo");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TypesTests, GoodNoOverlappingConstraints) {
  TestLibrary library(R"FIDL(
library example;

using zx;

alias MyVmo = zx.Handle:<VMO, zx.Rights.TRANSFER>;

type Foo = resource struct {
    foo MyVmo:optional;
};
)FIDL");
  library.UseLibraryZx();

  ASSERT_COMPILED(library);
}

TEST(TypesTests, BadWantTypeLayoutParameter) {
  TestLibrary library;
  library.AddFile("bad/fi-0165.test.fidl");
  library.ExpectFail(ErrExpectedType);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TypesTests, BadWantValueLayoutParameter) {
  TestLibrary library(R"FIDL(
library example;

type Foo = struct {
    foo array<uint8, uint8>;
};
)FIDL");

  library.ExpectFail(ErrExpectedValueButGotType, "uint8");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TypesTests, BadUnresolvableConstraint) {
  TestLibrary library;
  library.AddFile("bad/fi-0166.test.fidl");
  library.ExpectFail(ErrUnexpectedConstraint, "vector");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TypesTests, BadShadowedOptional) {
  TestLibrary library(R"FIDL(
library example;

const optional uint8 = 3;

type Foo = resource struct {
    foo vector<uint8>:<10, optional>;
};
)FIDL");

  library.ExpectFail(ErrUnexpectedConstraint, "vector");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TypesTests, BadWrongConstraintType) {
  TestLibrary library(R"FIDL(
library example;

type Foo = resource struct {
    foo vector<uint8>:"hello";
};
)FIDL");

  library.ExpectFail(ErrTypeCannotBeConvertedToType, "\"hello\"", "string:5", "uint32");
  library.ExpectFail(ErrCouldNotResolveSizeBound);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TypesTests, CannotReferToUnqualifiedInternalType) {
  TestLibrary library(R"FIDL(
library example;

type Foo = struct {
    foo FrameworkErr;
};
)FIDL");

  library.ExpectFail(ErrNameNotFound, "FrameworkErr", "library 'example'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TypesTests, CannotReferToQualifiedInternalType) {
  TestLibrary library(R"FIDL(
library example;

type Foo = struct {
    foo fidl.FrameworkErr;
};
)FIDL");

  library.ExpectFail(ErrNameNotFound, "FrameworkErr", "library 'fidl'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TypesTests, BadUsize64WithoutFlag) {
  TestLibrary library;
  library.AddFile("bad/fi-0180.test.fidl");
  library.ExpectFail(ErrExperimentalZxCTypesDisallowed, "usize64");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TypesTests, BadUintptr64WithoutFlag) {
  TestLibrary library("library example; alias T = uintptr64;");
  library.ExpectFail(ErrExperimentalZxCTypesDisallowed, "uintptr64");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TypesTests, BadUcharWithoutFlag) {
  TestLibrary library("library example; alias T = uchar;");
  library.ExpectFail(ErrExperimentalZxCTypesDisallowed, "uchar");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TypesTests, BadExperimentalPointerWithoutFlag) {
  TestLibrary library("library example; alias T = experimental_pointer<uint32>;");
  library.ExpectFail(ErrExperimentalZxCTypesDisallowed, "experimental_pointer");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TypesTests, GoodUsize64WithFlag) {
  TestLibrary library("library example; alias T = usize64;");
  library.EnableFlag(ExperimentalFlag::kZxCTypes);
  ASSERT_COMPILED(library);
}

TEST(TypesTests, GoodUintptr64WithFlag) {
  TestLibrary library("library example; alias T = uintptr64;");
  library.EnableFlag(ExperimentalFlag::kZxCTypes);
  ASSERT_COMPILED(library);
}

TEST(TypesTests, GoodUcharWithFlag) {
  TestLibrary library("library example; alias T = uchar;");
  library.EnableFlag(ExperimentalFlag::kZxCTypes);
  ASSERT_COMPILED(library);
}

TEST(TypesTests, GoodExperimentalPointerWithFlag) {
  TestLibrary library("library example; alias T = experimental_pointer<uint32>;");
  library.EnableFlag(ExperimentalFlag::kZxCTypes);
  ASSERT_COMPILED(library);
}

}  // namespace
}  // namespace fidlc
