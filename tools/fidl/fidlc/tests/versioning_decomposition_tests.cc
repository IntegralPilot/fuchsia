// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fstream>

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/tests/test_library.h"

// This file tests the temporal decomposition algorithm by comparing the JSON IR
// resulting from a versioned library and its manually decomposed equivalents.

namespace fidlc {
namespace {

// Returns true if str starts with prefix.
bool StartsWith(std::string_view str, std::string_view prefix) {
  return str.substr(0, prefix.size()) == prefix;
}

// If the line starts with whitespace followed by str, returns the whitespace.
std::optional<std::string> GetSpaceBefore(std::string_view line, std::string_view str) {
  size_t i = 0;
  while (i < line.size() && line[i] == ' ') {
    i++;
  }
  if (StartsWith(line.substr(i), str)) {
    return std::string(line.substr(0, i));
  }
  return std::nullopt;
}

// Erases fields from a JSON IR string that manual decomposition can change:
//
// * "platform": changes in DecompositionTests.UnversionedLibrary.
// * "location": decomposing changes all locations.
// * "maybe_attributes": decomposing changes @available attributes.
// * "declaration_order": decomposing can change the DFS order.
//
// Also removes all end-of-line commas since these can cause spurious diffs.
// Note that this means the returned string is not valid JSON.
std::string ScrubJson(const std::string& json) {
  // We scan the JSON line by line, filtering out the undesired lines. To do
  // this, we rely on JsonWriter emitting correct indentation and newlines.
  std::istringstream input(json);
  std::ostringstream output;
  std::string line;
  std::optional<std::string> skip_until;
  while (std::getline(input, line)) {
    if (skip_until) {
      if (StartsWith(line, skip_until.value())) {
        skip_until = std::nullopt;
      }
      continue;
    }
    if (GetSpaceBefore(line, "\"platform\": \"")) {
      // Skip platform line.
    } else if (auto indent = GetSpaceBefore(line, "\"location\": {")) {
      skip_until = indent.value() + '}';
    } else if (auto indent = GetSpaceBefore(line, "\"maybe_attributes\": [")) {
      skip_until = indent.value() + ']';
    } else if (auto indent = GetSpaceBefore(line, "\"declaration_order\": [")) {
      skip_until = indent.value() + ']';
    } else {
      if (line.back() == ',') {
        line.pop_back();
      }
      output << line << '\n';
    }
  }
  return output.str();
}

// Platform name and library name for all test libraries in this file.
const char* kLibraryName = "example";

// Helper function to implement ASSERT_EQUIVALENT.
void AssertEquivalent(const std::string& left_fidl, const std::string& right_fidl,
                      std::string_view versions_str) {
  TargetVersions versions(versions_str);
  TestLibrary left_lib(left_fidl);
  left_lib.SelectVersions(kLibraryName, versions);
  ASSERT_COMPILED(left_lib);
  ASSERT_EQ(left_lib.name(), kLibraryName);
  TestLibrary right_lib(right_fidl);
  right_lib.SelectVersions(kLibraryName, versions);
  ASSERT_COMPILED(right_lib);
  ASSERT_EQ(right_lib.name(), kLibraryName);
  auto left_json = ScrubJson(left_lib.GenerateJSON());
  auto right_json = ScrubJson(right_lib.GenerateJSON());
  if (left_json != right_json) {
    std::ofstream output_left("decomposition_tests_left.txt");
    output_left << left_json;
    output_left.close();
    std::ofstream output_right("decomposition_tests_right.txt");
    output_right << right_json;
    output_right.close();
  }
  ASSERT_EQ(left_json, right_json)
      << "To compare results, run:\n\n"
         "diff $(cat $FUCHSIA_DIR/.fx-build-dir)/decomposition_tests_{left,right}.txt\n";
}

// Asserts that left_fidl and right_fidl compile to JSON IR that is identical
// after scrubbbing (see ScrubJson) for the given version.
#define ASSERT_EQUIVALENT(left_fidl, right_fidl, version)          \
  {                                                                \
    SCOPED_TRACE("ASSERT_EQUIVALENT failed for version " version); \
    AssertEquivalent(left_fidl, right_fidl, version);              \
  }

TEST(VersioningDecompositionTests, EquivalentToSelf) {
  auto fidl = R"FIDL(
@available(added=1)
library example;
)FIDL";

  ASSERT_EQUIVALENT(fidl, fidl, "1");
  ASSERT_EQUIVALENT(fidl, fidl, "2");
  ASSERT_EQUIVALENT(fidl, fidl, "HEAD");
  ASSERT_EQUIVALENT(fidl, fidl, "1,HEAD");
  ASSERT_EQUIVALENT(fidl, fidl, "1,2,HEAD");
}

// An unversioned library behaves the same as an unchanging versioned library.
TEST(VersioningDecompositionTests, UnversionedLibrary) {
  auto unversioned = R"FIDL(
library example;

type Foo = struct {};
)FIDL";

