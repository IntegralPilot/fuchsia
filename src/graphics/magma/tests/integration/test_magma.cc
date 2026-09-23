// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <poll.h>
#include <time.h>

#include <array>
#include <cstdint>
#include <limits>
#include <thread>
#include <unordered_set>

#if defined(__Fuchsia__)
#include <fidl/fuchsia.gpu.magma.test/cpp/wire.h>
#include <fidl/fuchsia.gpu.magma/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.logger/cpp/wire.h>
#include <fidl/fuchsia.tracing.provider/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/io.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/server.h>
#include <lib/magma/magma_sysmem.h>
#include <lib/magma/platform/platform_logger.h>           // nogncheck
#include <lib/magma/platform/platform_logger_provider.h>  // nogncheck
#include <lib/magma/platform/platform_trace_provider.h>   // nogncheck
#include <lib/zx/channel.h>
#include <lib/zx/vmar.h>
#include <zircon/availability.h>

#include <filesystem>

#include "src/lib/diagnostics/fake-log-sink/cpp/fake_log_sink.h"

#endif

#if defined(__linux__)
#include <sys/mman.h>
#include <sys/resource.h>
#include <sys/time.h>
#include <unistd.h>
#endif

#include <lib/magma/magma.h>
#include <lib/magma/magma_common_defs.h>
#include <lib/magma_client/test_util/magma_map_cpu.h>

#include <gtest/gtest.h>

extern "C" {
#include "test_magma.h"
}

namespace {

inline uint64_t page_size() { return sysconf(_SC_PAGESIZE); }

inline constexpr int64_t ms_to_ns(int64_t ms) { return ms * 1000000ull; }

static inline uint32_t to_uint32(uint64_t val) {
  assert(val <= std::numeric_limits<uint32_t>::max());
  return static_cast<uint32_t>(val);
}

static uint64_t clock_gettime_monotonic_raw() {
  struct timespec ts;

  clock_gettime(CLOCK_MONOTONIC_RAW, &ts);

  return 1000000000ull * ts.tv_sec + ts.tv_nsec;
}

}  // namespace

#if defined(__Fuchsia__)
class FakePerfCountAccessServer
    : public fidl::WireServer<fuchsia_gpu_magma::PerformanceCounterAccess> {
  void GetPerformanceCountToken(GetPerformanceCountTokenCompleter::Sync& completer) override {
    zx::event event;
    zx::event::create(0, &event);
    completer.Reply(std::move(event));
  }
};

class FakeTraceRegistry : public fidl::WireServer<fuchsia_tracing_provider::Registry> {
 public:
  explicit FakeTraceRegistry(async::Loop& loop) : loop_(loop) {}
  void RegisterProvider(RegisterProviderRequestView request,
                        RegisterProviderCompleter::Sync& _completer) override {
    loop_.Quit();
  }
  void RegisterProviderSynchronously(
      RegisterProviderSynchronouslyRequestView request,
      RegisterProviderSynchronouslyCompleter::Sync& _completer) override {}

  void RegisterV2(RegisterV2RequestView request, RegisterV2Completer::Sync& _completer) override {
    loop_.Quit();
  }
  void RegisterV2Synchronously(RegisterV2SynchronouslyRequestView request,
                               RegisterV2SynchronouslyCompleter::Sync& _completer) override {}
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_tracing_provider::Registry> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

 private:
  async::Loop& loop_;
};
#endif

class TestConnection {
 public:
  static constexpr const char* kDevicePathFuchsiaUntrusted = "/svc/fuchsia.gpu.magma.Service";
  static constexpr const char* kDevicePathFuchsiaTrusted = "/svc/fuchsia.gpu.magma.TrustedService";
  static constexpr const char* kDeviceNameLinux = "/dev/dri/renderD128";
  static constexpr const char* kDeviceNameVirtioMagma = "/dev/magma0";

#if defined(__Fuchsia__)
  static constexpr bool is_valid_handle(magma_handle_t handle) { return handle != 0; }
#else
  static constexpr bool is_valid_handle(magma_handle_t handle) {
    return static_cast<int>(handle) >= 0;
  }
#endif

#if defined(__Fuchsia__)
  static bool OpenFuchsiaDevice(const char* device_path, std::string* device_name_out,
                                magma_device_t* device_out) {
    std::string device_name;
    magma_device_t device = 0;

    for (auto& p : std::filesystem::directory_iterator(device_path)) {
      EXPECT_FALSE(device) << " More than one GPU device found, specify --vendor-id";
      if (device) {
        magma_device_release(device);
        return false;
      }

      zx::channel server_end, client_end;
      zx::channel::create(0, &server_end, &client_end);

      device_name = (static_cast<std::string>(p.path()) + "/device");

      zx_status_t zx_status = fdio_service_connect(device_name.c_str(), server_end.release());
      EXPECT_EQ(ZX_OK, zx_status);
      if (zx_status != ZX_OK)
        return false;

      magma_status_t status = magma_device_import(client_end.release(), &device);
      EXPECT_EQ(MAGMA_STATUS_OK, status);
      if (status != MAGMA_STATUS_OK)
        return false;

      if (gVendorId) {
        uint64_t vendor_id;
        status = magma_device_query(device, MAGMA_QUERY_VENDOR_ID, NULL, &vendor_id);
        EXPECT_EQ(MAGMA_STATUS_OK, status);
        if (status != MAGMA_STATUS_OK)
          return false;

        if (vendor_id == gVendorId) {
          break;
        } else {
          magma_device_release(device);
          device = 0;
        }
      }
    }

    if (!device)
      return false;

    *device_name_out = device_name;
    *device_out = device;
    return true;
  }

#endif

  std::string device_name() { return device_name_; }

  bool is_virtmagma() { return is_virtmagma_; }

  TestConnection() {
#if defined(__Fuchsia__)
    auto client_end = component::Connect<fuchsia_gpu_magma_test::VendorHelper>();
    EXPECT_TRUE(client_end.is_ok()) << " status " << client_end.status_value();

    vendor_helper_ = fidl::WireSyncClient(std::move(*client_end));

    if (!OpenFuchsiaDevice(kDevicePathFuchsiaUntrusted, &device_name_, &device_)) {
      OpenFuchsiaDevice(kDevicePathFuchsiaTrusted, &device_name_, &device_);
    }
    EXPECT_TRUE(device_) << "Unable to open magma device";

#elif defined(__linux__)
    int fd = open(kDeviceNameVirtioMagma, O_RDWR);
    if (fd >= 0) {
      device_name_ = kDeviceNameVirtioMagma;
    } else {
      fd = open(kDeviceNameLinux, O_RDWR);
      if (fd >= 0) {
        device_name_ = kDeviceNameLinux;
      }
    }
    EXPECT_TRUE(fd >= 0);
    if (fd >= 0) {
      EXPECT_EQ(MAGMA_STATUS_OK, magma_device_import(fd, &device_));
    }
#if defined(VIRTMAGMA)
    is_virtmagma_ = true;
#endif
#else
#error Unimplemented
#endif
    if (device_) {
      magma_device_create_connection(device_, &connection_);
    }
  }

  ~TestConnection() {
    if (connection_)
      magma_connection_release(connection_);
    if (device_)
      magma_device_release(device_);
    if (fd_ >= 0)
      close(fd_);
  }

  int fd() { return fd_; }

  magma_connection_t connection() { return connection_; }

  void Connection() { ASSERT_TRUE(connection_); }

  bool vendor_has_unmap() {
#ifdef __Fuchsia__
    auto result = vendor_helper_->GetConfig();
    EXPECT_TRUE(result.ok());

    if (!result.Unwrap()->has_buffer_unmap_type()) {
      return false;
    }
    return result.Unwrap()->buffer_unmap_type() ==
           ::fuchsia_gpu_magma_test::wire::BufferUnmapType::kSupported;
#else
    return false;
#endif
  }

  bool vendor_has_perform_buffer_op() {
#ifdef __Fuchsia__
    auto result = vendor_helper_->GetConfig();
    EXPECT_TRUE(result.ok());

    if (!result.Unwrap()->has_connection_perform_buffer_op_type()) {
      return false;
    }
    return result.Unwrap()->connection_perform_buffer_op_type() ==
           ::fuchsia_gpu_magma_test::wire::ConnectionPerformBufferOpType::kSupported;
#else
    return false;
#endif
  }

