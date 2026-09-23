// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.cpu.profiler/cpp/fidl.h>
#include <fidl/fuchsia.sys2/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/spawn.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/clock.h>
#include <lib/zx/job.h>
#include <lib/zx/process.h>
#include <lib/zx/result.h>
#include <lib/zx/socket.h>
#include <lib/zx/thread.h>
#include <lib/zx/time.h>
#include <unistd.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/syscalls.h>
#include <zircon/threads.h>
#include <zircon/types.h>

#include <cctype>
#include <condition_variable>
#include <cstddef>
#include <cstdlib>
#include <mutex>
#include <set>
#include <sstream>
#include <string>
#include <thread>
#include <utility>
#include <vector>

#include <gtest/gtest.h>
#include <src/lib/fsl/socket/strings.h>
#include <trace-reader/reader.h>
#include <trace-reader/records.h>

namespace fprofiler = fuchsia_cpu_profiler;

namespace {
void MakeWork() {
  for (;;) {
    // We need to have at least some side effect producing code or a release build will elide the
    // entire function
    FX_LOGS(TRACE) << "Working!";
  }
  zx_thread_exit();
}

class SocketReader {
 public:
  explicit SocketReader(zx::socket sock) : sock_(std::move(sock)) {
    thread_ = std::thread([this]() { ReadLoop(); });
  }

  ~SocketReader() {
    sock_.reset();
    if (thread_.joinable()) {
      thread_.join();
    }
  }

  void ReadUntilRecordCount(size_t min_records = 11, size_t min_pids = 0, size_t min_tids = 0) {
    std::unique_lock<std::mutex> lock(mutex_);
    cv_.wait(lock, [this, min_records, min_pids, min_tids]() {
      return (record_count_ >= min_records && pids_.size() >= min_pids &&
              tids_.size() >= min_tids) ||
             closed_;
    });
  }

  std::set<zx_koid_t> pids() {
    std::lock_guard<std::mutex> lock(mutex_);
    return pids_;
  }

  std::set<zx_koid_t> tids() {
    std::lock_guard<std::mutex> lock(mutex_);
    return tids_;
  }

  size_t record_count() {
    std::lock_guard<std::mutex> lock(mutex_);
    return record_count_;
  }

 private:
  void ReadLoop() {
    auto record_consumer = [this](trace::Record record) {
      std::lock_guard<std::mutex> lock(mutex_);
      if (record.type() == trace::RecordType::kProfiler) {
        const auto& profiler_record = record.GetProfiler();
        if (profiler_record.type() == trace::ProfilerRecordType::kBacktrace) {
          record_count_++;
          const auto& pt = profiler_record.backtrace().process_thread;
          if (pt.process_koid() != 0) {
            pids_.insert(pt.process_koid());
          }
          if (pt.thread_koid() != 0) {
            tids_.insert(pt.thread_koid());
          }
        }
      } else if (record.type() == trace::RecordType::kLargeRecord) {
        const auto& large_record = record.GetLargeRecord();
        if (large_record.type() == trace::LargeRecordType::kBlob) {
          const auto& blob = large_record.GetBlob();
          if (std::holds_alternative<trace::LargeRecordData::BlobEvent>(blob)) {
            record_count_++;
            const auto& event = std::get<trace::LargeRecordData::BlobEvent>(blob);
            if (event.process_thread.process_koid() != 0) {
              pids_.insert(event.process_thread.process_koid());
            }
            if (event.process_thread.thread_koid() != 0) {
              tids_.insert(event.process_thread.thread_koid());
            }
          }
        }
      }
      cv_.notify_all();
    };

    trace::TraceReader reader(record_consumer, [](std::string_view) {});
    std::vector<uint8_t> buffer;
    size_t buffer_end = 0;

    while (true) {
      if (buffer.size() - buffer_end < 65536) {
        buffer.resize(buffer_end + 65536);
      }
      size_t actual = 0;
      zx_status_t status =
          sock_.read(0, buffer.data() + buffer_end, buffer.size() - buffer_end, &actual);
      if (status == ZX_ERR_SHOULD_WAIT) {
        zx_signals_t pending = 0;
        status = sock_.wait_one(ZX_SOCKET_READABLE | ZX_SOCKET_PEER_CLOSED, zx::time::infinite(),
                                &pending);
        if (status == ZX_OK && (pending & ZX_SOCKET_PEER_CLOSED) &&
            !(pending & ZX_SOCKET_READABLE)) {
          break;
        }
        continue;
      } else if (status == ZX_ERR_PEER_CLOSED) {
        break;
      } else if (status != ZX_OK) {
        break;
      }

      if (actual > 0) {
        buffer_end += actual;
        size_t words_in_chunk = buffer_end / sizeof(uint64_t);
        trace::Chunk chunk(reinterpret_cast<const uint64_t*>(buffer.data()), words_in_chunk);
        reader.ReadRecords(chunk);
        size_t words_consumed = words_in_chunk - chunk.remaining_words();
        size_t bytes_consumed = words_consumed * sizeof(uint64_t);
        if (bytes_consumed > 0) {
          memmove(buffer.data(), buffer.data() + bytes_consumed, buffer_end - bytes_consumed);
          buffer_end -= bytes_consumed;
        }
      }
    }
    {
      std::lock_guard<std::mutex> lock(mutex_);
      closed_ = true;
    }
    cv_.notify_all();
  }

