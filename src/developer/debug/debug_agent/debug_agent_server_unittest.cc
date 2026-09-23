// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/debug_agent_server.h"

#include <algorithm>
#include <memory>

#include <gtest/gtest.h>

#include "lib/async/default.h"
#include "src/developer/debug/debug_agent/mock_component_manager.h"
#include "src/developer/debug/debug_agent/mock_debug_agent_harness.h"
#include "src/developer/debug/debug_agent/mock_process.h"
#include "src/developer/debug/debug_agent/mock_process_handle.h"
#include "src/developer/debug/debug_agent/mock_thread.h"
#include "src/developer/debug/debug_agent/process_info_iterator.h"
#include "src/developer/debug/ipc/protocol.h"
#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/shared/message_loop.h"
#include "src/developer/debug/shared/result.h"
#include "src/developer/debug/shared/test_with_loop.h"

namespace debug_agent {

// This class is a friend of DebugAgentServer so that we may test the private, non-FIDL APIs
// directly.
class DebugAgentServerTest : public debug::TestWithLoop {
 public:
  DebugAgentServerTest()
      : server_(harness_.debug_agent()->GetWeakPtr(),
                debug::MessageLoopFuchsia::Current()->dispatcher()) {}

  DebugAgentServer::AddFilterResult AddFilter(const fuchsia_debugger::Filter& filter) {
    auto server_filters_before = server_.filters_;
    auto result = server_.AddFilter(filter);

    std::map<debug_ipc::Filter::Identifier, debug_ipc::Filter> diff;

    // Take the previous list of filters known by the server and compare it to the new list. The
    // resulting map of filters will contain all of the new filters that were added by this call.
    // There should always be at least one, and potentially two, added filters, depending on the
    // settings of |filter|.
    std::set_difference(server_.filters_.begin(), server_.filters_.end(),
                        server_filters_before.begin(), server_filters_before.end(),
                        std::inserter(diff, diff.begin()),
                        [](const auto& p1, const auto& p2) { return p1.first < p2.first; });

    if (result.ok()) {
      last_added_filters_.clear();
      last_added_filters_.insert(diff.begin(), diff.end());
    }

    return result;
  }

  const std::vector<debug_ipc::NotifyFilterCreated>& notify_filter_created() {
    return harness_.stream_backend()->filters();
  }

  uint32_t AttachToMatchingKoids(const debug_ipc::UpdateFilterReply& reply) {
    return server_.AttachToFilterMatches(reply.matched_processes_for_filter);
  }

  auto GetMatchingProcesses(std::optional<fuchsia_debugger::Filter> filter) {
    return server_.GetMatchingProcesses(std::move(filter));
  }

  debug_ipc::StatusReply GetAgentStatus() {
    debug_ipc::StatusReply reply;
    harness_.debug_agent()->OnStatus({}, &reply);
    return reply;
  }

  const std::map<debug_ipc::Filter::Identifier, debug_ipc::Filter>& last_added_filters() const {
    return last_added_filters_;
  }
  MockDebugAgentHarness* harness() { return &harness_; }
  DebugAgent* GetDebugAgent() { return harness_.debug_agent(); }
  DebugAgentServer* server() { return &server_; }

