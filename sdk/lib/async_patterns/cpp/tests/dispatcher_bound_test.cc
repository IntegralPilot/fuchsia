// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/testing/cpp/real_loop.h>
#include <lib/async/cpp/executor.h>
#include <lib/async_patterns/cpp/dispatcher_bound.h>
#include <lib/sync/cpp/completion.h>

#include <thread>

#include <gtest/gtest.h>
#include <sdk/lib/async_patterns/cpp/receiver.h>

#include "src/lib/testing/predicates/status.h"

namespace {

// In this test fixture, the loop should always be run from the main thread.
class DispatcherBound : public testing::Test, public loop_fixture::RealLoop {
 protected:
  void SetUp() override { loop_thread_id_ = std::this_thread::get_id(); }

  void TearDown() override {}

  async::Loop& loop() { return loop_fixture::RealLoop::loop(); }

  std::thread::id loop_thread_id() const { return loop_thread_id_; }

 private:
  std::thread::id loop_thread_id_;
};

using ArcAtomic = std::shared_ptr<std::atomic_int>;
ArcAtomic make_arc_atomic() { return std::make_shared<std::atomic_int>(); }

// |ThreadAffine| asserts that it is always used from a particular thread.
// It will be used to check that |DispatcherBound| schedules all work on the
// dispatcher thread.
class ThreadAffine {
 public:
  explicit ThreadAffine(std::thread::id expected, ArcAtomic counter)
      : expected_(expected), counter_(std::move(counter)) {
    EXPECT_EQ(expected_, std::this_thread::get_id());
    counter_->fetch_add(1);
  }

  ~ThreadAffine() {
    EXPECT_EQ(expected_, std::this_thread::get_id());
    counter_->fetch_sub(1);
  }

  // The following are a bunch of thread-affine operations that take different
  // kinds of data types and exercised in the tests later.

  void NoArg() { EXPECT_EQ(expected_, std::this_thread::get_id()); }

  void Add(int a, int b, const ArcAtomic& result) {
    EXPECT_EQ(expected_, std::this_thread::get_id());
    result->store(a + b);
  }

  void PassMoveOnly(std::unique_ptr<int> p) {
    EXPECT_EQ(expected_, std::this_thread::get_id());
    EXPECT_NE(nullptr, p.get());
  }

  void PassPointer(std::string* s) {
    EXPECT_EQ(expected_, std::this_thread::get_id());
    EXPECT_NE("", *s);
    *s = "";
  }

  void PassReference(std::string& s) {
    EXPECT_EQ(expected_, std::this_thread::get_id());
    EXPECT_NE("", s);
    s = "";
  }

  void PassConstReference(const std::string& s) {
    EXPECT_EQ(expected_, std::this_thread::get_id());
    EXPECT_NE("", s);
  }

  void PassValue(std::vector<int> v) {
    EXPECT_EQ(expected_, std::this_thread::get_id());
    EXPECT_GT(v.size(), 0u);
    v = {};
  }

