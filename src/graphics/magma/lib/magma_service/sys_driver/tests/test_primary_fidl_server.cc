// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/magma/platform/platform_connection_client.h>
#include <lib/magma/platform/platform_handle.h>
#include <lib/magma/platform/platform_port.h>
#include <lib/magma/platform/platform_semaphore.h>
#include <lib/magma_service/sys_driver/primary_fidl_server.h>
#include <lib/sync/cpp/completion.h>
#include <poll.h>

#include <chrono>
#include <mutex>
#include <thread>
#include <utility>

#include <gtest/gtest.h>

#include "fidl/fuchsia.gpu.magma/cpp/wire_types.h"
#include "lib/magma/magma_common_defs.h"

#if defined(__Fuchsia__)
#include <lib/magma/platform/zircon/zircon_platform_connection_client.h>  // nogncheck
#elif defined(__linux__)
#include "linux/linux_platform_connection_client.h"  // nogncheck
#endif

namespace msd {
namespace {
constexpr uint32_t kImmediateCommandCount = 128;
// The total size of all commands should not be a multiple of the receive buffer size.
constexpr uint32_t kImmediateCommandSize = 2048 * 3 / 2 / kImmediateCommandCount;

constexpr uint32_t kNotificationCount = 2;
constexpr uint32_t kNotificationData = 5;

static inline int64_t page_size() { return sysconf(_SC_PAGESIZE); }

}  // namespace

// Included by TestPlatformConnection; validates that each test checks for flow control.
// Since flow control values are written by the server (IPC) thread and read by the main
// test thread, we lock the shared data mutex to ensure safety of memory accesses.
class FlowControlChecker {
 public:
  FlowControlChecker(std::shared_ptr<msd::internal::PrimaryFidlServer> connection,
                     std::shared_ptr<magma::PlatformConnectionClient> client_connection)
      : connection_(connection), client_connection_(client_connection) {}

  ~FlowControlChecker() {
    if (!flow_control_skipped_) {
      EXPECT_TRUE(flow_control_checked_);
    }
  }

  void Init(std::mutex& mutex) {
    std::unique_lock<std::mutex> lock(mutex);
    auto locked_connection = connection_.lock();
    ASSERT_TRUE(locked_connection);
    std::tie(messages_consumed_start_, bytes_imported_start_) =
        locked_connection->GetFlowControlCounts();
    std::tie(messages_inflight_start_, bytes_inflight_start_) =
        client_connection_->GetFlowControlCounts();
  }

  void Release() {
    connection_.reset();
    client_connection_.reset();
  }

  void Check(uint64_t messages, uint64_t bytes, std::mutex& mutex) {
    std::unique_lock<std::mutex> lock(mutex);

    auto locked_connection = connection_.lock();
    ASSERT_TRUE(locked_connection);
    auto [messages_consumed, bytes_imported] = locked_connection->GetFlowControlCounts();
    EXPECT_EQ(messages_consumed_start_ + messages, messages_consumed);
    EXPECT_EQ(bytes_imported_start_ + bytes, bytes_imported);

    auto [messages_inflight, bytes_inflight] = client_connection_->GetFlowControlCounts();
    EXPECT_EQ(messages_inflight_start_ + messages, messages_inflight);
    EXPECT_EQ(bytes_inflight_start_ + bytes, bytes_inflight);
    flow_control_checked_ = true;
  }

  void Skip() {
    flow_control_skipped_ = true;
    Release();
  }

  std::weak_ptr<msd::internal::PrimaryFidlServer> connection_;
  std::shared_ptr<magma::PlatformConnectionClient> client_connection_;
  bool flow_control_checked_ = false;
  bool flow_control_skipped_ = false;
  // Server
  uint64_t messages_consumed_start_ = 0;
  uint64_t bytes_imported_start_ = 0;
  // Client
  uint64_t messages_inflight_start_ = 0;
  uint64_t bytes_inflight_start_ = 0;
};

struct SharedData {
  // This mutex is used to ensure safety of multi-threaded updates.
  std::mutex mutex;
  uint64_t test_buffer_id = 0xcafecafecafecafe;
  uint32_t test_context_id = 0xdeadbeef;
  uint64_t test_semaphore_id = ~0u;
  bool got_null_notification = false;
  bool is_trusted = false;
  magma_status_t test_error = 0x12345678;
  bool test_complete = false;
  std::unique_ptr<magma::PlatformSemaphore> test_semaphore;
  std::vector<magma_exec_resource> test_resources = {{.buffer_id = 10, .offset = 11, .length = 12},
                                                     {.buffer_id = 13, .offset = 14, .length = 15}};
  std::vector<uint64_t> test_wait_semaphores = {{1000, 1001}};
  std::vector<uint64_t> test_signal_semaphores = {{1010, 1011, 1012}};
  std::vector<magma_exec_command_buffer> test_command_buffers = {{
      .resource_index = 2,
      .start_offset = 4,
  }};
  zx::handle test_access_token;
  bool can_access_performance_counters;
  uint64_t pool_id = UINT64_MAX;
  std::function<void(msd::NotificationHandler*)> notification_handler;
  // Flow control defaults should avoid tests hitting flow control
  uint64_t max_inflight_messages = 1000u;
  uint64_t max_inflight_bytes = 1000000u;
  libsync::Completion notification_handler_initialization_complete;
};

// Most tests here execute the client commands in the test thread context,
// with a separate server thread processing the commands.
class TestPlatformConnection {
 public:
  static std::unique_ptr<TestPlatformConnection> Create(
      std::shared_ptr<SharedData> shared_data = std::make_shared<SharedData>());

  TestPlatformConnection(std::shared_ptr<magma::PlatformConnectionClient> client_connection,
                         std::shared_ptr<msd::internal::PrimaryFidlServerHolder> server_holder,
                         std::shared_ptr<SharedData> shared_data)
      : client_connection_(client_connection),
        server_holder_(std::move(server_holder)),
        flow_control_checker_(server_holder_->server_for_test(), client_connection),
        shared_data_(shared_data) {}

  ~TestPlatformConnection() {
    flow_control_checker_.Release();
    client_connection_.reset();
    if (server_holder_) {
      server_holder_->Shutdown();
    }

    EXPECT_TRUE(shared_data_->test_complete);
  }