 private:
  // This is a map to both reflect that sending one filter in an UpdateFilter request may result in
  // more than one filter actually being installed, and for easy lookup by filter identifier.
  std::map<debug_ipc::Filter::Identifier, debug_ipc::Filter> last_added_filters_;
  MockDebugAgentHarness harness_;
  DebugAgentServer server_;
};

TEST_F(DebugAgentServerTest, AddNewFilter) {
  DebugAgent* agent = GetDebugAgent();

  auto status_reply = GetAgentStatus();

  // There shouldn't be any installed filters yet.
  ASSERT_EQ(status_reply.filters.size(), 0u);

  fuchsia_debugger::Filter first;
  // This will match job koid 25 from mock_system_interface.
  first.pattern("fixed/moniker");
  first.type(fuchsia_debugger::FilterType::kMonikerSuffix);

  auto result = AddFilter(first);
  EXPECT_TRUE(result.ok());

  auto reply = result.take_value();

  // There should be one reported match.
  EXPECT_EQ(reply.matched_processes_for_filter.size(), 1u);
  EXPECT_EQ(reply.matched_processes_for_filter[0].matches.size(), 1u);
  EXPECT_EQ(debug_ipc::TaskType::kProcess, reply.matched_processes_for_filter[0].matches[0].type);

  // Now attach to the matching koid.
  EXPECT_EQ(AttachToMatchingKoids(reply), 1u);

  status_reply = GetAgentStatus();

  EXPECT_EQ(status_reply.filters.size(), 1u);
  EXPECT_EQ(status_reply.filters[0].pattern, first.pattern());
  EXPECT_EQ(status_reply.filters[0].type, debug_ipc::Filter::Type::kComponentMonikerSuffix);
  // The recursive flag was left unspecified, which should leave the default value of false in the
  // debug_ipc filter.
  EXPECT_EQ(status_reply.filters[0].config.recursive, false);
  EXPECT_EQ(status_reply.processes.size(), 1u);
  EXPECT_EQ(status_reply.processes[0].process_koid,
            reply.matched_processes_for_filter[0].matches[0].koid);

  // Run the loop so the process has a chance to update its thread list.
  loop().RunUntilNoTasks();

  auto proc = agent->GetDebuggedProcess(reply.matched_processes_for_filter[0].matches[0].koid);
  auto thread_records = proc->GetThreadRecords();

  ASSERT_FALSE(thread_records.empty());

  // No threads should be suspended because we should have attached weakly.
  for (auto& record : thread_records) {
    EXPECT_EQ(record.state, debug_ipc::ThreadRecord::State::kRunning);
  }

  // Corresponds to the koid of the process under the "fixed/moniker" component.
  constexpr zx_koid_t kProcessKoid = 26;
  EXPECT_NE(agent->GetDebuggedProcess(kProcessKoid), nullptr);

  // Simulate a test environment rooted in the collection "test" with name "test_root". A recursive
  // moniker suffix filter on "test:test_root" will implicitly install a second moniker prefix
  // filter for the entire moniker up to and including "test:test_root" so that any child components
  // spawned within its realm will be attached to. We don't need to know the moniker of any child
  // components in order to attach to any processes they contain. This is modeled after the real
  // moniker structure used for tests in TestManager.
  constexpr char kFullRootMoniker[] = "/moniker/generated/test:test_root";
  constexpr zx_koid_t kJob4Process1Koid = 33;

  fuchsia_debugger::Filter second;
  second.pattern("test:test_root");
  second.type(fuchsia_debugger::FilterType::kMonikerSuffix);
  second.options().recursive(true);

  result = AddFilter(second);
  EXPECT_TRUE(result.ok());

  reply = result.take_value();

  // When the recursive filter matches at installation time, then we also install and match against
  // the moniker prefix filter that's sent to the client via NotifyFilterCreated. At this level of
  // the interface, we can't get the filter ID for the filter moniker suffix filter, so we can't
  // check the originating_filter_id field of the notification, but we can assert our expectations
  // on the filter settings.
  const auto& new_filters = harness()->stream_backend()->filters();
  ASSERT_FALSE(new_filters.empty());
  ASSERT_EQ(new_filters.size(), 3u);
  // We are not required to send another UpdateFilter request to the Agent since it matched eagerly.
  EXPECT_TRUE(new_filters[0].participated_in_matching);

  const auto& job4_moniker_prefix_filter = std::ranges::find_if(
      new_filters, [&kFullRootMoniker](const debug_ipc::NotifyFilterCreated& notify) {
        return notify.filter.pattern == kFullRootMoniker;
      });

  ASSERT_NE(job4_moniker_prefix_filter, new_filters.end());
  EXPECT_EQ(job4_moniker_prefix_filter->filter.id.Decode().originator,
            debug_ipc::Filter::Originator::kAgent);
  EXPECT_EQ(job4_moniker_prefix_filter->filter.pattern, kFullRootMoniker);
  EXPECT_EQ(job4_moniker_prefix_filter->filter.type,
            debug_ipc::Filter::Type::kComponentMonikerPrefix);
  EXPECT_EQ(job4_moniker_prefix_filter->filter.config.recursive, false);

  // We will get back the process that we matched and attached to above with the first filter we
  // installed, and we should also get back the process under another component in
  // |kFullRootMoniker|'s realm.
  EXPECT_EQ(reply.matched_processes_for_filter.size(), 5u);

  const auto& job4p1_match = std::ranges::find_if(
      reply.matched_processes_for_filter, [](const debug_ipc::FilterMatch& match) {
        return std::ranges::any_of(
            match.matches, [](const auto& match) { return match.koid == kJob4Process1Koid; });
      });

  ASSERT_NE(job4p1_match, reply.matched_processes_for_filter.end());
  ASSERT_EQ(job4p1_match->matches.size(), 1u);
  EXPECT_EQ(job4p1_match->matches[0].koid, kJob4Process1Koid);
  EXPECT_EQ(job4p1_match->matches[0].type, debug_ipc::TaskType::kProcess);

  // We can issue an attach now. Since we're not doing any pre-filtering of koids we're already
  // attached to, this will also return the koid 26 attached above as well as the new one.
  EXPECT_EQ(AttachToMatchingKoids(reply), 5u);
  EXPECT_NE(agent->GetDebuggedProcess(kJob4Process1Koid), nullptr);

  // Now we install a job-only filter that will attach DebugAgent directly to a matching job's
  // exception channel.
  fuchsia_debugger::Filter third;
  third.type(fuchsia_debugger::FilterType::kMonikerPrefix);
  // Component moniker associated with job5 in mock_system_interface.
  third.pattern("/some");
  third.options().job_only(true);

  result = AddFilter(third);
  EXPECT_TRUE(result.ok());

  reply = result.take_value();

  auto third_filter_match = std::ranges::find_if(reply.matched_processes_for_filter,
                                                 [this](const debug_ipc::FilterMatch& match) {
                                                   return last_added_filters().contains(match.id);
                                                 });

  ASSERT_NE(third_filter_match, reply.matched_processes_for_filter.end());
  EXPECT_EQ(third_filter_match->matches.size(), 2u);

  constexpr zx_koid_t kJob5Koid = 35;
  constexpr zx_koid_t kJob51Koid = 38;

  // The order of the matches will always be in ascending order.
  EXPECT_EQ(third_filter_match->matches[0].koid, kJob5Koid);
  EXPECT_EQ(third_filter_match->matches[0].type, debug_ipc::TaskType::kJob);
  EXPECT_EQ(third_filter_match->matches[1].koid, kJob51Koid);
  EXPECT_EQ(third_filter_match->matches[1].type, debug_ipc::TaskType::kJob);

  // Now we test explicit attach requests. This will be the case if the filter is installed after
  // the component that matches is already launched and we have been notified of it. See the
  // job_only tests in debug_agent_unittests to see the case where the filter is matched upon a
  // notification that a component is starting.
  debug_ipc::AttachRequest attach_request;
  attach_request.koid = kJob5Koid;
  attach_request.config.target = debug_ipc::TaskType::kJob;

  debug_ipc::AttachReply attach_reply;
  agent->OnAttach(attach_request, &attach_reply);
  ASSERT_TRUE(attach_reply.status.ok()) << attach_reply.status.message();

  attach_request.koid = kJob51Koid;

  agent->OnAttach(attach_request, &attach_reply);
  ASSERT_TRUE(attach_reply.status.has_error());
  ASSERT_EQ(attach_reply.status.type(), debug::Status::kAlreadyExists);

  EXPECT_TRUE(agent->GetDebuggedJob(kJob5Koid));
  // Should not be attached to the child job.
  EXPECT_FALSE(agent->GetDebuggedJob(kJob51Koid));
}

TEST_F(DebugAgentServerTest, AddFilterErrors) {
  fuchsia_debugger::Filter f;

  auto result = AddFilter(f);
  EXPECT_TRUE(result.has_error());
  EXPECT_EQ(result.err(), fuchsia_debugger::FilterError::kNoPattern);

  // Set pattern but not type.
  f.pattern("test");

  result = AddFilter(f);
  EXPECT_TRUE(result.has_error());
  EXPECT_EQ(result.err(), fuchsia_debugger::FilterError::kUnknownType);

  // Some filter type from the future.
  f.type(static_cast<fuchsia_debugger::FilterType>(1234));

  result = AddFilter(f);
  EXPECT_TRUE(result.has_error());
  EXPECT_EQ(result.err(), fuchsia_debugger::FilterError::kUnknownType);

  // Set both recursive and job_only options.
  f.type(fuchsia_debugger::FilterType::kMonikerPrefix);
  f.options().recursive(true);
  f.options().job_only(true);
  result = AddFilter(f);
  EXPECT_TRUE(result.ok());
}

TEST_F(DebugAgentServerTest, GetMatchingProcesses) {
  auto agent = GetDebugAgent();

  // Not passing a filter will return all attached processes. There aren't any of those yet, so the
  // return value is empty.
  auto result = GetMatchingProcesses(std::nullopt);
  EXPECT_TRUE(result.ok());
  EXPECT_TRUE(result.value().empty());

  // If provided, the filter must be valid.
  result = GetMatchingProcesses({{{.pattern = ""}}});
  EXPECT_TRUE(result.has_error());
  EXPECT_EQ(result.err(), fuchsia_debugger::FilterError::kNoPattern);

  result = GetMatchingProcesses({{{.pattern = "some/pattern"}}});
  EXPECT_TRUE(result.has_error());
  EXPECT_EQ(result.err(), fuchsia_debugger::FilterError::kUnknownType);

  constexpr char kFullRootMoniker[] = "/moniker/generated/test:test_root";
  constexpr char kUrl[] = "fuchsia-pkg://devhost/root_package#meta/root_component.cm";

  // This filter is intended to match |kFullRootMoniker| and all child components.
  fuchsia_debugger::Filter f1;
  f1.pattern("test:test_root");
  f1.type(fuchsia_debugger::FilterType::kMonikerSuffix);
  f1.options({{.recursive = true}});

  AddFilter(f1);

  // Create the subfilter.
  harness()->system_interface()->mock_component_manager().InjectComponentEvent(
      FakeEventType::kDebugStarted, kFullRootMoniker, kUrl);

  loop().RunUntilNoTasks();

  // Koid of job4 from MockSystemInterface.
  constexpr zx_koid_t kJob4Koid = 32;
  // Inject a process starting event for the ELF process running under some child component of the
  // root component above.
  constexpr zx_koid_t kProcessKoid = 33;
  auto handle = std::make_unique<MockProcessHandle>(kProcessKoid);
  // Set the job koid so that we can look up the corresponding component information.
  handle->set_job_koid(kJob4Koid);
  agent->OnProcessChanged(DebugAgent::ProcessChangedHow::kStarting, std::move(handle));

  // We are now attached to something. Omitting the filter should give us back a process.
  result = GetMatchingProcesses(std::nullopt);
  EXPECT_TRUE(result.ok());
  EXPECT_EQ(result.value().size(), 1u);
  EXPECT_EQ(result.value()[0]->koid(), kProcessKoid);
}

TEST_F(DebugAgentServerTest, AttachToJobOnComponentStarting) {
  constexpr zx_koid_t kJobKoid = 101;
  constexpr std::string kComponentMoniker = "some/fake/moniker";
  constexpr std::string kComponentUrl = "url";

  fuchsia_debugger::Filter filter;
  filter.pattern("moniker");
  filter.type(fuchsia_debugger::FilterType::kMonikerSuffix);
  filter.options().job_only(true);

  AddFilter(filter);

  harness()->system_interface()->mock_component_manager().InjectComponentEvent(
      FakeEventType::kDebugStarted, kComponentMoniker, kComponentUrl, kJobKoid);

  loop().RunUntilNoTasks();

  EXPECT_NE(GetDebugAgent()->GetDebuggedJob(kJobKoid), nullptr);
}

// A client of ProcessInfoIterator that collects all processes returned by the iterator. Errors will
// raise an assertion.
class ProcessInfoCollector {
 public:
  ProcessInfoCollector(fidl::ClientEnd<fuchsia_debugger::ProcessInfoIterator> client_end,
                       async_dispatcher_t* dispatcher)
      : client_(std::move(client_end), dispatcher) {}