  auto versioned = R"FIDL(
@available(added=1)
library example;

type Foo = struct {};
)FIDL";

  ASSERT_EQUIVALENT(unversioned, versioned, "1");
  ASSERT_EQUIVALENT(unversioned, versioned, "2");
  ASSERT_EQUIVALENT(unversioned, versioned, "HEAD");
  ASSERT_EQUIVALENT(unversioned, versioned, "1,HEAD");
  ASSERT_EQUIVALENT(unversioned, versioned, "1,2,HEAD");
}

TEST(VersioningDecompositionTests, AbsentLibraryIsEmpty) {
  auto fidl = R"FIDL(
@available(added=2, removed=3)
library example;

type Foo = struct {};
)FIDL";

  auto v1 = R"FIDL(
@available(added=1, removed=2)
library example;
)FIDL";

  auto v2 = R"FIDL(
@available(added=2, removed=3)
library example;

type Foo = struct {};
)FIDL";

  auto v3_onward = R"FIDL(
@available(added=3)
library example;
)FIDL";

  ASSERT_EQUIVALENT(fidl, v1, "1");
  ASSERT_EQUIVALENT(fidl, v2, "2");
  ASSERT_EQUIVALENT(fidl, v3_onward, "3");
  ASSERT_EQUIVALENT(fidl, v3_onward, "HEAD");
  ASSERT_EQUIVALENT(fidl, v3_onward, "1,3,HEAD");
  ASSERT_EQUIVALENT(fidl, v2, "1,2,3,HEAD");
}

TEST(VersioningDecompositionTests, SplitByMembership) {
  auto fidl = R"FIDL(
@available(added=1)
library example;

type TopLevel = struct {
    @available(added=2)
    first uint32;
};
)FIDL";

  auto v1 = R"FIDL(
@available(added=1, removed=2)
library example;

type TopLevel = struct {};
)FIDL";

  auto v2_onward = R"FIDL(
@available(added=2)
library example;

type TopLevel = struct {
    first uint32;
};
)FIDL";

  ASSERT_EQUIVALENT(fidl, v1, "1");
  ASSERT_EQUIVALENT(fidl, v2_onward, "2");
  ASSERT_EQUIVALENT(fidl, v2_onward, "HEAD");
  ASSERT_EQUIVALENT(fidl, v2_onward, "1,HEAD");
  ASSERT_EQUIVALENT(fidl, v2_onward, "1,2,HEAD");
}

TEST(VersioningDecompositionTests, SplitByModifier) {
  auto fidl = R"FIDL(
@available(added=1)
library example;

type TopLevel = resource(added=2) struct {};
)FIDL";

  auto v1 = R"FIDL(
@available(added=1, removed=2)
library example;

type TopLevel = struct {};
)FIDL";

  auto v2_onward = R"FIDL(
@available(added=2)
library example;

type TopLevel = resource struct {};
)FIDL";

  ASSERT_EQUIVALENT(fidl, v1, "1");
  ASSERT_EQUIVALENT(fidl, v2_onward, "2");
  ASSERT_EQUIVALENT(fidl, v2_onward, "HEAD");
  ASSERT_EQUIVALENT(fidl, v2_onward, "1,HEAD");
  ASSERT_EQUIVALENT(fidl, v2_onward, "1,2,HEAD");
}

TEST(VersioningDecompositionTests, SplitByReference) {
  auto fidl = R"FIDL(
@available(added=1)
library example;

type This = struct {
    this_member That;
};

type That = struct {
    @available(added=2)
    that_member uint32;
};
)FIDL";

  auto v1 = R"FIDL(
@available(added=1, removed=2)
library example;

type This = struct {
    this_member That;
};

type That = struct {};
)FIDL";

  auto v2_onward = R"FIDL(
@available(added=2)
library example;

type This = struct {
    this_member That;
};

type That = struct {
    that_member uint32;
};
)FIDL";

  ASSERT_EQUIVALENT(fidl, v1, "1");
  ASSERT_EQUIVALENT(fidl, v2_onward, "2");
  ASSERT_EQUIVALENT(fidl, v2_onward, "HEAD");
  ASSERT_EQUIVALENT(fidl, v2_onward, "1,HEAD");
  ASSERT_EQUIVALENT(fidl, v2_onward, "1,2,HEAD");
}

TEST(VersioningDecompositionTests, SplitByTwoMembers) {
  auto fidl = R"FIDL(
@available(added=1)
library example;

type This = struct {
    @available(added=2)
    first That;
    @available(added=3)
    second That;
};

type That = struct {};
)FIDL";

  auto v1 = R"FIDL(
@available(added=1, removed=2)
library example;

type This = struct {};

type That = struct {};
)FIDL";

  auto v2 = R"FIDL(
@available(added=2, removed=3)
library example;

type This = struct {
    first That;
};

type That = struct {};
)FIDL";

  auto v3_onward = R"FIDL(
@available(added=3)
library example;

