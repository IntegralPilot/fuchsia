// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.gpu.magma.test/cpp/wire.h>
#include <fidl/fuchsia.gpu.magma/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <unistd.h>

#include <cstdint>
#include <filesystem>
#include <optional>

#include <gtest/gtest.h>
#include <src/lib/fsl/handles/object_info.h>
#include <src/lib/testing/loop_fixture/real_loop_fixture.h>

#include "fidl/fuchsia.gpu.magma/cpp/wire_types.h"
#include "test_magma.h"

namespace {

inline uint64_t page_size() { return sysconf(_SC_PAGESIZE); }

}  // namespace

// Magma clients are expected to use the libmagma client library, but the FIDL interface
// should be fully specified.  These tests ensure that.
//

using DeviceClient = fidl::WireSyncClient<fuchsia_gpu_magma::CombinedDevice>;
using PrimaryClient = fidl::WireClient<fuchsia_gpu_magma::Primary>;

class TestAsyncHandler : public fidl::WireAsyncEventHandler<fuchsia_gpu_magma::Primary> {
 public:
  void on_fidl_error(::fidl::UnbindInfo info) override { unbind_info_ = info; }

  auto& unbind_info() { return unbind_info_; }

  void OnNotifyMessagesConsumed(
      ::fidl::WireEvent<::fuchsia_gpu_magma::Primary::OnNotifyMessagesConsumed>* event) override {
    messages_consumed_ += event->count;
  }

  void OnNotifyMemoryImported(
      ::fidl::WireEvent<::fuchsia_gpu_magma::Primary::OnNotifyMemoryImported>* event) override {
    // noop
  }

  uint64_t get_messages_consumed_and_reset() {
    uint64_t count = messages_consumed_;
    messages_consumed_ = 0;
    return count;
  }

 private:
  std::optional<fidl::UnbindInfo> unbind_info_;
  uint64_t messages_consumed_ = 0;
};

class TestMagmaFidl : public gtest::RealLoopFixture {
 public:
  static constexpr const char* kDevicePathFuchsiaUntrusted = "/svc/fuchsia.gpu.magma.Service";
  static constexpr const char* kDevicePathFuchsiaTrusted = "/svc/fuchsia.gpu.magma.TrustedService";

  static void OpenFuchsiaDevice(const char* device_path, DeviceClient& device) {
    for (auto& p : std::filesystem::directory_iterator(device_path)) {
      ASSERT_FALSE(device.is_valid()) << " More than one GPU device found, specify --vendor-id";

      auto endpoints = fidl::Endpoints<fuchsia_gpu_magma::CombinedDevice>::Create();

      zx_status_t zx_status =
          fdio_service_connect((static_cast<std::string>(p.path()) + "/device").c_str(),
                               endpoints.server.TakeChannel().release());
      ASSERT_EQ(ZX_OK, zx_status);

      device = DeviceClient(std::move(endpoints.client));

      uint64_t vendor_id = 0;
      {
        auto wire_result = device->Query(fuchsia_gpu_magma::wire::QueryId::kVendorId);
        ASSERT_TRUE(wire_result.ok());

        ASSERT_TRUE(wire_result->value()->is_simple_result());
        vendor_id = wire_result->value()->simple_result();
      }

      if (gVendorId == 0 || vendor_id == gVendorId) {
        break;
      } else {
        device = {};
      }
    }
  }

