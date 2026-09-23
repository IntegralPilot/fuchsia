// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/utils/component/component.h"

#include <lib/fit/defer.h>
#include <lib/fit/function.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/sys/cpp/testing/component_context_provider.h>
#include <lib/syslog/cpp/macros.h>

#include <memory>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/files/path.h"
#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

namespace forensics {
namespace component {
namespace {

class ComponentTest : public gtest::TestLoopFixture {
 protected:
  ComponentTest() : context_provider_(dispatcher()) {}

  void TearDown() override {
    // Delete any files a component may have made.
    FX_CHECK(files::DeletePath("/tmp/component", /*recursive=*/true));
  }

  std::unique_ptr<sys::ComponentContext> TakeContext() { return context_provider_.TakeContext(); }
  std::shared_ptr<sys::ServiceDirectory> Services() {
    return context_provider_.public_service_directory();
  }

 private:
  sys::testing::ComponentContextProvider context_provider_;
};

// Allow Components in subsequent tests to use a user provided dispatcher.
class ComponentForTest : public Component {
 public:
  ComponentForTest(async_dispatcher_t* dispatcher, std::unique_ptr<sys::ComponentContext> context)
      : Component(dispatcher, std::move(context)) {}
};

TEST_F(ComponentTest, LogPreviousStarts) {
  {
    ComponentForTest instance1(dispatcher(),
                               std::make_unique<sys::ComponentContext>(nullptr, dispatcher()));
    EXPECT_TRUE(instance1.IsFirstInstance());
  }
  {
    ComponentForTest instance2(dispatcher(),
                               std::make_unique<sys::ComponentContext>(nullptr, dispatcher()));
    EXPECT_FALSE(instance2.IsFirstInstance());
  }
  {
    ComponentForTest instance3(dispatcher(),
                               std::make_unique<sys::ComponentContext>(nullptr, dispatcher()));
    EXPECT_FALSE(instance3.IsFirstInstance());
  }
}

}  // namespace
}  // namespace component
}  // namespace forensics