  // Should be called after any shared data initialization.
  void FlowControlInit() { flow_control_checker_.Init(shared_data_->mutex); }

  // Should be called before test checks for shared data writes.
  void FlowControlCheck(uint64_t messages, uint64_t bytes) {
    flow_control_checker_.Check(messages, bytes, shared_data_->mutex);
  }

  void FlowControlCheckOneMessage() { FlowControlCheck(1, 0); }
  void FlowControlSkip() { flow_control_checker_.Skip(); }

  void TestImportBufferDeprecated() {
    auto buf = magma::PlatformBuffer::Create(page_size() * 3, "test");
    shared_data_->test_buffer_id = buf->id();
    FlowControlInit();

    uint32_t handle;
    EXPECT_TRUE(buf->duplicate_handle(&handle));
    EXPECT_EQ(client_connection_->ImportObject(handle, /*flags=*/0, magma::PlatformObject::BUFFER,
                                               buf->id()),
              MAGMA_STATUS_OK);
    EXPECT_EQ(client_connection_->Flush(), MAGMA_STATUS_OK);
    FlowControlCheck(1, buf->size());
  }

  void TestImportBuffer() {
    auto buf = magma::PlatformBuffer::Create(page_size() * 3, "test");
    shared_data_->test_buffer_id = buf->id();
    FlowControlInit();

    uint32_t handle;
    EXPECT_TRUE(buf->duplicate_handle(&handle));
    EXPECT_EQ(client_connection_->ImportObject(handle, /*flags=*/0, magma::PlatformObject::BUFFER,
                                               buf->id()),
              MAGMA_STATUS_OK);
    EXPECT_EQ(client_connection_->Flush(), MAGMA_STATUS_OK);
    FlowControlCheck(1, buf->size());
  }

  void TestReleaseBuffer() {
    auto buf = magma::PlatformBuffer::Create(1, "test");
    shared_data_->test_buffer_id = buf->id();
    FlowControlInit();

    uint32_t handle;
    EXPECT_TRUE(buf->duplicate_handle(&handle));
    EXPECT_EQ(client_connection_->ImportObject(handle, /*flags=*/0, magma::PlatformObject::BUFFER,
                                               buf->id()),
              MAGMA_STATUS_OK);
    EXPECT_EQ(client_connection_->ReleaseObject(shared_data_->test_buffer_id,
                                                magma::PlatformObject::BUFFER),
              MAGMA_STATUS_OK);
    EXPECT_EQ(client_connection_->Flush(), MAGMA_STATUS_OK);
    FlowControlCheck(2, buf->size());
  }

  void TestImportSemaphoreDeprecated() {
    auto semaphore = magma::PlatformSemaphore::Create();
    ASSERT_TRUE(semaphore);
    shared_data_->test_semaphore_id = semaphore->id();
    FlowControlInit();

    uint32_t handle;
    EXPECT_TRUE(semaphore->duplicate_handle(&handle));
    EXPECT_EQ(client_connection_->ImportObject(handle, /*flags=*/0,
                                               magma::PlatformObject::SEMAPHORE, semaphore->id()),
              MAGMA_STATUS_OK);
    EXPECT_EQ(client_connection_->Flush(), MAGMA_STATUS_OK);
    FlowControlCheckOneMessage();
  }

  void TestImportSemaphore() {
    auto semaphore = magma::PlatformSemaphore::Create();
    ASSERT_TRUE(semaphore);
    shared_data_->test_semaphore_id = semaphore->id();
    FlowControlInit();

    uint32_t handle;
    EXPECT_TRUE(semaphore->duplicate_handle(&handle));
    EXPECT_EQ(client_connection_->ImportObject(handle, /*flags=*/0,
                                               magma::PlatformObject::SEMAPHORE, semaphore->id()),
              MAGMA_STATUS_OK);
    EXPECT_EQ(client_connection_->Flush(), MAGMA_STATUS_OK);
    FlowControlCheckOneMessage();
  }

  void TestReleaseSemaphore() {
    auto semaphore = magma::PlatformSemaphore::Create();
    ASSERT_TRUE(semaphore);
    shared_data_->test_semaphore_id = semaphore->id();
    FlowControlInit();

    uint32_t handle;
    EXPECT_TRUE(semaphore->duplicate_handle(&handle));
    EXPECT_EQ(client_connection_->ImportObject(handle, /*flags=*/0,
                                               magma::PlatformObject::SEMAPHORE, semaphore->id()),
              MAGMA_STATUS_OK);
    EXPECT_EQ(client_connection_->ReleaseObject(shared_data_->test_semaphore_id,
                                                magma::PlatformObject::SEMAPHORE),
              MAGMA_STATUS_OK);
    EXPECT_EQ(client_connection_->Flush(), MAGMA_STATUS_OK);
    FlowControlCheck(2, 0);
  }

  void TestCreateContext() {
    FlowControlInit();
    uint32_t context_id;
    client_connection_->CreateContext(&context_id);
    EXPECT_EQ(client_connection_->Flush(), MAGMA_STATUS_OK);
    FlowControlCheckOneMessage();
    EXPECT_EQ(shared_data_->test_context_id, context_id);
  }

  void TestCreateContext2() {
    FlowControlInit();
    uint32_t context_id;
    client_connection_->CreateContext2(&context_id, MAGMA_PRIORITY_MEDIUM);
    EXPECT_EQ(client_connection_->Flush(), MAGMA_STATUS_OK);
    FlowControlCheckOneMessage();
    EXPECT_EQ(shared_data_->test_context_id, context_id);
  }

  void TestCreateContext2HighPriorityTrusted() {
    FlowControlInit();
    uint32_t context_id;
    client_connection_->CreateContext2(&context_id, MAGMA_PRIORITY_HIGH);
    EXPECT_EQ(client_connection_->Flush(), MAGMA_STATUS_OK);
    FlowControlCheckOneMessage();
  }

  void TestCreateContext2HighPriorityUntrusted() {
    FlowControlSkip();
    uint32_t context_id;
    client_connection_->CreateContext2(&context_id, MAGMA_PRIORITY_HIGH);
    ASSERT_EQ(client_connection_->Flush(), MAGMA_STATUS_ACCESS_DENIED);
    shared_data_->test_complete = true;
  }

