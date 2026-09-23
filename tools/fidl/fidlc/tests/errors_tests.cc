// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/types.h"
#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

TEST(ErrorsTests, GoodError) {
  TestLibrary library(R"FIDL(
library example;

protocol Example {
    strict Method() -> (struct {
        foo string;
    }) error int32;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto methods = &library.LookupProtocol("Example")->methods;
  ASSERT_EQ(methods->size(), 1u);
  auto method = &methods->at(0);
  auto response = method->maybe_response.get();
  ASSERT_NE(response, nullptr);

  auto id = static_cast<const IdentifierType*>(response->type);
  auto result_union = static_cast<const Union*>(id->type_decl);
  ASSERT_NE(result_union, nullptr);
  ASSERT_EQ(result_union->members.size(), 2u);

  auto anonymous = result_union->name.as_anonymous();
  ASSERT_NE(anonymous, nullptr);
  ASSERT_EQ(anonymous->provenance, Name::Provenance::kGeneratedResultUnion);

  const auto& success = result_union->members.at(0);
  ASSERT_EQ("response", success.name.data());

  const Union::Member& error = result_union->members.at(1);
  ASSERT_EQ("err", error.name.data());

  ASSERT_EQ(error.type_ctor->type->kind, Type::Kind::kPrimitive);
  auto primitive_type = static_cast<const PrimitiveType*>(error.type_ctor->type);
  ASSERT_EQ(primitive_type->subtype, PrimitiveSubtype::kInt32);
}

TEST(ErrorsTests, GoodErrorUnsigned) {
  TestLibrary library(R"FIDL(
library example;

protocol Example {
    Method() -> (struct {
        foo string;
    }) error uint32;
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ErrorsTests, GoodErrorEmptyStructAsSuccess) {
  TestLibrary library(R"FIDL(
library example;

protocol MyProtocol {
  strict MyMethod() -> () error uint32;
};
)FIDL");
  ASSERT_COMPILED(library);
  auto protocol = library.LookupProtocol("MyProtocol");
  ASSERT_NE(protocol, nullptr);
  ASSERT_EQ(protocol->methods.size(), 1u);

  auto& method = protocol->methods[0];
  EXPECT_EQ(method.kind, Protocol::Method::Kind::kTwoWay);
  EXPECT_EQ(method.maybe_request, nullptr);
  ASSERT_NE(method.maybe_response, nullptr);

  auto id = static_cast<const IdentifierType*>(method.maybe_response->type);
  auto response = static_cast<const Union*>(id->type_decl);
  EXPECT_TRUE(response->kind == Decl::Kind::kUnion);
  ASSERT_EQ(response->members.size(), 2u);

  auto empty_struct_name = response->members[0].type_ctor->type->name.decl_name();
  auto empty_struct = library.LookupStruct(empty_struct_name);
  ASSERT_NE(empty_struct, nullptr);
  auto anonymous = empty_struct->name.as_anonymous();
  ASSERT_EQ(anonymous->provenance, Name::Provenance::kGeneratedEmptySuccessStruct);
}

TEST(ErrorsTests, GoodErrorEnum) {
  TestLibrary library(R"FIDL(
library example;

type ErrorType = enum : int32 {
    GOOD = 1;
    BAD = 2;
    UGLY = 3;
};

protocol Example {
    Method() -> (struct {
        foo string;
    }) error ErrorType;
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ErrorsTests, GoodErrorEnumAfter) {
  TestLibrary library(R"FIDL(
library example;

protocol Example {
    Method() -> (struct {
        foo string;
    }) error ErrorType;
};

type ErrorType = enum : int32 {
    GOOD = 1;
    BAD = 2;
    UGLY = 3;
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ErrorsTests, BadErrorUnknownIdentifier) {
  TestLibrary library;
  library.AddFile("bad/fi-0052.test.fidl");

  library.ExpectFail(ErrNameNotFound, "ParsingError", "library 'test.bad.fi0052'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ErrorsTests, BadErrorWrongPrimitive) {
  TestLibrary library;
  library.AddFile("bad/fi-0141.test.fidl");

  library.ExpectFail(ErrInvalidErrorType);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ErrorsTests, BadErrorMissingType) {
  TestLibrary library(R"FIDL(
library example;
protocol Example {
    Method() -> (flub int32) error;
};
)FIDL");
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kIdentifier),
                     Token::KindAndSubkind(Token::Kind::kRightParen));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ErrorsTests, BadErrorNotAType) {
  TestLibrary library(R"FIDL(
library example;
protocol Example {
    Method() -> (flub int32) error "hello";
};
)FIDL");
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kIdentifier),
                     Token::KindAndSubkind(Token::Kind::kRightParen));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ErrorsTests, BadErrorNoResponse) {
  TestLibrary library(R"FIDL(
library example;
protocol Example {
    Method() -> error int32;
};
)FIDL");
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Subkind::kError),
                     Token::KindAndSubkind(Token::Kind::kLeftParen));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ErrorsTests, BadErrorUnexpectedEndOfFile) {
  TestLibrary library(R"FIDL(
library example;
type ForgotTheSemicolon = table {}
)FIDL");

  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kEndOfFile),
                     Token::KindAndSubkind(Token::Kind::kSemicolon));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ErrorsTests, BadIncorrectIdentifier) {
  TestLibrary library;
  library.AddFile("bad/fi-0009.noformat.test.fidl");
  library.ExpectFail(ErrUnexpectedIdentifier, Token::KindAndSubkind(Token::Subkind::kUsing),
                     Token::KindAndSubkind(Token::Subkind::kLibrary));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ErrorsTests, BadErrorEmptyFile) {
  TestLibrary library("");

  library.ExpectFail(ErrUnexpectedIdentifier, Token::KindAndSubkind(Token::Kind::kEndOfFile),
                     Token::KindAndSubkind(Token::Subkind::kLibrary));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ErrorsTests, ExperimentalAllowArbitraryErrorTypes) {
  TestLibrary library(R"FIDL(
library example;
protocol Example {
    Method() -> () error table {};
};
)FIDL");
  library.EnableFlag(ExperimentalFlag::kAllowArbitraryErrorTypes);
  ASSERT_COMPILED(library);

  auto result_id = static_cast<const IdentifierType*>(
      library.LookupProtocol("Example")->methods.at(0).maybe_response->type);
  auto result_union = static_cast<const Union*>(result_id->type_decl);
  auto error_id = static_cast<const IdentifierType*>(result_union->members.at(1).type_ctor->type);
  ASSERT_EQ(error_id->type_decl->kind, Decl::Kind::kTable);
}

TEST(ErrorsTests, TransitionalRemoved) {
  TestLibrary library(R"FIDL(
library example;
protocol Example {
    @transitional
    Method();
};
)FIDL");
  library.ExpectFail(ErrDeprecatedAttribute, "transitional");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

}  // namespace
}  // namespace fidlc
