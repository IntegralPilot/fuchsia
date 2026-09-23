// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/crash_reports/info/inspect_manager.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>

#include <cstdint>
#include <memory>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/crash_reports/errors.h"
#include "src/developer/forensics/crash_reports/product.h"
#include "src/developer/forensics/crash_reports/reporting_policy_watcher.h"
#include "src/developer/forensics/testing/unit_test_fixture.h"
#include "src/developer/forensics/utils/errors.h"

namespace forensics {
namespace crash_reports {
namespace {

using inspect::testing::ChildrenMatch;
using inspect::testing::NameMatches;
using inspect::testing::NodeMatches;
using inspect::testing::PropertyList;
using inspect::testing::StringIs;
using inspect::testing::UintIs;
using testing::Contains;
using testing::ElementsAre;
using testing::UnorderedElementsAre;
using testing::UnorderedElementsAreArray;

constexpr char kComponentUrl[] = "fuchsia-pkg://fuchsia.com/my-pkg#meta/my-component.cm";

class InspectManagerTest : public UnitTestFixture {
 public:
  void SetUp() override { inspect_manager_ = std::make_unique<InspectManager>(&InspectRoot()); }

 protected:
  std::unique_ptr<InspectManager> inspect_manager_;
};

TEST_F(InspectManagerTest, InitialInspectTree) {
  EXPECT_THAT(InspectTree(),
              ChildrenMatch(UnorderedElementsAre(AllOf(NodeMatches(NameMatches("crash_reporter")),
                                                       ChildrenMatch(UnorderedElementsAreArray({
                                                           NodeMatches(NameMatches("settings")),
                                                       }))))));
}

class TestReportingPolicyWatcher : public ReportingPolicyWatcher {
 public:
  TestReportingPolicyWatcher() : ReportingPolicyWatcher(ReportingPolicy::kUndecided) {}

