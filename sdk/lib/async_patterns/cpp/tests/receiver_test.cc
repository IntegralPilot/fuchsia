// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/fpromise/promise.h>
#include <lib/fpromise/single_threaded_executor.h>
#include <lib/sync/cpp/completion.h>

#include <thread>

#include <gtest/gtest.h>
#include <sdk/lib/async_patterns/cpp/callback.h>
#include <sdk/lib/async_patterns/cpp/receiver.h>

#include "src/lib/testing/predicates/status.h"

namespace {

struct Owner {
 public:
  explicit Owner(async_dispatcher_t* dispatcher, int& count)
      : receiver_{this, dispatcher}, count_{count} {}

  void OnCallback(int arg) {
    EXPECT_EQ(arg, 42);
    count_++;
  }

  int OnCallbackWithCounter(int arg) {
    EXPECT_EQ(arg, 42);
    count_++;
    return count_;
  }

  async_patterns::Callback<void(int)> GetCallback() { return receiver_.Once(&Owner::OnCallback); }

  async_patterns::Callback<int(int)> GetCallbackWithCounter() {
    return receiver_.Once(&Owner::OnCallbackWithCounter);
  }

  void OnCallbackMoveOnly(std::unique_ptr<int> arg) {
    EXPECT_EQ(*arg, 42);
    count_++;
  }

  async_patterns::Callback<void(std::unique_ptr<int>)> GetCallbackMoveOnly() {
    return receiver_.Once(&Owner::OnCallbackMoveOnly);
  }

 private:
  async_patterns::Receiver<Owner> receiver_;
  int& count_;
};

TEST(Receiver, Receive) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);

  int count = 0;
  Owner owner{loop.dispatcher(), count};
  async_patterns::Callback<void(int)> callback = owner.GetCallback();

  EXPECT_EQ(count, 0);
  callback(42);
  EXPECT_EQ(count, 0);
  ASSERT_OK(loop.RunUntilIdle());
  EXPECT_EQ(count, 1);
}

TEST(Receiver, ReceiveMoveOnly) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);

  int count = 0;
  Owner owner{loop.dispatcher(), count};
  async_patterns::Callback callback = owner.GetCallbackMoveOnly();

  EXPECT_EQ(count, 0);
  callback(std::make_unique<int>(42));
  EXPECT_EQ(count, 0);
  ASSERT_OK(loop.RunUntilIdle());
  EXPECT_EQ(count, 1);
}

TEST(Receiver, ReceiveConvertToFitCallback) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);

  int count = 0;
  Owner owner{loop.dispatcher(), count};
  async_patterns::Callback<void(int)> callback = owner.GetCallback();
  fit::callback<void(int)> fit_callback = std::move(callback).ignore_result();

  EXPECT_EQ(count, 0);
  fit_callback(42);
  EXPECT_EQ(count, 0);
  ASSERT_OK(loop.RunUntilIdle());
  EXPECT_EQ(count, 1);
}

TEST(Receiver, OnceToVoidPromise) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  int count = 0;
  Owner owner{loop.dispatcher(), count};
  async_patterns::Callback<void(int)> callback = owner.GetCallback();

  // Fire-and-forget
  {
    EXPECT_EQ(count, 0);
    callback(42);
    EXPECT_EQ(count, 0);
    ASSERT_OK(loop.RunUntilIdle());
    EXPECT_EQ(count, 1);
  }

  // Get promise.
  {
    count = 0;
    callback = owner.GetCallback();
    EXPECT_EQ(count, 0);
    fpromise::promise<> promise = callback(42).promise();
    EXPECT_EQ(count, 0);
    fpromise::single_threaded_executor executor;
    bool called = false;
    executor.schedule_task(promise.and_then([&called]() { called = true; }));
    ASSERT_OK(loop.RunUntilIdle());
    ASSERT_FALSE(called);
    executor.run();
    ASSERT_TRUE(called);
    EXPECT_EQ(count, 1);
  }
}

TEST(Receiver, OnceToPromise) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  int count = 0;
  Owner owner{loop.dispatcher(), count};
  async_patterns::Callback<int(int)> callback = owner.GetCallbackWithCounter();

  // Fire-and-forget
  {
    EXPECT_EQ(count, 0);
    callback(42);
    EXPECT_EQ(count, 0);
    ASSERT_OK(loop.RunUntilIdle());
    EXPECT_EQ(count, 1);
  }

  // Get promise.
  {
    count = 0;
    callback = owner.GetCallbackWithCounter();
    EXPECT_EQ(count, 0);
    fpromise::promise<int> promise = callback(42).promise();
    EXPECT_EQ(count, 0);
    fpromise::single_threaded_executor executor;
    bool called = false;
    executor.schedule_task(promise.and_then([&called](int& count) {
      called = true;
      ASSERT_EQ(count, 1);
    }));
    ASSERT_OK(loop.RunUntilIdle());
    ASSERT_FALSE(called);
    executor.run();
    ASSERT_TRUE(called);
    EXPECT_EQ(count, 1);
  }
}

