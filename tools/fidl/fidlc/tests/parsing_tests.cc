// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <locale.h>

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/src/raw_ast.h"
#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

// Test that an invalid compound identifier fails parsing. Regression
// test for https://fxbug.dev/42155856.
TEST(ParsingTests, BadCompoundIdentifierTest) {
  // The leading 0 in the library name causes parsing an Identifier
  // to fail, and then parsing a CompoundIdentifier to fail.
  TestLibrary library(R"FIDL(
library 0fidl.test.badcompoundidentifier;
)FIDL");
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kNumericLiteral),
                     Token::KindAndSubkind(Token::Kind::kIdentifier));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, BadLibraryNameTest) {
  TestLibrary library;
  library.AddFile("bad/fi-0011.noformat.test.fidl");
  library.ExpectFail(ErrInvalidLibraryNameComponent, "name_with_underscores");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, GoodSpacesAroundDotsLibraryName) {
  TestLibrary library(R"FIDL(
library foo . bar;
)FIDL");
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.name(), "foo.bar");
}

TEST(ParsingTests, GoodSpacesAroundDotsMemberName) {
  TestLibrary library(R"FIDL(
library example;

type Foo = enum : fidl . uint32 {
  A = 42;
};
const VALUE Foo = Foo . A;
)FIDL");
  ASSERT_COMPILED(library);
  auto constant = library.LookupConstant("VALUE");
  EXPECT_NE(constant, nullptr);
  EXPECT_EQ(constant->value->Value().kind, ConstantValue::Kind::kUint32);
  EXPECT_EQ(static_cast<const NumericConstantValue<uint32_t>&>(constant->value->Value()).value,
            42u);
}

TEST(ParsingTests, GoodSpacesAroundDotsImport) {
  SharedAmongstLibraries shared;
  TestLibrary dependency(&shared, "dependency.fidl", R"FIDL(
library foo . bar . qux;

type Type = struct {};
const VALUE uint32 = 42;
)FIDL");
  ASSERT_COMPILED(dependency);
  TestLibrary library(&shared, "example.fidl", R"FIDL(
library example;

using foo  .  bar  .  qux;
alias Type = foo. bar. qux. Type;
const VALUE uint32 = foo .bar .qux .VALUE;
)FIDL");
  ASSERT_COMPILED(library);
}

// Test that otherwise reserved words can be appropriately parsed when context is clear.
TEST(ParsingTests, GoodParsingReservedWordsInStructTest) {
  TestLibrary library(R"FIDL(
library example;

type struct = struct {
    field bool;
};

type flexible = struct {};
type strict = struct {};
type resource = struct {};

type InStruct = struct {
    foo struct;
    bar flexible;
    baz strict;
    qux resource;

    as bool;
    library bool;
    using bool;

    array bool;
    handle bool;
    request bool;
    string bool;
    vector bool;

    bool bool;
    int8 bool;
    int16 bool;
    int32 bool;
    int64 bool;
    uint8 bool;
    uint16 bool;
    uint32 bool;
    uint64 bool;
    float32 bool;
    float64 bool;

    true bool;
    false bool;

    reserved bool;
};
)FIDL");
  ASSERT_COMPILED(library);
}

