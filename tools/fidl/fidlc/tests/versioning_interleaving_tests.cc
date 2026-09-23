// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/tests/test_library.h"

// This file tests ways of interleaving the availability of a source element
// with that of a target element that it references.

namespace fidlc {
namespace {

using ::testing::Contains;

struct TestCase {
  // A code describing how to order the availabilities relative to each other,
  // using (a, d, r) for the source and (A, D, R) for the target:
  //
  //     source: @available(added=a, deprecated=d, removed=r)
  //     target: @available(added=A, deprecated=D, removed=R)
  //
  // For example, "AadrR" means: add target, add source, deprecate source,
  // remove source, remove target. Additionally, the character "=" is used to
  // align two values. For example, "a=A" means the source and target are added
  // at the same version, and never deprecated/removed.
  //
  // Must contain at least "a" and "A", but all others are optional.
  std::string_view code;

  // Expected errors. The order does not matter, and the list does not need to
  // be complete, because this file contains a large number of test cases and
  // stricter requirements would make it painful to update when errors change.
  std::vector<const DiagnosticDef*> errors = {};

  struct Attributes {
    std::string source_available;
    std::string target_available;
  };

  // Generates the @available attributes for source and target.
  Attributes Format() const {
    std::stringstream source, target;
    source << "@available(";
    target << "@available(";
    int version = 1;
    for (auto c : code) {
      switch (c) {
        case 'a':
          source << "added=" << version;
          break;
        case 'd':
          source << ", deprecated=" << version;
          break;
        case 'r':
          source << ", removed=" << version;
          break;
        case 'A':
          target << "added=" << version;
          break;
        case 'D':
          target << ", deprecated=" << version;
          break;
        case 'R':
          target << ", removed=" << version;
          break;
        case '=':
          version -= 2;
          break;
      }
      ++version;
    }
    source << ')';
    target << ')';
    return Attributes{source.str(), target.str()};
  }