  void Context() {
    ASSERT_TRUE(connection_);

    uint32_t context_id[2];
    EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_create_context(connection_, &context_id[0]));
    EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_flush(connection_));

    EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_create_context(connection_, &context_id[1]));
    EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_flush(connection_));

    magma_connection_release_context(connection_, context_id[0]);
    EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_flush(connection_));

    magma_connection_release_context(connection_, context_id[1]);
    EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_flush(connection_));

    // Already released
    magma_connection_release_context(connection_, context_id[1]);
    EXPECT_EQ(MAGMA_STATUS_INVALID_ARGS, magma_connection_flush(connection_));
  }

  void Context2() const {
    ASSERT_TRUE(connection_);

    uint32_t context_id[2];
    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_connection_create_context2(connection_, MAGMA_PRIORITY_MEDIUM, &context_id[0]));
    EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_flush(connection_));

    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_connection_create_context2(connection_, MAGMA_PRIORITY_MEDIUM, &context_id[1]));
    EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_flush(connection_));

    magma_connection_release_context(connection_, context_id[0]);
    EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_flush(connection_));

    magma_connection_release_context(connection_, context_id[1]);
    EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_flush(connection_));

    // Already released
    magma_connection_release_context(connection_, context_id[1]);
    EXPECT_EQ(MAGMA_STATUS_INVALID_ARGS, magma_connection_flush(connection_));
  }

  void NotificationChannelHandle() {
    ASSERT_TRUE(connection_);

    uint32_t handle = magma_connection_get_notification_channel_handle(connection_);
    EXPECT_NE(0u, handle);

    uint32_t handle2 = magma_connection_get_notification_channel_handle(connection_);
    EXPECT_EQ(handle, handle2);
  }

  void ReadNotificationChannel() {
    ASSERT_TRUE(connection_);

    std::array<unsigned char, 1024> buffer;
    uint64_t buffer_size = ~0;
    magma_bool_t more_data = true;
    magma_status_t status = magma_connection_read_notification_channel(
        connection_, buffer.data(), buffer.size(), &buffer_size, &more_data);
    EXPECT_EQ(MAGMA_STATUS_OK, status);
    EXPECT_EQ(0u, buffer_size);
    EXPECT_EQ(false, more_data);
  }

  void Buffer() {
    ASSERT_TRUE(connection_);

    uint64_t size = page_size() + 16;
    uint64_t actual_size = 0;
    magma_buffer_t buffer = 0;
    magma_buffer_id_t buffer_id = 0;

    ASSERT_EQ(MAGMA_STATUS_OK,
              magma_connection_create_buffer(connection_, size, &actual_size, &buffer, &buffer_id));
    EXPECT_GE(actual_size, size);
    EXPECT_NE(buffer, 0u);

    {
      uint64_t size2 = page_size() + 16;
      uint64_t actual_size2 = 0;
      magma_buffer_t buffer2 = 0;
      magma_buffer_id_t buffer_id2 = 0;

      ASSERT_EQ(MAGMA_STATUS_OK, magma_connection_create_buffer(connection_, size2, &actual_size2,
                                                                &buffer2, &buffer_id2));
      EXPECT_GE(actual_size2, size2);
      EXPECT_NE(buffer2, 0u);
      EXPECT_NE(buffer_id2, buffer_id);
      magma_connection_release_buffer(connection_, buffer2);
    }

    magma_connection_release_buffer(connection_, buffer);
  }

  void BufferMap() {
    ASSERT_TRUE(connection_);

    uint64_t size = page_size();
    uint64_t actual_size;
    magma_buffer_t buffer = 0;
    magma_buffer_id_t buffer_id;

    ASSERT_EQ(MAGMA_STATUS_OK,
              magma_connection_create_buffer(connection_, size, &actual_size, &buffer, &buffer_id));
    EXPECT_NE(buffer, 0u);

    constexpr uint64_t kGpuAddress = 0x1000;
    EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_map_buffer(connection_, kGpuAddress, buffer, 0,
                                                           size, MAGMA_MAP_FLAG_READ));
    EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_flush(connection_));

    if (vendor_has_unmap()) {
      magma_connection_unmap_buffer(connection_, kGpuAddress, buffer);
      EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_flush(connection_));
    }

    // Invalid page offset, remote error
    constexpr uint64_t kInvalidPageOffset = 1024;
    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_connection_map_buffer(connection_, 0, buffer, kInvalidPageOffset * page_size(),
                                          size, MAGMA_MAP_FLAG_READ));
    EXPECT_EQ(MAGMA_STATUS_INVALID_ARGS, magma_connection_flush(connection_));

    magma_connection_release_buffer(connection_, buffer);
  }

  void BufferMapOverlapError() {
    ASSERT_TRUE(connection_);

    uint64_t size = page_size() * 2;
    std::array<magma_buffer_t, 2> buffer;

    {
      uint64_t actual_size;
      uint64_t buffer_id;
      ASSERT_EQ(MAGMA_STATUS_OK, magma_connection_create_buffer(connection_, size, &actual_size,
                                                                &buffer[0], &buffer_id));
      EXPECT_NE(buffer[0], 0u);
    }
    {
      uint64_t actual_size;
      uint64_t buffer_id;
      ASSERT_EQ(MAGMA_STATUS_OK, magma_connection_create_buffer(connection_, size, &actual_size,
                                                                &buffer[1], &buffer_id));
      EXPECT_NE(buffer[1], 0u);
    }

    constexpr uint64_t kGpuAddress = 0x1000;

    EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_map_buffer(connection_, kGpuAddress, buffer[0], 0,
                                                           size, MAGMA_MAP_FLAG_READ));
    EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_flush(connection_));

    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_connection_map_buffer(connection_, kGpuAddress + size / 2, buffer[1], 0, size,
                                          MAGMA_MAP_FLAG_READ));

    {
      magma_status_t status = magma_connection_flush(connection_);
      if (status != MAGMA_STATUS_INVALID_ARGS)
        EXPECT_EQ(MAGMA_STATUS_INTERNAL_ERROR, status);
    }

    magma_connection_release_buffer(connection_, buffer[1]);
    magma_connection_release_buffer(connection_, buffer[0]);
  }

  void BufferMapDuplicates(int count) {
    ASSERT_TRUE(connection_);

    uint64_t size = page_size();
    uint64_t actual_size;
    magma_buffer_t buffer;
    magma_buffer_id_t buffer_id;

    ASSERT_EQ(MAGMA_STATUS_OK,
              magma_connection_create_buffer(connection_, size, &actual_size, &buffer, &buffer_id));

    // Check that we can map the same underlying memory object many times
    std::vector<magma_buffer_t> imported_buffers;
    std::vector<uint64_t> imported_addrs;

    uint64_t gpu_address = 0x1000;

    for (int i = 0; i < count; i++) {
      magma_handle_t handle;
      ASSERT_EQ(MAGMA_STATUS_OK, magma_buffer_export(buffer, &handle));

      magma_buffer_id_t buffer_id2;
      uint64_t buffer_size2;
      magma_buffer_t buffer2;
      ASSERT_EQ(MAGMA_STATUS_OK, magma_connection_import_buffer(connection_, handle, &buffer_size2,
                                                                &buffer2, &buffer_id2))
          << "i " << i;

      EXPECT_EQ(actual_size, buffer_size2);
      EXPECT_NE(buffer_id, buffer_id2);

      ASSERT_EQ(MAGMA_STATUS_OK, magma_connection_map_buffer(connection_, gpu_address, buffer2, 0,
                                                             size, MAGMA_MAP_FLAG_READ))
          << "i " << i;

      ASSERT_EQ(MAGMA_STATUS_OK, magma_connection_flush(connection_)) << "i " << i;

      if (vendor_has_perform_buffer_op()) {
        ASSERT_EQ(MAGMA_STATUS_OK,
                  magma_connection_perform_buffer_op(
                      connection_, buffer2, MAGMA_BUFFER_RANGE_OP_POPULATE_TABLES, 0, size));
        ASSERT_EQ(MAGMA_STATUS_OK, magma_connection_flush(connection_)) << "i " << i;
      }

      imported_buffers.push_back(buffer2);
      imported_addrs.push_back(gpu_address);

      gpu_address += size + 10 * page_size();
    }

    for (size_t i = 0; i < imported_buffers.size(); i++) {
      if (vendor_has_unmap()) {
        magma_connection_unmap_buffer(connection_, imported_addrs[i], imported_buffers[i]);
      }

      EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_flush(connection_));

      magma_connection_release_buffer(connection_, imported_buffers[i]);
    }

    magma_connection_release_buffer(connection_, buffer);
  }

  void BufferMapInvalid(bool flush) {
    ASSERT_TRUE(connection_);

    if (flush) {
      EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_flush(connection_));
    } else {
      EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_get_error(connection_));
    }

    uint64_t size = page_size();
    uint64_t actual_size;
    magma_buffer_t buffer;
    uint64_t buffer_id;

    ASSERT_EQ(MAGMA_STATUS_OK,
              magma_connection_create_buffer(connection_, size, &actual_size, &buffer, &buffer_id));

    // Invalid page offset, remote error
    constexpr uint64_t kInvalidPageOffset = 1024;
    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_connection_map_buffer(connection_, 0, buffer, kInvalidPageOffset * page_size(),
                                          size, MAGMA_MAP_FLAG_READ));

    if (flush) {
      EXPECT_EQ(MAGMA_STATUS_INVALID_ARGS, magma_connection_flush(connection_));
    } else {
      std::vector<magma_poll_item_t> items({{
          .handle = magma_connection_get_notification_channel_handle(connection_),
          .type = MAGMA_POLL_TYPE_HANDLE,
          .condition = MAGMA_POLL_CONDITION_READABLE,
      }});

      constexpr uint64_t kTimeoutNs = std::numeric_limits<uint64_t>::max();

      EXPECT_EQ(MAGMA_STATUS_CONNECTION_LOST,
                magma_poll(items.data(), to_uint32(items.size()), kTimeoutNs));

      EXPECT_EQ(MAGMA_STATUS_INVALID_ARGS, magma_connection_get_error(connection_));
    }

    magma_connection_release_buffer(connection_, buffer);
  }

  void BufferExport(uint32_t* handle_out, uint64_t* id_out) {
    ASSERT_TRUE(connection_);

    uint64_t size = page_size();
    magma_buffer_t buffer;
    uint64_t buffer_id;

    ASSERT_EQ(MAGMA_STATUS_OK,
              magma_connection_create_buffer(connection_, size, &size, &buffer, &buffer_id));

    *id_out = buffer_id;

    EXPECT_EQ(MAGMA_STATUS_OK, magma_buffer_export(buffer, handle_out));

    magma_connection_release_buffer(connection_, buffer);
  }

  void BufferImportInvalid() {
    ASSERT_TRUE(connection_);

    constexpr uint32_t kInvalidHandle = 0xabcd1234;
    magma_buffer_t buffer;
#if defined(__Fuchsia__)
    constexpr magma_status_t kExpectedStatus = MAGMA_STATUS_INVALID_ARGS;
#elif defined(__linux__)
    constexpr magma_status_t kExpectedStatus = MAGMA_STATUS_INTERNAL_ERROR;
#endif
    uint64_t size;
    magma_buffer_id_t id;
    ASSERT_EQ(kExpectedStatus,
              magma_connection_import_buffer(connection_, kInvalidHandle, &size, &buffer, &id));
  }

  void BufferImport(uint32_t handle, uint64_t exported_id) {
    ASSERT_TRUE(connection_);

    magma_buffer_t buffer;
    uint64_t buffer_size;
    magma_buffer_id_t buffer_id;
    ASSERT_EQ(MAGMA_STATUS_OK, magma_connection_import_buffer(connection_, handle, &buffer_size,
                                                              &buffer, &buffer_id));
    EXPECT_NE(buffer_id, exported_id);

    magma_connection_release_buffer(connection_, buffer);
  }

  static magma_status_t wait_all(std::vector<magma_poll_item_t>& items, int64_t timeout_ns) {
    int64_t remaining_ns = timeout_ns;

    for (size_t i = 0; i < items.size(); i++) {
      if (remaining_ns < 0)
        remaining_ns = 0;

      auto start = std::chrono::steady_clock::now();

      magma_status_t status = magma_poll(&items[i], 1, remaining_ns);
      if (status != MAGMA_STATUS_OK)
        return status;

      remaining_ns -= std::chrono::duration_cast<std::chrono::nanoseconds>(
                          std::chrono::steady_clock::now() - start)
                          .count();
    }
    return MAGMA_STATUS_OK;
  }

  void Semaphore(uint32_t count) {
    ASSERT_TRUE(connection_);

    std::vector<magma_poll_item_t> items(count);

    for (uint32_t i = 0; i < count; i++) {
      items[i] = {.type = MAGMA_POLL_TYPE_SEMAPHORE, .condition = MAGMA_POLL_CONDITION_SIGNALED};
      magma_semaphore_id_t id;
      ASSERT_EQ(MAGMA_STATUS_OK,
                magma_connection_create_semaphore(connection_, &items[i].semaphore, &id));
      EXPECT_NE(0u, id);
    }

    magma_semaphore_signal(items[0].semaphore);

    constexpr uint32_t kTimeoutMs = 100;
    constexpr uint64_t kNsPerMs = 1000000;

    auto start = std::chrono::steady_clock::now();
    EXPECT_EQ(count == 1 ? MAGMA_STATUS_OK : MAGMA_STATUS_TIMED_OUT,
              wait_all(items, kNsPerMs * kTimeoutMs));
    if (count > 1) {
      // Subtract to allow for rounding errors in magma_wait_semaphores time calculations
      EXPECT_LE(kTimeoutMs - count, std::chrono::duration_cast<std::chrono::milliseconds>(
                                        std::chrono::steady_clock::now() - start)
                                        .count());
    }

    for (uint32_t i = 1; i < items.size(); i++) {
      magma_semaphore_signal(items[i].semaphore);
    }

    EXPECT_EQ(MAGMA_STATUS_OK, wait_all(items, 0));

    for (uint32_t i = 0; i < items.size(); i++) {
      magma_semaphore_reset(items[i].semaphore);
    }

    EXPECT_EQ(MAGMA_STATUS_TIMED_OUT, wait_all(items, 0));

    // Wait for one
    start = std::chrono::steady_clock::now();
    EXPECT_EQ(MAGMA_STATUS_TIMED_OUT,
              magma_poll(items.data(), to_uint32(items.size()), kNsPerMs * kTimeoutMs));

    // Subtract to allow for rounding errors in magma_wait_semaphores time calculations
    EXPECT_LE(kTimeoutMs - count, std::chrono::duration_cast<std::chrono::milliseconds>(
                                      std::chrono::steady_clock::now() - start)
                                      .count());

    magma_semaphore_signal(items.back().semaphore);

    EXPECT_EQ(MAGMA_STATUS_OK, magma_poll(items.data(), to_uint32(items.size()), 0));

    magma_semaphore_reset(items.back().semaphore);

    EXPECT_EQ(MAGMA_STATUS_TIMED_OUT, magma_poll(items.data(), to_uint32(items.size()), 0));

    for (auto& item : items) {
      magma_connection_release_semaphore(connection_, item.semaphore);
    }
  }

  void PollWithNotificationChannel(uint32_t semaphore_count) {
    ASSERT_TRUE(connection_);

    std::vector<magma_poll_item_t> items(semaphore_count + 1);

    static constexpr int kNotificationChannelItemIndex = 0;
    static constexpr int kFirstSemaphoreItemIndex = 1;

    for (uint32_t i = 0; i < semaphore_count; i++) {
      magma_semaphore_t semaphore;
      magma_semaphore_id_t id;
      ASSERT_EQ(MAGMA_STATUS_OK, magma_connection_create_semaphore(connection_, &semaphore, &id));

      items[kFirstSemaphoreItemIndex + i] = {.semaphore = semaphore,
                                             .type = MAGMA_POLL_TYPE_SEMAPHORE,
                                             .condition = MAGMA_POLL_CONDITION_SIGNALED};
    }

    items[kNotificationChannelItemIndex] = {
        .handle = magma_connection_get_notification_channel_handle(connection_),
        .type = MAGMA_POLL_TYPE_HANDLE,
        .condition = MAGMA_POLL_CONDITION_READABLE,
    };

    constexpr int64_t kTimeoutMs = 100;
    auto start = std::chrono::steady_clock::now();
    EXPECT_EQ(MAGMA_STATUS_TIMED_OUT,
              magma_poll(items.data(), to_uint32(items.size()), ms_to_ns(kTimeoutMs)));
    // TODO(https://fxbug.dev/42126035) - remove this adjustment for magma_poll timeout truncation
    // in ns to ms conversion
    EXPECT_LE(kTimeoutMs - 1, std::chrono::duration_cast<std::chrono::milliseconds>(
                                  std::chrono::steady_clock::now() - start)
                                  .count());

    if (semaphore_count == 0)
      return;

    magma_semaphore_signal(items[kFirstSemaphoreItemIndex].semaphore);

    EXPECT_EQ(MAGMA_STATUS_OK, magma_poll(items.data(), to_uint32(items.size()), 0));
    EXPECT_EQ(items[kFirstSemaphoreItemIndex].result, items[kFirstSemaphoreItemIndex].condition);
    EXPECT_EQ(items[kNotificationChannelItemIndex].result, 0u);

    magma_semaphore_reset(items[kFirstSemaphoreItemIndex].semaphore);

    start = std::chrono::steady_clock::now();
    EXPECT_EQ(MAGMA_STATUS_TIMED_OUT,
              magma_poll(items.data(), to_uint32(items.size()), ms_to_ns(kTimeoutMs)));
    // TODO(https://fxbug.dev/42126035) - remove this adjustment for magma_poll timeout truncation
    // in ns to ms conversion
    EXPECT_LE(kTimeoutMs - 1, std::chrono::duration_cast<std::chrono::milliseconds>(
                                  std::chrono::steady_clock::now() - start)
                                  .count());

    for (uint32_t i = 0; i < semaphore_count; i++) {
      magma_semaphore_signal(items[kFirstSemaphoreItemIndex + i].semaphore);
    }

    EXPECT_EQ(MAGMA_STATUS_OK, magma_poll(items.data(), to_uint32(items.size()), 0));

    for (uint32_t i = 0; i < items.size(); i++) {
      if (i >= kFirstSemaphoreItemIndex) {
        EXPECT_EQ(items[i].result, items[i].condition) << "item index " << i;
      } else {
        // Notification channel
        EXPECT_EQ(items[i].result, 0u) << "item index " << i;
      }
    }

    for (uint32_t i = 0; i < semaphore_count; i++) {
      magma_connection_release_semaphore(connection_,
                                         items[kFirstSemaphoreItemIndex + i].semaphore);
    }
  }

  void PollWithTestChannel() {
#ifdef __Fuchsia__
    ASSERT_TRUE(connection_);

    zx::channel local, remote;
    ASSERT_EQ(ZX_OK, zx::channel::create(0 /* flags */, &local, &remote));

    magma_semaphore_t semaphore;
    magma_semaphore_id_t id;
    ASSERT_EQ(MAGMA_STATUS_OK, magma_connection_create_semaphore(connection_, &semaphore, &id));

    std::vector<magma_poll_item_t> items;
    items.push_back({.semaphore = semaphore,
                     .type = MAGMA_POLL_TYPE_SEMAPHORE,
                     .condition = MAGMA_POLL_CONDITION_SIGNALED});
    items.push_back({
        .handle = local.get(),
        .type = MAGMA_POLL_TYPE_HANDLE,
        .condition = MAGMA_POLL_CONDITION_READABLE,
    });

    constexpr int64_t kTimeoutNs = ms_to_ns(100);
    auto start = std::chrono::steady_clock::now();
    EXPECT_EQ(MAGMA_STATUS_TIMED_OUT,
              magma_poll(items.data(), static_cast<uint32_t>(items.size()), kTimeoutNs));
    EXPECT_LE(kTimeoutNs, std::chrono::duration_cast<std::chrono::nanoseconds>(
                              std::chrono::steady_clock::now() - start)
                              .count());

    magma_semaphore_signal(semaphore);

    EXPECT_EQ(MAGMA_STATUS_OK, magma_poll(items.data(), static_cast<uint32_t>(items.size()), 0));
    EXPECT_EQ(items[0].result, items[0].condition);
    EXPECT_EQ(items[1].result, 0u);

    magma_semaphore_reset(semaphore);

    start = std::chrono::steady_clock::now();
    EXPECT_EQ(MAGMA_STATUS_TIMED_OUT,
              magma_poll(items.data(), static_cast<uint32_t>(items.size()), kTimeoutNs));
    EXPECT_LE(kTimeoutNs, std::chrono::duration_cast<std::chrono::nanoseconds>(
                              std::chrono::steady_clock::now() - start)
                              .count());

    uint32_t dummy = 0;
    EXPECT_EQ(ZX_OK, remote.write(0 /* flags */, &dummy, sizeof(dummy), nullptr /* handles */,
                                  0 /* num_handles*/));

    EXPECT_EQ(MAGMA_STATUS_OK, magma_poll(items.data(), static_cast<uint32_t>(items.size()), 0));
    EXPECT_EQ(items[0].result, 0u);
    EXPECT_EQ(items[1].result, items[1].condition);

    magma_semaphore_signal(semaphore);

    EXPECT_EQ(MAGMA_STATUS_OK, magma_poll(items.data(), static_cast<uint32_t>(items.size()), 0));
    EXPECT_EQ(items[0].result, items[0].condition);
    EXPECT_EQ(items[1].result, items[1].condition);

    magma_connection_release_semaphore(connection_, semaphore);
#else
    GTEST_SKIP();
#endif
  }

  void PollChannelClosed() {
#ifdef __Fuchsia__
    ASSERT_TRUE(connection_);

    zx::channel local, remote;
    ASSERT_EQ(ZX_OK, zx::channel::create(0 /* flags */, &local, &remote));

    magma_semaphore_t semaphore;
    magma_semaphore_id_t id;
    ASSERT_EQ(MAGMA_STATUS_OK, magma_connection_create_semaphore(connection_, &semaphore, &id));

    std::vector<magma_poll_item_t> items({{
                                              .handle = local.get(),
                                              .type = MAGMA_POLL_TYPE_HANDLE,
                                              .condition = MAGMA_POLL_CONDITION_READABLE,
                                          },
                                          {
                                              .semaphore = semaphore,
                                              .type = MAGMA_POLL_TYPE_SEMAPHORE,
                                              .condition = MAGMA_POLL_CONDITION_SIGNALED,
                                          }});

    {
      constexpr uint64_t kTimeoutMs = 10;
      EXPECT_EQ(
          MAGMA_STATUS_TIMED_OUT,
          magma_poll(items.data(), static_cast<uint32_t>(items.size()), kTimeoutMs * 1000000));
    }

    remote.reset();

    {
      constexpr uint64_t kTimeoutNs = std::numeric_limits<uint64_t>::max();

      EXPECT_EQ(MAGMA_STATUS_CONNECTION_LOST,
                magma_poll(items.data(), static_cast<uint32_t>(items.size()), kTimeoutNs));
    }

    magma_connection_release_semaphore(connection_, semaphore);

#else
    GTEST_SKIP();
#endif
  }

  void PollLongDeadline(bool forever_deadline) {
    ASSERT_TRUE(connection_);

    magma_poll_item_t item{.type = MAGMA_POLL_TYPE_SEMAPHORE,
                           .condition = MAGMA_POLL_CONDITION_SIGNALED};

    magma_semaphore_id_t id;
    ASSERT_EQ(MAGMA_STATUS_OK,
              magma_connection_create_semaphore(connection_, &item.semaphore, &id));
    EXPECT_NE(0u, id);

    auto start_time = std::chrono::steady_clock::now();

    constexpr std::chrono::seconds kSignalDelay(10);

    // The sleep may wake up early due to slack, so allow for that.
    constexpr std::chrono::milliseconds kSignalSlack(100);

    std::thread signal_thread([&]() {
      std::this_thread::sleep_for(kSignalDelay);
      magma_semaphore_signal(item.semaphore);
    });

    constexpr uint32_t kTimeoutS = 200;
    constexpr uint64_t kNsPerS = 1000000000;

    magma_status_t status =
        magma_poll(&item, 1, forever_deadline ? UINT64_MAX : kTimeoutS * kNsPerS);
    auto end_time = std::chrono::steady_clock::now();

    auto duration = std::chrono::duration_cast<std::chrono::milliseconds>(end_time - start_time);

    EXPECT_LE(kSignalDelay - kSignalSlack, duration) << duration.count();

    EXPECT_EQ(status, MAGMA_STATUS_OK);
    EXPECT_EQ(item.result, MAGMA_POLL_CONDITION_SIGNALED);
    signal_thread.join();
    magma_connection_release_semaphore(connection_, item.semaphore);
  }

  static void CheckNativeHandle(magma_handle_t handle, bool expect_signaled) {
#if defined(__Fuchsia__)
    zx_handle_t zx_handle = handle;
    if (expect_signaled) {
      EXPECT_EQ(ZX_OK, zx_object_wait_one(zx_handle, ZX_EVENT_SIGNALED, /*deadline=*/0,
                                          /*observed=*/nullptr));
    } else {
      EXPECT_EQ(ZX_ERR_TIMED_OUT, zx_object_wait_one(zx_handle, ZX_EVENT_SIGNALED, /*deadline=*/0,
                                                     /*observed=*/nullptr));
    }
#elif defined(__linux__)
    struct pollfd pfd = {
        .fd = static_cast<int>(handle),
        .events = POLLIN,
        .revents = 0,
    };
    if (expect_signaled) {
      EXPECT_EQ(1, poll(&pfd, 1, /*timeout=*/0));
      EXPECT_EQ(POLLIN, pfd.revents);
    } else {
      EXPECT_EQ(0, poll(&pfd, 1, /*timeout=*/0));
      EXPECT_EQ(0, pfd.revents);
    }
#endif
  }

  void SemaphoreExport(magma_handle_t* handle_out) {
    ASSERT_TRUE(connection_);

    magma_semaphore_t semaphore;
    magma_semaphore_id_t id;
    ASSERT_EQ(magma_connection_create_semaphore(connection_, &semaphore, &id), MAGMA_STATUS_OK);
    EXPECT_EQ(magma_semaphore_export(semaphore, handle_out), MAGMA_STATUS_OK);

    EXPECT_NO_FATAL_FAILURE(CheckNativeHandle(*handle_out, false));
    magma_semaphore_signal(semaphore);
    EXPECT_NO_FATAL_FAILURE(CheckNativeHandle(*handle_out, true));
    magma_semaphore_reset(semaphore);
    EXPECT_NO_FATAL_FAILURE(CheckNativeHandle(*handle_out, false));

    magma_connection_release_semaphore(connection_, semaphore);
  }

  void SemaphoreImport2(magma_handle_t handle, bool one_shot = false) {
    ASSERT_TRUE(connection_);

    magma_semaphore_t semaphore;
    magma_semaphore_id_t id;
    uint64_t flags = one_shot ? MAGMA_IMPORT_SEMAPHORE_ONE_SHOT : 0;
    ASSERT_EQ(magma_connection_import_semaphore2(connection_, handle, flags, &semaphore, &id),
              MAGMA_STATUS_OK);

    {
      magma_poll_item_t item = {
          .semaphore = semaphore,
          .type = MAGMA_POLL_TYPE_SEMAPHORE,
          .condition = MAGMA_POLL_CONDITION_SIGNALED,
          .result = 0,
      };
      ASSERT_EQ(MAGMA_STATUS_TIMED_OUT, magma_poll(&item, /*count=*/1, /*timeout=*/0));
    }

    magma_semaphore_signal(semaphore);

    {
      magma_poll_item_t item = {
          .semaphore = semaphore,
          .type = MAGMA_POLL_TYPE_SEMAPHORE,
          .condition = MAGMA_POLL_CONDITION_SIGNALED,
          .result = 0,
      };
      ASSERT_EQ(MAGMA_STATUS_OK, magma_poll(&item, /*count=*/1, /*timeout=*/0));
    }

    magma_semaphore_reset(semaphore);

    {
      magma_poll_item_t item = {
          .semaphore = semaphore,
          .type = MAGMA_POLL_TYPE_SEMAPHORE,
          .condition = MAGMA_POLL_CONDITION_SIGNALED,
          .result = 0,
      };
      if (one_shot) {
        ASSERT_EQ(MAGMA_STATUS_OK, magma_poll(&item, /*count=*/1, /*timeout=*/0));
      } else {
        ASSERT_EQ(MAGMA_STATUS_TIMED_OUT, magma_poll(&item, /*count=*/1, /*timeout=*/0));
      }
    }

    magma_connection_release_semaphore(connection_, semaphore);
  }

  void InlineCommands() {
    ASSERT_TRUE(connection_);

    uint32_t context_id;
    EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_create_context(connection_, &context_id));
    EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_flush(connection_));

    uint64_t some_pattern = 0xabcd12345678beef;
    uint64_t invalid_semaphore_id = 0;
    magma_inline_command_buffer inline_command_buffer = {
        .data = &some_pattern,
        .size = sizeof(some_pattern),
        .semaphore_ids = &invalid_semaphore_id,
        .semaphore_count = 1,
    };

    magma_status_t status = magma_connection_execute_inline_commands(connection_, context_id, 1,
                                                                     &inline_command_buffer);
    if (status == MAGMA_STATUS_OK) {
      // Invalid semaphore ID prevents execution of pattern data
      EXPECT_EQ(MAGMA_STATUS_INVALID_ARGS, magma_connection_flush(connection_));
    } else {
      EXPECT_EQ(MAGMA_STATUS_INVALID_ARGS, status);
    }

    magma_connection_release_context(connection_, context_id);
  }

  void Sysmem(bool use_format_modifier) {
#if !defined(__Fuchsia__)
    GTEST_SKIP();
#else
    magma_sysmem_connection_t connection;
    zx::channel local_endpoint, server_endpoint;
    EXPECT_EQ(ZX_OK, zx::channel::create(0u, &local_endpoint, &server_endpoint));
    EXPECT_EQ(ZX_OK,
              fdio_service_connect("/svc/fuchsia.sysmem2.Allocator", server_endpoint.release()));
    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_sysmem2_connection_import(local_endpoint.release(), &connection));

    magma_buffer_collection_t collection;
    EXPECT_EQ(MAGMA_STATUS_OK, magma_sysmem_connection_import_buffer_collection(
                                   connection, ZX_HANDLE_INVALID, &collection));

    magma_buffer_format_constraints_t buffer_constraints{};

    buffer_constraints.count = 1;
    buffer_constraints.usage = 0;
    buffer_constraints.secure_permitted = false;
    buffer_constraints.secure_required = false;
    buffer_constraints.cpu_domain_supported = true;
    buffer_constraints.min_buffer_count_for_camping = 1;
    buffer_constraints.min_buffer_count_for_dedicated_slack = 1;
    buffer_constraints.min_buffer_count_for_shared_slack = 1;
    buffer_constraints.options = MAGMA_BUFFER_FORMAT_CONSTRAINT_OPTIONS_EXTRA_COUNTS;
    magma_sysmem_buffer_constraints_t constraints;
    EXPECT_EQ(MAGMA_STATUS_OK, magma_sysmem_connection_create_buffer_constraints(
                                   connection, &buffer_constraints, &constraints));

    // Create a set of basic 512x512 RGBA image constraints.
    magma_image_format_constraints2_t image_constraints{};
    image_constraints.image_format = MAGMA_FORMAT_R8G8B8A8;
    image_constraints.has_format_modifier = use_format_modifier;
    image_constraints.format_modifier = use_format_modifier ? MAGMA_FORMAT_MODIFIER_LINEAR : 0;
    image_constraints.width = 512;
    image_constraints.height = 512;
    image_constraints.plane_count = 1;
    image_constraints.bytes_per_row_divisor = 1;
    image_constraints.min_bytes_per_row = 0;
    image_constraints.pad_for_block_width = 0;
    image_constraints.pad_for_block_height = 0;

    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_buffer_constraints_set_format(constraints, 0, &image_constraints));

    uint32_t color_space_in = MAGMA_COLORSPACE_SRGB;
    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_buffer_constraints_set_colorspaces2(constraints, 0, 1, &color_space_in));

    EXPECT_EQ(MAGMA_STATUS_OK, magma_buffer_collection_set_constraints2(collection, constraints));

    // Buffer should be allocated now.
    magma_collection_info_t collection_info;
    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_buffer_collection_get_collection_info(collection, &collection_info));

    uint32_t expected_buffer_count = buffer_constraints.min_buffer_count_for_camping +
                                     buffer_constraints.min_buffer_count_for_dedicated_slack +
                                     buffer_constraints.min_buffer_count_for_shared_slack;
    uint32_t buffer_count;
    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_collection_info_get_buffer_count(collection_info, &buffer_count));
    EXPECT_EQ(expected_buffer_count, buffer_count);
    magma_bool_t is_secure;
    EXPECT_EQ(MAGMA_STATUS_OK, magma_collection_info_get_is_secure(collection_info, &is_secure));
    EXPECT_FALSE(is_secure);

    uint32_t format;
    EXPECT_EQ(MAGMA_STATUS_OK, magma_collection_info_get_format(collection_info, &format));
    EXPECT_EQ(MAGMA_FORMAT_R8G8B8A8, format);
    uint32_t color_space = 0;
    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_collection_info_get_color_space(collection_info, &color_space));
    EXPECT_EQ(MAGMA_COLORSPACE_SRGB, color_space);

    magma_bool_t has_format_modifier;
    uint64_t format_modifier;
    EXPECT_EQ(MAGMA_STATUS_OK, magma_collection_info_get_format_modifier(
                                   collection_info, &has_format_modifier, &format_modifier));
    if (has_format_modifier) {
      EXPECT_EQ(MAGMA_FORMAT_MODIFIER_LINEAR, format_modifier);
    }

    magma_image_plane_t planes[4];
    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_collection_info_get_plane_info_with_size(collection_info, 512u, 512u, planes));
    EXPECT_EQ(512 * 4u, planes[0].bytes_per_row);
    EXPECT_EQ(0u, planes[0].byte_offset);
    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_collection_info_get_plane_info_with_size(collection_info, 512, 512, planes));
    EXPECT_EQ(512 * 4u, planes[0].bytes_per_row);
    EXPECT_EQ(0u, planes[0].byte_offset);

    magma_collection_info_release(collection_info);

    magma_handle_t handle;
    uint32_t offset;
    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_buffer_collection_get_buffer_handle(collection, 0, &handle, &offset));
    EXPECT_EQ(ZX_OK, zx_handle_close(handle));

    magma_buffer_collection_release2(collection);
    magma_buffer_constraints_release2(constraints);
    magma_sysmem_connection_release(connection);
