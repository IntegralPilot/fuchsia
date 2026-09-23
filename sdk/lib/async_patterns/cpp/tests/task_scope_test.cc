// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-testing/test_loop.h>
#include <lib/async/cpp/time.h>
#include <lib/async/task.h>
#include <lib/async_patterns/cpp/task_scope.h>
#include <lib/fit/defer.h>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

TEST(TaskScope, Construction) {
  async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};
  async_patterns::TaskScope tasks{loop.dispatcher()};
}

TEST(TaskScope, Post) {
  async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};
  async_patterns::TaskScope tasks{loop.dispatcher()};

  bool ok = false;
  tasks.Post([&] {
    EXPECT_FALSE(ok);
    ok = true;
  });

  EXPECT_FALSE(ok);
  ASSERT_OK(loop.RunUntilIdle());
  EXPECT_TRUE(ok);
}

TEST(TaskScope, PostCanceledOnDestruction) {
  async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};

  enum Action : uint8_t {
    TASK_SCOPE_DESTROYED = 1,
    TASK_DESTROYED,
    TASK_RUN,
  };
  std::vector<Action> actions;
  {
    auto task_scope_destroyed = fit::defer([&] { actions.push_back(TASK_SCOPE_DESTROYED); });
    async_patterns::TaskScope tasks{loop.dispatcher()};
    auto task_destroyed = fit::defer([&] { actions.push_back(TASK_DESTROYED); });
    tasks.Post([&, _ = std::move(task_destroyed)] { actions.push_back(TASK_RUN); });
  }

  std::vector<Action> expectation = {
      TASK_DESTROYED,
      TASK_SCOPE_DESTROYED,
  };
  EXPECT_EQ(actions, expectation);
  ASSERT_OK(loop.RunUntilIdle());
  EXPECT_EQ(actions, expectation);
}

TEST(TaskScope, PostCanceledOnDestructionTaskDestructorPostTask) {
  async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};

  enum Action : uint8_t {
    TASK_SCOPE_DESTROYED = 1,
    FIRST_TASK_DESTROYED,
    FIRST_TASK_RUN,
    REENTRANT_TASK_DESTROYED,
    REENTRANT_TASK_RUN,
  };
  std::vector<Action> actions;
  {
    auto task_scope_destroyed = fit::defer([&] { actions.push_back(TASK_SCOPE_DESTROYED); });
    async_patterns::TaskScope tasks{loop.dispatcher()};
    auto deferred = fit::defer([&] {
      actions.push_back(FIRST_TASK_DESTROYED);
      auto deferred = fit::defer([&] { actions.push_back(REENTRANT_TASK_DESTROYED); });
      tasks.Post([&, _ = std::move(deferred)] { actions.push_back(REENTRANT_TASK_RUN); });
    });
    tasks.Post([&, _ = std::move(deferred)] { actions.push_back(FIRST_TASK_RUN); });
    EXPECT_EQ(actions.size(), 0u);
  }

  std::vector<Action> expectation = {
      FIRST_TASK_DESTROYED,
      REENTRANT_TASK_DESTROYED,
      TASK_SCOPE_DESTROYED,
  };
  EXPECT_EQ(actions, expectation);
  ASSERT_OK(loop.RunUntilIdle());
  EXPECT_EQ(actions, expectation);
}

TEST(TaskScope, PostCanceledOnShutdown) {
  async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};
  async_patterns::TaskScope tasks{loop.dispatcher()};

  bool ok = false;
  bool destroyed = false;
  auto deferred = fit::defer([&] { destroyed = true; });
  tasks.Post([&, _ = std::move(deferred)] { ok = true; });

  EXPECT_FALSE(ok);
  EXPECT_FALSE(destroyed);
  loop.Shutdown();
  EXPECT_FALSE(ok);
  EXPECT_TRUE(destroyed);
}

