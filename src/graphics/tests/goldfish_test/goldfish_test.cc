// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.goldfish/cpp/wire.h>
#include <fidl/fuchsia.math/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fdio.h>
#include <lib/zx/channel.h>
#include <lib/zx/time.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <unistd.h>
#include <zircon/rights.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <bind/fuchsia/goldfish/platform/sysmem/heap/cpp/bind.h>
#include <gtest/gtest.h>

#include "src/lib/fsl/handles/object_info.h"

namespace {

const size_t kPageSize = zx_system_get_page_size();

// TODO(https://fxbug.dev/42065067): Stop hardcoding the 000 in this path.
zx::result<fidl::ClientEnd<fuchsia_hardware_goldfish::PipeDevice>> ConnectToPipe() {
  return component::Connect<fuchsia_hardware_goldfish::PipeDevice>("/dev/class/goldfish-pipe/000");
}

// TODO(https://fxbug.dev/42065067): Stop hardcoding the 000 in this path.
zx::result<fidl::ClientEnd<fuchsia_hardware_goldfish::ControlDevice>> ConnectToControl() {
  return component::Connect<fuchsia_hardware_goldfish::ControlDevice>(
      "/dev/class/goldfish-control/000");
}

// TODO(https://fxbug.dev/42065067): Stop hardcoding the 000 in this path.
zx::result<fidl::ClientEnd<fuchsia_hardware_goldfish::AddressSpaceDevice>> ConnectToAddress() {
  return component::Connect<fuchsia_hardware_goldfish::AddressSpaceDevice>(
      "/dev/class/goldfish-address-space/000");
}

fidl::WireSyncClient<fuchsia_sysmem2::Allocator> CreateSysmemAllocator() {
  zx::result client_end = component::Connect<fuchsia_sysmem2::Allocator>();
  EXPECT_EQ(client_end.status_value(), ZX_OK);
  if (!client_end.is_ok()) {
    return {};
  }
  fidl::WireSyncClient allocator(std::move(*client_end));
  // TODO(https://fxbug.dev/42180237) Consider handling the error instead of ignoring it.
  fidl::Arena arena;
  (void)allocator->SetDebugClientInfo(
      ::fuchsia_sysmem2::wire::AllocatorSetDebugClientInfoRequest::Builder(arena)
          .id(fsl::GetCurrentProcessKoid())
          .name(fidl::StringView::FromExternal(fsl::GetCurrentProcessName()))
          .Build());
  return allocator;
}

void SetDefaultCollectionName(fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>& collection) {
  constexpr uint32_t kTestNamePriority = 1000u;
  std::string test_name = ::testing::UnitTest::GetInstance()->current_test_info()->name();
  fidl::Arena arena;
  EXPECT_TRUE(collection
                  ->SetName(fuchsia_sysmem2::wire::NodeSetNameRequest::Builder(arena)
                                .name(fidl::StringView::FromExternal(test_name))
                                .priority(kTestNamePriority)
                                .Build())
                  .ok());
}
}  // namespace

TEST(GoldfishPipeTests, GoldfishPipeTest) {
  zx::result controller = ConnectToPipe();
  ASSERT_EQ(controller.status_value(), ZX_OK);

  auto [pipe_client, pipe_server] = fidl::Endpoints<fuchsia_hardware_goldfish::Pipe>::Create();

  ASSERT_EQ(fidl::WireCall(controller.value())->Connect(std::move(pipe_server)).status(), ZX_OK);
  fidl::WireSyncClient pipe(std::move(pipe_client));

  const size_t kSize = 3 * 4096;
  {
    auto result = pipe->SetBufferSize(kSize);
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_OK);
  }

  zx::vmo vmo;
  {
    auto result = pipe->GetBuffer();
    ASSERT_TRUE(result.ok());
    vmo = std::move(result->vmo);
  }

  // Connect to pingpong service.
  constexpr char kPipeName[] = "pipe:pingpong";
  size_t bytes = strlen(kPipeName) + 1;
  EXPECT_EQ(vmo.write(kPipeName, 0, bytes), ZX_OK);

  {
    auto result = pipe->Write(bytes, 0);
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_OK);
    EXPECT_EQ(result->actual, bytes);
  }

  // Write 1 byte.
  const uint8_t kSentinel = 0xaa;
  EXPECT_EQ(vmo.write(&kSentinel, 0, 1), ZX_OK);
  {
    auto result = pipe->Write(1, 0);
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_OK);
    EXPECT_EQ(result->actual, 1U);
  }

  // Read 1 byte result.
  {
    auto result = pipe->Read(1, 0);
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_OK);
    EXPECT_EQ(result->actual, 1U);
  }

  uint8_t result = 0;
  EXPECT_EQ(vmo.read(&result, 0, 1), ZX_OK);
  // pingpong service should have returned the data received.
  EXPECT_EQ(result, kSentinel);

  // Write 3 * 4096 bytes.
  uint8_t send_buffer[kSize];
  memset(send_buffer, kSentinel, kSize);
  EXPECT_EQ(vmo.write(send_buffer, 0, kSize), ZX_OK);
  {
    auto result = pipe->Write(kSize, 0);
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_OK);
    EXPECT_EQ(result->actual, kSize);
  }

  // Read 3 * 4096 bytes.
  {
    auto result = pipe->Read(kSize, 0);
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_OK);
    EXPECT_EQ(result->actual, kSize);
  }
  uint8_t recv_buffer[kSize];
  EXPECT_EQ(vmo.read(recv_buffer, 0, kSize), ZX_OK);

  // pingpong service should have returned the data received.
  EXPECT_EQ(memcmp(send_buffer, recv_buffer, kSize), 0);

  // Write & Read 4096 bytes.
  const size_t kSmallSize = kSize / 3;
  const size_t kRecvOffset = kSmallSize;
  memset(send_buffer, kSentinel, kSmallSize);
  EXPECT_EQ(vmo.write(send_buffer, 0, kSmallSize), ZX_OK);

  {
    auto result = pipe->DoCall(kSmallSize, 0u, kSmallSize, kRecvOffset);
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_OK);
    EXPECT_EQ(result->actual, 2 * kSmallSize);
  }

  EXPECT_EQ(vmo.read(recv_buffer, kRecvOffset, kSmallSize), ZX_OK);

  // pingpong service should have returned the data received.
  EXPECT_EQ(memcmp(send_buffer, recv_buffer, kSmallSize), 0);
}