type This = struct {
    first That;
    second That;
};

type That = struct {};
)FIDL";

  ASSERT_EQUIVALENT(fidl, v1, "1");
  ASSERT_EQUIVALENT(fidl, v2, "2");
  ASSERT_EQUIVALENT(fidl, v3_onward, "3");
  ASSERT_EQUIVALENT(fidl, v3_onward, "HEAD");
  ASSERT_EQUIVALENT(fidl, v3_onward, "1,HEAD");
  ASSERT_EQUIVALENT(fidl, v3_onward, "1,2,3,HEAD");
}

TEST(VersioningDecompositionTests, Recursion) {
  auto fidl = R"FIDL(
@available(added=1)
library example;

type Expr = flexible union {
    1: num int64;

    @available(removed=3)
    2: add struct {
        left Expr:optional;
        right Expr:optional;
    };

    @available(added=2, removed=3)
    3: mul struct {
        left Expr:optional;
        right Expr:optional;
    };

    @available(added=3)
    4: bin struct {
        kind flexible enum {
            ADD = 1;
            MUL = 2;
            DIV = 3;

            @available(added=4)
            MOD = 4;
        };
        left Expr:optional;
        right Expr:optional;
    };
};
)FIDL";

  auto v1 = R"FIDL(
@available(added=1, removed=2)
library example;

type Expr = flexible union {
    1: num int64;
    2: add struct {
        left Expr:optional;
        right Expr:optional;
    };
};
)FIDL";

  auto v2 = R"FIDL(
@available(added=2, removed=3)
library example;

type Expr = flexible union {
    1: num int64;
    2: add struct {
        left Expr:optional;
        right Expr:optional;
    };
    3: mul struct {
        left Expr:optional;
        right Expr:optional;
    };
};
)FIDL";

  auto v3 = R"FIDL(
@available(added=3, removed=4)
library example;

type Expr = flexible union {
    1: num int64;
    4: bin struct {
        kind flexible enum {
            ADD = 1;
            MUL = 2;
            DIV = 3;
        };
        left Expr:optional;
        right Expr:optional;
    };
};
)FIDL";

  auto v4_onward = R"FIDL(
@available(added=4)
library example;

type Expr = flexible union {
    1: num int64;
    4: bin struct {
        kind flexible enum {
            ADD = 1;
            MUL = 2;
            DIV = 3;
            MOD = 4;
        };
        left Expr:optional;
        right Expr:optional;
    };
};
)FIDL";

  auto all_versions = R"FIDL(
library example;

type Expr = flexible union {
    1: num int64;

    2: add struct {
        left Expr:optional;
        right Expr:optional;
    };

    3: mul struct {
        left Expr:optional;
        right Expr:optional;
    };

    4: bin struct {
        kind flexible enum {
            ADD = 1;
            MUL = 2;
            DIV = 3;
            MOD = 4;
        };
        left Expr:optional;
        right Expr:optional;
    };
};
)FIDL";

  ASSERT_EQUIVALENT(fidl, v1, "1");
  ASSERT_EQUIVALENT(fidl, v2, "2");
  ASSERT_EQUIVALENT(fidl, v3, "3");
  ASSERT_EQUIVALENT(fidl, v4_onward, "4");
  ASSERT_EQUIVALENT(fidl, v4_onward, "HEAD");
  ASSERT_EQUIVALENT(fidl, all_versions, "1,2,3,4,HEAD");
}

TEST(VersioningDecompositionTests, MutualRecursion) {
  auto fidl = R"FIDL(
@available(added=1)
library example;

@available(added=2)
type Foo = table {
    1: str string;
    @available(added=3)
    // Struct wrapper needed because tables aren't allowed to be boxed.
    2: bars vector<box<struct { bar Bar; }>>;
};

@available(added=2)
type Bar = table {
    @available(removed=5)
    // OuterStruct needed because aren't allowed to contain boxes.
    // InnerStruct needed because tables aren't allowed to be boxed.
    1: foo @generated_name("OuterStruct") struct {
        inner box<@generated_name("InnerStruct") struct { foo Foo; }>;
    };
    @available(added=4)
    2: str string;
};
)FIDL";

  auto v1 = R"FIDL(
@available(added=1, removed=2)
library example;
)FIDL";

  auto v2 = R"FIDL(
@available(added=2, removed=3)
library example;

type Foo = table {
    1: str string;
};

type Bar = table {
    1: foo @generated_name("OuterStruct") struct {
        inner box<@generated_name("InnerStruct") struct { foo Foo; }>;
    };
};
)FIDL";

  auto v3 = R"FIDL(
@available(added=3, removed=4)
library example;

type Foo = table {
    1: str string;
    2: bars vector<box<struct { bar Bar; }>>;
};

type Bar = table {
    1: foo @generated_name("OuterStruct") struct {
        inner box<@generated_name("InnerStruct") struct { foo Foo; }>;
    };
};
)FIDL";

  auto v4 = R"FIDL(
