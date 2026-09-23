// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "src/developer/debug/ipc/filter_utils.h"
#include "src/developer/debug/ipc/records.h"

namespace debug_ipc {

TEST(FilterUtils, FilterMatches) {
  Filter filter{.type = debug_ipc::Filter::Type::kProcessName, .pattern = "foo"};
  EXPECT_TRUE(FilterMatches(filter, "foo", {}));
  EXPECT_FALSE(FilterMatches(filter, "foobar", {}));

  filter = {.type = debug_ipc::Filter::Type::kProcessNameSubstr, .pattern = "foo"};
  EXPECT_TRUE(FilterMatches(filter, "foo", {}));
  EXPECT_TRUE(FilterMatches(filter, "foobar", {}));

  filter = {.type = debug_ipc::Filter::Type::kComponentMoniker, .pattern = "/core/abc"};
  EXPECT_TRUE(FilterMatches(filter, "", {ComponentInfo{.moniker = "/core/abc"}}));
  EXPECT_FALSE(FilterMatches(filter, "", {ComponentInfo{.moniker = "/core/abc/def"}}));

  filter = {.type = debug_ipc::Filter::Type::kComponentMonikerSuffix, .pattern = "abc/def"};
  EXPECT_TRUE(FilterMatches(filter, "", {ComponentInfo{.moniker = "/core/abc/def"}}));
  EXPECT_FALSE(FilterMatches(filter, "", {ComponentInfo{.moniker = "/core/abc"}}));

  filter = {.type = debug_ipc::Filter::Type::kComponentName, .pattern = "foo.cm"};
  EXPECT_TRUE(FilterMatches(filter, "", {ComponentInfo{.url = "pkg://host#meta/foo.cm"}}));

  filter = {.type = debug_ipc::Filter::Type::kComponentUrl, .pattern = "pkg://host#meta/foo.cm"};
  EXPECT_TRUE(
      FilterMatches(filter, "", {ComponentInfo{.url = "pkg://host?hash=abcd#meta/foo.cm"}}));
}

TEST(FilterUtils, GetAttachConfigsForFilterMatches) {
  // None of the filters need patterns, because they've already been determined to be a match.
  Filter filter1 = {
      .id = Filter::Identifier(1, Filter::Originator::kUnknown),
      .config =
          {
              .weak = true,
          },
  };

  Filter filter2 = {
      .id = Filter::Identifier(2, Filter::Originator::kUnknown),
      .config = {},
  };

  Filter filter3 = {
      .id = Filter::Identifier(3, Filter::Originator::kUnknown),
      .config =
          {
              .job_only = true,
          },
  };

  Filter filter4 = {
      .id = Filter::Identifier(4, Filter::Originator::kUnknown),
      .config =
          {
              .never_attach = true,
          },
  };

  std::vector<const Filter*> filters = {&filter1, &filter2, &filter3, &filter4};

  constexpr MatchedTask kWeakPid1 = {.koid = 1234, .type = TaskType::kProcess};
  constexpr MatchedTask kWeakPid2 = {.koid = 1235, .type = TaskType::kProcess};
  constexpr MatchedTask kStrongProcessPid = {.koid = 1236, .type = TaskType::kProcess};
  constexpr MatchedTask kJobPid = {.koid = 1237, .type = TaskType::kJob};
  constexpr MatchedTask kNoAttachPid = {.koid = 1238, .type = TaskType::kProcess};

  std::vector<FilterMatch> matches = {
      FilterMatch(filter1.id, {kWeakPid1, kWeakPid2, kStrongProcessPid}),
      FilterMatch(filter2.id, {kStrongProcessPid}), FilterMatch(filter3.id, {kJobPid}),
      FilterMatch(filter4.id, {kWeakPid2, kNoAttachPid})};

  auto result = GetAttachConfigsForFilterMatches(matches, filters);

  // There are 5 unique pids.
  EXPECT_EQ(result.size(), 5u);

  // kStrongProcessPid is matched by both a strong a weak filter, the attach request should be
  // strong.
  auto strong_attach = result.find(kStrongProcessPid.koid);
  ASSERT_NE(strong_attach, result.end());
  EXPECT_EQ(strong_attach->second.priority, AttachConfig::Priority::kStrong);
  EXPECT_EQ(strong_attach->second.target, TaskType::kProcess);

  auto job_attach = result.find(kJobPid.koid);
  ASSERT_NE(job_attach, result.end());
  EXPECT_EQ(job_attach->second.priority, AttachConfig::Priority::kStrong);
  EXPECT_EQ(job_attach->second.target, TaskType::kJob);

  auto weak_attach1 = result.find(kWeakPid1.koid);
  ASSERT_NE(weak_attach1, result.end());
  EXPECT_EQ(weak_attach1->second.priority, AttachConfig::Priority::kWeak);
  EXPECT_EQ(weak_attach1->second.target, TaskType::kProcess);

  auto weak_attach2 = result.find(kWeakPid2.koid);
  ASSERT_NE(weak_attach2, result.end());
  EXPECT_EQ(weak_attach2->second.priority, AttachConfig::Priority::kWeak);
  EXPECT_EQ(weak_attach2->second.target, TaskType::kProcess);

  auto no_attach = result.find(kNoAttachPid.koid);
  ASSERT_NE(no_attach, result.end());
  EXPECT_EQ(no_attach->second.priority, AttachConfig::Priority::kMinimal);
  EXPECT_EQ(no_attach->second.target, TaskType::kProcess);
}

TEST(FilterUtils, WeakOverridesNeverAttach) {
  // None of the filters need patterns, because they've already been determined to be a match.
  Filter filter1 = {
      .id = Filter::Identifier(1, Filter::Originator::kUnknown),
      .config =
          {
              .never_attach = true,
          },
  };

  Filter filter2 = {
      .id = Filter::Identifier(2, Filter::Originator::kUnknown),
      .config =
          {
              .weak = true,
          },
  };

  constexpr MatchedTask kWeakPid = {.koid = 12345, .type = TaskType::kProcess};
  constexpr MatchedTask kNoAttachPid = {.koid = 54321, .type = TaskType::kProcess};

  std::vector<const Filter*> filters = {&filter1, &filter2};

  // Make sure the NeverAttach filter match comes first.
  std::vector<FilterMatch> matches = {
      FilterMatch(filter1.id, {kWeakPid, kNoAttachPid}),
      FilterMatch(filter2.id, {kWeakPid}),
  };

  auto result = GetAttachConfigsForFilterMatches(matches, filters);

  // When the filter settings collide, the weak filter takes precedence, since it requires an
  // exception channel to be claimed to work as advertised.
  auto weak_attach = result.find(kWeakPid.koid);
  ASSERT_NE(weak_attach, result.end());
  EXPECT_EQ(weak_attach->second.priority, AttachConfig::Priority::kWeak);
  EXPECT_EQ(weak_attach->second.target, TaskType::kProcess);

  // No collision for this pid means the never attach setting from the filter is working as
  // intended.
  auto no_attach = result.find(kNoAttachPid.koid);
  ASSERT_NE(no_attach, result.end());
  EXPECT_EQ(no_attach->second.priority, AttachConfig::Priority::kMinimal);
  EXPECT_EQ(no_attach->second.target, TaskType::kProcess);
}

TEST(FilterUtils, ConflictingFiltersDifferentTargets) {
  // A filter that would be configured to match against a root job of a particular component's
  // realm.
  Filter filter1 = {
      .id = Filter::Identifier(1, Filter::Originator::kUnknown),
      .config =
          {
              .recursive = true,
              .job_only = true,
          },
  };

  // And this is the resulting component moniker prefix filter that gets installed by the above
  // recursive job only filter.
  Filter filter2 = {
      .id = Filter::Identifier(2, Filter::Originator::kUnknown),
      .config =
          {
              .never_attach = true,
          },
  };

  constexpr MatchedTask kMatchedJob = {.koid = 12345, .type = TaskType::kJob};
  constexpr MatchedTask kMatchedProcess = {.koid = 12346, .type = TaskType::kProcess};

  std::vector<const Filter*> filters = {&filter1, &filter2};

  // The job_only filter should only be reported as matching the job, since it's configured to be
  // job only. Likewise, the resulting component moniker prefix filter is NOT labeled as job_only,
  // so it will match the process.
  std::vector<FilterMatch> matches = {
      FilterMatch(filter1.id, {kMatchedJob}),
      FilterMatch(filter2.id, {kMatchedProcess}),
  };

  auto result = GetAttachConfigsForFilterMatches(matches, filters);
  ASSERT_EQ(result.size(), 2u);

  auto job_attach = result.find(kMatchedJob.koid);
  ASSERT_NE(job_attach, result.end());
  EXPECT_EQ(job_attach->second.priority, AttachConfig::Priority::kStrong);
  EXPECT_EQ(job_attach->second.target, TaskType::kJob);

  auto no_attach = result.find(kMatchedProcess.koid);
  ASSERT_NE(no_attach, result.end());
  EXPECT_EQ(no_attach->second.priority, AttachConfig::Priority::kMinimal);
  EXPECT_EQ(no_attach->second.target, TaskType::kProcess);
}

}  // namespace debug_ipc
