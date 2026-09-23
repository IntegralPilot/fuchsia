// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "zircon/system/utest/device-enumeration/common.h"

#include <array>
#include <string>
#include <unordered_set>
#include <vector>

#include <zxtest/zxtest.h>

namespace device_enumeration {
namespace {

class DeviceEnumerationLibraryTest : public DeviceEnumerationTest {
 public:
  void SetUp() override {
    SetSkipNodeRetrieval(true);
    DeviceEnumerationTest::SetUp();
  }

 protected:
  void SetNodes(std::span<const char* const> monikers) {
    std::vector<std::string> str_monikers(monikers.begin(), monikers.end());
    SetNodeMonikers(std::move(str_monikers));
  }

  static void ExpectMatchedNodes(const MatchResult& result,
                                 std::span<const char* const> expected_monikers) {
    EXPECT_EQ(expected_monikers.size(), result.matched_nodes.size());
    std::unordered_set<std::string_view> actual(result.matched_nodes.begin(),
                                                result.matched_nodes.end());
    for (const char* expected : expected_monikers) {
      EXPECT_TRUE(actual.contains(expected));
    }
  }

  static void ExpectMissingErrors(const MatchResult& result,
                                  std::span<const char* const> missing_monikers) {
    EXPECT_EQ(missing_monikers.size(), result.errors.size());
    for (const char* missing : missing_monikers) {
      EXPECT_TRUE(std::ranges::any_of(
          result.errors, [missing](const std::string& err) { return err.contains(missing); }));
    }
  }
};

// HasNode matches present monikers.
TEST_F(DeviceEnumerationLibraryTest, HasNode) {
  static const char* kMonikers[] = {"first/node", "second/node"};
  SetNodes(kMonikers);

  EXPECT_TRUE(HasNode(kMonikers[0]));
  EXPECT_TRUE(HasNode(kMonikers[1]));
  EXPECT_FALSE(HasNode("nonexistent/node"));
}

// Single node requirement.
TEST_F(DeviceEnumerationLibraryTest, SingleNode) {
  static const char* kMonikers[] = {"single/node"};
  SetNodes(kMonikers);
  VerifyNodes(kMonikers, /*fail_on_unexpected_nodes=*/true);
}

// AllOf requires every listed moniker.
TEST_F(DeviceEnumerationLibraryTest, AllOf) {
  static const char* kMonikers[] = {
      "first/of/allof",
      "second/of/allof",
  };
  SetNodes(kMonikers);
  VerifyNodes(kMonikers, /*fail_on_unexpected_nodes=*/true);
}

// OneOf matches when any single option is present.
TEST_F(DeviceEnumerationLibraryTest, OneOf) {
  static const char* kOneOfOptions[] = {
      "first/of/oneof",
      "second/of/oneof",
      "third/of/oneof",
  };
  for (const char* option : kOneOfOptions) {
    SetNodes(std::array{option});
    VerifyOneOf(kOneOfOptions);
  }
}

// AllOf combining OneOf and AllOf requirements.
TEST_F(DeviceEnumerationLibraryTest, Nested) {
  static const char* kOneOfOptions[] = {"first/of/nested_one_of", "second/of/nested_one_of"};
  static const char* kAllOfMonikers[] = {"first/of/nested_all_of"};

  SetNodes(Combine(kOneOfOptions[1], kAllOfMonikers));

  Requirement req = AllOf({
      OneOf(kOneOfOptions),
      AllOf(kAllOfMonikers),
  });

  Verify(req, /*fail_on_unexpected_nodes=*/true);
}

// AllOf requiring multiple OneOf branches.
TEST_F(DeviceEnumerationLibraryTest, AllOfOneOfOneOf) {
  static const char* kFirstOneOfOptions[] = {"first/of/first_one_of", "second/of/first_one_of"};
  static const char* kSecondOneOfOptions[] = {"first/of/second_one_of", "second/of/second_one_of"};

  SetNodes(std::array{kFirstOneOfOptions[0], kSecondOneOfOptions[1]});

  Requirement req = AllOf({
      OneOf(kFirstOneOfOptions),
      OneOf(kSecondOneOfOptions),
  });

  Verify(req, /*fail_on_unexpected_nodes=*/true);
}

// OneOf matching an alternative AllOf branch.
TEST_F(DeviceEnumerationLibraryTest, OneOfAllOfAllOf) {
  static const char* kFirstAllOfMonikers[] = {"first/of/first_all_of", "second/of/first_all_of"};
  static const char* kSecondAllOfMonikers[] = {"first/of/second_all_of", "second/of/second_all_of"};

  SetNodes(kFirstAllOfMonikers);

  Requirement req = OneOf({
      AllOf(kFirstAllOfMonikers),
      AllOf(kSecondAllOfMonikers),
  });

  Verify(req, /*fail_on_unexpected_nodes=*/true);
}

// OneOf matching a fallback OneOf branch.
TEST_F(DeviceEnumerationLibraryTest, OneOfAllOfOneOf) {
  static const char* kAllOfMonikers[] = {"first/of/nested_all_of", "second/of/nested_all_of"};
  static const char* kOneOfOptions[] = {"first/of/nested_one_of", "second/of/nested_one_of"};

  SetNodes(std::array{kOneOfOptions[0]});

  Requirement req = OneOf({
      AllOf(kAllOfMonikers),
      OneOf(kOneOfOptions),
  });

  Verify(req, /*fail_on_unexpected_nodes=*/true);
}

// Ignore unlisted nodes when fail_on_unexpected_nodes is false.
TEST_F(DeviceEnumerationLibraryTest, AllowsUnexpectedNodes) {
  static const char* kMonikers[] = {"expected/node"};
  SetNodes(Combine(kMonikers, "unlisted/extra/node"));
  VerifyNodes(kMonikers, /*fail_on_unexpected_nodes=*/false);
}

// Missing node error includes partial matches.
TEST_F(DeviceEnumerationLibraryTest, PartialMatchMissingNode) {
  static const char* kMonikers[] = {
      "first/of/allof",
      "second/of/allof",
      "third/of/allof",
  };
  SetNodes(kMonikers);

  Requirement req = AllOf(Combine(kMonikers, "missing/node"));
  MatchResult result = GetMatchedNodes(req);

  EXPECT_TRUE(result.is_error());
  ExpectMatchedNodes(result, kMonikers);
  ExpectMissingErrors(result, {{"missing/node"}});
}

// Partial match count excludes unlisted nodes.
TEST_F(DeviceEnumerationLibraryTest, PartialMatchLeftoverNode) {
  static const char* kMonikers[] = {
      "first/of/allof",
      "second/of/allof",
  };
  SetNodes(Combine(kMonikers, "extra/unlisted/node"));

  Requirement req = AllOf(Combine(kMonikers, "missing/node"));
  MatchResult result = GetMatchedNodes(req);

  EXPECT_TRUE(result.is_error());
  ExpectMatchedNodes(result, kMonikers);
  ExpectMissingErrors(result, {{"missing/node"}});
}

// OneOf returns error when no options match.
TEST_F(DeviceEnumerationLibraryTest, OneOfNoMatch) {
  static const char* kOneOfOptions[] = {
      "first/of/oneof",
      "second/of/oneof",
  };
  SetNodes(std::array{"unmatched/node"});

  Requirement req = OneOf(kOneOfOptions);
  MatchResult result = GetMatchedNodes(req);

  EXPECT_TRUE(result.is_error());
  EXPECT_TRUE(result.matched_nodes.empty());
  EXPECT_FALSE(result.errors.empty());
}

// AllOf returns error when device has no nodes.
TEST_F(DeviceEnumerationLibraryTest, EmptyNodeSet) {
  static const char* kMonikers[] = {"single/node"};
  SetNodes({});

  Requirement req = AllOf(kMonikers);
  MatchResult result = GetMatchedNodes(req);

  EXPECT_TRUE(result.is_error());
  EXPECT_TRUE(result.matched_nodes.empty());
}

// Leftover nodes leave matched_nodes subset smaller than total nodes.
TEST_F(DeviceEnumerationLibraryTest, AllOfLeftoverNode) {
  static const char* kMonikers[] = {"expected/node"};
  SetNodes(Combine(kMonikers, "unlisted/extra/node"));

  Requirement req = AllOf(kMonikers);
  MatchResult result = GetMatchedNodes(req);

  EXPECT_TRUE(result.is_ok());
  EXPECT_EQ(1u, result.matched_nodes.size());
}

// Partial match in failing OneOf branches preserves matched nodes.
TEST_F(DeviceEnumerationLibraryTest, PartialMatchOneOf) {
  static const char* kFirstBranch[] = {"first/matched", "first/missing"};
  static const char* kSecondBranch[] = {"second/matched", "second/missing"};
  static const char* kMatchedMonikers[] = {"first/matched", "second/matched"};
  static const char* kMissingMonikers[] = {"first/missing", "second/missing"};

  SetNodes(kMatchedMonikers);

  Requirement req = OneOf({
      AllOf(kFirstBranch),
      AllOf(kSecondBranch),
  });
  MatchResult result = GetMatchedNodes(req);

  EXPECT_TRUE(result.is_error());
  ExpectMatchedNodes(result, kMatchedMonikers);
  ExpectMissingErrors(result, kMissingMonikers);
}

}  // namespace
}  // namespace device_enumeration