#endif
  }

  void TracingInit() {
#if defined(__Fuchsia__)
    zx::channel local_endpoint, server_endpoint;
    EXPECT_EQ(ZX_OK, zx::channel::create(0u, &local_endpoint, &server_endpoint));
    EXPECT_EQ(ZX_OK, fdio_service_connect("/svc/fuchsia.tracing.provider.Registry",
                                          server_endpoint.release()));
    EXPECT_EQ(MAGMA_STATUS_OK, magma_initialize_tracing(local_endpoint.release()));

#if !defined(MAGMA_HERMETIC)
    if (magma::PlatformTraceProvider::Get())
      EXPECT_TRUE(magma::PlatformTraceProvider::Get()->IsInitialized());
#endif

#else
    int handle = -1;
    EXPECT_EQ(MAGMA_STATUS_OK, magma_initialize_tracing(handle));
#endif
  }

  void TracingInitFake() {
#if defined(__Fuchsia__)
    auto endpoints = fidl::Endpoints<fuchsia_tracing_provider::Registry>::Create();
    async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
    FakeTraceRegistry registry(loop);

    fidl::BindServer(loop.dispatcher(), std::move(endpoints.server), &registry);

    EXPECT_EQ(MAGMA_STATUS_OK, magma_initialize_tracing(endpoints.client.TakeChannel().release()));
    // The loop runs until RegisterProvider is received.
    loop.Run();
#else
    int handle = -1;
    EXPECT_EQ(MAGMA_STATUS_OK, magma_initialize_tracing(handle));
#endif
  }

  void LoggingInit() {
#if defined(__Fuchsia__) && !defined(MAGMA_HERMETIC)
    // Logging should be set up by the test fixture, so just add more logs here to help manually
    // verify that the fixture is working correctly.
    EXPECT_TRUE(magma::PlatformLoggerProvider::IsInitialized());
    MAGMA_LOG(INFO, "LoggingInit test complete");
#endif
  }

  void LoggingInitFake() {
#if defined(__Fuchsia__)
    auto endpoints = fidl::Endpoints<fuchsia_logger::LogSink>::Create();
    fake_log_sink_.emplace(fuchsia_logging::Info, std::move(endpoints.server));

    EXPECT_EQ(MAGMA_STATUS_OK, magma_initialize_logging(endpoints.client.TakeChannel().release()));
#else
    int handle = -1;
    EXPECT_EQ(MAGMA_STATUS_OK, magma_initialize_logging(handle));
#endif
  }

  void GetDeviceIdImported() {
    ASSERT_TRUE(device_);

    // Ensure failure if result pointer not provided
    EXPECT_EQ(MAGMA_STATUS_INVALID_ARGS,
              magma_device_query(device_, MAGMA_QUERY_DEVICE_ID, nullptr, nullptr));

    uint64_t device_id = 0;
    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_device_query(device_, MAGMA_QUERY_DEVICE_ID, nullptr, &device_id));
    EXPECT_NE(0u, device_id);

    magma_handle_t unused;
    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_device_query(device_, MAGMA_QUERY_DEVICE_ID, &unused, &device_id));
    EXPECT_FALSE(is_valid_handle(unused));
    EXPECT_NE(0u, device_id);
  }

  void GetVendorIdImported() {
    ASSERT_TRUE(device_);

    // Ensure failure if result pointer not provided
    EXPECT_EQ(MAGMA_STATUS_INVALID_ARGS,
              magma_device_query(device_, MAGMA_QUERY_VENDOR_ID, nullptr, nullptr));

    uint64_t vendor_id = 0;
    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_device_query(device_, MAGMA_QUERY_VENDOR_ID, nullptr, &vendor_id));
    EXPECT_NE(0u, vendor_id);

    magma_handle_t unused;
    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_device_query(device_, MAGMA_QUERY_VENDOR_ID, &unused, &vendor_id));
    EXPECT_FALSE(is_valid_handle(unused));
    EXPECT_NE(0u, vendor_id);
  }

  void GetVendorVersionImported() {
    ASSERT_TRUE(device_);

    // Ensure failure if result pointer not provided
    EXPECT_EQ(MAGMA_STATUS_INVALID_ARGS,
              magma_device_query(device_, MAGMA_QUERY_VENDOR_VERSION, nullptr, nullptr));

    uint64_t vendor_version = 0;
    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_device_query(device_, MAGMA_QUERY_VENDOR_VERSION, nullptr, &vendor_version));
    EXPECT_NE(0u, vendor_version);

    magma_handle_t unused;
    vendor_version = 0;
    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_device_query(device_, MAGMA_QUERY_VENDOR_VERSION, &unused, &vendor_version));
    EXPECT_FALSE(is_valid_handle(unused));
    EXPECT_NE(0u, vendor_version);
  }

  void QueryReturnsBufferImported(bool leaky = false, bool check_clock = false) {
    ASSERT_TRUE(device_);
    ASSERT_TRUE(connection_);

    std::optional<uint64_t> maybe_get_device_timestamp_query_id;

#ifdef __Fuchsia__
    auto result = vendor_helper_->GetConfig();
    ASSERT_TRUE(result.ok()) << " status " << result.status();

    auto get_device_timestamp_type =
        result.Unwrap()->has_get_device_timestamp_type()
            ? result.Unwrap()->get_device_timestamp_type()
            : ::fuchsia_gpu_magma_test::GetDeviceTimestampType::kNotImplemented;

    switch (get_device_timestamp_type) {
      case ::fuchsia_gpu_magma_test::GetDeviceTimestampType::kNotImplemented:
        break;
      case ::fuchsia_gpu_magma_test::GetDeviceTimestampType::kSupported:
        EXPECT_TRUE(result.Unwrap()->has_get_device_timestamp_query_id());
        maybe_get_device_timestamp_query_id = result.Unwrap()->get_device_timestamp_query_id();
        break;
      default:
        ASSERT_TRUE(false) << "Unhandled get_device_timestamp_type";
    }
#endif

    if (!maybe_get_device_timestamp_query_id) {
      GTEST_SKIP();
    }

    // Ensure failure if handle pointer not provided
    EXPECT_EQ(MAGMA_STATUS_INVALID_ARGS,
              magma_device_query(device_, *maybe_get_device_timestamp_query_id, nullptr, nullptr));

    uint64_t before_ns = clock_gettime_monotonic_raw();

    magma_handle_t buffer_handle;
    EXPECT_EQ(MAGMA_STATUS_OK, magma_device_query(device_, *maybe_get_device_timestamp_query_id,
                                                  &buffer_handle, nullptr));
    EXPECT_TRUE(is_valid_handle(buffer_handle));

    uint64_t after_ns = clock_gettime_monotonic_raw();

    ASSERT_NE(0u, buffer_handle);

#if defined(__Fuchsia__)
    zx_vaddr_t zx_vaddr;
    size_t size = page_size();
    {
      zx::vmo vmo(buffer_handle);

      ASSERT_EQ(ZX_OK, zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE,
                                                  0,  // vmar_offset,
                                                  vmo, 0 /*offset*/, size, &zx_vaddr));
    }

    // Check that clock_gettime is synchronized between client and driver.
    // Required for clients using VK_EXT_calibrated_timestamps.
    if (check_clock) {
      auto result = vendor_helper_->ValidateCalibratedTimestamps(
          fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(zx_vaddr), size),
          before_ns, after_ns);
      ASSERT_TRUE(result.ok());
      bool validate_result = result.Unwrap()->result;
      EXPECT_TRUE(validate_result);
    }

    if (!leaky) {
      EXPECT_EQ(ZX_OK, zx::vmar::root_self()->unmap(zx_vaddr, page_size()));
    }
