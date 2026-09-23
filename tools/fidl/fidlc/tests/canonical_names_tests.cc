// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sstream>

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/src/utils.h"
#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

TEST(CanonicalNamesTests, BadCollision) {
  TestLibrary library;
  library.AddFile("bad/fi-0035.test.fidl");
  library.ExpectFail(ErrNameCollisionCanonical, Element::Kind::kProtocol, "Color",
                     Element::Kind::kConst, "COLOR", "bad/fi-0035.test.fidl:6:7", "color");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(CanonicalNamesTests, GoodCollisionFixRename) {
  TestLibrary library;
  library.AddFile("good/fi-0035.test.fidl");
  ASSERT_COMPILED(library);
}

TEST(CanonicalNamesTests, GoodTopLevel) {
  TestLibrary library(R"FIDL(
library example;

alias foobar = bool;
const f_oobar bool = true;
type fo_obar = struct {};
type foo_bar = struct {};
type foob_ar = table {};
type fooba_r = strict union {
    1: x bool;
};
type FoObAr = strict enum {
    A = 1;
};
type FooBaR = strict bits {
    A = 1;
};
protocol FoObaR {};
service FOoBAR {};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(CanonicalNamesTests, GoodAttributes) {
  TestLibrary library(R"FIDL(
library example;

@foobar
@foo_bar
@f_o_o_b_a_r
type Example = struct {};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(CanonicalNamesTests, GoodAttributeArguments) {
  TestLibrary library(R"FIDL(
library example;

@some_attribute(foobar="", foo_bar="", f_o_o_b_a_r="")
type Example = struct {};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(CanonicalNamesTests, GoodStructMembers) {
  TestLibrary library(R"FIDL(
library example;

type Example = struct {
    foobar bool;
    foo_bar bool;
    f_o_o_b_a_r bool;
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(CanonicalNamesTests, GoodTableMembers) {
  TestLibrary library(R"FIDL(
library example;

type Example = table {
    1: foobar bool;
    2: foo_bar bool;
    3: f_o_o_b_a_r bool;
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(CanonicalNamesTests, GoodUnionMembers) {
  TestLibrary library(R"FIDL(
library example;

type Example = strict union {
    1: foobar bool;
    2: foo_bar bool;
    3: f_o_o_b_a_r bool;
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(CanonicalNamesTests, GoodEnumMembers) {
  TestLibrary library(R"FIDL(
library example;

type Example = strict enum {
    foobar = 1;
    foo_bar = 2;
    f_o_o_b_a_r = 3;
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(CanonicalNamesTests, GoodBitsMembers) {
  TestLibrary library(R"FIDL(
library example;

type Example = strict bits {
    foobar = 1;
    foo_bar = 2;
    f_o_o_b_a_r = 4;
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(CanonicalNamesTests, GoodProtocolMethods) {
  TestLibrary library(R"FIDL(
library example;

protocol Example {
    foobar() -> ();
    foo_bar() -> ();
    f_o_o_b_a_r() -> ();
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(CanonicalNamesTests, GoodMethodParameters) {
  TestLibrary library(R"FIDL(
library example;

protocol Example {
    example(struct {
        foobar bool;
        foo_bar bool;
        f_o_o_b_a_r bool;
    }) -> ();
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(CanonicalNamesTests, GoodMethodResults) {
  TestLibrary library(R"FIDL(
library example;

protocol Example {
    example() -> (struct {
        foobar bool;
        foo_bar bool;
        f_o_o_b_a_r bool;
    });
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(CanonicalNamesTests, GoodServiceMembers) {
  TestLibrary library(R"FIDL(
library example;

protocol P {};
service Example {
    foobar client_end:P;
    foo_bar client_end:P;
    f_o_o_b_a_r client_end:P;
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(CanonicalNamesTests, GoodResourceProperties) {
  TestLibrary library(R"FIDL(
library example;

resource_definition Example {
    properties {
        // This property is required for compilation, but is not otherwise under test.
        subtype flexible enum : uint32 {};
        foobar uint32;
        foo_bar uint32;
        f_o_o_b_a_r uint32;
    };
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(CanonicalNamesTests, GoodUpperAcronym) {
  TestLibrary library(R"FIDL(
library example;

type HTTPServer = struct {};
type httpserver = struct {};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(CanonicalNamesTests, GoodCurrentLibrary) {
  TestLibrary library(R"FIDL(
library example;

type example = struct {};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(CanonicalNamesTests, GoodDependentLibrary) {
  SharedAmongstLibraries shared;
  TestLibrary dependency(&shared, "foobar.fidl", R"FIDL(
library foobar;

type Something = struct {};
)FIDL");
  ASSERT_COMPILED(dependency);

  TestLibrary library(&shared, "example.fidl", R"FIDL(
library example;

using foobar;

alias f_o_o_b_a_r = foobar.Something;
const f_oobar bool = true;
type fo_obar = struct {};
type foo_bar = struct {};
type foob_ar = table {};
type fooba_r = union { 1: x bool; };
type FoObAr = enum { A = 1; };
type FooBaR = bits { A = 1; };
protocol FoObaR {};
service FOoBAR {};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(CanonicalNamesTests, BadTopLevel) {
  using Kind = Element::Kind;
  const std::pair<Kind, std::string_view> lower[] = {
      {Kind::kAlias, "alias fooBar = bool;"},
      {Kind::kConst, "const fooBar bool = true;"},
      {Kind::kStruct, "type fooBar = struct {};"},
      {Kind::kTable, "type fooBar = table {};"},
      {Kind::kUnion, "type fooBar = union { 1: x bool; };"},
      {Kind::kEnum, "type fooBar = enum { A = 1; };"},
      {Kind::kBits, "type fooBar = bits { A = 1; };"},
      {Kind::kProtocol, "protocol fooBar {};"},
      {Kind::kService, "service fooBar {};"},
  };
  const std::pair<Kind, std::string_view> upper[] = {
      {Kind::kAlias, "alias FooBar = bool;"},
      {Kind::kConst, "const FooBar bool = true;"},
      {Kind::kStruct, "type FooBar = struct {};"},
      {Kind::kTable, "type FooBar = table {};"},
      {Kind::kUnion, "type FooBar = union { 1: x bool; };"},
      {Kind::kEnum, "type FooBar = enum { A = 1; };"},
      {Kind::kBits, "type FooBar = bits { A = 1; };"},
      {Kind::kProtocol, "protocol FooBar {};"},
      {Kind::kService, "service FooBar {};"},
  };

  for (auto& [lowerKind, lowerLine] : lower) {
    for (auto& [upperKind, upperLine] : upper) {
      std::ostringstream s;
      s << "library example;\n" << lowerLine << '\n' << upperLine << '\n';
      const auto fidl = s.str();
      SCOPED_TRACE(fidl);
      TestLibrary library(fidl);
      char location[20];
      snprintf(location, sizeof location, "example.fidl:3:%zu", 1 + upperLine.find("FooBar"));
      library.ExpectFail(ErrNameCollisionCanonical, lowerKind, "fooBar", upperKind, "FooBar",
                         location, "foo_bar");
      ASSERT_COMPILER_DIAGNOSTICS(library);
    }
  }
}

TEST(CanonicalNamesTests, BadAttributes) {
  TestLibrary library(R"FIDL(
library example;

@fooBar
@FooBar
type Example = struct {};
)FIDL");
  library.ExpectFail(ErrDuplicateAttributeCanonical, "FooBar", "fooBar", "example.fidl:4:2",
                     "foo_bar");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(CanonicalNamesTests, BadAttributeArguments) {
  TestLibrary library(R"FIDL(
library example;

@some_attribute(fooBar="", FooBar="")
type Example = struct {};
)FIDL");
  library.ExpectFail(ErrDuplicateAttributeArgCanonical, "some_attribute", "FooBar", "fooBar",
                     "example.fidl:4:17", "foo_bar");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(CanonicalNamesTests, BadStructMembers) {
  TestLibrary library(R"FIDL(
library example;

type MyStruct = struct {
    myStructMember string;
    MyStructMember uint64;
};
)FIDL");
  library.ExpectFail(ErrNameCollisionCanonical, Element::Kind::kStructMember, "MyStructMember",
                     Element::Kind::kStructMember, "myStructMember", "example.fidl:5:5",
                     "my_struct_member");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(CanonicalNamesTests, BadTableMembers) {
  TestLibrary library(R"FIDL(
library example;

type MyTable = table {
    1: myField bool;
    2: MyField bool;
};
)FIDL");
  library.ExpectFail(ErrNameCollisionCanonical, Element::Kind::kTableMember, "MyField",
                     Element::Kind::kTableMember, "myField", "example.fidl:5:8", "my_field");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(CanonicalNamesTests, BadUnionMembers) {
  TestLibrary library(R"FIDL(
library example;

type MyUnion = union {
    1: myVariant bool;
    2: MyVariant bool;
};
)FIDL");
  library.ExpectFail(ErrNameCollisionCanonical, Element::Kind::kUnionMember, "MyVariant",
                     Element::Kind::kUnionMember, "myVariant", "example.fidl:5:8", "my_variant");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(CanonicalNamesTests, BadEnumMembers) {
  TestLibrary library(R"FIDL(
library example;

type Example = enum {
  fooBar = 1;
  FooBar = 2;
};
)FIDL");
  library.ExpectFail(ErrNameCollisionCanonical, Element::Kind::kEnumMember, "FooBar",
                     Element::Kind::kEnumMember, "fooBar", "example.fidl:5:3", "foo_bar");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(CanonicalNamesTests, BadBitsMembers) {
  TestLibrary library(R"FIDL(
library example;

type MyBits = bits {
    fooBar = 1;
    FooBar = 2;
};
)FIDL");
  library.ExpectFail(ErrNameCollisionCanonical, Element::Kind::kBitsMember, "FooBar",
                     Element::Kind::kBitsMember, "fooBar", "example.fidl:5:5", "foo_bar");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(CanonicalNamesTests, BadProtocolMethods) {
  TestLibrary library(R"FIDL(
library example;

protocol MyProtocol {
    strict myMethod() -> ();
    strict MyMethod() -> ();
};
)FIDL");
  library.ExpectFail(ErrNameCollisionCanonical, Element::Kind::kProtocolMethod, "MyMethod",
                     Element::Kind::kProtocolMethod, "myMethod",
                     library.find_source_span("myMethod"), "my_method");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(CanonicalNamesTests, BadMethodParameters) {
  TestLibrary library(R"FIDL(
library example;

protocol Example {
  example(struct { fooBar bool; FooBar bool; }) -> ();
};
)FIDL");
  library.ExpectFail(ErrNameCollisionCanonical, Element::Kind::kStructMember, "FooBar",
                     Element::Kind::kStructMember, "fooBar", "example.fidl:5:20", "foo_bar");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(CanonicalNamesTests, BadMethodResults) {
  TestLibrary library(R"FIDL(
library example;

protocol Example {
  example() -> (struct { fooBar bool; FooBar bool; });
};
)FIDL");
  library.ExpectFail(ErrNameCollisionCanonical, Element::Kind::kStructMember, "FooBar",
                     Element::Kind::kStructMember, "fooBar", "example.fidl:5:26", "foo_bar");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(CanonicalNamesTests, BadServiceMembers) {
  TestLibrary library(R"FIDL(
library example;

protocol MyProtocol {};

service MyService {
    myServiceMember client_end:MyProtocol;
    MyServiceMember client_end:MyProtocol;
};
)FIDL");
  library.ExpectFail(ErrNameCollisionCanonical, Element::Kind::kServiceMember, "MyServiceMember",
                     Element::Kind::kServiceMember, "myServiceMember", "example.fidl:7:5",
                     "my_service_member");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(CanonicalNamesTests, BadResourceProperties) {
  TestLibrary library(R"FIDL(
library example;

resource_definition MyResource : uint32 {
    properties {
        subtype flexible enum : uint32 {};
        rights uint32;
        Rights uint32;
    };
};
)FIDL");
  library.ExpectFail(ErrNameCollisionCanonical, Element::Kind::kResourceProperty, "Rights",
                     Element::Kind::kResourceProperty, "rights", "example.fidl:7:9", "rights");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(CanonicalNamesTests, BadMemberValues) {
  TestLibrary library;
  library.AddFile("bad/fi-0054.test.fidl");
  library.ExpectFail(ErrMemberNotFound, "enum 'Enum'", "FOO_BAR");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(CanonicalNamesTests, BadUpperAcronym) {
  TestLibrary library(R"FIDL(
library example;

type HTTPServer = struct {};
type HttpServer = struct {};
)FIDL");
  library.ExpectFail(ErrNameCollisionCanonical, Element::Kind::kStruct, "HttpServer",
                     Element::Kind::kStruct, "HTTPServer", "example.fidl:4:6", "http_server");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(CanonicalNamesTests, BadDependentLibrary) {
  SharedAmongstLibraries shared;
  TestLibrary dependency(&shared, "foobar.fidl", R"FIDL(
library foobar;

type Something = struct {};
)FIDL");
  ASSERT_COMPILED(dependency);

  TestLibrary library(&shared, "lib.fidl", R"FIDL(
library example;

using foobar;

alias FOOBAR = foobar.Something;
)FIDL");
  library.ExpectFail(ErrDeclNameConflictsWithLibraryImportCanonical, "FOOBAR", "foobar");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(CanonicalNamesTests, BadVariousCollisions) {
  const auto base_names = {
      "a", "a1", "x_single_start", "single_end_x", "x_single_both_x", "single_x_middle",
  };
  const auto functions = {
      ToLowerSnakeCase,
      ToUpperSnakeCase,
      ToLowerCamelCase,
      ToUpperCamelCase,
  };

  for (const auto base_name : base_names) {
    for (const auto f1 : functions) {
      for (const auto f2 : functions) {
        std::ostringstream s;
        std::string name1 = f1(base_name);
        std::string name2 = f2(base_name);
        s << "library example;\ntype " << name1 << " = struct {};\ntype " << name2
          << " = struct {};";
        auto fidl = s.str();
        SCOPED_TRACE(fidl);
        TestLibrary library(fidl);
        if (name1 == name2) {
          library.ExpectFail(ErrNameCollision, Element::Kind::kStruct, name1,
                             Element::Kind::kStruct, "example.fidl:2:6");
        } else if (name1 < name2) {
          // We compile name1 first, and see that name2 collides with it.
          library.ExpectFail(ErrNameCollisionCanonical, Element::Kind::kStruct, name2,
                             Element::Kind::kStruct, name1, "example.fidl:2:6",
                             Canonicalize(name1));
        } else {
          // We compile name2 first, and see that name1 collides with it.
          library.ExpectFail(ErrNameCollisionCanonical, Element::Kind::kStruct, name1,
                             Element::Kind::kStruct, name2, "example.fidl:3:6",
                             Canonicalize(name1));
        }
        ASSERT_COMPILER_DIAGNOSTICS(library);
      }
    }
  }
}

TEST(CanonicalNamesTests, BadConsecutiveUnderscores) {
  TestLibrary library(R"FIDL(
library example;

type it_is_the_same = struct {};
type it__is___the____same = struct {};
)FIDL");
  library.ExpectFail(ErrNameCollisionCanonical, Element::Kind::kStruct, "it_is_the_same",
                     Element::Kind::kStruct, "it__is___the____same", "example.fidl:5:6",
                     "it_is_the_same");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(CanonicalNamesTests, BadInconsistentTypeSpelling) {
  const auto decl_templates = {
      "alias %s = bool;",
      "type %s = struct {};",
      "type %s = struct {};",
      "type %s = table {};",
      "type %s = union { 1: x bool; };",
      "type %s = enum { A = 1; };",
      "type %s = bits { A = 1; };",
  };
  const auto use_template = "type Example = struct { val %s; };";

  const auto names = {
      std::make_pair("foo_bar", "FOO_BAR"),
      std::make_pair("FOO_BAR", "foo_bar"),
      std::make_pair("fooBar", "FooBar"),
  };

  for (const auto decl_template : decl_templates) {
    for (const auto& [decl_name, use_name] : names) {
      std::string decl(decl_template), use(use_template);
      decl.replace(decl.find("%s"), 2, decl_name);
      use.replace(use.find("%s"), 2, use_name);
      std::ostringstream s;
      s << "library example;\n" << decl << '\n' << use;
      auto fidl = s.str();
      SCOPED_TRACE(fidl);
      TestLibrary library(fidl);
      library.ExpectFail(ErrNameNotFound, use_name, "library 'example'");
      ASSERT_COMPILER_DIAGNOSTICS(library);
    }
  }
}

TEST(CanonicalNamesTests, BadInconsistentConstSpelling) {
  const auto names = {
      std::make_pair("foo_bar", "FOO_BAR"),
      std::make_pair("FOO_BAR", "foo_bar"),
      std::make_pair("fooBar", "FooBar"),
  };

  for (const auto& [decl_name, use_name] : names) {
    std::ostringstream s;
    s << "library example;\n"
      << "const " << decl_name << " bool = false;\n"
      << "const EXAMPLE bool = " << use_name << ";";
    auto fidl = s.str();
    TestLibrary library(fidl);
    library.ExpectFail(ErrNameNotFound, use_name, "library 'example'");
    ASSERT_COMPILER_DIAGNOSTICS(library);
  }
}

TEST(CanonicalNamesTests, BadInconsistentEnumMemberSpelling) {
  const auto names = {
      std::make_pair("foo_bar", "FOO_BAR"),
      std::make_pair("FOO_BAR", "foo_bar"),
      std::make_pair("fooBar", "FooBar"),
  };

  for (const auto& [decl_name, use_name] : names) {
    std::ostringstream s;
    s << "library example;\n"
      << "type Enum = enum { " << decl_name << " = 1; };\n"
      << "const EXAMPLE Enum = Enum." << use_name << ";";
    auto fidl = s.str();
    SCOPED_TRACE(fidl);
    TestLibrary library(fidl);
    library.ExpectFail(ErrMemberNotFound, "enum 'Enum'", use_name);
    ASSERT_COMPILER_DIAGNOSTICS(library);
  }
}

TEST(CanonicalNamesTests, BadInconsistentBitsMemberSpelling) {
  const auto names = {
      std::make_pair("foo_bar", "FOO_BAR"),
      std::make_pair("FOO_BAR", "foo_bar"),
      std::make_pair("fooBar", "FooBar"),
  };

  for (const auto& [decl_name, use_name] : names) {
    std::ostringstream s;
    s << "library example;\n"
      << "type Bits = bits { " << decl_name << " = 1; };\n"
      << "const EXAMPLE Bits = Bits." << use_name << ";";
    auto fidl = s.str();
    SCOPED_TRACE(fidl);
    TestLibrary library(fidl);
    library.ExpectFail(ErrMemberNotFound, "bits 'Bits'", use_name);
    ASSERT_COMPILER_DIAGNOSTICS(library);
  }
}

}  // namespace
}  // namespace fidlc