  void TestDestroyContext() {
    FlowControlInit();
    client_connection_->DestroyContext(shared_data_->test_context_id);
    EXPECT_EQ(client_connection_->Flush(), MAGMA_STATUS_OK);
    FlowControlCheckOneMessage();
  }

  void TestGetError() {
    FlowControlSkip();
    EXPECT_EQ(client_connection_->GetError(), MAGMA_STATUS_OK);
    shared_data_->test_complete = true;
  }

  void TestFlush() {
    constexpr uint64_t kNumMessages = 10;
    uint32_t context_id;
    for (uint32_t i = 0; i < kNumMessages; i++) {
      client_connection_->CreateContext(&context_id);
    }
    EXPECT_EQ(client_connection_->Flush(), MAGMA_STATUS_OK);
    FlowControlCheck(kNumMessages, 0);
    EXPECT_EQ(shared_data_->test_context_id, context_id);
  }

  void TestMapUnmapBuffer() {
    auto buf = magma::PlatformBuffer::Create(1, "test");
    shared_data_->test_buffer_id = buf->id();
    FlowControlInit();

    uint32_t handle;
    EXPECT_TRUE(buf->duplicate_handle(&handle));
    EXPECT_EQ(client_connection_->ImportObject(handle, /*flags=*/0, magma::PlatformObject::BUFFER,
                                               buf->id()),
              MAGMA_STATUS_OK);
    EXPECT_EQ(client_connection_->MapBuffer(buf->id(), /*address=*/page_size() * 1000,
                                            /*offset=*/1u * page_size(),
                                            /*length=*/2u * page_size(), /*flags=*/5),
              MAGMA_STATUS_OK);
    EXPECT_EQ(client_connection_->UnmapBuffer(buf->id(), page_size() * 1000), MAGMA_STATUS_OK);
    EXPECT_EQ(client_connection_->BufferRangeOp(buf->id(), MAGMA_BUFFER_RANGE_OP_POPULATE_TABLES,
                                                1000, 2000),
              MAGMA_STATUS_OK);
    EXPECT_EQ(client_connection_->BufferRangeOp(buf->id(), MAGMA_BUFFER_RANGE_OP_DEPOPULATE_TABLES,
                                                1000, 2000),
              MAGMA_STATUS_OK);
    EXPECT_EQ(client_connection_->Flush(), MAGMA_STATUS_OK);
    FlowControlCheck(5, buf->size());
  }

  void TestNotificationChannel() {
    FlowControlSkip();

    // Notification requests will be sent when the:PrimaryFidlServer is created, before this test is
    // called.
    shared_data_->notification_handler_initialization_complete.Wait();

    {
      uint8_t buffer_too_small;
      uint64_t out_data_size;
      magma_bool_t more_data;
      magma_status_t status = client_connection_->ReadNotificationChannel(
          &buffer_too_small, sizeof(buffer_too_small), &out_data_size, &more_data);
      EXPECT_EQ(MAGMA_STATUS_INVALID_ARGS, status);
    }

    uint32_t out_data;
    uint64_t out_data_size;
    magma_bool_t more_data = false;
    magma_status_t status = client_connection_->ReadNotificationChannel(&out_data, sizeof(out_data),
                                                                        &out_data_size, &more_data);
    EXPECT_EQ(MAGMA_STATUS_OK, status);
    EXPECT_EQ(sizeof(out_data), out_data_size);
    EXPECT_EQ(kNotificationData, out_data);
    EXPECT_EQ(true, more_data);

    status = client_connection_->ReadNotificationChannel(&out_data, sizeof(out_data),
                                                         &out_data_size, &more_data);
    EXPECT_EQ(MAGMA_STATUS_OK, status);
    EXPECT_EQ(sizeof(out_data), out_data_size);
    EXPECT_EQ(kNotificationData + 1, out_data);
    EXPECT_EQ(false, more_data);

    // No more data to read.
    status = client_connection_->ReadNotificationChannel(&out_data, sizeof(out_data),
                                                         &out_data_size, &more_data);
    EXPECT_EQ(MAGMA_STATUS_OK, status);
    EXPECT_EQ(0u, out_data_size);

    // Shutdown other end of pipe.
    server_holder_->Shutdown();
    server_holder_.reset();
    EXPECT_TRUE(shared_data_->got_null_notification);

    status = client_connection_->ReadNotificationChannel(&out_data, sizeof(out_data),
                                                         &out_data_size, &more_data);
    EXPECT_EQ(MAGMA_STATUS_CONNECTION_LOST, status);
    shared_data_->test_complete = true;
  }

  void TestExecuteInlineCommands() {
    uint64_t semaphore_ids[]{0, 1, 2};
    magma_inline_command_buffer commands[kImmediateCommandCount];

    for (size_t i = 0; i < kImmediateCommandCount; i++) {
      commands[i].data = malloc(kImmediateCommandSize);
      memset(commands[i].data, static_cast<uint8_t>(i), kImmediateCommandSize);
      commands[i].size = kImmediateCommandSize;
      commands[i].semaphore_count = 3;
      commands[i].semaphore_ids = semaphore_ids;
    }
    FlowControlInit();

    uint64_t messages_sent = 0;
    client_connection_->ExecuteInlineCommands(shared_data_->test_context_id, kImmediateCommandCount,
                                              commands, &messages_sent);
    EXPECT_EQ(client_connection_->Flush(), MAGMA_STATUS_OK);
    FlowControlCheck(messages_sent, 0);

    for (size_t i = 0; i < kImmediateCommandCount; i++) {
      free(commands[i].data);
    }
  }

  void TestMultipleFlush() {
    FlowControlSkip();

    std::vector<std::thread> threads;
    for (uint32_t i = 0; i < 1000; i++) {
      threads.push_back(
          std::thread([this]() { EXPECT_EQ(MAGMA_STATUS_OK, client_connection_->Flush()); }));
    }

    for (auto& thread : threads) {
      thread.join();
    }
    shared_data_->test_complete = true;
  }

