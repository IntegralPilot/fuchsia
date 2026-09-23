// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fidl.cpp.wire.bindinggroup.test/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/fidl/cpp/wire/server.h>
#include <zircon/fidl.h>
#include <zircon/syscalls.h>

#include <thread>
#include <vector>

#include <zxtest/zxtest.h>

namespace {

using ::fidl_cpp_wire_bindinggroup_test::Testable;
constexpr zx_status_t kTestEpitaph = 1234;
constexpr char kSimpleEcho[] = "test";

// These are the default values for the number of impls and number of bindings per impl when
// testing. Some test running functions may use template parameters to override these as needed.
const size_t kTestNumImpls = 2;
const size_t kTestNumBindingsPerImpl = 2;

struct TestImpl : fidl::Server<Testable> {
 public:
  explicit TestImpl(async::Loop* loop) : loop(loop) {}

  void Echo(EchoRequest& request, EchoCompleter::Sync& completer) override {
    echo_count_++;
    completer.Reply(request.str());
  }

  // Always abruptly close the connection. This is not a good implementation to copy - its just
  // useful to check that close handling works properly.
  void Terminate(TerminateCompleter::Sync& completer) override {
    terminate_count_++;
    completer.Close(kTestEpitaph);
  }

  // Fired whenever the binding is closed via a |Close*| call on its parent |ServerBindingGroup|.
  void close_handler_fired() { close_count_++; }

  size_t get_echo_count() const { return echo_count_; }

  size_t get_close_count() const { return close_count_; }

  size_t get_terminate_count() const { return terminate_count_; }

  async::Loop* loop = nullptr;

 private:
  size_t close_count_ = 0;
  size_t echo_count_ = 0;
  size_t terminate_count_ = 0;
};

constexpr auto kCloseHandler = [](TestImpl* impl, fidl::UnbindInfo info) {
  impl->close_handler_fired();
  EXPECT_TRUE(info.did_send_epitaph());
  EXPECT_EQ(fidl::Reason::kClose, info.reason());
  EXPECT_OK(info.status());
};

TEST(BindingGroup, Trivial) { fidl::ServerBindingGroup<Testable> group; }

// Tests simple patterns for adding various numbers of bindings for various numbers of
// implementations. Additionally, this test template tests that the |size| and |ForEachBinding|
// methods work as expected.
template <size_t NumImpls = kTestNumImpls, size_t NumBindingsPerImpl = kTestNumBindingsPerImpl>
void AddBindingTest() {
  constexpr size_t TotalServerBindings = NumImpls * NumBindingsPerImpl;
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);

  // Because we're going to be using raw pointers into these vectors for the test, its important to
  // pre-allocate, so that the underlying storage doesn't move.
  std::vector<TestImpl> impls;
  std::vector<fidl::WireClient<Testable>> clients;
  impls.reserve(NumImpls);
  clients.reserve(TotalServerBindings);

  // Data we are tracking for the duration of the test which we will assert against.
  std::map<const TestImpl*, size_t> unvisited_bindings_per_impl;

  fidl::ServerBindingGroup<Testable> group;

  // Create the right number of bindings for each impl as requested. Hold on to the client so that
  // we may poke at it later.
  for (size_t i = 0; i < NumImpls; i++) {
    impls.emplace_back(&loop);
    for (size_t j = i * NumBindingsPerImpl; j < (i + 1) * NumBindingsPerImpl; j++) {
      auto endpoints = fidl::Endpoints<Testable>::Create();
      group.AddBinding(loop.dispatcher(), std::move(endpoints.server), &impls.back(),
                       kCloseHandler);
      clients.emplace_back(std::move(endpoints.client), loop.dispatcher());
    }
    unvisited_bindings_per_impl.insert({&impls.back(), NumBindingsPerImpl});
  }
  EXPECT_EQ(group.size(), TotalServerBindings);
  EXPECT_EQ(clients.size(), TotalServerBindings);
  EXPECT_EQ(unvisited_bindings_per_impl.size(), NumImpls);