 private:
  std::thread::id expected_;
  ArcAtomic counter_;
};

TEST_F(DispatcherBound, ConstructDisengaged) {
  async_patterns::DispatcherBound<ThreadAffine> obj{loop().dispatcher()};
  EXPECT_FALSE(obj.has_value());
}

TEST_F(DispatcherBound, Construct) {
  libsync::Completion create;
  libsync::Completion destroy;
  auto object_count = make_arc_atomic();

  // From a foreign thread |t1|, we schedule the creation and destruction of a
  // |ThreadAffine| object onto the loop.
  std::thread t1{[&] {
    async_patterns::DispatcherBound<ThreadAffine> obj{loop().dispatcher(), std::in_place,
                                                      loop_thread_id(), object_count};
    EXPECT_TRUE(obj.has_value());
    create.Signal();

    destroy.Wait();
    obj.reset();
    EXPECT_FALSE(obj.has_value());
  }};

  create.Wait();
  EXPECT_EQ(0, object_count->load());

  EXPECT_OK(loop().RunUntilIdle());
  EXPECT_EQ(1, object_count->load());

  destroy.Signal();
  t1.join();
  EXPECT_EQ(1, object_count->load());

  EXPECT_OK(loop().RunUntilIdle());
  EXPECT_EQ(0, object_count->load());
}

TEST_F(DispatcherBound, Emplace) {
  libsync::Completion did_create_1;
  libsync::Completion will_create_2;
  libsync::Completion did_create_2;
  libsync::Completion will_destroy;
  auto object_count_1 = make_arc_atomic();
  auto object_count_2 = make_arc_atomic();

  // From a foreign thread |t1|, we schedule the creation and destruction of
  // two |ThreadAffine| objects onto the loop.
  std::thread t1{[&] {
    async_patterns::DispatcherBound<ThreadAffine> obj{loop().dispatcher()};
    obj.emplace(loop_thread_id(), object_count_1);
    EXPECT_TRUE(obj.has_value());
    did_create_1.Signal();

    will_create_2.Wait();
    obj.emplace(loop_thread_id(), object_count_2);
    EXPECT_TRUE(obj.has_value());
    did_create_2.Signal();

    will_destroy.Wait();
  }};

  did_create_1.Wait();
  EXPECT_EQ(0, object_count_1->load());
  EXPECT_EQ(0, object_count_2->load());

  EXPECT_OK(loop().RunUntilIdle());
  EXPECT_EQ(1, object_count_1->load());
  EXPECT_EQ(0, object_count_2->load());

  will_create_2.Signal();
  did_create_2.Wait();
  EXPECT_EQ(1, object_count_1->load());
  EXPECT_EQ(0, object_count_2->load());

  EXPECT_OK(loop().RunUntilIdle());
  EXPECT_EQ(0, object_count_1->load());
  EXPECT_EQ(1, object_count_2->load());

  will_destroy.Signal();
  t1.join();
  EXPECT_EQ(0, object_count_1->load());
  EXPECT_EQ(1, object_count_2->load());

  EXPECT_OK(loop().RunUntilIdle());
  EXPECT_EQ(0, object_count_1->load());
  EXPECT_EQ(0, object_count_2->load());
}

TEST_F(DispatcherBound, AsyncCall) {
  auto count = make_arc_atomic();
  async_patterns::DispatcherBound<ThreadAffine> obj{loop().dispatcher(), std::in_place,
                                                    loop_thread_id(), count};
  EXPECT_OK(loop().RunUntilIdle());

  {
    obj.AsyncCall(&ThreadAffine::NoArg);
    EXPECT_OK(loop().RunUntilIdle());
  }

  {
    auto result = make_arc_atomic();
    obj.AsyncCall(&ThreadAffine::Add, 1, 2, result);
    EXPECT_EQ(0, result->load());
    EXPECT_OK(loop().RunUntilIdle());
    EXPECT_EQ(3, result->load());
  }

  {
    std::unique_ptr p = std::make_unique<int>(42);
    obj.AsyncCall(&ThreadAffine::PassMoveOnly, std::move(p));
    EXPECT_EQ(nullptr, p);
    EXPECT_OK(loop().RunUntilIdle());
  }

  // Copy a |T| to a receiver that expects a |const T&|.
  {
    std::string s = "abc";
    obj.AsyncCall(&ThreadAffine::PassConstReference, s);
    EXPECT_EQ("abc", s);
    EXPECT_OK(loop().RunUntilIdle());
    EXPECT_EQ("abc", s);
  }

  // Move a |T| to a receiver that expects a |const T&|.
  {
    std::string s = "abc";
    obj.AsyncCall(&ThreadAffine::PassConstReference, std::move(s));
    EXPECT_EQ("", s);
    EXPECT_OK(loop().RunUntilIdle());
    EXPECT_EQ("", s);
  }

  // Copy a |T&| to a receiver that expects a |const T&|.
  {
    std::optional<std::string> s = std::string{"abc"};
    std::string& s_ref = s.value();
    obj.AsyncCall(&ThreadAffine::PassConstReference, s_ref);
    // After firing the async call, the queued call should have its own private
    // copy of |s|, so it should be allowed to destroy our |s| here.
    s.reset();
    EXPECT_OK(loop().RunUntilIdle());
  }

  // Move a |T| to a receiver that expects a |T|.
  {
    std::vector<int> v{1, 2, 3};
    obj.AsyncCall(&ThreadAffine::PassValue, std::move(v));
    EXPECT_EQ(0u, v.size());
    EXPECT_OK(loop().RunUntilIdle());
    EXPECT_EQ(0u, v.size());
  }

  // Copy a |T&| to a receiver that expects a |T|.
  {
    std::vector<int> v2{1, 2, 3};
    std::vector<int>& v2_ref = v2;
    obj.AsyncCall(&ThreadAffine::PassValue, v2_ref);
    EXPECT_EQ(3u, v2_ref.size());
    EXPECT_OK(loop().RunUntilIdle());
    EXPECT_EQ(3u, v2_ref.size());
  }

  // Calling a function that consumes a move-only type with |T&| is not allowed.
#if 0
  {
    std::unique_ptr p = std::make_unique<int>(42);
    obj.AsyncCall(&ThreadAffine::PassMoveOnly, p);
    EXPECT_EQ(nullptr, p);
    EXPECT_OK(loop().RunUntilIdle());
  }
#endif

  // Pass-through a |T&| to a receiver that expects a |T&| is not supported.
#if 0
  {
    std::string s = "abc";
    std::string& s2 = s;
    obj.AsyncCall(&ThreadAffine::PassReference, s2);
    EXPECT_EQ("abc", s2);
    EXPECT_OK(loop().RunUntilIdle());
    EXPECT_EQ("", s2);
  }
#endif

  // Pointer arguments are not supported.
#if 0
  {
    std::string s = "abc";
    obj.AsyncCall(&ThreadAffine::PassPointer, &s);
    EXPECT_EQ("abc", s);
    EXPECT_OK(loop().RunUntilIdle());
    EXPECT_EQ("", s);
  }
#endif
}

TEST_F(DispatcherBound, AsyncCallWithReply) {
  class Background {
   public:
    explicit Background(std::string base) : base_(std::move(base)) {}

    std::string Concat(const std::string& arg) { return base_ + arg; }

   private:
    std::string base_;
  };

  // |Owner| asynchronously tells |Background| to concatenate a string,
  // and check the result in |DoneConcat|.
  class Owner {
   public:
    explicit Owner(async_dispatcher_t* owner_dispatcher) : receiver_{this, owner_dispatcher} {
      background_.AsyncCall(&Background::Concat, std::string("def"))
          .Then(receiver_.Once(&Owner::DoneConcat));

      // Passing incompatible types is not allowed.
#if 0
      background_.AsyncCall(&Background::Concat, std::string("def"))
          .Then(receiver_.Once([](Owner*, int not_a_string) {}));
#endif
    }

    bool got_result() const { return got_result_; }

    async::Loop& background_loop() { return background_loop_; }

   private:
    void DoneConcat(const std::string& result) {
      EXPECT_FALSE(got_result_);
      EXPECT_EQ(result, "abcdef");
      got_result_ = true;
    }

    async::Loop background_loop_{&kAsyncLoopConfigNeverAttachToThread};
    async_patterns::DispatcherBound<Background> background_{background_loop_.dispatcher(),
                                                            std::in_place, std::string("abc")};
    async_patterns::Receiver<Owner> receiver_;
    bool got_result_ = false;
  };

  Owner owner{loop().dispatcher()};
  EXPECT_FALSE(owner.got_result());
  // Nothing to process on the main loop.
  ASSERT_OK(loop().RunUntilIdle());
  EXPECT_FALSE(owner.got_result());
  // Background loop should process |Concat| and post back the result.
  ASSERT_OK(owner.background_loop().RunUntilIdle());
  EXPECT_FALSE(owner.got_result());
  // Main loop should process |DoneConcat|.
  ASSERT_OK(loop().RunUntilIdle());
  EXPECT_TRUE(owner.got_result());
}

TEST_F(DispatcherBound, AsyncCallOverloaded) {
  struct Object {
    int Pass(int a) { return a; }
    std::string Pass(std::string a) { return a; }
  };

  struct Owner {
    explicit Owner(async_dispatcher_t* dispatcher)
        : receiver{this, dispatcher}, obj{dispatcher, std::in_place} {
      obj.AsyncCall<int(int)>(&Object::Pass, 1).Then(receiver.Once([](Owner* owner, int a) {
        EXPECT_EQ(1, a);
        owner->count++;
      }));

      obj.AsyncCall<std::string(std::string)>(&Object::Pass, std::string{"a"})
          .Then(receiver.Once([](Owner* owner, const std::string& a) {
            EXPECT_EQ("a", a);
            owner->count++;
          }));
    }

    async_patterns::Receiver<Owner> receiver;
    async_patterns::DispatcherBound<Object> obj;
    int count = 0;
  } owner{loop().dispatcher()};

  loop().RunUntilIdle();
  EXPECT_EQ(2, owner.count);
}

namespace {

// This is shared among the next few tests.
class Background {
 public:
  explicit Background(std::shared_ptr<int> count) : count_(std::move(count)) {}