@available(added=4, removed=5)
library example;

type Foo = table {
    1: str string;
    2: bars vector<box<struct { bar Bar; }>>;
};

type Bar = table {
    1: foo @generated_name("OuterStruct") struct {
        inner box<@generated_name("InnerStruct") struct { foo Foo; }>;
    };
    2: str string;
};
)FIDL";

  auto v5_onward = R"FIDL(
@available(added=5)
library example;

type Foo = table {
    1: str string;
    2: bars vector<box<struct { bar Bar; }>>;
};

type Bar = table {
    2: str string;
};
)FIDL";

  auto all_versions = R"FIDL(
library example;

type Foo = table {
    1: str string;
    2: bars vector<box<struct { bar Bar; }>>;
};

type Bar = table {
    1: foo @generated_name("OuterStruct") struct {
        inner box<@generated_name("InnerStruct") struct { foo Foo; }>;
    };
    2: str string;
};
)FIDL";

  ASSERT_EQUIVALENT(fidl, v1, "1");
  ASSERT_EQUIVALENT(fidl, v2, "2");
  ASSERT_EQUIVALENT(fidl, v3, "3");
  ASSERT_EQUIVALENT(fidl, v4, "4");
  ASSERT_EQUIVALENT(fidl, v5_onward, "5");
  ASSERT_EQUIVALENT(fidl, v5_onward, "HEAD");
  ASSERT_EQUIVALENT(fidl, all_versions, "1,2,3,4,5,HEAD");
}

TEST(VersioningDecompositionTests, MisalignedSwapping) {
  auto fidl = R"FIDL(
@available(added=1)
library example;

@available(replaced=4)
const LEN uint64 = 16;
@available(added=4)
const LEN uint64 = 32;

@available(added=2)
type Foo = table {
    @available(replaced=3)
    1: bar string;
    @available(added=3)
    1: bar string:LEN;
};
)FIDL";

  auto v1 = R"FIDL(
@available(added=1, removed=2)
library example;

const LEN uint64 = 16;
)FIDL";

  auto v2 = R"FIDL(
@available(added=2, removed=3)
library example;

const LEN uint64 = 16;
type Foo = table {
    1: bar string;
};
)FIDL";

  auto v3 = R"FIDL(
@available(added=3, removed=4)
library example;

const LEN uint64 = 16;
type Foo = table {
    1: bar string:LEN;
};
)FIDL";

  auto v4_onward = R"FIDL(
@available(added=4)
library example;

const LEN uint64 = 32;
type Foo = table {
    1: bar string:LEN;
};
)FIDL";

  ASSERT_EQUIVALENT(fidl, v1, "1");
  ASSERT_EQUIVALENT(fidl, v2, "2");
  ASSERT_EQUIVALENT(fidl, v3, "3");
  ASSERT_EQUIVALENT(fidl, v4_onward, "4");
  ASSERT_EQUIVALENT(fidl, v4_onward, "HEAD");
  ASSERT_EQUIVALENT(fidl, v4_onward, "1,HEAD");
  ASSERT_EQUIVALENT(fidl, v4_onward, "1,2,3,4,HEAD");
}

TEST(VersioningDecompositionTests, StrictToFlexible) {
  auto fidl = R"FIDL(
@available(added=1)
library example;

type X = struct {
    @available(added=2, removed=4)
    y Y;
};

@available(added=2)
type Y = strict(removed=3) flexible(added=3) enum { A = 1; };
)FIDL";

  auto v1 = R"FIDL(
@available(added=1, removed=2)
library example;

type X = struct {};
)FIDL";

  auto v2 = R"FIDL(
@available(added=2, removed=3)
library example;

type X = struct {
    y Y;
};

type Y = strict enum { A = 1; };
)FIDL";

  auto v3 = R"FIDL(
@available(added=3, removed=4)
library example;

type X = struct {
    y Y;
};

type Y = flexible enum { A = 1; };
)FIDL";

  auto v4_onward = R"FIDL(
@available(added=4)
library example;

type X = struct {};

type Y = flexible enum { A = 1; };
)FIDL";

  auto all_versions = R"FIDL(
library example;

type X = struct {
    y Y;
};

type Y = flexible enum { A = 1; };
)FIDL";

  ASSERT_EQUIVALENT(fidl, v1, "1");
  ASSERT_EQUIVALENT(fidl, v2, "2");
  ASSERT_EQUIVALENT(fidl, v3, "3");
  ASSERT_EQUIVALENT(fidl, v4_onward, "4");
  ASSERT_EQUIVALENT(fidl, v4_onward, "HEAD");
  ASSERT_EQUIVALENT(fidl, all_versions, "1,2,3,4,HEAD");
}

TEST(VersioningDecompositionTests, NameReuse) {
  auto fidl = R"FIDL(
@available(added=1)
library example;

@available(added=2, removed=3)
type Foo = struct {
    bar Bar;
};
@available(added=1, replaced=4)
type Bar = struct {};

@available(added=4, removed=7)
type Foo = struct {};
@available(added=4, removed=6)
type Bar = struct {
    foo Foo;
};
)FIDL";

  auto v1 = R"FIDL(