TEST(TaskScope, PostCanceledOnShutdownTaskDestructorPostTask) {
  async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};

  enum Action : uint8_t {
    FIRST_TASK_DESTROYED = 1,
    FIRST_TASK_RUN,
    REENTRANT_TASK_DESTROYED,
    REENTRANT_TASK_RUN,
  };
  std::vector<Action> actions;
  async_patterns::TaskScope tasks{loop.dispatcher()};
  auto deferred = fit::defer([&] {
    actions.push_back(FIRST_TASK_DESTROYED);
    auto deferred = fit::defer([&] { actions.push_back(REENTRANT_TASK_DESTROYED); });
    tasks.Post([&, _ = std::move(deferred)] { actions.push_back(REENTRANT_TASK_RUN); });
  });
  tasks.Post([&, _ = std::move(deferred)] { actions.push_back(FIRST_TASK_RUN); });
  EXPECT_EQ(actions.size(), 0u);

  loop.Shutdown();

  std::vector<Action> expectation = {
      FIRST_TASK_DESTROYED,
      REENTRANT_TASK_DESTROYED,
  };
  EXPECT_EQ(actions, expectation);
}

TEST(TaskScope, PostDelayed) {
  async::TestLoop loop;
  async_patterns::TaskScope tasks{loop.dispatcher()};

  bool ok = false;
  tasks.PostDelayed(
      [&] {
        EXPECT_FALSE(ok);
        ok = true;
      },
      zx::sec(2));

  loop.RunFor(zx::sec(1));
  EXPECT_FALSE(ok);

  loop.RunFor(zx::sec(1));
  EXPECT_TRUE(ok);
}

TEST(TaskScope, PostDelayedCanceledOnDestruction) {
  async::TestLoop loop;

  enum Action : uint8_t {
    TASK_SCOPE_DESTROYED = 1,
    TASK_DESTROYED,
    TASK_RUN,
  };
  std::vector<Action> actions;
  {
    auto task_scope_destroyed = fit::defer([&] { actions.push_back(TASK_SCOPE_DESTROYED); });
    async_patterns::TaskScope tasks{loop.dispatcher()};

    auto task_destroyed = fit::defer([&] { actions.push_back(TASK_DESTROYED); });
    tasks.PostDelayed([&, _ = std::move(task_destroyed)] { actions.push_back(TASK_RUN); },
                      zx::sec(1));
  }

  std::vector<Action> expectation = {
      TASK_DESTROYED,
      TASK_SCOPE_DESTROYED,
  };
  EXPECT_EQ(actions, expectation);
  loop.RunFor(zx::sec(3));
  EXPECT_EQ(actions, expectation);
}

TEST(TaskScope, PostDelayedCanceledOnDestructionTaskDestructorPostTask) {
  async::TestLoop loop;

  enum Action : uint8_t {
    TASK_SCOPE_DESTROYED = 1,
    FIRST_TASK_DESTROYED,
    FIRST_TASK_RUN,
    REENTRANT_TASK_DESTROYED,
    REENTRANT_TASK_RUN,
  };
  std::vector<Action> actions;
  {
    auto task_scope_destroyed = fit::defer([&] { actions.push_back(TASK_SCOPE_DESTROYED); });
    async_patterns::TaskScope tasks{loop.dispatcher()};
    auto deferred = fit::defer([&] {
      actions.push_back(FIRST_TASK_DESTROYED);
      auto deferred = fit::defer([&] { actions.push_back(REENTRANT_TASK_DESTROYED); });
      tasks.PostDelayed([&, _ = std::move(deferred)] { actions.push_back(REENTRANT_TASK_RUN); },
                        zx::sec(1));
    });
    tasks.PostDelayed([&, _ = std::move(deferred)] { actions.push_back(FIRST_TASK_RUN); },
                      zx::sec(1));
    EXPECT_EQ(actions.size(), 0u);
  }

  std::vector<Action> expectation = {
      FIRST_TASK_DESTROYED,
      REENTRANT_TASK_DESTROYED,
      TASK_SCOPE_DESTROYED,
  };
  EXPECT_EQ(actions, expectation);
  loop.RunFor(zx::sec(10));
  EXPECT_EQ(actions, expectation);
}

