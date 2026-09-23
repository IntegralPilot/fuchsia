// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/attribute_schema.h"
#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

TEST(AttributesTests, GoodPlacementOfAttributes) {
  SharedAmongstLibraries shared;
  TestLibrary dependency(&shared, "exampleusing.fidl", R"FIDL(
library exampleusing;

@on_dep_struct
type Empty = struct {};
)FIDL");
  ASSERT_COMPILED(dependency);

  TestLibrary library(&shared, "example.fidl", R"FIDL(
@on_library
library example;

using exampleusing;

@on_bits
type ExampleBits = bits {
    @on_bits_member
    MEMBER = 1;
};

@on_const
const EXAMPLE_CONST uint32 = 0;

@on_enum
type ExampleEnum = enum {
    @on_enum_member
    MEMBER = 1;
};

@on_protocol
protocol ExampleChildProtocol {
    @on_method
    Method(struct { @on_parameter arg exampleusing.Empty; });
};

@on_protocol
protocol ExampleParentProtocol {
    @on_compose
    compose ExampleChildProtocol;
};

@on_service
service ExampleService {
    @on_service_member
    member client_end:ExampleParentProtocol;
};

@on_struct
type ExampleStruct = struct {
    @on_struct_member
    member uint32;
};

@on_table
type ExampleTable = table {
    @on_table_member
    1: member uint32;
};

@on_alias
alias ExampleAlias = uint32;

@on_union
type ExampleUnion = union {
    @on_union_member
    1: variant uint32;
};
)FIDL");
  ASSERT_COMPILED(library);

  EXPECT_TRUE(library.attributes()->Get("on_library"));

  auto example_bits = library.LookupBits("ExampleBits");
  ASSERT_NE(example_bits, nullptr);
  EXPECT_TRUE(example_bits->attributes->Get("on_bits"));
  EXPECT_TRUE(example_bits->members.front().attributes->Get("on_bits_member"));

  auto example_const = library.LookupConstant("EXAMPLE_CONST");
  ASSERT_NE(example_const, nullptr);
  EXPECT_TRUE(example_const->attributes->Get("on_const"));

  auto example_enum = library.LookupEnum("ExampleEnum");
  ASSERT_NE(example_enum, nullptr);
  EXPECT_TRUE(example_enum->attributes->Get("on_enum"));
  EXPECT_TRUE(example_enum->members.front().attributes->Get("on_enum_member"));

  auto example_child_protocol = library.LookupProtocol("ExampleChildProtocol");
  ASSERT_NE(example_child_protocol, nullptr);
  EXPECT_TRUE(example_child_protocol->attributes->Get("on_protocol"));
  EXPECT_TRUE(example_child_protocol->methods.front().attributes->Get("on_method"));
  ASSERT_NE(example_child_protocol->methods.front().maybe_request, nullptr);

  auto id = static_cast<const IdentifierType*>(
      example_child_protocol->methods.front().maybe_request->type);
  auto as_struct = static_cast<const Struct*>(id->type_decl);
  EXPECT_TRUE(as_struct->members.front().attributes->Get("on_parameter"));

  auto example_parent_protocol = library.LookupProtocol("ExampleParentProtocol");
  ASSERT_NE(example_parent_protocol, nullptr);
  EXPECT_TRUE(example_parent_protocol->attributes->Get("on_protocol"));
  EXPECT_TRUE(example_parent_protocol->composed_protocols.front().attributes->Get("on_compose"));

  auto example_service = library.LookupService("ExampleService");
  ASSERT_NE(example_service, nullptr);
  EXPECT_TRUE(example_service->attributes->Get("on_service"));
  EXPECT_TRUE(example_service->members.front().attributes->Get("on_service_member"));

  auto example_struct = library.LookupStruct("ExampleStruct");
  ASSERT_NE(example_struct, nullptr);
  EXPECT_TRUE(example_struct->attributes->Get("on_struct"));
  EXPECT_TRUE(example_struct->members.front().attributes->Get("on_struct_member"));

  auto example_table = library.LookupTable("ExampleTable");
  ASSERT_NE(example_table, nullptr);
  EXPECT_TRUE(example_table->attributes->Get("on_table"));
  EXPECT_TRUE(example_table->members.front().attributes->Get("on_table_member"));

  auto example_alias = library.LookupAlias("ExampleAlias");
  ASSERT_NE(example_alias, nullptr);
  EXPECT_TRUE(example_alias->attributes->Get("on_alias"));

  auto example_union = library.LookupUnion("ExampleUnion");
  ASSERT_NE(example_union, nullptr);
  EXPECT_TRUE(example_union->attributes->Get("on_union"));
  EXPECT_TRUE(example_union->members.front().attributes->Get("on_union_member"));
}

TEST(AttributesTests, GoodOfficialAttributes) {
  TestLibrary library(R"FIDL(
@no_doc
library example;

/// For EXAMPLE_CONSTANT
@no_doc
@deprecated("Note")
const EXAMPLE_CONSTANT string = "foo";

/// For ExampleEnum
@deprecated("Reason")
type ExampleEnum = flexible enum {
    A = 1;
    /// For EnumMember
    @unknown
    B = 2;
};

/// For ExampleStruct
@max_bytes("1234")
@max_handles("5678")
type ExampleStruct = resource struct {
  data @generated_name("CustomName") table {
    1: a uint8;
  };
};

/// For ExampleProtocol
@discoverable
@transport("Syscall")
protocol ExampleProtocol {
    /// For ExampleMethod
    @internal
    @selector("Bar")
    ExampleMethod();
};

/// For ExampleService
@foo("ExampleService")
@no_doc
service ExampleService {
    /// For ExampleProtocol
    @foo("ExampleProtocol")
    @no_doc
    p client_end:ExampleProtocol;
};
)FIDL");
  ASSERT_COMPILED(library);

  EXPECT_TRUE(library.attributes()->Get("no_doc"));

  auto example_const = library.LookupConstant("EXAMPLE_CONSTANT");
  ASSERT_NE(example_const, nullptr);
  EXPECT_TRUE(example_const->attributes->Get("no_doc"));
  EXPECT_TRUE(example_const->attributes->Get("doc")->GetArg("value"));
  auto& const_doc_value = static_cast<const DocCommentConstantValue&>(
      example_const->attributes->Get("doc")->GetArg("value")->value->Value());
  EXPECT_EQ(const_doc_value.value, " For EXAMPLE_CONSTANT\n");
  EXPECT_TRUE(example_const->attributes->Get("deprecated")->GetArg("value"));
  auto& const_str_value = static_cast<const StringConstantValue&>(
      example_const->attributes->Get("deprecated")->GetArg("value")->value->Value());
  EXPECT_EQ(const_str_value.value, "Note");

  auto example_enum = library.LookupEnum("ExampleEnum");
  ASSERT_NE(example_enum, nullptr);
  EXPECT_TRUE(example_enum->attributes->Get("doc")->GetArg("value"));
  auto& enum_doc_value = static_cast<const DocCommentConstantValue&>(
      example_enum->attributes->Get("doc")->GetArg("value")->value->Value());
  EXPECT_EQ(enum_doc_value.value, " For ExampleEnum\n");
  EXPECT_TRUE(example_enum->attributes->Get("deprecated")->GetArg("value"));
  auto& enum_str_value = static_cast<const StringConstantValue&>(
      example_enum->attributes->Get("deprecated")->GetArg("value")->value->Value());
  EXPECT_EQ(enum_str_value.value, "Reason");
  EXPECT_TRUE(example_enum->members.back().attributes->Get("unknown"));

  auto example_struct = library.LookupStruct("ExampleStruct");
  ASSERT_NE(example_struct, nullptr);
  EXPECT_TRUE(example_struct->attributes->Get("doc")->GetArg("value"));
  auto& struct_doc_value = static_cast<const DocCommentConstantValue&>(
      example_struct->attributes->Get("doc")->GetArg("value")->value->Value());
  EXPECT_EQ(struct_doc_value.value, " For ExampleStruct\n");
  EXPECT_TRUE(example_struct->attributes->Get("max_bytes")->GetArg("value"));
  auto& struct_str_value1 = static_cast<const StringConstantValue&>(
      example_struct->attributes->Get("max_bytes")->GetArg("value")->value->Value());
  EXPECT_EQ(struct_str_value1.value, "1234");
  EXPECT_TRUE(example_struct->attributes->Get("max_handles")->GetArg("value"));
  auto& struct_str_value2 = static_cast<const StringConstantValue&>(
      example_struct->attributes->Get("max_handles")->GetArg("value")->value->Value());
  EXPECT_EQ(struct_str_value2.value, "5678");

  auto example_anon = library.LookupTable("CustomName");
  ASSERT_NE(example_anon, nullptr);
  EXPECT_TRUE(example_anon->attributes->Get("generated_name"));

  auto& generated_name_value = static_cast<const StringConstantValue&>(
      example_anon->attributes->Get("generated_name")->GetArg("value")->value->Value());
  EXPECT_EQ(generated_name_value.value, "CustomName");

  auto example_protocol = library.LookupProtocol("ExampleProtocol");
  ASSERT_NE(example_protocol, nullptr);
  EXPECT_TRUE(example_protocol->attributes->Get("discoverable"));
  EXPECT_TRUE(example_protocol->attributes->Get("doc")->GetArg("value"));
  auto& protocol_doc_value = static_cast<const DocCommentConstantValue&>(
      example_protocol->attributes->Get("doc")->GetArg("value")->value->Value());
  EXPECT_EQ(protocol_doc_value.value, " For ExampleProtocol\n");
  EXPECT_TRUE(example_protocol->attributes->Get("transport")->GetArg("value"));
  auto& protocol_str_value = static_cast<const StringConstantValue&>(
      example_protocol->attributes->Get("transport")->GetArg("value")->value->Value());
  EXPECT_EQ(protocol_str_value.value, "Syscall");

  auto& example_method = example_protocol->methods.front();
  EXPECT_TRUE(example_method.attributes->Get("internal"));
  EXPECT_TRUE(example_method.attributes->Get("doc")->GetArg("value"));
  auto& method_doc_value = static_cast<const DocCommentConstantValue&>(
      example_method.attributes->Get("doc")->GetArg("value")->value->Value());
  EXPECT_EQ(method_doc_value.value, " For ExampleMethod\n");
  EXPECT_TRUE(example_method.attributes->Get("selector")->GetArg("value"));
  auto& method_str_value = static_cast<const StringConstantValue&>(
      example_method.attributes->Get("selector")->GetArg("value")->value->Value());
  EXPECT_EQ(method_str_value.value, "Bar");

  auto example_service = library.LookupService("ExampleService");
  ASSERT_NE(example_service, nullptr);
  EXPECT_TRUE(example_service->attributes->Get("no_doc"));
  EXPECT_TRUE(example_service->attributes->Get("doc")->GetArg("value"));
  auto& service_doc_value = static_cast<const DocCommentConstantValue&>(
      example_service->attributes->Get("doc")->GetArg("value")->value->Value());
  EXPECT_EQ(service_doc_value.value, " For ExampleService\n");
  EXPECT_TRUE(example_service->attributes->Get("foo")->GetArg("value"));
  auto& service_str_value = static_cast<const StringConstantValue&>(
      example_service->attributes->Get("foo")->GetArg("value")->value->Value());
  EXPECT_EQ(service_str_value.value, "ExampleService");

  auto& example_service_member = example_service->members.front();
  EXPECT_TRUE(example_service_member.attributes->Get("no_doc"));
  EXPECT_TRUE(example_service_member.attributes->Get("doc")->GetArg("value"));
  auto& service_member_doc_value = static_cast<const DocCommentConstantValue&>(
      example_service_member.attributes->Get("doc")->GetArg("value")->value->Value());
  EXPECT_EQ(service_member_doc_value.value, " For ExampleProtocol\n");
  EXPECT_TRUE(example_service_member.attributes->Get("foo")->GetArg("value"));
  auto& service_member_str_value = static_cast<const StringConstantValue&>(
      example_service_member.attributes->Get("foo")->GetArg("value")->value->Value());
  EXPECT_EQ(service_member_str_value.value, "ExampleProtocol");
}

