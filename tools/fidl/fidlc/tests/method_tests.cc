// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/src/types.h"
#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

TEST(MethodTests, GoodValidComposeMethod) {
  TestLibrary library(R"FIDL(
library example;

open protocol HasComposeMethod1 {
    compose();
};

open protocol HasComposeMethod2 {
    compose() -> ();
};
)FIDL");
  ASSERT_COMPILED(library);

  auto protocol1 = library.LookupProtocol("HasComposeMethod1");
  ASSERT_NE(protocol1, nullptr);
  ASSERT_EQ(protocol1->methods.size(), 1u);
  EXPECT_EQ(protocol1->methods[0].strictness, Strictness::kFlexible);
  EXPECT_EQ(protocol1->all_methods.size(), 1u);

  auto protocol2 = library.LookupProtocol("HasComposeMethod2");
  ASSERT_NE(protocol2, nullptr);
  ASSERT_EQ(protocol2->methods.size(), 1u);
  EXPECT_EQ(protocol2->methods[0].strictness, Strictness::kFlexible);
  EXPECT_EQ(protocol2->all_methods.size(), 1u);
}

TEST(MethodTests, GoodValidStrictComposeMethod) {
  TestLibrary library(R"FIDL(
library example;

open protocol HasComposeMethod1 {
    strict compose();
};

open protocol HasComposeMethod2 {
    strict compose() -> ();
};
)FIDL");
  ASSERT_COMPILED(library);

  auto protocol1 = library.LookupProtocol("HasComposeMethod1");
  ASSERT_NE(protocol1, nullptr);
  ASSERT_EQ(protocol1->methods.size(), 1u);
  EXPECT_EQ(protocol1->methods[0].strictness, Strictness::kStrict);
  EXPECT_EQ(protocol1->all_methods.size(), 1u);

  auto protocol2 = library.LookupProtocol("HasComposeMethod2");
  ASSERT_NE(protocol2, nullptr);
  ASSERT_EQ(protocol2->methods.size(), 1u);
  EXPECT_EQ(protocol2->methods[0].strictness, Strictness::kStrict);
  EXPECT_EQ(protocol2->all_methods.size(), 1u);
}

TEST(MethodTests, GoodValidFlexibleComposeMethod) {
  TestLibrary library(R"FIDL(
library example;

open protocol HasComposeMethod1 {
    flexible compose();
};

open protocol HasComposeMethod2 {
    flexible compose() -> ();
};
)FIDL");
  ASSERT_COMPILED(library);

  auto protocol1 = library.LookupProtocol("HasComposeMethod1");
  ASSERT_NE(protocol1, nullptr);
  ASSERT_EQ(protocol1->methods.size(), 1u);
  EXPECT_EQ(protocol1->methods[0].strictness, Strictness::kFlexible);
  EXPECT_EQ(protocol1->all_methods.size(), 1u);

  auto protocol2 = library.LookupProtocol("HasComposeMethod2");
  ASSERT_NE(protocol2, nullptr);
  ASSERT_EQ(protocol2->methods.size(), 1u);
  EXPECT_EQ(protocol2->methods[0].strictness, Strictness::kFlexible);
  EXPECT_EQ(protocol2->all_methods.size(), 1u);
}