@available(added=1, removed=2)
library example;

type Bar = struct {};
)FIDL";

  auto v2 = R"FIDL(
@available(added=2, removed=3)
library example;

type Foo = struct {
    bar Bar;
};
type Bar = struct {};
)FIDL";

  auto v3 = R"FIDL(
@available(added=3, removed=4)
library example;

type Bar = struct {};
)FIDL";

  auto v4_to_5 = R"FIDL(
@available(added=4, removed=6)
library example;

type Foo = struct {};
type Bar = struct {
    foo Foo;
};
)FIDL";

  auto v6 = R"FIDL(
@available(added=6, removed=7)
library example;

type Foo = struct {};
)FIDL";

  auto v7_onward = R"FIDL(
@available(added=7)
library example;
)FIDL";

  ASSERT_EQUIVALENT(fidl, v1, "1");
  ASSERT_EQUIVALENT(fidl, v2, "2");
  ASSERT_EQUIVALENT(fidl, v3, "3");
  ASSERT_EQUIVALENT(fidl, v4_to_5, "4");
  ASSERT_EQUIVALENT(fidl, v4_to_5, "5");
  ASSERT_EQUIVALENT(fidl, v6, "6");
  ASSERT_EQUIVALENT(fidl, v7_onward, "7");
  ASSERT_EQUIVALENT(fidl, v7_onward, "HEAD");
  ASSERT_EQUIVALENT(fidl, v4_to_5, "4,5,6,7,HEAD");
}

TEST(VersioningDecompositionTests, ConstsAndConstraints) {
  auto fidl = R"FIDL(
@available(added=1)
library example;

@available(removed=4)
const LEN uint64 = 10;

type Foo = table {
    @available(replaced=3)
    1: bar Bar;
    @available(added=3, replaced=4)
    1: bar string:LEN;
    @available(added=4, removed=5)
    1: bar Bar;
};

@available(replaced=2)
type Bar = struct {};
@available(added=2)
type Bar = table {};
)FIDL";

  auto v1 = R"FIDL(
@available(added=1, removed=2)
library example;

const LEN uint64 = 10;
type Foo = table {
    1: bar Bar;
};
type Bar = struct {};
)FIDL";

  auto v2 = R"FIDL(
@available(added=2, removed=3)
library example;

const LEN uint64 = 10;
type Foo = table {
    1: bar Bar;
};
type Bar = table {};
)FIDL";

  auto v3 = R"FIDL(
@available(added=3, removed=4)
library example;

const LEN uint64 = 10;
type Foo = table {
    1: bar string:LEN;
};
type Bar = table {};
)FIDL";

  auto v4 = R"FIDL(
@available(added=4, removed=5)
library example;

type Foo = table {
    1: bar Bar;
};
type Bar = table {};
)FIDL";

  auto v5_onward = R"FIDL(
@available(added=5)
library example;

type Foo = table {};
type Bar = table {};
)FIDL";

  auto all_versions = R"FIDL(
library example;

const LEN uint64 = 10;

type Foo = table {
    1: bar Bar;
};

type Bar = table {};
)FIDL";

  ASSERT_EQUIVALENT(fidl, v1, "1");
  ASSERT_EQUIVALENT(fidl, v2, "2");
  ASSERT_EQUIVALENT(fidl, v3, "3");
  ASSERT_EQUIVALENT(fidl, v4, "4");
  ASSERT_EQUIVALENT(fidl, v5_onward, "5");
  ASSERT_EQUIVALENT(fidl, v5_onward, "HEAD");
  ASSERT_EQUIVALENT(fidl, all_versions, "1,2,3,4,5,HEAD");
}

TEST(VersioningDecompositionTests, AllElementsSplitByMembership) {
  auto fidl = R"FIDL(
@available(added=1)
library example;

@available(added=2, removed=5)
type Bits = bits {
    FIRST = 1;
    @available(added=3, removed=4)
    SECOND = 2;
};

@available(added=2, removed=5)
type Enum = enum {
    FIRST = 1;
    @available(added=3, removed=4)
    SECOND = 2;
};

@available(added=2, removed=5)
type Struct = struct {
    first string;
    @available(added=3, removed=4)
    second string;
};

@available(added=2, removed=5)
type Table = table {
    1: first string;
    @available(added=3, removed=4)
    2: second string;
};

@available(added=2, removed=5)
type Union = union {
    1: first string;
    @available(added=3, removed=4)
    2: second string;
};

@available(added=2, removed=5)
protocol TargetProtocol {};

@available(added=2, removed=5)
protocol ProtocolComposition {
    @available(added=3, removed=4)
    compose TargetProtocol;
};

@available(added=2, removed=5)
protocol ProtocolMethods {
    @available(added=3, removed=4)
    Method() -> ();
};

@available(added=2, removed=5)
service Service {
    first client_end:TargetProtocol;
    @available(added=3, removed=4)
    second client_end:TargetProtocol;
};

@available(added=2, removed=5)
resource_definition Resource : uint32 {
    properties {
        first uint32;
        @available(added=3, removed=4)
        second uint32;
        // This property is required for compilation, but is not otherwise under test.
        subtype flexible enum : uint32 {};
    };
};
)FIDL";

  auto v1 = R"FIDL(