TEST(TaskScope, PostDelayedCanceledOnShutdown) {
  async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};
  async_patterns::TaskScope tasks{loop.dispatcher()};

  bool ok = false;
  bool destroyed = false;
  auto deferred = fit::defer([&] { destroyed = true; });
  tasks.PostDelayed([&, _ = std::move(deferred)] { ok = true; }, zx::sec(1));
  EXPECT_FALSE(ok);
  EXPECT_FALSE(destroyed);

  loop.Shutdown();
  EXPECT_FALSE(ok);
  EXPECT_TRUE(destroyed);
}

TEST(TaskScope, PostDelayedCanceledOnShutdownTaskDestructorPostTask) {
  async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};

  enum Action : uint8_t {
    FIRST_TASK_DESTROYED = 1,
    FIRST_TASK_RUN,
    REENTRANT_TASK_DESTROYED,
    REENTRANT_TASK_RUN,
  };
  std::vector<Action> actions;
  async_patterns::TaskScope tasks{loop.dispatcher()};
  auto deferred = fit::defer([&] {
    actions.push_back(FIRST_TASK_DESTROYED);
    auto deferred = fit::defer([&] { actions.push_back(REENTRANT_TASK_DESTROYED); });
    tasks.PostDelayed([&, _ = std::move(deferred)] { actions.push_back(REENTRANT_TASK_RUN); },
                      zx::sec(1));
  });
  tasks.PostDelayed([&, _ = std::move(deferred)] { actions.push_back(FIRST_TASK_RUN); },
                    zx::sec(1));
  EXPECT_EQ(actions.size(), 0u);

  loop.Shutdown();

  std::vector<Action> expectation = {
      FIRST_TASK_DESTROYED,
      REENTRANT_TASK_DESTROYED,
  };
  EXPECT_EQ(actions, expectation);
}

TEST(TaskScope, PostForTime) {
  async::TestLoop loop;
  async_patterns::TaskScope tasks{loop.dispatcher()};

  bool ok = false;
  tasks.PostForTime(
      [&] {
        EXPECT_FALSE(ok);
        ok = true;
      },
      async::Now(loop.dispatcher()) + zx::sec(2));

  loop.RunFor(zx::sec(1));
  EXPECT_FALSE(ok);

  loop.RunFor(zx::sec(1));
  EXPECT_TRUE(ok);
}

TEST(TaskScope, PostForTimeCanceledOnDestruction) {
  async::TestLoop loop;

  enum Action : uint8_t {
    TASK_SCOPE_DESTROYED = 1,
    TASK_DESTROYED,
    TASK_RUN,
  };
  std::vector<Action> actions;
  {
    auto task_scope_destroyed = fit::defer([&] { actions.push_back(TASK_SCOPE_DESTROYED); });
    async_patterns::TaskScope tasks{loop.dispatcher()};

    auto deferred = fit::defer([&] { actions.push_back(TASK_DESTROYED); });
    tasks.PostForTime([&, _ = std::move(deferred)] { actions.push_back(TASK_DESTROYED); },
                      async::Now(loop.dispatcher()) + zx::sec(1));
  }

  std::vector<Action> expectation = {
      TASK_DESTROYED,
      TASK_SCOPE_DESTROYED,
  };
  EXPECT_EQ(actions, expectation);
  loop.RunFor(zx::sec(3));
  EXPECT_EQ(actions, expectation);
}