// TODO(https://fxbug.dev/348403275): The ability to name methods "strict" or "flexible"
// is temporarily regressed. Re-enable after we implement explicit method kind syntax.
TEST(MethodTests, DISABLED_GoodValidStrictMethod) {
  TestLibrary library(R"FIDL(
library example;

open protocol HasStrictMethod1 {
    strict();
};

open protocol HasStrictMethod2 {
    strict() -> ();
};

open protocol HasStrictMethod3 {
    strict strict();
};

open protocol HasStrictMethod4 {
    strict strict() -> ();
};

open protocol HasStrictMethod5 {
    flexible strict();
};

open protocol HasStrictMethod6 {
    flexible strict() -> ();
};
)FIDL");
  ASSERT_COMPILED(library);

  auto protocol1 = library.LookupProtocol("HasStrictMethod1");
  ASSERT_NE(protocol1, nullptr);
  ASSERT_EQ(protocol1->methods.size(), 1u);
  EXPECT_EQ(protocol1->methods[0].strictness, Strictness::kFlexible);
  EXPECT_EQ(protocol1->all_methods.size(), 1u);

  auto protocol2 = library.LookupProtocol("HasStrictMethod2");
  ASSERT_NE(protocol2, nullptr);
  ASSERT_EQ(protocol2->methods.size(), 1u);
  EXPECT_EQ(protocol2->methods[0].strictness, Strictness::kFlexible);
  EXPECT_EQ(protocol2->all_methods.size(), 1u);

  auto protocol3 = library.LookupProtocol("HasStrictMethod3");
  ASSERT_NE(protocol3, nullptr);
  ASSERT_EQ(protocol3->methods.size(), 1u);
  EXPECT_EQ(protocol3->methods[0].strictness, Strictness::kStrict);
  EXPECT_EQ(protocol3->all_methods.size(), 1u);

  auto protocol4 = library.LookupProtocol("HasStrictMethod4");
  ASSERT_NE(protocol4, nullptr);
  ASSERT_EQ(protocol4->methods.size(), 1u);
  EXPECT_EQ(protocol4->methods[0].strictness, Strictness::kStrict);
  EXPECT_EQ(protocol4->all_methods.size(), 1u);

  auto protocol5 = library.LookupProtocol("HasStrictMethod5");
  ASSERT_NE(protocol5, nullptr);
  ASSERT_EQ(protocol5->methods.size(), 1u);
  EXPECT_EQ(protocol5->methods[0].strictness, Strictness::kFlexible);
  EXPECT_EQ(protocol5->all_methods.size(), 1u);

  auto protocol6 = library.LookupProtocol("HasStrictMethod6");
  ASSERT_NE(protocol6, nullptr);
  ASSERT_EQ(protocol6->methods.size(), 1u);
  EXPECT_EQ(protocol6->methods[0].strictness, Strictness::kFlexible);
  EXPECT_EQ(protocol6->all_methods.size(), 1u);
}

// TODO(https://fxbug.dev/348403275): The ability to name methods "strict" or "flexible"
// is temporarily regressed. Re-enable after we implement explicit method kind syntax.
TEST(MethodTests, DISABLED_GoodValidFlexibleTwoWayMethod) {
  TestLibrary library(R"FIDL(
library example;

open protocol HasFlexibleTwoWayMethod1 {
    flexible();
};

open protocol HasFlexibleTwoWayMethod2 {
    flexible() -> ();
};

open protocol HasFlexibleTwoWayMethod3 {
    strict flexible();
};

open protocol HasFlexibleTwoWayMethod4 {
    strict flexible() -> ();
};

open protocol HasFlexibleTwoWayMethod5 {
    flexible flexible();
};

open protocol HasFlexibleTwoWayMethod6 {
    flexible flexible() -> ();
};
)FIDL");
  ASSERT_COMPILED(library);

  auto protocol1 = library.LookupProtocol("HasFlexibleTwoWayMethod1");
  ASSERT_NE(protocol1, nullptr);
  ASSERT_EQ(protocol1->methods.size(), 1u);
  EXPECT_EQ(protocol1->methods[0].strictness, Strictness::kFlexible);
  EXPECT_EQ(protocol1->all_methods.size(), 1u);

  auto protocol2 = library.LookupProtocol("HasFlexibleTwoWayMethod2");
  ASSERT_NE(protocol2, nullptr);
  ASSERT_EQ(protocol2->methods.size(), 1u);
  EXPECT_EQ(protocol2->methods[0].strictness, Strictness::kFlexible);
  EXPECT_EQ(protocol2->all_methods.size(), 1u);

  auto protocol3 = library.LookupProtocol("HasFlexibleTwoWayMethod3");
  ASSERT_NE(protocol3, nullptr);
  ASSERT_EQ(protocol3->methods.size(), 1u);
  EXPECT_EQ(protocol3->methods[0].strictness, Strictness::kStrict);
  EXPECT_EQ(protocol3->all_methods.size(), 1u);

  auto protocol4 = library.LookupProtocol("HasFlexibleTwoWayMethod4");
  ASSERT_NE(protocol4, nullptr);
  ASSERT_EQ(protocol4->methods.size(), 1u);
  EXPECT_EQ(protocol4->methods[0].strictness, Strictness::kStrict);
  EXPECT_EQ(protocol4->all_methods.size(), 1u);

  auto protocol5 = library.LookupProtocol("HasFlexibleTwoWayMethod5");
  ASSERT_NE(protocol5, nullptr);
  ASSERT_EQ(protocol5->methods.size(), 1u);
  EXPECT_EQ(protocol5->methods[0].strictness, Strictness::kFlexible);
  EXPECT_EQ(protocol5->all_methods.size(), 1u);

  auto protocol6 = library.LookupProtocol("HasFlexibleTwoWayMethod6");
  ASSERT_NE(protocol6, nullptr);
  ASSERT_EQ(protocol6->methods.size(), 1u);
  EXPECT_EQ(protocol6->methods[0].strictness, Strictness::kFlexible);
  EXPECT_EQ(protocol6->all_methods.size(), 1u);
}