  zx::socket sock_;
  std::thread thread_;
  std::mutex mutex_;
  std::condition_variable cv_;
  std::set<zx_koid_t> pids_;
  std::set<zx_koid_t> tids_;
  size_t record_count_ = 0;
  bool closed_ = false;
};

// Sample the callstack via frame pointer at 100hz.
std::vector<fprofiler::SamplingConfig> default_sample_configs() {
  return std::vector{fprofiler::SamplingConfig{{
      .period = 10'000'000,
      .timebase = fprofiler::Counter::WithPlatformIndependent(fprofiler::CounterId::kNanoseconds),
      .sample = fprofiler::Sample{{
          .callgraph =
              fprofiler::CallgraphConfig{{.strategy = fprofiler::CallgraphStrategy::kFramePointer}},
          .counters = std::vector<fprofiler::Counter>{},
      }},
  }}};
}

// Sample the stack via DWARF at 100hz.
std::vector<fprofiler::SamplingConfig> dwarf_sample_configs() {
  return std::vector{fprofiler::SamplingConfig{{
      .period = 10'000'000,
      .timebase = fprofiler::Counter::WithPlatformIndependent(fprofiler::CounterId::kNanoseconds),
      .sample = fprofiler::Sample{{
          .callgraph =
              fprofiler::CallgraphConfig{{.strategy = fprofiler::CallgraphStrategy::kDwarf}},
          .counters = std::vector<fprofiler::Counter>{},
      }},
  }}};
}

// Launch a component as a dynamic child in the tests's launchpad collection.
// This will Create, Resolve, and Start the requested instance.
zx::result<fidl::ClientEnd<fuchsia_component::Binder>> RunInstance(
    const fidl::SyncClient<fuchsia_sys2::LifecycleController>& lifecycle_client,
    const std::string& name, const std::string& url, const std::string& moniker) {
  auto [client, server] = fidl::Endpoints<fuchsia_component::Binder>::Create();

  if (auto create_res = lifecycle_client->CreateInstance({{
          .parent_moniker = ".",
          .collection = {"launchpad"},
          .decl = {{
              .name = name,
              .url = url,
              .startup = fuchsia_component_decl::StartupMode::kLazy,
          }},
      }});
      create_res.is_error()) {
    FX_LOGS(ERROR) << create_res.error_value();
    return zx::error(ZX_ERR_BAD_STATE);
  }

  if (auto resolve_res = lifecycle_client->ResolveInstance({{
          .moniker = moniker,
      }});
      resolve_res.is_error()) {
    FX_LOGS(ERROR) << resolve_res.error_value();
    return zx::error(ZX_ERR_BAD_STATE);
  }

  if (auto start_res = lifecycle_client->StartInstance({{
          .moniker = moniker,
          .binder = std::move(server),
      }});
      start_res.is_error()) {
    FX_LOGS(ERROR) << start_res.error_value();
    return zx::error(ZX_ERR_BAD_STATE);
  }

  // Ensure component instance is fully registered in Component Manager before returning
  if (zx::result query_client_end = component::Connect<fuchsia_sys2::RealmQuery>();
      query_client_end.is_ok()) {
    fidl::SyncClient realm_query{std::move(*query_client_end)};
    for (int i = 0; i < 100; ++i) {
      if (realm_query->GetInstance(moniker).is_ok()) {
        break;
      }
      zx::nanosleep(zx::deadline_after(zx::msec(50)));
    }
  }

  return zx::ok(std::move(client));
}

// Tear down a child component in the tests's launchpad collection.
// This will Stop and Destroy the requested instance.
zx::result<> TearDownInstance(
    const fidl::SyncClient<fuchsia_sys2::LifecycleController>& lifecycle_client,
    const std::string& name, const std::string& moniker) {
  if (auto stop_res = lifecycle_client->StopInstance({{
          .moniker = moniker,
      }});
      stop_res.is_error()) {
    FX_LOGS(ERROR) << stop_res.error_value();
    return zx::error(ZX_ERR_BAD_STATE);
  }

  if (auto destroy_res = lifecycle_client->DestroyInstance({{.parent_moniker = ".",
                                                             .child = {{
                                                                 .name = name,
                                                                 .collection = "launchpad",
                                                             }}}});
      destroy_res.is_error()) {
    FX_LOGS(ERROR) << destroy_res.error_value();
    return zx::error(ZX_ERR_BAD_STATE);
  }
  return zx::ok();
}
}  // namespace

TEST(ProfilerIntegrationTest, EndToEnd) {
  zx::result client_end = component::Connect<fprofiler::Session>();
  ASSERT_TRUE(client_end.is_ok());
  const fidl::SyncClient client{std::move(*client_end)};

  zx::socket in_socket;
  zx::socket outgoing_socket;

  ASSERT_EQ(zx::socket::create(0u, &in_socket, &outgoing_socket), ZX_OK);
  zx::process self;
  zx::process::self()->duplicate(ZX_RIGHT_SAME_RIGHTS, &self);

  std::thread child(MakeWork);
  const zx::unowned_thread child_handle{native_thread_get_zx_handle(child.native_handle())};
  child.detach();

  zx_status_t res =
      child_handle->wait_one(ZX_THREAD_RUNNING, zx::deadline_after(zx::sec(1)), nullptr);
  ASSERT_EQ(ZX_OK, res);

  zx_info_handle_basic_t info;
  res = child_handle->get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  ASSERT_EQ(ZX_OK, res);

  fprofiler::TargetConfig target_config =
      fprofiler::TargetConfig::WithTasks(std::vector{fprofiler::Task::WithThread(info.koid)});
  fprofiler::Config config{{
      .configs = default_sample_configs(),
      .target = std::move(target_config),
  }};

  ASSERT_TRUE(client
                  ->Configure({{
                      .output = std::move(outgoing_socket),
                      .config = std::move(config),
                  }})
                  .is_ok());

  SocketReader reader(std::move(in_socket));
  ASSERT_TRUE(client->Start({{.buffer_results = true}}).is_ok());
  reader.ReadUntilRecordCount(11);

  auto stop_response = client->Stop();
  ASSERT_TRUE(stop_response.is_ok());
  ASSERT_TRUE(stop_response.value().samples_collected().has_value());
  EXPECT_GT(stop_response.value().samples_collected().value(), size_t{10});

  ASSERT_TRUE(client->Reset().is_ok());
}