  std::string Concat(const std::string& arg) {
    (*count_)++;
    return arg;
  }

 private:
  std::shared_ptr<int> count_;
};

TEST_F(DispatcherBound, AsyncCallFireAndForget) {
  std::shared_ptr<int> count = std::make_shared<int>(0);
  async_patterns::DispatcherBound<Background> obj{loop().dispatcher(), std::in_place, count};
  obj.AsyncCall(&Background::Concat, std::string("foo"));
  EXPECT_EQ(*count, 0);
  loop().RunUntilIdle();
  EXPECT_EQ(*count, 1);
}

TEST_F(DispatcherBound, AsyncCallToPromise) {
  std::shared_ptr<int> count = std::make_shared<int>(0);
  async_patterns::DispatcherBound<Background> obj{loop().dispatcher(), std::in_place, count};
  auto promise = obj.AsyncCall(&Background::Concat, std::string("foo")).promise();
  async::Executor executor(loop().dispatcher());
  bool received = false;
  executor.schedule_task(promise.and_then([&](std::string& result) {
    EXPECT_EQ(result, "foo");
    received = true;
  }));
  EXPECT_EQ(*count, 0);
  EXPECT_FALSE(received);
  loop().RunUntilIdle();
  EXPECT_EQ(*count, 1);
  EXPECT_TRUE(received);
}

}  // namespace

TEST_F(DispatcherBound, WaitForVoidAsyncCallToFinish) {
  class Background {
   public:
    explicit Background(std::shared_ptr<int> count) : count_(std::move(count)) {}

    void DoThing() { (*count_)++; }

   private:
    std::shared_ptr<int> count_;
  };

  std::shared_ptr<int> count = std::make_shared<int>(0);
  async_patterns::DispatcherBound<Background> obj{loop().dispatcher(), std::in_place, count};
  auto promise = obj.AsyncCall(&Background::DoThing).promise();
  async::Executor executor(loop().dispatcher());
  bool received = false;
  executor.schedule_task(promise.and_then([&]() { received = true; }));
  EXPECT_EQ(*count, 0);
  EXPECT_FALSE(received);
  loop().RunUntilIdle();
  EXPECT_EQ(*count, 1);
  EXPECT_TRUE(received);
}

TEST_F(DispatcherBound, PassDispatcherInConstructor) {
  std::shared_ptr shared_dispatcher_slot = std::make_shared<async_dispatcher_t*>(nullptr);
  struct Object {
    Object(async_dispatcher_t* dispatcher, std::shared_ptr<async_dispatcher_t*> out_dispatcher) {
      *out_dispatcher = dispatcher;
    }
  };
  async_patterns::DispatcherBound<Object> obj{
      loop().dispatcher(), std::in_place, async_patterns::PassDispatcher, shared_dispatcher_slot};
  EXPECT_EQ(nullptr, *shared_dispatcher_slot);
  loop().RunUntilIdle();
  EXPECT_EQ(loop().dispatcher(), *shared_dispatcher_slot);
}

TEST_F(DispatcherBound, PassDispatcherInAsyncCall) {
  std::shared_ptr shared_dispatcher_slot = std::make_shared<async_dispatcher_t*>(nullptr);
  struct Object {
    void Method(async_dispatcher_t* dispatcher,
                std::shared_ptr<async_dispatcher_t*> out_dispatcher) {
      *out_dispatcher = dispatcher;
    }
  };
  async_patterns::DispatcherBound<Object> obj{loop().dispatcher(), std::in_place};
  obj.AsyncCall(&Object::Method, async_patterns::PassDispatcher, shared_dispatcher_slot);
  EXPECT_EQ(nullptr, *shared_dispatcher_slot);
  loop().RunUntilIdle();
  EXPECT_EQ(loop().dispatcher(), *shared_dispatcher_slot);
}

TEST_F(DispatcherBound, SynchronouslyRunIfShutdown) {
  loop().Shutdown();
  auto object_count = make_arc_atomic();
  {
    async_patterns::DispatcherBound<ThreadAffine> obj{loop().dispatcher()};
    EXPECT_EQ(0, object_count->load());
    obj.emplace(loop_thread_id(), object_count);
    EXPECT_EQ(1, object_count->load());

    auto result = make_arc_atomic();
    obj.AsyncCall(&ThreadAffine::Add, 1, 2, result);
    EXPECT_EQ(3, result->load());
  }
  EXPECT_EQ(0, object_count->load());
}

TEST_F(DispatcherBound, ShutdownAfterConstruction) {
  auto object_count = make_arc_atomic();
  {
    async_patterns::DispatcherBound<ThreadAffine> obj{loop().dispatcher()};
    EXPECT_EQ(0, object_count->load());
    obj.emplace(loop_thread_id(), object_count);
    loop().Shutdown();
    EXPECT_EQ(1, object_count->load());
  }
  EXPECT_EQ(0, object_count->load());
}

TEST_F(DispatcherBound, ShutdownAfterAsyncCall) {
  auto object_count = make_arc_atomic();
  {
    async_patterns::DispatcherBound<ThreadAffine> obj{loop().dispatcher()};
    EXPECT_EQ(0, object_count->load());
    obj.emplace(loop_thread_id(), object_count);
    loop().RunUntilIdle();
    EXPECT_EQ(1, object_count->load());

    auto result = make_arc_atomic();
    obj.AsyncCall(&ThreadAffine::Add, 1, 2, result);
    loop().Shutdown();
    EXPECT_EQ(3, result->load());
  }
  EXPECT_EQ(0, object_count->load());
}

TEST_F(DispatcherBound, DispatcherOutlivesDispatcherBound) {
  auto count = make_arc_atomic();
  {
    async_patterns::DispatcherBound<ThreadAffine> obj{loop().dispatcher()};
    obj.emplace(loop_thread_id(), count);
  }
  loop().RunUntilIdle();
  EXPECT_EQ(0, count->load());
}

TEST_F(DispatcherBound, MakeDispatcherBound) {
  auto object_count = make_arc_atomic();
  auto obj = async_patterns::MakeDispatcherBound<ThreadAffine>(loop().dispatcher(),
                                                               loop_thread_id(), object_count);
  EXPECT_EQ(0, object_count->load());
  loop().RunUntilIdle();
  EXPECT_EQ(1, object_count->load());
}

TEST(DispatcherBoundDeathTest, DispatcherIsNotSynchronized) {
  struct Object {
    void Method() {}
  };

  // Construct the object on the main thread.
  async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};
  async_patterns::DispatcherBound<Object> object(loop.dispatcher());
  object.emplace();