  void Collect(fit::callback<void(std::vector<fuchsia_debugger::ProcessInfo>)> on_done) {
    client_->GetNext().Then(
        [this, on_done = std::move(on_done)](
            fidl::Result<fuchsia_debugger::ProcessInfoIterator::GetNext>& result) mutable {
          ASSERT_TRUE(result.is_ok());

          const auto& new_infos = result->info();

          if (new_infos.empty()) {
            return on_done(std::move(collected_infos_));
          }

          collected_infos_.insert(collected_infos_.end(), new_infos.begin(), new_infos.end());

          debug::MessageLoop::Current()->PostTask(
              FROM_HERE,
              [this, on_done = std::move(on_done)]() mutable { Collect(std::move(on_done)); });
        });
  }

 private:
  fidl::Client<fuchsia_debugger::ProcessInfoIterator> client_;
  std::vector<fuchsia_debugger::ProcessInfo> collected_infos_;
};

class FakeFidlClient : public fidl::AsyncEventHandler<fuchsia_debugger::DebugAgent> {
 public:
  explicit FakeFidlClient(fidl::ClientEnd<fuchsia_debugger::DebugAgent> client_end,
                          async_dispatcher_t* dispatcher)
      : client_(std::move(client_end), dispatcher, this), dispatcher_(dispatcher) {}