// Monitor ourself and check that if we start new threads after the profiling session starts, that
// one or more of them show up in the samples we take.
TEST(ProfilerIntegrationTest, NewThreads) {
  zx::result client_end = component::Connect<fprofiler::Session>();
  ASSERT_TRUE(client_end.is_ok());
  const fidl::SyncClient client{std::move(*client_end)};

  zx::socket in_socket;
  zx::socket outgoing_socket;

  ASSERT_EQ(zx::socket::create(0u, &in_socket, &outgoing_socket), ZX_OK);
  zx::unowned_process self = zx::process::self();

  zx_info_handle_basic_t info;
  zx_status_t info_result =
      self->get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  ASSERT_EQ(ZX_OK, info_result);

  // We'll sample ourself.
  fprofiler::TargetConfig target_config =
      fprofiler::TargetConfig::WithTasks(std::vector{fprofiler::Task::WithProcess(info.koid)});

  fprofiler::Config config{{
      .configs = default_sample_configs(),
      .target = std::move(target_config),
  }};

  ASSERT_TRUE(client
                  ->Configure({{
                      .output = std::move(outgoing_socket),
                      .config = std::move(config),
                  }})
                  .is_ok());
  SocketReader reader(std::move(in_socket));
  ASSERT_TRUE(client->Start({{.buffer_results = true}}).is_ok());

  // Start some threads;
  std::thread t1{MakeWork};
  std::thread t2{MakeWork};
  std::thread t3{MakeWork};
  t1.detach();
  t2.detach();
  t3.detach();
  reader.ReadUntilRecordCount(11, 1, 2);

  auto stop_response = client->Stop();
  ASSERT_TRUE(stop_response.is_ok());
  ASSERT_TRUE(stop_response.value().samples_collected().has_value());
  EXPECT_GT(stop_response.value().samples_collected().value(), size_t{10});

  ASSERT_TRUE(client->Reset().is_ok());

  // We should only have one pid
  EXPECT_EQ(size_t{1}, reader.pids().size());

  // We should only have more than one thread
  EXPECT_GT(reader.tids().size(), size_t{1});
}

// Monitor ourself via our job id
TEST(ProfilerIntegrationTest, OwnJobId) {
  zx::result client_end = component::Connect<fprofiler::Session>();
  ASSERT_TRUE(client_end.is_ok());
  const fidl::SyncClient client{std::move(*client_end)};

  zx::socket in_socket;
  zx::socket outgoing_socket;

  ASSERT_EQ(zx::socket::create(0u, &in_socket, &outgoing_socket), ZX_OK);
  zx::unowned_job self = zx::job::default_job();

  zx_info_handle_basic_t info;
  zx_status_t info_result =
      self->get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  ASSERT_EQ(ZX_OK, info_result);

  // We'll sample ourself by our job id
  fprofiler::TargetConfig target_config =
      fprofiler::TargetConfig::WithTasks(std::vector{fprofiler::Task::WithJob(info.koid)});

  fprofiler::Config config{{
      .configs = default_sample_configs(),
      .target = std::move(target_config),
  }};

  ASSERT_TRUE(client
                  ->Configure({{
                      .output = std::move(outgoing_socket),
                      .config = std::move(config),
                  }})
                  .is_ok());
  SocketReader reader(std::move(in_socket));
  ASSERT_TRUE(client->Start({{.buffer_results = true}}).is_ok());

  std::thread t1{MakeWork};
  std::thread t2{MakeWork};
  std::thread t3{MakeWork};
  t1.detach();
  t2.detach();
  t3.detach();
  reader.ReadUntilRecordCount(11, 1, 1);

  auto stop_response = client->Stop();
  ASSERT_TRUE(stop_response.is_ok());
  ASSERT_TRUE(stop_response.value().samples_collected().has_value());
  ASSERT_GT(stop_response.value().samples_collected().value(), size_t{10});

  ASSERT_TRUE(client->Reset().is_ok());

  // We should only have one pid
  EXPECT_EQ(size_t{1}, reader.pids().size());

  // And that pid should be us
  zx::unowned_process process_self = zx::process::self();
  zx_info_handle_basic_t process_info;
  ASSERT_EQ(ZX_OK, process_self->get_info(ZX_INFO_HANDLE_BASIC, &process_info, sizeof(process_info),
                                          nullptr, nullptr));
  EXPECT_EQ(*reader.pids().begin(), process_info.koid);
}

