// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

TEST(HandleTests, GoodHandleRightsTest) {
  TestLibrary library(R"FIDL(
library example;

using zx;

type MyStruct = resource struct {
    h zx.Handle:<THREAD, zx.Rights.DUPLICATE | zx.Rights.TRANSFER>;
};
)FIDL");
  library.UseLibraryZx();
  ASSERT_COMPILED(library);

  const auto& h_type_ctor = library.LookupStruct("MyStruct")->members[0].type_ctor;

  ASSERT_TRUE(h_type_ctor->resolved_params.subtype_raw != nullptr);
  EXPECT_EQ("THREAD", h_type_ctor->resolved_params.subtype_raw->span.data());

  auto h_type = h_type_ctor->type;
  ASSERT_NE(h_type, nullptr);
  ASSERT_EQ(h_type->kind, Type::Kind::kHandle);
  auto handle_type = static_cast<const HandleType*>(h_type);

  EXPECT_EQ(HandleSubtype::kThread, handle_type->subtype);
  EXPECT_EQ(static_cast<const NumericConstantValue<uint32_t>*>(handle_type->rights)->value, 3u);
}

TEST(HandleTests, GoodNoHandleRightsTest) {
  TestLibrary library(R"FIDL(
library example;

using zx;

type MyStruct = resource struct {
    h zx.Handle:VMO;
};
)FIDL");
  library.UseLibraryZx();

  ASSERT_COMPILED(library);

  const auto& h_type_ctor = library.LookupStruct("MyStruct")->members[0].type_ctor;
  auto h_type = h_type_ctor->type;
  ASSERT_NE(h_type, nullptr);
  ASSERT_EQ(h_type->kind, Type::Kind::kHandle);
  auto handle_type = static_cast<const HandleType*>(h_type);

  ASSERT_TRUE(h_type_ctor->resolved_params.subtype_raw != nullptr);
  EXPECT_EQ("VMO", h_type_ctor->resolved_params.subtype_raw->span.data());
  EXPECT_EQ(HandleSubtype::kVmo, handle_type->subtype);
  EXPECT_EQ(static_cast<const NumericConstantValue<uint32_t>*>(handle_type->rights)->value,
            kHandleSameRights);
}

TEST(HandleTests, BadInvalidHandleRightsTest) {
  TestLibrary library(R"FIDL(
library example;

using zx;

protocol P {
    Method(struct { h zx.Handle:<VMO, 1>; });  // rights must be zx.Rights-typed.
};
)FIDL");
  library.UseLibraryZx();

  // NOTE(https://fxbug.dev/42152439): we provide a more general error because there are multiple
  // possible interpretations.
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "1", "untyped numeric", "zx/Rights");
  library.ExpectFail(ErrUnexpectedConstraint, "Handle");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(HandleTests, GoodPlainHandleTest) {
  TestLibrary library(R"FIDL(
library example;

using zx;

type MyStruct = resource struct {
    h zx.Handle;
};
)FIDL");
  library.UseLibraryZx();
  ASSERT_COMPILED(library);

  const auto& h_type_ctor = library.LookupStruct("MyStruct")->members[0].type_ctor;

  auto h_type = h_type_ctor->type;
  ASSERT_NE(h_type, nullptr);
  ASSERT_EQ(h_type->kind, Type::Kind::kHandle);
  auto handle_type = static_cast<const HandleType*>(h_type);

  EXPECT_EQ(HandleSubtype::kHandle, handle_type->subtype);
  EXPECT_EQ(static_cast<const NumericConstantValue<uint32_t>*>(handle_type->rights)->value,
            kHandleSameRights);
}

TEST(HandleTests, GoodHandleFidlDefinedTest) {
  TestLibrary library(R"FIDL(
library example;

using zx;

type MyStruct = resource struct {
  a zx.Handle:THREAD;
  b zx.Handle:<PROCESS>;
  c zx.Handle:<VMO, zx.Rights.TRANSFER>;
};
)FIDL");
  library.UseLibraryZx();

  ASSERT_COMPILED(library);
  const auto& a = library.LookupStruct("MyStruct")->members[0].type_ctor;
  auto a_type = a->type;
  ASSERT_NE(a_type, nullptr);
  ASSERT_EQ(a_type->kind, Type::Kind::kHandle);
  auto a_handle_type = static_cast<const HandleType*>(a_type);
  EXPECT_EQ(HandleSubtype::kThread, a_handle_type->subtype);
  EXPECT_EQ(static_cast<const HandleRightsValue*>(a_handle_type->rights)->value, kHandleSameRights);

  const auto& b = library.LookupStruct("MyStruct")->members[1].type_ctor;
  auto b_type = b->type;
  ASSERT_NE(b_type, nullptr);
  ASSERT_EQ(b_type->kind, Type::Kind::kHandle);
  auto b_handle_type = static_cast<const HandleType*>(b_type);
  EXPECT_EQ(HandleSubtype::kProcess, b_handle_type->subtype);
  EXPECT_EQ(static_cast<const HandleRightsValue*>(b_handle_type->rights)->value, kHandleSameRights);

  const auto& c = library.LookupStruct("MyStruct")->members[2].type_ctor;
  auto c_type = c->type;
  ASSERT_NE(c_type, nullptr);
  ASSERT_EQ(c_type->kind, Type::Kind::kHandle);
  auto c_handle_type = static_cast<const HandleType*>(c_type);
  EXPECT_EQ(HandleSubtype::kVmo, c_handle_type->subtype);
  ASSERT_NE(c_handle_type->rights, nullptr);
  EXPECT_EQ(static_cast<const HandleRightsValue*>(c_handle_type->rights)->value, 2u);
}