  // Make an |Echo| call on each |client| to ensure that its binding is actually responsive.
  for (auto& client : clients) {
    client->Echo(kSimpleEcho).ThenExactlyOnce([&](fidl::WireUnownedResult<Testable::Echo>& result) {
      ASSERT_TRUE(result.ok());
      EXPECT_OK(result.status());
      EXPECT_EQ(result.value().str.get(), kSimpleEcho);
    });

    // Run the loop until the callback is resolved.
    loop.RunUntilIdle();
  }

  // Ensure that each |impl| was called the number of times that we expect.
  for (const auto& impl : impls) {
    EXPECT_EQ(impl.get_echo_count(), NumBindingsPerImpl);
  }

  // Visit each binding, matching its implementation to one of the ones we're storing in the |impls|
  // array. Decrement the count in that array on each visit.
  size_t bindings_visited = 0;
  group.ForEachBinding([&](const fidl::ServerBinding<Testable>& binding) {
    bool matched = false;
    bindings_visited++;
    binding.AsImpl<TestImpl>([&](const TestImpl* binding_impl) {
      auto matched_impl = unvisited_bindings_per_impl.find(binding_impl);
      EXPECT_NE(matched_impl, unvisited_bindings_per_impl.end());
      matched = true;
      matched_impl->second--;
    });

    EXPECT_TRUE(matched);
  });

  // Because the previous loop decremented the count for each impl visited, we can iterate over the
  // |unvisited_bindings_per_impl| counters to ensure that they are all zero, confirming that every
  // |impl| has been visited the appropriate number of times.
  EXPECT_EQ(bindings_visited, TotalServerBindings);
  for (const auto& impl : unvisited_bindings_per_impl) {
    EXPECT_EQ(impl.second, 0);
  }
}

TEST(BindingGroup, AddBindingOneImplWithOneBinding) { AddBindingTest<1, 1>(); }

TEST(BindingGroup, AddBindingOneImplWithManyBindings) { AddBindingTest<1, 2>(); }

TEST(BindingGroup, AddBindingManyImplsWithOneBindingEach) { AddBindingTest<2, 1>(); }

TEST(BindingGroup, AddBindingManyImplsWithManyBindingsEach) { AddBindingTest<2, 2>(); }

// Tests adding bindings using the generator produced by the |CreateHandler| method.
template <size_t NumImpls = kTestNumImpls, size_t NumBindingsPerImpl = kTestNumBindingsPerImpl>
void CreateHandlerTest() {
  constexpr size_t TotalServerBindings = NumImpls * NumBindingsPerImpl;
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);

  // Because we're going to be using raw pointers into these vectors for the test, its important to
  // pre-allocate, so that the underlying storage doesn't move.
  std::vector<TestImpl> impls;
  std::vector<fidl::WireClient<Testable>> clients;
  impls.reserve(NumImpls);
  clients.reserve(TotalServerBindings);

  fidl::ServerBindingGroup<Testable> group;

  // Create the right number of bindings for each impl as requested. Hold on to the client so that
  // we may poke at it later.
  for (size_t i = 0; i < NumImpls; i++) {
    impls.emplace_back(&loop);
    auto handler = group.CreateHandler(&impls.back(), loop.dispatcher(), kCloseHandler);
    for (size_t j = i * NumBindingsPerImpl; j < (i + 1) * NumBindingsPerImpl; j++) {
      auto endpoints = fidl::Endpoints<Testable>::Create();
      handler(std::move(endpoints.server));
      clients.emplace_back(std::move(endpoints.client), loop.dispatcher());
    }
  }
  EXPECT_EQ(group.size(), TotalServerBindings);
  EXPECT_EQ(clients.size(), TotalServerBindings);

  // Make an |Echo| call on each |client| to ensure that its binding is actually responsive.
  for (auto& client : clients) {
    client->Echo(kSimpleEcho).ThenExactlyOnce([&](fidl::WireUnownedResult<Testable::Echo>& result) {
      ASSERT_TRUE(result.ok());
      EXPECT_OK(result.status());
      EXPECT_EQ(result.value().str.get(), kSimpleEcho);
    });

    // Run the loop until the callback is resolved.
    loop.RunUntilIdle();
  }

  // Ensure that each |impl| was called the number of times that we expect.
  for (const auto& impl : impls) {
    EXPECT_EQ(impl.get_echo_count(), NumBindingsPerImpl);
  }
}