// Monitor ourself via our job id and then launch a process as part of our job and check that it
// gets added to the profiling set
TEST(ProfilerIntegrationTest, LaunchedProcess) {
  zx::result client_end = component::Connect<fprofiler::Session>();
  ASSERT_TRUE(client_end.is_ok());
  const fidl::SyncClient client{std::move(*client_end)};

  zx::socket in_socket;
  zx::socket outgoing_socket;

  ASSERT_EQ(zx::socket::create(0u, &in_socket, &outgoing_socket), ZX_OK);
  zx::unowned_job self = zx::job::default_job();

  zx_info_handle_basic_t info;
  zx_status_t info_result =
      self->get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  ASSERT_EQ(ZX_OK, info_result);

  // We'll sample ourself by our job id
  fprofiler::TargetConfig target_config =
      fprofiler::TargetConfig::WithTasks(std::vector{fprofiler::Task::WithJob(info.koid)});

  fprofiler::Config config{{
      .configs = default_sample_configs(),
      .target = std::move(target_config),
  }};

  // Launch an additional process before starting
  zx::process process1;
  const char* kArgs[] = {"/pkg/bin/demo_target", nullptr};
  ASSERT_EQ(ZX_OK, fdio_spawn(self->get(), FDIO_SPAWN_CLONE_ALL, "/pkg/bin/demo_target", kArgs,
                              process1.reset_and_get_address()));

  size_t num_processes;
  self->get_info(ZX_INFO_JOB_PROCESSES, nullptr, 0, nullptr, &num_processes);
  ASSERT_EQ(num_processes, size_t{2});

  ASSERT_TRUE(client
                  ->Configure({{
                      .output = std::move(outgoing_socket),
                      .config = std::move(config),
                  }})
                  .is_ok());

  SocketReader reader(std::move(in_socket));
  ASSERT_TRUE(client->Start({{.buffer_results = true}}).is_ok());

  // Launch a thread in our process to ensure we get samples that aren't
  // just this process sleeping
  std::thread t1{MakeWork};
  t1.detach();

  // Then launch another process after starting
  zx::process process2;
  ASSERT_EQ(ZX_OK, fdio_spawn(self->get(), FDIO_SPAWN_CLONE_ALL, "/pkg/bin/demo_target", kArgs,
                              process2.reset_and_get_address()));

  self->get_info(ZX_INFO_JOB_PROCESSES, nullptr, 0, nullptr, &num_processes);
  ASSERT_EQ(num_processes, size_t{3});
  reader.ReadUntilRecordCount(11, 3, 1);

  auto stop_response = client->Stop();
  ASSERT_TRUE(stop_response.is_ok());
  ASSERT_TRUE(stop_response.value().samples_collected().has_value());
  ASSERT_GT(stop_response.value().samples_collected().value(), size_t{10});

  ASSERT_TRUE(client->Reset().is_ok());

  // We should three pids, our pid, the pid of process1, and the pid of process2
  zx_info_handle_basic_t pid_info;
  ASSERT_EQ(ZX_OK, zx::process::self()->get_info(ZX_INFO_HANDLE_BASIC, &pid_info, sizeof(pid_info),
                                                 nullptr, nullptr));
  zx_koid_t our_pid = pid_info.koid;
  ASSERT_EQ(ZX_OK,
            process1.get_info(ZX_INFO_HANDLE_BASIC, &pid_info, sizeof(pid_info), nullptr, nullptr));
  zx_koid_t process1_pid = pid_info.koid;
  ASSERT_EQ(ZX_OK,
            process2.get_info(ZX_INFO_HANDLE_BASIC, &pid_info, sizeof(pid_info), nullptr, nullptr));
  zx_koid_t process2_pid = pid_info.koid;
  EXPECT_EQ(size_t{3}, reader.pids().size());
  EXPECT_TRUE(reader.pids().find(our_pid) != reader.pids().end());
  EXPECT_TRUE(reader.pids().find(process1_pid) != reader.pids().end());
  EXPECT_TRUE(reader.pids().find(process2_pid) != reader.pids().end());
  process1.kill();
  process2.kill();
}

// Monitor ourself via our job id and then launch a process as part of our job and check that it we
// see the threads it spawns
TEST(ProfilerIntegrationTest, LaunchedProcessThreadSpawner) {
  zx::result client_end = component::Connect<fprofiler::Session>();
  ASSERT_TRUE(client_end.is_ok());
  const fidl::SyncClient client{std::move(*client_end)};

  zx::socket in_socket;
  zx::socket outgoing_socket;

  ASSERT_EQ(zx::socket::create(0u, &in_socket, &outgoing_socket), ZX_OK);
  zx::unowned_job self = zx::job::default_job();

  zx_info_handle_basic_t info;
  zx_status_t info_result =
      self->get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  ASSERT_EQ(ZX_OK, info_result);

  // We'll sample ourself by our job id
  fprofiler::TargetConfig target_config =
      fprofiler::TargetConfig::WithTasks(std::vector{fprofiler::Task::WithJob(info.koid)});

  fprofiler::Config config{{
      .configs = default_sample_configs(),
      .target = std::move(target_config),
  }};

  ASSERT_TRUE(client
                  ->Configure({{
                      .output = std::move(outgoing_socket),
                      .config = std::move(config),
                  }})
                  .is_ok());

  SocketReader reader(std::move(in_socket));
  ASSERT_TRUE(client->Start({{.buffer_results = true}}).is_ok());

  // Launch the thread spawner process after starting
  zx::process process;
  const char* kArgs[] = {"/pkg/bin/thread_spawner", nullptr};

  ASSERT_EQ(ZX_OK, fdio_spawn(self->get(), FDIO_SPAWN_CLONE_ALL, "/pkg/bin/thread_spawner", kArgs,
                              process.reset_and_get_address()));
  reader.ReadUntilRecordCount(11, 1, 11);

  auto stop_response = client->Stop();
  ASSERT_TRUE(stop_response.is_ok());
  ASSERT_TRUE(stop_response.value().samples_collected().has_value());
  ASSERT_GT(stop_response.value().samples_collected().value(), size_t{10});

  ASSERT_TRUE(client->Reset().is_ok());

  // We should have many sampled threads
  EXPECT_GT(reader.tids().size(), size_t{10});

  process.kill();
}

