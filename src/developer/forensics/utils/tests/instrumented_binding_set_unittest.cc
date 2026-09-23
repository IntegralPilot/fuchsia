// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/utils/instrumented_binding_set.h"

#include <fuchsia/feedback/cpp/fidl.h>
#include <fuchsia/feedback/cpp/fidl_test_base.h>
#include <lib/syslog/cpp/macros.h>

#include <string>

#include "src/developer/forensics/testing/unit_test_fixture.h"
#include "src/developer/forensics/utils/inspect_node_manager.h"

namespace forensics {
namespace {

using inspect::testing::ChildrenMatch;
using inspect::testing::NameMatches;
using inspect::testing::NodeMatches;
using inspect::testing::PropertyList;
using inspect::testing::UintIs;
using testing::AllOf;
using testing::UnorderedElementsAreArray;

class StubCrashReporter : public fuchsia::feedback::testing::CrashReporter_TestBase {
 public:
  void NotImplemented_(const std::string& name) override { FX_CHECK(false); }
};

class InstrumentedBindingSetTest : public UnitTestFixture {
 public:
  InstrumentedBindingSetTest()
      : inspect_node_manager_(&InspectRoot()),
        bindings_(dispatcher(), &crash_reporter_, &inspect_node_manager_, "/fidl") {}

  InstrumentedBindingSet<fuchsia::feedback::CrashReporter>& Bindings() { return bindings_; }

 protected:
  InspectNodeManager inspect_node_manager_;
  StubCrashReporter crash_reporter_;
  InstrumentedBindingSet<fuchsia::feedback::CrashReporter> bindings_;
};

TEST_F(InstrumentedBindingSetTest, Check_MakingAndClosingConnections) {
  fuchsia::feedback::CrashReporterPtr ptr1;
  fuchsia::feedback::CrashReporterPtr ptr2;
  fuchsia::feedback::CrashReporterPtr ptr3;

  EXPECT_THAT(InspectTree(), ChildrenMatch(ElementsAre(NodeMatches(AllOf(
                                 NameMatches("fidl"), PropertyList(UnorderedElementsAreArray({
                                                          UintIs("current_num_connections", 0u),
                                                          UintIs("total_num_connections", 0u),
                                                      })))))));
  // 2 New connections: 2 created, 2 active
  Bindings().AddBinding(ptr1.NewRequest(dispatcher()));
  Bindings().AddBinding(ptr2.NewRequest(dispatcher()));

  EXPECT_THAT(InspectTree(), ChildrenMatch(ElementsAre(NodeMatches(AllOf(
                                 NameMatches("fidl"), PropertyList(UnorderedElementsAreArray({
                                                          UintIs("current_num_connections", 2u),
                                                          UintIs("total_num_connections", 2u),
                                                      })))))));

  // Close 1 connection: 2 created, 1 active
  ptr1.Unbind();
  RunLoopUntilIdle();

  EXPECT_THAT(InspectTree(), ChildrenMatch(ElementsAre(NodeMatches(AllOf(
                                 NameMatches("fidl"), PropertyList(UnorderedElementsAreArray({
                                                          UintIs("current_num_connections", 1u),
                                                          UintIs("total_num_connections", 2u),
                                                      })))))));

  // 1 New Connection: 3 created, 2 active
  Bindings().AddBinding(ptr3.NewRequest(dispatcher()));

  EXPECT_THAT(InspectTree(), ChildrenMatch(ElementsAre(NodeMatches(AllOf(
                                 NameMatches("fidl"), PropertyList(UnorderedElementsAreArray({
                                                          UintIs("current_num_connections", 2u),
                                                          UintIs("total_num_connections", 3u),
                                                      })))))));

  // Close 2 connections: 3 created, 0 active
  ptr2.Unbind();
  ptr3.Unbind();
  RunLoopUntilIdle();

  EXPECT_THAT(InspectTree(), ChildrenMatch(ElementsAre(NodeMatches(AllOf(
                                 NameMatches("fidl"), PropertyList(UnorderedElementsAreArray({
                                                          UintIs("current_num_connections", 0u),
                                                          UintIs("total_num_connections", 3u),
                                                      })))))));
}

}  // namespace
}  // namespace forensics