  void SetUp() override {
    auto client_end = component::Connect<fuchsia_gpu_magma_test::VendorHelper>();
    EXPECT_TRUE(client_end.is_ok()) << " status " << client_end.status_value();

    vendor_helper_ = fidl::WireSyncClient(std::move(*client_end));

    OpenFuchsiaDevice(kDevicePathFuchsiaUntrusted, device_);
    if (!device_.is_valid()) {
      OpenFuchsiaDevice(kDevicePathFuchsiaTrusted, device_);
    }

    ASSERT_TRUE(device_.is_valid());

    {
      auto wire_result = device_->Query(fuchsia_gpu_magma::wire::QueryId::kVendorVersion);
      ASSERT_TRUE(wire_result.ok());

      ASSERT_TRUE(wire_result->value()->is_simple_result());
      EXPECT_NE(0u, wire_result->value()->simple_result());
    }

    {
      auto wire_result = device_->Query(fuchsia_gpu_magma::wire::QueryId::kMaximumInflightParams);
      ASSERT_TRUE(wire_result.ok());

      ASSERT_TRUE(wire_result->value()->is_simple_result());
      uint64_t params = wire_result->value()->simple_result();
      max_inflight_messages_ = static_cast<uint32_t>(params >> 32);
    }

    auto primary_endpoints = fidl::Endpoints<fuchsia_gpu_magma::Primary>::Create();

    auto notification_endpoints = fidl::CreateEndpoints<fuchsia_gpu_magma::Notification>();
    ASSERT_TRUE(notification_endpoints.is_ok());

    uint64_t client_id = 0xabcd;  // anything
    auto wire_result = device_->Connect2(client_id, std::move(primary_endpoints.server),
                                         std::move(notification_endpoints->server));
    ASSERT_TRUE(wire_result.ok());

    primary_ = PrimaryClient(std::move(primary_endpoints.client), dispatcher(), &async_handler_);
    ASSERT_TRUE(primary_.is_valid());

    notification_channel_ = std::move(notification_endpoints->client.channel());
  }

  void TearDown() override {}

  bool vendor_has_unmap() {
    auto result = vendor_helper_->GetConfig();
    EXPECT_TRUE(result.ok());

    if (!result.Unwrap()->has_buffer_unmap_type()) {
      return false;
    }
    return result.Unwrap()->buffer_unmap_type() ==
           ::fuchsia_gpu_magma_test::wire::BufferUnmapType::kSupported;
  }

  bool vendor_has_perform_buffer_op() {
    auto result = vendor_helper_->GetConfig();
    EXPECT_TRUE(result.ok());

    if (!result.Unwrap()->has_connection_perform_buffer_op_type()) {
      return false;
    }
    return result.Unwrap()->connection_perform_buffer_op_type() ==
           ::fuchsia_gpu_magma_test::wire::ConnectionPerformBufferOpType::kSupported;
  }

  fuchsia_gpu_magma::wire::MapFlags vendor_set_buffer_map_flags(
      fuchsia_gpu_magma::wire::MapFlags flags) {
    auto result = vendor_helper_->GetConfig();
    EXPECT_TRUE(result.ok());

    if (result.Unwrap()->has_buffer_map_features() &&
        result.Unwrap()->buffer_map_features() &
            ::fuchsia_gpu_magma_test::wire::BufferMapFeatures::kSupportsGrowable) {
      return flags | fuchsia_gpu_magma::wire::MapFlags::kGrowable;
    }
    return flags;
  }

  bool CheckForUnbind() {
    // TODO(https://fxbug.dev/42180237) Consider handling the error instead of ignoring it.
    (void)primary_.sync()->Flush();
    RunLoopUntilIdle();
    return async_handler_.unbind_info().has_value();
  }

  DeviceClient device_;
  uint32_t max_inflight_messages_ = 0;
  TestAsyncHandler async_handler_;
  PrimaryClient primary_;
  zx::channel notification_channel_;
  fidl::WireSyncClient<fuchsia_gpu_magma_test::VendorHelper> vendor_helper_;
};

TEST_F(TestMagmaFidl, Connect) {
  // Just setup and teardown
}

TEST_F(TestMagmaFidl, Query) {
  {
    auto wire_response = device_->Query(fuchsia_gpu_magma::wire::QueryId::kVendorId);
    EXPECT_TRUE(wire_response.ok());
    EXPECT_TRUE(wire_response.value().value()->is_simple_result());
    EXPECT_FALSE(wire_response.value().value()->is_buffer_result());
  }
  {
    auto wire_response = device_->Query(fuchsia_gpu_magma::wire::QueryId::kDeviceId);
    EXPECT_TRUE(wire_response.ok());
    EXPECT_TRUE(wire_response.value().value()->is_simple_result());
    EXPECT_FALSE(wire_response.value().value()->is_buffer_result());
  }
  {
    auto wire_response = device_->Query(fuchsia_gpu_magma::wire::QueryId::kIsTotalTimeSupported);
    EXPECT_TRUE(wire_response.ok());
    EXPECT_TRUE(wire_response.value().value()->is_simple_result());
    EXPECT_FALSE(wire_response.value().value()->is_buffer_result());
  }
  {
    auto wire_response = device_->Query(fuchsia_gpu_magma::wire::QueryId::kMaximumInflightParams);
    EXPECT_TRUE(wire_response.ok());
    EXPECT_TRUE(wire_response.value().value()->is_simple_result());
    EXPECT_FALSE(wire_response.value().value()->is_buffer_result());
  }
}