  void TestEnablePerformanceCounters() {
    FlowControlSkip();

    bool enabled = false;
    EXPECT_EQ(MAGMA_STATUS_OK, client_connection_->IsPerformanceCounterAccessAllowed(&enabled));
    EXPECT_FALSE(enabled);

    {
      std::unique_lock<std::mutex> lock(shared_data_->mutex);
      shared_data_->can_access_performance_counters = true;
    }

    EXPECT_EQ(MAGMA_STATUS_OK, client_connection_->IsPerformanceCounterAccessAllowed(&enabled));
    EXPECT_TRUE(enabled);

    auto semaphore = magma::PlatformSemaphore::Create();
    uint32_t handle;
    EXPECT_TRUE(semaphore->duplicate_handle(&handle));
    EXPECT_EQ(MAGMA_STATUS_OK, client_connection_->EnablePerformanceCounterAccess(
                                   magma::PlatformHandle::Create(handle)));

    EXPECT_EQ(client_connection_->Flush(), MAGMA_STATUS_OK);

    {
      std::unique_lock<std::mutex> lock(shared_data_->mutex);
      zx_info_handle_basic_t handle_info{};
      shared_data_->test_access_token.get_info(ZX_INFO_HANDLE_BASIC, &handle_info,
                                               sizeof(handle_info), nullptr, nullptr);
      EXPECT_EQ(handle_info.koid, semaphore->id());
    }
  }

  void TestPerformanceCounters() {
    FlowControlInit();
    uint32_t trigger_id;
    uint64_t buffer_id;
    uint32_t buffer_offset;
    uint64_t time;
    uint32_t result_flags;
    uint64_t counter = 2;
    EXPECT_EQ(MAGMA_STATUS_OK, client_connection_->EnablePerformanceCounters(&counter, 1).get());
    std::unique_ptr<magma::PlatformPerfCountPoolClient> pool;
    EXPECT_EQ(MAGMA_STATUS_OK, client_connection_->CreatePerformanceCounterBufferPool(&pool).get());

    EXPECT_EQ(client_connection_->Flush(), MAGMA_STATUS_OK);

    // The Flush() above should wait until the performance counter completion event sent in
    // CreatePerformanceCounterBufferPool is sent and therefore readable.
    {
      std::lock_guard<std::mutex> lock(shared_data_->mutex);
      EXPECT_EQ(shared_data_->pool_id, pool->pool_id());
    }
    EXPECT_EQ(MAGMA_STATUS_OK,
              pool->ReadPerformanceCounterCompletion(&trigger_id, &buffer_id, &buffer_offset, &time,
                                                     &result_flags)
                  .get());
    EXPECT_EQ(1u, trigger_id);
    EXPECT_EQ(2u, buffer_id);
    EXPECT_EQ(3u, buffer_offset);
    EXPECT_EQ(4u, time);
    EXPECT_EQ(1u, result_flags);

    EXPECT_EQ(MAGMA_STATUS_OK, client_connection_->ReleasePerformanceCounterBufferPool(1).get());
    magma_buffer_offset offset = {2, 3, 4};
    EXPECT_EQ(MAGMA_STATUS_OK,
              client_connection_->AddPerformanceCounterBufferOffsetsToPool(1, &offset, 1).get());
    EXPECT_EQ(MAGMA_STATUS_OK,
              client_connection_->RemovePerformanceCounterBufferFromPool(1, 2).get());
    EXPECT_EQ(MAGMA_STATUS_OK, client_connection_->ClearPerformanceCounters(&counter, 1).get());
    EXPECT_EQ(MAGMA_STATUS_OK, client_connection_->DumpPerformanceCounters(1, 2).get());
    EXPECT_EQ(client_connection_->Flush(), MAGMA_STATUS_OK);

    // The CreatePerformanceCounterBufferPool implementation threw away the server side, so the
    // client should be able to detect that.
    EXPECT_EQ(MAGMA_STATUS_CONNECTION_LOST,
              pool->ReadPerformanceCounterCompletion(&trigger_id, &buffer_id, &buffer_offset, &time,
                                                     &result_flags)
                  .get());
    EXPECT_EQ(client_connection_->Flush(), MAGMA_STATUS_OK);
    FlowControlCheck(7, 0);
  }

 private:
  std::shared_ptr<magma::PlatformConnectionClient> client_connection_;
  std::shared_ptr<msd::internal::PrimaryFidlServerHolder> server_holder_;
  msd::FlowControlChecker flow_control_checker_;
  std::shared_ptr<SharedData> shared_data_;
};

class TestDelegate : public msd::internal::PrimaryFidlServer::Delegate {
 public:
  TestDelegate(std::shared_ptr<SharedData> shared_data) : shared_data_(shared_data) {}

  magma::Status ImportObject(zx::handle handle, uint64_t flags,
                             fuchsia_gpu_magma::wire::ObjectType object_type,
                             uint64_t object_id) override {
    std::unique_lock<std::mutex> lock(shared_data_->mutex);
    switch (object_type) {
      case fuchsia_gpu_magma::wire::ObjectType::kSemaphore: {
        auto semaphore = magma::PlatformSemaphore::Import(std::move(handle), flags);
        if (!semaphore)
          return MAGMA_STATUS_INVALID_ARGS;
        EXPECT_EQ(object_id, shared_data_->test_semaphore_id);
      } break;
      case fuchsia_gpu_magma::wire::ObjectType::kBuffer: {
        auto buffer = magma::PlatformBuffer::Import(zx::vmo(std::move(handle)));
        if (!buffer)
          return MAGMA_STATUS_INVALID_ARGS;
        EXPECT_EQ(object_id, shared_data_->test_buffer_id);
      } break;
      default:
        EXPECT_TRUE(false) << static_cast<uint32_t>(object_type);
    }
    shared_data_->test_complete = true;
    return MAGMA_STATUS_OK;
  }

  magma::Status ReleaseObject(uint64_t object_id,
                              fuchsia_gpu_magma::wire::ObjectType object_type) override {
    std::unique_lock<std::mutex> lock(shared_data_->mutex);
    switch (object_type) {
      case fuchsia_gpu_magma::wire::ObjectType::kSemaphore: {
        EXPECT_EQ(object_id, shared_data_->test_semaphore_id);
        break;
      }
      case fuchsia_gpu_magma::wire::ObjectType::kBuffer: {
        EXPECT_EQ(object_id, shared_data_->test_buffer_id);
        break;
      }
      default:
        EXPECT_TRUE(false);
    }
    shared_data_->test_complete = true;
    return MAGMA_STATUS_OK;
  }