// Monitor a component via moniker. Since we're running in the test realm, we only have access to
// our children components.
TEST(ProfilerIntegrationTest, ComponentByMoniker) {
  zx::result client_end = component::Connect<fprofiler::Session>();
  ASSERT_TRUE(client_end.is_ok());
  const fidl::SyncClient client{std::move(*client_end)};

  zx::socket in_socket;
  zx::socket outgoing_socket;

  ASSERT_EQ(zx::socket::create(0u, &in_socket, &outgoing_socket), ZX_OK);

  fprofiler::TargetConfig target_config = fprofiler::TargetConfig::WithComponent(
      fprofiler::AttachConfig::WithAttachToComponentMoniker("demo_target"));

  fprofiler::Config demo_target_config{{
      .configs = default_sample_configs(),
      .target = std::move(target_config),
  }};

  ASSERT_TRUE(client
                  ->Configure({{
                      .output = std::move(outgoing_socket),
                      .config = std::move(demo_target_config),
                  }})
                  .is_ok());
  SocketReader reader(std::move(in_socket));
  ASSERT_TRUE(client->Start({{.buffer_results = true}}).is_ok());
  reader.ReadUntilRecordCount(11, 1, 1);

  auto stop_response = client->Stop();
  ASSERT_TRUE(stop_response.is_ok());
  ASSERT_TRUE(stop_response.value().samples_collected().has_value());
  ASSERT_GT(stop_response.value().samples_collected().value(), size_t{10});

  ASSERT_TRUE(client->Reset().is_ok());

  // We should have only one thread and one process
  EXPECT_EQ(reader.tids().size(), size_t{1});
  EXPECT_EQ(reader.pids().size(), size_t{1});
}

TEST(ProfilerIntegrationTest, LaunchedComponent) {
  zx::result client_end = component::Connect<fprofiler::Session>();
  ASSERT_TRUE(client_end.is_ok());
  const fidl::SyncClient client{std::move(*client_end)};

  zx::socket in_socket, outgoing_socket;
  ASSERT_EQ(zx::socket::create(0u, &in_socket, &outgoing_socket), ZX_OK);

  fprofiler::TargetConfig target_config =
      fprofiler::TargetConfig::WithComponent(fprofiler::AttachConfig::WithLaunchComponent({{
          .url = "demo_target#meta/demo_target.cm",
          .moniker = "./launchpad:demo_target",
      }}));

  fprofiler::Config config{{
      .configs = default_sample_configs(),
      .target = std::move(target_config),
  }};

  ASSERT_TRUE(client
                  ->Configure({{
                      .output = std::move(outgoing_socket),
                      .config = std::move(config),
                  }})
                  .is_ok());
  SocketReader reader(std::move(in_socket));
  ASSERT_TRUE(client->Start({{.buffer_results = true}}).is_ok());
  reader.ReadUntilRecordCount(11);

  auto stop_response = client->Stop();
  ASSERT_TRUE(stop_response.is_ok());
  ASSERT_TRUE(stop_response.value().samples_collected().has_value());
  EXPECT_GT(stop_response.value().samples_collected().value(), size_t{10});

  ASSERT_TRUE(client->Reset().is_ok());
}

TEST(ProfilerIntegrationTest, ChildComponents) {
  zx::result client_end = component::Connect<fprofiler::Session>();
  ASSERT_TRUE(client_end.is_ok());
  const fidl::SyncClient client{std::move(*client_end)};

  zx::socket in_socket, outgoing_socket;
  ASSERT_EQ(zx::socket::create(0u, &in_socket, &outgoing_socket), ZX_OK);

  fprofiler::TargetConfig target_config =
      fprofiler::TargetConfig::WithComponent(fprofiler::AttachConfig::WithLaunchComponent({{
          .url = "component_with_children#meta/component_with_children.cm",
          .moniker = "./launchpad:component_with_children",
      }}));

  fprofiler::Config config{{
      .configs = default_sample_configs(),
      .target = std::move(target_config),
  }};

  ASSERT_TRUE(client
                  ->Configure({{
                      .output = std::move(outgoing_socket),
                      .config = std::move(config),
                  }})
                  .is_ok());

  SocketReader reader(std::move(in_socket));
  ASSERT_TRUE(client->Start({{.buffer_results = true}}).is_ok());
  reader.ReadUntilRecordCount(11, 4, 4);

  auto stop_response = client->Stop();
  ASSERT_TRUE(stop_response.is_ok());
  ASSERT_TRUE(stop_response.value().samples_collected().has_value());
  EXPECT_GT(stop_response.value().samples_collected().value(), size_t{10});

  ASSERT_TRUE(client->Reset().is_ok());

  // We should see 4 different pids and tids
  EXPECT_EQ(reader.tids().size(), size_t{4});
  EXPECT_EQ(reader.pids().size(), size_t{4});
}