TEST_F(TestMagmaFidl, DumpState) {
  // TODO: define dumpstate param in magma.fidl. Or for testing only (use inspect instead)?
  auto wire_result = device_->DumpState(0);
  EXPECT_TRUE(wire_result.ok());
}

TEST_F(TestMagmaFidl, GetIcdList) {
  auto wire_result = device_->GetIcdList();
  EXPECT_TRUE(wire_result.ok());
}

TEST_F(TestMagmaFidl, ImportObjectInvalidType) {
  zx::vmo vmo;
  ASSERT_EQ(ZX_OK, zx::vmo::create(4 /*size*/, 0 /*options*/, &vmo));
  constexpr auto kInvalidObjectType = fuchsia_gpu_magma::ObjectType(1000);
  fidl::Arena allocator;
  auto builder = fuchsia_gpu_magma::wire::PrimaryImportObjectRequest::Builder(allocator);
  builder.object(fuchsia_gpu_magma::wire::Object::WithBuffer(std::move(vmo)))
      .object_id(/*id=*/1)
      .object_type(kInvalidObjectType);
  auto wire_result = primary_->ImportObject(builder.Build());
  EXPECT_TRUE(wire_result.ok());
  EXPECT_TRUE(CheckForUnbind());
}

TEST_F(TestMagmaFidl, ImportReleaseBuffer) {
  uint64_t buffer_id;

  {
    zx::vmo vmo;
    ASSERT_EQ(ZX_OK, zx::vmo::create(4 /*size*/, 0 /*options*/, &vmo));
    buffer_id = fsl::GetKoid(vmo.get());

    fidl::Arena allocator;
    auto builder = fuchsia_gpu_magma::wire::PrimaryImportObjectRequest::Builder(allocator);
    builder.object(fuchsia_gpu_magma::wire::Object::WithBuffer(std::move(vmo)))
        .object_id(buffer_id)
        .object_type(fuchsia_gpu_magma::wire::ObjectType::kBuffer);

    auto wire_result = primary_->ImportObject(builder.Build());
    EXPECT_TRUE(wire_result.ok());
    EXPECT_FALSE(CheckForUnbind());
  }

  {
    auto wire_result =
        primary_->ReleaseObject(buffer_id, fuchsia_gpu_magma::wire::ObjectType::kBuffer);
    EXPECT_TRUE(wire_result.ok());
    EXPECT_FALSE(CheckForUnbind());
  }

  {
    uint64_t kBadId = buffer_id + 1;
    auto wire_result =
        primary_->ReleaseObject(kBadId, fuchsia_gpu_magma::wire::ObjectType::kBuffer);
    EXPECT_TRUE(wire_result.ok());
    EXPECT_TRUE(CheckForUnbind());
  }
}

