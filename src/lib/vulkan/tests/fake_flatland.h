// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dlfcn.h>
#include <fuchsia/ui/composition/cpp/fidl.h>
#include <fuchsia/ui/composition/cpp/fidl_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/fdio/directory.h>
#include <lib/fidl/cpp/binding.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>

#include <atomic>
#include <set>

#include <gtest/gtest.h>
#include <vulkan/vulkan.h>

#include "sdk/lib/ui/scenic/cpp/view_creation_tokens.h"
#include "src/lib/fsl/handles/object_info.h"

namespace {

uint64_t ZirconIdFromHandle(uint32_t handle) {
  zx_info_handle_basic_t info;
  zx_status_t status =
      zx_object_get_info(handle, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  if (status != ZX_OK)
    return 0;
  return info.koid;
}

// FakeFlatland runs async loop on its own thread to allow the test
// to use blocking Vulkan calls while present callbacks are processed.
class FakeFlatland : public fuchsia::ui::composition::testing::Allocator_TestBase,
                     public fuchsia::ui::composition::testing::Flatland_TestBase,
                     public fuchsia::ui::composition::testing::ParentViewportWatcher_TestBase {
 public:
  FakeFlatland(fidl::InterfaceRequest<fuchsia::ui::composition::Allocator> allocator_request,
               fidl::InterfaceRequest<fuchsia::ui::composition::Flatland> flatland_request,
               bool should_present)
      : loop_(&kAsyncLoopConfigNoAttachToCurrentThread),
        allocator_binding_(this, std::move(allocator_request), loop_.dispatcher()),
        flatland_binding_(this, std::move(flatland_request), loop_.dispatcher()),
        should_present_(should_present) {
    loop_.StartThread("fake flatland");
  }

  ~FakeFlatland() { loop_.Shutdown(); }

  void NotImplemented_(const std::string& name) override {
    fprintf(stderr, "NotImplemented: %s\n", name.c_str());
  }

  // |fuchsia::ui::composition::testing::Allocator|
  void RegisterBufferCollection(fuchsia::ui::composition::RegisterBufferCollectionArgs args,
                                RegisterBufferCollectionCallback callback) override {
    EXPECT_EQ(fuchsia::ui::composition::RegisterBufferCollectionUsage::DEFAULT, args.usage());
    auto [_, import_token_koid] = fsl::GetKoids(args.export_token().value.get());
    registered_koids.insert(import_token_koid);

    fuchsia::sysmem2::AllocatorSyncPtr sysmem_allocator;
    zx_status_t status = fdio_service_connect(
        "/svc/fuchsia.sysmem2.Allocator", sysmem_allocator.NewRequest().TakeChannel().release());
    EXPECT_EQ(status, ZX_OK);
    sysmem_allocator->SetDebugClientInfo(
        std::move(fuchsia::sysmem2::AllocatorSetDebugClientInfoRequest{}
                      .set_name(fsl::GetCurrentProcessName())
                      .set_id(fsl::GetCurrentProcessKoid())));

    // Exactly one of these must be set.
    EXPECT_TRUE(!!args.has_buffer_collection_token2() ^ !!args.has_buffer_collection_token());
    fuchsia::sysmem2::BufferCollectionTokenHandle token;
    if (args.has_buffer_collection_token2()) {
      token = std::move(*args.mutable_buffer_collection_token2());
    } else {
      token = fuchsia::sysmem2::BufferCollectionTokenHandle(
          args.mutable_buffer_collection_token()->TakeChannel());
    }

    fuchsia::sysmem2::BufferCollectionSyncPtr buffer_collection;
    status = sysmem_allocator->BindSharedCollection(
        std::move(fuchsia::sysmem2::AllocatorBindSharedCollectionRequest{}
                      .set_token(std::move(token))
                      .set_buffer_collection_request(buffer_collection.NewRequest())));
    EXPECT_EQ(status, ZX_OK);

    status = buffer_collection->SetConstraints(
        fuchsia::sysmem2::BufferCollectionSetConstraintsRequest{});
    EXPECT_EQ(status, ZX_OK);

    status = buffer_collection->Release();
    EXPECT_EQ(status, ZX_OK);

    callback(fuchsia::ui::composition::Allocator_RegisterBufferCollection_Result::WithResponse({}));
  }

  // |fuchsia::ui::composition::testing::Flatland|
  void SetDebugName(std::string debug_name) override {
    // Do nothing.
  }

  // |fuchsia::ui::composition::testing::Flatland|
  void CreateView(fuchsia::ui::views::ViewCreationToken token,
                  fidl::InterfaceRequest<fuchsia::ui::composition::ParentViewportWatcher>
                      parent_viewport_watcher) override {
    // Do nothing.
  }

  // |fuchsia::ui::composition::testing::Flatland|
  void CreateView2(fuchsia::ui::views::ViewCreationToken token,
                   fuchsia::ui::views::ViewIdentityOnCreation view_identity,
                   fuchsia::ui::composition::ViewBoundProtocols view_protocols,
                   fidl::InterfaceRequest<fuchsia::ui::composition::ParentViewportWatcher>
                       parent_viewport_watcher) override {
    // Do nothing.
  }

  // |fuchsia::ui::composition::testing::Flatland|
  void CreateImage(fuchsia::ui::composition::ContentId image_id,
                   fuchsia::ui::composition::BufferCollectionImportToken import_token,
                   uint32_t vmo_index,
                   fuchsia::ui::composition::ImageProperties properties) override {
    auto [import_token_koid, _] = fsl::GetKoids(import_token.value.get());
    EXPECT_TRUE(registered_koids.find(import_token_koid) != registered_koids.end());

    EXPECT_TRUE(registered_images.find(image_id.value) == registered_images.end());
    registered_images.insert(image_id.value);
  }

  // |fuchsia::ui::composition::testing::Flatland|
  void ReleaseImage(fuchsia::ui::composition::ContentId image_id) override {
    EXPECT_TRUE(registered_images.find(image_id.value) != registered_images.end());
    registered_images.erase(image_id.value);
  }

  // |fuchsia::ui::composition::testing::Flatland|
  void CreateTransform(fuchsia::ui::composition::TransformId transform_id) override {
    EXPECT_EQ(kRootTransform.value, transform_id.value);
  }

  // |fuchsia::ui::composition::testing::Flatland|
  void SetRootTransform(fuchsia::ui::composition::TransformId transform_id) override {
    EXPECT_EQ(kRootTransform.value, transform_id.value);
  }

  // |fuchsia::ui::composition::testing::Flatland|
  void SetImageDestinationSize(fuchsia::ui::composition::ContentId image_id,
                               fuchsia::math::SizeU size) override {
    EXPECT_TRUE(registered_images.find(image_id.value) != registered_images.end());
  }

  // |fuchsia::ui::composition::testing::Flatland|
  void SetContent(fuchsia::ui::composition::TransformId transform_id,
                  fuchsia::ui::composition::ContentId content_id) override {
    EXPECT_EQ(kRootTransform.value, transform_id.value);
    EXPECT_TRUE(registered_images.find(content_id.value) != registered_images.end());
    image_on_root_transform_ = content_id.value;
  }

  static bool squashable(const fuchsia::ui::composition::PresentArgs& args) {
    return !args.has_unsquashable() || !args.unsquashable();
  }

  // |fuchsia::ui::composition::testing::Flatland|
  void Present(fuchsia::ui::composition::PresentArgs args) override {
    std::unique_lock<std::mutex> lock(mutex_);

    ASSERT_EQ(args.acquire_fences().size(), 1U);
    acquire_fences_.insert(ZirconIdFromHandle(args.acquire_fences()[0].get()));

    zx_signals_t pending;
    zx_status_t status = args.acquire_fences()[0].wait_one(
        ZX_EVENT_SIGNALED, zx::deadline_after(zx::sec(10)), &pending);

    if (status != ZX_OK || !should_present_) {
      presented_.push_back({image_on_root_transform_, status});
      return;
    }

    pending_present_count_++;
    if (!squashable(args) || pending_present_count_ == 1) {
      static zx::time last_present_time;
      zx::time current_time = zx::clock::get_monotonic();
      zx::duration time_since_last_present = current_time - last_present_time;
      zx::duration time_to_next_vsync = zx::usec(16666) - time_since_last_present;
      if (time_to_next_vsync < zx::duration(0)) {
        time_to_next_vsync = zx::duration(0);
      }
      last_present_time = current_time + time_to_next_vsync;

      zx_status_t status = async::PostDelayedTask(
          loop_.dispatcher(),
          [this, args = std::move(args)]() mutable {
            PresentImpl(image_on_root_transform_, std::move(args));
          },
          time_to_next_vsync);
      // Loop may be shutting down.
      EXPECT_TRUE(status == ZX_OK || status == ZX_ERR_BAD_STATE);
    } else {
      dropped_frame_count_ += 1;
      if (!args.release_fences().empty()) {
        args.release_fences()[0].signal(0, ZX_EVENT_SIGNALED);
      }
    }
  }

  void PresentImpl(uint64_t image_id, fuchsia::ui::composition::PresentArgs args) {
    std::unique_lock<std::mutex> lock(mutex_);

    if (!args.release_fences().empty()) {
      args.release_fences()[0].signal(0, ZX_EVENT_SIGNALED);
    }

    bool first_time = presented_.empty();

    // When the client is sending squashable presents assume they're trying to maximize fps,
    // so allow lots of present credits.
    uint32_t present_credits = (first_time && squashable(args)) ? 10 : pending_present_count_;

    // Run OnNextFrameBegin callback.
    fuchsia::ui::composition::OnNextFrameBeginValues values;
    values.set_additional_present_credits(present_credits);
    flatland_binding_.events().OnNextFrameBegin(std::move(values));

    pending_present_count_ = 0;
    presented_.push_back({image_id, ZX_OK});

    // Run OnFramePresented callback.
    fuchsia::scenic::scheduling::FramePresentedInfo frame_presented_info;
    fuchsia::scenic::scheduling::PresentReceivedInfo received_info;
    frame_presented_info.presentation_infos.emplace_back(std::move(received_info));
    flatland_binding_.events().OnFramePresented(std::move(frame_presented_info));
  }

  uint32_t presented_count() {
    std::unique_lock<std::mutex> lock(mutex_);
    return static_cast<uint32_t>(presented_.size());
  }

  uint32_t acquire_fences_count() {
    std::unique_lock<std::mutex> lock(mutex_);
    return static_cast<uint32_t>(acquire_fences_.size());
  }

  int dropped_frame_count() { return dropped_frame_count_.load(); }

  struct Presented {
    uint64_t image_id;
    zx_status_t acquire_wait_status;
  };

 private:
  async::Loop loop_;
  fidl::Binding<fuchsia::ui::composition::Allocator> allocator_binding_;
  fidl::Binding<fuchsia::ui::composition::Flatland> flatland_binding_;

  const fuchsia::ui::composition::TransformId kRootTransform = {1};
  std::set<zx_koid_t> registered_koids;
  std::set<uint64_t> registered_images;
  uint64_t image_on_root_transform_ = 0;
  const bool should_present_;
  std::mutex mutex_;
  std::vector<Presented> presented_;
  std::set<uint64_t> acquire_fences_;
  int pending_present_count_ = 0;
  std::atomic_int dropped_frame_count_ = 0;
};

void GetFakeFlatlandInjectedToLib(std::unique_ptr<FakeFlatland>* flatland, bool should_present) {
  // Instantiate a FakeFlatland. Pass it InterfaceRequests for the
  // Allocator and Flatland protocols, and inject the client ends of
  // these channels into static variables in VkLayer_image_pipe_swapchain.so,
  // so that they can be called by Vulkan.
  zx::channel local_allocator_endpoint, remote_allocator_endpoint;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &local_allocator_endpoint, &remote_allocator_endpoint));
  zx::channel local_flatland_endpoint, remote_flatland_endpoint;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &local_flatland_endpoint, &remote_flatland_endpoint));

  // Inject it to vulkan swapchain lib.
  void* libvulkan = dlopen("VkLayer_image_pipe_swapchain.so", RTLD_NOW | RTLD_LOCAL);
  ASSERT_NE(libvulkan, nullptr);
  typedef bool (*imagepipe_initialize_service_channel_fn)(zx::channel, zx::channel);
  auto fn = reinterpret_cast<imagepipe_initialize_service_channel_fn>(
      dlsym(libvulkan, "imagepipe_initialize_service_channel"));
  ASSERT_NE(fn, nullptr);
  ASSERT_TRUE(fn(std::move(remote_allocator_endpoint), std::move(remote_flatland_endpoint)));

  *flatland =
      std::make_unique<FakeFlatland>(fidl::InterfaceRequest<fuchsia::ui::composition::Allocator>(
                                         std::move(local_allocator_endpoint)),
                                     fidl::InterfaceRequest<fuchsia::ui::composition::Flatland>(
                                         std::move(local_flatland_endpoint)),
                                     should_present);
}

}  // namespace