#else
    (void)before_ns;
    (void)after_ns;
#endif  // __Fuchsia__
  }

  void BufferCaching(magma_cache_policy_t policy) {
    uint64_t size = page_size() + 16;
    uint64_t actual_size = 0;
    magma_buffer_t buffer = 0;
    magma_buffer_id_t buffer_id = 0;

    ASSERT_EQ(MAGMA_STATUS_OK, magma_connection_create_buffer(connection(), size, &actual_size,
                                                              &buffer, &buffer_id));

    EXPECT_EQ(MAGMA_STATUS_OK, magma_buffer_set_cache_policy(buffer, policy));

    {
      magma_cache_policy_t policy_check;
      EXPECT_EQ(MAGMA_STATUS_OK, magma_buffer_get_cache_policy(buffer, &policy_check));
      EXPECT_EQ(policy_check, policy);
    }

    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_buffer_clean_cache(buffer, 0, actual_size, MAGMA_CACHE_OPERATION_CLEAN));
    EXPECT_EQ(MAGMA_STATUS_OK, magma_buffer_clean_cache(buffer, 0, actual_size,
                                                        MAGMA_CACHE_OPERATION_CLEAN_INVALIDATE));

    magma_connection_release_buffer(connection(), buffer);
  }

  void BufferNaming() {
    uint64_t size = page_size() + 16;
    uint64_t actual_size = 0;
    magma_buffer_t buffer = 0;
    magma_buffer_id_t buffer_id = 0;

    ASSERT_EQ(MAGMA_STATUS_OK, magma_connection_create_buffer(connection(), size, &actual_size,
                                                              &buffer, &buffer_id));

    const char* kSomeName = "some_name";
    EXPECT_EQ(MAGMA_STATUS_OK, magma_buffer_set_name(buffer, kSomeName));

#if defined(__Fuchsia__)
    zx::vmo vmo;
    ASSERT_EQ(MAGMA_STATUS_OK, magma_buffer_get_handle(buffer, vmo.reset_and_get_address()));
    char name[ZX_MAX_NAME_LEN] = {};
    ASSERT_EQ(ZX_OK, vmo.get_property(ZX_PROP_NAME, name, sizeof(name)));
    EXPECT_EQ(0, strcmp(name, kSomeName));
#endif

    magma_connection_release_buffer(connection(), buffer);
  }

