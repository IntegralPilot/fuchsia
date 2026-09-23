// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/assert.h>

#include <random>
#include <string>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

const int kRepeatTestCount = 10;

struct DeclarationOrderTests : public testing::Test {
 public:
  // Adds random prefixes to names surrounded by "#" characters. For example,
  // "#Foo#" might become "W__Foo". This helps to eliminate the effect of
  // lexicographical name ordering and exclusively test dependency ordering.
  std::string Mangle(std::string input) {
    size_t start = 0;
    std::map<std::string, char> letters;
    std::uniform_int_distribution<size_t> distribution(0, 25);
    while ((start = input.find_first_of('#', start)) != std::string::npos) {
      auto end = input.find_first_of('#', start + 1);
      auto key = input.substr(start + 1, end - start - 1);
      auto [it, inserted] = letters.emplace(key, 0);
      if (inserted)
        it->second = static_cast<char>('A' + distribution(rng_));
      char letter = it->second;
      input.replace(start, end - start + 1, std::string(1, letter) + "__" + key);
    }
    return input;
  }

  // Given a vector of decls, returns a vector of their unmangled names.
  std::vector<std::string_view> Unmangle(const std::vector<const Decl*>& decls) {
    std::vector<std::string_view> result;
    for (auto& decl : decls) {
      auto name = decl->name.decl_name();
      auto idx = name.find("__");
      if (decl->name.as_anonymous()) {
        // Anonymous type names are converted to CamelCase with underscores removed.
        ZX_ASSERT_MSG(idx == std::string::npos, "%s", std::string(name).c_str());
        result.push_back(name.substr(1));
      } else {
        ZX_ASSERT_MSG(idx == 1, "%s", std::string(name).c_str());
        result.push_back(name.substr(idx + 2));
      }
    }
    return result;
  }

 private:
  static const size_t kSeed = 1337;
  std::default_random_engine rng_{kSeed};
};

// Checks if an order (vector of strings) is the union of a set of suborders.
MATCHER_P(IsUnionOf, suborders, "union of suborders " + testing::PrintToString(suborders)) {
  std::vector<bool> used(arg.size(), false);
  for (auto& suborder : suborders) {
    std::optional<std::string_view> prev;
    size_t prev_index = 0;
    for (auto& name : suborder) {
      auto it = std::find(arg.begin(), arg.end(), name);
      if (it == arg.end()) {
        *result_listener << "'" << name << "' not found";
        return false;
      }
      size_t index = it - arg.begin();
      if (used[index]) {
        *result_listener << "'" << name << "' used twice";
        return false;
      }
      used[index] = true;
      if (prev && index < prev_index) {
        *result_listener << "'" << name << "' came before '" << *prev << "'";
        return false;
      }
      prev = name;
      prev_index = index;
    }
  }
  for (size_t i = 0; i < arg.size(); i++) {
    if (!used[i]) {
      *result_listener << "unexpected '" << arg[i] << "'";
      return false;
    }
  }
  return true;
}

// This test ensures that there are no unused anonymous structs in the
// declaration order output.
TEST_F(DeclarationOrderTests, GoodNoUnusedAnonymousNames) {
  for (int i = 0; i < kRepeatTestCount; i++) {
    auto source = Mangle(R"FIDL(
library example;

protocol #Protocol# {
    strict Method() -> ();
};
)FIDL");
    TestLibrary library(source);
    ASSERT_COMPILED(library);
    std::vector<std::string_view> expected = {"Protocol"};
    ASSERT_EQ(Unmangle(library.declaration_order()), expected);
  }
}

TEST_F(DeclarationOrderTests, GoodNonnullableRef) {
  for (int i = 0; i < kRepeatTestCount; i++) {
    auto source = Mangle(R"FIDL(
library example;

type #Request# = struct {
  req array<#Element#, 4>;
};

type #Element# = struct {};

protocol #Protocol# {
  SomeMethod(struct { req #Request#; });
};
)FIDL");
    TestLibrary library(source);
    ASSERT_COMPILED(library);
    std::vector<std::string_view> expected = {
        "Element",
        "Request",
        "ProtocolSomeMethodRequest",
        "Protocol",
    };
    ASSERT_EQ(Unmangle(library.declaration_order()), expected);
  }
}