TEST(ProfilerIntegrationTest, ChildComponentsByMoniker) {
  // Create and launch a component to attach to
  auto lifecycle_client_end = component::Connect<fuchsia_sys2::LifecycleController>();
  ASSERT_TRUE(lifecycle_client_end.is_ok());

  fidl::SyncClient lifecycle_client{std::move(*lifecycle_client_end)};

  const std::string name = "component_with_children";
  const std::string url = "component_with_children#meta/component_with_children.cm";
  const std::string moniker = "./launchpad:" + name;
  ASSERT_TRUE(RunInstance(lifecycle_client, name, url, moniker).is_ok());

  zx::result client_end = component::Connect<fprofiler::Session>();
  ASSERT_TRUE(client_end.is_ok());
  const fidl::SyncClient client{std::move(*client_end)};

  zx::socket in_socket, outgoing_socket;
  ASSERT_EQ(zx::socket::create(0u, &in_socket, &outgoing_socket), ZX_OK);

  fprofiler::TargetConfig target_config = fprofiler::TargetConfig::WithComponent(
      fprofiler::AttachConfig::WithAttachToComponentMoniker(moniker));

  fprofiler::Config config{{
      .configs = default_sample_configs(),
      .target = std::move(target_config),
  }};

  ASSERT_TRUE(client
                  ->Configure({{
                      .output = std::move(outgoing_socket),
                      .config = std::move(config),
                  }})
                  .is_ok());

  SocketReader reader(std::move(in_socket));
  ASSERT_TRUE(client->Start({{.buffer_results = true}}).is_ok());
  reader.ReadUntilRecordCount(11, 4, 4);

  auto stop_response = client->Stop();
  ASSERT_TRUE(stop_response.is_ok());
  ASSERT_TRUE(stop_response.value().samples_collected().has_value());
  EXPECT_GT(stop_response.value().samples_collected().value(), size_t{10});

  ASSERT_TRUE(client->Reset().is_ok());

  // We should see 4 different pids and tids
  EXPECT_EQ(reader.tids().size(), size_t{4});
  EXPECT_EQ(reader.pids().size(), size_t{4});
  ASSERT_TRUE(TearDownInstance(lifecycle_client, name, moniker).is_ok());
}

TEST(ProfilerIntegrationTest, DelayedConnectByMoniker) {
  // Start profiling targeting a moniker and check that we attach if it's launched after profiling
  // started
  zx::result client_end = component::Connect<fprofiler::Session>();
  ASSERT_TRUE(client_end.is_ok());
  const fidl::SyncClient client{std::move(*client_end)};

  zx::socket in_socket, outgoing_socket;
  ASSERT_EQ(zx::socket::create(0u, &in_socket, &outgoing_socket), ZX_OK);

  const std::string name = "demo_target";
  const std::string url = "demo_target#meta/demo_target.cm";
  const std::string moniker = "./launchpad:demo_target";

  fprofiler::TargetConfig target_config = fprofiler::TargetConfig::WithComponent(
      fprofiler::AttachConfig::WithAttachToComponentMoniker(moniker));

  fprofiler::Config config{{
      .configs = default_sample_configs(),
      .target = std::move(target_config),
  }};

  ASSERT_TRUE(client
                  ->Configure({{
                      .output = std::move(outgoing_socket),
                      .config = std::move(config),
                  }})
                  .is_ok());
  SocketReader reader(std::move(in_socket));
  ASSERT_TRUE(client->Start({{.buffer_results = true}}).is_ok());

  auto lifecycle_client_end = component::Connect<fuchsia_sys2::LifecycleController>();
  ASSERT_TRUE(lifecycle_client_end.is_ok());
  fidl::SyncClient lifecycle_client{std::move(*lifecycle_client_end)};

  ASSERT_TRUE(RunInstance(lifecycle_client, name, url, moniker).is_ok());

  reader.ReadUntilRecordCount(11, 1, 1);

  auto stop_response = client->Stop();
  ASSERT_TRUE(stop_response.is_ok());
  ASSERT_TRUE(stop_response.value().samples_collected().has_value());
  EXPECT_GT(stop_response.value().samples_collected().value(), size_t{10});

  ASSERT_TRUE(client->Reset().is_ok());

  // We should see 1 pid and tid from the demo target
  EXPECT_EQ(reader.tids().size(), size_t{1});
  EXPECT_EQ(reader.pids().size(), size_t{1});
  ASSERT_TRUE(TearDownInstance(lifecycle_client, name, moniker).is_ok());
}