  magma::Status CreateContext(uint32_t context_id) override {
    std::unique_lock<std::mutex> lock(shared_data_->mutex);
    shared_data_->test_context_id = context_id;
    shared_data_->test_complete = true;
    return MAGMA_STATUS_OK;
  }

  magma::Status CreateContext2(uint32_t context_id, uint64_t priority) override {
    std::unique_lock<std::mutex> lock(shared_data_->mutex);
    shared_data_->test_context_id = context_id;
    shared_data_->test_complete = true;
    return MAGMA_STATUS_OK;
  }

  magma::Status DestroyContext(uint32_t context_id) override {
    std::unique_lock<std::mutex> lock(shared_data_->mutex);
    EXPECT_EQ(context_id, shared_data_->test_context_id);
    shared_data_->test_complete = true;
    return MAGMA_STATUS_OK;
  }

  magma::Status ExecuteCommandBuffers(uint32_t context_id,
                                      std::vector<magma_exec_command_buffer>& command_buffers,
                                      std::vector<magma_exec_resource>& resources,
                                      std::vector<uint64_t>& wait_semaphore_ids,
                                      std::vector<uint64_t>& signal_semaphore_ids,
                                      uint64_t flags) override {
    std::unique_lock<std::mutex> lock(shared_data_->mutex);

    EXPECT_EQ(context_id, shared_data_->test_context_id);
    EXPECT_EQ(0, memcmp(command_buffers.data(), shared_data_->test_command_buffers.data(),
                        shared_data_->test_command_buffers.size() *
                            sizeof(shared_data_->test_command_buffers[0])));
    EXPECT_EQ(
        0, memcmp(resources.data(), shared_data_->test_resources.data(),
                  shared_data_->test_resources.size() * sizeof(shared_data_->test_resources[0])));
    EXPECT_EQ(0, memcmp(wait_semaphore_ids.data(), shared_data_->test_wait_semaphores.data(),
                        shared_data_->test_wait_semaphores.size() *
                            sizeof(shared_data_->test_wait_semaphores[0])));
    EXPECT_EQ(0, memcmp(signal_semaphore_ids.data(), shared_data_->test_signal_semaphores.data(),
                        shared_data_->test_signal_semaphores.size() *
                            sizeof(shared_data_->test_signal_semaphores[0])));
    shared_data_->test_complete = true;
    return MAGMA_STATUS_OK;
  }

  magma::Status MapBuffer(uint64_t buffer_id, uint64_t gpu_va, uint64_t offset, uint64_t length,
                          uint64_t flags) override {
    std::unique_lock<std::mutex> lock(shared_data_->mutex);
    EXPECT_EQ(shared_data_->test_buffer_id, buffer_id);
    EXPECT_EQ(page_size() * 1000lu, gpu_va);
    EXPECT_EQ(page_size() * 1lu, offset);
    EXPECT_EQ(page_size() * 2lu, length);
    EXPECT_EQ(5u, flags);
    return MAGMA_STATUS_OK;
  }

  magma::Status UnmapBuffer(uint64_t buffer_id, uint64_t gpu_va) override {
    std::unique_lock<std::mutex> lock(shared_data_->mutex);
    EXPECT_EQ(shared_data_->test_buffer_id, buffer_id);
    EXPECT_EQ(page_size() * 1000lu, gpu_va);
    return MAGMA_STATUS_OK;
  }

  void SetNotificationCallback(msd::NotificationHandler* handler) override {
    std::unique_lock<std::mutex> lock(shared_data_->mutex);

    if (!handler) {
      // This doesn't count as test complete because it should happen in every test when the
      // server shuts down.
      shared_data_->got_null_notification = true;
      return;
    }

    if (shared_data_->notification_handler) {
      shared_data_->notification_handler(handler);
    }

    shared_data_->notification_handler_initialization_complete.Signal();
  }

  magma::Status ExecuteInlineCommands(
      uint32_t context_id, std::vector<magma_inline_command_buffer_t> commands) override {
    std::unique_lock<std::mutex> lock(shared_data_->mutex);

    for (auto iter = commands.begin(); iter != commands.end(); iter++) {
      uint8_t index = static_cast<uint8_t>(immediate_commands_executed_ +
                                           (std::distance(commands.begin(), iter)));
      auto& command = *iter;
      EXPECT_EQ(kImmediateCommandSize, command.size);
      for (size_t i = 0; i < command.size; i++) {
        EXPECT_EQ(reinterpret_cast<uint8_t*>(command.data)[i], index);
      }
      EXPECT_EQ(3u, command.semaphore_count);
      EXPECT_EQ(0u, command.semaphore_ids[0]);
      EXPECT_EQ(1u, command.semaphore_ids[1]);
      EXPECT_EQ(2u, command.semaphore_ids[2]);
    }
    immediate_commands_executed_ += commands.size();
    shared_data_->test_complete = immediate_commands_executed_ == kImmediateCommandCount;

    // Also check thread name
    EXPECT_EQ("ConnectionThread 1", magma::PlatformThreadHelper::GetCurrentThreadName());

    return MAGMA_STATUS_OK;
  }

  magma::Status EnablePerformanceCounterAccess(zx::handle event) override {
    std::unique_lock<std::mutex> lock(shared_data_->mutex);
    shared_data_->test_access_token = std::move(event);
    shared_data_->test_complete = true;
    return MAGMA_STATUS_OK;
  }

  bool IsPerformanceCounterAccessAllowed() override {
    std::unique_lock<std::mutex> lock(shared_data_->mutex);
    return shared_data_->can_access_performance_counters;
  }

  magma::Status EnablePerformanceCounters(const uint64_t* counters,
                                          uint64_t counter_count) override {
    EXPECT_EQ(counter_count, 1u);
    EXPECT_EQ(2u, counters[0]);

    return MAGMA_STATUS_OK;
  }