  ASSERT_OK(loop.RunUntilIdle());

  // Call a member method, while the loop is now run from thread B.
  // This should panic while running the loop.
  object.AsyncCall(&Object::Method);
  std::thread thread_b([&] {
    ASSERT_DEATH(loop.RunUntilIdle(),
                 "\\|async_patterns::DispatcherBound\\| is meant to manage thread-unsafe "
                 "asynchronous objects.");
  });
  thread_b.join();
}

TEST_F(DispatcherBound, SubClass) {
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

  {
    async_patterns::DispatcherBound<Base> base(dispatcher());
    base.emplace(1);
    EXPECT_EQ(RunPromise(base.AsyncCall(&Base::Speak).promise()).value(), "Base");
  }

  {
    async_patterns::DispatcherBound<Base> base(dispatcher());
    base.emplace<Derived>(1, 2);
    EXPECT_EQ(RunPromise(base.AsyncCall(&Base::Speak).promise()).value(), "Derived");
  }
}

TEST_F(DispatcherBound, PureVirtualMethod) {
  struct Base {
    Base() = default;
    virtual ~Base() {}
    virtual std::string Speak() = 0;
    int a_;
  };
  struct Derived : public Base {
    Derived() = default;
    std::string Speak() override { return "Derived"; }
    int b_;
  };

  {
    async_patterns::DispatcherBound<Base> base(dispatcher());
    base.emplace<Derived>();
    EXPECT_EQ(RunPromise(base.AsyncCall(&Base::Speak).promise()).value(), "Derived");
  }
}

TEST_F(DispatcherBound, ComplexLayout) {
  class Foo {
   public:
    Foo() = default;
    virtual ~Foo() {}

   private:
    virtual std::string GetFoo() { return std::to_string(foo_); }
    int foo_;
  };
  class Bar {
   public:
    Bar() = default;
    virtual ~Bar() {}

   private:
    virtual std::string GetBar() { return std::to_string(bar_); }
    int bar_;
  };
  struct Base {
    Base() = default;
    virtual ~Base() {}
    virtual std::string Speak() = 0;
    int a_;
  };
  struct Derived : public Foo, public Base, public Bar {
    Derived() = default;
    std::string Speak() override { return "Derived"; }
    int b_;
  };

  {
    async_patterns::DispatcherBound<Base> base(dispatcher());
    base.emplace<Derived>();
    EXPECT_EQ(RunPromise(base.AsyncCall(&Base::Speak).promise()).value(), "Derived");
  }
}

}  // namespace
