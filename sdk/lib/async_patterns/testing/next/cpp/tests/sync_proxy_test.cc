// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async/cpp/sequence_checker.h>
#include <lib/async_patterns/testing/next/cpp/sync_proxy.h>

#include <memory>
#include <string>

#include <gtest/gtest.h>

namespace {

class Background {
 public:
  explicit Background(async_dispatcher_t* dispatcher, std::shared_ptr<int> count)
      : checker_(dispatcher), count_(std::move(count)) {}

  void Void() const {
    std::lock_guard guard(checker_);
    (*count_)++;
  }

  int Int() {
    std::lock_guard guard(checker_);
    (*count_)++;
    return 42;
  }

  std::string String(int a, int b) {
    std::lock_guard guard(checker_);
    (*count_)++;
    return std::to_string(a + b);
  }

 private:
  async::synchronization_checker checker_;
  std::shared_ptr<int> count_;
};

ASYNC_PATTERNS_DEFINE_SYNC_METHOD(BackgroundSyncProxy, Void);
ASYNC_PATTERNS_DEFINE_SYNC_METHOD(BackgroundSyncProxy, Int);
ASYNC_PATTERNS_DEFINE_SYNC_METHOD(BackgroundSyncProxy, String);

using BackgroundSyncProxy = async_patterns::SyncProxy<
    Background, ASYNC_PATTERNS_ADD_SYNC_METHOD(BackgroundSyncProxy, Background, Void),
    ASYNC_PATTERNS_ADD_SYNC_METHOD(BackgroundSyncProxy, Background, Int),
    ASYNC_PATTERNS_ADD_SYNC_METHOD(BackgroundSyncProxy, Background, String)>;

TEST(SyncProxy, SyncCall) {
  async::Loop remote_loop{&kAsyncLoopConfigNeverAttachToThread};
  remote_loop.StartThread();

  std::shared_ptr<int> count = std::make_shared<int>(0);
  BackgroundSyncProxy background(remote_loop.dispatcher(), std::in_place,
                                 async_patterns::PassDispatcher, count);
  EXPECT_EQ(*count, 0);

  background.Void();
  EXPECT_EQ(*count, 1);

  EXPECT_EQ(background.Int(), 42);
  EXPECT_EQ(*count, 2);

  EXPECT_EQ(background.String(2, 3), "5");
  EXPECT_EQ(*count, 3);
}

TEST(SyncProxy, ConstructDestruct) {
  async::Loop remote_loop{&kAsyncLoopConfigNeverAttachToThread};
  remote_loop.StartThread();

  std::shared_ptr<int> count = std::make_shared<int>(0);
  BackgroundSyncProxy background(remote_loop.dispatcher());
  EXPECT_FALSE(background.has_value());

  background.emplace(async_patterns::PassDispatcher, count);
  EXPECT_TRUE(background.has_value());

  background.reset();
  EXPECT_FALSE(background.has_value());
}

struct Base {
  explicit Base(int a) : a_(a) {}
  virtual ~Base() {}
  virtual std::string Speak() { return "Base"; }
  int a_;
};
struct Derived : public Base {
  Derived(int a, int b) : Base(a), b_(b) {}
  std::string Speak() override { return "Derived"; }
  int b_;
};

ASYNC_PATTERNS_DEFINE_SYNC_METHOD(BaseSyncProxy, Speak);

using BaseSyncProxy =
    async_patterns::SyncProxy<Base, ASYNC_PATTERNS_ADD_SYNC_METHOD(BaseSyncProxy, Base, Speak)>;

TEST(SyncProxy, SubClass) {
  async::Loop remote_loop{&kAsyncLoopConfigNeverAttachToThread};
  remote_loop.StartThread();

  {
    BaseSyncProxy base(remote_loop.dispatcher());
    base.emplace(1);
    EXPECT_EQ(base.Speak(), "Base");
  }

  {
    BaseSyncProxy base(remote_loop.dispatcher());
    base.emplace<Derived>(1, 2);
    EXPECT_EQ(base.Speak(), "Derived");
  }
}

}  // namespace