@available(added=1, removed=2)
library example;
)FIDL";

  auto v2 = R"FIDL(
@available(added=2, removed=3)
library example;

type Bits = bits {
    FIRST = 1;
};

type Enum = enum {
    FIRST = 1;
};

type Struct = struct {
    first string;
};

type Table = table {
    1: first string;
};

type Union = union {
    1: first string;
};

protocol TargetProtocol {};

protocol ProtocolComposition {};

protocol ProtocolMethods {};

service Service {
    first client_end:TargetProtocol;
};

resource_definition Resource : uint32 {
    properties {
        first uint32;
        // This property is required for compilation, but is not otherwise under test.
        subtype flexible enum : uint32 {};
    };
};
)FIDL";

  auto v3 = R"FIDL(
@available(added=3, removed=4)
library example;

type Bits = bits {
    FIRST = 1;
    SECOND = 2;
};

type Enum = enum {
    FIRST = 1;
    SECOND = 2;
};

type Struct = struct {
    first string;
    second string;
};

type Table = table {
    1: first string;
    2: second string;
};

type Union = union {
    1: first string;
    2: second string;
};

protocol TargetProtocol {};

protocol ProtocolComposition {
    compose TargetProtocol;
};

protocol ProtocolMethods {
    Method() -> ();
};

service Service {
    first client_end:TargetProtocol;
    second client_end:TargetProtocol;
};

resource_definition Resource : uint32 {
    properties {
        first uint32;
        second uint32;
        // This property is required for compilation, but is not otherwise under test.
        subtype flexible enum : uint32 {};
    };
};
)FIDL";

  auto v4 = R"FIDL(
@available(added=4, removed=5)
library example;

type Bits = bits {
    FIRST = 1;
};

type Enum = enum {
    FIRST = 1;
};

type Struct = struct {
    first string;
};


type Table = table {
    1: first string;
};

type Union = union {
    1: first string;
};

protocol TargetProtocol {};

protocol ProtocolComposition {};

protocol ProtocolMethods {};

service Service {
    first client_end:TargetProtocol;
};

resource_definition Resource : uint32 {
    properties {
        first uint32;
        // This property is required for compilation, but is not otherwise under test.
        subtype flexible enum : uint32 {};
    };
};
)FIDL";

  auto v5_onward = R"FIDL(
@available(added=5)
library example;
)FIDL";

  ASSERT_EQUIVALENT(fidl, v1, "1");
  ASSERT_EQUIVALENT(fidl, v2, "2");
  ASSERT_EQUIVALENT(fidl, v3, "3");
  ASSERT_EQUIVALENT(fidl, v4, "4");
  ASSERT_EQUIVALENT(fidl, v5_onward, "5");
  ASSERT_EQUIVALENT(fidl, v5_onward, "HEAD");
  ASSERT_EQUIVALENT(fidl, v3, "1,2,3,4,5,HEAD");
}

TEST(VersioningDecompositionTests, AllElementsSplitByReference) {
  auto fidl_prefix = R"FIDL(
@available(added=1)
library example;

@available(replaced=2)
const VALUE uint32 = 1;
@available(added=2)
const VALUE uint32 = 2;

@available(replaced=2)
type Type = struct {
    value bool;
};
@available(added=2)
type Type = table {
    1: value bool;
};

// Need unsigned integers for bits underlying type.
@available(replaced=2)
alias IntegerType = uint32;
@available(added=2)
alias IntegerType = uint64;

// Need uint32/int32 for error type.
@available(replaced=2)
alias ErrorIntegerType = uint32;
@available(added=2)
alias ErrorIntegerType = int32;

@available(replaced=2)
protocol TargetProtocol {};
@available(added=2)
protocol TargetProtocol {
    Method();
};
)FIDL";

  auto v1_prefix = R"FIDL(
@available(added=1, removed=2)
library example;

const VALUE uint32 = 1;

type Type = struct {
    value bool;
};

alias IntegerType = uint32;

alias ErrorIntegerType = uint32;

protocol TargetProtocol {};
)FIDL";

  auto v2_onward_prefix = R"FIDL(
@available(added=2)
library example;

const VALUE uint32 = 2;

type Type = table {
    1: value bool;
};

alias IntegerType = uint64;

alias ErrorIntegerType = int32;

protocol TargetProtocol { Method(); };
)FIDL";

  auto common_suffix = R"FIDL(
const CONST uint32 = VALUE;