TEST(GoldfishControlTests, GoldfishControlTest) {
  zx::result control_connection = ConnectToControl();
  ASSERT_EQ(control_connection.status_value(), ZX_OK);
  fidl::WireSyncClient<fuchsia_hardware_goldfish::ControlDevice> control(
      std::move(control_connection.value()));

  auto allocator = CreateSysmemAllocator();
  ASSERT_TRUE(allocator.is_valid());

  auto token_endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  fidl::Arena arena;
  ASSERT_EQ(allocator
                ->AllocateSharedCollection(
                    fuchsia_sysmem2::wire::AllocatorAllocateSharedCollectionRequest::Builder(arena)
                        .token_request(std::move(token_endpoints.server))
                        .Build())
                .status(),
            ZX_OK);

  auto collection_endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
  ASSERT_EQ(allocator
                ->BindSharedCollection(
                    fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
                        .buffer_collection_request(std::move(collection_endpoints.server))
                        .token(std::move(token_endpoints.client))
                        .Build())
                .status(),
            ZX_OK);

  auto constraints = fuchsia_sysmem2::wire::BufferCollectionConstraints::Builder(arena);
  constraints
      .usage(fuchsia_sysmem2::wire::BufferUsage::Builder(arena)
                 .vulkan(fuchsia_sysmem2::wire::kVulkanImageUsageTransferDst)
                 .Build())
      .min_buffer_count_for_camping(1)
      .buffer_memory_constraints(
          fuchsia_sysmem2::wire::BufferMemoryConstraints::Builder(arena)
              .min_size_bytes(4 * 1024)
              .max_size_bytes(4 * 1024)
              .physically_contiguous_required(false)
              .secure_required(false)
              .ram_domain_supported(false)
              .cpu_domain_supported(false)
              .inaccessible_domain_supported(true)
              .permitted_heaps(std::array{
                  fuchsia_sysmem2::wire::Heap::Builder(arena)
                      .heap_type(bind_fuchsia_goldfish_platform_sysmem_heap::HEAP_TYPE_DEVICE_LOCAL)
                      .id(0)
                      .Build()})
              .Build());

  fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection> collection(
      std::move(collection_endpoints.client));

  SetDefaultCollectionName(collection);
  EXPECT_TRUE(collection
                  ->SetConstraints(
                      fuchsia_sysmem2::wire::BufferCollectionSetConstraintsRequest::Builder(arena)
                          .constraints(constraints.Build())
                          .Build())
                  .ok());

  auto wait_result = collection->WaitForAllBuffersAllocated();
  ASSERT_TRUE(wait_result.ok());
  EXPECT_TRUE(wait_result->is_ok());

  const auto& info = wait_result->value()->buffer_collection_info();
  EXPECT_EQ(info.buffers().size(), 1U);

  zx::vmo vmo = std::move(info.buffers()[0].vmo());
  ASSERT_TRUE(vmo.is_valid());

  EXPECT_TRUE(collection->Release().ok());

  zx::vmo vmo_copy;
  EXPECT_EQ(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_copy), ZX_OK);

  {
    fidl::Arena allocator;
    fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params create_params(allocator);
    create_params.set_width(64)
        .set_height(64)
        .set_format(fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kBgra)
        .set_memory_property(fuchsia_hardware_goldfish::wire::kMemoryPropertyDeviceLocal);

    auto result = control->CreateColorBuffer2(std::move(vmo_copy), std::move(create_params));
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_OK);
  }

  zx::vmo vmo_copy2;
  EXPECT_EQ(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_copy2), ZX_OK);

  {
    auto result = control->GetBufferHandle(std::move(vmo_copy2));
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_OK);
    EXPECT_NE(result->id, 0u);
    EXPECT_EQ(result->type, fuchsia_hardware_goldfish::wire::BufferHandleType::kColorBuffer);
  }

  zx::vmo vmo_copy3;
  EXPECT_EQ(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_copy3), ZX_OK);

  {
    fidl::Arena allocator;
    fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params create_params(allocator);
    create_params.set_width(64)
        .set_height(64)
        .set_format(fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kBgra)
        .set_memory_property(fuchsia_hardware_goldfish::wire::kMemoryPropertyDeviceLocal);

    auto result = control->CreateColorBuffer2(std::move(vmo_copy3), std::move(create_params));
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_ERR_ALREADY_EXISTS);
  }
}