TEST(AttributesTests, BadNoAttributeOnUsingNotEventDoc) {
  SharedAmongstLibraries shared;
  TestLibrary dependency(&shared);
  dependency.AddFile("bad/fi-0045-a.test.fidl");
  ASSERT_COMPILED(dependency);
  TestLibrary library(&shared);
  library.AddFile("bad/fi-0045-b.test.fidl");
  library.ExpectFail(ErrAttributesNotAllowedOnLibraryImport);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

// Test that a duplicate attribute is caught, and nicely reported.
TEST(AttributesTests, BadNoTwoSameAttribute) {
  TestLibrary library(R"FIDL(
library fidl.test.dupattributes;

@dup("first")
@dup("second")
protocol A {
    MethodA();
};
)FIDL");
  library.ExpectFail(ErrDuplicateAttribute, "dup", "example.fidl:4:2");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

// Test that attributes with the same canonical form are considered duplicates.
TEST(AttributesTests, BadNoTwoSameAttributeCanonical) {
  TestLibrary library;
  library.AddFile("bad/fi-0123.test.fidl");
  library.ExpectFail(ErrDuplicateAttributeCanonical, "CustomAttribute", "custom_attribute",
                     "bad/fi-0123.test.fidl:6:2", "custom_attribute");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, GoodDocAttribute) {
  TestLibrary library;
  library.AddFile("good/fi-0028-b.test.fidl");

  ASSERT_COMPILED(library);
}

// Test that doc comments and doc attributes clash are properly checked.
TEST(AttributesTests, BadNoTwoSameDocAttribute) {
  TestLibrary library(R"FIDL(
library fidl.test.dupattributes;

/// first
@doc("second")
protocol A {
    MethodA();
};
)FIDL");
  library.ExpectFail(ErrDuplicateAttribute, "doc", "generated:1:1");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadNoTwoSameAttributeOnLibrary) {
  TestLibrary library;
  library.AddSource("first.fidl", R"FIDL(
@dup("first")
library fidl.test.dupattributes;
)FIDL");
  library.AddSource("second.fidl", R"FIDL(
@dup("second")
library fidl.test.dupattributes;
)FIDL");
  library.ExpectFail(ErrDuplicateAttribute, "dup", "first.fidl:2:2");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

// Test that a close attribute is caught.
TEST(AttributesTests, WarnOnCloseToOfficialAttribute) {
  TestLibrary library;
  library.AddFile("bad/fi-0145.test.fidl");

  library.ExpectWarn(WarnAttributeTypo, "duc", "doc");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, GoodNotTooCloseUnofficialAttribute) {
  TestLibrary library;
  library.AddFile("good/fi-0145.test.fidl");

  ASSERT_COMPILED(library);
  auto example_protocol = library.LookupProtocol("Example");
  ASSERT_NE(example_protocol, nullptr);
  EXPECT_TRUE(example_protocol->attributes->Get("duck"));
  auto& struct_str_value1 = static_cast<const StringConstantValue&>(
      example_protocol->attributes->Get("duck")->GetArg("value")->value->Value());
  EXPECT_EQ(struct_str_value1.value, "quack");
}

// Ensures we detect typos early enough that we still report them, even if there
// were other compilation errors.
TEST(AttributesTests, WarnOnCloseAttributeWithOtherErrors) {
  TestLibrary library(R"FIDL(
@available(platform="foo", added=1)
library fidl.test;

@available(added=1, removed=2)
type Foo = struct {};

// This actually gets added at 1 because we misspelled "available".
@availabe(added=2)
type Foo = resource struct {};
)FIDL");
  library.SelectVersion("foo", "1");
  library.ExpectWarn(WarnAttributeTypo, "availabe", "available");
  library.ExpectFail(ErrNameOverlap, Element::Kind::kStruct, "Foo", Element::Kind::kStruct,
                     "example.fidl:6:6",
                     VersionSet(VersionRange(Version::From(1).value(), Version::From(2).value())),
                     Platform::Parse("foo").value());
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

// This tests our ability to treat warnings as errors.  It is here because this
// is the most convenient warning.
TEST(AttributesTests, BadWarningsAsErrors) {
  TestLibrary library(R"FIDL(
library fidl.test;

@duc("should be doc")
protocol A {
    MethodA();
};
)FIDL");
  library.set_warnings_as_errors(true);
  ASSERT_FALSE(library.Compile());
  ASSERT_EQ(library.warnings().size(), 0u);
  ASSERT_EQ(library.errors().size(), 1u);
  ASSERT_EQ(library.errors()[0]->def.msg, WarnAttributeTypo.msg);
}

TEST(AttributesTests, BadUnknownArgument) {
  TestLibrary library;
  library.AddFile("bad/fi-0129.test.fidl");
  library.SelectVersion("test", "HEAD");
  library.ExpectFail(ErrUnknownAttributeArg, "available", "discontinued");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadEmptyTransport) {
  TestLibrary library;
  library.AddFile("bad/fi-0128.test.fidl");
  library.ExpectFail(ErrMissingRequiredAnonymousAttributeArg, "transport");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadUnrecognizedTransport) {
  TestLibrary library;
  library.AddFile("bad/fi-0142.test.fidl");
  library.ExpectFail(ErrInvalidTransportType, "Invalid",
                     std::set<std::string_view>{"Banjo", "Channel", "Driver", "Syscall"});
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, GoodChannelTransport) {
  TestLibrary library(R"FIDL(
library fidl.test.transportattributes;

@transport("Channel")
protocol A {
    MethodA();
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(AttributesTests, GoodSyscallTransport) {
  TestLibrary library(R"FIDL(
library fidl.test.transportattributes;

@transport("Syscall")
protocol A {
    MethodA();
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(AttributesTests, BadMultipleTransports) {
  TestLibrary library(R"FIDL(
library fidl.test.transportattributes;

@transport("Channel, Syscall")
protocol A {
    MethodA();
};
)FIDL");
  library.ExpectFail(ErrInvalidTransportType, "Channel, Syscall",
                     std::set<std::string_view>{"Banjo", "Channel", "Driver", "Syscall"});
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadUnknownInvalidPlacementOnUnion) {
  TestLibrary library(R"FIDL(
library fidl.test;

@unknown
type U = flexible union {
  1: a int32;
};
)FIDL");

  library.ExpectFail(ErrInvalidAttributePlacement, "unknown");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadUnknownInvalidPlacementOnUnionMember) {
  TestLibrary library(R"FIDL(
library fidl.test;

type U = flexible union {
  @unknown 1: a int32;
};
)FIDL");

  library.ExpectFail(ErrInvalidAttributePlacement, "unknown");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadUnknownInvalidPlacementOnBitsMember) {
  TestLibrary library(R"FIDL(
library fidl.test;

type B = flexible bits : uint32 {
  @unknown A = 0x1;
};
)FIDL");

  library.ExpectFail(ErrInvalidAttributePlacement, "unknown");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadUnknownInvalidOnStrictEnumMember) {
  TestLibrary library;
  library.AddFile("bad/fi-0071.test.fidl");
  library.ExpectFail(ErrUnknownAttributeOnStrictEnumMember);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadIncorrectPlacementLayout) {
  TestLibrary library(R"FIDL(
@selector("test") // 1
library fidl.test;

@selector("test") // 2
const MyConst uint32 = 0;

@selector("test") // 3
type MyEnum = enum {
    @selector("test") // 4
    MyMember = 5;
};

@selector("test") // 5
type MyStruct = struct {
    @selector("test") // 6
    MyMember int32;
};

@selector("test") // 7
type MyUnion = union {
    @selector("test") // 8
    1: MyMember int32;
};

@selector("test") // 9
type MyTable = table {
    @selector("test") // 10
    1: MyMember int32;
};

@selector("test") // 11
protocol MyProtocol {
    @selector("test") // no error, this placement is allowed
    MyMethod();
};
)FIDL");
  for (int i = 0; i < 11; i++) {
    library.ExpectFail(ErrInvalidAttributePlacement, "selector");
  }
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadSingleDeprecatedAttribute) {
  TestLibrary library;
  library.AddFile("bad/fi-0121.test.fidl");
  library.ExpectFail(ErrDeprecatedAttribute, "example_deprecated_attribute");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadDeprecatedAttributes) {
  TestLibrary library(R"FIDL(
library fidl.test;

@example_deprecated_attribute
type MyStruct = struct {};

@example_deprecated_attribute
protocol MyOtherProtocol {
  MyMethod();
};

@example_deprecated_attribute
protocol MyProtocol {
  MyMethod();
};
)FIDL");
  for (int i = 0; i < 3; i++) {
    library.ExpectFail(ErrDeprecatedAttribute, "example_deprecated_attribute");
  }
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

constexpr ErrorDef<123> TestErrIncorrectNumberOfMembers("incorrect number of members");

bool MustHaveThreeMembers(Reporter* reporter, const ExperimentalFlagSet flags,
                          const Attribute* attribute, const Element* element) {
  switch (element->kind) {
    case Element::Kind::kStruct: {
      auto struct_decl = static_cast<const Struct*>(element);
      if (struct_decl->members.size() != 3) {
        return reporter->Fail(TestErrIncorrectNumberOfMembers, attribute->span);
      }
      return true;
    }
    default:
      return reporter->Fail(ErrInvalidAttributePlacement, attribute->span, attribute);
  }
}

TEST(AttributesTests, BadConstraintOnlyThreeMembersOnStruct) {
  TestLibrary library(R"FIDL(
library fidl.test;

@must_have_three_members
type MyStruct = struct {
    one int64;
    two int64;
    three int64;
    oh_no_four int64;
};
)FIDL");
  library.AddAttributeSchema("must_have_three_members").Constrain(MustHaveThreeMembers);
  library.ExpectFail(TestErrIncorrectNumberOfMembers);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadConstraintOnlyThreeMembersOnMethod) {
  TestLibrary library(R"FIDL(
library fidl.test;

protocol MyProtocol {
    @must_have_three_members MyMethod();
};
)FIDL");
  library.AddAttributeSchema("must_have_three_members").Constrain(MustHaveThreeMembers);
  library.ExpectFail(ErrInvalidAttributePlacement, "must_have_three_members");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadConstraintOnlyThreeMembersOnProtocol) {
  TestLibrary library(R"FIDL(
library fidl.test;

@must_have_three_members
protocol MyProtocol {
    MyMethod();
    MySecondMethod();
};
)FIDL");
  library.AddAttributeSchema("must_have_three_members").Constrain(MustHaveThreeMembers);
  library.ExpectFail(ErrInvalidAttributePlacement, "must_have_three_members");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadAttributeValue) {
  TestLibrary library;
  library.AddFile("bad/fi-0132.test.fidl");
  library.ExpectFail(ErrAttributeDisallowsArgs, "unknown");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadSelectorIncorrectPlacement) {
  TestLibrary library;
  library.AddFile("bad/fi-0120-a.test.fidl");
  library.ExpectFail(ErrInvalidAttributePlacement, "selector");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadParameterAttributeIncorrectPlacement) {
  TestLibrary library(R"FIDL(
library fidl.test;

protocol ExampleProtocol {
    Method(struct { arg exampleusing.Empty; } @on_parameter);
};
)FIDL");
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kAt),
                     Token::KindAndSubkind(Token::Kind::kRightParen));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadAttributeOnTopLevelLayout) {
  TestLibrary library;
  library.AddFile("bad/fi-0023.noformat.test.fidl");
  library.ExpectFail(ErrAttributeInsideTypeDeclaration);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, GoodLayoutAttributePlacements) {
  TestLibrary library(R"FIDL(
library fidl.test;

@foo
type Foo = struct {};

protocol MyProtocol {
  MyMethod(@baz struct {
    inner_layout @qux struct {};
  });
};
)FIDL");
  ASSERT_COMPILED(library);

  auto foo = library.LookupStruct("Foo");
  ASSERT_NE(foo, nullptr);
  EXPECT_TRUE(foo->attributes->Get("foo"));

  auto req = library.LookupStruct("MyProtocolMyMethodRequest");
  ASSERT_NE(req, nullptr);
  EXPECT_TRUE(req->attributes->Get("baz"));

  auto inner = library.LookupStruct("InnerLayout");
  ASSERT_NE(inner, nullptr);
  EXPECT_TRUE(inner->attributes->Get("qux"));
}

TEST(AttributesTests, BadNoArgumentsEmptyParens) {
  TestLibrary library;
  library.AddFile("bad/fi-0014.noformat.test.fidl");
  library.ExpectFail(ErrAttributeWithEmptyParens);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, GoodMultipleArguments) {
  TestLibrary library(R"FIDL(
library example;

@foo(bar="abc", baz="def")
type MyStruct = struct {};
)FIDL");
  ASSERT_COMPILED(library);

  auto example_struct = library.LookupStruct("MyStruct");
  ASSERT_NE(example_struct, nullptr);
  EXPECT_TRUE(example_struct->attributes->Get("foo"));
  EXPECT_TRUE(example_struct->attributes->Get("foo")->GetArg("bar"));
  EXPECT_EQ(example_struct->attributes->Get("foo")->GetArg("bar")->value->span.data(), "\"abc\"");
  EXPECT_TRUE(example_struct->attributes->Get("foo")->GetArg("baz"));
  EXPECT_EQ(example_struct->attributes->Get("foo")->GetArg("baz")->value->span.data(), "\"def\"");
}

TEST(AttributesTests, BadMultipleArgumentsWithNoNames) {
  TestLibrary library;
  library.AddFile("bad/fi-0015.noformat.test.fidl");
  library.ExpectFail(ErrAttributeArgsMustAllBeNamed);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadMultipleArgumentsSomeNamesUnnamedStringArgFirst) {
  TestLibrary library(R"FIDL(
library example;

@foo("abc", bar="def")
type MyStruct = struct {};
)FIDL");

  library.ExpectFail(ErrAttributeArgsMustAllBeNamed);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadMultipleArgumentsSomeNamesUnnamedStringArgSecond) {
  TestLibrary library(R"FIDL(
library example;

@foo(bar="abc", "def")
type MyStruct = struct {};
)FIDL");
  // TODO(https://fxbug.dev/42063565): If an unnamed string argument follows a named
  // argument, it incorrectly produces ErrUnexpectedTokenOfKind instead of
  // ErrAttributeArgsMustAllBeNamed.
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kStringLiteral),
                     Token::KindAndSubkind(Token::Kind::kIdentifier));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadMultipleArgumentsSomeNamesUnnamedIdentifierArgFirst) {
  TestLibrary library(R"FIDL(
library example;

@foo("abc", bar=def)
type MyStruct = struct {};
)FIDL");
  library.ExpectFail(ErrAttributeArgsMustAllBeNamed);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadMultipleArgumentsSomeNamesUnnamedIdentifierArgSecond) {
  TestLibrary library(R"FIDL(
library example;

@foo(bar="abc", def)
type MyStruct = struct {};
)FIDL");
  library.ExpectFail(ErrAttributeArgsMustAllBeNamed);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadMultipleArgumentsDuplicateNames) {
  TestLibrary library;
  library.AddFile("bad/fi-0130.test.fidl");
  library.ExpectFail(ErrDuplicateAttributeArg, "custom_attribute", "custom_arg",
                     "bad/fi-0130.test.fidl:6:19");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadMultipleArgumentsDuplicateCanonicalNames) {
  TestLibrary library;
  library.AddFile("bad/fi-0131.test.fidl");
  library.ExpectFail(ErrDuplicateAttributeArgCanonical, "custom_attribute", "CustomArg",
                     "custom_arg", "bad/fi-0131.test.fidl:6:19", "custom_arg");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, GoodSingleArgumentIsNotNamed) {
  TestLibrary library(R"FIDL(
library example;

@foo("bar")
type MyStruct = struct {};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(AttributesTests, GoodSingleArgumentIsNamedWithoutSchema) {
  TestLibrary library(R"FIDL(
library example;

@foo(a="bar")
type MyStruct = struct {};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(AttributesTests, GoodSingleSchemaArgument) {
  TestLibrary library(R"FIDL(
library example;

@foo("bar")
type MyStruct = struct {};
)FIDL");
  library.AddAttributeSchema("foo").AddArg(
      "value",
      AttributeArgSchema(ConstantValue::Kind::kString, AttributeArgSchema::Optionality::kRequired));
  ASSERT_COMPILED(library);
}

TEST(AttributesTests, GoodSingleSchemaArgumentWithInferredName) {
  TestLibrary library(R"FIDL(
library example;

@foo("bar")
type MyStruct = struct {};
)FIDL");
  library.AddAttributeSchema("foo").AddArg(
      "inferrable",
      AttributeArgSchema(ConstantValue::Kind::kString, AttributeArgSchema::Optionality::kRequired));
  ASSERT_COMPILED(library);

  auto example_struct = library.LookupStruct("MyStruct");
  ASSERT_NE(example_struct, nullptr);
  EXPECT_TRUE(example_struct->attributes->Get("foo"));
  EXPECT_TRUE(example_struct->attributes->Get("foo")->GetArg("inferrable"));
}

// If a schema is provided (ie, this is an "official" FIDL attribute), and it specifies that only
// a single optional argument is allowed, respect both the inclusion and omission of that argument.
TEST(AttributesTests, GoodSingleSchemaArgumentRespectOptionality) {
  TestLibrary library(R"FIDL(
library example;

@foo("bar")
type MyStruct = struct {};

@foo
type MyOtherStruct = struct {};
)FIDL");
  library.AddAttributeSchema("foo").AddArg(
      "value",
      AttributeArgSchema(ConstantValue::Kind::kString, AttributeArgSchema::Optionality::kOptional));
  ASSERT_COMPILED(library);
}

// If a schema is provided (ie, this is an "official" FIDL attribute), and it specifies that only
// a single argument is allowed, naming that argument is an error.
TEST(AttributesTests, BadSingleSchemaArgumentIsNamed) {
  TestLibrary library;
  library.AddFile("bad/fi-0125.test.fidl");
  library.ExpectFail(ErrAttributeArgMustNotBeNamed);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

// If a schema is provided (ie, this is an "official" FIDL attribute), and it specifies that
// multiple arguments are allowed, a single unnamed argument is an error.
TEST(AttributesTests, BadSingleSchemaArgumentIsNotNamed) {
  TestLibrary library;
  library.AddFile("bad/fi-0126.test.fidl");
  library.SelectVersion("test", "HEAD");
  // Here we are demonstrating ErrAttributeArgNotNamed. There is another error
  // because @available is the only attribute that takes multiple arguments, and
  // omitting the required "added" causes another error.
  library.ExpectFail(ErrAttributeArgNotNamed, "1");
  library.ExpectFail(ErrLibraryAvailabilityMissingAdded);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, GoodMultipleSchemaArgumentsRequiredOnly) {
  TestLibrary library(R"FIDL(
library fidl.test;

@multiple_args(first="foo", second="bar")
type MyStruct = struct {};

// Order independent.
@multiple_args(second="bar", first="foo")
type MyOtherStruct = struct {};
)FIDL");
  library.AddAttributeSchema("multiple_args")
      .AddArg("first", AttributeArgSchema(ConstantValue::Kind::kString,
                                          AttributeArgSchema::Optionality::kRequired))
      .AddArg("second", AttributeArgSchema(ConstantValue::Kind::kString,
                                           AttributeArgSchema::Optionality::kRequired));
  ASSERT_COMPILED(library);
}

TEST(AttributesTests, GoodMultipleSchemaArgumentsOptionalOnly) {
  TestLibrary library(R"FIDL(
library fidl.test;

@multiple_args(first="foo", second="bar")
type MyStruct = struct {};

// Order independent.
@multiple_args(second="bar", first="foo")
type MyStruct2 = struct {};

// Only 1 argument present.
@multiple_args(first="foo")
type MyStruct3 = struct {};
@multiple_args(second="bar")
type MyStruct4 = struct {};

// No arguments at all.
@multiple_args
type MyStruct5 = struct {};
)FIDL");
  library.AddAttributeSchema("multiple_args")
      .AddArg("first", AttributeArgSchema(ConstantValue::Kind::kString,
                                          AttributeArgSchema::Optionality::kOptional))
      .AddArg("second", AttributeArgSchema(ConstantValue::Kind::kString,
                                           AttributeArgSchema::Optionality::kOptional));
  ASSERT_COMPILED(library);
}

TEST(AttributesTests, GoodMultipleSchemaArgumentsRequiredAndOptional) {
  TestLibrary library(R"FIDL(
library fidl.test;

@multiple_args(first="foo", second="bar")
type MyStruct = struct {};

// Order independent.
@multiple_args(second="bar", first="foo")
type MyStruct2 = struct {};

// Only 1 argument present.
@multiple_args(first="foo")
type MyStruct3 = struct {};
)FIDL");
  library.AddAttributeSchema("multiple_args")
      .AddArg("first", AttributeArgSchema(ConstantValue::Kind::kString,
                                          AttributeArgSchema::Optionality::kRequired))
      .AddArg("second", AttributeArgSchema(ConstantValue::Kind::kString,
                                           AttributeArgSchema::Optionality::kOptional));
  ASSERT_COMPILED(library);
}

TEST(AttributesTests, BadMultipleSchemaArgumentsRequiredMissing) {
  TestLibrary library;
  library.AddFile("bad/fi-0127.test.fidl");
  library.AddAttributeSchema("has_required_arg")
      .AddArg("required", AttributeArgSchema(ConstantValue::Kind::kString,
                                             AttributeArgSchema::Optionality::kRequired))
      .AddArg("optional", AttributeArgSchema(ConstantValue::Kind::kString,
                                             AttributeArgSchema::Optionality::kOptional));
  library.ExpectFail(ErrMissingRequiredAttributeArg, "has_required_arg", "required");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, GoodLiteralTypesWithoutSchema) {
  TestLibrary library(R"FIDL(
library example;

@attr(foo="abc", bar=true, baz=false)
type MyStruct = struct {};
)FIDL");
  ASSERT_COMPILED(library);

  auto example_struct = library.LookupStruct("MyStruct");
  ASSERT_NE(example_struct, nullptr);
  EXPECT_TRUE(example_struct->attributes->Get("attr"));

  // Check `foo` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("foo"));
  const auto& foo = example_struct->attributes->Get("attr")->GetArg("foo")->value;
  EXPECT_EQ(foo->span.data(), "\"abc\"");
  ASSERT_EQ(foo->kind, Constant::Kind::kLiteral);

  std::unique_ptr<ConstantValue> resolved_foo;
  EXPECT_TRUE(foo->Value().Convert(ConstantValue::Kind::kString, &resolved_foo));

  // Check `baz` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("baz"));
  const auto& baz = example_struct->attributes->Get("attr")->GetArg("baz")->value;
  EXPECT_EQ(baz->span.data(), "false");
  ASSERT_EQ(baz->kind, Constant::Kind::kLiteral);

  std::unique_ptr<ConstantValue> resolved_baz;
  EXPECT_TRUE(baz->Value().Convert(ConstantValue::Kind::kBool, &resolved_baz));
}

TEST(AttributesTests, BadLiteralNumericTypesWithoutSchema) {
  TestLibrary library;
  library.AddFile("bad/fi-0124.test.fidl");
  library.ExpectFail(ErrCanOnlyUseStringOrBool, "foo", "my_custom_attr");
  library.ExpectFail(ErrCanOnlyUseStringOrBool, "bar", "my_custom_attr");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, GoodReferencedTypesWithoutSchema) {
  TestLibrary library(R"FIDL(
library example;

const foo string:3 = "abc";
const bar bool = true;
const baz bool = false;

@attr(foo=foo, bar=bar, baz=baz)
type MyStruct = struct {};
)FIDL");
  ASSERT_COMPILED(library);

  auto example_struct = library.LookupStruct("MyStruct");
  ASSERT_NE(example_struct, nullptr);
  EXPECT_TRUE(example_struct->attributes->Get("attr"));

  // Check `foo` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("foo"));
  const auto& foo = example_struct->attributes->Get("attr")->GetArg("foo")->value;
  EXPECT_EQ(foo->span.data(), "foo");
  ASSERT_EQ(foo->kind, Constant::Kind::kIdentifier);

  std::unique_ptr<ConstantValue> resolved_foo;
  EXPECT_TRUE(foo->Value().Convert(ConstantValue::Kind::kString, &resolved_foo));
  EXPECT_EQ(static_cast<StringConstantValue*>(resolved_foo.get())->value, "abc");

  // Check `bar` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("bar"));
  const auto& bar = example_struct->attributes->Get("attr")->GetArg("bar")->value;
  EXPECT_EQ(bar->span.data(), "bar");
  ASSERT_EQ(bar->kind, Constant::Kind::kIdentifier);

  std::unique_ptr<ConstantValue> resolved_bar;
  EXPECT_TRUE(bar->Value().Convert(ConstantValue::Kind::kBool, &resolved_bar));
  EXPECT_TRUE(static_cast<BoolConstantValue*>(resolved_bar.get())->value);

  // Check `baz` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("baz"));
  const auto& baz = example_struct->attributes->Get("attr")->GetArg("baz")->value;
  EXPECT_EQ(baz->span.data(), "baz");
  ASSERT_EQ(baz->kind, Constant::Kind::kIdentifier);

  std::unique_ptr<ConstantValue> resolved_baz;
  EXPECT_TRUE(baz->Value().Convert(ConstantValue::Kind::kBool, &resolved_baz));
  EXPECT_TRUE(!static_cast<BoolConstantValue*>(resolved_baz.get())->value);
}

TEST(AttributesTests, BadReferencedNumericTypesWithoutSchema) {
  TestLibrary library(R"FIDL(
library example;

const foo int8 = -1;
const bar float32 = -2.3;

@attr(foo=foo, bar=bar)
type MyStruct = struct {};
)FIDL");
  library.ExpectFail(ErrCanOnlyUseStringOrBool, "foo", "attr");
  library.ExpectFail(ErrCanOnlyUseStringOrBool, "bar", "attr");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, GoodLiteralTypesWithSchema) {
  TestLibrary library(R"FIDL(
library fidl.test;

@attr(
        string="foo",
        bool=true,
        int8=-1,
        int16=-2,
        int32=-3,
        int64=-4,
        uint8=1,
        uint16=2,
        uint32=3,
        uint64=4,
        usize64=5,
        uintptr64=6,
        uchar=7,
        float32=1.2,
        float64=-3.4)
type MyStruct = struct {};
)FIDL");
  library.AddAttributeSchema("attr")
      .AddArg("string", AttributeArgSchema(ConstantValue::Kind::kString))
      .AddArg("bool", AttributeArgSchema(ConstantValue::Kind::kBool))
      .AddArg("int8", AttributeArgSchema(ConstantValue::Kind::kInt8))
      .AddArg("int16", AttributeArgSchema(ConstantValue::Kind::kInt16))
      .AddArg("int32", AttributeArgSchema(ConstantValue::Kind::kInt32))
      .AddArg("int64", AttributeArgSchema(ConstantValue::Kind::kInt64))
      .AddArg("uint8", AttributeArgSchema(ConstantValue::Kind::kUint8))
      .AddArg("uint16", AttributeArgSchema(ConstantValue::Kind::kUint16))
      .AddArg("uint32", AttributeArgSchema(ConstantValue::Kind::kUint32))
      .AddArg("uint64", AttributeArgSchema(ConstantValue::Kind::kUint64))
      .AddArg("usize64", AttributeArgSchema(ConstantValue::Kind::kZxUsize64))
      .AddArg("uintptr64", AttributeArgSchema(ConstantValue::Kind::kZxUintptr64))
      .AddArg("uchar", AttributeArgSchema(ConstantValue::Kind::kZxUchar))
      .AddArg("float32", AttributeArgSchema(ConstantValue::Kind::kFloat32))
      .AddArg("float64", AttributeArgSchema(ConstantValue::Kind::kFloat64));
  ASSERT_COMPILED(library);

  auto example_struct = library.LookupStruct("MyStruct");
  ASSERT_NE(example_struct, nullptr);
  EXPECT_TRUE(example_struct->attributes->Get("attr"));

  // Check `string` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("string"));
  const auto& string_val = example_struct->attributes->Get("attr")->GetArg("string")->value;
  EXPECT_EQ(string_val->span.data(), "\"foo\"");
  ASSERT_EQ(string_val->kind, Constant::Kind::kLiteral);

  std::unique_ptr<ConstantValue> resolved_string;
  EXPECT_TRUE(string_val->Value().Convert(ConstantValue::Kind::kString, &resolved_string));
  EXPECT_EQ(static_cast<StringConstantValue*>(resolved_string.get())->value, "foo");

  // Check `bool` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("bool"));
  const auto& bool_val = example_struct->attributes->Get("attr")->GetArg("bool")->value;
  EXPECT_EQ(bool_val->span.data(), "true");
  ASSERT_EQ(bool_val->kind, Constant::Kind::kLiteral);

  std::unique_ptr<ConstantValue> resolved_bool;
  EXPECT_TRUE(bool_val->Value().Convert(ConstantValue::Kind::kBool, &resolved_bool));
  EXPECT_EQ(static_cast<BoolConstantValue*>(resolved_bool.get())->value, true);

  // Check `int8` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("int8"));
  const auto& int8_val = example_struct->attributes->Get("attr")->GetArg("int8")->value;
  EXPECT_EQ(int8_val->span.data(), "-1");
  ASSERT_EQ(int8_val->kind, Constant::Kind::kLiteral);

  std::unique_ptr<ConstantValue> resolved_int8;
  EXPECT_TRUE(int8_val->Value().Convert(ConstantValue::Kind::kInt8, &resolved_int8));
  EXPECT_EQ(static_cast<NumericConstantValue<int8_t>*>(resolved_int8.get())->value, -1);

  // Check `int16` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("int16"));
  const auto& int16_val = example_struct->attributes->Get("attr")->GetArg("int16")->value;
  EXPECT_EQ(int16_val->span.data(), "-2");
  ASSERT_EQ(int16_val->kind, Constant::Kind::kLiteral);

  std::unique_ptr<ConstantValue> resolved_int16;
  EXPECT_TRUE(int16_val->Value().Convert(ConstantValue::Kind::kInt16, &resolved_int16));
  EXPECT_EQ(static_cast<NumericConstantValue<int16_t>*>(resolved_int16.get())->value, -2);

  // Check `int32` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("int32"));
  const auto& int32_val = example_struct->attributes->Get("attr")->GetArg("int32")->value;
  EXPECT_EQ(int32_val->span.data(), "-3");
  ASSERT_EQ(int32_val->kind, Constant::Kind::kLiteral);

  std::unique_ptr<ConstantValue> resolved_int32;
  EXPECT_TRUE(int32_val->Value().Convert(ConstantValue::Kind::kInt32, &resolved_int32));
  EXPECT_EQ(static_cast<NumericConstantValue<int32_t>*>(resolved_int32.get())->value, -3);

  // Check `int64` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("int64"));
  const auto& int64_val = example_struct->attributes->Get("attr")->GetArg("int64")->value;
  EXPECT_EQ(int64_val->span.data(), "-4");
  ASSERT_EQ(int64_val->kind, Constant::Kind::kLiteral);

  std::unique_ptr<ConstantValue> resolved_int64;
  EXPECT_TRUE(int64_val->Value().Convert(ConstantValue::Kind::kInt64, &resolved_int64));
  EXPECT_EQ(static_cast<NumericConstantValue<int64_t>*>(resolved_int64.get())->value, -4);

  // Check `uint8` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("uint8"));
  const auto& uint8_val = example_struct->attributes->Get("attr")->GetArg("uint8")->value;
  EXPECT_EQ(uint8_val->span.data(), "1");
  ASSERT_EQ(uint8_val->kind, Constant::Kind::kLiteral);

  std::unique_ptr<ConstantValue> resolved_uint8;
  EXPECT_TRUE(uint8_val->Value().Convert(ConstantValue::Kind::kUint8, &resolved_uint8));
  EXPECT_EQ(static_cast<NumericConstantValue<uint8_t>*>(resolved_uint8.get())->value, 1u);

  // Check `uint16` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("uint16"));
  const auto& uint16_val = example_struct->attributes->Get("attr")->GetArg("uint16")->value;
  EXPECT_EQ(uint16_val->span.data(), "2");
  ASSERT_EQ(uint16_val->kind, Constant::Kind::kLiteral);

  std::unique_ptr<ConstantValue> resolved_uint16;
  EXPECT_TRUE(uint16_val->Value().Convert(ConstantValue::Kind::kUint16, &resolved_uint16));
  EXPECT_EQ(static_cast<NumericConstantValue<uint16_t>*>(resolved_uint16.get())->value, 2u);

  // Check `uint32` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("uint32"));
  const auto& uint32_val = example_struct->attributes->Get("attr")->GetArg("uint32")->value;
  EXPECT_EQ(uint32_val->span.data(), "3");
  ASSERT_EQ(uint32_val->kind, Constant::Kind::kLiteral);

  std::unique_ptr<ConstantValue> resolved_uint32;
  EXPECT_TRUE(uint32_val->Value().Convert(ConstantValue::Kind::kUint32, &resolved_uint32));
  EXPECT_EQ(static_cast<NumericConstantValue<uint32_t>*>(resolved_uint32.get())->value, 3u);

  // Check `uint64` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("uint64"));
  const auto& uint64_val = example_struct->attributes->Get("attr")->GetArg("uint64")->value;
  EXPECT_EQ(uint64_val->span.data(), "4");
  ASSERT_EQ(uint64_val->kind, Constant::Kind::kLiteral);

  std::unique_ptr<ConstantValue> resolved_uint64;
  EXPECT_TRUE(uint64_val->Value().Convert(ConstantValue::Kind::kUint64, &resolved_uint64));
  EXPECT_EQ(static_cast<NumericConstantValue<uint64_t>*>(resolved_uint64.get())->value, 4u);

  // Check `usize64` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("usize64"));
  const auto& usize_val = example_struct->attributes->Get("attr")->GetArg("usize64")->value;
  EXPECT_EQ(usize_val->span.data(), "5");
  ASSERT_EQ(usize_val->kind, Constant::Kind::kLiteral);

  std::unique_ptr<ConstantValue> resolved_usize;
  EXPECT_TRUE(usize_val->Value().Convert(ConstantValue::Kind::kZxUsize64, &resolved_usize));
  EXPECT_EQ(static_cast<NumericConstantValue<uint64_t>*>(resolved_usize.get())->value, 5u);

  // Check `uintptr64` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("uintptr64"));
  const auto& uintptr_val = example_struct->attributes->Get("attr")->GetArg("uintptr64")->value;
  EXPECT_EQ(uintptr_val->span.data(), "6");
  ASSERT_EQ(uintptr_val->kind, Constant::Kind::kLiteral);

  std::unique_ptr<ConstantValue> resolved_uintptr;
  EXPECT_TRUE(uintptr_val->Value().Convert(ConstantValue::Kind::kZxUintptr64, &resolved_uintptr));
  EXPECT_EQ(static_cast<NumericConstantValue<uint64_t>*>(resolved_uintptr.get())->value, 6u);

  // Check `uchar` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("uchar"));
  const auto& uchar_val = example_struct->attributes->Get("attr")->GetArg("uchar")->value;
  EXPECT_EQ(uchar_val->span.data(), "7");
  ASSERT_EQ(uchar_val->kind, Constant::Kind::kLiteral);

  std::unique_ptr<ConstantValue> resolved_uchar;
  EXPECT_TRUE(uchar_val->Value().Convert(ConstantValue::Kind::kZxUchar, &resolved_uchar));
  EXPECT_EQ(static_cast<NumericConstantValue<uint8_t>*>(resolved_uchar.get())->value, 7u);

  // Check `float32` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("float32"));
  const auto& float32_val = example_struct->attributes->Get("attr")->GetArg("float32")->value;
  EXPECT_EQ(float32_val->span.data(), "1.2");
  ASSERT_EQ(float32_val->kind, Constant::Kind::kLiteral);

  std::unique_ptr<ConstantValue> resolved_float32;
  EXPECT_TRUE(float32_val->Value().Convert(ConstantValue::Kind::kFloat32, &resolved_float32));
  EXPECT_TRUE(static_cast<NumericConstantValue<float>*>(resolved_float32.get())->value > 1.1);
  EXPECT_TRUE(static_cast<NumericConstantValue<float>*>(resolved_float32.get())->value < 1.3);

  // Check `float64` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("float64"));
  const auto& float64_val = example_struct->attributes->Get("attr")->GetArg("float64")->value;
  EXPECT_EQ(float64_val->span.data(), "-3.4");
  ASSERT_EQ(float64_val->kind, Constant::Kind::kLiteral);

  std::unique_ptr<ConstantValue> resolved_float64;
  EXPECT_TRUE(float64_val->Value().Convert(ConstantValue::Kind::kFloat64, &resolved_float64));
  EXPECT_TRUE(static_cast<NumericConstantValue<double>*>(resolved_float64.get())->value > -3.5);
  EXPECT_TRUE(static_cast<NumericConstantValue<double>*>(resolved_float64.get())->value < -3.3);
}

TEST(AttributesTests, BadInvalidLiteralStringTypeWithSchema) {
  TestLibrary library(R"FIDL(
library example;

@attr(true)
type MyStruct = struct {};
)FIDL");
  library.AddAttributeSchema("attr").AddArg("string",
                                            AttributeArgSchema(ConstantValue::Kind::kString));
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "true", "bool", "string");
  library.ExpectFail(ErrCouldNotResolveAttributeArg);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadInvalidLiteralBoolTypeWithSchema) {
  TestLibrary library(R"FIDL(
library example;

@attr("foo")
type MyStruct = struct {};
)FIDL");
  library.AddAttributeSchema("attr").AddArg("bool", AttributeArgSchema(ConstantValue::Kind::kBool));
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "\"foo\"", "string:3", "bool");
  library.ExpectFail(ErrCouldNotResolveAttributeArg);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadInvalidLiteralNumericTypeWithSchema) {
  TestLibrary library(R"FIDL(
library example;

@attr(-1)
type MyStruct = struct {};
)FIDL");
  library.AddAttributeSchema("attr").AddArg("uint8",
                                            AttributeArgSchema(ConstantValue::Kind::kUint8));
  library.ExpectFail(ErrConstantOverflowsType, "-1", "uint8");
  library.ExpectFail(ErrCouldNotResolveAttributeArg);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadInvalidLiteralWithRealSchema) {
  TestLibrary library;
  library.AddFile("bad/fi-0065-c.test.fidl");
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "3840912312901827381273", "untyped numeric",
                     "string");
  library.ExpectFail(ErrCouldNotResolveAttributeArg);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, GoodReferencedTypesWithSchema) {
  TestLibrary library(R"FIDL(
library fidl.test;

const string fidl.string = "foo";
const bool fidl.bool = true;
const int8 fidl.int8 = -1;
const int16 fidl.int16 = -2;
const int32 fidl.int32 = -3;
type int64 = enum : fidl.int64 {
    MEMBER = -4;
};
const uint8 fidl.uint8 = 1;
const uint16 fidl.uint16 = 2;
const uint32 fidl.uint32 = 3;
type uint64 = bits : fidl.uint64 {
    MEMBER = 4;
};
const usize64 fidl.usize64 = 5;
const uintptr64 fidl.uintptr64 = 6;
const uchar fidl.uchar = 7;
const float32 fidl.float32 = 1.2;
const float64 fidl.float64 = -3.4;

@attr(
        string=string,
        bool=bool,
        int8=int8,
        int16=int16,
        int32=int32,
        int64=int64.MEMBER,
        uint8=uint8,
        uint16=uint16,
        uint32=uint32,
        uint64=uint64.MEMBER,
        usize64=usize64,
        uintptr64=uintptr64,
        uchar=uchar,
        float32=float32,
        float64=float64)
type MyStruct = struct {};
)FIDL");
  library.AddAttributeSchema("attr")
      .AddArg("string", AttributeArgSchema(ConstantValue::Kind::kString))
      .AddArg("bool", AttributeArgSchema(ConstantValue::Kind::kBool))
      .AddArg("int8", AttributeArgSchema(ConstantValue::Kind::kInt8))
      .AddArg("int16", AttributeArgSchema(ConstantValue::Kind::kInt16))
      .AddArg("int32", AttributeArgSchema(ConstantValue::Kind::kInt32))
      .AddArg("int64", AttributeArgSchema(ConstantValue::Kind::kInt64))
      .AddArg("uint8", AttributeArgSchema(ConstantValue::Kind::kUint8))
      .AddArg("uint16", AttributeArgSchema(ConstantValue::Kind::kUint16))
      .AddArg("uint32", AttributeArgSchema(ConstantValue::Kind::kUint32))
      .AddArg("uint64", AttributeArgSchema(ConstantValue::Kind::kUint64))
      .AddArg("usize64", AttributeArgSchema(ConstantValue::Kind::kZxUsize64))
      .AddArg("uintptr64", AttributeArgSchema(ConstantValue::Kind::kZxUintptr64))
      .AddArg("uchar", AttributeArgSchema(ConstantValue::Kind::kZxUchar))
      .AddArg("float32", AttributeArgSchema(ConstantValue::Kind::kFloat32))
      .AddArg("float64", AttributeArgSchema(ConstantValue::Kind::kFloat64));

  // For the use of usize64, uintptr64, and uchar.
  library.EnableFlag(ExperimentalFlag::kZxCTypes);

  ASSERT_COMPILED(library);

  auto example_struct = library.LookupStruct("MyStruct");
  ASSERT_NE(example_struct, nullptr);
  EXPECT_TRUE(example_struct->attributes->Get("attr"));

  // Check `string` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("string"));
  const auto& string_val = example_struct->attributes->Get("attr")->GetArg("string")->value;
  EXPECT_EQ(string_val->span.data(), "string");
  ASSERT_EQ(string_val->kind, Constant::Kind::kIdentifier);

  std::unique_ptr<ConstantValue> resolved_string;
  EXPECT_TRUE(string_val->Value().Convert(ConstantValue::Kind::kString, &resolved_string));
  EXPECT_EQ(static_cast<StringConstantValue*>(resolved_string.get())->value, "foo");

  // Check `bool` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("bool"));
  const auto& bool_val = example_struct->attributes->Get("attr")->GetArg("bool")->value;
  EXPECT_EQ(bool_val->span.data(), "bool");
  ASSERT_EQ(bool_val->kind, Constant::Kind::kIdentifier);

  std::unique_ptr<ConstantValue> resolved_bool;
  EXPECT_TRUE(bool_val->Value().Convert(ConstantValue::Kind::kBool, &resolved_bool));
  EXPECT_EQ(static_cast<BoolConstantValue*>(resolved_bool.get())->value, true);

  // Check `int8` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("int8"));
  const auto& int8_val = example_struct->attributes->Get("attr")->GetArg("int8")->value;
  EXPECT_EQ(int8_val->span.data(), "int8");
  ASSERT_EQ(int8_val->kind, Constant::Kind::kIdentifier);

  std::unique_ptr<ConstantValue> resolved_int8;
  EXPECT_TRUE(int8_val->Value().Convert(ConstantValue::Kind::kInt8, &resolved_int8));
  EXPECT_EQ(static_cast<NumericConstantValue<int8_t>*>(resolved_int8.get())->value, -1);

  // Check `int16` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("int16"));
  const auto& int16_val = example_struct->attributes->Get("attr")->GetArg("int16")->value;
  EXPECT_EQ(int16_val->span.data(), "int16");
  ASSERT_EQ(int16_val->kind, Constant::Kind::kIdentifier);

  std::unique_ptr<ConstantValue> resolved_int16;
  EXPECT_TRUE(int16_val->Value().Convert(ConstantValue::Kind::kInt16, &resolved_int16));
  EXPECT_EQ(static_cast<NumericConstantValue<int16_t>*>(resolved_int16.get())->value, -2);

  // Check `int32` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("int32"));
  const auto& int32_val = example_struct->attributes->Get("attr")->GetArg("int32")->value;
  EXPECT_EQ(int32_val->span.data(), "int32");
  ASSERT_EQ(int32_val->kind, Constant::Kind::kIdentifier);

  std::unique_ptr<ConstantValue> resolved_int32;
  EXPECT_TRUE(int32_val->Value().Convert(ConstantValue::Kind::kInt32, &resolved_int32));
  EXPECT_EQ(static_cast<NumericConstantValue<int32_t>*>(resolved_int32.get())->value, -3);

  // Check `int64` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("int64"));
  const auto& int64_val = example_struct->attributes->Get("attr")->GetArg("int64")->value;
  EXPECT_EQ(int64_val->span.data(), "int64.MEMBER");
  ASSERT_EQ(int64_val->kind, Constant::Kind::kIdentifier);

  std::unique_ptr<ConstantValue> resolved_int64;
  EXPECT_TRUE(int64_val->Value().Convert(ConstantValue::Kind::kInt64, &resolved_int64));
  EXPECT_EQ(static_cast<NumericConstantValue<int64_t>*>(resolved_int64.get())->value, -4);

  // Check `uint8` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("uint8"));
  const auto& uint8_val = example_struct->attributes->Get("attr")->GetArg("uint8")->value;
  EXPECT_EQ(uint8_val->span.data(), "uint8");
  ASSERT_EQ(uint8_val->kind, Constant::Kind::kIdentifier);

  std::unique_ptr<ConstantValue> resolved_uint8;
  EXPECT_TRUE(uint8_val->Value().Convert(ConstantValue::Kind::kUint8, &resolved_uint8));
  EXPECT_EQ(static_cast<NumericConstantValue<uint8_t>*>(resolved_uint8.get())->value, 1u);

  // Check `uint16` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("uint16"));
  const auto& uint16_val = example_struct->attributes->Get("attr")->GetArg("uint16")->value;
  EXPECT_EQ(uint16_val->span.data(), "uint16");
  ASSERT_EQ(uint16_val->kind, Constant::Kind::kIdentifier);

  std::unique_ptr<ConstantValue> resolved_uint16;
  EXPECT_TRUE(uint16_val->Value().Convert(ConstantValue::Kind::kUint16, &resolved_uint16));
  EXPECT_EQ(static_cast<NumericConstantValue<uint16_t>*>(resolved_uint16.get())->value, 2u);

  // Check `uint32` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("uint32"));
  const auto& uint32_val = example_struct->attributes->Get("attr")->GetArg("uint32")->value;
  EXPECT_EQ(uint32_val->span.data(), "uint32");
  ASSERT_EQ(uint32_val->kind, Constant::Kind::kIdentifier);

  std::unique_ptr<ConstantValue> resolved_uint32;
  EXPECT_TRUE(uint32_val->Value().Convert(ConstantValue::Kind::kUint32, &resolved_uint32));
  EXPECT_EQ(static_cast<NumericConstantValue<uint32_t>*>(resolved_uint32.get())->value, 3u);

  // Check `uint64` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("uint64"));
  const auto& uint64_val = example_struct->attributes->Get("attr")->GetArg("uint64")->value;
  EXPECT_EQ(uint64_val->span.data(), "uint64.MEMBER");
  ASSERT_EQ(uint64_val->kind, Constant::Kind::kIdentifier);

  std::unique_ptr<ConstantValue> resolved_uint64;
  EXPECT_TRUE(uint64_val->Value().Convert(ConstantValue::Kind::kUint64, &resolved_uint64));
  EXPECT_EQ(static_cast<NumericConstantValue<uint64_t>*>(resolved_uint64.get())->value, 4u);

  // Check `usize64` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("usize64"));
  const auto& usize_val = example_struct->attributes->Get("attr")->GetArg("usize64")->value;
  EXPECT_EQ(usize_val->span.data(), "usize64");
  ASSERT_EQ(usize_val->kind, Constant::Kind::kIdentifier);

  std::unique_ptr<ConstantValue> resolved_usize;
  EXPECT_TRUE(usize_val->Value().Convert(ConstantValue::Kind::kZxUsize64, &resolved_usize));
  EXPECT_EQ(static_cast<NumericConstantValue<uint64_t>*>(resolved_usize.get())->value, 5u);

  // Check `uintptr64` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("uintptr64"));
  const auto& uintptr_val = example_struct->attributes->Get("attr")->GetArg("uintptr64")->value;
  EXPECT_EQ(uintptr_val->span.data(), "uintptr64");
  ASSERT_EQ(uintptr_val->kind, Constant::Kind::kIdentifier);

  std::unique_ptr<ConstantValue> resolved_uintptr;
  EXPECT_TRUE(uintptr_val->Value().Convert(ConstantValue::Kind::kZxUintptr64, &resolved_uintptr));
  EXPECT_EQ(static_cast<NumericConstantValue<uint64_t>*>(resolved_uintptr.get())->value, 6u);

  // Check `uchar` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("uchar"));
  const auto& uchar_val = example_struct->attributes->Get("attr")->GetArg("uchar")->value;
  EXPECT_EQ(uchar_val->span.data(), "uchar");
  ASSERT_EQ(uchar_val->kind, Constant::Kind::kIdentifier);

  std::unique_ptr<ConstantValue> resolved_uchar;
  EXPECT_TRUE(uchar_val->Value().Convert(ConstantValue::Kind::kZxUchar, &resolved_uchar));
  EXPECT_EQ(static_cast<NumericConstantValue<uint8_t>*>(resolved_uchar.get())->value, 7u);

  // Check `float32` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("float32"));
  const auto& float32_val = example_struct->attributes->Get("attr")->GetArg("float32")->value;
  EXPECT_EQ(float32_val->span.data(), "float32");
  ASSERT_EQ(float32_val->kind, Constant::Kind::kIdentifier);

  std::unique_ptr<ConstantValue> resolved_float32;
  EXPECT_TRUE(float32_val->Value().Convert(ConstantValue::Kind::kFloat32, &resolved_float32));
  EXPECT_TRUE(static_cast<NumericConstantValue<float>*>(resolved_float32.get())->value > 1.1);
  EXPECT_TRUE(static_cast<NumericConstantValue<float>*>(resolved_float32.get())->value < 1.3);

  // Check `float64` arg.
  EXPECT_TRUE(example_struct->attributes->Get("attr")->GetArg("float64"));
  const auto& float64_val = example_struct->attributes->Get("attr")->GetArg("float64")->value;
  EXPECT_EQ(float64_val->span.data(), "float64");
  ASSERT_EQ(float64_val->kind, Constant::Kind::kIdentifier);

  std::unique_ptr<ConstantValue> resolved_float64;
  EXPECT_TRUE(float64_val->Value().Convert(ConstantValue::Kind::kFloat64, &resolved_float64));
  EXPECT_TRUE(static_cast<NumericConstantValue<double>*>(resolved_float64.get())->value > -3.5);
  EXPECT_TRUE(static_cast<NumericConstantValue<double>*>(resolved_float64.get())->value < -3.3);
}

TEST(AttributesTests, BadInvalidReferencedStringTypeWithSchema) {
  TestLibrary library(R"FIDL(
library example;

const foo bool = true;

@attr(foo)
type MyStruct = struct {};
)FIDL");
  library.AddAttributeSchema("attr").AddArg("string",
                                            AttributeArgSchema(ConstantValue::Kind::kString));
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "example/foo", "bool", "string");
  library.ExpectFail(ErrCouldNotResolveAttributeArg);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadInvalidReferencedBoolTypeWithSchema) {
  TestLibrary library(R"FIDL(
library example;

const foo string:3 = "foo";

@attr(foo)
type MyStruct = struct {};
)FIDL");
  library.AddAttributeSchema("attr").AddArg("bool", AttributeArgSchema(ConstantValue::Kind::kBool));
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "example/foo", "string:3", "bool");
  library.ExpectFail(ErrCouldNotResolveAttributeArg);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadInvalidReferencedNumericTypeWithSchema) {
  TestLibrary library(R"FIDL(
library example;

const foo uint16 = 259;

@attr(foo)
type MyStruct = struct {};
)FIDL");
  library.AddAttributeSchema("attr").AddArg("int8", AttributeArgSchema(ConstantValue::Kind::kInt8));
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "example/foo", "uint16", "int8");
  library.ExpectFail(ErrCouldNotResolveAttributeArg);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, GoodCompileEarlyAttributeLiteralArgument) {
  TestLibrary library(R"FIDL(
library example;

@attr(1)
type MyStruct = struct {};
)FIDL");
  library.AddAttributeSchema("attr")
      .AddArg("int8", AttributeArgSchema(ConstantValue::Kind::kUint8))
      .CompileEarly();
  ASSERT_COMPILED(library);
}

TEST(AttributesTests, BadCompileEarlyAttributeReferencedArgument) {
  TestLibrary library(R"FIDL(
library example;

@attr(BAD)
type MyStruct = struct {};

const BAD uint8 = 1;
)FIDL");
  library.AddAttributeSchema("attr")
      .AddArg("int8", AttributeArgSchema(ConstantValue::Kind::kUint8))
      .CompileEarly();
  library.ExpectFail(ErrAttributeArgRequiresLiteral, "int8", "attr");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, GoodAnonymousArgumentGetsNamedValue) {
  TestLibrary library(R"FIDL(
library example;

@attr("abc")
type MyStruct = struct {};
)FIDL");
  ASSERT_COMPILED(library);

  auto example_struct = library.LookupStruct("MyStruct");
  ASSERT_NE(example_struct, nullptr);
  ASSERT_EQ(example_struct->attributes->attributes.size(), 1u);
  ASSERT_EQ(example_struct->attributes->attributes[0]->args.size(), 1u);
  EXPECT_EQ(example_struct->attributes->attributes[0]->args[0]->name.value().data(), "value");
}

TEST(AttributesTests, GoodSingleNamedArgumentKeepsName) {
  TestLibrary library(R"FIDL(
library example;

@attr(foo="abc")
type MyStruct = struct {};
)FIDL");
  ASSERT_COMPILED(library);

  auto example_struct = library.LookupStruct("MyStruct");
  ASSERT_NE(example_struct, nullptr);
  ASSERT_EQ(example_struct->attributes->attributes.size(), 1u);
  ASSERT_EQ(example_struct->attributes->attributes[0]->args.size(), 1u);
  EXPECT_EQ(example_struct->attributes->attributes[0]->args[0]->name.value().data(), "foo");
}

TEST(AttributesTests, BadReferencesNonexistentConstWithoutSchema) {
  TestLibrary library(R"FIDL(
library example;

@foo(nonexistent)
type MyStruct = struct {};
)FIDL");
  library.ExpectFail(ErrNameNotFound, "nonexistent", "library 'example'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadReferencesNonexistentConstWithSingleArgSchema) {
  TestLibrary library(R"FIDL(
library example;

@foo(nonexistent)
type MyStruct = struct {};
)FIDL");
  library.AddAttributeSchema("foo").AddArg("value", AttributeArgSchema(ConstantValue::Kind::kBool));
  library.ExpectFail(ErrNameNotFound, "nonexistent", "library 'example'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadReferencesNonexistentConstWithMultipleArgSchema) {
  TestLibrary library(R"FIDL(
library example;

@foo(nonexistent)
type MyStruct = struct {};
)FIDL");
  library.AddAttributeSchema("foo")
      .AddArg("first", AttributeArgSchema(ConstantValue::Kind::kBool))
      .AddArg("second", AttributeArgSchema(ConstantValue::Kind::kBool));
  library.ExpectFail(ErrNameNotFound, "nonexistent", "library 'example'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadReferencesInvalidConstWithoutSchema) {
  TestLibrary library(R"FIDL(
library example;

@foo(BAD)
type MyStruct = struct {};

const BAD bool = "not a bool";
)FIDL");
  library.ExpectFail(ErrCouldNotResolveAttributeArg);
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "\"not a bool\"", "string:10", "bool");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadReferencesInvalidConstWithSingleArgSchema) {
  TestLibrary library(R"FIDL(
library example;

@foo(BAD)
type MyStruct = struct {};

const BAD bool = "not a bool";
)FIDL");
  library.AddAttributeSchema("foo").AddArg("value", AttributeArgSchema(ConstantValue::Kind::kBool));
  library.ExpectFail(ErrCouldNotResolveAttributeArg);
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "\"not a bool\"", "string:10", "bool");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadReferencesInvalidConstWithMultipleArgSchema) {
  TestLibrary library(R"FIDL(
library example;

@foo(BAD)
type MyStruct = struct {};

const BAD bool = "not a bool";
)FIDL");
  library.AddAttributeSchema("foo")
      .AddArg("first", AttributeArgSchema(ConstantValue::Kind::kBool))
      .AddArg("second", AttributeArgSchema(ConstantValue::Kind::kBool));
  library.ExpectFail(ErrAttributeArgNotNamed, "BAD");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "\"not a bool\"", "string:10", "bool");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadSelfReferenceWithoutSchemaBool) {
  TestLibrary library(R"FIDL(
library example;

@foo(BAR)
const BAR bool = true;
)FIDL");
  library.ExpectFail(ErrCouldNotResolveAttributeArg);
  library.ExpectFail(ErrIncludeCycle, "const 'BAR' -> const 'BAR'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadSelfReferenceWithoutSchemaString) {
  TestLibrary library(R"FIDL(
library example;

@foo(BAR)
const BAR string = "bar";
)FIDL");
  library.ExpectFail(ErrCouldNotResolveAttributeArg);
  library.ExpectFail(ErrIncludeCycle, "const 'BAR' -> const 'BAR'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadSelfReferenceWithSchema) {
  TestLibrary library(R"FIDL(
library example;

@foo(BAR)
const BAR bool = true;
)FIDL");
  library.AddAttributeSchema("foo").AddArg("value", AttributeArgSchema(ConstantValue::Kind::kBool));
  library.ExpectFail(ErrCouldNotResolveAttributeArg);
  library.ExpectFail(ErrIncludeCycle, "const 'BAR' -> const 'BAR'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadMutualReferenceWithoutSchemaBool) {
  TestLibrary library(R"FIDL(
library example;

@foo(SECOND)
const FIRST bool = true;
@foo(FIRST)
const SECOND bool = false;
)FIDL");
  library.ExpectFail(ErrIncludeCycle, "const 'FIRST' -> const 'SECOND' -> const 'FIRST'");
  library.ExpectFail(ErrCouldNotResolveAttributeArg);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadMutualReferenceWithoutSchemaString) {
  TestLibrary library(R"FIDL(
library example;

@foo(SECOND)
const FIRST string = "first";
@foo(FIRST)
const SECOND string = "second";
)FIDL");
  library.ExpectFail(ErrIncludeCycle, "const 'FIRST' -> const 'SECOND' -> const 'FIRST'");
  library.ExpectFail(ErrCouldNotResolveAttributeArg);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadMutualReferenceWithSchema) {
  TestLibrary library(R"FIDL(
library example;

@foo(SECOND)
const FIRST bool = true;
@foo(FIRST)
const SECOND bool = false;
)FIDL");
  library.AddAttributeSchema("foo").AddArg("value", AttributeArgSchema(ConstantValue::Kind::kBool));
  library.ExpectFail(ErrIncludeCycle, "const 'FIRST' -> const 'SECOND' -> const 'FIRST'");
  library.ExpectFail(ErrCouldNotResolveAttributeArg);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadLibraryReferencesNonexistentConst) {
  TestLibrary library(R"FIDL(
@foo(nonexistent)
library example;
)FIDL");
  library.ExpectFail(ErrNameNotFound, "nonexistent", "library 'example'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadLibraryReferencesConst) {
  TestLibrary library(R"FIDL(
@foo(BAR)
library example;

const BAR bool = true;
)FIDL");
  library.ExpectFail(ErrReferenceInLibraryAttribute);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, BadLibraryReferencesExternalConst) {
  SharedAmongstLibraries shared;
  TestLibrary dependency(&shared, "dependency.fidl", R"FIDL(
library dependency;

const BAR bool = true;
)FIDL");
  ASSERT_COMPILED(dependency);

  TestLibrary library(&shared, "example.fidl", R"FIDL(
@foo(dependency.BAR)
library example;

using dependency;
)FIDL");
  library.ExpectFail(ErrReferenceInLibraryAttribute);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributesTests, GoodDiscoverableImplicitName) {
  TestLibrary library(R"FIDL(
library example;

@discoverable
protocol Foo {};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(AttributesTests, GoodDiscoverableExplicitName) {
  for (auto name : {"example.Foo", "notexample.NotFoo", "not.example.NotFoo"}) {
    std::string library_str = R"FIDL(
library example;

@discoverable(name="%1")
protocol Foo {};
)FIDL";
    library_str.replace(library_str.find("%1"), 2, name);
    TestLibrary library(library_str);
    ASSERT_COMPILED(library);
  }
}

TEST(AttributesTests, BadDiscoverableInvalidName) {
  for (auto name : {"", "example/Foo", "Foo", "not example.Not Foo"}) {
    std::string library_str = R"FIDL(
library example;

@discoverable(name="%1")
protocol Foo {};
)FIDL";
    library_str.replace(library_str.find("%1"), 2, name);
    TestLibrary library(library_str);
    library.ExpectFail(ErrInvalidDiscoverableName, name);
    ASSERT_COMPILER_DIAGNOSTICS(library);
  }
}

TEST(AttributesTests, BadDiscoverableInvalidNameErrcat) {
  TestLibrary library;
  library.AddFile("bad/fi-0135.test.fidl");
  library.ExpectFail(ErrInvalidDiscoverableName, "test.bad.fi0135/Parser");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributeTests, GoodDiscoverableLocation) {
  TestLibrary library(R"FIDL(
library example;

@discoverable(client="")
protocol P{};

@discoverable(client="platform", server="external")
protocol Q{};

@discoverable(client="platform,external", server="external,platform")
protocol R{};

@discoverable(client="platform, external", server="external, platform")
protocol S{};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(AttributesTests, BadDiscoverableLocationErrcat) {
  TestLibrary library;
  library.AddFile("bad/fi-0210.test.fidl");
  library.ExpectFail(ErrInvalidDiscoverableLocation, "platform,canada");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

// The @result attribute was originally used to implement method error results.
// It no longer does anything. This test just makes sure it doesn't cause a crash.
TEST(AttributesTests, GoodResultAttribute) {
  TestLibrary library(R"FIDL(
library example;

@result
type Foo = union {
    1: s string;
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(AttributeTests, GoodNoResource) {
  TestLibrary library;
  library.experimental_flags().Enable(ExperimentalFlag::kNoResourceAttribute);
  library.AddSource("example.fidl", R"FIDL(
library example;

@no_resource
protocol P{
  compose Q;
  Method(struct { i int32; }) -> ();
};

@no_resource
protocol Q{};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(AttributeTests, BadNoResourceUsesResource) {
  TestLibrary library;
  library.experimental_flags().Enable(ExperimentalFlag::kNoResourceAttribute);
  library.AddSource("example.fidl", R"FIDL(
library example;

@no_resource
protocol P{
  compose Q;
  Method(resource struct { i int32; }) -> ();
};

@no_resource
protocol Q{};
)FIDL");
  library.ExpectFail(ErrResourceForbiddenHere);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributeTests, BadNoResourceComposition) {
  TestLibrary library;
  library.experimental_flags().Enable(ExperimentalFlag::kNoResourceAttribute);
  library.AddSource("example.fidl", R"FIDL(
library example;

@no_resource
protocol P{
  compose Q;
  Method( struct { i int32; }) -> ();
};

protocol Q{};
)FIDL");
  library.ExpectFail(ErrNoResourceForbidsCompose, "P", "Q");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributeTests, BadNoResourceIsExperimental) {
  TestLibrary library;
  library.AddSource("example.fidl", R"FIDL(
library example;

@no_resource
protocol P{
  compose Q;
  Method(struct { i int32; }) -> ();
};

@no_resource
protocol Q{};
)FIDL");
  library.ExpectFail(ErrExperimentalNoResource);
  library.ExpectFail(ErrExperimentalNoResource);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributeTests, GoodSensitivePlacement) {
  TestLibrary library(R"FIDL(
library example;

@sensitive
type MyStruct = struct {
    @sensitive
    s string;
};

@sensitive
type MyTable = table {
    @sensitive
    1: s string;
};

@sensitive
type MyUnion = strict union {
    @sensitive
    1: s string;
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(AttributeTests, BadSensitivePlacementOnProtocol) {
  TestLibrary library(R"FIDL(
library example;

@sensitive
protocol MyProtocol {
    Method();
};
)FIDL");
  library.ExpectFail(ErrInvalidAttributePlacement, "sensitive");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributeTests, BadSensitivePlacementOnMethod) {
  TestLibrary library(R"FIDL(
library example;

protocol MyProtocol {
    @sensitive
    Method();
};
)FIDL");
  library.ExpectFail(ErrInvalidAttributePlacement, "sensitive");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributeTests, BadSensitivePlacementOnEnum) {
  TestLibrary library(R"FIDL(
library example;

@sensitive
type MyEnum = strict enum {
    VALUE = 1;
};
)FIDL");
  library.ExpectFail(ErrInvalidAttributePlacement, "sensitive");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AttributeTests, BadSensitivePlacementOnEnumMember) {
  TestLibrary library(R"FIDL(
library example;

type MyEnum = strict enum {
    @sensitive
    VALUE = 1;
};
)FIDL");
  library.ExpectFail(ErrInvalidAttributePlacement, "sensitive");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

}  // namespace
}  // namespace fidlc