  void AttachTo(const fuchsia_debugger::Filter& filter,
                fit::callback<void(fidl::Result<fuchsia_debugger::DebugAgent::AttachTo>&)> cb) {
    client_->AttachTo(filter).Then(
        [cb = std::move(cb)](fidl::Result<fuchsia_debugger::DebugAgent::AttachTo>& reply) mutable {
          ASSERT_TRUE(cb);
          cb(reply);
        });
  }

  // This method is allowed to be synchronous, clients may begin feeding messages into the returned
  // iterator immediately.
  ProcessInfoCollector MakeProcessInfoCollector(
      std::optional<const fuchsia_debugger::Filter> filter,
      std::optional<const fuchsia_debugger::ThreadDetailsInterest> interest) {
    auto [client_end, server_end] = *fidl::CreateEndpoints<fuchsia_debugger::ProcessInfoIterator>();

    fuchsia_debugger::GetProcessInfoOptions options(
        {.filter = std::move(filter), .interest = std::move(interest)});

    client_
        ->GetProcessInfo(fuchsia_debugger::DebugAgentGetProcessInfoRequest(std::move(options),
                                                                           std::move(server_end)))
        .Then([](fidl::Result<fuchsia_debugger::DebugAgent::GetProcessInfo>& reply) {
          ASSERT_TRUE(reply.is_ok());
        });

    return ProcessInfoCollector(std::move(client_end), dispatcher_);
  }