TEST(BindingGroup, CreateHandlerOneImplWithOneBinding) { CreateHandlerTest<1, 1>(); }

TEST(BindingGroup, CreateHandlerOneImplWithManyBindings) { CreateHandlerTest<1, 2>(); }

TEST(BindingGroup, CreateHandlerManyImplsWithOneBindingEach) { CreateHandlerTest<2, 1>(); }

TEST(BindingGroup, CreateHandlerManyImplsWithManyBindingsEach) { CreateHandlerTest<2, 2>(); }

// Tests that |CloseHandler| functions are correctly passed to, and fired by, bindings in the group.
template <size_t NumImpls = kTestNumImpls, size_t NumBindingsPerImpl = kTestNumBindingsPerImpl>
void CloseHandlerTest() {
  constexpr size_t TotalServerBindings = NumImpls * NumBindingsPerImpl;
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);

  // Because we're going to be using raw pointers into these vectors for the test, its important to
  // pre-allocate, so that the underlying storage doesn't move.
  std::vector<TestImpl> impls;
  std::vector<fidl::WireClient<Testable>> clients;
  impls.reserve(NumImpls);
  clients.reserve(TotalServerBindings);

  // Data we are tracking for the duration of the test which we will assert against.
  size_t empty_set_handler_call_count = 0;

  fidl::ServerBindingGroup<Testable> group;

  // Add an |empty_handler| lambda to the |group|, and ensure that it only gets called once.
  group.set_empty_set_handler([&]() { empty_set_handler_call_count++; });

  // Create the right number of bindings for each impl as requested. Hold on to the client so that
  // we may poke at it later.
  for (size_t i = 0; i < NumImpls; i++) {
    impls.emplace_back(&loop);
    for (size_t j = i * NumBindingsPerImpl; j < (i + 1) * NumBindingsPerImpl; j++) {
      auto endpoints = fidl::Endpoints<Testable>::Create();
      group.AddBinding(loop.dispatcher(), std::move(endpoints.server), &impls.back(),
                       kCloseHandler);
      clients.emplace_back(std::move(endpoints.client), loop.dispatcher());
    }
  }
  EXPECT_EQ(group.size(), TotalServerBindings);
  EXPECT_EQ(clients.size(), TotalServerBindings);

  // Make a |Terminate| call on each |client| to ensure that it is abruptly torn down.
  for (auto& client : clients) {
    EXPECT_EQ(empty_set_handler_call_count, 0);

    auto result = client->Terminate();
    EXPECT_TRUE(result.ok());

    // Run the loop until the close handlers are resolved.
    loop.RunUntilIdle();
  }

  // Ensure that each |impl| was closed the number of times that we expect. In this case, that means
  // that every closure came from a |Terminate| method call on the client, and that every binding
  // was closed in this manner.
  for (const auto& impl : impls) {
    EXPECT_EQ(impl.get_terminate_count(), impl.get_close_count());
    EXPECT_EQ(impl.get_terminate_count(), NumBindingsPerImpl);
  }

  // Ensure that empty handler was only called once, after the last binding resolved its |Terminate|
  // handler.
  EXPECT_EQ(empty_set_handler_call_count, 1);
}

TEST(BindingGroup, CloseHandlerOneImplWithOneBinding) { CloseHandlerTest<1, 1>(); }

TEST(BindingGroup, CloseHandlerOneImplWithManyBindings) { CloseHandlerTest<1, 2>(); }

TEST(BindingGroup, CloseHandlerManyImplsWithOneBindingEach) { CloseHandlerTest<2, 1>(); }

TEST(BindingGroup, CloseHandlerManyImplsWithManyBindingsEach) { CloseHandlerTest<2, 2>(); }

using KillSomeBindings = fit::function<void(fidl::ServerBindingGroup<Testable>& group,
                                            async::Loop* loop, std::vector<TestImpl>& impls,
                                            std::map<size_t, const TestImpl*>& open_bindings)>;