  // Compiles the library and asserts that it conforms to the test case.
  void CompileAndAssert(TestLibrary& library) const {
    if (errors.empty()) {
      ASSERT_COMPILED(library);
      return;
    }
    ASSERT_FALSE(library.Compile());
    std::set<std::string_view> actual_errors;
    for (auto& actual_error : library.errors()) {
      actual_errors.insert(actual_error->def.msg);
    }
    for (auto expected_error : errors) {
      EXPECT_THAT(actual_errors, Contains(expected_error->msg));
    }
  }
};

// These cases (except for some extras at the bottom) were generated with the
// following Python code:
//
//     def go(x, y):
//         if x is None or y is None:
//             return set()
//         if not (x or y):
//             return {""}
//         rest = lambda x: x[1:] if x else None
//         rx, ry, rxy = go(rest(x), y), go(x, rest(y)), go(rest(x), rest(y))
//         return {*rx, *ry, *rxy, *(x[0] + s for s in rx), *(y[0] + s for s in ry),
//                 *(f"{x[0]}={y[0]}{s}" for s in rxy)}
//
//     print("\n".join(sorted(s for s in go("adr", "ADR") if "a" in s and "A" in s)))
//
const TestCase kTestCases[] = {
    {"ADRa", {&ErrNameNotFoundInVersionRange}},
    {"ADRad", {&ErrNameNotFoundInVersionRange}},
    {"ADRadr", {&ErrNameNotFoundInVersionRange}},
    {"ADRar", {&ErrNameNotFoundInVersionRange}},
    {"ADa", {&ErrInvalidReferenceToDeprecated}},
    {"ADa=R", {&ErrNameNotFoundInVersionRange}},
    {"ADa=Rd", {&ErrNameNotFoundInVersionRange}},
    {"ADa=Rdr", {&ErrNameNotFoundInVersionRange}},
    {"ADa=Rr", {&ErrNameNotFoundInVersionRange}},
    {"ADaR", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"ADaRd", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"ADaRdr", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"ADaRr", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"ADad", {&ErrInvalidReferenceToDeprecated}},
    {"ADad=R", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"ADad=Rr", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"ADadR", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"ADadRr", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"ADadr", {&ErrInvalidReferenceToDeprecated}},
    {"ADadr=R", {&ErrInvalidReferenceToDeprecated}},
    {"ADadrR", {&ErrInvalidReferenceToDeprecated}},
    {"ADar", {&ErrInvalidReferenceToDeprecated}},
    {"ADar=R", {&ErrInvalidReferenceToDeprecated}},
    {"ADarR", {&ErrInvalidReferenceToDeprecated}},
    {"ARa", {&ErrNameNotFoundInVersionRange}},
    {"ARad", {&ErrNameNotFoundInVersionRange}},
    {"ARadr", {&ErrNameNotFoundInVersionRange}},
    {"ARar", {&ErrNameNotFoundInVersionRange}},
    {"Aa"},
    {"Aa=D", {&ErrInvalidReferenceToDeprecated}},
    {"Aa=DR", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"Aa=DRd", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"Aa=DRdr", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"Aa=DRr", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"Aa=Dd", {&ErrInvalidReferenceToDeprecated}},
    {"Aa=Dd=R", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"Aa=Dd=Rr", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"Aa=DdR", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"Aa=DdRr", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"Aa=Ddr", {&ErrInvalidReferenceToDeprecated}},
    {"Aa=Ddr=R", {&ErrInvalidReferenceToDeprecated}},
    {"Aa=DdrR", {&ErrInvalidReferenceToDeprecated}},
    {"Aa=Dr", {&ErrInvalidReferenceToDeprecated}},
    {"Aa=Dr=R", {&ErrInvalidReferenceToDeprecated}},
    {"Aa=DrR", {&ErrInvalidReferenceToDeprecated}},
    {"Aa=R", {&ErrNameNotFoundInVersionRange}},
    {"Aa=Rd", {&ErrNameNotFoundInVersionRange}},
    {"Aa=Rdr", {&ErrNameNotFoundInVersionRange}},
    {"Aa=Rr", {&ErrNameNotFoundInVersionRange}},
    {"AaD", {&ErrInvalidReferenceToDeprecated}},
    {"AaDR", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"AaDRd", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"AaDRdr", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"AaDRr", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"AaDd", {&ErrInvalidReferenceToDeprecated}},
    {"AaDd=R", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"AaDd=Rr", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"AaDdR", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"AaDdRr", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"AaDdr", {&ErrInvalidReferenceToDeprecated}},
    {"AaDdr=R", {&ErrInvalidReferenceToDeprecated}},
    {"AaDdrR", {&ErrInvalidReferenceToDeprecated}},
    {"AaDr", {&ErrInvalidReferenceToDeprecated}},
    {"AaDr=R", {&ErrInvalidReferenceToDeprecated}},
    {"AaDrR", {&ErrInvalidReferenceToDeprecated}},
    {"AaR", {&ErrNameNotFoundInVersionRange}},
    {"AaRd", {&ErrNameNotFoundInVersionRange}},
    {"AaRdr", {&ErrNameNotFoundInVersionRange}},
    {"AaRr", {&ErrNameNotFoundInVersionRange}},
    {"Aad"},
    {"Aad=D"},
    {"Aad=DR", {&ErrNameNotFoundInVersionRange}},
    {"Aad=DRr", {&ErrNameNotFoundInVersionRange}},
    {"Aad=Dr"},
    {"Aad=Dr=R"},
    {"Aad=DrR"},
    {"Aad=R", {&ErrNameNotFoundInVersionRange}},
    {"Aad=Rr", {&ErrNameNotFoundInVersionRange}},
    {"AadD"},
    {"AadDR", {&ErrNameNotFoundInVersionRange}},
    {"AadDRr", {&ErrNameNotFoundInVersionRange}},
    {"AadDr"},
    {"AadDr=R"},
    {"AadDrR"},
    {"AadR", {&ErrNameNotFoundInVersionRange}},
    {"AadRr", {&ErrNameNotFoundInVersionRange}},
    {"Aadr"},
    {"Aadr=D"},
    {"Aadr=DR"},
    {"Aadr=R"},
    {"AadrD"},
    {"AadrDR"},
    {"AadrR"},
    {"Aar"},
    {"Aar=D"},
    {"Aar=DR"},
    {"Aar=R"},
    {"AarD"},
    {"AarDR"},
    {"AarR"},
    {"a=A"},
    {"a=AD", {&ErrInvalidReferenceToDeprecated}},
    {"a=ADR", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"a=ADRd", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"a=ADRdr", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"a=ADRr", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"a=ADd", {&ErrInvalidReferenceToDeprecated}},
    {"a=ADd=R", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"a=ADd=Rr", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"a=ADdR", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"a=ADdRr", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"a=ADdr", {&ErrInvalidReferenceToDeprecated}},
    {"a=ADdr=R", {&ErrInvalidReferenceToDeprecated}},
    {"a=ADdrR", {&ErrInvalidReferenceToDeprecated}},
    {"a=ADr", {&ErrInvalidReferenceToDeprecated}},
    {"a=ADr=R", {&ErrInvalidReferenceToDeprecated}},
    {"a=ADrR", {&ErrInvalidReferenceToDeprecated}},
    {"a=AR", {&ErrNameNotFoundInVersionRange}},
    {"a=ARd", {&ErrNameNotFoundInVersionRange}},
    {"a=ARdr", {&ErrNameNotFoundInVersionRange}},
    {"a=ARr", {&ErrNameNotFoundInVersionRange}},
    {"a=Ad"},
    {"a=Ad=D"},
    {"a=Ad=DR", {&ErrNameNotFoundInVersionRange}},
    {"a=Ad=DRr", {&ErrNameNotFoundInVersionRange}},
    {"a=Ad=Dr"},
    {"a=Ad=Dr=R"},
    {"a=Ad=DrR"},
    {"a=Ad=R", {&ErrNameNotFoundInVersionRange}},
    {"a=Ad=Rr", {&ErrNameNotFoundInVersionRange}},
    {"a=AdD"},
    {"a=AdDR", {&ErrNameNotFoundInVersionRange}},
    {"a=AdDRr", {&ErrNameNotFoundInVersionRange}},
    {"a=AdDr"},
    {"a=AdDr=R"},
    {"a=AdDrR"},
    {"a=AdR", {&ErrNameNotFoundInVersionRange}},
    {"a=AdRr", {&ErrNameNotFoundInVersionRange}},
    {"a=Adr"},
    {"a=Adr=D"},
    {"a=Adr=DR"},
    {"a=Adr=R"},
    {"a=AdrD"},
    {"a=AdrDR"},
    {"a=AdrR"},
    {"a=Ar"},
    {"a=Ar=D"},
    {"a=Ar=DR"},
    {"a=Ar=R"},
    {"a=ArD"},
    {"a=ArDR"},
    {"a=ArR"},
    {"aA", {&ErrNameNotFoundInVersionRange}},
    {"aAD", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"aADR", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"aADRd", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"aADRdr", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"aADRr", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"aADd", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"aADd=R", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"aADd=Rr", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"aADdR", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"aADdRr", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"aADdr", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"aADdr=R", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"aADdrR", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"aADr", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"aADr=R", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"aADrR", {&ErrInvalidReferenceToDeprecated, &ErrNameNotFoundInVersionRange}},
    {"aAR", {&ErrNameNotFoundInVersionRange}},
    {"aARd", {&ErrNameNotFoundInVersionRange}},
    {"aARdr", {&ErrNameNotFoundInVersionRange}},
    {"aARr", {&ErrNameNotFoundInVersionRange}},
    {"aAd", {&ErrNameNotFoundInVersionRange}},
    {"aAd=D", {&ErrNameNotFoundInVersionRange}},
    {"aAd=DR", {&ErrNameNotFoundInVersionRange}},
    {"aAd=DRr", {&ErrNameNotFoundInVersionRange}},
    {"aAd=Dr", {&ErrNameNotFoundInVersionRange}},
    {"aAd=Dr=R", {&ErrNameNotFoundInVersionRange}},
    {"aAd=DrR", {&ErrNameNotFoundInVersionRange}},
    {"aAd=R", {&ErrNameNotFoundInVersionRange}},
    {"aAd=Rr", {&ErrNameNotFoundInVersionRange}},
    {"aAdD", {&ErrNameNotFoundInVersionRange}},
    {"aAdDR", {&ErrNameNotFoundInVersionRange}},
    {"aAdDRr", {&ErrNameNotFoundInVersionRange}},
    {"aAdDr", {&ErrNameNotFoundInVersionRange}},
    {"aAdDr=R", {&ErrNameNotFoundInVersionRange}},
    {"aAdDrR", {&ErrNameNotFoundInVersionRange}},
    {"aAdR", {&ErrNameNotFoundInVersionRange}},
    {"aAdRr", {&ErrNameNotFoundInVersionRange}},
    {"aAdr", {&ErrNameNotFoundInVersionRange}},
    {"aAdr=D", {&ErrNameNotFoundInVersionRange}},
    {"aAdr=DR", {&ErrNameNotFoundInVersionRange}},
    {"aAdr=R", {&ErrNameNotFoundInVersionRange}},
    {"aAdrD", {&ErrNameNotFoundInVersionRange}},
    {"aAdrDR", {&ErrNameNotFoundInVersionRange}},
    {"aAdrR", {&ErrNameNotFoundInVersionRange}},
    {"aAr", {&ErrNameNotFoundInVersionRange}},
    {"aAr=D", {&ErrNameNotFoundInVersionRange}},
    {"aAr=DR", {&ErrNameNotFoundInVersionRange}},
    {"aAr=R", {&ErrNameNotFoundInVersionRange}},
    {"aArD", {&ErrNameNotFoundInVersionRange}},
    {"aArDR", {&ErrNameNotFoundInVersionRange}},
    {"aArR", {&ErrNameNotFoundInVersionRange}},
    {"ad=A", {&ErrNameNotFoundInVersionRange}},
    {"ad=AD", {&ErrNameNotFoundInVersionRange}},
    {"ad=ADR", {&ErrNameNotFoundInVersionRange}},
    {"ad=ADRr", {&ErrNameNotFoundInVersionRange}},
    {"ad=ADr", {&ErrNameNotFoundInVersionRange}},
    {"ad=ADr=R", {&ErrNameNotFoundInVersionRange}},
    {"ad=ADrR", {&ErrNameNotFoundInVersionRange}},
    {"ad=AR", {&ErrNameNotFoundInVersionRange}},
    {"ad=ARr", {&ErrNameNotFoundInVersionRange}},
    {"ad=Ar", {&ErrNameNotFoundInVersionRange}},
    {"ad=Ar=D", {&ErrNameNotFoundInVersionRange}},
    {"ad=Ar=DR", {&ErrNameNotFoundInVersionRange}},
    {"ad=Ar=R", {&ErrNameNotFoundInVersionRange}},
    {"ad=ArD", {&ErrNameNotFoundInVersionRange}},
    {"ad=ArDR", {&ErrNameNotFoundInVersionRange}},
    {"ad=ArR", {&ErrNameNotFoundInVersionRange}},
    {"adA", {&ErrNameNotFoundInVersionRange}},
    {"adAD", {&ErrNameNotFoundInVersionRange}},
    {"adADR", {&ErrNameNotFoundInVersionRange}},
    {"adADRr", {&ErrNameNotFoundInVersionRange}},
    {"adADr", {&ErrNameNotFoundInVersionRange}},
    {"adADr=R", {&ErrNameNotFoundInVersionRange}},
    {"adADrR", {&ErrNameNotFoundInVersionRange}},
    {"adAR", {&ErrNameNotFoundInVersionRange}},
    {"adARr", {&ErrNameNotFoundInVersionRange}},
    {"adAr", {&ErrNameNotFoundInVersionRange}},
    {"adAr=D", {&ErrNameNotFoundInVersionRange}},
    {"adAr=DR", {&ErrNameNotFoundInVersionRange}},
    {"adAr=R", {&ErrNameNotFoundInVersionRange}},
    {"adArD", {&ErrNameNotFoundInVersionRange}},
    {"adArDR", {&ErrNameNotFoundInVersionRange}},
    {"adArR", {&ErrNameNotFoundInVersionRange}},
    {"adr=A", {&ErrNameNotFoundInVersionRange}},
    {"adr=AD", {&ErrNameNotFoundInVersionRange}},
    {"adr=ADR", {&ErrNameNotFoundInVersionRange}},
    {"adr=AR", {&ErrNameNotFoundInVersionRange}},
    {"adrA", {&ErrNameNotFoundInVersionRange}},
    {"adrAD", {&ErrNameNotFoundInVersionRange}},
    {"adrADR", {&ErrNameNotFoundInVersionRange}},
    {"adrAR", {&ErrNameNotFoundInVersionRange}},
    {"ar=A", {&ErrNameNotFoundInVersionRange}},
    {"ar=AD", {&ErrNameNotFoundInVersionRange}},
    {"ar=ADR", {&ErrNameNotFoundInVersionRange}},
    {"ar=AR", {&ErrNameNotFoundInVersionRange}},
    {"arA", {&ErrNameNotFoundInVersionRange}},
    {"arAD", {&ErrNameNotFoundInVersionRange}},
    {"arADR", {&ErrNameNotFoundInVersionRange}},
    {"arAR", {&ErrNameNotFoundInVersionRange}},
};

// Substitutes replacement for placeholder in str.
void substitute(std::string& str, std::string_view placeholder, std::string_view replacement) {
  str.replace(str.find(placeholder), placeholder.size(), replacement);
}

TEST(VersioningInterleavingTests, SameLibrary) {
  for (auto& test_case : kTestCases) {
    auto attributes = test_case.Format();
    std::string fidl = R"FIDL(
@available(added=1)
library example;

${source_available}
const SOURCE bool = TARGET;

${target_available}
const TARGET bool = false;
)FIDL";
    substitute(fidl, "${source_available}", attributes.source_available);
    substitute(fidl, "${target_available}", attributes.target_available);
    SCOPED_TRACE(testing::Message() << "code: " << test_case.code << ", fidl:\n\n" << fidl);
    TestLibrary library(fidl);
    library.SelectVersion("example", "HEAD");
    test_case.CompileAndAssert(library);
  }
}

// Tests compilation of example_fidl and dependency_fidl after substituting
// ${source_available} in example_fidl and ${target_available} in
// dependency_fidl using the values from test_case.
void TestExternalLibrary(const TestCase& test_case, std::string example_fidl,
                         std::string dependency_fidl) {
  SharedAmongstLibraries shared;
  shared.SelectVersion("platform", "HEAD");
  auto attributes = test_case.Format();
  substitute(dependency_fidl, "${target_available}", attributes.target_available);
  substitute(example_fidl, "${source_available}", attributes.source_available);
  SCOPED_TRACE(testing::Message() << "code: " << test_case.code << ", dependency.fidl:\n\n"
                                  << dependency_fidl << "\n\nexample.fidl:\n\n"
                                  << example_fidl);
  TestLibrary dependency(&shared, "dependency.fidl", dependency_fidl);
  ASSERT_COMPILED(dependency);
  TestLibrary example(&shared, "example.fidl", example_fidl);
  test_case.CompileAndAssert(example);
}

TEST(VersioningInterleavingTests, DeclToDeclExternal) {
  std::string example_fidl = R"FIDL(
@available(added=1)
library platform.example;

using platform.dependency;

${source_available}
const SOURCE bool = platform.dependency.TARGET;
)FIDL";
  std::string dependency_fidl = R"FIDL(
@available(added=1)
library platform.dependency;

${target_available}
const TARGET bool = false;
)FIDL";
  for (auto& test_case : kTestCases) {
    TestExternalLibrary(test_case, example_fidl, dependency_fidl);
  }
}