TEST_F(TestMagmaFidl, ImportReleaseSemaphoreDeprecated) {
  uint64_t event_id;

  {
    zx::event event;
    ASSERT_EQ(ZX_OK, zx::event::create(/*options=*/0, &event));
    event_id = fsl::GetKoid(event.get());

    fidl::Arena allocator;
    auto builder = fuchsia_gpu_magma::wire::PrimaryImportObjectRequest::Builder(allocator);
    builder.object(fuchsia_gpu_magma::wire::Object::WithSemaphore(std::move(event)))
        .object_id(event_id)
        .object_type(fuchsia_gpu_magma::wire::ObjectType::kSemaphore);

    auto wire_result = primary_->ImportObject(builder.Build());
    EXPECT_TRUE(wire_result.ok());
    EXPECT_FALSE(CheckForUnbind());
  }

  {
    auto wire_result =
        primary_->ReleaseObject(event_id, fuchsia_gpu_magma::wire::ObjectType::kSemaphore);
    EXPECT_TRUE(wire_result.ok());
    EXPECT_FALSE(CheckForUnbind());
  }

  {
    uint64_t kBadId = event_id + 1;
    auto wire_result =
        primary_->ReleaseObject(kBadId, fuchsia_gpu_magma::wire::ObjectType::kSemaphore);
    EXPECT_TRUE(wire_result.ok());
    EXPECT_TRUE(CheckForUnbind());
  }
}

TEST_F(TestMagmaFidl, ImportReleaseSemaphore) {
  uint64_t event_id;

  {
    zx::event event;
    ASSERT_EQ(ZX_OK, zx::event::create(0 /*options*/, &event));
    event_id = fsl::GetKoid(event.get());

    fidl::Arena allocator;
    auto builder = fuchsia_gpu_magma::wire::PrimaryImportObjectRequest::Builder(allocator);
    builder.object(fuchsia_gpu_magma::wire::Object::WithSemaphore(std::move(event)))
        .object_id(event_id)
        .object_type(fuchsia_gpu_magma::wire::ObjectType::kSemaphore);

    auto wire_result = primary_->ImportObject(builder.Build());
    EXPECT_TRUE(wire_result.ok());
    EXPECT_FALSE(CheckForUnbind());
  }

  {
    auto wire_result =
        primary_->ReleaseObject(event_id, fuchsia_gpu_magma::wire::ObjectType::kSemaphore);
    EXPECT_TRUE(wire_result.ok());
    EXPECT_FALSE(CheckForUnbind());
  }

  {
    uint64_t kBadId = event_id + 1;
    auto wire_result =
        primary_->ReleaseObject(kBadId, fuchsia_gpu_magma::wire::ObjectType::kSemaphore);
    EXPECT_TRUE(wire_result.ok());
    EXPECT_TRUE(CheckForUnbind());
  }
}

TEST_F(TestMagmaFidl, ImportReleaseCounterSemaphore) {
  uint64_t event_id;

  {
    zx::counter counter;
    ASSERT_EQ(ZX_OK, zx::counter::create(0 /*options*/, &counter));
    event_id = fsl::GetKoid(counter.get());

    fidl::Arena allocator;
    auto builder = fuchsia_gpu_magma::wire::PrimaryImportObjectRequest::Builder(allocator);
    builder.object(fuchsia_gpu_magma::wire::Object::WithCounterSemaphore(std::move(counter)))
        .object_id(event_id)
        .object_type(fuchsia_gpu_magma::wire::ObjectType::kSemaphore)
        .flags(fuchsia_gpu_magma::ImportFlags::kSemaphoreOneShot);

    auto wire_result = primary_->ImportObject(builder.Build());
    EXPECT_TRUE(wire_result.ok());
    EXPECT_FALSE(CheckForUnbind());
  }

  {
    auto wire_result =
        primary_->ReleaseObject(event_id, fuchsia_gpu_magma::wire::ObjectType::kSemaphore);
    EXPECT_TRUE(wire_result.ok());
    EXPECT_FALSE(CheckForUnbind());
  }

  {
    uint64_t kBadId = event_id + 1;
    auto wire_result =
        primary_->ReleaseObject(kBadId, fuchsia_gpu_magma::wire::ObjectType::kSemaphore);
    EXPECT_TRUE(wire_result.ok());
    EXPECT_TRUE(CheckForUnbind());
  }
}

TEST_F(TestMagmaFidl, CreateDestroyContext) {
  uint32_t context_id = 10;

  {
    auto wire_result = primary_->CreateContext(context_id);
    EXPECT_TRUE(wire_result.ok());
    EXPECT_FALSE(CheckForUnbind());
  }

  {
    auto wire_result = primary_->DestroyContext(context_id);
    EXPECT_TRUE(wire_result.ok());
    EXPECT_FALSE(CheckForUnbind());
  }

  {
    uint32_t kBadId = context_id + 1;
    auto wire_result = primary_->DestroyContext(kBadId);
    EXPECT_TRUE(wire_result.ok());
    EXPECT_TRUE(CheckForUnbind());
  }
}