TEST(Receiver, CannotReceiveAfterDestruction) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);

  int count = 0;
  std::optional<Owner> owner_store;
  owner_store.emplace(loop.dispatcher(), count);
  Owner& owner = *owner_store;
  async_patterns::Callback<void(int)> callback = owner.GetCallback();

  EXPECT_EQ(count, 0);
  callback(42);
  EXPECT_EQ(count, 0);
  owner_store.reset();
  ASSERT_OK(loop.RunUntilIdle());
  EXPECT_EQ(count, 0);
}

TEST(Receiver, CannotReceiveAfterDispatcherShutdown) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);

  int count = 0;
  Owner owner{loop.dispatcher(), count};
  async_patterns::Callback<void(int)> callback = owner.GetCallback();

  EXPECT_EQ(count, 0);
  callback(42);
  EXPECT_EQ(count, 0);
  loop.Shutdown();
  EXPECT_EQ(count, 0);
}

TEST(Receiver, CannotReceiveAfterDispatcherShutdownAndBothGoAway) {
  std::unique_ptr loop = std::make_unique<async::Loop>(&kAsyncLoopConfigNeverAttachToThread);

  int count = 0;
  std::unique_ptr owner = std::make_unique<Owner>(loop->dispatcher(), count);
  async_patterns::Callback<void(int)> callback = owner->GetCallback();

  owner.reset();
  loop.reset();

  EXPECT_EQ(count, 0);
  callback(42);
  EXPECT_EQ(count, 0);
}

TEST(Receiver, CheckSynchronization) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);

  int count = 0;
  Owner owner{loop.dispatcher(), count};

  async_patterns::Callback<void(int)> callback = owner.GetCallback();
  callback(42);

  // If |Owner| lives on the main thread, we cannot dispatch tasks to it
  // from an arbitrary thread.
  std::thread([&] { ASSERT_DEATH(loop.RunUntilIdle(), "thread-unsafe"); }).join();
}

TEST(Receiver, BindLambda) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);

  struct Owner {
   public:
    explicit Owner(async_dispatcher_t* dispatcher) : receiver_{this, dispatcher} {}

    async_patterns::Callback<void(int)> GetCallback() {
      return receiver_.Once([](Owner* owner, int arg) {
        EXPECT_EQ(arg, 42);
        owner->count_++;
      });
    }

    // Won't compile due to static assertion.
#if 0
    async_patterns::Callback<int> GetCallbackWithStatefulLambda() {
      return receiver_.Once([this](Owner* owner, int arg) {
        EXPECT_EQ(arg, 42);
        owner->count_++;
      });
    }
#endif

    int count() const { return count_; }

   private:
    int count_ = 0;
    async_patterns::Receiver<Owner> receiver_;
  };
  Owner owner{loop.dispatcher()};
  EXPECT_EQ(owner.count(), 0);

  async_patterns::Callback<void(int)> callback = owner.GetCallback();
  EXPECT_EQ(owner.count(), 0);

  callback(42);
  EXPECT_EQ(owner.count(), 0);

  ASSERT_OK(loop.RunUntilIdle());
  EXPECT_EQ(owner.count(), 1);
}

class DiagnosticsExample {
 public:
  explicit DiagnosticsExample(async_dispatcher_t* dispatcher) : receiver_{this, dispatcher} {}

  async_patterns::Function<void(std::string)> StreamLogs() {
    return receiver_.Repeating(&DiagnosticsExample::OnLog);
  }

  async_patterns::Function<size_t(std::string)> StreamLogsWithCounter() {
    return receiver_.Repeating(&DiagnosticsExample::OnLogWithCounter);
  }

  async_patterns::Function<void(std::string)> StreamLogsWithLambda() {
    return receiver_.Repeating(
        [](DiagnosticsExample* self, std::string log) { self->OnLog(std::move(log)); });
  }

  async_patterns::Function<void(std::unique_ptr<std::string>)> StreamUniqueLogs() {
    return receiver_.Repeating(&DiagnosticsExample::OnUniqueLog);
  }

  async_patterns::Function<void(std::unique_ptr<std::string>)> StreamUniqueLogsWithLambda() {
    return receiver_.Repeating([](DiagnosticsExample* self, std::unique_ptr<std::string> log) {
      self->OnUniqueLog(std::move(log));
    });
  }

  const std::vector<std::string>& logs() const { return logs_; }