  void OnFatalException(
      fidl::Event<fuchsia_debugger::DebugAgent::OnFatalException>& event) override {
    exceptions_.emplace_back(event);
    debug::MessageLoop::Current()->QuitNow();
  }

  const auto& GetExceptions() const { return exceptions_; }

  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_debugger::DebugAgent> metadata) override {
    FX_LOGS(WARNING) << "Unknown event: " << metadata.event_ordinal;
  }

 private:
  std::vector<fidl::Event<fuchsia_debugger::DebugAgent::OnFatalException>> exceptions_;
  fidl::Client<fuchsia_debugger::DebugAgent> client_;
  async_dispatcher_t* dispatcher_;
};

class DebugAgentServerTestWithClient : public debug::TestWithLoop {
 public:
  void SetUp() override {
    auto [client_end, server_end] = *fidl::CreateEndpoints<fuchsia_debugger::DebugAgent>();
    client_ =
        std::make_unique<FakeFidlClient>(std::move(client_end), async_get_default_dispatcher());

    // The server is owned by the message loop, but we hold on to a pointer here so we can actually
    // poke at the server to debug ourselves.
    server_ = DebugAgentServer::BindServer(async_get_default_dispatcher(), std::move(server_end),
                                           harness()->debug_agent()->GetWeakPtr());
  }