  void Set(const ReportingPolicy policy) { SetPolicy(policy); }
};

TEST_F(InspectManagerTest, ExposeSettings_TrackUploadPolicyChanges) {
  TestReportingPolicyWatcher watcher;
  inspect_manager_->ExposeReportingPolicy(&watcher);
  EXPECT_THAT(InspectTree(),
              ChildrenMatch(Contains(
                  AllOf(NodeMatches(NameMatches("crash_reporter")),
                        ChildrenMatch(Contains(NodeMatches(AllOf(
                            NameMatches("settings"),
                            PropertyList(ElementsAre(StringIs(
                                "upload_policy", ToString(ReportingPolicy::kUndecided))))))))))));

  watcher.Set(ReportingPolicy::kArchive);
  EXPECT_THAT(InspectTree(),
              ChildrenMatch(Contains(
                  AllOf(NodeMatches(NameMatches("crash_reporter")),
                        ChildrenMatch(Contains(NodeMatches(AllOf(
                            NameMatches("settings"),
                            PropertyList(ElementsAre(StringIs(
                                "upload_policy", ToString(ReportingPolicy::kArchive))))))))))));

  watcher.Set(ReportingPolicy::kDoNotFileAndDelete);
  EXPECT_THAT(
      InspectTree(),
      ChildrenMatch(Contains(
          AllOf(NodeMatches(NameMatches("crash_reporter")),
                ChildrenMatch(Contains(NodeMatches(AllOf(
                    NameMatches("settings"),
                    PropertyList(ElementsAre(StringIs(
                        "upload_policy", ToString(ReportingPolicy::kDoNotFileAndDelete))))))))))));

  watcher.Set(ReportingPolicy::kUpload);
  EXPECT_THAT(InspectTree(),
              ChildrenMatch(Contains(
                  AllOf(NodeMatches(NameMatches("crash_reporter")),
                        ChildrenMatch(Contains(NodeMatches(AllOf(
                            NameMatches("settings"),
                            PropertyList(ElementsAre(StringIs(
                                "upload_policy", ToString(ReportingPolicy::kUpload))))))))))));
}

TEST_F(InspectManagerTest, IncreaseReportsGarbageCollectedBy) {
  const uint64_t kNumReportsGarbageCollected = 10;
  for (size_t i = 1; i < 5; ++i) {
    inspect_manager_->IncreaseReportsGarbageCollectedBy(kNumReportsGarbageCollected);
    EXPECT_THAT(InspectTree(),
                ChildrenMatch(Contains(AllOf(
                    NodeMatches(NameMatches("crash_reporter")),
                    ChildrenMatch(Contains(NodeMatches(AllOf(
                        NameMatches("store"),
                        PropertyList(ElementsAre(UintIs("num_reports_garbage_collected",
                                                        i * kNumReportsGarbageCollected)))))))))));
  }
}

TEST_F(InspectManagerTest, UpsertComponentToProductMapping) {
  // 1. We insert a product with all the fields set.
  const Product product{
      .name = "some name",
      .version = ErrorOrString("some version"),
      .channel = ErrorOrString("some channel"),
  };
  inspect_manager_->UpsertComponentToProductMapping(kComponentUrl, product);
  EXPECT_THAT(InspectTree(), ChildrenMatch(Contains(AllOf(
                                 NodeMatches(NameMatches("crash_register")),
                                 ChildrenMatch(Contains(AllOf(
                                     NodeMatches(NameMatches("mappings")),
                                     ChildrenMatch(UnorderedElementsAreArray({
                                         NodeMatches(AllOf(NameMatches(kComponentUrl),
                                                           PropertyList(UnorderedElementsAreArray({
                                                               StringIs("name", "some name"),
                                                               StringIs("version", "some version"),
                                                               StringIs("channel", "some channel"),
                                                           })))),
                                     })))))))));

  // 2. We insert the same product under a different component URL.
  const std::string another_component_url = std::string(kComponentUrl) + "2";
  inspect_manager_->UpsertComponentToProductMapping(another_component_url, product);
  EXPECT_THAT(InspectTree(), ChildrenMatch(Contains(AllOf(
                                 NodeMatches(NameMatches("crash_register")),
                                 ChildrenMatch(Contains(AllOf(
                                     NodeMatches(NameMatches("mappings")),
                                     ChildrenMatch(UnorderedElementsAreArray({
                                         NodeMatches(AllOf(NameMatches(kComponentUrl),
                                                           PropertyList(UnorderedElementsAreArray({
                                                               StringIs("name", "some name"),
                                                               StringIs("version", "some version"),
                                                               StringIs("channel", "some channel"),
                                                           })))),
                                         NodeMatches(AllOf(NameMatches(another_component_url),
                                                           PropertyList(UnorderedElementsAreArray({
                                                               StringIs("name", "some name"),
                                                               StringIs("version", "some version"),
                                                               StringIs("channel", "some channel"),
                                                           })))),
                                     })))))))));

  // 3. We update the product under the first component URL with some missing fields.
  const Product another_product{
      .name = "some other name",
      .version = ErrorOrString(Error::kMissingValue),
      .channel = ErrorOrString(Error::kMissingValue),
  };
  inspect_manager_->UpsertComponentToProductMapping(kComponentUrl, another_product);
  EXPECT_THAT(InspectTree(),
              ChildrenMatch(Contains(AllOf(
                  NodeMatches(NameMatches("crash_register")),
                  ChildrenMatch(Contains(AllOf(
                      NodeMatches(NameMatches("mappings")),
                      ChildrenMatch(UnorderedElementsAreArray({
                          NodeMatches(AllOf(NameMatches(kComponentUrl),
                                            PropertyList(UnorderedElementsAreArray({
                                                StringIs("name", "some other name"),
                                                StringIs("version", ToReason(Error::kMissingValue)),
                                                StringIs("channel", ToReason(Error::kMissingValue)),
                                            })))),
                          NodeMatches(AllOf(NameMatches(another_component_url),
                                            PropertyList(UnorderedElementsAreArray({
                                                StringIs("name", "some name"),
                                                StringIs("version", "some version"),
                                                StringIs("channel", "some channel"),
                                            })))),
                      })))))))));
}

}  // namespace
}  // namespace crash_reports
}  // namespace forensics