TEST(VersioningInterleavingTests, LibraryToLibraryExternal) {
  std::string example_fidl = R"FIDL(
${source_available}
library platform.example;

using platform.dependency;

const SOURCE bool = platform.dependency.TARGET;
)FIDL";
  std::string dependency_fidl = R"FIDL(
${target_available}
library platform.dependency;

const TARGET bool = false;
)FIDL";
  for (auto& test_case : kTestCases) {
    TestExternalLibrary(test_case, example_fidl, dependency_fidl);
  }
}

TEST(VersioningInterleavingTests, LibraryToDeclExternal) {
  std::string example_fidl = R"FIDL(
${source_available}
library platform.example;

using platform.dependency;

const SOURCE bool = platform.dependency.TARGET;
)FIDL";
  std::string dependency_fidl = R"FIDL(
@available(added=1)
library platform.dependency;

${target_available}
const TARGET bool = false;
)FIDL";
  for (auto& test_case : kTestCases) {
    TestExternalLibrary(test_case, example_fidl, dependency_fidl);
  }
}

TEST(VersioningInterleavingTests, DeclToLibraryExternal) {
  std::string example_fidl = R"FIDL(
@available(added=1)
library platform.example;

using platform.dependency;

${source_available}
const SOURCE bool = platform.dependency.TARGET;
)FIDL";
  std::string dependency_fidl = R"FIDL(
