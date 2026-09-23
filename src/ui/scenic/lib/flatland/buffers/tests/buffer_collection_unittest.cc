// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/buffers/buffer_collection.h"

#include <lib/fdio/directory.h>

#include <gtest/gtest.h>

#include "src/lib/fsl/handles/object_info.h"
#include "src/lib/testing/loop_fixture/real_loop_fixture.h"
#include "src/ui/scenic/lib/flatland/buffers/util.h"

namespace flatland {
namespace test {

// Common testing base class to be used across different unittests that
// require Vulkan and a SysmemAllocator.
class BufferCollectionTest : public gtest::RealLoopFixture {
 protected:
  void SetUp() override {
    ::testing::Test::SetUp();
    // Create the SysmemAllocator.
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_sysmem2::Allocator>::Create();
    zx_status_t status =
        fdio_service_connect("/svc/fuchsia.sysmem2.Allocator", server_end.TakeChannel().release());
    ASSERT_EQ(status, ZX_OK);
    sysmem_allocator_.Bind(std::move(client_end), dispatcher());

    fidl::Arena arena;
    fidl::OneWayStatus result = sysmem_allocator_->SetDebugClientInfo(
        fuchsia_sysmem2::wire::AllocatorSetDebugClientInfoRequest::Builder(arena)
            .name(arena, fsl::GetCurrentProcessName() + " BufferCollectionTest")
            .id(fsl::GetCurrentProcessKoid())
            .Build());
    ASSERT_TRUE(result.ok());
  }

  void TearDown() override {
    sysmem_allocator_ = {};
    ::testing::Test::TearDown();
  }

  fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator_;
};

// Test the creation of a buffer collection that doesn't have any additional vulkan
// constraints to show that it doesn't need vulkan to be valid.
TEST_F(BufferCollectionTest, CreateCollectionTest) {
  auto [_, dup_token] = SysmemTokens::Create(sysmem_allocator_);
  auto result = BufferCollectionInfo::New(sysmem_allocator_, std::move(dup_token));
  EXPECT_TRUE(result.is_ok());
}

// This test ensures that the buffer collection can still be allocated even if the server
// does not set extra customizable constraints via a call to GenerateToken(). This is
// necessary due to the fact that the buffer collection keeps around a dummy token in
// case new constraints need to be added, but the existence of the dummy token itself
// prevents allocation until it is closed out. So this test makes sure that when we close
// out the dummy token inside the call to WaitUntilAllocated() that this is enough to ensure
// that we can still allocate the buffer collection.
TEST_F(BufferCollectionTest, AllocationWithoutExtraConstraints) {
  fuchsia_sysmem2::BufferUsage buffer_usage;
  buffer_usage.cpu(fuchsia_sysmem2::kCpuUsageWriteOften);
  auto [local_token, dup_token] = SysmemTokens::Create(sysmem_allocator_);
  auto result = BufferCollectionInfo::New(sysmem_allocator_, std::move(dup_token), std::nullopt,
                                          std::move(buffer_usage));
  EXPECT_TRUE(result.is_ok());

  auto collection = std::move(result.value());

  // Client hasn't set their constraints yet, so this should be false.
  EXPECT_FALSE(collection.BuffersAreAllocated());

  {
    const uint32_t kWidth = 32;
    const uint32_t kHeight = 64;
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();

    fidl::Arena arena;
    fidl::OneWayStatus result = sysmem_allocator_->BindSharedCollection(
        fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
            .token(std::move(local_token))
            .buffer_collection_request(std::move(server_end))
            .Build());
    EXPECT_TRUE(result.ok());
    fidl::SyncClient<fuchsia_sysmem2::BufferCollection> buffer_collection(std::move(client_end));

    fuchsia_sysmem2::NodeSetNameRequest set_name_request;
    set_name_request.priority(10u);
    set_name_request.name("FlatlandImageMemory");
    auto set_name_result = buffer_collection->SetName(std::move(set_name_request));
    EXPECT_TRUE(set_name_result.is_ok());

    fuchsia_sysmem2::BufferCollectionConstraints constraints;
    fuchsia_sysmem2::BufferMemoryConstraints bmc;
    bmc.cpu_domain_supported(true);
    bmc.ram_domain_supported(true);
    constraints.buffer_memory_constraints(std::move(bmc));
    fuchsia_sysmem2::BufferUsage usage;
    usage.cpu(fuchsia_sysmem2::kCpuUsageWriteOften);
    constraints.usage(std::move(usage));
    constraints.min_buffer_count(1);

    fuchsia_sysmem2::ImageFormatConstraints image_constraints;
    image_constraints.color_spaces({{fuchsia_images2::ColorSpace::kSrgb}});
    image_constraints.pixel_format(fuchsia_images2::PixelFormat::kB8G8R8A8);
    image_constraints.pixel_format_modifier(fuchsia_images2::PixelFormatModifier::kLinear);

    image_constraints.min_size(fuchsia_math::SizeU{kWidth, kHeight});
    image_constraints.max_size(fuchsia_math::SizeU{kWidth, kHeight});
    constraints.image_format_constraints({{std::move(image_constraints)}});

    fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
    set_constraints_request.constraints(std::move(constraints));
    auto set_constraints_result =
        buffer_collection->SetConstraints(std::move(set_constraints_request));
    EXPECT_TRUE(set_constraints_result.is_ok());

    // Have the client wait for allocation.
    auto wait_result = buffer_collection->WaitForAllBuffersAllocated();
    EXPECT_TRUE(wait_result.is_ok());

    auto release_result = buffer_collection->Release();
    EXPECT_TRUE(release_result.is_ok());
  }

  // Checking allocation on the server should return true.
  EXPECT_TRUE(collection.BuffersAreAllocated());
}

// Check to make sure |CreateBufferCollectionAndSetConstraints| returns false if
// an invalid BufferCollectionHandle is provided by the user.
TEST_F(BufferCollectionTest, NullTokenTest) {
  auto result = BufferCollectionInfo::New(sysmem_allocator_,
                                          /*token*/ {});
  EXPECT_TRUE(result.is_error());
}

// We pass in a valid channel to |CreateBufferCollectionAndSetConstraints|, but
// it's not actually a channel to a BufferCollection.
TEST_F(BufferCollectionTest, WrongTokenTypeTest) {
  zx::channel local_endpoint;
  zx::channel remote_endpoint;
  zx::channel::create(0, &local_endpoint, &remote_endpoint);

  // Here we inject a generic channel into a BufferCollectionHandle before passing the
  // handle into |CreateCollectionAndSetConstraints|. So the channel is valid,
  // but it is just not a BufferCollectionToken.
  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> handle{std::move(remote_endpoint)};

  // Make sure the handle is valid before passing it in.
  ASSERT_TRUE(handle.is_valid());

  // We should not be able to make a BufferCollectionInfon object with the wrong token type
  // passed in as a parameter.
  auto result = BufferCollectionInfo::New(sysmem_allocator_, std::move(handle));
  EXPECT_TRUE(result.is_error());
}

// If the client sets constraints on the buffer collection that are incompatible
// with the constraints set on the server-side by the renderer, then waiting on
// the buffers to be allocated should fail.
TEST_F(BufferCollectionTest, IncompatibleConstraintsTest) {
  auto [local_token, dup_token] = SysmemTokens::Create(sysmem_allocator_);
  auto result = BufferCollectionInfo::New(sysmem_allocator_, std::move(dup_token));
  EXPECT_TRUE(result.is_ok());

  auto collection = std::move(result.value());

  // Create a client-side handle to the buffer collection and set the client
  // constraints. We set it to have a max of zero buffers and to declare no
  // vulkan usage, whereas the server side will specify that vulkan sampling
  // is necessary.
  {
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();

    fidl::Arena arena;
    fidl::OneWayStatus result = sysmem_allocator_->BindSharedCollection(
        fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
            .token(std::move(local_token))
            .buffer_collection_request(std::move(server_end))
            .Build());
    EXPECT_TRUE(result.ok());
    fidl::SyncClient<fuchsia_sysmem2::BufferCollection> client_collection(std::move(client_end));

    fuchsia_sysmem2::NodeSetNameRequest set_name_request;
    set_name_request.priority(100u);
    set_name_request.name("FlatlandIncompatibleConstraintsTest");
    auto set_name_result = client_collection->SetName(std::move(set_name_request));
    EXPECT_TRUE(set_name_result.is_ok());

    fuchsia_sysmem2::BufferCollectionConstraints constraints;
    fuchsia_sysmem2::BufferMemoryConstraints bmc;
    bmc.cpu_domain_supported(true);
    bmc.ram_domain_supported(true);
    constraints.buffer_memory_constraints(std::move(bmc));
    fuchsia_sysmem2::BufferUsage usage;
    usage.cpu(fuchsia_sysmem2::kCpuUsageWriteOften);

    // Need at least one buffer normally.
    constraints.min_buffer_count(0);
    constraints.max_buffer_count(0);

    constraints.usage(std::move(usage));

    fuchsia_sysmem2::ImageFormatConstraints image_constraints;

    image_constraints.pixel_format(fuchsia_images2::PixelFormat::kR8G8B8A8);
    image_constraints.pixel_format_modifier(fuchsia_images2::PixelFormatModifier::kLinear);

    // The renderer requires that the the buffer can at least have a
    // width/height of 1, which is not possible here.
    image_constraints.required_min_size(fuchsia_math::SizeU(0, 0));
    image_constraints.required_max_size(fuchsia_math::SizeU(0, 0));
    image_constraints.max_size(fuchsia_math::SizeU(0, 0));
    image_constraints.max_bytes_per_row(0x0);
    constraints.image_format_constraints({{std::move(image_constraints)}});

    fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
    set_constraints_request.constraints(std::move(constraints));
    auto set_constraints_result =
        client_collection->SetConstraints(std::move(set_constraints_request));
    EXPECT_TRUE(set_constraints_result.is_ok());

    // Have the client wait for allocation.
    auto wait_result = client_collection->WaitForAllBuffersAllocated();

    // We'll see the error here one of two ways. Either sysmem has already disconnected due to
    // allocation failure by the time the wait starts, or the wait starts before the allocation
    // failure and reports CONSTRAINTS_INTERSECTION_EMPTY.
    ASSERT_FALSE(wait_result.is_ok());
    if (wait_result.error_value().is_framework_error()) {
      EXPECT_EQ(wait_result.error_value().framework_error().status(), ZX_ERR_PEER_CLOSED);
    } else {
      EXPECT_EQ(wait_result.error_value().domain_error(),
                fuchsia_sysmem2::Error::kConstraintsIntersectionEmpty);
    }
  }

  // This should fail as sysmem won't be able to allocate anything.
  EXPECT_FALSE(collection.BuffersAreAllocated());
}

}  // namespace test
}  // namespace flatland