TEST_F(TestMagmaFidl, MapUnmap) {
  fuchsia_gpu_magma::wire::BufferRange range;

  {
    zx::vmo vmo;
    ASSERT_EQ(ZX_OK, zx::vmo::create(4 /*size*/, 0 /*options*/, &vmo));

    uint64_t length;
    ASSERT_EQ(ZX_OK, vmo.get_size(&length));

    range = {.buffer_id = fsl::GetKoid(vmo.get()), .offset = 0, .size = length};

    fidl::Arena allocator;
    auto builder = fuchsia_gpu_magma::wire::PrimaryImportObjectRequest::Builder(allocator);
    builder.object(fuchsia_gpu_magma::wire::Object::WithBuffer(std::move(vmo)))
        .object_id(range.buffer_id)
        .object_type(fuchsia_gpu_magma::wire::ObjectType::kBuffer);

    auto wire_result = primary_->ImportObject(builder.Build());
    EXPECT_TRUE(wire_result.ok());
    EXPECT_FALSE(CheckForUnbind());
  }

  constexpr uint64_t kGpuAddress = 0x1000;

  {
    fuchsia_gpu_magma::wire::MapFlags flags = vendor_set_buffer_map_flags(
        fuchsia_gpu_magma::wire::MapFlags::kRead | fuchsia_gpu_magma::wire::MapFlags::kWrite |
        fuchsia_gpu_magma::wire::MapFlags::kExecute);

    fidl::Arena allocator;
    auto builder = fuchsia_gpu_magma::wire::PrimaryMapBufferRequest::Builder(allocator);
    builder.hw_va(kGpuAddress).range(range).flags(flags);

    auto wire_result = primary_->MapBuffer(builder.Build());
    EXPECT_TRUE(wire_result.ok());
    EXPECT_FALSE(CheckForUnbind());
  }

  {
    fidl::Arena allocator;
    auto builder = fuchsia_gpu_magma::wire::PrimaryUnmapBufferRequest::Builder(allocator);
    builder.hw_va(kGpuAddress).buffer_id(range.buffer_id);

    auto wire_result = primary_->UnmapBuffer(builder.Build());
    EXPECT_TRUE(wire_result.ok());

    if (!vendor_has_unmap()) {
      EXPECT_TRUE(CheckForUnbind());
    } else {
      EXPECT_FALSE(CheckForUnbind());
    }
  }
}
// Sends a bunch of zero command bytes
TEST_F(TestMagmaFidl, ExecuteCommand) {
  uint32_t context_id = 10;

  {
    auto wire_result = primary_->CreateContext(context_id);
    EXPECT_TRUE(wire_result.ok());
    EXPECT_FALSE(CheckForUnbind());
  }

  uint64_t buffer_id;

  {
    zx::vmo vmo;
    ASSERT_EQ(ZX_OK, zx::vmo::create(4096, 0 /*options*/, &vmo));
    buffer_id = fsl::GetKoid(vmo.get());
    fidl::Arena allocator;
    auto builder = fuchsia_gpu_magma::wire::PrimaryImportObjectRequest::Builder(allocator);
    builder.object(fuchsia_gpu_magma::wire::Object::WithBuffer(std::move(vmo)))
        .object_id(buffer_id)
        .object_type(fuchsia_gpu_magma::wire::ObjectType::kBuffer);
    auto wire_result = primary_->ImportObject(builder.Build());
    EXPECT_TRUE(wire_result.ok());
    EXPECT_FALSE(CheckForUnbind());
  }

  {
    fuchsia_gpu_magma::wire::BufferRange resource = {
        .buffer_id = buffer_id, .offset = 0, .size = 0};
    std::vector<fuchsia_gpu_magma::wire::BufferRange> resources{std::move(resource)};
    std::vector<fuchsia_gpu_magma::wire::CommandBuffer> command_buffers{{
        .resource_index = 0,
        .start_offset = 0,
    }};
    std::vector<uint64_t> wait_semaphores;
    std::vector<uint64_t> signal_semaphores;
    auto wire_result = primary_->ExecuteCommand(
        context_id, fidl::VectorView<fuchsia_gpu_magma::wire::BufferRange>::FromExternal(resources),
        fidl::VectorView<fuchsia_gpu_magma::wire::CommandBuffer>::FromExternal(command_buffers),
        fidl::VectorView<uint64_t>::FromExternal(wait_semaphores),
        fidl::VectorView<uint64_t>::FromExternal(signal_semaphores),
        fuchsia_gpu_magma::wire::CommandBufferFlags(0));
    EXPECT_TRUE(wire_result.ok());

    // Fails checking (resource not mapped), does not execute on GPU
    EXPECT_TRUE(CheckForUnbind());
  }
}