TEST(HandleTests, BadInvalidFidlDefinedHandleSubtype) {
  TestLibrary library(R"FIDL(
library example;

using zx;

type MyStruct = struct {
  a zx.Handle:ZIPPY;
};
)FIDL");
  library.UseLibraryZx();

  library.ExpectFail(ErrNameNotFound, "ZIPPY", "library 'example'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(HandleTests, BadDisallowOldHandles) {
  TestLibrary library(R"FIDL(
library example;

using zx;

type MyStruct = struct {
    h handle<vmo>;
};
)FIDL");
  library.UseLibraryZx();

  library.ExpectFail(ErrNameNotFound, "handle", "library 'example'");
  library.ExpectFail(ErrNameNotFound, "vmo", "library 'example'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(HandleTests, GoodResourceDefinitionOnlySubtypeNoRightsTest) {
  TestLibrary library(R"FIDL(
library example;

type ObjType = strict enum : uint32 {
    NONE = 0;
    VMO = 3;
};

resource_definition handle : uint32 {
    properties {
        subtype ObjType;
    };
};

type MyStruct = resource struct {
    h handle:VMO;
};
)FIDL");

  ASSERT_COMPILED(library);

  const auto& h_type_ctor = library.LookupStruct("MyStruct")->members[0].type_ctor;
  auto h_type = h_type_ctor->type;
  ASSERT_NE(h_type, nullptr);
  ASSERT_EQ(h_type->kind, Type::Kind::kHandle);
  auto handle_type = static_cast<const HandleType*>(h_type);

  ASSERT_TRUE(h_type_ctor->resolved_params.subtype_raw != nullptr);
  EXPECT_TRUE(h_type_ctor->resolved_params.subtype_raw->span.data() == "VMO");
  EXPECT_EQ(HandleSubtype::kVmo, handle_type->subtype);
  EXPECT_EQ(static_cast<const NumericConstantValue<uint32_t>*>(handle_type->rights)->value,
            kHandleSameRights);
}

TEST(HandleTests, BadInvalidSubtypeAtUseSite) {
  TestLibrary library(R"FIDL(
library example;

type ObjType = enum : uint32 {
    NONE = 0;
    VMO = 3;
};

resource_definition handle : uint32 {
    properties {
        subtype ObjType;
    };
};

type MyStruct = resource struct {
    h handle:<1, optional>;
};
)FIDL");

  library.ExpectFail(ErrTypeCannotBeConvertedToType, "1", "untyped numeric", "example/ObjType");
  library.ExpectFail(ErrUnexpectedConstraint, "handle");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(HandleTests, BadInvalidRightsAtUseSite) {
  TestLibrary library(R"FIDL(
library example;

type ObjType = enum : uint32 {
    NONE = 0;
    VMO = 3;
};

resource_definition handle : uint32 {
    properties {
        subtype ObjType;
        rights uint32;
    };
};

type MyStruct = resource struct {
    h handle:<VMO, "my_improperly_typed_rights", optional>;
};
)FIDL");

  library.ExpectFail(ErrTypeCannotBeConvertedToType, "\"my_improperly_typed_rights\"", "string:26",
                     "uint32");
  library.ExpectFail(ErrUnexpectedConstraint, "handle");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(HandleTests, BadBareHandleNoConstraints) {
  TestLibrary library(R"FIDL(
library example;

type MyStruct = resource struct {
    h handle;
};
)FIDL");
  library.ExpectFail(ErrNameNotFound, "handle", "library 'example'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(HandleTests, BadBareHandleWithConstraints) {
  TestLibrary library(R"FIDL(
library example;

type MyStruct = resource struct {
    h handle:VMO;
};
)FIDL");
  library.ExpectFail(ErrNameNotFound, "handle", "library 'example'");
  library.ExpectFail(ErrNameNotFound, "VMO", "library 'example'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(HandleTests, BadBareHandleWithConstraintsThroughAlias) {
  TestLibrary library(R"FIDL(
library example;

alias my_handle = handle;

type MyStruct = resource struct {
    h my_handle:VMO;
};
)FIDL");
  library.ExpectFail(ErrNameNotFound, "handle", "library 'example'");
  library.ExpectFail(ErrNameNotFound, "VMO", "library 'example'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

}  // namespace
}  // namespace fidlc