  void TearDown() override { client_.reset(); }

  FakeFidlClient& client() { return *client_; }
  MockDebugAgentHarness* harness() { return &harness_; }
  DebugAgent* agent() { return harness_.debug_agent(); }

 private:
  std::unique_ptr<FakeFidlClient> client_ = nullptr;
  DebugAgentServer* server_;
  MockDebugAgentHarness harness_;
};

TEST_F(DebugAgentServerTestWithClient, OnFatalException) {
  constexpr zx_koid_t kProcessKoid = 0x1234;
  constexpr zx_koid_t kThreadKoid = 0x2345;
  auto mock_process = harness()->AddProcess(kProcessKoid);
  auto mock_thread = mock_process->AddThread(kThreadKoid);

  // Now that the server is bound to the message loop with a client, we can send the notification.
  // Note that there may be an error from inspector complaining about a process koid that doesn't
  // exist, but that's not important for this test.
  mock_thread->SendException(0x12345678, debug_ipc::ExceptionType::kGeneral);

  loop().Run();

  ASSERT_EQ(client().GetExceptions().size(), 1u);
  EXPECT_TRUE(client().GetExceptions()[0].thread());
  EXPECT_EQ(*client().GetExceptions()[0].thread(), kThreadKoid);
}

// Debug exception types should not send notifications to clients, e.g. single step, software
// breakpoints, etc.
TEST_F(DebugAgentServerTestWithClient, DebugExceptionDoesNotSendEvent) {
  constexpr zx_koid_t kProcessKoid = 0x1234;
  constexpr zx_koid_t kThreadKoid = 0x2345;
  auto mock_process = harness()->AddProcess(kProcessKoid);
  auto mock_thread = mock_process->AddThread(kThreadKoid);

  // clang-format off
  constexpr std::array<debug_ipc::ExceptionType, 5> debug_exceptions = {
    // These are taken from the same set that populates the IsDebug function in ipc/records.cc. We
    // don't need to worry about the process and thread lifetime exceptions that will return true in
    // that function because separate debug_ipc notifications will be sent for those and if we
    // decide to make them !IsDebug then it shouldn't affect this FIDL event.
    debug_ipc::ExceptionType::kHardwareBreakpoint,
    debug_ipc::ExceptionType::kWatchpoint,
    debug_ipc::ExceptionType::kSingleStep,
    debug_ipc::ExceptionType::kSoftwareBreakpoint,
    debug_ipc::ExceptionType::kSynthetic,
  };
  // clang-format on

  constexpr uint64_t kExceptionAddress = 0x12345678;
  for (auto exception_type : debug_exceptions) {
    // Watchpoints need some special set up.
    if (exception_type == debug_ipc::ExceptionType::kWatchpoint) {
      DebugRegisters debug_registers;
      auto wp_info = debug_registers.SetWatchpoint(debug_ipc::BreakpointType::kReadWrite,
                                                   {kExceptionAddress, kExceptionAddress + 1}, 4);
      ASSERT_TRUE(wp_info);
      debug_registers.SetForHitWatchpoint(wp_info->slot);

      mock_thread->mock_thread_handle().SetDebugRegisters(debug_registers);
    }

    mock_thread->SendException(kExceptionAddress, exception_type);

    // This should return immediately.
    loop().RunUntilNoTasks();

    ASSERT_TRUE(client().GetExceptions().empty());
  }
}