TEST(GoldfishControlTests, GoldfishControlTest_HostVisible) {
  zx::result control_connection = ConnectToControl();
  ASSERT_EQ(control_connection.status_value(), ZX_OK);
  fidl::WireSyncClient<fuchsia_hardware_goldfish::ControlDevice> control(
      std::move(control_connection.value()));

  auto allocator = CreateSysmemAllocator();
  ASSERT_TRUE(allocator.is_valid());

  auto token_endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  fidl::Arena arena;
  ASSERT_EQ(allocator
                ->AllocateSharedCollection(
                    fuchsia_sysmem2::wire::AllocatorAllocateSharedCollectionRequest::Builder(arena)
                        .token_request(std::move(token_endpoints.server))
                        .Build())
                .status(),
            ZX_OK);

  auto collection_endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
  ASSERT_EQ(allocator
                ->BindSharedCollection(
                    fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
                        .token(std::move(token_endpoints.client))
                        .buffer_collection_request(std::move(collection_endpoints.server))
                        .Build())

                .status(),
            ZX_OK);

  const size_t kMinSizeBytes = 4 * 1024;
  const size_t kMaxSizeBytes = 4 * 4096;
  auto constraints = fuchsia_sysmem2::wire::BufferCollectionConstraints::Builder(arena);
  constraints.usage(fuchsia_sysmem2::wire::BufferUsage::Builder(arena)
                        .vulkan(fuchsia_sysmem2::wire::kVulkanImageUsageTransferDst)
                        .Build());
  constraints.min_buffer_count_for_camping(1)
      .buffer_memory_constraints(
          fuchsia_sysmem2::wire::BufferMemoryConstraints::Builder(arena)
              .min_size_bytes(kMinSizeBytes)
              .max_size_bytes(kMaxSizeBytes)
              .cpu_domain_supported(true)
              .permitted_heaps(std::array{
                  fuchsia_sysmem2::wire::Heap::Builder(arena)
                      .heap_type(bind_fuchsia_goldfish_platform_sysmem_heap::HEAP_TYPE_HOST_VISIBLE)
                      .id(0)
                      .Build()})
              .Build())
      .image_format_constraints(
          std::array{fuchsia_sysmem2::wire::ImageFormatConstraints::Builder(arena)
                         .pixel_format(fuchsia_images2::wire::PixelFormat::kB8G8R8A8)
                         .color_spaces(std::array{fuchsia_images2::wire::ColorSpace::kSrgb})
                         .min_size(fuchsia_math::wire::SizeU{.width = 32, .height = 32})
                         .Build()});

  fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection> collection(
      std::move(collection_endpoints.client));
  SetDefaultCollectionName(collection);
  EXPECT_TRUE(collection
                  ->SetConstraints(
                      fuchsia_sysmem2::wire::BufferCollectionSetConstraintsRequest::Builder(arena)
                          .constraints(constraints.Build())
                          .Build())
                  .ok());

  auto wait_result = collection->WaitForAllBuffersAllocated();
  ASSERT_TRUE(wait_result.ok());
  EXPECT_TRUE(wait_result->is_ok());

  const auto& info = wait_result->value()->buffer_collection_info();
  EXPECT_EQ(info.buffers().size(), 1U);
  EXPECT_EQ(info.settings().buffer_settings().coherency_domain(),
            fuchsia_sysmem2::wire::CoherencyDomain::kCpu);

  zx::vmo vmo = std::move(info.buffers()[0].vmo());
  ASSERT_TRUE(vmo.is_valid());

  uint64_t vmo_size;
  EXPECT_EQ(vmo.get_size(&vmo_size), ZX_OK);
  EXPECT_GE(vmo_size, kMinSizeBytes);
  EXPECT_LE(vmo_size, kMaxSizeBytes);

  // Test if the vmo is mappable.
  zx_vaddr_t addr;
  EXPECT_EQ(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, /*vmar_offset*/ 0, vmo,
                                       /*vmo_offset*/ 0, vmo_size, &addr),
            ZX_OK);

  // Test if write and read works correctly.
  uint8_t* ptr = reinterpret_cast<uint8_t*>(addr);
  std::vector<uint8_t> copy_target(vmo_size, 0u);
  for (uint32_t trial = 0; trial < 10u; trial++) {
    memset(ptr, trial, vmo_size);
    memcpy(copy_target.data(), ptr, vmo_size);
    zx_cache_flush(ptr, vmo_size, ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE);
    EXPECT_EQ(memcmp(copy_target.data(), ptr, vmo_size), 0);
  }

  EXPECT_EQ(zx::vmar::root_self()->unmap(addr, kPageSize), ZX_OK);

  EXPECT_TRUE(collection->Release().ok());
}