alias Alias = Type;

// TODO(https://fxbug.dev/42158155): Uncomment.
// type Newtype = Type;

type BitsUnderlying = bits : IntegerType {
    MEMBER = 1;
};

type BitsMemberValue = bits {
    MEMBER = VALUE;
};

type EnumUnderlying = enum : IntegerType {
    MEMBER = 1;
};

type EnumMemberValue = enum {
    MEMBER = VALUE;
};

type StructMemberType = struct {
    member Type;
};

type StructMemberDefault = struct {
    @allow_deprecated_struct_defaults
    member uint32 = VALUE;
};

type Table = table {
    1: member Type;
};

type Union = union {
    1: member Type;
};

protocol ProtocolComposition {
    compose TargetProtocol;
};

protocol ProtocolMethodRequest {
    Method(Type);
};

protocol ProtocolMethodResponse {
    Method() -> (Type);
};

protocol ProtocolEvent {
    -> Event(Type);
};

protocol ProtocolSuccess {
    Method() -> (Type) error uint32;
};

protocol ProtocolError {
    Method() -> () error ErrorIntegerType;
};

service Service {
    member client_end:TargetProtocol;
};

resource_definition Resource : uint32 {
    properties {
        first IntegerType;
        // This property is required for compilation, but is not otherwise under test.
        subtype flexible enum : uint32 {};
    };
};

type NestedTypes = struct {
    first vector<Type>;
    second vector<array<Type, 3>>;
};

type LayoutParameters = struct {
    member array<bool, VALUE>;
};

type Constraints = struct {
    member vector<bool>:VALUE;
};

type AnonymousLayouts = struct {
    first_member table {
        1: second_member union {
            1: third_member Type;
        };
    };
};

protocol AnonymousLayoutsInProtocol {
    Request(struct { member Type; });
    Response() -> (struct { member Type; });
    -> Event(struct { member Type; });
    Success() -> (struct { member Type; }) error uint32;
    Error() -> () error ErrorIntegerType;
};
)FIDL";

  auto fidl = std::string(fidl_prefix) + common_suffix;
  auto v1 = std::string(v1_prefix) + common_suffix;
  auto v2_onward = std::string(v2_onward_prefix) + common_suffix;

  ASSERT_EQUIVALENT(fidl, v1, "1");
  ASSERT_EQUIVALENT(fidl, v2_onward, "2");
  ASSERT_EQUIVALENT(fidl, v2_onward, "HEAD");
  ASSERT_EQUIVALENT(fidl, v2_onward, "1,2,HEAD");
}

TEST(VersioningDecompositionTests, Complicated) {
  auto fidl = R"FIDL(
@available(added=1)
library example;

type X = resource table {
    @available(removed=7)
    1: x1 bool;
    @available(added=3)
    2: x2 Y;
    @available(added=4)
    3: x3 Z;
};

@available(added=3)
type Y = resource union {
    1: y1 client_end:A;
    @available(added=4, removed=5)
    2: y2 client_end:B;
};

@available(added=3)
type Z = resource struct {
    z1 Y:optional;
    z2 vector<W>:optional;
};

@available(added=3)
type W = resource table {
    1: w1 X;
};

protocol A {
    A1(X);
    @available(added=7)
    A2(resource struct { y Y; });
};

@available(added=3)
protocol B {
    @available(removed=5)
    B1(X);
    @available(added=5)
    B2(resource struct {
      x X;
      y Y;
    });
};

@available(removed=6)
protocol AB {
    compose A;
    @available(added=4)
    compose B;
};
)FIDL";

  auto v1_to_2 = R"FIDL(
@available(added=1, removed=3)
library example;

type X = resource table {
    1: x1 bool;
};

protocol A {
    A1(X);
};

protocol AB {
    compose A;
};
)FIDL";

  auto v3 = R"FIDL(
@available(added=3, removed=4)
library example;

type X = resource table {
    1: x1 bool;
    2: x2 Y;
};

type Y = resource union {
    1: y1 client_end:A;
};

type Z = resource struct {
    z1 Y:optional;
    z2 vector<W>:optional;
};

type W = resource table {
    1: w1 X;
};

protocol A {
    A1(X);
};

protocol B {
    B1(X);
};

protocol AB {
    compose A;
};
)FIDL";

  auto v4 = R"FIDL(
@available(added=4, removed=5)
library example;

type X = resource table {
    1: x1 bool;
    2: x2 Y;
    3: x3 Z;
};

type Y = resource union {
    1: y1 client_end:A;
    2: y2 client_end:B;
};

type Z = resource struct {
    z1 Y:optional;
    z2 vector<W>:optional;
};

type W = resource table {
    1: w1 X;
};

protocol A {
    A1(X);
};

protocol B {
    B1(X);
};

protocol AB {
    compose A;
    compose B;
};
)FIDL";

  auto v5 = R"FIDL(
@available(added=5, removed=6)
library example;