// Tests that calling methods in the |Close*()| and |Remove*()| families works as expected. The
// |KillSomeBindings| lambda is used to |Remove*| or |Close*| some number of bindings as the
// specific test requires.
void ExternalKillBindingTest(KillSomeBindings kill_some_bindings) {
  constexpr size_t TotalServerBindings = kTestNumImpls * kTestNumBindingsPerImpl;
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);

  class EventHandler : public fidl::WireAsyncEventHandler<Testable> {
   public:
    void on_fidl_error(fidl::UnbindInfo info) override {
      ASSERT_TRUE(info.is_peer_closed());
      closed_ = true;
    }

    bool closed() const { return closed_; }

   private:
    bool closed_ = false;
  };

  // Because we're going to be using raw pointers into these vectors for the test, its important to
  // pre-allocate, so that the underlying storage doesn't move.
  std::vector<TestImpl> impls;
  std::vector<fidl::WireClient<Testable>> clients;
  std::vector<EventHandler> event_handlers;
  impls.reserve(kTestNumImpls);
  clients.reserve(TotalServerBindings);
  event_handlers.reserve(TotalServerBindings);

  // Data we are tracking for the duration of the test which we will assert against.  The
  // |kill_some_bindings| handler should replace all entries in |open_bindings| with |nullptr| when
  // killing them.
  std::map<size_t, const TestImpl*> open_bindings;
  size_t empty_set_handler_call_count = 0;

  // Create the |group| under test, and define a counter-decrementing |close_handler| to attach to
  // each binding it holds.
  fidl::ServerBindingGroup<Testable> group;

  // Add an |empty_handler| lambda to the |group|, incrementing a simple counter each time it gets
  // called.
  group.set_empty_set_handler([&]() { empty_set_handler_call_count++; });

  // Create the right number of bindings for each impl as requested. Hold on to the client so that
  // we may poke at it later.
  for (size_t i = 0; i < kTestNumImpls; i++) {
    impls.emplace_back(&loop);
    for (size_t j = i * kTestNumBindingsPerImpl; j < (i + 1) * kTestNumBindingsPerImpl; j++) {
      auto endpoints = fidl::Endpoints<Testable>::Create();

      group.AddBinding(loop.dispatcher(), std::move(endpoints.server), &impls.back(),
                       kCloseHandler);
      event_handlers.emplace_back();
      clients.emplace_back(std::move(endpoints.client), loop.dispatcher(), &event_handlers.back());
      open_bindings.insert({j, &impls.back()});
    }
  }
  EXPECT_EQ(group.size(), TotalServerBindings);
  EXPECT_EQ(clients.size(), TotalServerBindings);

  // Call the |kill_some_bindings| lambda to kill the servers that the test requires.
  kill_some_bindings(group, &loop, impls, open_bindings);
  loop.RunUntilIdle();

  // Make a |Terminate| call on each remaining |client| to ensure that it is abruptly torn down.
  for (const auto& open_binding : open_bindings) {
    // If a |open_binding| is set to |nullptr|, that means the test body
    // explicitly removed/closed the binding, and we verify this has happened.
    // Otherwise, call |Terminate| to tell the server to close it.
    if (open_binding.second != nullptr) {
      EXPECT_FALSE(event_handlers[open_binding.first].closed());
      auto result = clients[open_binding.first]->Terminate();
      EXPECT_OK(result.status());
    } else {
      EXPECT_TRUE(event_handlers[open_binding.first].closed());
    }

    // Run the loop until the close handlers are resolved.
    loop.RunUntilIdle();
  }

  // All connections should now be closed.
  for (const auto& open_binding : open_bindings) {
    EXPECT_TRUE(event_handlers[open_binding.first].closed());
  }

  // Ensure that empty handler was only called once, after the last binding resolved its |Terminate|
  // handler.
  EXPECT_EQ(empty_set_handler_call_count, 1);
}