 private:
  void OnLog(std::string log) { logs_.push_back(std::move(log)); }

  size_t OnLogWithCounter(std::string log) {
    logs_.push_back(std::move(log));
    return logs_.size();
  }

  void OnUniqueLog(std::unique_ptr<std::string> log) { OnLog(std::move(*log)); }

  async_patterns::Receiver<DiagnosticsExample> receiver_;
  std::vector<std::string> logs_;
};

TEST(Receiver, RepeatingConstruction) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  DiagnosticsExample diag{loop.dispatcher()};
  async_patterns::Function<void(std::string)> log = diag.StreamLogs();
  static_assert(std::is_move_constructible_v<decltype(log)>);
  static_assert(std::is_copy_constructible_v<decltype(log)>);
}

TEST(Receiver, Repeating) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  DiagnosticsExample diag{loop.dispatcher()};
  async_patterns::Function<void(std::string)> log = diag.StreamLogs();

  {
    log(std::string{"hello"});
    EXPECT_EQ(diag.logs().size(), 0u);
    ASSERT_OK(loop.RunUntilIdle());
    std::vector<std::string> expected{"hello"};
    EXPECT_EQ(diag.logs(), expected);
  }

  // |Function| can be copied.
  {
    async_patterns::Function<void(std::string)> log2 = log;
    log2(std::string{"world"});
    log(std::string{"abc"});
    ASSERT_OK(loop.RunUntilIdle());
    std::vector<std::string> expected{"hello", "world", "abc"};
    EXPECT_EQ(diag.logs(), expected);
  }
}

TEST(Receiver, RepeatingMoveOnly) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  DiagnosticsExample diag{loop.dispatcher()};
  async_patterns::Function<void(std::unique_ptr<std::string>)> log = diag.StreamUniqueLogs();
  log(std::make_unique<std::string>("abc"));
  ASSERT_OK(loop.RunUntilIdle());
  std::vector<std::string> expected{"abc"};
  EXPECT_EQ(diag.logs(), expected);
}

TEST(Receiver, RepeatingLambda) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  DiagnosticsExample diag{loop.dispatcher()};
  async_patterns::Function<void(std::string)> log = diag.StreamLogsWithLambda();

  log(std::string{"hello"});
  EXPECT_EQ(diag.logs().size(), 0u);
  ASSERT_OK(loop.RunUntilIdle());
  std::vector<std::string> expected{"hello"};
  EXPECT_EQ(diag.logs(), expected);
}

TEST(Receiver, RepeatingLambdaMoveOnly) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  DiagnosticsExample diag{loop.dispatcher()};
  async_patterns::Function<void(std::unique_ptr<std::string>)> log =
      diag.StreamUniqueLogsWithLambda();

  log(std::make_unique<std::string>("hello"));
  EXPECT_EQ(diag.logs().size(), 0u);
  ASSERT_OK(loop.RunUntilIdle());
  std::vector<std::string> expected{"hello"};
  EXPECT_EQ(diag.logs(), expected);
}

TEST(Receiver, RepeatingConvertToFitFunction) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  DiagnosticsExample diag{loop.dispatcher()};
  async_patterns::Function<void(std::string)> function = diag.StreamLogs();
  fit::function<void(std::string)> fit_function = std::move(function).ignore_result();

  fit_function(std::string("abc"));
  EXPECT_EQ(diag.logs().size(), 0u);
  ASSERT_OK(loop.RunUntilIdle());
  std::vector<std::string> expected{"abc"};
  EXPECT_EQ(diag.logs(), expected);
}

TEST(Receiver, RepeatingToVoidPromise) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  DiagnosticsExample diag{loop.dispatcher()};
  async_patterns::Function<void(std::string)> function = diag.StreamLogs();

  // Fire-and-forget.
  {
    function(std::string("abc"));
    EXPECT_EQ(diag.logs().size(), 0u);
    ASSERT_OK(loop.RunUntilIdle());
    std::vector<std::string> expected{"abc"};
    EXPECT_EQ(diag.logs(), expected);
  }

  // Get a promise.
  {
    fpromise::promise<> promise = function(std::string("def")).promise();
    fpromise::single_threaded_executor executor;
    bool called = false;
    executor.schedule_task(promise.and_then([&called]() { called = true; }));
    ASSERT_OK(loop.RunUntilIdle());
    ASSERT_FALSE(called);
    executor.run();
    ASSERT_TRUE(called);
    std::vector<std::string> expected{"abc", "def"};
    EXPECT_EQ(diag.logs(), expected);
  }
}