#if defined(__Fuchsia__)
  void CheckAccessWithInvalidToken(magma_status_t expected_result) {
    FakePerfCountAccessServer server;
    async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
    ASSERT_EQ(ZX_OK, loop.StartThread("server-loop"));

    auto endpoints = fidl::Endpoints<fuchsia_gpu_magma::PerformanceCounterAccess>::Create();
    fidl::BindServer(loop.dispatcher(), std::move(endpoints.server), &server);

    EXPECT_EQ(expected_result, magma_connection_enable_performance_counter_access(
                                   connection_, endpoints.client.TakeChannel().release()));
  }
#endif

  void EnablePerformanceCounters() {
#if !defined(__Fuchsia__)
    GTEST_SKIP();
#else
    CheckAccessWithInvalidToken(MAGMA_STATUS_ACCESS_DENIED);

    bool success = false;
    for (auto& p :
         std::filesystem::directory_iterator("/svc/fuchsia.gpu.magma.PerformanceCounterService")) {
      zx::channel server_end, client_end;
      zx::channel::create(0, &server_end, &client_end);

      zx_status_t zx_status = fdio_service_connect(
          (static_cast<std::string>(p.path()) + "/access").c_str(), server_end.release());
      EXPECT_EQ(ZX_OK, zx_status);
      magma_status_t status =
          magma_connection_enable_performance_counter_access(connection_, client_end.release());
      EXPECT_TRUE(status == MAGMA_STATUS_OK || status == MAGMA_STATUS_ACCESS_DENIED);
      if (status == MAGMA_STATUS_OK) {
        success = true;
      }
    }
    EXPECT_TRUE(success);
    // Access should remain enabled even though an invalid token is used.
    CheckAccessWithInvalidToken(MAGMA_STATUS_OK);
#endif
  }

  void DisabledPerformanceCounters() {
#if !defined(__Fuchsia__)
    GTEST_SKIP();
#else
    uint64_t counter = 5;
    magma_semaphore_t semaphore;
    magma_semaphore_id_t semaphore_id;
    ASSERT_EQ(magma_connection_create_semaphore(connection_, &semaphore, &semaphore_id),
              MAGMA_STATUS_OK);
    uint64_t size = page_size();
    magma_buffer_t buffer;
    magma_buffer_id_t buffer_id;
    ASSERT_EQ(MAGMA_STATUS_OK,
              magma_connection_create_buffer(connection_, size, &size, &buffer, &buffer_id));

    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_connection_enable_performance_counters(connection_, &counter, 1));
    EXPECT_EQ(MAGMA_STATUS_ACCESS_DENIED, magma_connection_flush(connection_));

    magma_connection_release_buffer(connection_, buffer);
    magma_connection_release_semaphore(connection_, semaphore);