type X = resource table {
    1: x1 bool;
    2: x2 Y;
    3: x3 Z;
};

type Y = resource union {
    1: y1 client_end:A;
};

type Z = resource struct {
    z1 Y:optional;
    z2 vector<W>:optional;
};

type W = resource table {
    1: w1 X;
};

protocol A {
    A1(X);
};

protocol B {
    B2(resource struct {
      x X;
      y Y;
    });
};

protocol AB {
    compose A;
    compose B;
};
)FIDL";

  auto v6 = R"FIDL(
@available(added=6, removed=7)
library example;

type X = resource table {
    1: x1 bool;
    2: x2 Y;
    3: x3 Z;
};

type Y = resource union {
    1: y1 client_end:A;
};

type Z = resource struct {
    z1 Y:optional;
    z2 vector<W>:optional;
};

type W = resource table {
    1: w1 X;
};

protocol A {
    A1(X);
};

protocol B {
    B2(resource struct {
      x X;
      y Y;
    });
};
)FIDL";

  auto v7_onward = R"FIDL(
@available(added=7)
library example;

type X = resource table {
    2: x2 Y;
    3: x3 Z;
};

type Y = resource union {
    1: y1 client_end:A;
};

type Z = resource struct {
    z1 Y:optional;
    z2 vector<W>:optional;
};

type W = resource table {
    1: w1 X;
};

protocol A {
    A1(X);
    A2(resource struct { y Y; });
};

protocol B {
    B2(resource struct {
      x X;
      y Y;
    });
};
)FIDL";

  auto all_versions = R"FIDL(
library example;

type X = resource table {
    1: x1 bool;
    2: x2 Y;
    3: x3 Z;
};

type Y = resource union {
    1: y1 client_end:A;
    2: y2 client_end:B;
};

type Z = resource struct {
    z1 Y:optional;
    z2 vector<W>:optional;
};

type W = resource table {
    1: w1 X;
};

protocol A {
    A1(X);
    A2(resource struct { y Y; });
};

protocol B {
    B1(X);
    B2(resource struct {
      x X;
      y Y;
    });
};

protocol AB {
    compose A;
    compose B;
};
)FIDL";

  ASSERT_EQUIVALENT(fidl, v1_to_2, "1");
  ASSERT_EQUIVALENT(fidl, v1_to_2, "2");
  ASSERT_EQUIVALENT(fidl, v3, "3");
  ASSERT_EQUIVALENT(fidl, v4, "4");
  ASSERT_EQUIVALENT(fidl, v5, "5");
  ASSERT_EQUIVALENT(fidl, v6, "6");
  ASSERT_EQUIVALENT(fidl, v7_onward, "7");
  ASSERT_EQUIVALENT(fidl, v7_onward, "HEAD");
  ASSERT_EQUIVALENT(fidl, all_versions, "1,2,3,4,5,6,7,HEAD");
}

TEST(VersioningDecompositionTests, ConvertNamedToAnonymous) {
  auto fidl = R"FIDL(
@available(added=1)
library example;

@available(replaced=2)
type Foo = struct {
    member Bar;
};

@available(replaced=2)
type Bar = struct {};

@available(added=2)
type Foo = struct {
    member @generated_name("Bar") struct {};
};
)FIDL";

  auto v1 = R"FIDL(
@available(added=1, removed=2)
library example;

type Foo = struct {
    member Bar;
};

type Bar = struct {};
)FIDL";

  auto v2_onward = R"FIDL(
@available(added=2)
library example;

type Foo = struct {
    member @generated_name("Bar") struct {};
};
)FIDL";

  ASSERT_EQUIVALENT(fidl, v1, "1");
  ASSERT_EQUIVALENT(fidl, v2_onward, "2");
  ASSERT_EQUIVALENT(fidl, v2_onward, "HEAD");
  ASSERT_EQUIVALENT(fidl, v2_onward, "1,2,HEAD");
}

TEST(VersioningDecompositionTests, ConvertAnonymousToNamed) {
  auto fidl = R"FIDL(
@available(added=1)
library example;

@available(replaced=2)
type Foo = struct {
    member @generated_name("Bar") struct {};
};

@available(added=2)
type Foo = struct {
    member Bar;
};

@available(added=2)
type Bar = struct {};
)FIDL";

  auto v1 = R"FIDL(
@available(added=1, removed=2)
library example;

type Foo = struct {
    member @generated_name("Bar") struct {};
};
)FIDL";

  auto v2_onward = R"FIDL(
@available(added=2)
library example;

type Foo = struct {
    member Bar;
};

type Bar = struct {};
)FIDL";

  ASSERT_EQUIVALENT(fidl, v1, "1");
  ASSERT_EQUIVALENT(fidl, v2_onward, "2");
  ASSERT_EQUIVALENT(fidl, v2_onward, "HEAD");
  ASSERT_EQUIVALENT(fidl, v2_onward, "1,2,HEAD");
}

}  // namespace
}  // namespace fidlc