  magma::Status CreatePerformanceCounterBufferPool(
      std::unique_ptr<msd::PerfCountPoolServer> pool) override {
    std::unique_lock<std::mutex> lock(shared_data_->mutex);
    shared_data_->pool_id = pool->pool_id();
    constexpr uint32_t kTriggerId = 1;
    constexpr uint64_t kBufferId = 2;
    constexpr uint32_t kBufferOffset = 3;
    constexpr uint64_t kTimestamp = 4;
    constexpr uint64_t kResultFlags = 1;

    EXPECT_EQ(MAGMA_STATUS_OK,
              pool->SendPerformanceCounterCompletion(kTriggerId, kBufferId, kBufferOffset,
                                                     kTimestamp, kResultFlags)
                  .get());
    return MAGMA_STATUS_OK;
  }

  magma::Status ReleasePerformanceCounterBufferPool(uint64_t pool_id) override {
    EXPECT_EQ(1u, pool_id);
    return MAGMA_STATUS_OK;
  }

  magma::Status AddPerformanceCounterBufferOffsetToPool(uint64_t pool_id, uint64_t buffer_id,
                                                        uint64_t buffer_offset,
                                                        uint64_t buffer_size) override {
    EXPECT_EQ(1u, pool_id);
    EXPECT_EQ(2u, buffer_id);
    EXPECT_EQ(3u, buffer_offset);
    EXPECT_EQ(4u, buffer_size);
    return MAGMA_STATUS_OK;
  }

  magma::Status RemovePerformanceCounterBufferFromPool(uint64_t pool_id,
                                                       uint64_t buffer_id) override {
    EXPECT_EQ(1u, pool_id);
    EXPECT_EQ(2u, buffer_id);
    return MAGMA_STATUS_OK;
  }

  magma::Status DumpPerformanceCounters(uint64_t pool_id, uint32_t trigger_id) override {
    EXPECT_EQ(1u, pool_id);
    EXPECT_EQ(2u, trigger_id);
    std::unique_lock<std::mutex> lock(shared_data_->mutex);
    shared_data_->test_complete = true;

    return MAGMA_STATUS_OK;
  }

  magma::Status ClearPerformanceCounters(const uint64_t* counters,
                                         uint64_t counter_count) override {
    EXPECT_EQ(1u, counter_count);
    EXPECT_EQ(2u, counters[0]);
    return MAGMA_STATUS_OK;
  }

  magma::Status BufferRangeOp(uint64_t buffer_id, uint32_t op, uint64_t start,
                              uint64_t length) override {
    std::unique_lock<std::mutex> lock(shared_data_->mutex);
    EXPECT_EQ(shared_data_->test_buffer_id, buffer_id);
    EXPECT_EQ(1000lu, start);
    EXPECT_EQ(2000lu, length);
    return MAGMA_STATUS_OK;
  }

  uint64_t immediate_commands_executed_ = 0;
  std::shared_ptr<SharedData> shared_data_;
};

std::unique_ptr<TestPlatformConnection> TestPlatformConnection::Create(
    std::shared_ptr<SharedData> shared_data) {
  auto delegate = std::make_unique<TestDelegate>(shared_data);

  std::shared_ptr<magma::PlatformConnectionClient> client_connection;

  auto endpoints = fidl::CreateEndpoints<fuchsia_gpu_magma::Primary>();
  if (!endpoints.is_ok())
    return MAGMA_DRETP(nullptr, "Failed to create primary endpoints");

  auto notification_endpoints = fidl::CreateEndpoints<fuchsia_gpu_magma::Notification>();
  if (!notification_endpoints.is_ok())
    return MAGMA_DRETP(nullptr, "Failed to create notification endpoints");

  auto connection = msd::internal::PrimaryFidlServer::Create(
      std::move(delegate), 1u, std::move(endpoints->server),
      std::move(notification_endpoints->server),
      shared_data->is_trusted ? MagmaClientType::kTrusted : MagmaClientType::kUntrusted);
  if (!connection)
    return MAGMA_DRETP(nullptr, "failed to create PlatformConnection");

  client_connection = magma::PlatformConnectionClient::Create(
      endpoints->client.channel().release(), notification_endpoints->client.TakeChannel().release(),
      shared_data->max_inflight_messages, shared_data->max_inflight_bytes);

  if (!client_connection)
    return MAGMA_DRETP(nullptr, "failed to create PlatformConnectionClient");

  auto server_holder = std::make_shared<internal::PrimaryFidlServerHolder>();
  server_holder->Start(std::move(connection), nullptr, [](const char* role_profile) {});

  return std::make_unique<TestPlatformConnection>(std::move(client_connection),
                                                  std::move(server_holder), shared_data);
}

TEST(PlatformConnection, GetError) {
  auto Test = TestPlatformConnection::Create();
  ASSERT_NE(Test, nullptr);
  Test->TestGetError();
}

TEST(PlatformConnection, TestImportBufferDeprecated) {
  auto Test = TestPlatformConnection::Create();
  ASSERT_NE(Test, nullptr);
  Test->TestImportBufferDeprecated();
}

TEST(PlatformConnection, ImportBuffer) {
  auto Test = TestPlatformConnection::Create();
  ASSERT_NE(Test, nullptr);
  Test->TestImportBuffer();
}

TEST(PlatformConnection, ReleaseBuffer) {
  auto Test = TestPlatformConnection::Create();
  ASSERT_NE(Test, nullptr);
  Test->TestReleaseBuffer();
}

TEST(PlatformConnection, TestImportSemaphoreDeprecated) {
  auto Test = TestPlatformConnection::Create();
  ASSERT_NE(Test, nullptr);
  Test->TestImportSemaphoreDeprecated();
}

TEST(PlatformConnection, ImportSemaphore) {
  auto Test = TestPlatformConnection::Create();
  ASSERT_NE(Test, nullptr);
  Test->TestImportSemaphore();
}

TEST(PlatformConnection, ReleaseSemaphore) {
  auto Test = TestPlatformConnection::Create();
  ASSERT_NE(Test, nullptr);
  Test->TestReleaseSemaphore();
}

TEST(PlatformConnection, CreateContext) {
  auto Test = TestPlatformConnection::Create();
  ASSERT_NE(Test, nullptr);
  Test->TestCreateContext();
}

TEST(PlatformConnection, CreateContext2) {
  auto Test = TestPlatformConnection::Create();
  ASSERT_NE(Test, nullptr);
  Test->TestCreateContext2();
}

TEST(PlatformConnection, CreateContext2HighPriorityUntrusted) {
  auto Test = TestPlatformConnection::Create();
  ASSERT_NE(Test, nullptr);
  Test->TestCreateContext2HighPriorityUntrusted();
}

TEST(PlatformConnection, CreateContext2HighPriorityTrusted) {
  std::shared_ptr shared_data = std::make_shared<SharedData>();
  shared_data->is_trusted = true;
  auto Test = TestPlatformConnection::Create(shared_data);
  ASSERT_NE(Test, nullptr);
  Test->TestCreateContext2HighPriorityTrusted();
}

TEST(PlatformConnection, DestroyContext) {
  auto Test = TestPlatformConnection::Create();
  ASSERT_NE(Test, nullptr);
  Test->TestDestroyContext();
}

TEST(PlatformConnection, MapUnmapBuffer) {
  auto Test = TestPlatformConnection::Create();
  ASSERT_NE(Test, nullptr);
  Test->TestMapUnmapBuffer();
}

TEST(PlatformConnection, NotificationChannel) {
  auto shared_data = std::make_shared<SharedData>();

  shared_data->notification_handler = [](msd::NotificationHandler* handler) {
    uint32_t data_to_send = kNotificationData;
    uint8_t* data_ptr = reinterpret_cast<uint8_t*>(&data_to_send);

    for (uint32_t i = 0; i < kNotificationCount; i++) {
      handler->NotificationChannelSend(cpp20::span(data_ptr, data_ptr + sizeof(uint32_t)));
      data_to_send += 1;
    }
  };

  auto Test = TestPlatformConnection::Create(shared_data);
  ASSERT_NE(Test, nullptr);
  Test->TestNotificationChannel();
}

namespace {
struct CompleterContext {
  bool expect_cancelled = false;
  msd::NotificationHandler* notification_handler = nullptr;