#endif
  }

 protected:
  std::string device_name_;
  bool is_virtmagma_ = false;
  int fd_ = -1;
  magma_device_t device_ = 0;
  magma_connection_t connection_ = 0;
#ifdef __Fuchsia__
  fidl::WireSyncClient<fuchsia_gpu_magma_test::VendorHelper> vendor_helper_;
  std::optional<fuchsia_logging::FakeLogSink> fake_log_sink_;
#endif
};

class TestConnectionWithContext : public TestConnection {
 public:
  TestConnectionWithContext() {
    if (connection()) {
      EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_create_context(connection(), &context_id_));
    }
  }

  ~TestConnectionWithContext() {
    if (connection()) {
      magma_connection_release_context(connection(), context_id_);
    }
  }

  uint32_t context_id() { return context_id_; }

  void ExecuteCommand(uint32_t resource_count, uint32_t wait_semaphore_count = 0,
                      uint32_t signal_semaphore_count = 0) {
    ASSERT_TRUE(connection());

    magma_exec_command_buffer command_buffer = {.resource_index = 0, .start_offset = 0};

    std::vector<magma_exec_resource> resources(resource_count);
    memset(resources.data(), 0, sizeof(magma_exec_resource) * resources.size());

    std::vector<magma_semaphore_t> semaphores(signal_semaphore_count + wait_semaphore_count);
    std::vector<magma_semaphore_id_t> semaphore_ids(signal_semaphore_count + wait_semaphore_count);

    for (uint32_t i = 0; i < signal_semaphore_count + wait_semaphore_count; i++) {
      ASSERT_EQ(MAGMA_STATUS_OK,
                magma_connection_create_semaphore(connection(), &semaphores[i], &semaphore_ids[i]));
    }

    magma_command_descriptor descriptor = {.resource_count = resource_count,
                                           .command_buffer_count = 1,
                                           .wait_semaphore_count = wait_semaphore_count,
                                           .signal_semaphore_count = signal_semaphore_count,
                                           .resources = resources.data(),
                                           .command_buffers = &command_buffer,
                                           .semaphore_ids = semaphore_ids.data()};

    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_connection_execute_command(connection(), context_id(), &descriptor));

    // Command buffer is mostly zeros, so we expect an error here
    EXPECT_EQ(MAGMA_STATUS_INVALID_ARGS, magma_connection_flush(connection()));

    for (uint32_t i = 0; i < signal_semaphore_count + wait_semaphore_count; i++) {
      magma_connection_release_semaphore(connection(), semaphores[i]);
    }
  }

  void ExecuteCommandNoResources() {
    ASSERT_TRUE(connection());

    magma_command_descriptor descriptor = {.resource_count = 0, .command_buffer_count = 0};

    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_connection_execute_command(connection(), context_id(), &descriptor));

    // Empty command buffers may or may not be valid.
    magma_status_t status = magma_connection_flush(connection());

    EXPECT_TRUE(status == MAGMA_STATUS_OK || status == MAGMA_STATUS_UNIMPLEMENTED ||
                status == MAGMA_STATUS_INVALID_ARGS);

#ifdef __Fuchsia__
    auto result = vendor_helper_->GetConfig();
    EXPECT_TRUE(result.ok());

    auto execute_command_no_resources_type =
        result.Unwrap()->has_execute_command_no_resources_type()
            ? result.Unwrap()->execute_command_no_resources_type()
            : ::fuchsia_gpu_magma_test::ExecuteCommandNoResourcesType::kUnknown;

    switch (execute_command_no_resources_type) {
      case ::fuchsia_gpu_magma_test::ExecuteCommandNoResourcesType::kUnknown:
        break;
      case ::fuchsia_gpu_magma_test::ExecuteCommandNoResourcesType::kSupported:
        EXPECT_TRUE(status == MAGMA_STATUS_OK) << "status: " << status;
        break;
      case ::fuchsia_gpu_magma_test::ExecuteCommandNoResourcesType::kNotImplemented:
        EXPECT_TRUE(status == MAGMA_STATUS_UNIMPLEMENTED) << "status: " << status;
        break;
      case ::fuchsia_gpu_magma_test::ExecuteCommandNoResourcesType::kInvalid:
        EXPECT_TRUE(status == MAGMA_STATUS_INVALID_ARGS) << "status: " << status;
        break;
      default:
        ASSERT_TRUE(false) << "Unhandled execute_command_no_resources_type";
    }