TEST(BindingGroup, RemoveBindings) {
  ExternalKillBindingTest([](fidl::ServerBindingGroup<Testable>& group, async::Loop* loop,
                             std::vector<TestImpl>& impls,
                             std::map<size_t, const TestImpl*>& open_bindings) {
    const TestImpl* target_impl = &impls[0];
    EXPECT_EQ(group.RemoveBindings(target_impl), true);
    EXPECT_EQ(group.RemoveBindings(target_impl), false);
    EXPECT_EQ(group.size(), 2);

    loop->RunUntilIdle();

    // Ensure that no close counters were incremented, since this was merely a removal.
    for (auto& impl : open_bindings) {
      EXPECT_EQ(impl.second->get_close_count(), 0);
    }

    // Mark the removed bindings as killed, so that the rest of the test knows which bindings
    // should and should not be closed via the client making a |Terminate| call.
    for (const auto& open_binding : open_bindings) {
      if (open_binding.second == target_impl) {
        open_bindings[open_binding.first] = nullptr;
      }
    }
  });
}

TEST(BindingGroup, RemoveAll) {
  ExternalKillBindingTest([](fidl::ServerBindingGroup<Testable>& group, async::Loop* loop,
                             std::vector<TestImpl>& impls,
                             std::map<size_t, const TestImpl*>& open_bindings) {
    EXPECT_EQ(group.RemoveAll(), true);
    EXPECT_EQ(group.RemoveAll(), false);
    EXPECT_EQ(group.size(), 0);

    loop->RunUntilIdle();

    // Ensure that no close counters were incremented, since this was merely a removal.
    for (auto& impl : open_bindings) {
      EXPECT_EQ(impl.second->get_close_count(), 0);
    }

    // Mark the removed bindings as killed, so that the rest of the test knows which bindings
    // should and should not be closed via the client making a |Terminate| call.
    for (const auto& open_binding : open_bindings) {
      open_bindings[open_binding.first] = nullptr;
    }
  });
}

TEST(BindingGroup, CloseBindings) {
  ExternalKillBindingTest([](fidl::ServerBindingGroup<Testable>& group, async::Loop* loop,
                             std::vector<TestImpl>& impls,
                             std::map<size_t, const TestImpl*>& open_bindings) {
    const TestImpl* target_impl = &impls[0];
    EXPECT_EQ(group.CloseBindings(target_impl, kTestEpitaph), true);
    EXPECT_EQ(group.CloseBindings(target_impl, kTestEpitaph), false);
    EXPECT_EQ(group.size(), 2);

    // Run the loop until the close handlers are resolved.
    loop->RunUntilIdle();

    // Ensure that the close handler was fired for the closed binding's |impl| the correct
    // number of times.
    EXPECT_EQ(target_impl->get_close_count(), 2);
    for (const auto& impl : impls) {
      if (&impl != target_impl) {
        EXPECT_EQ(impl.get_close_count(), 0);
      }
    }

    // Mark the closed binding as killed, so that the rest of the test knows which bindings
    // should and should not be closed via the client making a |Terminate| call.
    for (const auto& open_binding : open_bindings) {
      if (open_binding.second == target_impl) {
        open_bindings[open_binding.first] = nullptr;
      }
    }
  });
}

TEST(BindingGroup, CloseAll) {
  ExternalKillBindingTest([](fidl::ServerBindingGroup<Testable>& group, async::Loop* loop,
                             std::vector<TestImpl>& impls,
                             std::map<size_t, const TestImpl*>& open_bindings) {
    EXPECT_EQ(group.CloseAll(kTestEpitaph), true);
    EXPECT_EQ(group.CloseAll(kTestEpitaph), false);
    EXPECT_EQ(group.size(), 0);

    // Run the loop until the close handlers are resolved.
    loop->RunUntilIdle();

    // Ensure that the close handler was fired for the closed bindings' |impl|s the correct
    // number of times.
    for (const auto& impl : impls) {
      EXPECT_EQ(impl.get_close_count(), 2);
    }

    // Mark the closed bindings as killed, so that the rest of the test knows which bindings
    // should and should not be closed via the client making a |Terminate| call.
    for (const auto& open_binding : open_bindings) {
      open_bindings[open_binding.first] = nullptr;
    }
  });
}