TEST(Receiver, RepeatingToPromise) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  DiagnosticsExample diag{loop.dispatcher()};
  async_patterns::Function<size_t(std::string)> function = diag.StreamLogsWithCounter();

  // Fire-and-forget.
  {
    function(std::string("abc"));
    EXPECT_EQ(diag.logs().size(), 0u);
    ASSERT_OK(loop.RunUntilIdle());
    std::vector<std::string> expected{"abc"};
    EXPECT_EQ(diag.logs(), expected);
  }

  // Get a promise.
  {
    fpromise::promise<size_t> promise = function(std::string("def")).promise();
    fpromise::single_threaded_executor executor;
    bool called = false;
    executor.schedule_task(promise.and_then([&called](size_t& size) {
      called = true;
      ASSERT_EQ(size, 2u);
    }));
    ASSERT_OK(loop.RunUntilIdle());
    ASSERT_FALSE(called);
    executor.run();
    ASSERT_TRUE(called);
    std::vector<std::string> expected{"abc", "def"};
    EXPECT_EQ(diag.logs(), expected);
  }
}

TEST(Receiver, OnceManyArgs) {
  class Owner {
   public:
    explicit Owner(async_dispatcher_t* dispatcher) : receiver_{this, dispatcher} {}

    async_patterns::Callback<void(int, const std::string&, float)> Test() {
      return receiver_.Once(&Owner::OnTest);
    }

    int count() const { return count_; }

   private:
    void OnTest(int a, const std::string& b, float c) {
      EXPECT_EQ(a, 1);
      EXPECT_EQ(b, "2");
      EXPECT_EQ(c, 3.0);
      count_++;
    }

    async_patterns::Receiver<Owner> receiver_;
    int count_ = 0;
  };

  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  Owner owner{loop.dispatcher()};
  async_patterns::Callback f = owner.Test();
  EXPECT_EQ(owner.count(), 0);
  f(1, std::string{"2"}, 3.0);
  EXPECT_EQ(owner.count(), 0);
  ASSERT_OK(loop.RunUntilIdle());
  EXPECT_EQ(owner.count(), 1);

  ASSERT_DEATH(f(1, std::string{"2"}, 3.0), "");
}

TEST(Receiver, RepeatingManyArgs) {
  class Owner {
   public:
    explicit Owner(async_dispatcher_t* dispatcher) : receiver_{this, dispatcher} {}

    async_patterns::Function<void(int, const std::string&, float)> Test() {
      return receiver_.Repeating(&Owner::OnTest);
    }

    int count() const { return count_; }

   private:
    void OnTest(int a, const std::string& b, float c) {
      EXPECT_EQ(a, 1);
      EXPECT_EQ(b, "2");
      EXPECT_EQ(c, 3.0);
      count_++;
    }

    async_patterns::Receiver<Owner> receiver_;
    int count_ = 0;
  };

  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  Owner owner{loop.dispatcher()};
  async_patterns::Function f = owner.Test();
  EXPECT_EQ(owner.count(), 0);
  f(1, std::string{"2"}, 3.0);
  EXPECT_EQ(owner.count(), 0);
  ASSERT_OK(loop.RunUntilIdle());
  EXPECT_EQ(owner.count(), 1);

  f(1, std::string{"2"}, 3.0);
  ASSERT_OK(loop.RunUntilIdle());
  EXPECT_EQ(owner.count(), 2);
}

TEST(Receiver, OrderingAcrossCallbacksAndFunctions) {
  class Owner {
   public:
    explicit Owner(async_dispatcher_t* dispatcher) : receiver_{this, dispatcher} {}

    async_patterns::Function<void(const std::string&)> Method1() {
      return receiver_.Repeating(&Owner::LogMethodCall);
    }
    async_patterns::Callback<void(const std::string&)> Method2() {
      return receiver_.Once(&Owner::LogMethodCall);
    }
    async_patterns::Function<void(const std::string&)> Method3() {
      return receiver_.Repeating(&Owner::LogMethodCall);
    }

    const std::vector<std::string>& logs() const { return logs_; }

   private:
    void LogMethodCall(const std::string& b) { logs_.push_back(b); }

    async_patterns::Receiver<Owner> receiver_;
    std::vector<std::string> logs_;
  };

  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  Owner owner{loop.dispatcher()};

  async_patterns::Function method_1 = owner.Method1();
  async_patterns::Callback method_2 = owner.Method2();
  async_patterns::Function method_3 = owner.Method3();

  method_3("1");
  method_1("2");
  method_1("3");
  method_3("4");
  method_1("5");
  method_2("6");
  method_3("7");

  ASSERT_OK(loop.RunUntilIdle());
  std::vector<std::string> expected{"1", "2", "3", "4", "5", "6", "7"};
  EXPECT_EQ(owner.logs(), expected);
}

}  // namespace