#endif
  }

  void ExecuteCommandTwoCommandBuffers() {
    ASSERT_TRUE(connection());

    std::array<magma_exec_resource, 2> resources{};
    std::array<magma_exec_command_buffer, 2> command_buffers = {
        magma_exec_command_buffer{.resource_index = 0, .start_offset = 0},
        magma_exec_command_buffer{.resource_index = 1, .start_offset = 0}};

    magma_command_descriptor descriptor = {.resource_count = resources.size(),
                                           .command_buffer_count = command_buffers.size(),
                                           .resources = resources.data(),
                                           .command_buffers = command_buffers.data()};

    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_connection_execute_command(connection(), context_id(), &descriptor));

    magma_status_t status = magma_connection_flush(connection());
    EXPECT_TRUE(status == MAGMA_STATUS_UNIMPLEMENTED || status == MAGMA_STATUS_INVALID_ARGS);
  }

 private:
  uint32_t context_id_;
};

class Magma : public testing::Test {
 protected:
  void SetUp() override {
#if defined(__Fuchsia__)
    zx::channel local_endpoint, server_endpoint;
    EXPECT_EQ(ZX_OK, zx::channel::create(0u, &local_endpoint, &server_endpoint));
    EXPECT_EQ(ZX_OK,
              fdio_service_connect("/svc/fuchsia.logger.LogSink", server_endpoint.release()));
    EXPECT_EQ(MAGMA_STATUS_OK, magma_initialize_logging(local_endpoint.release()));
#else
    int handle = -1;
    EXPECT_EQ(MAGMA_STATUS_OK, magma_initialize_logging(handle));
#endif
  }
};

TEST_F(Magma, LoggingInit) {
  TestConnection test;
  test.LoggingInit();
}

TEST(MagmaNoDefaultLogging, LoggingInitFake) {
  TestConnection test;
  test.LoggingInitFake();
}

TEST_F(Magma, DeviceId) {
  TestConnection test;
  test.GetDeviceIdImported();
}

TEST_F(Magma, VendorId) {
  TestConnection test;
  test.GetVendorIdImported();
}

TEST_F(Magma, VendorVersion) {
  TestConnection test;
  test.GetVendorVersionImported();
}

TEST_F(Magma, QueryReturnsBuffer) {
  TestConnection test;
  test.QueryReturnsBufferImported();
}

// Test for cleanup of leaked mapping
TEST_F(Magma, QueryReturnsBufferLeaky) {
  constexpr bool kLeaky = true;
  TestConnection test;
  test.QueryReturnsBufferImported(kLeaky);
}

TEST_F(Magma, QueryReturnsBufferCalibratedTimestamps) {
  constexpr bool kLeaky = false;
  constexpr bool kCheckClock = true;
  TestConnection test;
  test.QueryReturnsBufferImported(kLeaky, kCheckClock);
}

TEST_F(Magma, TracingInit) {
  TestConnection test;
  test.TracingInit();
}

TEST_F(Magma, TracingInitFake) {
  TestConnection test;
  test.TracingInitFake();
}

TEST_F(Magma, Buffer) {
  TestConnection test;
  test.Buffer();
}

TEST_F(Magma, Connection) {
  TestConnection test;
  test.Connection();
}

TEST_F(Magma, Context) {
  TestConnection test;
  test.Context();
}

TEST_F(Magma, Context2) {
  TestConnection test;
  test.Context2();
}

TEST_F(Magma, NotificationChannelHandle) {
  TestConnection test;
  test.NotificationChannelHandle();
}

TEST_F(Magma, ReadNotificationChannel) {
  TestConnection test;
  test.ReadNotificationChannel();
}

TEST_F(Magma, BufferMap) {
  TestConnection test;
  test.BufferMap();
}

TEST_F(Magma, BufferMapInvalidFlush) {
  TestConnection test;
  test.BufferMapInvalid(/*flush=*/true);
}

TEST_F(Magma, BufferMapInvalidGetError) {
  TestConnection test;
  test.BufferMapInvalid(/*flush=*/false);
}

TEST_F(Magma, BufferMapOverlapError) {
  TestConnection test;
  test.BufferMapOverlapError();
}

TEST_F(Magma, BufferMapDuplicates) {
  TestConnection test;
  test.BufferMapDuplicates(31);  // MSDs are limited by the kernel BTI pin limit
}

TEST_F(Magma, BufferImportInvalid) { TestConnection().BufferImportInvalid(); }

TEST_F(Magma, BufferImportExport) {
  TestConnection test1;
  TestConnection test2;

  uint32_t handle;
  uint64_t exported_id;
  test1.BufferExport(&handle, &exported_id);
  test2.BufferImport(handle, exported_id);
}

TEST_F(Magma, Semaphore) {
  TestConnection test;
  test.Semaphore(1);
  test.Semaphore(2);
  test.Semaphore(3);
}

TEST_F(Magma, SemaphoreExportImport2) {
  TestConnection test1;
  TestConnection test2;
  magma_handle_t handle;
  test1.SemaphoreExport(&handle);
  test2.SemaphoreImport2(handle);
}

TEST_F(Magma, SemaphoreExportImportOneShot) {
  TestConnection test1;
  TestConnection test2;
  magma_handle_t handle;
  test1.SemaphoreExport(&handle);
  test2.SemaphoreImport2(handle, /*one_shot=*/true);
}

TEST_F(Magma, InlineCommands) { TestConnection().InlineCommands(); }

class MagmaPoll : public testing::TestWithParam<uint32_t> {};

TEST_P(MagmaPoll, PollWithNotificationChannel) {
  uint32_t semaphore_count = GetParam();
  TestConnection().PollWithNotificationChannel(semaphore_count);
}

INSTANTIATE_TEST_SUITE_P(MagmaPoll, MagmaPoll, ::testing::Values(0, 1, 2, 3));

TEST_F(Magma, PollWithTestChannel) { TestConnection().PollWithTestChannel(); }

TEST_F(Magma, PollChannelClosed) { TestConnection().PollChannelClosed(); }

TEST_F(Magma, PollLongDeadline) { TestConnection().PollLongDeadline(/*forever_deadline=*/false); }

TEST_F(Magma, PollInfiniteDeadline) {
  TestConnection().PollLongDeadline(/*forever_deadline=*/true);
}

TEST_F(Magma, Sysmem) {
  TestConnection test;
  test.Sysmem(false);
}

TEST_F(Magma, SysmemLinearFormatModifier) {
  TestConnection test;
  test.Sysmem(true);
}

TEST_F(Magma, FromC) { EXPECT_TRUE(test_magma_from_c(TestConnection().device_name().c_str())); }

TEST_F(Magma, ExecuteCommand) { TestConnectionWithContext().ExecuteCommand(5); }

TEST_F(Magma, ExecuteCommandWaitSemaphore) {
  TestConnectionWithContext().ExecuteCommand(5, /*wait_semaphore_count=*/1);
}

TEST_F(Magma, ExecuteCommandSignalSemaphore) {
  TestConnectionWithContext().ExecuteCommand(5, /*wait_semaphore_count=*/0,
                                             /*signal_semaphore_count=*/1);
}

TEST_F(Magma, ExecuteCommandNoResources) {
  TestConnectionWithContext().ExecuteCommandNoResources();
}

TEST_F(Magma, ExecuteCommandTwoCommandBuffers) {
  TestConnectionWithContext().ExecuteCommandTwoCommandBuffers();
}

TEST_F(Magma, FlowControl) {
  TestConnection test;

  // Each call to Buffer is 2 messages.
  // Without flow control, this will trigger a policy exception (too many channel messages)
  // or an OOM.
  constexpr uint32_t kIterations = 10000 / 2;

  for (uint32_t i = 0; i < kIterations; i++) {
    test.Buffer();
  }
}

TEST_F(Magma, EnablePerformanceCounters) { TestConnection().EnablePerformanceCounters(); }

TEST_F(Magma, DisabledPerformanceCounters) { TestConnection().DisabledPerformanceCounters(); }

TEST_F(Magma, BufferCommit) {
  TestConnection connection;
  magma_buffer_t buffer;
  uint64_t size_out;
  uint64_t buffer_size = page_size() * 10;
  uint64_t buffer_id;
  EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_create_buffer(connection.connection(), buffer_size,
                                                            &size_out, &buffer, &buffer_id));
  {
    magma_buffer_info_t info;
    EXPECT_EQ(MAGMA_STATUS_OK, magma_buffer_get_info(buffer, &info));
    EXPECT_EQ(info.size, buffer_size);
    EXPECT_EQ(0u, info.committed_byte_count);
  }

  EXPECT_EQ(MAGMA_STATUS_INVALID_ARGS,
            magma_connection_perform_buffer_op(connection.connection(), buffer,
                                               MAGMA_BUFFER_RANGE_OP_COMMIT, 0, page_size() + 1));
  EXPECT_EQ(MAGMA_STATUS_MEMORY_ERROR, magma_connection_perform_buffer_op(
                                           connection.connection(), buffer,
                                           MAGMA_BUFFER_RANGE_OP_COMMIT, page_size(), buffer_size));
  EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_perform_buffer_op(connection.connection(), buffer,
                                                                MAGMA_BUFFER_RANGE_OP_COMMIT,
                                                                page_size(), page_size()));
  {
    magma_buffer_info_t info;
    EXPECT_EQ(MAGMA_STATUS_OK, magma_buffer_get_info(buffer, &info));
    EXPECT_EQ(page_size(), info.committed_byte_count);
  }

  EXPECT_EQ(MAGMA_STATUS_INVALID_ARGS,
            magma_connection_perform_buffer_op(connection.connection(), buffer,
                                               MAGMA_BUFFER_RANGE_OP_DECOMMIT, 0, page_size() + 1));
  EXPECT_EQ(
      MAGMA_STATUS_INVALID_ARGS,
      magma_connection_perform_buffer_op(connection.connection(), buffer,
                                         MAGMA_BUFFER_RANGE_OP_DECOMMIT, page_size(), buffer_size));
  EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_perform_buffer_op(connection.connection(), buffer,
                                                                MAGMA_BUFFER_RANGE_OP_DECOMMIT,
                                                                2 * page_size(), page_size()));
  {
    magma_buffer_info_t info;
    EXPECT_EQ(MAGMA_STATUS_OK, magma_buffer_get_info(buffer, &info));
    EXPECT_EQ(page_size(), info.committed_byte_count);
  }

  EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_perform_buffer_op(connection.connection(), buffer,
                                                                MAGMA_BUFFER_RANGE_OP_DECOMMIT,
                                                                page_size(), page_size()));
  {
    magma_buffer_info_t info;
    EXPECT_EQ(MAGMA_STATUS_OK, magma_buffer_get_info(buffer, &info));
    EXPECT_EQ(0u, info.committed_byte_count);
  }

  magma_connection_release_buffer(connection.connection(), buffer);
}