TEST(BindingGroup, CannotRemoveAfterClose) {
  ExternalKillBindingTest([](fidl::ServerBindingGroup<Testable>& group, async::Loop* loop,
                             std::vector<TestImpl>& impls,
                             std::map<size_t, const TestImpl*>& open_bindings) {
    const TestImpl* target_impl = &impls[1];
    EXPECT_EQ(group.CloseBindings(target_impl, kTestEpitaph), true);
    EXPECT_EQ(group.RemoveBindings(target_impl), false);
    EXPECT_EQ(group.size(), 2);
    EXPECT_EQ(group.CloseAll(kTestEpitaph), true);
    EXPECT_EQ(group.RemoveAll(), false);
    EXPECT_EQ(group.size(), 0);

    // Run the loop until the close handlers are resolved.
    loop->RunUntilIdle();

    // Ensure that the close handler was fired for the closed bindings' |impl|s the correct
    // number of times.
    for (const auto& impl : impls) {
      EXPECT_EQ(impl.get_close_count(), 2);
    }

    // Mark the closed bindings as killed, so that the rest of the test knows which bindings
    // should and should not be closed via the client making a |Terminate| call.
    for (const auto& open_binding : open_bindings) {
      open_bindings[open_binding.first] = nullptr;
    }
  });
}

TEST(BindingGroup, CannotCloseAfterRemove) {
  ExternalKillBindingTest([](fidl::ServerBindingGroup<Testable>& group, async::Loop* loop,
                             std::vector<TestImpl>& impls,
                             std::map<size_t, const TestImpl*>& open_bindings) {
    const TestImpl* target_impl = &impls[1];
    EXPECT_EQ(group.RemoveBindings(target_impl), true);
    EXPECT_EQ(group.CloseBindings(target_impl, kTestEpitaph), false);
    EXPECT_EQ(group.size(), 2);
    EXPECT_EQ(group.RemoveAll(), true);
    EXPECT_EQ(group.CloseAll(kTestEpitaph), false);
    EXPECT_EQ(group.size(), 0);

    loop->RunUntilIdle();

    // Ensure that no close counters were incremented, since this was merely a removal.
    for (auto& impl : open_bindings) {
      EXPECT_EQ(impl.second->get_close_count(), 0);
    }

    // Mark the removed bindings as killed, so that the rest of the test knows which bindings
    // should and should not be closed via the client making a |Terminate| call.
    for (const auto& open_binding : open_bindings) {
      open_bindings[open_binding.first] = nullptr;
    }
  });
}

// These classes are used to create a server implementation with multiple
// inheritance.
class PlaceholderBase1 {
 public:
  virtual void Foo() = 0;
  int a;
};

class PlaceholderBase2 {
 public:
  virtual void Bar() = 0;
  int b;
};

class MultiInheritanceServer : public PlaceholderBase1,
                               public fidl::Server<fidl_cpp_wire_bindinggroup_test::Foo>,
                               public fidl::Server<Testable>,
                               public fidl::WireServer<fidl_cpp_wire_bindinggroup_test::Bar>,
                               public PlaceholderBase2 {
 public:
  MultiInheritanceServer() = default;
  MultiInheritanceServer(MultiInheritanceServer&& other) = delete;
  MultiInheritanceServer(const MultiInheritanceServer& other) = delete;
  MultiInheritanceServer& operator=(MultiInheritanceServer&& other) = delete;
  MultiInheritanceServer& operator=(const MultiInheritanceServer& other) = delete;

  void Foo() override {}
  void Bar() override {}

  void Echo(EchoRequest& request, EchoCompleter::Sync& completer) override {
    completer.Reply(request.str());
  }

  void Terminate(TerminateCompleter::Sync& completer) override { completer.Close(kTestEpitaph); }
};

TEST(BindingGroup, DisambiguateMultiProtocolImplementations) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  fidl::ServerBindingGroup<Testable> binding_group;
  MultiInheritanceServer impl;
  auto endpoints = fidl::Endpoints<Testable>::Create();
  binding_group.AddBinding(loop.dispatcher(), std::move(endpoints.server), &impl,
                           fidl::kIgnoreBindingClosure);
  fidl::Client client(std::move(endpoints.client), loop.dispatcher());
  client->Echo({"test"}).ThenExactlyOnce([&](fidl::Result<Testable::Echo>& result) {
    loop.Quit();
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ("test", result->str());
  });
  loop.Run();
}

}  // namespace