TEST(MethodTests, GoodValidNormalMethod) {
  TestLibrary library(R"FIDL(
library example;

open protocol HasNormalMethod1 {
    MyMethod();
};

open protocol HasNormalMethod2 {
    MyMethod() -> ();
};
)FIDL");
  ASSERT_COMPILED(library);

  auto protocol1 = library.LookupProtocol("HasNormalMethod1");
  ASSERT_NE(protocol1, nullptr);
  ASSERT_EQ(protocol1->methods.size(), 1u);
  EXPECT_EQ(protocol1->methods[0].strictness, Strictness::kFlexible);
  EXPECT_EQ(protocol1->all_methods.size(), 1u);

  auto protocol2 = library.LookupProtocol("HasNormalMethod2");
  ASSERT_NE(protocol2, nullptr);
  ASSERT_EQ(protocol2->methods.size(), 1u);
  EXPECT_EQ(protocol2->methods[0].strictness, Strictness::kFlexible);
  EXPECT_EQ(protocol2->all_methods.size(), 1u);
}

TEST(MethodTests, GoodValidStrictNormalMethod) {
  TestLibrary library(R"FIDL(
library example;

open protocol HasNormalMethod1 {
    strict MyMethod();
};

open protocol HasNormalMethod2 {
    strict MyMethod() -> ();
};
)FIDL");
  ASSERT_COMPILED(library);

  auto protocol1 = library.LookupProtocol("HasNormalMethod1");
  ASSERT_NE(protocol1, nullptr);
  ASSERT_EQ(protocol1->methods.size(), 1u);
  EXPECT_EQ(protocol1->methods[0].strictness, Strictness::kStrict);
  EXPECT_EQ(protocol1->all_methods.size(), 1u);

  auto protocol2 = library.LookupProtocol("HasNormalMethod2");
  ASSERT_NE(protocol2, nullptr);
  ASSERT_EQ(protocol2->methods.size(), 1u);
  EXPECT_EQ(protocol2->methods[0].strictness, Strictness::kStrict);
  EXPECT_EQ(protocol2->all_methods.size(), 1u);
}

