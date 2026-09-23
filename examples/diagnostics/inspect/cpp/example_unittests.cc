// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fidl/cpp/binding.h>

#include "echo_connection.h"

// [START test_imports]
#include <fidl/fidl.examples.routing.echo/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/testing/cpp/inspect.h>

#include <gtest/gtest.h>
#include <src/lib/testing/loop_fixture/real_loop_fixture.h>

using namespace inspect::testing;
// [END test_imports]

namespace example {

class EchoConnectionTest : public gtest::RealLoopFixture {
 public:
  EchoConnectionTest()
      : inspector_(),
        stats_{std::make_shared<EchoConnectionStats>(EchoConnectionStats{
            inspector_.GetRoot().CreateUint("bytes_processed", 0),
            inspector_.GetRoot().CreateUint("total_requests", 0),
        })},
        connection_(stats_) {}

  void SetUp() override {
    auto [client_end, server_end] =
        fidl::CreateEndpoints<fidl_examples_routing_echo::Echo>().value();

    fidl::BindServer(dispatcher(), std::move(server_end), &connection_);

    echo_client_.emplace(
        fidl::Client<fidl_examples_routing_echo::Echo>(std::move(client_end), dispatcher()));
  }

 protected:
  inspect::Inspector inspector_;
  std::shared_ptr<EchoConnectionStats> stats_;
  EchoConnection connection_;
  std::optional<fidl::Client<fidl_examples_routing_echo::Echo>> echo_client_;
};

TEST_F(EchoConnectionTest, EchoServerWritesStats) {
  auto on_response = [&](fidl::Result<fidl_examples_routing_echo::Echo::EchoString>& result) {
    ASSERT_TRUE(result.is_ok());
  };

  // Invoke the echo server
  echo_client_.value()->EchoString({"Hello World!"}).ThenExactlyOnce(on_response);
  echo_client_.value()->EchoString({"Hello World!"}).ThenExactlyOnce(on_response);

  RunLoopUntilIdle();

  // [START inspect_test]
  // Validate the contents of the tree match
  auto hierarchy_result = inspect::ReadFromVmo(inspector_.DuplicateVmo());
  ASSERT_TRUE(hierarchy_result.is_ok());
  EXPECT_THAT(hierarchy_result.take_value(),
              NodeMatches(AllOf(PropertyList(::testing::UnorderedElementsAre(
                  UintIs("bytes_processed", 24), UintIs("total_requests", 2))))));
  // [END inspect_test]
}

}  // namespace example