TEST_F(DebugAgentServerTestWithClient, AttachToJobOnFilterInstalled) {
  // The non-exhaustive job structure with matching component information will look like this from
  // MockSystemInterface:
  // ..
  // root
  // ├─j: 8 "/moniker"
  // ├─j: 25 "/a/long/generated_to_here/fixed/moniker"
  // └─j: 35 "/some/moniker"
  //    └─j: 38 "/some/other/moniker"
  // ..
  // Since job 38 is a child of 35, DebugAgent shouldn't attach to it, but it will appear as a match
  // to the filter. Therefore, there will be four matches for the filter, and three expected
  // attaches.
  constexpr std::array kExpectedJobKoids = {8, 25, 35};

  // See the default pre-populated system in mock_system_interface.h to see which monikers will be
  // matched.
  fuchsia_debugger::Filter filter;
  filter.pattern("moniker");
  filter.type(fuchsia_debugger::FilterType::kMonikerSuffix);
  filter.options().job_only(true);

  client().AttachTo(
      filter, [=, this](fidl::Result<fuchsia_debugger::DebugAgent::AttachTo>& result) mutable {
        ASSERT_TRUE(result.is_ok());
        EXPECT_EQ(result->num_matches(), 4u);
        for (auto koid : kExpectedJobKoids) {
          EXPECT_NE(agent()->GetDebuggedJob(koid), nullptr);
        }

        loop().QuitNow();
      });

  loop().Run();
}

TEST_F(DebugAgentServerTestWithClient, ProcessInfoIterator) {
  fuchsia_debugger::Filter filter;
  filter.pattern("/moniker");
  filter.type(fuchsia_debugger::FilterType::kMoniker);

  client().AttachTo(
      filter, [=, this](fidl::Result<fuchsia_debugger::DebugAgent::AttachTo>& result) mutable {
        ASSERT_TRUE(result.is_ok());
        loop().QuitNow();
      });

  loop().Run();

  // Don't need to wait for the message loop to start sending messages.
  auto collector = client().MakeProcessInfoCollector(std::nullopt, std::nullopt);

  std::vector<fuchsia_debugger::ProcessInfo> infos;
  collector.Collect([&, this](std::vector<fuchsia_debugger::ProcessInfo> new_infos) {
    infos = std::move(new_infos);
    loop().QuitNow();
  });

  loop().Run();

  // From mock_system_interface.
  constexpr std::array kExpectedMonikers = {"/moniker"};
  constexpr std::array kExpectedProcessIds = {9, 11, 14, 19, 21};
  constexpr std::array kExpectedThreadIds = {10, 12, 15, 16, 20, 22, 23, 24};

  ASSERT_FALSE(infos.empty());

  for (const auto& info : infos) {
    EXPECT_NE(std::ranges::find(kExpectedProcessIds, info.process()), kExpectedProcessIds.end());
    EXPECT_NE(std::ranges::find(kExpectedThreadIds, info.thread()), kExpectedThreadIds.end());
    EXPECT_NE(std::ranges::find(kExpectedMonikers, info.moniker()), kExpectedMonikers.end());
    // We didn't ask for any interest, so we don't get back any backtraces.
    EXPECT_EQ(info.details().backtrace(), std::nullopt);
  }
}