TEST(GoldfishControlTests, GoldfishControlTest_HostVisible_MultiClients) {
  using fuchsia_sysmem2::BufferCollection;
  using fuchsia_sysmem2::wire::BufferCollectionConstraints;

  zx::result control = ConnectToControl();
  EXPECT_EQ(control.status_value(), ZX_OK);

  auto allocator = CreateSysmemAllocator();
  ASSERT_TRUE(allocator.is_valid());

  constexpr size_t kNumClients = 2;

  fidl::Arena arena;
  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token_client[kNumClients];
  fidl::ClientEnd<fuchsia_sysmem2::BufferCollection> collection_client[kNumClients];

  // Client 0.
  {
    auto endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
    ASSERT_EQ(
        allocator
            ->AllocateSharedCollection(
                fuchsia_sysmem2::wire::AllocatorAllocateSharedCollectionRequest::Builder(arena)
                    .token_request(std::move(endpoints.server))
                    .Build())
            .status(),
        ZX_OK);
    token_client[0] = std::move(endpoints.client);
  }

  // Client 1.
  {
    auto endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
    ASSERT_EQ(
        fidl::WireCall(token_client[0].borrow())
            ->Duplicate(fuchsia_sysmem2::wire::BufferCollectionTokenDuplicateRequest::Builder(arena)
                            .token_request(std::move(endpoints.server))
                            .rights_attenuation_mask(ZX_RIGHT_SAME_RIGHTS)
                            .Build())
            .status(),
        ZX_OK);
    ASSERT_EQ(fidl::WireCall(token_client[0].borrow())->Sync().status(), ZX_OK);
    token_client[1] = std::move(endpoints.client);
  }

  for (size_t i = 0; i < kNumClients; i++) {
    auto endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();

    ASSERT_EQ(allocator
                  ->BindSharedCollection(
                      fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
                          .token(std::move(token_client[i]))
                          .buffer_collection_request(std::move(endpoints.server))
                          .Build())
                  .status(),
              ZX_OK);
    collection_client[i] = std::move(endpoints.client);
  }

  const size_t kMinSizeBytes = 4 * 1024;
  const size_t kMaxSizeBytes = 4 * 1024 * 512;
  const size_t kTargetSizeBytes = 4 * 1024 * 512;
  std::vector<fuchsia_sysmem2::wire::BufferCollectionConstraints> constraints;
  for (size_t i = 0; i < kNumClients; i++) {
    auto builder = fuchsia_sysmem2::wire::BufferCollectionConstraints::Builder(arena);
    auto image_constraints = fuchsia_sysmem2::wire::ImageFormatConstraints::Builder(arena);
    image_constraints.pixel_format(fuchsia_images2::wire::PixelFormat::kB8G8R8A8)
        .color_spaces(std::array{fuchsia_images2::wire::ColorSpace::kSrgb});

    // Set different min_coded_width and required_max_coded_width for each client.
    if (i == 0) {
      image_constraints.min_size(fuchsia_math::wire::SizeU{.width = 32, .height = 64});
    }
    if (i == 1) {
      image_constraints.min_size(fuchsia_math::wire::SizeU{.width = 16, .height = 512});
      image_constraints.required_max_size(fuchsia_math::wire::SizeU{.width = 1024, .height = 256});
    }
    builder
        .usage(fuchsia_sysmem2::wire::BufferUsage::Builder(arena)
                   .vulkan(fuchsia_sysmem2::wire::kVulkanImageUsageTransferDst)
                   .Build())
        .min_buffer_count(1)
        .buffer_memory_constraints(
            fuchsia_sysmem2::wire::BufferMemoryConstraints::Builder(arena)
                .cpu_domain_supported(true)
                .min_size_bytes(kMinSizeBytes)
                .max_size_bytes(kMaxSizeBytes)
                .permitted_heaps(std::array{
                    fuchsia_sysmem2::wire::Heap::Builder(arena)
                        .heap_type(
                            bind_fuchsia_goldfish_platform_sysmem_heap::HEAP_TYPE_HOST_VISIBLE)
                        .id(0)
                        .Build()})
                .Build())
        .image_format_constraints(std::array{image_constraints

                                                 .Build()});
    constraints.push_back(builder.Build());
  }

  fidl::WireSyncClient<BufferCollection> collection[kNumClients];
  for (size_t i = 0; i < kNumClients; i++) {
    collection[i] = fidl::WireSyncClient<BufferCollection>(std::move(collection_client[i])),
    SetDefaultCollectionName(collection[i]);
    EXPECT_TRUE(collection[i]
                    ->SetConstraints(
                        fuchsia_sysmem2::wire::BufferCollectionSetConstraintsRequest::Builder(arena)
                            .constraints(std::move(constraints[i]))
                            .Build())
                    .ok());
  };

  auto wait_result = collection[0]->WaitForAllBuffersAllocated();
  ASSERT_TRUE(wait_result.ok());
  EXPECT_TRUE(wait_result->is_ok());

  const auto& info = wait_result->value()->buffer_collection_info();
  EXPECT_EQ(info.buffers().size(), 1U);
  EXPECT_EQ(info.settings().buffer_settings().coherency_domain(),
            fuchsia_sysmem2::wire::CoherencyDomain::kCpu);

  const auto& image_format_constraints =
      wait_result->value()->buffer_collection_info().settings().image_format_constraints();

  EXPECT_EQ(image_format_constraints.min_size().width, 32u);
  EXPECT_EQ(image_format_constraints.min_size().height, 512u);
  EXPECT_EQ(image_format_constraints.required_max_size().width, 1024u);
  EXPECT_EQ(image_format_constraints.required_max_size().height, 256u);

  // Expected coded_width = max(min_coded_width, required_max_coded_width);
  // Expected coded_height = max(min_coded_height, required_max_coded_height).
  // Thus target size should be 1024 x 512 x 4.
  EXPECT_GE(info.settings().buffer_settings().size_bytes(), kTargetSizeBytes);

  zx::vmo vmo = std::move(info.buffers()[0].vmo());
  ASSERT_TRUE(vmo.is_valid());

  uint64_t vmo_size;
  EXPECT_EQ(vmo.get_size(&vmo_size), ZX_OK);
  EXPECT_GE(vmo_size, kTargetSizeBytes);
  EXPECT_LE(vmo_size, kMaxSizeBytes);

  // Test if the vmo is mappable.
  zx_vaddr_t addr;
  EXPECT_EQ(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, /*vmar_offset*/ 0, vmo,
                                       /*vmo_offset*/ 0, vmo_size, &addr),
            ZX_OK);

  // Test if write and read works correctly.
  uint8_t* ptr = reinterpret_cast<uint8_t*>(addr);
  std::vector<uint8_t> copy_target(vmo_size, 0u);
  for (uint32_t trial = 0; trial < 10u; trial++) {
    memset(ptr, trial, vmo_size);
    memcpy(copy_target.data(), ptr, vmo_size);
    zx_cache_flush(ptr, vmo_size, ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE);
    EXPECT_EQ(memcmp(copy_target.data(), ptr, vmo_size), 0);
  }

  EXPECT_EQ(zx::vmar::root_self()->unmap(addr, kPageSize), ZX_OK);

  for (size_t i = 0; i < kNumClients; i++) {
    EXPECT_TRUE(collection[i]->Release().ok());
  }
}

// In this test case we call CreateColorBuffer() and GetBufferHandle()
// on VMOs not registered with goldfish sysmem heap.
//
// The IPC transmission should succeed but FIDL interface should
// return ZX_ERR_INVALID_ARGS.
TEST(GoldfishControlTests, GoldfishControlTest_InvalidVmo) {
  zx::result control_connection = ConnectToControl();
  ASSERT_EQ(control_connection.status_value(), ZX_OK);
  fidl::WireSyncClient<fuchsia_hardware_goldfish::ControlDevice> control(
      std::move(control_connection.value()));

  zx::vmo non_sysmem_vmo;
  EXPECT_EQ(zx::vmo::create(1024u, 0u, &non_sysmem_vmo), ZX_OK);

  // Call CreateColorBuffer() using vmo not registered with goldfish
  // sysmem heap.
  zx::vmo vmo_copy;
  EXPECT_EQ(non_sysmem_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_copy), ZX_OK);

  {
    fidl::Arena allocator;
    fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params create_params(allocator);
    create_params.set_width(16)
        .set_height(16)
        .set_format(fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kBgra)
        .set_memory_property(fuchsia_hardware_goldfish::wire::kMemoryPropertyDeviceLocal);

    auto result = control->CreateColorBuffer2(std::move(vmo_copy), std::move(create_params));
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_ERR_INVALID_ARGS);
  }

  // Call GetBufferHandle() using vmo not registered with goldfish
  // sysmem heap.
  zx::vmo vmo_copy2;
  EXPECT_EQ(non_sysmem_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_copy2), ZX_OK);

  {
    auto result = control->GetBufferHandle(std::move(vmo_copy2));
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_ERR_INVALID_ARGS);
  }
}

