// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>

#include <memory>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace {

TEST(TestDispatcherBound, SyncCallReturnInt) {
  async::Loop remote_loop{&kAsyncLoopConfigNeverAttachToThread};
  remote_loop.StartThread();
  class Object {
   public:
    int Get() { return 42; }
    int Add(int a, int b) { return a + b; }
  };
  async_patterns::TestDispatcherBound<Object> object{remote_loop.dispatcher(), std::in_place};
  int result = object.SyncCall(&Object::Get);
  EXPECT_EQ(result, 42);
  int sum = object.SyncCall(&Object::Add, 1, 2);
  EXPECT_EQ(sum, 3);
}

TEST(TestDispatcherBound, SyncCallReturnMoveOnly) {
  async::Loop remote_loop{&kAsyncLoopConfigNeverAttachToThread};
  remote_loop.StartThread();
  class Object {
   public:
    std::unique_ptr<int> PassThrough(std::unique_ptr<int> a) { return a; }
  };
  async_patterns::TestDispatcherBound<Object> object{remote_loop.dispatcher(), std::in_place};
  std::unique_ptr<int> result = object.SyncCall(&Object::PassThrough, std::make_unique<int>(1));
  EXPECT_TRUE(result);
  EXPECT_EQ(*result, 1);
}

TEST(TestDispatcherBound, SyncCallReturnVoid) {
  async::Loop remote_loop{&kAsyncLoopConfigNeverAttachToThread};
  remote_loop.StartThread();

  class Object {
   public:
    explicit Object(std::shared_ptr<std::atomic_int> count) : count_(std::move(count)) {}
    void Increment() { count_->fetch_add(1); }

   private:
    std::shared_ptr<std::atomic_int> count_;
  };
  std::shared_ptr<std::atomic_int> count = std::make_shared<std::atomic_int>();
  async_patterns::TestDispatcherBound<Object> object{remote_loop.dispatcher(), std::in_place,
                                                     count};
  EXPECT_EQ(count->load(), 0);
  static_assert(std::is_void_v<decltype(object.SyncCall(&Object::Increment))>);
  object.SyncCall(&Object::Increment);
  EXPECT_EQ(count->load(), 1);
  object.SyncCall(&Object::Increment);
  EXPECT_EQ(count->load(), 2);
}

TEST(TestDispatcherBound, SyncCallWithGeneralLambda) {
  async::Loop remote_loop{&kAsyncLoopConfigNeverAttachToThread};
  remote_loop.StartThread();

  class Object {
   public:
    int Get() { return 1; }
  };
  async_patterns::TestDispatcherBound<Object> object{remote_loop.dispatcher(), std::in_place};
  int result = object.SyncCall([num = 2](Object* object) { return object->Get() + num; });
  EXPECT_EQ(result, 3);
}

TEST(TestDispatcherBound, AsyncCallWithReplyUsingFuture) {
  class Background {
   public:
    explicit Background(std::string base) : base_(std::move(base)) {}

    std::string Concat(const std::string& arg) { return base_ + arg; }

   private:
    std::string base_;
  };

  async::Loop background_loop{&kAsyncLoopConfigNeverAttachToThread};
  async_patterns::TestDispatcherBound<Background> background{background_loop.dispatcher(),
                                                             std::in_place, std::string("abc")};

  std::future fut = background.AsyncCall(&Background::Concat, std::string("def")).ToFuture();
  std::chrono::time_point infinite_past = std::chrono::system_clock::time_point::min();

  EXPECT_EQ(std::future_status::timeout, fut.wait_until(infinite_past));
  // Background loop should process |Concat| and post back the result.
  ASSERT_OK(background_loop.RunUntilIdle());
  EXPECT_EQ(std::future_status::ready, fut.wait_until(infinite_past));
  EXPECT_EQ("abcdef", fut.get());
}

TEST(TestDispatcherBound, SyncCallOverloaded) {
  async::Loop remote_loop{&kAsyncLoopConfigNeverAttachToThread};
  remote_loop.StartThread();

  struct Object {
    int Pass(int a) { return a; }
    std::string Pass(std::string a) { return a; }
  };
  async_patterns::TestDispatcherBound<Object> obj{remote_loop.dispatcher(), std::in_place};

  EXPECT_EQ(1, obj.SyncCall<int(int)>(&Object::Pass, 1));
  EXPECT_EQ("a", obj.SyncCall<std::string(std::string)>(&Object::Pass, std::string{"a"}));
}

TEST(TestDispatcherBound, PassDispatcherInSyncCall) {
  async::Loop remote_loop{&kAsyncLoopConfigNeverAttachToThread};
  remote_loop.StartThread();

  std::shared_ptr shared_dispatcher_slot = std::make_shared<async_dispatcher_t*>(nullptr);
  struct Object {
    void Method(async_dispatcher_t* dispatcher,
                std::shared_ptr<async_dispatcher_t*> out_dispatcher) {
      *out_dispatcher = dispatcher;
    }
  };
  async_patterns::TestDispatcherBound<Object> obj{remote_loop.dispatcher(), std::in_place};
  obj.SyncCall(&Object::Method, async_patterns::PassDispatcher, shared_dispatcher_slot);
  EXPECT_EQ(remote_loop.dispatcher(), *shared_dispatcher_slot);
}

TEST(TestDispatcherBound, MakeTestDispatcherBound) {
  async::Loop remote_loop{&kAsyncLoopConfigNeverAttachToThread};
  remote_loop.StartThread();
  class Object {
   public:
    explicit Object(int a) : a_(a) {}
    int a() { return a_; }

   private:
    int a_;
  };
  auto obj = async_patterns::MakeTestDispatcherBound<Object>(remote_loop.dispatcher(), 1);
  EXPECT_EQ(1, obj.SyncCall(&Object::a));
}

}  // namespace