TEST_F(DebugAgentServerTestWithClient, RecursiveJobOnlyProcessInfoIterator) {
  fuchsia_debugger::Filter filter;
  filter.pattern("test_root");
  filter.type(fuchsia_debugger::FilterType::kMonikerSuffix);
  filter.options().job_only(true);
  filter.options().recursive(true);

  client().AttachTo(
      filter, [=, this](fidl::Result<fuchsia_debugger::DebugAgent::AttachTo>& result) mutable {
        ASSERT_TRUE(result.is_ok());
        loop().QuitNow();
      });

  loop().Run();

  // Don't need to wait for the message loop to start sending messages.
  auto collector = client().MakeProcessInfoCollector(std::nullopt, std::nullopt);

  std::vector<fuchsia_debugger::ProcessInfo> infos;
  collector.Collect([&, this](std::vector<fuchsia_debugger::ProcessInfo> new_infos) {
    infos = std::move(new_infos);
    loop().QuitNow();
  });

  loop().Run();

  // From mock_system_interface.
  constexpr std::array kExpectedMonikers = {
      "/moniker/generated/test:test_root/driver", "moniker/abcdef/test:test_root/test_driver",
      "moniker/abcdef/test:test_root/under_test", "moniker/abc123/test:test_root"};
  constexpr std::array kExpectedProcessIds = {33, 42, 45, 49};
  constexpr std::array kExpectedThreadIds = {34, 43, 46, 47, 50};

  ASSERT_FALSE(infos.empty());

  for (const auto& info : infos) {
    EXPECT_NE(std::ranges::find(kExpectedProcessIds, info.process()), kExpectedProcessIds.end());
    EXPECT_NE(std::ranges::find(kExpectedThreadIds, info.thread()), kExpectedThreadIds.end());
    EXPECT_NE(std::ranges::find(kExpectedMonikers, info.moniker()), kExpectedMonikers.end());
    // We didn't ask for any interest, so we don't get back any backtraces.
    EXPECT_EQ(info.details().backtrace(), std::nullopt);
  }
}

TEST_F(DebugAgentServerTestWithClient, RecursiveJobOnlyProcessInfoIteratorWithFilter) {
  // Attach to all test realms.
  fuchsia_debugger::Filter filter;
  filter.pattern("test_root");
  filter.type(fuchsia_debugger::FilterType::kMonikerSuffix);
  filter.options().job_only(true);
  filter.options().recursive(true);

  client().AttachTo(
      filter, [=, this](fidl::Result<fuchsia_debugger::DebugAgent::AttachTo>& result) mutable {
        ASSERT_TRUE(result.is_ok());
        loop().QuitNow();
      });

  loop().Run();

  // Filters that are given to the iterator are typically going to be package urls e.g. from
  // TestManager. This is tricky, because we may get a request to recursively match against a
  // realm's component URL recursively, without installing any additional filters.
  fuchsia_debugger::Filter iterator_filter;
  iterator_filter.pattern("fuchsia-pkg://devhost/test_root_package#meta/root_test_component.cm");
  iterator_filter.type(fuchsia_debugger::FilterType::kUrl);
  iterator_filter.options().recursive(true);

  // Don't need to wait for the message loop to start sending messages.
  auto collector = client().MakeProcessInfoCollector(std::move(iterator_filter), std::nullopt);

  std::vector<fuchsia_debugger::ProcessInfo> infos;
  collector.Collect([&, this](std::vector<fuchsia_debugger::ProcessInfo> new_infos) {
    infos = std::move(new_infos);
    loop().QuitNow();
  });

  loop().Run();

  // From mock_system_interface.
  constexpr std::array kExpectedMonikers = {
      "moniker/abcdef/test:test_root/test_driver",
      "moniker/abcdef/test:test_root/under_test",
  };
  constexpr std::array kExpectedProcessIds = {42, 45};
  constexpr std::array kExpectedThreadIds = {43, 46, 47};

  ASSERT_FALSE(infos.empty());

  for (const auto& info : infos) {
    EXPECT_NE(std::ranges::find(kExpectedProcessIds, info.process()), kExpectedProcessIds.end());
    EXPECT_NE(std::ranges::find(kExpectedThreadIds, info.thread()), kExpectedThreadIds.end());
    EXPECT_NE(std::ranges::find(kExpectedMonikers, info.moniker()), kExpectedMonikers.end());
    // We didn't ask for any interest, so we don't get back any backtraces.
    EXPECT_EQ(info.details().backtrace(), std::nullopt);
  }
}

}  // namespace debug_agent