// In this test case we test arguments of CreateColorBuffer2() method.
// If a mandatory field is missing, it should return "ZX_ERR_INVALID_ARGS".
TEST(GoldfishControlTests, GoldfishControlTest_CreateColorBuffer2Args) {
  // Setup control device.
  zx::result control_connection = ConnectToControl();
  ASSERT_EQ(control_connection.status_value(), ZX_OK);
  fidl::WireSyncClient<fuchsia_hardware_goldfish::ControlDevice> control(
      std::move(control_connection.value()));

  // ----------------------------------------------------------------------//
  // Setup sysmem allocator and buffer collection.
  auto allocator = CreateSysmemAllocator();
  ASSERT_TRUE(allocator.is_valid());

  auto token_endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  fidl::Arena arena;
  ASSERT_EQ(allocator
                ->AllocateSharedCollection(
                    fuchsia_sysmem2::wire::AllocatorAllocateSharedCollectionRequest::Builder(arena)
                        .token_request(std::move(token_endpoints.server))
                        .Build())
                .status(),
            ZX_OK);

  auto collection_endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
  ASSERT_EQ(allocator
                ->BindSharedCollection(
                    fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
                        .buffer_collection_request(std::move(collection_endpoints.server))
                        .token(std::move(token_endpoints.client))
                        .Build())
                .status(),
            ZX_OK);

  // ----------------------------------------------------------------------//
  // Use device local heap which only *registers* the koid of vmo to control
  // device.
  auto constraints = fuchsia_sysmem2::wire::BufferCollectionConstraints::Builder(arena);
  constraints
      .usage(fuchsia_sysmem2::wire::BufferUsage::Builder(arena)
                 .vulkan(fuchsia_sysmem2::wire::kVulkanImageUsageTransferDst)
                 .Build())
      .min_buffer_count_for_camping(1)
      .buffer_memory_constraints(
          fuchsia_sysmem2::wire::BufferMemoryConstraints::Builder(arena)
              .min_size_bytes(4 * 1024)
              .max_size_bytes(4 * 1024)
              .physically_contiguous_required(false)
              .secure_required(false)
              .ram_domain_supported(false)
              .cpu_domain_supported(false)
              .inaccessible_domain_supported(true)
              .permitted_heaps(std::array{
                  fuchsia_sysmem2::wire::Heap::Builder(arena)
                      .heap_type(bind_fuchsia_goldfish_platform_sysmem_heap::HEAP_TYPE_DEVICE_LOCAL)
                      .id(0)
                      .Build()})
              .Build());

  fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection> collection(
      std::move(collection_endpoints.client));
  SetDefaultCollectionName(collection);
  EXPECT_TRUE(collection
                  ->SetConstraints(
                      fuchsia_sysmem2::wire::BufferCollectionSetConstraintsRequest::Builder(arena)
                          .constraints(constraints.Build())
                          .Build())
                  .ok());

  auto wait_result = collection->WaitForAllBuffersAllocated();
  ASSERT_TRUE(wait_result.ok());
  EXPECT_TRUE(wait_result->is_ok());

  const auto& info = wait_result->value()->buffer_collection_info();
  EXPECT_EQ(info.buffers().size(), 1U);

  zx::vmo vmo = std::move(info.buffers()[0].vmo());
  ASSERT_TRUE(vmo.is_valid());

  EXPECT_TRUE(collection->Release().ok());

  // ----------------------------------------------------------------------//
  // Try creating color buffer.
  zx::vmo vmo_copy;

  {
    // Verify that a CreateColorBuffer2() call without width will fail.
    EXPECT_EQ(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_copy), ZX_OK);
    fidl::Arena allocator;
    fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params create_params(allocator);
    // Without width
    create_params.set_height(64)
        .set_format(fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kBgra)
        .set_memory_property(fuchsia_hardware_goldfish::wire::kMemoryPropertyDeviceLocal);

    auto result = control->CreateColorBuffer2(std::move(vmo_copy), std::move(create_params));

    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_ERR_INVALID_ARGS);
    EXPECT_LT(result->hw_address_page_offset, 0);
  }

  {
    // Verify that a CreateColorBuffer2() call without height will fail.
    EXPECT_EQ(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_copy), ZX_OK);
    fidl::Arena allocator;
    fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params create_params(allocator);
    // Without height
    create_params.set_width(64)
        .set_format(fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kBgra)
        .set_memory_property(fuchsia_hardware_goldfish::wire::kMemoryPropertyDeviceLocal);

    auto result = control->CreateColorBuffer2(std::move(vmo_copy), std::move(create_params));

    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_ERR_INVALID_ARGS);
    EXPECT_LT(result->hw_address_page_offset, 0);
  }

  {
    // Verify that a CreateColorBuffer2() call without color format will fail.
    EXPECT_EQ(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_copy), ZX_OK);
    fidl::Arena allocator;
    fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params create_params(allocator);
    // Without format
    create_params.set_width(64).set_height(64).set_memory_property(
        fuchsia_hardware_goldfish::wire::kMemoryPropertyDeviceLocal);

    auto result = control->CreateColorBuffer2(std::move(vmo_copy), std::move(create_params));

    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_ERR_INVALID_ARGS);
    EXPECT_LT(result->hw_address_page_offset, 0);
  }

  {
    // Verify that a CreateColorBuffer2() call without memory property will fail.
    EXPECT_EQ(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_copy), ZX_OK);
    fidl::Arena allocator;
    fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params create_params(allocator);
    // Without memory property
    create_params.set_width(64).set_height(64).set_format(
        fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kBgra);

    auto result = control->CreateColorBuffer2(std::move(vmo_copy), std::move(create_params));

    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_ERR_INVALID_ARGS);
    EXPECT_LT(result->hw_address_page_offset, 0);
  }
}