${target_available}
library platform.dependency;

const TARGET bool = false;
)FIDL";
  for (auto& test_case : kTestCases) {
    TestExternalLibrary(test_case, example_fidl, dependency_fidl);
  }
}

TEST(VersioningInterleavingTests, Error0055) {
  TestLibrary library;
  library.AddFile("bad/fi-0055.test.fidl");
  library.SelectVersion("test", "HEAD");
  library.ExpectFail(ErrInvalidReferenceToDeprecated, "alias 'RGB'",
                     VersionRange(Version::From(3).value(), Version::kPosInf),
                     Platform::Parse("test").value(), "table member 'color'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningInterleavingTests, Error0056) {
  SharedAmongstLibraries shared;
  shared.SelectVersion("foo", "HEAD");
  shared.SelectVersion("bar", "HEAD");
  TestLibrary dependency(&shared);
  dependency.AddFile("bad/fi-0056-a.test.fidl");
  ASSERT_COMPILED(dependency);
  TestLibrary library(&shared);
  library.AddFile("bad/fi-0056-b.test.fidl");
  library.ExpectFail(ErrInvalidReferenceToDeprecatedOtherPlatform, "alias 'RGB'",
                     VersionRange(Version::From(2).value(), Version::kPosInf),
                     Platform::Parse("foo").value(), "table member 'color'",
                     VersionRange(Version::From(3).value(), Version::kPosInf),
                     Platform::Parse("bar").value());
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

}  // namespace
}  // namespace fidlc
