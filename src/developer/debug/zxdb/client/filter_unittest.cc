// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/filter.h"

#include <gtest/gtest.h>

#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/shared/platform_message_loop.h"
#include "src/developer/debug/zxdb/client/remote_api_test.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/setting_schema_definition.h"

namespace zxdb {

namespace {

using debug::MessageLoop;

class FilterSink : public RemoteAPI {
 public:
  void UpdateFilter(const debug_ipc::UpdateFilterRequest& request,
                    fit::callback<void(const Err&, debug_ipc::UpdateFilterReply)> cb) override {
    filter_requests_.push_back(request);

    MessageLoop::Current()->PostTask(
        FROM_HERE, [cb = std::move(cb)]() mutable { cb(Err(), debug_ipc::UpdateFilterReply()); });
  }

  void Attach(const debug_ipc::AttachRequest& request,
              fit::callback<void(const Err&, debug_ipc::AttachReply)> cb) override {
    debug_ipc::AttachReply reply;
    reply.koid = request.koid;
    reply.name = "test";

    MessageLoop::Current()->PostTask(FROM_HERE,
                                     [cb = std::move(cb), reply]() mutable { cb(Err(), reply); });
  }

  std::vector<debug_ipc::UpdateFilterRequest> filter_requests_;
};

class FilterTest : public RemoteAPITest {
 public:
  FilterTest() = default;
  ~FilterTest() override = default;

  FilterSink& sink() { return *sink_; }

 protected:
  std::unique_ptr<RemoteAPI> GetRemoteAPIImpl() override {
    auto sink = std::make_unique<FilterSink>();
    sink_ = sink.get();
    return std::move(sink);
  }

 private:
  FilterSink* sink_;  // Owned by the session.
};

}  // namespace

TEST_F(FilterTest, SetFilters) {
  Filter* filter = session().system().CreateNewFilter();

  // There is no filter to send yet.
  ASSERT_EQ(sink().filter_requests_.size(), 0u);

  filter->SetType(debug_ipc::Filter::Type::kProcessNameSubstr);
  filter->SetPattern("foo");
  MessageLoop::Current()->RunUntilNoTasks();

  // There should be a filter request.
  ASSERT_EQ(sink().filter_requests_.size(), 1u);
  ASSERT_EQ(sink().filter_requests_[0].filters.size(), 1u);
  ASSERT_EQ(sink().filter_requests_[0].filters[0].pattern, "foo");

  // Deleting the filter should clean up the filters.
  session().system().DeleteFilter(filter);
  MessageLoop::Current()->RunUntilNoTasks();

  // There should be a filter request.
  ASSERT_EQ(sink().filter_requests_.size(), 2u);
  EXPECT_TRUE(sink().filter_requests_[1].filters.empty());
}

TEST_F(FilterTest, SetTypeSetting) {
  Filter* filter = session().system().CreateNewFilter();

  filter->settings().SetString(ClientSettings::Filter::kType, "component moniker prefix");
  filter->settings().SetString(ClientSettings::Filter::kPattern, "/my/pattern");

  // Because Filter's setting implementation guarantees synchronization with the backend.
  MessageLoop::Current()->RunUntilNoTasks();

  // There should be a filter request.
  ASSERT_EQ(sink().filter_requests_.size(), 1u);
  ASSERT_EQ(sink().filter_requests_[0].filters.size(), 1u);
  ASSERT_EQ(sink().filter_requests_[0].filters[0].pattern, "/my/pattern");
  ASSERT_EQ(sink().filter_requests_[0].filters[0].type,
            debug_ipc::Filter::Type::kComponentMonikerPrefix);

  filter->settings().SetString(ClientSettings::Filter::kType, "component moniker suffix");
  MessageLoop::Current()->RunUntilNoTasks();

  // Now it should be updated to the suffix version.
  ASSERT_EQ(sink().filter_requests_.size(), 2u);
  ASSERT_EQ(sink().filter_requests_.back().filters.size(), 1u);
  EXPECT_EQ(sink().filter_requests_.back().filters[0].type,
            debug_ipc::Filter::Type::kComponentMonikerSuffix);

  filter->settings().SetString(ClientSettings::Filter::kType, "component url");
  MessageLoop::Current()->RunUntilNoTasks();

  // And finally it should be updated to a URL type.
  ASSERT_EQ(sink().filter_requests_.size(), 3u);
  ASSERT_EQ(sink().filter_requests_.back().filters.size(), 1u);
  EXPECT_EQ(sink().filter_requests_.back().filters[0].type, debug_ipc::Filter::Type::kComponentUrl);
}

}  // namespace zxdb