// In this test case we call GetBufferHandle() on a vmo
// registered to the control device but we haven't created
// the color buffer yet.
//
// The FIDL interface should return ZX_ERR_NOT_FOUND.
TEST(GoldfishControlTests, GoldfishControlTest_GetNotCreatedColorBuffer) {
  zx::result control_connection = ConnectToControl();
  ASSERT_EQ(control_connection.status_value(), ZX_OK);
  fidl::WireSyncClient<fuchsia_hardware_goldfish::ControlDevice> control(
      std::move(control_connection.value()));

  auto allocator = CreateSysmemAllocator();
  ASSERT_TRUE(allocator.is_valid());

  auto token_endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  fidl::Arena arena;
  ASSERT_EQ(allocator
                ->AllocateSharedCollection(
                    fuchsia_sysmem2::wire::AllocatorAllocateSharedCollectionRequest::Builder(arena)
                        .token_request(std::move(token_endpoints.server))
                        .Build())
                .status(),
            ZX_OK);
  auto collection_endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
  ASSERT_EQ(allocator
                ->BindSharedCollection(
                    fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
                        .buffer_collection_request(std::move(collection_endpoints.server))
                        .token(std::move(token_endpoints.client))
                        .Build())
                .status(),
            ZX_OK);
  auto constraints = fuchsia_sysmem2::wire::BufferCollectionConstraints::Builder(arena);
  constraints
      .usage(fuchsia_sysmem2::wire::BufferUsage::Builder(arena)
                 .vulkan(fuchsia_sysmem2::wire::kVulkanImageUsageTransferDst)
                 .Build())
      .min_buffer_count_for_camping(1)
      .buffer_memory_constraints(
          fuchsia_sysmem2::wire::BufferMemoryConstraints::Builder(arena)
              .min_size_bytes(4 * 1024)
              .max_size_bytes(4 * 1024)
              .physically_contiguous_required(false)
              .secure_required(false)
              .ram_domain_supported(false)
              .cpu_domain_supported(false)
              .inaccessible_domain_supported(true)
              .permitted_heaps(std::array{
                  fuchsia_sysmem2::wire::Heap::Builder(arena)
                      .heap_type(bind_fuchsia_goldfish_platform_sysmem_heap::HEAP_TYPE_DEVICE_LOCAL)
                      .id(0)
                      .Build()})
              .Build());

  fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection> collection(
      std::move(collection_endpoints.client));
  SetDefaultCollectionName(collection);
  EXPECT_TRUE(collection
                  ->SetConstraints(
                      fuchsia_sysmem2::wire::BufferCollectionSetConstraintsRequest::Builder(arena)
                          .constraints(constraints.Build())
                          .Build())
                  .ok());
  auto wait_result = collection->WaitForAllBuffersAllocated();
  ASSERT_TRUE(wait_result.ok());
  EXPECT_TRUE(wait_result->is_ok());

  const auto& info = wait_result->value()->buffer_collection_info();
  EXPECT_EQ(info.buffers().size(), 1U);

  zx::vmo vmo = std::move(info.buffers()[0].vmo());
  ASSERT_TRUE(vmo.is_valid());

  EXPECT_TRUE(collection->Release().ok());

  zx::vmo vmo_copy;
  EXPECT_EQ(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_copy), ZX_OK);

  {
    auto result = control->GetBufferHandle(std::move(vmo_copy));
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_ERR_NOT_FOUND);
  }
}

TEST(GoldfishAddressSpaceTests, GoldfishAddressSpaceTest) {
  zx::result asd_connection = ConnectToAddress();
  ASSERT_EQ(asd_connection.status_value(), ZX_OK);
  fidl::WireSyncClient asd_parent(std::move(asd_connection.value()));

  auto child_endpoints =
      fidl::Endpoints<fuchsia_hardware_goldfish::AddressSpaceChildDriver>::Create();
  {
    auto result = asd_parent->OpenChildDriver(
        fuchsia_hardware_goldfish::wire::AddressSpaceChildDriverType::kDefault,
        std::move(child_endpoints.server));
    ASSERT_TRUE(result.ok());
  }

  constexpr uint64_t kHeapSize = 16ULL * 1048576ULL;

  fidl::WireSyncClient asd_child(std::move(child_endpoints.client));
  uint64_t paddr = 0;
  zx::vmo vmo;
  {
    auto result = asd_child->AllocateBlock(kHeapSize);
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_OK);

    paddr = result->paddr;
    EXPECT_NE(paddr, 0U);

    vmo = std::move(result->vmo);
    EXPECT_EQ(vmo.is_valid(), true);
    uint64_t actual_size = 0;
    EXPECT_EQ(vmo.get_size(&actual_size), ZX_OK);
    EXPECT_GE(actual_size, kHeapSize);
  }

  zx::vmo vmo2;
  uint64_t paddr2 = 0;
  {
    auto result = asd_child->AllocateBlock(kHeapSize);
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_OK);

    paddr2 = result->paddr;
    EXPECT_NE(paddr2, 0U);
    EXPECT_NE(paddr2, paddr);

    vmo2 = std::move(result->vmo);
    EXPECT_EQ(vmo2.is_valid(), true);
    uint64_t actual_size = 0;
    EXPECT_EQ(vmo2.get_size(&actual_size), ZX_OK);
    EXPECT_GE(actual_size, kHeapSize);
  }

  {
    auto result = asd_child->DeallocateBlock(paddr);
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_OK);
  }

  {
    auto result = asd_child->DeallocateBlock(paddr2);
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_OK);
  }

  // No testing into this too much, as it's going to be child driver-specific.
  // Use fixed values for shared offset/size and ping metadata.
  const uint64_t shared_offset = 4096;
  const uint64_t shared_size = 4096;

  const uint64_t overlap_offsets[] = {
      4096,
      0,
      8191,
  };
  const uint64_t overlap_sizes[] = {
      2048,
      4097,
      4096,
  };

  const size_t overlaps_to_test = sizeof(overlap_offsets) / sizeof(overlap_offsets[0]);

  using fuchsia_hardware_goldfish::wire::AddressSpaceChildDriverPingMessage;

  AddressSpaceChildDriverPingMessage msg;
  msg.metadata = 0;

  EXPECT_TRUE(asd_child->Ping(msg).ok());

  {
    auto result = asd_child->ClaimSharedBlock(shared_offset, shared_size);
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_OK);
  }

  // Test that overlapping blocks cannot be claimed in the same connection.
  for (size_t i = 0; i < overlaps_to_test; ++i) {
    auto result = asd_child->ClaimSharedBlock(overlap_offsets[i], overlap_sizes[i]);
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_ERR_INVALID_ARGS);
  }

  {
    auto result = asd_child->UnclaimSharedBlock(shared_offset);
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_OK);
  }

  // Test that removed or unknown offsets cannot be unclaimed.
  {
    auto result = asd_child->UnclaimSharedBlock(shared_offset);
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_ERR_INVALID_ARGS);
  }

  {
    auto result = asd_child->UnclaimSharedBlock(0);
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_ERR_INVALID_ARGS);
  }
}