TEST(ProfilerIntegrationTest, DelayedConnectByUrl) {
  // Start profiling targeting a moniker and check that we attach if it's launched after profiling
  // started

  zx::result client_end = component::Connect<fprofiler::Session>();
  ASSERT_TRUE(client_end.is_ok());
  const fidl::SyncClient client{std::move(*client_end)};

  zx::socket in_socket, outgoing_socket;
  ASSERT_EQ(zx::socket::create(0u, &in_socket, &outgoing_socket), ZX_OK);
  const std::string name = "demo_target";
  const std::string url = "demo_target#meta/demo_target.cm";
  const std::string moniker = "./launchpad:demo_target";

  fprofiler::TargetConfig target_config = fprofiler::TargetConfig::WithComponent(
      fprofiler::AttachConfig::WithAttachToComponentUrl(url));

  fprofiler::Config config{{
      .configs = default_sample_configs(),
      .target = std::move(target_config),
  }};

  ASSERT_TRUE(client
                  ->Configure({{
                      .output = std::move(outgoing_socket),
                      .config = std::move(config),
                  }})
                  .is_ok());
  SocketReader reader(std::move(in_socket));
  ASSERT_TRUE(client->Start({{.buffer_results = true}}).is_ok());

  auto lifecycle_client_end = component::Connect<fuchsia_sys2::LifecycleController>();
  ASSERT_TRUE(lifecycle_client_end.is_ok());
  fidl::SyncClient lifecycle_client{std::move(*lifecycle_client_end)};

  ASSERT_TRUE(RunInstance(lifecycle_client, name, url, moniker).is_ok());

  reader.ReadUntilRecordCount(11, 1, 1);

  auto stop_response = client->Stop();
  ASSERT_TRUE(stop_response.is_ok());
  ASSERT_TRUE(stop_response.value().samples_collected().has_value());
  EXPECT_GT(stop_response.value().samples_collected().value(), size_t{10});

  ASSERT_TRUE(client->Reset().is_ok());

  // We should see 1 pid and tid from the demo target
  EXPECT_EQ(reader.tids().size(), size_t{1});
  EXPECT_EQ(reader.pids().size(), size_t{1});
  ASSERT_TRUE(TearDownInstance(lifecycle_client, name, moniker).is_ok());
}

// If a process exits from underneath us, we should still be able to return the samples we got
TEST(ProfilerIntegrationTest, ExitedProcess) {
  auto lifecycle_client_end = component::Connect<fuchsia_sys2::LifecycleController>();
  ASSERT_TRUE(lifecycle_client_end.is_ok());
  const fidl::SyncClient lifecycle_client{std::move(*lifecycle_client_end)};

  zx::result client_end = component::Connect<fprofiler::Session>();
  ASSERT_TRUE(client_end.is_ok());
  const fidl::SyncClient client{std::move(*client_end)};

  zx::socket in_socket, outgoing_socket;
  ASSERT_EQ(zx::socket::create(0u, &in_socket, &outgoing_socket), ZX_OK);

  const std::string name = "demo_target";
  const std::string url = "demo_target#meta/demo_target.cm";
  const std::string moniker = "./launchpad:demo_target";

  // TODO(https://fxbug.dev/476409475): Fix the profiler to handle the case where
  // a process starts between configuring and starting without race conditions.
  ASSERT_TRUE(RunInstance(lifecycle_client, name, url, moniker).is_ok());

  fprofiler::TargetConfig target_config = fprofiler::TargetConfig::WithComponent(
      fprofiler::AttachConfig::WithAttachToComponentMoniker(moniker));

  ASSERT_TRUE(client
                  ->Configure({{.output = std::move(outgoing_socket),
                                .config = fprofiler::Config{{
                                    .configs = default_sample_configs(),
                                    .target = std::move(target_config),
                                }}}})
                  .is_ok());

  SocketReader reader(std::move(in_socket));
  ASSERT_TRUE(client->Start({{.buffer_results = true}}).is_ok());

  reader.ReadUntilRecordCount(11, 1, 1);

  // Destroy the target before the profiler stops
  ASSERT_TRUE(TearDownInstance(lifecycle_client, name, moniker).is_ok());

  auto stop_response = client->Stop();
  if (stop_response.is_error()) {
    FX_LOGS(INFO) << "Stop response: " << stop_response.error_value();
  }
  ASSERT_TRUE(stop_response.is_ok());
  ASSERT_TRUE(stop_response.value().samples_collected().has_value());
  EXPECT_GT(stop_response.value().samples_collected().value(), size_t{10});

  // We should see 1 pid and tid from the demo target.
  EXPECT_EQ(reader.tids().size(), size_t{1});
  EXPECT_EQ(reader.pids().size(), size_t{1});

  // We should still see the mapping we eagerly pulled.
  EXPECT_EQ(stop_response->missing_process_mappings()->size(), size_t{0});

  ASSERT_TRUE(client->Reset().is_ok());
}