TEST(MethodTests, GoodValidFlexibleNormalMethod) {
  TestLibrary library(R"FIDL(
library example;

open protocol HasNormalMethod1 {
    flexible MyMethod();
};

open protocol HasNormalMethod2 {
    flexible MyMethod() -> ();
};
)FIDL");
  ASSERT_COMPILED(library);

  auto protocol1 = library.LookupProtocol("HasNormalMethod1");
  ASSERT_NE(protocol1, nullptr);
  ASSERT_EQ(protocol1->methods.size(), 1u);
  EXPECT_EQ(protocol1->methods[0].strictness, Strictness::kFlexible);
  EXPECT_EQ(protocol1->all_methods.size(), 1u);

  auto protocol2 = library.LookupProtocol("HasNormalMethod2");
  ASSERT_NE(protocol2, nullptr);
  ASSERT_EQ(protocol2->methods.size(), 1u);
  EXPECT_EQ(protocol2->methods[0].strictness, Strictness::kFlexible);
  EXPECT_EQ(protocol2->all_methods.size(), 1u);
}

TEST(MethodTests, GoodValidEvent) {
  TestLibrary library(R"FIDL(
library example;

protocol HasEvent {
    -> MyEvent();
};
)FIDL");
  ASSERT_COMPILED(library);

  auto protocol = library.LookupProtocol("HasEvent");
  ASSERT_NE(protocol, nullptr);
  ASSERT_EQ(protocol->methods.size(), 1u);
  EXPECT_EQ(protocol->methods[0].strictness, Strictness::kFlexible);
  EXPECT_EQ(protocol->all_methods.size(), 1u);
}

TEST(MethodTests, GoodValidStrictEvent) {
  TestLibrary library(R"FIDL(
library example;

protocol HasEvent {
    strict -> MyMethod();
};
)FIDL");
  ASSERT_COMPILED(library);

  auto protocol = library.LookupProtocol("HasEvent");
  ASSERT_NE(protocol, nullptr);
  ASSERT_EQ(protocol->methods.size(), 1u);
  EXPECT_EQ(protocol->methods[0].strictness, Strictness::kStrict);
  EXPECT_EQ(protocol->all_methods.size(), 1u);
}

TEST(MethodTests, GoodValidFlexibleEvent) {
  TestLibrary library(R"FIDL(
library example;

protocol HasEvent {
    flexible -> MyMethod();
};
)FIDL");

  ASSERT_COMPILED(library);

  auto protocol = library.LookupProtocol("HasEvent");
  ASSERT_NE(protocol, nullptr);
  ASSERT_EQ(protocol->methods.size(), 1u);
  EXPECT_EQ(protocol->methods[0].strictness, Strictness::kFlexible);
  EXPECT_EQ(protocol->all_methods.size(), 1u);
}

TEST(MethodTests, GoodValidStrictnessModifiers) {
  TestLibrary library(R"FIDL(
library example;

closed protocol Closed {
  strict StrictOneWay();
  strict StrictTwoWay() -> ();
  strict -> StrictEvent();
};

ajar protocol Ajar {
  strict StrictOneWay();
  flexible FlexibleOneWay();

  strict StrictTwoWay() -> ();

  strict -> StrictEvent();
  flexible -> FlexibleEvent();
};

open protocol Open {
  strict StrictOneWay();
  flexible FlexibleOneWay();

  strict StrictTwoWay() -> ();
  flexible FlexibleTwoWay() -> ();

  strict -> StrictEvent();
  flexible -> FlexibleEvent();
};
)FIDL");
  ASSERT_COMPILED(library);

  auto closed = library.LookupProtocol("Closed");
  ASSERT_NE(closed, nullptr);
  ASSERT_EQ(closed->methods.size(), 3u);

  auto ajar = library.LookupProtocol("Ajar");
  ASSERT_NE(ajar, nullptr);
  ASSERT_EQ(ajar->methods.size(), 5u);

  auto open = library.LookupProtocol("Open");
  ASSERT_NE(open, nullptr);
  ASSERT_EQ(open->methods.size(), 6u);
}