TEST_F(DeclarationOrderTests, GoodNullableRefBreaksDependency) {
  for (int i = 0; i < kRepeatTestCount; i++) {
    auto source = Mangle(R"FIDL(
library example;

type #Request# = resource struct {
  req array<box<#Element#>, 4>;
};

type #Element# = resource struct {
  prot client_end:#Protocol#;
};

protocol #Protocol# {
  SomeMethod(resource struct { req #Request#; });
};
)FIDL");
    TestLibrary library(source);
    ASSERT_COMPILED(library);
    std::vector<std::vector<std::string_view>> expected_suborders = {
        {"Element"},
        {"Request", "ProtocolSomeMethodRequest", "Protocol"},
    };
    ASSERT_THAT(Unmangle(library.declaration_order()), IsUnionOf(expected_suborders));
  }
}

TEST_F(DeclarationOrderTests, GoodRequestTypeBreaksDependencyGraph) {
  for (int i = 0; i < kRepeatTestCount; i++) {
    auto source = Mangle(R"FIDL(
library example;

type #Request# = resource struct {
  req server_end:#Protocol#;
};

protocol #Protocol# {
  SomeMethod(resource struct { req #Request#; });
};
)FIDL");
    TestLibrary library(source);
    ASSERT_COMPILED(library);
    std::vector<std::string_view> expected = {"Request", "ProtocolSomeMethodRequest", "Protocol"};
    ASSERT_EQ(Unmangle(library.declaration_order()), expected);
  }
}

TEST_F(DeclarationOrderTests, GoodNonnullableUnion) {
  for (int i = 0; i < kRepeatTestCount; i++) {
    auto source = Mangle(R"FIDL(
library example;

type #Union# = resource union {
  1: req server_end:#Protocol#;
  2: foo #Payload#;
};

protocol #Protocol# {
  SomeMethod(resource struct { req #Union#; });
};

type #Payload# = struct {
  a int32;
};
)FIDL");
    TestLibrary library(source);
    ASSERT_COMPILED(library);
    std::vector<std::string_view> expected = {
        "Payload",
        "Union",
        "ProtocolSomeMethodRequest",
        "Protocol",
    };
    ASSERT_EQ(Unmangle(library.declaration_order()), expected);
  }
}

TEST_F(DeclarationOrderTests, GoodNullableUnion) {
  for (int i = 0; i < kRepeatTestCount; i++) {
    auto source = Mangle(R"FIDL(
library example;

type #Union# = resource union {
  1: req server_end:#Protocol#;
  2: foo #Payload#;
};

protocol #Protocol# {
  SomeMethod(resource struct { req #Union#:optional; });
};

type #Payload# = struct {
  a int32;
};
)FIDL");
    TestLibrary library(source);
    ASSERT_COMPILED(library);
    std::vector<std::vector<std::string_view>> expected_suborders = {
        {"Payload", "Union"},
        {"ProtocolSomeMethodRequest", "Protocol"},
    };
    ASSERT_THAT(Unmangle(library.declaration_order()), IsUnionOf(expected_suborders));
  }
}

TEST_F(DeclarationOrderTests, GoodNonnullableUnionInStruct) {
  for (int i = 0; i < kRepeatTestCount; i++) {
    auto source = Mangle(R"FIDL(
library example;

type #Payload# = struct {
  a int32;
};

protocol #Protocol# {
  SomeMethod(struct { req #Request#; });
};

type #Request# = struct {
  u #Union#;
};

type #Union# = union {
  1: foo #Payload#;
};
)FIDL");
    TestLibrary library(source);
    ASSERT_COMPILED(library);
    std::vector<std::string_view> expected = {
        "Payload", "Union", "Request", "ProtocolSomeMethodRequest", "Protocol",
    };
    ASSERT_EQ(Unmangle(library.declaration_order()), expected);
  }
}