// If a process exits from underneath us, we should still be able to return the samples we got
//
// Attaching while a a session is in progress goes down a different path, so test that too here.
TEST(ProfilerIntegrationTest, ExitedProcessLateAttach) {
  auto lifecycle_client_end = component::Connect<fuchsia_sys2::LifecycleController>();
  ASSERT_TRUE(lifecycle_client_end.is_ok());
  const fidl::SyncClient lifecycle_client{std::move(*lifecycle_client_end)};

  zx::result client_end = component::Connect<fprofiler::Session>();
  ASSERT_TRUE(client_end.is_ok());
  const fidl::SyncClient client{std::move(*client_end)};

  zx::socket in_socket, outgoing_socket;
  ASSERT_EQ(zx::socket::create(0u, &in_socket, &outgoing_socket), ZX_OK);

  const std::string name = "demo_target";
  const std::string url = "demo_target#meta/demo_target.cm";
  const std::string moniker = "./launchpad:demo_target";

  fprofiler::TargetConfig target_config = fprofiler::TargetConfig::WithComponent(
      fprofiler::AttachConfig::WithAttachToComponentMoniker(moniker));

  ASSERT_TRUE(client
                  ->Configure({{.output = std::move(outgoing_socket),
                                .config = fprofiler::Config{{
                                    .configs = default_sample_configs(),
                                    .target = std::move(target_config),
                                }}}})
                  .is_ok());

  SocketReader reader(std::move(in_socket));
  ASSERT_TRUE(client->Start({{.buffer_results = true}}).is_ok());

  ASSERT_TRUE(RunInstance(lifecycle_client, name, url, moniker).is_ok());
  reader.ReadUntilRecordCount(11, 1, 1);

  // Destroy the target before the profiler stops
  ASSERT_TRUE(TearDownInstance(lifecycle_client, name, moniker).is_ok());

  auto stop_response = client->Stop();
  if (stop_response.is_error()) {
    FX_LOGS(INFO) << "Stop response: " << stop_response.error_value();
  }
  ASSERT_TRUE(stop_response.is_ok());
  ASSERT_TRUE(stop_response.value().samples_collected().has_value());
  EXPECT_GT(stop_response.value().samples_collected().value(), size_t{10});

  // We should see 1 pid and tid from the demo target.
  EXPECT_EQ(reader.tids().size(), size_t{1});
  EXPECT_EQ(reader.pids().size(), size_t{1});

  // We should still see the mapping we eagerly pulled.
  EXPECT_EQ(stop_response->missing_process_mappings()->size(), size_t{0});

  ASSERT_TRUE(client->Reset().is_ok());
}

TEST(ProfilerIntegrationTest, ExitedAfterConfigure) {
  auto lifecycle_client_end = component::Connect<fuchsia_sys2::LifecycleController>();
  ASSERT_TRUE(lifecycle_client_end.is_ok());
  const fidl::SyncClient lifecycle_client{std::move(*lifecycle_client_end)};

  zx::result client_end = component::Connect<fprofiler::Session>();
  ASSERT_TRUE(client_end.is_ok());
  const fidl::SyncClient client{std::move(*client_end)};

  zx::socket in_socket, outgoing_socket;
  ASSERT_EQ(zx::socket::create(0u, &in_socket, &outgoing_socket), ZX_OK);

  const std::string name = "demo_target";
  const std::string url = "demo_target#meta/demo_target.cm";
  const std::string moniker = "./launchpad:demo_target";

  fprofiler::TargetConfig target_config = fprofiler::TargetConfig::WithComponent(
      fprofiler::AttachConfig::WithAttachToComponentMoniker(moniker));

  ASSERT_TRUE(RunInstance(lifecycle_client, name, url, moniker).is_ok());
  ASSERT_TRUE(client
                  ->Configure({{.output = std::move(outgoing_socket),
                                .config = fprofiler::Config{{
                                    .configs = default_sample_configs(),
                                    .target = std::move(target_config),
                                }}}})
                  .is_ok());
  // Destroy the instance after configuring, but before starting. We should still be able to run the
  // profiler, though we may not get any samples.
  ASSERT_TRUE(TearDownInstance(lifecycle_client, name, moniker).is_ok());
  SocketReader reader(std::move(in_socket));
  ASSERT_TRUE(client->Start({{.buffer_results = true}}).is_ok());
  ASSERT_TRUE(client->Stop().is_ok());
  ASSERT_TRUE(client->Reset().is_ok());
}

TEST(ProfilerIntegrationTest, StackSampling) {
  zx::result client_end = component::Connect<fprofiler::Session>();
  ASSERT_TRUE(client_end.is_ok());
  const fidl::SyncClient client{std::move(*client_end)};

  zx::socket in_socket;
  zx::socket outgoing_socket;

  ASSERT_EQ(zx::socket::create(0u, &in_socket, &outgoing_socket), ZX_OK);
  zx::process self;
  zx::process::self()->duplicate(ZX_RIGHT_SAME_RIGHTS, &self);

  std::thread child(MakeWork);
  const zx::unowned_thread child_handle{native_thread_get_zx_handle(child.native_handle())};
  child.detach();

  zx_status_t res =
      child_handle->wait_one(ZX_THREAD_RUNNING, zx::deadline_after(zx::sec(1)), nullptr);
  ASSERT_EQ(ZX_OK, res);

  zx_info_handle_basic_t info;
  res = child_handle->get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  ASSERT_EQ(ZX_OK, res);

  fprofiler::TargetConfig target_config =
      fprofiler::TargetConfig::WithTasks(std::vector{fprofiler::Task::WithThread(info.koid)});
  fprofiler::Config config{{
      .configs = dwarf_sample_configs(),
      .target = std::move(target_config),
  }};

  ASSERT_TRUE(client
                  ->Configure({{
                      .output = std::move(outgoing_socket),
                      .config = std::move(config),
                  }})
                  .is_ok());

  SocketReader reader(std::move(in_socket));
  ASSERT_TRUE(client->Start({{.buffer_results = true}}).is_ok());
  reader.ReadUntilRecordCount(11, 1, 1);

  auto stop_response = client->Stop();
  ASSERT_TRUE(stop_response.is_ok());
  ASSERT_TRUE(stop_response.value().samples_collected().has_value());
  EXPECT_GT(stop_response.value().samples_collected().value(), size_t{10});

  EXPECT_EQ(reader.tids().size(), size_t{1});
  EXPECT_EQ(reader.pids().size(), size_t{1});

  ASSERT_TRUE(client->Reset().is_ok());
}