TEST_F(Magma, MapWithBufferHandle2) {
  TestConnection connection;

  magma_buffer_t buffer;
  uint64_t actual_size;
  constexpr uint64_t kBufferSizeInPages = 10;
  uint64_t buffer_id;
  EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_create_buffer(connection.connection(),
                                                            kBufferSizeInPages * page_size(),
                                                            &actual_size, &buffer, &buffer_id));

  magma_handle_t handle;
  ASSERT_EQ(MAGMA_STATUS_OK, magma_buffer_get_handle(buffer, &handle));

  void* full_range_ptr;
  ASSERT_TRUE(magma::MapCpuHelper(buffer, 0 /*offset*/, actual_size, &full_range_ptr));

  // Some arbitrary constants
  constexpr uint32_t kPattern[] = {
      0x12345678,
      0x89abcdef,
      0xfedcba98,
      0x87654321,
  };

  reinterpret_cast<uint32_t*>(full_range_ptr)[0] = kPattern[0];
  reinterpret_cast<uint32_t*>(full_range_ptr)[1] = kPattern[1];
  reinterpret_cast<uint32_t*>(full_range_ptr)[actual_size / sizeof(uint32_t) - 2] = kPattern[2];
  reinterpret_cast<uint32_t*>(full_range_ptr)[actual_size / sizeof(uint32_t) - 1] = kPattern[3];

  EXPECT_TRUE(magma::UnmapCpuHelper(full_range_ptr, actual_size));

  // virtio-gpu doesn't support partial mappings
  if (!connection.is_virtmagma()) {
    void* first_page_ptr;
    EXPECT_TRUE(magma::MapCpuHelper(buffer, 0 /*offset*/, page_size(), &first_page_ptr));

    void* last_page_ptr;
    EXPECT_TRUE(magma::MapCpuHelper(buffer, (kBufferSizeInPages - 1) * page_size() /*offset*/,
                                    page_size(), &last_page_ptr));

    // Check that written values match.
    EXPECT_EQ(reinterpret_cast<uint32_t*>(first_page_ptr)[0], kPattern[0]);
    EXPECT_EQ(reinterpret_cast<uint32_t*>(first_page_ptr)[1], kPattern[1]);

    EXPECT_EQ(reinterpret_cast<uint32_t*>(last_page_ptr)[page_size() / sizeof(uint32_t) - 2],
              kPattern[2]);
    EXPECT_EQ(reinterpret_cast<uint32_t*>(last_page_ptr)[page_size() / sizeof(uint32_t) - 1],
              kPattern[3]);

    EXPECT_TRUE(magma::UnmapCpuHelper(last_page_ptr, page_size()));
    EXPECT_TRUE(magma::UnmapCpuHelper(first_page_ptr, page_size()));
  }

  magma_connection_release_buffer(connection.connection(), buffer);
}

TEST_F(Magma, MaxBufferHandle2) {
  TestConnection connection;

  magma_buffer_t buffer;
  uint64_t actual_size;
  constexpr uint64_t kBufferSizeInPages = 1;
  uint64_t buffer_id;
  ASSERT_EQ(MAGMA_STATUS_OK, magma_connection_create_buffer(connection.connection(),
                                                            kBufferSizeInPages * page_size(),
                                                            &actual_size, &buffer, &buffer_id));

  std::unordered_set<magma_handle_t> handles;

  // This may fail on Linux if the open file limit is too small.

  constexpr size_t kMaxBufferHandles = 10000;
#if defined(__linux__)
  struct rlimit rlimit{};
  rlimit.rlim_cur = kMaxBufferHandles * 2;
  rlimit.rlim_max = rlimit.rlim_cur;
  EXPECT_EQ(0, setrlimit(RLIMIT_NOFILE, &rlimit));
#endif

  for (size_t i = 0; i < kMaxBufferHandles; i++) {
    magma_handle_t handle;

    magma_status_t status = magma_buffer_get_handle(buffer, &handle);
    if (status != MAGMA_STATUS_OK) {
      EXPECT_EQ(status, MAGMA_STATUS_OK) << "magma_get_buffer_handle2 failed count: " << i;
      break;
    }
    handles.insert(handle);
  }

  EXPECT_EQ(handles.size(), kMaxBufferHandles);

  for (auto& handle : handles) {
#if defined(__Fuchsia__)
    zx_handle_close(handle);
#elif defined(__linux__)
    close(handle);
#endif
  }

  magma_connection_release_buffer(connection.connection(), buffer);
}

TEST_F(Magma, MaxBufferMappings) {
  TestConnection connection;

  magma_buffer_t buffer;
  uint64_t actual_size;
  constexpr uint64_t kBufferSizeInPages = 1;
  uint64_t buffer_id;
  ASSERT_EQ(MAGMA_STATUS_OK, magma_connection_create_buffer(connection.connection(),
                                                            kBufferSizeInPages * page_size(),
                                                            &actual_size, &buffer, &buffer_id));

  std::unordered_set<void*> maps;

  // The helper closes the buffer handle, so the Linux open file limit shouldn't matter.
  constexpr size_t kMaxBufferMaps = 10000;

  for (size_t i = 0; i < kMaxBufferMaps; i++) {
    void* ptr;
    if (!magma::MapCpuHelper(buffer, 0 /*offset*/, actual_size, &ptr)) {
      EXPECT_TRUE(false) << "MapCpuHelper failed count: " << i;
      break;
    }
    maps.insert(ptr);
  }

  EXPECT_EQ(maps.size(), kMaxBufferMaps);

  for (void* ptr : maps) {
    EXPECT_TRUE(magma::UnmapCpuHelper(ptr, actual_size));
  }

  magma_connection_release_buffer(connection.connection(), buffer);
}

TEST_F(Magma, Flush) {
  TestConnection connection;
  EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_flush(connection.connection()));
}

TEST_F(Magma, BufferCached) { TestConnection().BufferCaching(MAGMA_CACHE_POLICY_CACHED); }

TEST_F(Magma, BufferUncached) { TestConnection().BufferCaching(MAGMA_CACHE_POLICY_UNCACHED); }

TEST_F(Magma, BufferWriteCombining) {
  TestConnection().BufferCaching(MAGMA_CACHE_POLICY_WRITE_COMBINING);
}

TEST_F(Magma, BufferNaming) { TestConnection().BufferNaming(); }

class MagmaEnumerate : public Magma {
 public:
#if defined(__Fuchsia__)
  MagmaEnumerate() : loop_(&kAsyncLoopConfigNeverAttachToThread) {}

 protected:
  void SetUp() override {
    Magma::SetUp();

    ASSERT_EQ(ZX_OK, loop_.StartThread("server-loop"));

    endpoints_ = fidl::Endpoints<fuchsia_io::Directory>::Create();

    ASSERT_EQ(ZX_OK, fdio_open3("/pkg/data/devices-for-enumeration-test",
                                uint64_t{fuchsia_io::wire::kPermReadable |
                                         fuchsia_io::Flags::kProtocolDirectory},
                                endpoints_.server.TakeChannel().release()));
  }

  async::Loop loop_;
  fidl::Endpoints<fuchsia_io::Directory> endpoints_;
#endif
};

TEST_F(MagmaEnumerate, Ok) {
  uint32_t device_path_count = 4;
  uint32_t device_path_size = PATH_MAX;

  auto device_paths = std::vector<char>(device_path_count * device_path_size);

#if defined(__Fuchsia__)
  EXPECT_EQ(MAGMA_STATUS_OK, magma_enumerate_devices(
                                 MAGMA_DEVICE_NAMESPACE, endpoints_.client.TakeChannel().release(),
                                 &device_path_count, device_path_size, device_paths.data()));
  EXPECT_EQ(device_path_count, 2u);
  {
    std::string expected = std::string(MAGMA_DEVICE_NAMESPACE) +
                           "abcd1234";  // MAGMA_DEVICE_NAMESPACE is slash terminated
    EXPECT_STREQ(device_paths.data(), expected.c_str());
  }
  {
    std::string expected =
        std::string(MAGMA_DEVICE_NAMESPACE) +
        "slightly-longer-entry-name";  // MAGMA_DEVICE_NAMESPACE is slash terminated
    EXPECT_STREQ(device_paths.data() + device_path_size, expected.c_str());
  }
#else
  EXPECT_EQ(MAGMA_STATUS_OK, magma_enumerate_devices(MAGMA_DEVICE_NAMESPACE, 0, &device_path_count,
                                                     device_path_size, device_paths.data()));
  EXPECT_EQ(device_path_count, 1u);

  std::string expected = "/dev/magma0";
  EXPECT_STREQ(device_paths.data(), expected.c_str());
#endif
}

#if defined(__Fuchsia__)
TEST_F(MagmaEnumerate, BadParam1) {
  uint32_t device_path_count = 1;
  uint32_t device_path_size = PATH_MAX;

  auto device_paths = std::vector<char>(device_path_count * device_path_size);

  EXPECT_EQ(
      MAGMA_STATUS_MEMORY_ERROR,
      magma_enumerate_devices(MAGMA_DEVICE_NAMESPACE, endpoints_.client.TakeChannel().release(),
                              &device_path_count, device_path_size, device_paths.data()));
}
#endif

#if defined(__Fuchsia__)
TEST_F(MagmaEnumerate, BadParam2) {
  uint32_t device_path_count = 4;
  uint32_t device_path_size = 10;

  auto device_paths = std::vector<char>(device_path_count * device_path_size);

  EXPECT_EQ(
      MAGMA_STATUS_INVALID_ARGS,
      magma_enumerate_devices(MAGMA_DEVICE_NAMESPACE, endpoints_.client.TakeChannel().release(),
                              &device_path_count, device_path_size, device_paths.data()));
}
#endif