TEST(MethodTests, BadInvalidStrictnessFlexibleEventInClosed) {
  TestLibrary library(R"FIDL(
library example;

closed protocol Closed {
  flexible -> Event();
};
)FIDL");
  library.ExpectFail(ErrFlexibleOneWayMethodInClosedProtocol, "event");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(MethodTests, BadInvalidStrictnessFlexibleOneWayMethodInClosed) {
  TestLibrary library;
  library.AddFile("bad/fi-0116.test.fidl");
  library.ExpectFail(ErrFlexibleOneWayMethodInClosedProtocol, "one-way method");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(MethodTests, BadInvalidStrictnessFlexibleTwoWayMethodInClosed) {
  TestLibrary library(R"FIDL(
library example;

closed protocol Closed {
  flexible Method() -> ();
};
)FIDL");
  library.ExpectFail(ErrFlexibleTwoWayMethodRequiresOpenProtocol, Openness::kClosed);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(MethodTests, BadInvalidStrictnessFlexibleTwoWayMethodInAjar) {
  TestLibrary library;
  library.AddFile("bad/fi-0115.test.fidl");
  library.ExpectFail(ErrFlexibleTwoWayMethodRequiresOpenProtocol, Openness::kAjar);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(MethodTests, BadInvalidOpennessModifierOnMethod) {
  TestLibrary library(R"FIDL(
library example;

protocol BadMethod {
    open Method();
};
)FIDL");
  library.ExpectFail(ErrInvalidProtocolMember);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(MethodTests, GoodValidEmptyPayloads) {
  TestLibrary library(R"FIDL(
library example;

open protocol Test {
  strict MethodA() -> ();
  flexible MethodB() -> ();
  strict MethodC() -> () error int32;
  flexible MethodD() -> () error int32;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto closed = library.LookupProtocol("Test");
  ASSERT_NE(closed, nullptr);
  ASSERT_EQ(closed->methods.size(), 4u);
}

TEST(MethodTests, BadInvalidEmptyStructPayloadStrictNoError) {
  TestLibrary library(R"FIDL(
library example;

open protocol Test {
  strict Method() -> (struct {});
};
)FIDL");
  library.ExpectFail(ErrEmptyPayloadStructs);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(MethodTests, BadEmptyStructPayloadFlexibleNoError) {
  TestLibrary library(R"FIDL(
library example;

open protocol Test {
  flexible Method() -> (struct {});
};
)FIDL");
  library.ExpectFail(ErrEmptyPayloadStructs);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(MethodTests, BadEmptyStructPayloadStrictError) {
  TestLibrary library(R"FIDL(
library example;

open protocol Test {
  strict Method() -> (struct {}) error int32;
};
)FIDL");
  library.ExpectFail(ErrEmptyPayloadStructs);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(MethodTests, BadEmptyStructPayloadFlexibleError) {
  TestLibrary library(R"FIDL(
library example;

open protocol Test {
  flexible Method() -> (struct {}) error int32;
};
)FIDL");
  library.ExpectFail(ErrEmptyPayloadStructs);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(MethodTests, GoodAbsentPayloadFlexibleNoError) {
  TestLibrary library(R"FIDL(
library example;

open protocol Test {
  flexible Method() -> ();
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(MethodTests, GoodAbsentPayloadStrictError) {
  TestLibrary library(R"FIDL(
library example;

open protocol Test {
  strict Method() -> () error int32;
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(MethodTests, GoodAbsentPayloadFlexibleError) {
  TestLibrary library(R"FIDL(
library example;

open protocol Test {
  flexible Method() -> () error int32;
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(MethodTests, GoodFlexibleNoErrorResponseUnion) {
  TestLibrary library(R"FIDL(
library example;

open protocol Example {
    flexible Method() -> (struct {
        foo string;
    });
};
)FIDL");
  ASSERT_COMPILED(library);

  auto methods = &library.LookupProtocol("Example")->methods;
  ASSERT_EQ(methods->size(), 1u);
  auto method = &methods->at(0);
  auto response = method->maybe_response.get();
  ASSERT_NE(response, nullptr);

  ASSERT_EQ(response->type->kind, Type::Kind::kIdentifier);
  auto id = static_cast<const IdentifierType*>(response->type);
  ASSERT_EQ(id->type_decl->kind, Decl::Kind::kUnion);
  auto result_union = static_cast<const Union*>(id->type_decl);
  ASSERT_NE(result_union, nullptr);
  ASSERT_EQ(result_union->members.size(), 2u);

  auto anonymous = result_union->name.as_anonymous();
  ASSERT_NE(anonymous, nullptr);
  ASSERT_EQ(anonymous->provenance, Name::Provenance::kGeneratedResultUnion);

  const auto& success = result_union->members.at(0);
  ASSERT_EQ(success.ordinal->value, 1u);
  ASSERT_EQ(success.name.data(), "response");

  const Union::Member& framework_error = result_union->members.at(1);
  ASSERT_EQ(framework_error.ordinal->value, 3u);
  ASSERT_EQ(framework_error.name.data(), "framework_err");
  ASSERT_NE(framework_error.type_ctor->type, nullptr);
  ASSERT_EQ(framework_error.type_ctor->type->kind, Type::Kind::kInternal);
  auto framework_err_internal_type =
      static_cast<const InternalType*>(framework_error.type_ctor->type);
  ASSERT_EQ(framework_err_internal_type->subtype, InternalSubtype::kFrameworkErr);
}

TEST(MethodTests, GoodFlexibleErrorResponseUnion) {
  TestLibrary library(R"FIDL(
library example;

open protocol Example {
    flexible Method() -> (struct {
        foo string;
    }) error uint32;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto methods = &library.LookupProtocol("Example")->methods;
  ASSERT_EQ(methods->size(), 1u);
  auto method = &methods->at(0);
  auto response = method->maybe_response.get();
  ASSERT_NE(response, nullptr);

  ASSERT_EQ(response->type->kind, Type::Kind::kIdentifier);
  auto id = static_cast<const IdentifierType*>(response->type);
  ASSERT_EQ(id->type_decl->kind, Decl::Kind::kUnion);
  auto result_union = static_cast<const Union*>(id->type_decl);
  ASSERT_NE(result_union, nullptr);
  ASSERT_EQ(result_union->members.size(), 3u);

  auto anonymous = result_union->name.as_anonymous();
  ASSERT_NE(anonymous, nullptr);
  ASSERT_EQ(anonymous->provenance, Name::Provenance::kGeneratedResultUnion);

  const auto& success = result_union->members.at(0);
  ASSERT_EQ(success.ordinal->value, 1u);
  ASSERT_EQ(success.name.data(), "response");

  const Union::Member& error = result_union->members.at(1);
  ASSERT_EQ(error.ordinal->value, 2u);
  ASSERT_EQ(error.name.data(), "err");
  ASSERT_NE(error.type_ctor->type, nullptr);
  ASSERT_EQ(error.type_ctor->type->kind, Type::Kind::kPrimitive);
  auto err_primitive_type = static_cast<const PrimitiveType*>(error.type_ctor->type);
  ASSERT_EQ(err_primitive_type->subtype, PrimitiveSubtype::kUint32);

  const Union::Member& framework_error = result_union->members.at(2);
  ASSERT_EQ(framework_error.ordinal->value, 3u);
  ASSERT_EQ(framework_error.name.data(), "framework_err");
  ASSERT_NE(framework_error.type_ctor->type, nullptr);
  ASSERT_EQ(framework_error.type_ctor->type->kind, Type::Kind::kInternal);
  auto framework_err_internal_type =
      static_cast<const InternalType*>(framework_error.type_ctor->type);
  ASSERT_EQ(framework_err_internal_type->subtype, InternalSubtype::kFrameworkErr);
}
}  // namespace
}  // namespace fidlc