TEST(TaskScope, PostForTimeCanceledOnDestructionTaskDestructorPostTask) {
  async::TestLoop loop;

  enum Action : uint8_t {
    TASK_SCOPE_DESTROYED = 1,
    FIRST_TASK_DESTROYED,
    FIRST_TASK_RUN,
    REENTRANT_TASK_DESTROYED,
    REENTRANT_TASK_RUN,
  };
  std::vector<Action> actions;
  {
    auto task_scope_destroyed = fit::defer([&] { actions.push_back(TASK_SCOPE_DESTROYED); });
    async_patterns::TaskScope tasks{loop.dispatcher()};
    auto deferred = fit::defer([&] {
      actions.push_back(FIRST_TASK_DESTROYED);
      auto deferred = fit::defer([&] { actions.push_back(REENTRANT_TASK_DESTROYED); });
      tasks.PostForTime([&, _ = std::move(deferred)] { actions.push_back(REENTRANT_TASK_RUN); },
                        async::Now(loop.dispatcher()) + zx::sec(1));
    });
    tasks.PostForTime([&, _ = std::move(deferred)] { actions.push_back(FIRST_TASK_RUN); },
                      async::Now(loop.dispatcher()) + zx::sec(1));
    EXPECT_EQ(actions.size(), 0u);
  }

  std::vector<Action> expectation = {
      FIRST_TASK_DESTROYED,
      REENTRANT_TASK_DESTROYED,
      TASK_SCOPE_DESTROYED,
  };
  EXPECT_EQ(actions, expectation);
  loop.RunFor(zx::sec(10));
  EXPECT_EQ(actions, expectation);
}

TEST(TaskScope, PostForTimeCanceledOnShutdown) {
  async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};
  async_patterns::TaskScope tasks{loop.dispatcher()};

  bool ok = false;
  bool destroyed = false;
  auto deferred = fit::defer([&] { destroyed = true; });
  tasks.PostForTime([&, _ = std::move(deferred)] { ok = true; },
                    async::Now(loop.dispatcher()) + zx::sec(1));
  EXPECT_FALSE(ok);
  EXPECT_FALSE(destroyed);

  loop.Shutdown();

  EXPECT_FALSE(ok);
  EXPECT_TRUE(destroyed);
}

TEST(TaskScope, PostForTimeCanceledOnShutdownTaskDestructorPostTask) {
  async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};

  enum Action : uint8_t {
    FIRST_TASK_DESTROYED = 1,
    FIRST_TASK_RUN,
    REENTRANT_TASK_DESTROYED,
    REENTRANT_TASK_RUN,
  };
  std::vector<Action> actions;
  async_patterns::TaskScope tasks{loop.dispatcher()};
  auto deferred = fit::defer([&] {
    actions.push_back(FIRST_TASK_DESTROYED);
    auto deferred = fit::defer([&] { actions.push_back(REENTRANT_TASK_DESTROYED); });
    tasks.PostForTime([&, _ = std::move(deferred)] { actions.push_back(REENTRANT_TASK_RUN); },
                      async::Now(loop.dispatcher()) + zx::sec(1));
  });
  tasks.PostForTime([&, deferred = std::move(deferred)] { actions.push_back(FIRST_TASK_RUN); },
                    async::Now(loop.dispatcher()) + zx::sec(1));
  EXPECT_EQ(actions.size(), 0u);

  loop.Shutdown();

  std::vector<Action> expectation = {
      FIRST_TASK_DESTROYED,
      REENTRANT_TASK_DESTROYED,
  };
  EXPECT_EQ(actions, expectation);
}

TEST(TaskScope, Mixture) {
  async::TestLoop loop;
  async_patterns::TaskScope tasks{loop.dispatcher()};

  int count = 0;
  tasks.Post([&] { count++; });
  tasks.PostDelayed([&] { count++; }, zx::sec(1));
  tasks.PostForTime([&] { count++; }, async::Now(loop.dispatcher()) + zx::sec(1));
  tasks.Post([&] { count++; });

  EXPECT_EQ(count, 0);
  loop.RunFor(zx::sec(1));
  EXPECT_EQ(count, 4);
}

TEST(TaskScope, CheckSynchronization) {
  async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};
  async_patterns::TaskScope tasks{loop.dispatcher()};

  std::thread([&] {
    ASSERT_DEATH(tasks.Post([] {}), "\\|async_patterns::TaskQueue\\| is thread-unsafe");
  }).join();
  std::thread([&] {
    ASSERT_DEATH(tasks.PostDelayed([] {}, zx::sec(1)),
                 "\\|async_patterns::TaskQueue\\| is thread-unsafe");
  }).join();
  std::thread([&] {
    ASSERT_DEATH(tasks.PostForTime([] {}, zx::time::infinite()),
                 "\\|async_patterns::TaskQueue\\| is thread-unsafe");
  }).join();
}