TEST_F(DeclarationOrderTests, GoodNullableUnionInStruct) {
  for (int i = 0; i < kRepeatTestCount; i++) {
    auto source = Mangle(R"FIDL(
library example;

type #Payload# = struct {
  a int32;
};

protocol #Protocol# {
  SomeMethod(struct { req #Request#; });
};

type #Request# = struct {
  u #Union#:optional;
};

type #Union# = union {
  1: foo #Payload#;
};
)FIDL");
    TestLibrary library(source);
    ASSERT_COMPILED(library);
    std::vector<std::vector<std::string_view>> expected_suborders = {
        {"Payload", "Union"},
        {"Request", "ProtocolSomeMethodRequest", "Protocol"},
    };
    ASSERT_THAT(Unmangle(library.declaration_order()), IsUnionOf(expected_suborders));
  }
}

TEST_F(DeclarationOrderTests, GoodMultipleLibraries) {
  for (int i = 0; i < kRepeatTestCount; i++) {
    auto sources = Mangle(R"FIDL(
// dependency.fidl
library dependency;

type #Decl1# = struct {};

// example.fidl
library example;

using dependency;

type #Decl0# = struct {};
type #Decl2# = struct {};

protocol #Decl1# {
  Method(struct { arg dependency.#Decl1#; });
};
)FIDL");
    auto index = sources.find("// example.fidl");
    SharedAmongstLibraries shared;
    TestLibrary dependency(&shared, "dependency.fidl", sources.substr(0, index));
    ASSERT_COMPILED(dependency);
    TestLibrary library(&shared, "example.fidl", sources.substr(index));
    ASSERT_COMPILED(library);

    std::vector<std::string_view> expected = {"Decl1"};
    ASSERT_EQ(Unmangle(dependency.declaration_order()), expected);

    std::vector<std::vector<std::string_view>> expected_suborders = {
        {"Decl0"},
        {"Decl2"},
        {"Decl1MethodRequest", "Decl1"},
    };
    ASSERT_THAT(Unmangle(library.declaration_order()), IsUnionOf(expected_suborders));
  }
}

TEST_F(DeclarationOrderTests, GoodConstTypeComesFirst) {
  for (int i = 0; i < kRepeatTestCount; i++) {
    auto source = Mangle(R"FIDL(
library example;

const #Constant# #Alias# = 42;

alias #Alias# = uint32;
)FIDL");
    TestLibrary library(source);
    ASSERT_COMPILED(library);
    std::vector<std::string_view> expected = {"Alias", "Constant"};
    ASSERT_EQ(Unmangle(library.declaration_order()), expected);
  }
}

TEST_F(DeclarationOrderTests, GoodEnumOrdinalTypeComesFirst) {
  for (int i = 0; i < kRepeatTestCount; i++) {
    auto source = Mangle(R"FIDL(
library example;

type #Enum# = enum : #Alias# { A = 1; };

alias #Alias# = uint32;
)FIDL");
    TestLibrary library(source);
    ASSERT_COMPILED(library);
    std::vector<std::string_view> expected = {"Alias", "Enum"};
    ASSERT_EQ(Unmangle(library.declaration_order()), expected);
  }
}

TEST_F(DeclarationOrderTests, GoodBitsOrdinalTypeComesFirst) {
  for (int i = 0; i < kRepeatTestCount; i++) {
    auto source = Mangle(R"FIDL(
library example;

type #Bits# = bits : #Alias# { A = 1; };

alias #Alias# = uint32;
)FIDL");
    TestLibrary library(source);
    ASSERT_COMPILED(library);
    std::vector<std::string_view> expected = {"Alias", "Bits"};
    ASSERT_EQ(Unmangle(library.declaration_order()), expected);
  }
}

}  // namespace
}  // namespace fidlc