// This is a test case testing goldfish Heap, control device, address space
// device, and host implementation of host-visible memory allocation.
//
// This test case using a device-local Heap and a pre-allocated address space
// block to simulate a host-visible sysmem Heap. It does the following things:
//
// 1) It allocates a memory block (vmo = |address_space_vmo| and gpa =
//    |physical_addr|) from address space device.
//
// 2) It allocates an vmo (vmo = |vmo|) from the goldfish device-local Heap
//    so that |vmo| is registered for color buffer creation.
//
// 3) It calls goldfish Control FIDL API to create a color buffer using |vmo|.
//    and maps memory to |physical_addr|.
//
// 4) The color buffer creation and memory process should work correctly, and
//    heap offset should be a non-negative value.
//
TEST(GoldfishHostMemoryTests, GoldfishHostVisibleColorBuffer) {
  // Setup control device.
  zx::result control_connection = ConnectToControl();
  ASSERT_EQ(control_connection.status_value(), ZX_OK);
  fidl::WireSyncClient<fuchsia_hardware_goldfish::ControlDevice> control(
      std::move(control_connection.value()));

  // ----------------------------------------------------------------------//
  // Setup sysmem allocator and buffer collection.
  auto allocator = CreateSysmemAllocator();
  ASSERT_TRUE(allocator.is_valid());

  auto token_endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  fidl::Arena arena;
  ASSERT_EQ(allocator
                ->AllocateSharedCollection(
                    fuchsia_sysmem2::wire::AllocatorAllocateSharedCollectionRequest::Builder(arena)
                        .token_request(std::move(token_endpoints.server))
                        .Build())
                .status(),
            ZX_OK);

  auto collection_endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
  ASSERT_EQ(allocator
                ->BindSharedCollection(
                    fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
                        .buffer_collection_request(std::move(collection_endpoints.server))
                        .token(std::move(token_endpoints.client))
                        .Build())
                .status(),
            ZX_OK);

  // ----------------------------------------------------------------------//
  // Setup address space driver.
  zx::result asd_connection = ConnectToAddress();
  ASSERT_EQ(asd_connection.status_value(), ZX_OK);
  fidl::WireSyncClient asd_parent(std::move(asd_connection.value()));

  auto child_endpoints =
      fidl::Endpoints<fuchsia_hardware_goldfish::AddressSpaceChildDriver>::Create();

  {
    auto result = asd_parent->OpenChildDriver(
        fuchsia_hardware_goldfish::wire::AddressSpaceChildDriverType::kDefault,
        std::move(child_endpoints.server));
    ASSERT_TRUE(result.ok());
  }

  // Allocate device memory block using address space device.
  constexpr uint64_t kHeapSize = 32768ULL;

  fidl::WireSyncClient asd_child(std::move(child_endpoints.client));
  uint64_t physical_addr = 0;
  zx::vmo address_space_vmo;
  {
    auto result = asd_child->AllocateBlock(kHeapSize);
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_OK);

    physical_addr = result->paddr;
    EXPECT_NE(physical_addr, 0U);

    address_space_vmo = std::move(result->vmo);
    EXPECT_EQ(address_space_vmo.is_valid(), true);
    uint64_t actual_size = 0;
    EXPECT_EQ(address_space_vmo.get_size(&actual_size), ZX_OK);
    EXPECT_GE(actual_size, kHeapSize);
  }

  // ----------------------------------------------------------------------//
  // Use device local heap which only *registers* the koid of vmo to control
  // device.
  auto constraints = fuchsia_sysmem2::wire::BufferCollectionConstraints::Builder(arena);
  constraints
      .usage(fuchsia_sysmem2::wire::BufferUsage::Builder(arena)
                 .vulkan(fuchsia_sysmem2::wire::kVulkanImageUsageTransferDst)
                 .Build())
      .min_buffer_count_for_camping(1)
      .buffer_memory_constraints(
          fuchsia_sysmem2::wire::BufferMemoryConstraints::Builder(arena)
              .min_size_bytes(4 * 1024)
              .max_size_bytes(4 * 1024)
              .physically_contiguous_required(false)
              .secure_required(false)
              .ram_domain_supported(false)
              .cpu_domain_supported(false)
              .inaccessible_domain_supported(true)
              .permitted_heaps(std::array{
                  fuchsia_sysmem2::wire::Heap::Builder(arena)
                      .heap_type(bind_fuchsia_goldfish_platform_sysmem_heap::HEAP_TYPE_DEVICE_LOCAL)
                      .id(0)
                      .Build()})
              .Build());

  fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection> collection(
      std::move(collection_endpoints.client));

  SetDefaultCollectionName(collection);
  EXPECT_TRUE(collection
                  ->SetConstraints(
                      fuchsia_sysmem2::wire::BufferCollectionSetConstraintsRequest::Builder(arena)
                          .constraints(constraints.Build())
                          .Build())
                  .ok());
  auto wait_result = collection->WaitForAllBuffersAllocated();
  ASSERT_TRUE(wait_result.ok());
  EXPECT_TRUE(wait_result->is_ok());

  const auto& info = wait_result->value()->buffer_collection_info();
  EXPECT_EQ(info.buffers().size(), 1U);

  zx::vmo vmo = std::move(info.buffers()[0].vmo());
  ASSERT_TRUE(vmo.is_valid());

  EXPECT_TRUE(collection->Release().ok());

  // ----------------------------------------------------------------------//
  // Creates color buffer and map host memory.
  zx::vmo vmo_copy;
  EXPECT_EQ(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_copy), ZX_OK);

  {
    // Verify that a CreateColorBuffer2() call with host-visible memory property,
    // but without physical address will fail.
    fidl::Arena allocator;
    fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params create_params(allocator);
    // Without physical address
    create_params.set_width(64)
        .set_height(64)
        .set_format(fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kBgra)
        .set_memory_property(fuchsia_hardware_goldfish::wire::kMemoryPropertyHostVisible);

    auto result = control->CreateColorBuffer2(std::move(vmo_copy), create_params);

    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_ERR_INVALID_ARGS);
    EXPECT_LT(result->hw_address_page_offset, 0);
  }

  EXPECT_EQ(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_copy), ZX_OK);
  {
    fidl::Arena allocator;
    fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params create_params(allocator);
    create_params.set_width(64)
        .set_height(64)
        .set_format(fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kBgra)
        .set_memory_property(0x02u)
        .set_physical_address(allocator, physical_addr);

    auto result = control->CreateColorBuffer2(std::move(vmo_copy), create_params);

    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_OK);
    EXPECT_GE(result->hw_address_page_offset, 0);
  }

  // Verify if the color buffer works correctly.
  zx::vmo vmo_copy2;
  EXPECT_EQ(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_copy2), ZX_OK);
  {
    auto result = control->GetBufferHandle(std::move(vmo_copy2));
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_OK);
    EXPECT_NE(result->id, 0u);
    EXPECT_EQ(result->type, fuchsia_hardware_goldfish::wire::BufferHandleType::kColorBuffer);
  }

  // Cleanup.
  {
    auto result = asd_child->DeallocateBlock(physical_addr);
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_OK);
  }
}