// Sends a bunch of zero command bytes
TEST_F(TestMagmaFidl, ExecuteInlineCommands) {
  uint32_t context_id = 10;

  {
    auto wire_result = primary_->CreateContext(context_id);
    EXPECT_TRUE(wire_result.ok());
    EXPECT_FALSE(CheckForUnbind());
  }

  {
    std::array<fuchsia_gpu_magma::wire::InlineCommand, 1> inline_commands;
    auto wire_result = primary_->ExecuteInlineCommands(
        context_id,
        fidl::VectorView<fuchsia_gpu_magma::wire::InlineCommand>::FromExternal(inline_commands));
    EXPECT_TRUE(wire_result.ok());

    // Fails checking, does not execute on GPU
    EXPECT_TRUE(CheckForUnbind());
  }
}

TEST_F(TestMagmaFidl, BufferRangeOp2) {
  if (!vendor_has_perform_buffer_op()) {
    GTEST_SKIP();
  }

  constexpr uint64_t kPageCount = 10;
  uint64_t size = kPageCount * page_size();
  uint64_t buffer_id;
  zx::vmo vmo;
  fuchsia_gpu_magma::wire::BufferRange range;

  {
    ASSERT_EQ(ZX_OK, zx::vmo::create(size, 0 /*options*/, &vmo));
    buffer_id = fsl::GetKoid(vmo.get());

    zx::vmo vmo_dupe;
    ASSERT_EQ(ZX_OK, vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_dupe));

    fidl::Arena allocator;
    auto builder = fuchsia_gpu_magma::wire::PrimaryImportObjectRequest::Builder(allocator);
    builder.object(fuchsia_gpu_magma::wire::Object::WithBuffer(std::move(vmo_dupe)))
        .object_id(buffer_id)
        .object_type(fuchsia_gpu_magma::wire::ObjectType::kBuffer);

    auto wire_result = primary_->ImportObject(builder.Build());
    EXPECT_TRUE(wire_result.ok());
    EXPECT_FALSE(CheckForUnbind());

    range = {.buffer_id = fsl::GetKoid(vmo.get()), .offset = 0, .size = size};
  }

  {
    zx_info_vmo_t info;
    ASSERT_EQ(ZX_OK, vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
    EXPECT_EQ(0u, info.committed_bytes);
  }

  {
    fidl::Arena allocator;
    auto builder = fuchsia_gpu_magma::wire::PrimaryMapBufferRequest::Builder(allocator);
    builder.hw_va(0x1000).range(range).flags(fuchsia_gpu_magma::wire::MapFlags::kRead);

    auto wire_result = primary_->MapBuffer(builder.Build());
    EXPECT_TRUE(wire_result.ok());
    EXPECT_FALSE(CheckForUnbind());
  }

  {
    auto wire_result =
        primary_->BufferRangeOp2(fuchsia_gpu_magma::wire::BufferOp::kPopulateTables, range);
    EXPECT_TRUE(wire_result.ok());
    EXPECT_FALSE(CheckForUnbind());
  }

  // Should be sync'd after the unbind check
  {
    zx_info_vmo_t info;
    ASSERT_EQ(ZX_OK, vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
    EXPECT_EQ(size, info.committed_bytes);
  }

  {
    auto wire_result =
        primary_->BufferRangeOp2(fuchsia_gpu_magma::wire::BufferOp::kDepopulateTables, range);
    EXPECT_TRUE(wire_result.ok());
    EXPECT_FALSE(CheckForUnbind());
  }

  // Depopulate doesn't decommit
  {
    zx_info_vmo_t info;
    ASSERT_EQ(ZX_OK, vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
    EXPECT_EQ(size, info.committed_bytes);
  }

  // Check invalid range op
  {
    constexpr auto kInvalidBufferRangeOp = fuchsia_gpu_magma::BufferOp(1000);
    auto wire_result = primary_->BufferRangeOp2(kInvalidBufferRangeOp, range);
    EXPECT_TRUE(wire_result.ok());
    EXPECT_TRUE(CheckForUnbind());
  }
}

TEST_F(TestMagmaFidl, FlowControl) {
  // Without flow control, this will trigger a policy exception (too many channel messages)
  // or an OOM.
  auto result = primary_->EnableFlowControl();
  ZX_ASSERT(result.ok());

  constexpr uint32_t kIterations = 10000 / 2;

  int64_t messages_inflight = 0;

  for (uint32_t i = 0; i < kIterations; i++) {
    uint64_t buffer_id;
    {
      zx::vmo vmo;
      ASSERT_EQ(ZX_OK, zx::vmo::create(4 /*size*/, 0 /*options*/, &vmo));
      buffer_id = fsl::GetKoid(vmo.get());

      fidl::Arena allocator;
      auto builder = fuchsia_gpu_magma::wire::PrimaryImportObjectRequest::Builder(allocator);
      builder.object(fuchsia_gpu_magma::wire::Object::WithBuffer(std::move(vmo)))
          .object_id(buffer_id)
          .object_type(fuchsia_gpu_magma::wire::ObjectType::kBuffer);

      auto wire_result = primary_->ImportObject(builder.Build());
      EXPECT_TRUE(wire_result.ok());
    }

    {
      auto wire_result =
          primary_->ReleaseObject(buffer_id, fuchsia_gpu_magma::wire::ObjectType::kBuffer);
      EXPECT_TRUE(wire_result.ok());
    }

    messages_inflight += 2;

    if (messages_inflight < max_inflight_messages_)
      continue;

    RunLoopUntil([&messages_inflight, this]() {
      uint64_t count = async_handler_.get_messages_consumed_and_reset();
      if (count) {
        messages_inflight -= count;
        EXPECT_GE(messages_inflight, 0u);
      }
      return messages_inflight < max_inflight_messages_;
    });
  }
}

TEST_F(TestMagmaFidl, EnablePerformanceCounters) {
  bool success = false;
  for (auto& p :
       std::filesystem::directory_iterator("/svc/fuchsia.gpu.magma.PerformanceCounterService")) {
    fidl::WireSyncClient<fuchsia_gpu_magma::PerformanceCounterAccess> perf_counter_access;

    {
      auto endpoints = fidl::Endpoints<fuchsia_gpu_magma::PerformanceCounterAccess>::Create();
      ASSERT_EQ(ZX_OK,
                fdio_service_connect((static_cast<std::string>(p.path()) + "/access").c_str(),
                                     endpoints.server.TakeChannel().release()));
      perf_counter_access = fidl::WireSyncClient(std::move(endpoints.client));
    }

    zx::event access_token;

    {
      auto wire_result = perf_counter_access->GetPerformanceCountToken();
      ASSERT_TRUE(wire_result.ok());
      access_token = std::move(wire_result->access_token);
    }

    {
      auto wire_result = primary_->EnablePerformanceCounterAccess(std::move(access_token));
      ASSERT_TRUE(wire_result.ok());
    }

    {
      auto wire_result = primary_.sync()->IsPerformanceCounterAccessAllowed();
      ASSERT_TRUE(wire_result.ok());
      // Should be enabled if the gpu-performance-counters device matches the device under test
      if (wire_result->enabled) {
        success = true;
        break;
      }
    }
  }
  EXPECT_TRUE(success);
}