  std::shared_ptr<magma::PlatformSemaphore> wait_semaphore = magma::PlatformSemaphore::Create();
  std::shared_ptr<magma::PlatformSemaphore> signal_semaphore = magma::PlatformSemaphore::Create();
  std::shared_ptr<magma::PlatformSemaphore> started = magma::PlatformSemaphore::Create();
  void* cancel_token = nullptr;

  static void Starter(void* _context, void* cancel_token) {
    auto context = reinterpret_cast<CompleterContext*>(_context);
    context->cancel_token = cancel_token;
    context->started->Signal();
  }

  static void Completer(void* _context, magma_status_t status, magma_handle_t handle) {
    auto context = reinterpret_cast<CompleterContext*>(_context);
    if (context->expect_cancelled) {
      EXPECT_NE(MAGMA_STATUS_OK, status);
    } else {
      EXPECT_EQ(MAGMA_STATUS_OK, status);
    }

    ASSERT_NE(handle, magma::PlatformHandle::kInvalidHandle);

    auto semaphore = magma::PlatformSemaphore::Import(handle, /*flags=*/0);
    ASSERT_TRUE(semaphore);

    EXPECT_EQ(context->wait_semaphore->id(), semaphore->id());

    context->signal_semaphore->Signal();
  }
};
}  // namespace

TEST(PlatformConnection, ExecuteInlineCommands) {
  auto Test = TestPlatformConnection::Create();
  ASSERT_NE(Test, nullptr);
  Test->TestExecuteInlineCommands();
}

TEST(PlatformConnection, MultipleFlush) {
  auto Test = TestPlatformConnection::Create();
  ASSERT_NE(Test, nullptr);
  Test->TestMultipleFlush();
}

TEST(PlatformConnection, EnablePerformanceCounters) {
  auto Test = TestPlatformConnection::Create();
  ASSERT_NE(Test, nullptr);
  Test->TestEnablePerformanceCounters();
}

TEST(PlatformConnection, PrimaryWrapperFlowControlWithoutBytes) {
#ifdef __Fuchsia__
  constexpr uint64_t kMaxMessages = 10;
  constexpr uint64_t kMaxBytes = 10;
  {
    zx::channel local, remote;
    ASSERT_EQ(ZX_OK, zx::channel::create(0, &local, &remote));
    magma::PrimaryWrapper wrapper(std::move(local), kMaxMessages, kMaxBytes);
    auto [wait, count, bytes] = wrapper.ShouldWait(0);
    EXPECT_FALSE(wait);
    EXPECT_EQ(1u, count);
    EXPECT_EQ(0u, bytes);
  }
  {
    zx::channel local, remote;
    ASSERT_EQ(ZX_OK, zx::channel::create(0, &local, &remote));
    magma::PrimaryWrapper wrapper(std::move(local), kMaxMessages, kMaxBytes);
    constexpr uint64_t kStartMessages = 9;
    wrapper.set_for_test(kStartMessages, 0);
    auto [wait, count, bytes] = wrapper.ShouldWait(0);
    EXPECT_FALSE(wait);
    EXPECT_EQ(kStartMessages + 1, count);
    EXPECT_EQ(0u, bytes);
  }
  {
    zx::channel local, remote;
    ASSERT_EQ(ZX_OK, zx::channel::create(0, &local, &remote));
    magma::PrimaryWrapper wrapper(std::move(local), kMaxMessages, kMaxBytes);
    constexpr uint64_t kStartMessages = 10;
    wrapper.set_for_test(kStartMessages, 0);
    auto [wait, count, bytes] = wrapper.ShouldWait(0);
    EXPECT_TRUE(wait);
    EXPECT_EQ(kStartMessages + 1, count);
    EXPECT_EQ(0u, bytes);
  }
#else
  GTEST_SKIP();
#endif
}

TEST(PlatformConnection, PrimaryWrapperFlowControlWithBytes) {
#ifdef __Fuchsia__
  constexpr uint64_t kMaxMessages = 10;
  constexpr uint64_t kMaxBytes = 10;
  {
    zx::channel local, remote;
    ASSERT_EQ(ZX_OK, zx::channel::create(0, &local, &remote));
    magma::PrimaryWrapper wrapper(std::move(local), kMaxMessages, kMaxBytes);
    constexpr uint64_t kNewBytes = 5;
    auto [wait, count, bytes] = wrapper.ShouldWait(kNewBytes);
    EXPECT_FALSE(wait);
    EXPECT_EQ(1u, count);
    EXPECT_EQ(kNewBytes, bytes);
  }
  {
    zx::channel local, remote;
    ASSERT_EQ(ZX_OK, zx::channel::create(0, &local, &remote));
    magma::PrimaryWrapper wrapper(std::move(local), kMaxMessages, kMaxBytes);
    constexpr uint64_t kNewBytes = 15;
    auto [wait, count, bytes] = wrapper.ShouldWait(kNewBytes);
    EXPECT_FALSE(wait);  // Limit exceeded ok, we can pass a single message of any size
    EXPECT_EQ(1u, count);
    EXPECT_EQ(kNewBytes, bytes);
  }
  {
    zx::channel local, remote;
    ASSERT_EQ(ZX_OK, zx::channel::create(0, &local, &remote));
    magma::PrimaryWrapper wrapper(std::move(local), kMaxMessages, kMaxBytes);
    constexpr uint64_t kStartBytes = 4;
    constexpr uint64_t kNewBytes = 10;
    wrapper.set_for_test(0, kStartBytes);
    auto [wait, count, bytes] = wrapper.ShouldWait(kNewBytes);
    EXPECT_FALSE(wait);  // Limit exceeded ok, we're at less than half byte limit
    EXPECT_EQ(1u, count);
    EXPECT_EQ(kStartBytes + kNewBytes, bytes);
  }
  {
    zx::channel local, remote;
    ASSERT_EQ(ZX_OK, zx::channel::create(0, &local, &remote));
    magma::PrimaryWrapper wrapper(std::move(local), kMaxMessages, kMaxBytes);
    constexpr uint64_t kStartBytes = 5;
    constexpr uint64_t kNewBytes = 5;
    wrapper.set_for_test(0, 5);
    auto [wait, count, bytes] = wrapper.ShouldWait(kNewBytes);
    EXPECT_FALSE(wait);
    EXPECT_EQ(1u, count);
    EXPECT_EQ(kStartBytes + kNewBytes, bytes);
  }
  {
    zx::channel local, remote;
    ASSERT_EQ(ZX_OK, zx::channel::create(0, &local, &remote));
    magma::PrimaryWrapper wrapper(std::move(local), kMaxMessages, kMaxBytes);
    constexpr uint64_t kStartBytes = 5;
    constexpr uint64_t kNewBytes = 6;
    wrapper.set_for_test(0, kStartBytes);
    auto [wait, count, bytes] = wrapper.ShouldWait(kNewBytes);
    EXPECT_TRUE(wait);
    EXPECT_EQ(1u, count);
    EXPECT_EQ(kStartBytes + kNewBytes, bytes);
  }
  {
    zx::channel local, remote;
    ASSERT_EQ(ZX_OK, zx::channel::create(0, &local, &remote));
    magma::PrimaryWrapper wrapper(std::move(local), kMaxMessages, kMaxBytes);
    constexpr uint64_t kStartBytes = kMaxBytes;
    constexpr uint64_t kNewBytes = 0;
    wrapper.set_for_test(0, kStartBytes);
    auto [wait, count, bytes] = wrapper.ShouldWait(kNewBytes);
    EXPECT_FALSE(wait);  // At max bytes, not sending more
    EXPECT_EQ(1u, count);
    EXPECT_EQ(kStartBytes + kNewBytes, bytes);
  }
  {
    zx::channel local, remote;
    ASSERT_EQ(ZX_OK, zx::channel::create(0, &local, &remote));
    magma::PrimaryWrapper wrapper(std::move(local), kMaxMessages, kMaxBytes);
    constexpr uint64_t kStartBytes = kMaxBytes + 1;
    constexpr uint64_t kNewBytes = 0;
    wrapper.set_for_test(0, kStartBytes);
    auto [wait, count, bytes] = wrapper.ShouldWait(kNewBytes);
    EXPECT_FALSE(wait);  // Above max bytes, not sending more
    EXPECT_EQ(1u, count);
    EXPECT_EQ(kStartBytes + kNewBytes, bytes);
  }
  {
    zx::channel local, remote;
    ASSERT_EQ(ZX_OK, zx::channel::create(0, &local, &remote));
    magma::PrimaryWrapper wrapper(std::move(local), kMaxMessages, kMaxBytes);
    constexpr uint64_t kStartBytes = kMaxBytes;
    constexpr uint64_t kNewBytes = 1;
    wrapper.set_for_test(0, kStartBytes);
    auto [wait, count, bytes] = wrapper.ShouldWait(kNewBytes);
    EXPECT_TRUE(wait);  // At max bytes, sending more
    EXPECT_EQ(1u, count);
    EXPECT_EQ(kStartBytes + kNewBytes, bytes);
  }
  {
    zx::channel local, remote;
    ASSERT_EQ(ZX_OK, zx::channel::create(0, &local, &remote));
    magma::PrimaryWrapper wrapper(std::move(local), kMaxMessages, kMaxBytes);
    constexpr uint64_t kStartBytes = kMaxBytes + 1;
    constexpr uint64_t kNewBytes = 1;
    wrapper.set_for_test(0, kStartBytes);
    auto [wait, count, bytes] = wrapper.ShouldWait(kNewBytes);
    EXPECT_TRUE(wait);  // Above max bytes, sending more
    EXPECT_EQ(1u, count);
    EXPECT_EQ(kStartBytes + kNewBytes, bytes);
  }
#else
  GTEST_SKIP();
#endif
}

TEST(PlatformConnection, TestPerformanceCounters) {
  auto Test = TestPlatformConnection::Create();
  ASSERT_NE(Test, nullptr);
  Test->TestPerformanceCounters();
}

TEST(PlatformConnection, TestFlush) {
  auto Test = TestPlatformConnection::Create();
  ASSERT_NE(Test, nullptr);
  Test->TestFlush();
}

}  // namespace msd