// Test that otherwise reserved words can be appropriately parsed when context
// is clear.
TEST(ParsingTests, GoodParsingReservedWordsInConstraint) {
  TestLibrary library(R"FIDL(
library example;

alias T = fidl.uint8;
type S = struct {};

// Keywords
const as T = 1;
alias as_constraint = vector<S>:as;
const library T = 1;
alias library_constraint = vector<S>:library;
const using T = 1;
alias using_constraint = vector<S>:using;
const alias T = 1;
alias alias_constraint = vector<S>:alias;
const type T = 1;
alias type_constraint = vector<S>:type;
const const T = 1;
alias const_constraint = vector<S>:const;
const protocol T = 1;
alias protocol_constraint = vector<S>:protocol;
const service T = 1;
alias service_constraint = vector<S>:service;
const compose T = 1;
alias compose_constraint = vector<S>:compose;
const reserved T = 1;
alias reserved_constraint = vector<S>:reserved;

// Layouts
const bits T = 1;
alias bits_constraint = vector<S>:bits;
const enum T = 1;
alias enum_constraint = vector<S>:enum;
const struct T = 1;
alias struct_constraint = vector<S>:struct;
const table T = 1;
alias table_constraint = vector<S>:table;
const union T = 1;
alias union_constraint = vector<S>:union;

// Builtins
const array T = 1;
alias array_constraint = vector<S>:array;
const handle T = 1;
alias handle_constraint = vector<S>:handle;
const request T = 1;
alias request_constraint = vector<S>:request;
const string T = 1;
alias string_constraint = vector<S>:string;
const optional T = 1;
alias optional_constraint = vector<S>:optional;

// Primitives
const bool T = 1;
alias bool_constraint = vector<S>:bool;
const int8 T = 1;
alias int8_constraint = vector<S>:int8;
const int16 T = 1;
alias int16_constraint = vector<S>:int16;
const int32 T = 1;
alias int32_constraint = vector<S>:int32;
const int64 T = 1;
alias int64_constraint = vector<S>:int64;
const uint8 T = 1;
alias uint8_constraint = vector<S>:uint8;
const uint16 T = 1;
alias uint16_constraint = vector<S>:uint16;
const uint32 T = 1;
alias uint32_constraint = vector<S>:uint32;
const uint64 T = 1;
alias uint64_constraint = vector<S>:uint64;
const float32 T = 1;
alias float32_constraint = vector<S>:float32;
const float64 T = 1;
alias float64_constraint = vector<S>:float64;
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ParsingTests, GoodParsingHandlesInStructTest) {
  TestLibrary library(R"FIDL(
library example;

type ObjType = strict enum : uint32 {
    NONE = 0;
    PROCESS = 1;
    THREAD = 2;
    VMO = 3;
    CHANNEL = 4;
    EVENT = 5;
    PORT = 6;
    INTERRUPT = 9;
    PCI_DEVICE = 11;
    LOG = 12;
    SOCKET = 14;
    RESOURCE = 15;
    EVENTPAIR = 16;
    JOB = 17;
    VMAR = 18;
    FIFO = 19;
    GUEST = 20;
    VCPU = 21;
    TIMER = 22;
    IOMMU = 23;
    BTI = 24;
    PROFILE = 25;
    PMT = 26;
    SUSPEND_TOKEN = 27;
    PAGER = 28;
    EXCEPTION = 29;
    CLOCK = 30;
};

resource_definition handle : uint32 {
    properties {
        subtype ObjType;
    };
};

type Handles = resource struct {
    plain_handle handle;

    bti_handle handle:BTI;
    channel_handle handle:CHANNEL;
    clock_handle handle:CLOCK;
    debuglog_handle handle:LOG;
    event_handle handle:EVENT;
    eventpair_handle handle:EVENTPAIR;
    exception_handle handle:EXCEPTION;
    fifo_handle handle:FIFO;
    guest_handle handle:GUEST;
    interrupt_handle handle:INTERRUPT;
    iommu_handle handle:IOMMU;
    job_handle handle:JOB;
    pager_handle handle:PAGER;
    pcidevice_handle handle:PCI_DEVICE;
    pmt_handle handle:PMT;
    port_handle handle:PORT;
    process_handle handle:PROCESS;
    profile_handle handle:PROFILE;
    resource_handle handle:RESOURCE;
    socket_handle handle:SOCKET;
    suspendtoken_handle handle:SUSPEND_TOKEN;
    thread_handle handle:THREAD;
    timer_handle handle:TIMER;
    vcpu_handle handle:VCPU;
    vmar_handle handle:VMAR;
    vmo_handle handle:VMO;
};
)FIDL");

  ASSERT_COMPILED(library);
}

TEST(ParsingTests, GoodParsingHandleConstraintTest) {
  TestLibrary library(R"FIDL(
library example;

type ObjType = strict enum : uint32 {
    NONE = 0;
    VMO = 3;
};

type Rights = strict bits : uint32 {
    TRANSFER = 1;
};

resource_definition handle : uint32 {
    properties {
        subtype ObjType;
        rights Rights;
    };
};

type Handles = resource struct {
    plain_handle handle;
    subtype_handle handle:VMO;
    rights_handle handle:<VMO, Rights.TRANSFER>;
};
)FIDL");

  ASSERT_COMPILED(library);
}

// Test that otherwise reserved words can be appropriarely parsed when context
// is clear.
TEST(ParsingTests, GoodParsingReservedWordsInUnionTest) {
  TestLibrary library(R"FIDL(
library example;

type struct = struct {
    field bool;
};

type InUnion = strict union {
    1: foo struct;

    2: as bool;
    3: library bool;
    4: using bool;

    5: array bool;
    6: handle bool;
    7: request bool;
    8: string bool;
    9: vector bool;

   10: bool bool;
   11: int8 bool;
   12: int16 bool;
   13: int32 bool;
   14: int64 bool;
   15: uint8 bool;
   16: uint16 bool;
   17: uint32 bool;
   18: uint64 bool;
   19: float32 bool;
   20: float64 bool;

   21: true bool;
   22: false bool;

   23: reserved bool;
};
)FIDL");
  ASSERT_COMPILED(library);
}

// Test that otherwise reserved words can be appropriately parsed when context
// is clear.
TEST(ParsingTests, GoodParsingReservedWordsInProtocolTest) {
  TestLibrary library(R"FIDL(
library example;

type struct = struct {
    field bool;
};

protocol InProtocol {
    as(struct {
        as bool;
    });
    library(struct {
        library bool;
    });
    using(struct {
        using bool;
    });

    array(struct {
        array bool;
    });
    handle(struct {
        handle bool;
    });
    request(struct {
        request bool;
    });
    string(struct {
        string bool;
    });
    vector(struct {
        vector bool;
    });

    bool(struct {
        bool bool;
    });
    int8(struct {
        int8 bool;
    });
    int16(struct {
        int16 bool;
    });
    int32(struct {
        int32 bool;
    });
    int64(struct {
        int64 bool;
    });
    uint8(struct {
        uint8 bool;
    });
    uint16(struct {
        uint16 bool;
    });
    uint32(struct {
        uint32 bool;
    });
    uint64(struct {
        uint64 bool;
    });
    float32(struct {
        float32 bool;
    });
    float64(struct {
        float64 bool;
    });

    true(struct {
        true bool;
    });
    false(struct {
        false bool;
    });

    reserved(struct {
        reserved bool;
    });

    foo(struct {
        arg struct;
        arg2 int32;
        arg3 struct;
    });
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ParsingTests, BadCharPoundSignTest) {
  TestLibrary library(R"FIDL(
library test;

type Test = struct {
    #uint8 uint8;
};
)FIDL");
  library.ExpectFail(ErrInvalidCharacter, "#");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, BadCharSlashTest) {
  TestLibrary library(R"FIDL(
library test;

type Test = struct / {
    uint8 uint8;
};
)FIDL");
  library.ExpectFail(ErrInvalidCharacter, "/");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, BadIdentifierTest) {
  TestLibrary library;
  library.AddFile("bad/fi-0010-a.noformat.test.fidl");
  library.ExpectFail(ErrInvalidIdentifier, "Foo_");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

class LocaleSwapper {
 public:
  explicit LocaleSwapper(const char* new_locale) {
    old_locale_ = setlocale(LC_ALL, nullptr);
    setlocale(LC_ALL, new_locale);
  }
  ~LocaleSwapper() { setlocale(LC_ALL, old_locale_); }

 private:
  const char* old_locale_;
};

TEST(ParsingTests, BadInvalidCharacterTest) {
  LocaleSwapper swapper("de_DE.iso88591");
  TestLibrary library;
  // This is all alphanumeric in the appropriate locale, but not a valid
  // identifier.
  library.AddFile("bad/fi-0001.noformat.test.fidl");
  library.ExpectFail(ErrInvalidCharacter, std::string_view("ß", 1));
  library.ExpectFail(ErrInvalidCharacter, "ß");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, GoodEmptyStructTest) {
  TestLibrary library(R"FIDL(
library fidl.test.emptystruct;

type Empty = struct {};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ParsingTests, BadErrorOnAliasBeforeImports) {
  SharedAmongstLibraries shared;
  TestLibrary dependency(&shared, "dependent.fidl", R"FIDL(
library dependent;

type Something = struct {};
)FIDL");
  ASSERT_COMPILED(dependency);

  TestLibrary library;
  library.AddFile("bad/fi-0025.noformat.test.fidl");
  library.ExpectFail(ErrLibraryImportsMustBeGroupedAtTopOfFile);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, GoodAttributeValueHasCorrectContents) {
  TestLibrary library(R"FIDL(
  library example;

  @foo("Bar")
  type Empty = struct{};
)FIDL");

  std::unique_ptr<File> ast;
  ASSERT_TRUE(library.Parse(&ast));

  std::unique_ptr<RawAttribute> attribute =
      std::move(ast->type_decls.front()->attributes->attributes.front());
  ASSERT_EQ(attribute->maybe_name->span().data(), "foo");
  ASSERT_TRUE(attribute->args.size() == 1);

  std::unique_ptr<RawAttributeArg> arg = std::move(attribute->args[0]);
  auto arg_value = static_cast<RawLiteralConstant*>(arg->value.get());
  ASSERT_EQ(static_cast<RawStringLiteral*>(arg_value->literal.get())->value, "Bar");
}

TEST(ParsingTests, BadAttributeWithDottedIdentifier) {
  TestLibrary library;
  library.AddFile("bad/fi-0010-b.noformat.test.fidl");
  library.ExpectFail(ErrInvalidIdentifier, "bar.baz");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, GoodAttributeWithMultipleParameters) {
  TestLibrary library;
  library.AddFile("good/fi-0010-b.test.fidl");

  std::unique_ptr<File> ast;
  ASSERT_TRUE(library.Parse(&ast));

  std::unique_ptr<RawAttribute> attribute =
      std::move(ast->type_decls.front()->attributes->attributes.front());
  ASSERT_EQ(attribute->maybe_name->span().data(), "foo");
  ASSERT_TRUE(attribute->args.size() == 2);

  std::unique_ptr<RawAttributeArg> arg1 = std::move(attribute->args[0]);
  ASSERT_EQ(arg1->maybe_name->span().data(), "bar");
  auto arg1_value = static_cast<RawLiteralConstant*>(arg1->value.get());
  ASSERT_EQ(static_cast<RawStringLiteral*>(arg1_value->literal.get())->value, "Bar");

  std::unique_ptr<RawAttributeArg> arg2 = std::move(attribute->args[1]);
  ASSERT_EQ(arg2->maybe_name->span().data(), "zork");
  auto arg2_value = static_cast<RawLiteralConstant*>(arg2->value.get());
  ASSERT_EQ(static_cast<RawStringLiteral*>(arg2_value->literal.get())->value, "Zoom");
}

TEST(ParsingTests, GoodSimpleDocComment) {
  TestLibrary library;
  library.AddFile("good/fi-0027-a.test.fidl");

  std::unique_ptr<File> ast;
  ASSERT_TRUE(library.Parse(&ast));

  std::unique_ptr<RawAttribute> attribute =
      std::move(ast->type_decls.front()->attributes->attributes.front());
  ASSERT_EQ(attribute->provenance, RawAttribute::Provenance::kDocComment);

  // We set the name to "doc" in the flat AST.
  ASSERT_EQ(attribute->maybe_name, nullptr);
  ASSERT_TRUE(attribute->args.size() == 1);

  std::unique_ptr<RawAttributeArg> arg = std::move(attribute->args[0]);
  auto arg_value = static_cast<RawLiteralConstant*>(arg->value.get());
  ASSERT_EQ(static_cast<RawDocCommentLiteral*>(arg_value->literal.get())->value,
            " A doc comment\n");
}

TEST(ParsingTests, GoodMultilineDocCommentHasCorrectContents) {
  TestLibrary library(R"FIDL(
  library example;

  /// A
  /// multiline
  /// comment!
  type Empty = struct {};
)FIDL");

  std::unique_ptr<File> ast;
  ASSERT_TRUE(library.Parse(&ast));

  std::unique_ptr<RawAttribute> attribute =
      std::move(ast->type_decls.front()->attributes->attributes.front());
  ASSERT_EQ(attribute->provenance, RawAttribute::Provenance::kDocComment);
  // We set the name to "doc" in the flat AST.
  ASSERT_EQ(attribute->maybe_name, nullptr);
  ASSERT_TRUE(attribute->args.size() == 1);

  std::unique_ptr<RawAttributeArg> arg = std::move(attribute->args[0]);
  auto arg_value = static_cast<RawLiteralConstant*>(arg->value.get());
  ASSERT_EQ(static_cast<RawDocCommentLiteral*>(arg_value->literal.get())->value,
            " A\n multiline\n comment!\n");
}

TEST(ParsingTests, WarnDocCommentBlankLineTest) {
  TestLibrary library;
  library.AddFile("bad/fi-0027.noformat.test.fidl");

  library.ExpectWarn(WarnBlankLinesWithinDocCommentBlock);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, WarnCommentInsideDocCommentTest) {
  TestLibrary library;
  library.AddFile("bad/fi-0026.noformat.test.fidl");

  library.ExpectWarn(WarnCommentWithinDocCommentBlock);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, WarnDocCommentWithCommentBlankLineTest) {
  TestLibrary library(R"FIDL(
library example;

/// start
// middle

/// end
type Empty = struct {};
)FIDL");

  library.ExpectWarn(WarnCommentWithinDocCommentBlock);
  library.ExpectWarn(WarnBlankLinesWithinDocCommentBlock);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, BadDocCommentNotAllowedOnParams) {
  TestLibrary library;
  library.AddFile("bad/fi-0024.noformat.test.fidl");

  library.ExpectFail(ErrDocCommentOnParameters);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, GoodCommentsSurroundingDocCommentTest) {
  TestLibrary library;
  library.AddFile("good/fi-0026.test.fidl");

  library.set_warnings_as_errors(true);
  ASSERT_COMPILED(library);
}

TEST(ParsingTests, GoodBlankLinesAfterDocCommentTest) {
  TestLibrary library;
  library.AddFile("good/fi-0027-a.test.fidl");

  library.set_warnings_as_errors(true);
  ASSERT_COMPILED(library);
}

TEST(ParsingTests, GoodBlankLinesAfterDocCommentWithCommentTest) {
  TestLibrary library(R"FIDL(
library example;

/// doc comment


// regular comment

type Empty = struct {};
)FIDL");

  library.set_warnings_as_errors(true);
  ASSERT_COMPILED(library);
}

TEST(ParsingTests, WarnTrailingDocCommentTest) {
  TestLibrary library;
  library.AddFile("bad/fi-0028.noformat.test.fidl");

  library.ExpectWarn(WarnDocCommentMustBeFollowedByDeclaration);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, BadTrailingDocCommentInDeclTest) {
  TestLibrary library(R"FIDL(
library example;

type Empty = struct {
   a = int8;
   /// bad
};
)FIDL");

  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kEqual),
                     Token::KindAndSubkind(Token::Kind::kIdentifier));
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kRightCurly),
                     Token::KindAndSubkind(Token::Kind::kIdentifier));
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kEndOfFile),
                     Token::KindAndSubkind(Token::Kind::kIdentifier));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, BadFinalMemberMissingSemicolon) {
  TestLibrary library(R"FIDL(
library example;

type Struct = struct {
    uint_value uint8;
    foo string // error: missing semicolon
};
)FIDL");

  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kRightCurly),
                     Token::KindAndSubkind(Token::Kind::kSemicolon));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, BadFinalMemberMissingTypeAndSemicolon) {
  TestLibrary library(R"FIDL(
library example;

type Struct = struct {
    uint_value uint8;
    string_value
}; // error: want type, got "}"
   // error: want "}", got EOF
)FIDL");

  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kRightCurly),
                     Token::KindAndSubkind(Token::Kind::kIdentifier));
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kEndOfFile),
                     Token::KindAndSubkind(Token::Kind::kIdentifier));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, BadMissingConstraintBrackets) {
  TestLibrary library(R"FIDL(
library example;

type Foo = struct {
    bad_no_brackets vector<uint8>:10,optional;
};
)FIDL");
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kComma),
                     Token::KindAndSubkind(Token::Kind::kSemicolon));
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kComma),
                     Token::KindAndSubkind(Token::Kind::kIdentifier));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, BadMultipleConstraintDefinitionDoubleColon) {
  TestLibrary library;
  library.AddFile("bad/fi-0163.noformat.test.fidl");
  library.ExpectFail(ErrMultipleConstraintDefinitions);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, BadMultipleConstraintDefinitions) {
  TestLibrary library(R"FIDL(
library example;

const LENGTH uint32 = 123;

type Foo = struct {
  bad_double_colon string:LENGTH:optional;
  bad_double_colon_bracketed string:LENGTH:<LENGTH,optional>;
};
)FIDL");
  library.ExpectFail(ErrMultipleConstraintDefinitions);
  library.ExpectFail(ErrMultipleConstraintDefinitions);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, GoodSingleConstraint) {
  TestLibrary library(R"FIDL(
library example;

type Foo = struct {
  with_brackets vector<int32>:<10>;
  without_brackets vector<int32>:10;
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ParsingTests, BadSubtypeConstructor) {
  TestLibrary library;
  library.AddFile("bad/fi-0031.noformat.test.fidl");
  library.ExpectFail(ErrCannotSpecifySubtype, Token::KindAndSubkind(Token::Subkind::kUnion));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, BadLayoutClass) {
  TestLibrary library;
  library.AddFile("bad/fi-0012.noformat.test.fidl");
  library.ExpectFail(ErrInvalidLayoutClass);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, BadIdentifierModifiers) {
  TestLibrary library(R"FIDL(
library example;

type Foo = struct {
  data strict uint32;
};
)FIDL");
  library.ExpectFail(ErrCannotSpecifyModifier, Token::KindAndSubkind(Token::Subkind::kStrict),
                     Token::KindAndSubkind(Token::Kind::kIdentifier));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, BadIdentifierWithConstraintsModifiers) {
  TestLibrary library(R"FIDL(
library example;

type Bar = table {};

type Foo = struct {
  data strict Bar:optional;
};
)FIDL");
  library.ExpectFail(ErrCannotSpecifyModifier, Token::KindAndSubkind(Token::Subkind::kStrict),
                     Token::KindAndSubkind(Token::Kind::kIdentifier));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, BadTypeDeclarationWithConstraintsModifiers) {
  TestLibrary library(R"FIDL(
library example;

type t1 = union { 1: foo uint8; };
type t2 = strict t1;
)FIDL");

  library.ExpectFail(ErrCannotSpecifyModifier, Token::KindAndSubkind(Token::Subkind::kStrict),
                     Token::KindAndSubkind(Token::Kind::kIdentifier));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, BadIdentifierAttributes) {
  TestLibrary library;
  library.AddFile("bad/fi-0022.noformat.test.fidl");
  library.ExpectFail(ErrCannotAttachAttributeToIdentifier);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, BadIdentifierWithConstraintsAttributes) {
  TestLibrary library(R"FIDL(
library example;

type Bar = table {};

type Foo = struct {
  data @foo Bar:optional;
};
)FIDL");
  library.ExpectFail(ErrCannotAttachAttributeToIdentifier);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, BadTypeDeclarationOfEnumLayoutWithInvalidSubtype) {
  TestLibrary library;
  library.AddFile("bad/fi-0013.noformat.test.fidl");
  library.ExpectFail(ErrInvalidWrappedType);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, BadMissingComma) {
  TestLibrary library(R"FIDL(
library example;

type Foo = struct {
  data array<uint8 5>;
};
)FIDL");

  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kNumericLiteral),
                     Token::KindAndSubkind(Token::Kind::kRightAngle));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, BadMissingEqualsValueEnum) {
  TestLibrary library;
  library.AddFile("bad/fi-0008.noformat.test.fidl");
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kSemicolon),
                     Token::KindAndSubkind(Token::Kind::kEqual));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ParsingTests, BadReservedFieldNotAllowed) {
  TestLibrary library;
  library.AddFile("bad/fi-0209.noformat.test.fidl");
  library.ExpectFail(ErrReservedNotAllowed);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

}  // namespace
}  // namespace fidlc