using GoldfishCreateColorBufferTest =
    testing::TestWithParam<fuchsia_hardware_goldfish::wire::ColorBufferFormatType>;

TEST_P(GoldfishCreateColorBufferTest, CreateColorBufferWithFormat) {
  zx::result control_connection = ConnectToControl();
  ASSERT_EQ(control_connection.status_value(), ZX_OK);
  fidl::WireSyncClient<fuchsia_hardware_goldfish::ControlDevice> control(
      std::move(control_connection.value()));

  auto allocator = CreateSysmemAllocator();
  ASSERT_TRUE(allocator.is_valid());

  auto token_endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  fidl::Arena arena;
  ASSERT_EQ(allocator
                ->AllocateSharedCollection(
                    fuchsia_sysmem2::wire::AllocatorAllocateSharedCollectionRequest::Builder(arena)
                        .token_request(std::move(token_endpoints.server))
                        .Build())
                .status(),
            ZX_OK);

  auto collection_endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
  ASSERT_EQ(allocator
                ->BindSharedCollection(
                    fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
                        .buffer_collection_request(std::move(collection_endpoints.server))
                        .token(std::move(token_endpoints.client))
                        .Build())
                .status(),
            ZX_OK);

  auto constraints = fuchsia_sysmem2::wire::BufferCollectionConstraints::Builder(arena);
  constraints
      .usage(fuchsia_sysmem2::wire::BufferUsage::Builder(arena)
                 .vulkan(fuchsia_sysmem2::wire::kVulkanImageUsageTransferDst)
                 .Build())
      .min_buffer_count_for_camping(1)
      .buffer_memory_constraints(
          fuchsia_sysmem2::wire::BufferMemoryConstraints::Builder(arena)
              .min_size_bytes(4 * 1024)
              .max_size_bytes(4 * 1024)
              .physically_contiguous_required(false)
              .secure_required(false)
              .ram_domain_supported(false)
              .cpu_domain_supported(false)
              .inaccessible_domain_supported(true)
              .permitted_heaps(std::array{
                  fuchsia_sysmem2::wire::Heap::Builder(arena)
                      .heap_type(bind_fuchsia_goldfish_platform_sysmem_heap::HEAP_TYPE_DEVICE_LOCAL)
                      .id(0)
                      .Build()})
              .Build());

  fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection> collection(
      std::move(collection_endpoints.client));

  SetDefaultCollectionName(collection);
  EXPECT_TRUE(collection
                  ->SetConstraints(
                      fuchsia_sysmem2::wire::BufferCollectionSetConstraintsRequest::Builder(arena)
                          .constraints(constraints.Build())
                          .Build())
                  .ok());

  auto wait_result = collection->WaitForAllBuffersAllocated();
  ASSERT_TRUE(wait_result.ok());
  EXPECT_TRUE(wait_result->is_ok());

  const auto& info = wait_result->value()->buffer_collection_info();
  EXPECT_EQ(info.buffers().size(), 1U);

  zx::vmo vmo = std::move(info.buffers()[0].vmo());
  ASSERT_TRUE(vmo.is_valid());

  EXPECT_TRUE(collection->Release().ok());

  zx::vmo vmo_copy;
  EXPECT_EQ(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_copy), ZX_OK);

  {
    fidl::Arena allocator;
    fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params create_params(allocator);
    create_params.set_width(64)
        .set_height(64)
        .set_format(GetParam())
        .set_memory_property(fuchsia_hardware_goldfish::wire::kMemoryPropertyDeviceLocal);

    auto result = control->CreateColorBuffer2(std::move(vmo_copy), create_params);
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_OK);
  }

  zx::vmo vmo_copy2;
  EXPECT_EQ(vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo_copy2), ZX_OK);

  {
    auto result = control->GetBufferHandle(std::move(vmo_copy2));
    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result->res, ZX_OK);
    EXPECT_NE(result->id, 0u);
    EXPECT_EQ(result->type, fuchsia_hardware_goldfish::wire::BufferHandleType::kColorBuffer);
  }
}

TEST(GoldfishControlTests, CreateSyncKhr) {
  zx::result control_connection = ConnectToControl();
  ASSERT_EQ(control_connection.status_value(), ZX_OK);
  fidl::WireSyncClient<fuchsia_hardware_goldfish::ControlDevice> control(
      std::move(control_connection.value()));

  zx::eventpair event_client, event_server;
  zx_status_t status = zx::eventpair::create(0u, &event_client, &event_server);
  {
    auto result = control->CreateSyncFence(std::move(event_server));
    ASSERT_TRUE(result.ok());
  }

  zx_signals_t pending;
  status = event_client.wait_one(ZX_EVENTPAIR_SIGNALED, zx::deadline_after(zx::sec(10)), &pending);
  EXPECT_EQ(status, ZX_OK);
}

INSTANTIATE_TEST_SUITE_P(
    ColorBufferTests, GoldfishCreateColorBufferTest,
    testing::Values(fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kRgba,
                    fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kBgra,
                    fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kRg,
                    fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kLuminance),
    [](const testing::TestParamInfo<GoldfishCreateColorBufferTest::ParamType>& info)
        -> std::string {
      switch (info.param) {
        case fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kRgba:
          return "RGBA";
        case fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kBgra:
          return "BGRA";
        case fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kRg:
          return "RG";
        case fuchsia_hardware_goldfish::wire::ColorBufferFormatType::kLuminance:
          return "LUMINANCE";
      }
    });
