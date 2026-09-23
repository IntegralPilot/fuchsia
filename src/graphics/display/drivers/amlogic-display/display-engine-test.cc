// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/display-engine.h"

#include <fidl/fuchsia.component.runner/cpp/fidl.h>
#include <fidl/fuchsia.images2/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/inspect/cpp/inspect.h>

#include <atomic>
#include <memory>
#include <utility>

#include <fake-mmio-reg/fake-mmio-reg.h>
#include <gtest/gtest.h>

#include "src/graphics/display/drivers/amlogic-display/pixel-grid-size2d.h"
#include "src/graphics/display/drivers/amlogic-display/structured_config.h"
#include "src/graphics/display/drivers/amlogic-display/vout-dsi.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-fidl.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/graphics/display/lib/driver-utils/poll-until.h"
#include "src/lib/fsl/handles/object_info.h"
#include "src/lib/testing/predicates/status.h"

namespace amlogic_display {

namespace {

// TODO(https://fxbug.dev/42072949): Consider creating and using a unified set of sysmem
// testing doubles instead of writing fakes for each display driver test.
class FakeBufferCollectionBase
    : public fidl::testing::WireTestBase<fuchsia_sysmem2::BufferCollection> {
 public:
  FakeBufferCollectionBase() = default;
  ~FakeBufferCollectionBase() override = default;

  virtual void VerifyBufferCollectionConstraints(
      const fuchsia_sysmem2::wire::BufferCollectionConstraints& constraints) = 0;
  virtual void VerifyName(const std::string& name) = 0;

  void SetConstraints(SetConstraintsRequestView request,
                      SetConstraintsCompleter::Sync& completer) override {
    if (!request->has_constraints()) {
      return;
    }
    VerifyBufferCollectionConstraints(request->constraints());
    set_constraints_called_ = true;
  }

  void SetName(SetNameRequestView request, SetNameCompleter::Sync& completer) override {
    EXPECT_EQ(10u, request->priority());
    VerifyName(std::string(request->name().data(), request->name().size()));
    set_name_called_ = true;
  }

  void CheckAllBuffersAllocated(CheckAllBuffersAllocatedCompleter::Sync& completer) override {
    completer.Reply(fit::ok());
  }

  void WaitForAllBuffersAllocated(WaitForAllBuffersAllocatedCompleter::Sync& completer) override {
    zx::vmo vmo;
    EXPECT_OK(zx::vmo::create(zx_system_get_page_size(), 0u, &vmo));
    auto collection = fuchsia_sysmem2::wire::BufferCollectionInfo::Builder(arena_)
                          .buffers(std::vector{fuchsia_sysmem2::wire::VmoBuffer::Builder(arena_)
                                                   .vmo(std::move(vmo))
                                                   .vmo_usable_start(0)
                                                   .Build()})
                          .settings(fuchsia_sysmem2::wire::SingleBufferSettings::Builder(arena_)
                                        .image_format_constraints(image_format_constraints_)
                                        .Build())
                          .Build();
    auto response =
        fuchsia_sysmem2::wire::BufferCollectionWaitForAllBuffersAllocatedResponse::Builder(arena_)
            .buffer_collection_info(collection)
            .Build();
    completer.Reply(fit::ok(&response));
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    EXPECT_TRUE(false);
  }

  void set_allocated_image_format_constraints(
      const fuchsia_sysmem2::wire::ImageFormatConstraints& image_format_constraints) {
    image_format_constraints_ = image_format_constraints;
  }
  bool set_constraints_called() const { return set_constraints_called_; }
  bool set_name_called() const { return set_name_called_; }

 private:
  fidl::Arena<fidl::kDefaultArenaInitialCapacity> arena_;
  bool set_constraints_called_ = false;
  bool set_name_called_ = false;
  fuchsia_sysmem2::wire::ImageFormatConstraints image_format_constraints_;
};

class FakeBufferCollection : public FakeBufferCollectionBase {
 public:
  explicit FakeBufferCollection(
      const std::vector<fuchsia_images2::wire::PixelFormat>& pixel_format_types =
          {fuchsia_images2::wire::PixelFormat::kB8G8R8A8,
           fuchsia_images2::wire::PixelFormat::kR8G8B8A8})
      : supported_pixel_format_types_(pixel_format_types) {
    set_allocated_image_format_constraints(
        fuchsia_sysmem2::wire::ImageFormatConstraints::Builder(arena_)
            .pixel_format(fuchsia_images2::wire::PixelFormat::kB8G8R8A8)
            .pixel_format_modifier(fuchsia_images2::wire::PixelFormatModifier::kLinear)
            .min_size(fuchsia_math::wire::SizeU{.width = 1, .height = 4})
            .min_bytes_per_row(4096)
            .Build());
  }
  ~FakeBufferCollection() override = default;

  void VerifyBufferCollectionConstraints(
      const fuchsia_sysmem2::wire::BufferCollectionConstraints& constraints) override {
    EXPECT_TRUE(constraints.buffer_memory_constraints().inaccessible_domain_supported());
    EXPECT_FALSE(constraints.buffer_memory_constraints().cpu_domain_supported());
    EXPECT_EQ(64u, constraints.image_format_constraints().at(0).bytes_per_row_divisor());

    size_t expected_format_constraints_count = 0u;
    const std::span<const fuchsia_sysmem2::wire::ImageFormatConstraints> image_format_constraints(
        constraints.image_format_constraints().data(),
        constraints.image_format_constraints().size());

    const bool has_bgra =
        std::find(supported_pixel_format_types_.begin(), supported_pixel_format_types_.end(),
                  fuchsia_images2::wire::PixelFormat::kB8G8R8A8) !=
        supported_pixel_format_types_.end();
    if (has_bgra) {
      expected_format_constraints_count += 2;
      const bool image_constraints_contains_bgra32_and_linear = std::any_of(
          image_format_constraints.begin(), image_format_constraints.end(),
          [](const fuchsia_sysmem2::wire::ImageFormatConstraints& format) {
            return format.pixel_format() == fuchsia_images2::wire::PixelFormat::kB8G8R8A8 &&
                   format.pixel_format_modifier() ==
                       fuchsia_images2::wire::PixelFormatModifier::kLinear;
          });
      EXPECT_TRUE(image_constraints_contains_bgra32_and_linear);
    }

    const bool has_rgba =
        std::find(supported_pixel_format_types_.begin(), supported_pixel_format_types_.end(),
                  fuchsia_images2::wire::PixelFormat::kR8G8B8A8) !=
        supported_pixel_format_types_.end();
    if (has_rgba) {
      expected_format_constraints_count += 4;
      const bool image_constraints_contains_rgba32_and_linear = std::any_of(
          image_format_constraints.begin(), image_format_constraints.end(),
          [](const fuchsia_sysmem2::wire::ImageFormatConstraints& format) {
            return format.pixel_format() == fuchsia_images2::wire::PixelFormat::kR8G8B8A8 &&
                   format.pixel_format_modifier() ==
                       fuchsia_images2::wire::PixelFormatModifier::kLinear;
          });
      EXPECT_TRUE(image_constraints_contains_rgba32_and_linear);
      const bool image_constraints_contains_rgba32_and_afbc_16x16 = std::any_of(
          image_format_constraints.begin(), image_format_constraints.end(),
          [](const fuchsia_sysmem2::wire::ImageFormatConstraints& format) {
            return format.pixel_format() == fuchsia_images2::wire::PixelFormat::kR8G8B8A8 &&
                   format.pixel_format_modifier() ==
                       fuchsia_images2::wire::PixelFormatModifier::kArmAfbc16X16SplitBlockSparseYuv;
          });
      EXPECT_TRUE(image_constraints_contains_rgba32_and_afbc_16x16);
    }

    EXPECT_EQ(expected_format_constraints_count, constraints.image_format_constraints().size());
  }

  void VerifyName(const std::string& name) override { EXPECT_EQ(name, "Display"); }

 private:
  fidl::Arena<fidl::kDefaultArenaInitialCapacity> arena_;
  std::vector<fuchsia_images2::wire::PixelFormat> supported_pixel_format_types_;
};

class FakeBufferCollectionForCapture : public FakeBufferCollectionBase {
 public:
  FakeBufferCollectionForCapture() {
    set_allocated_image_format_constraints(
        fuchsia_sysmem2::wire::ImageFormatConstraints::Builder(arena_)
            .pixel_format(fuchsia_images2::wire::PixelFormat::kB8G8R8)
            .pixel_format_modifier(fuchsia_images2::wire::PixelFormatModifier::kLinear)
            .min_size(fuchsia_math::wire::SizeU{.width = 1, .height = 4})
            .min_bytes_per_row(4096)
            .Build());
  }
  ~FakeBufferCollectionForCapture() override = default;

  void VerifyBufferCollectionConstraints(
      const fuchsia_sysmem2::wire::BufferCollectionConstraints& constraints) override {
    EXPECT_TRUE(constraints.buffer_memory_constraints().inaccessible_domain_supported());
    EXPECT_FALSE(constraints.buffer_memory_constraints().cpu_domain_supported());
    EXPECT_EQ(64u, constraints.image_format_constraints().at(0).bytes_per_row_divisor());

    size_t expected_format_constraints_count = 1u;
    EXPECT_EQ(expected_format_constraints_count, constraints.image_format_constraints().size());

    const auto& image_format_constraints = constraints.image_format_constraints();
    EXPECT_EQ(image_format_constraints.at(0).pixel_format(),
              fuchsia_images2::wire::PixelFormat::kB8G8R8);
    EXPECT_TRUE(image_format_constraints.at(0).has_pixel_format_modifier());
    EXPECT_EQ(image_format_constraints.at(0).pixel_format_modifier(),
              fuchsia_images2::wire::PixelFormatModifier::kLinear);
  }

  void VerifyName(const std::string& name) override { EXPECT_EQ(name, "Display capture"); }

 private:
  fidl::Arena<fidl::kDefaultArenaInitialCapacity> arena_;
};

// This class is thread-unsafe. It must be created, used and destroyed in the
// `dispatcher` passed in the constructor.
class FakeAllocator : public fidl::testing::WireTestBase<fuchsia_sysmem2::Allocator> {
 public:
  using FakeBufferCollectionBuilder =
      fit::function<std::unique_ptr<FakeBufferCollectionBase>(void)>;

  FakeAllocator() = default;

  void Bind(async_dispatcher_t* dispatcher,
            fidl::ServerEnd<fuchsia_sysmem2::Allocator> server_end) {
    ZX_ASSERT(dispatcher != nullptr);
    dispatcher_ = dispatcher;
    binding_.emplace(dispatcher_, std::move(server_end), this, fidl::kIgnoreBindingClosure);
  }

  void set_fake_buffer_collection_builder(FakeBufferCollectionBuilder builder) {
    fake_buffer_collection_builder_ = std::move(builder);
  }

  void BindSharedCollection(BindSharedCollectionRequestView request,
                            BindSharedCollectionCompleter::Sync& completer) override {
    ZX_ASSERT(fake_buffer_collection_builder_ != nullptr);
    auto buffer_collection_id = next_buffer_collection_id_++;

    auto fake_buffer_collection = fake_buffer_collection_builder_();
    auto binding = std::make_unique<fidl::ServerBinding<fuchsia_sysmem2::BufferCollection>>(
        dispatcher_, std::move(request->buffer_collection_request()), fake_buffer_collection.get(),
        [this, buffer_collection_id](FakeBufferCollectionBase*, fidl::UnbindInfo) {
          inactive_buffer_collection_tokens_.push_back(
              std::move(active_buffer_collections_[buffer_collection_id].token_client));
          active_buffer_collections_.erase(buffer_collection_id);
        });

    active_buffer_collections_[buffer_collection_id] = {
        .token_client = std::move(request->token()),
        .fake_buffer_collection = std::move(fake_buffer_collection),
        .binding = std::move(binding),
    };
  }

  FakeBufferCollectionBase* GetMostRecentBufferCollection() {
    const display::DriverBufferCollectionId most_recent_collection_id(
        next_buffer_collection_id_.value() - 1);
    if (active_buffer_collections_.find(most_recent_collection_id) ==
        active_buffer_collections_.end()) {
      return nullptr;
    }
    return active_buffer_collections_.at(most_recent_collection_id).fake_buffer_collection.get();
  }

  std::vector<fidl::UnownedClientEnd<fuchsia_sysmem2::BufferCollectionToken>>
  GetActiveBufferCollectionTokenClients() const {
    std::vector<fidl::UnownedClientEnd<fuchsia_sysmem2::BufferCollectionToken>>
        unowned_token_clients;
    unowned_token_clients.reserve(active_buffer_collections_.size());

    for (const auto& kv : active_buffer_collections_) {
      unowned_token_clients.push_back(kv.second.token_client);
    }
    return unowned_token_clients;
  }

  std::vector<fidl::UnownedClientEnd<fuchsia_sysmem2::BufferCollectionToken>>
  GetInactiveBufferCollectionTokenClients() const {
    std::vector<fidl::UnownedClientEnd<fuchsia_sysmem2::BufferCollectionToken>>
        unowned_token_clients;
    unowned_token_clients.reserve(inactive_buffer_collection_tokens_.size());

    for (const auto& token : inactive_buffer_collection_tokens_) {
      unowned_token_clients.push_back(token);
    }
    return unowned_token_clients;
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    EXPECT_TRUE(false);
  }

 private:
  struct BufferCollection {
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token_client;
    std::unique_ptr<FakeBufferCollectionBase> fake_buffer_collection;
    std::unique_ptr<fidl::ServerBinding<fuchsia_sysmem2::BufferCollection>> binding;
  };

  std::unordered_map<display::DriverBufferCollectionId, BufferCollection>
      active_buffer_collections_;
  std::vector<fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>>
      inactive_buffer_collection_tokens_;

  display::DriverBufferCollectionId next_buffer_collection_id_ =
      display::DriverBufferCollectionId(0);
  FakeBufferCollectionBuilder fake_buffer_collection_builder_ = nullptr;

  async_dispatcher_t* dispatcher_ = nullptr;
  std::optional<fidl::ServerBinding<fuchsia_sysmem2::Allocator>> binding_;
};

// This class is thread-unsafe. It must be created, used and destroyed in the
// `dispatcher` passed in the constructor.
class FakeCanvas : public fidl::WireServer<fuchsia_hardware_amlogiccanvas::Device> {
 public:
  FakeCanvas() = default;

  void Bind(async_dispatcher_t* dispatcher,
            fidl::ServerEnd<fuchsia_hardware_amlogiccanvas::Device> server_end) {
    binding_.emplace(dispatcher, std::move(server_end), this, fidl::kIgnoreBindingClosure);
  }

  void Config(ConfigRequestView request, ConfigCompleter::Sync& completer) override {
    for (size_t i = 0; i < std::size(in_use_); i++) {
      ZX_DEBUG_ASSERT_MSG(i <= std::numeric_limits<uint8_t>::max(),
                          "Canvas index out of range: %zu", i);
      if (!in_use_[i]) {
        in_use_[i] = true;
        completer.ReplySuccess(static_cast<uint8_t>(i));
        return;
      }
    }
    completer.ReplyError(ZX_ERR_NO_MEMORY);
  }

  void Free(FreeRequestView request, FreeCompleter::Sync& completer) override {
    EXPECT_TRUE(in_use_[request->canvas_idx]);
    in_use_[request->canvas_idx] = false;
    completer.ReplySuccess();
  }

  void CheckThatNoEntriesInUse() {
    for (uint32_t i = 0; i < std::size(in_use_); i++) {
      EXPECT_FALSE(in_use_[i]);
    }
  }

 private:
  static constexpr uint32_t kCanvasEntries = 256;
  bool in_use_[kCanvasEntries] = {};
  std::optional<fidl::ServerBinding<fuchsia_hardware_amlogiccanvas::Device>> binding_;
};

class TestDriver : public fdf::DriverBase2 {
 public:
  TestDriver() : fdf::DriverBase2("test-driver") {}

  zx::result<> Start(fdf::DriverContext context) override {
    incoming_namespace_ = std::shared_ptr<fdf::Namespace>(context.take_incoming());
    return zx::ok();
  }

  const std::shared_ptr<fdf::Namespace>& incoming_namespace() const { return incoming_namespace_; }

  static DriverRegistration GetDriverRegistration() {
    return FUCHSIA_DRIVER_REGISTRATION_V1(fdf_internal::DriverServer2<TestDriver>::initialize,
                                          fdf_internal::DriverServer2<TestDriver>::destroy);
  }

 private:
  std::shared_ptr<fdf::Namespace> incoming_namespace_;
};

class AmlogicDisplayTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();

    // 1. Configure and serve FakePDev
    fdf_fake::FakePDev::Config config;
    config.use_fake_bti = true;
    fake_pdev_.SetConfig(std::move(config));

    auto pdev_handler = fake_pdev_.GetInstanceHandler(dispatcher);
    zx::result<> status = to_driver_vfs.AddService<fuchsia_hardware_platform_device::Service>(
        std::move(pdev_handler), "pdev");
    if (status.is_error()) {
      return status;
    }

    // 2. Serve FakeCanvas under service name "canvas" using Bind()
    fuchsia_hardware_amlogiccanvas::Service::InstanceHandler canvas_handler({
        .device =
            [this, dispatcher](fidl::ServerEnd<fuchsia_hardware_amlogiccanvas::Device> server_end) {
              canvas_.Bind(dispatcher, std::move(server_end));
            },
    });
    status = to_driver_vfs.AddService<fuchsia_hardware_amlogiccanvas::Service>(
        std::move(canvas_handler), "canvas");
    if (status.is_error()) {
      return status;
    }

    // 3. Serve FakeAllocator using Bind()
    status = to_driver_vfs.component().AddUnmanagedProtocol<fuchsia_sysmem2::Allocator>(
        [this, dispatcher](fidl::ServerEnd<fuchsia_sysmem2::Allocator> server_end) {
          allocator_.Bind(dispatcher, std::move(server_end));
        });
    if (status.is_error()) {
      return status;
    }

    return zx::ok();
  }

  FakeAllocator& allocator() { return allocator_; }
  FakeCanvas& canvas() { return canvas_; }

 private:
  fdf_fake::FakePDev fake_pdev_;
  FakeAllocator allocator_;
  FakeCanvas canvas_;
};

struct TestConfig {
  using DriverType = TestDriver;
  using EnvironmentType = AmlogicDisplayTestEnvironment;
};

class FakeSysmemTest : public testing::Test {
 public:
  static constexpr int kWidth = 1024;
  static constexpr int kHeight = 600;

  void SetUp() override {
    zx::result<> start_result = driver_test_.StartDriver();
    ASSERT_OK(start_result);

    display_engine_ = std::make_unique<DisplayEngine>(driver_test_.driver()->incoming_namespace(),
                                                      &engine_events_, structured_config::Config());
    ASSERT_EQ(display_engine_->GetCommonProtocolsAndResources(), ZX_OK);

    display_engine_->SetFormatSupportCheck([](auto) { return true; });

    zx::result<std::unique_ptr<VoutDsi>> create_dsi_vout_result =
        VoutDsi::CreateForTesting(display::PanelType::kBoeTv070wsmFitipowerJd9364Astro);
    ASSERT_OK(create_dsi_vout_result);
    display_engine_->SetVoutForTesting(std::move(create_dsi_vout_result).value());

    PixelGridSize2D layer_image_size = {
        .width = kWidth,
        .height = kHeight,
    };
    PixelGridSize2D display_contents_size = {
        .width = kWidth,
        .height = kHeight,
    };
    zx::result<std::unique_ptr<VideoInputUnit>> video_input_unit_result =
        VideoInputUnit::CreateForTesting(vpu_mmio_.GetMmioBuffer(), /*rdma=*/nullptr,
                                         layer_image_size, display_contents_size);
    ASSERT_OK(video_input_unit_result);
    display_engine_->SetVideoInputUnitForTesting(std::move(video_input_unit_result).value());

    SetBufferCollectionBuilder([] {
      // Allocate importable primary Image by default.
      const std::vector<fuchsia_images2::wire::PixelFormat> kPixelFormats = {
          fuchsia_images2::wire::PixelFormat::kB8G8R8A8,
          fuchsia_images2::wire::PixelFormat::kR8G8B8A8};
      return std::make_unique<FakeBufferCollection>(kPixelFormats);
    });
  }

  void TearDown() override {
    display_engine_.reset();
    zx::result<> stop_result = driver_test_.StopDriver();
    ASSERT_OK(stop_result);
  }

  fidl::ServerEnd<fuchsia_sysmem2::BufferCollectionToken> ImportBufferCollection(
      display::DriverBufferCollectionId id) {
    auto [token_client, token_server] =
        fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
    EXPECT_OK(display_engine_->ImportBufferCollection(id, std::move(token_client)));
    return std::move(token_server);
  }

  void SetBufferCollectionBuilder(FakeAllocator::FakeBufferCollectionBuilder builder) {
    driver_test_.RunInEnvironmentTypeContext(
        [builder = std::move(builder)](AmlogicDisplayTestEnvironment& env) mutable {
          env.allocator().set_fake_buffer_collection_builder(std::move(builder));
        });
  }

  void SetBufferCollectionBuilderForCapture() {
    SetBufferCollectionBuilder([] { return std::make_unique<FakeBufferCollectionForCapture>(); });
  }

  void PollUntilActiveTokensCount(size_t expected_count) {
    EXPECT_TRUE(display::PollUntil(
        [&]() {
          return driver_test_.RunInEnvironmentTypeContext<bool>(
              [expected_count](AmlogicDisplayTestEnvironment& env) {
                return env.allocator().GetActiveBufferCollectionTokenClients().size() ==
                       expected_count;
              });
        },
        zx::msec(5), 1000));
  }

  void VerifyTokenIsActive(const zx::channel& token_server_channel) {
    auto [server_koid, server_related_koid] = fsl::GetKoids(token_server_channel.get());
    driver_test_.RunInEnvironmentTypeContext([&](AmlogicDisplayTestEnvironment& env) {
      auto active_buffer_token_clients = env.allocator().GetActiveBufferCollectionTokenClients();
      ASSERT_EQ(active_buffer_token_clients.size(), 1u);

      auto [client_koid, client_related_koid] =
          fsl::GetKoids(active_buffer_token_clients[0].channel()->get());
      EXPECT_NE(client_koid, ZX_KOID_INVALID);
      EXPECT_NE(client_related_koid, ZX_KOID_INVALID);
      EXPECT_EQ(client_koid, server_related_koid);
      EXPECT_EQ(server_koid, client_related_koid);

      auto inactive_buffer_token_clients =
          env.allocator().GetInactiveBufferCollectionTokenClients();
      EXPECT_EQ(inactive_buffer_token_clients.size(), 0u);
    });
  }

  void VerifyTokenIsInactive(const zx::channel& token_server_channel) {
    auto [server_koid, server_related_koid] = fsl::GetKoids(token_server_channel.get());
    driver_test_.RunInEnvironmentTypeContext([&](AmlogicDisplayTestEnvironment& env) {
      auto active_buffer_token_clients = env.allocator().GetActiveBufferCollectionTokenClients();
      EXPECT_EQ(active_buffer_token_clients.size(), 0u);

      auto inactive_buffer_token_clients =
          env.allocator().GetInactiveBufferCollectionTokenClients();
      ASSERT_EQ(inactive_buffer_token_clients.size(), 1u);

      auto [client_koid, client_related_koid] =
          fsl::GetKoids(inactive_buffer_token_clients[0].channel()->get());
      EXPECT_NE(client_koid, ZX_KOID_INVALID);
      EXPECT_NE(client_related_koid, ZX_KOID_INVALID);
      EXPECT_EQ(client_koid, server_related_koid);
      EXPECT_EQ(server_koid, client_related_koid);
    });
  }

 protected:
  fdf_testing::ScopedGlobalLogger logger_;

  fdf_testing::ForegroundDriverTest<TestConfig> driver_test_;

  display::DisplayEngineEventsFidl engine_events_;

  ddk_fake::FakeMmioRegRegion vpu_mmio_ =
      ddk_fake::FakeMmioRegRegion(/*reg_size=*/4, /*reg_count=*/0x10000);

  std::unique_ptr<DisplayEngine> display_engine_;
};

TEST_F(FakeSysmemTest, ImportBufferCollection) {
  constexpr display::DriverBufferCollectionId kValidBufferCollectionId(1);
  fidl::ServerEnd<fuchsia_sysmem2::BufferCollectionToken> token1_server =
      ImportBufferCollection(kValidBufferCollectionId);

  // `driver_buffer_collection_id` must be unused.
  auto [token2_client, token2_server] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  EXPECT_STATUS(
      display_engine_->ImportBufferCollection(kValidBufferCollectionId, std::move(token2_client)),
      zx::error(ZX_ERR_ALREADY_EXISTS));

  PollUntilActiveTokensCount(1);

  // Verify that the current buffer collection token is used (active).
  VerifyTokenIsActive(token1_server.channel());

  // Test ReleaseBufferCollection().
  constexpr display::DriverBufferCollectionId kInvalidBufferCollectionId(2);
  EXPECT_STATUS(display_engine_->ReleaseBufferCollection(kInvalidBufferCollectionId),
                zx::error(ZX_ERR_NOT_FOUND));
  EXPECT_OK(display_engine_->ReleaseBufferCollection(kValidBufferCollectionId));

  PollUntilActiveTokensCount(0);

  // Verify that the current buffer collection token is released (inactive).
  VerifyTokenIsInactive(token1_server.channel());
}

TEST_F(FakeSysmemTest, ImportImage) {
  constexpr display::DriverBufferCollectionId kBufferCollectionId(1);
  fidl::ServerEnd<fuchsia_sysmem2::BufferCollectionToken> token_server =
      ImportBufferCollection(kBufferCollectionId);

  static constexpr display::ImageBufferUsage kDisplayUsage({
      .tiling_type = display::ImageTilingType::kLinear,
  });
  EXPECT_OK(display_engine_->SetBufferCollectionConstraints(kDisplayUsage, kBufferCollectionId));

  constexpr display::DriverBufferCollectionId kInvalidBufferCollectionId(100);
  EXPECT_STATUS(
      display_engine_->SetBufferCollectionConstraints(kDisplayUsage, kInvalidBufferCollectionId),
      zx::error(ZX_ERR_NOT_FOUND));

  // Invalid import: Bad image type.
  static constexpr display::ImageMetadata kInvalidTilingMetadata({
      .width = 1024,
      .height = 768,
      .tiling_type = display::ImageTilingType::kCapture,
  });
  EXPECT_STATUS(display_engine_->ImportImage(kInvalidTilingMetadata, kBufferCollectionId,
                                             /*buffer_index=*/0),
                zx::error(ZX_ERR_INVALID_ARGS));

  // Invalid import: Invalid collection ID.
  static constexpr display::ImageMetadata kDisplayImageMetadata({
      .width = 1024,
      .height = 768,
      .tiling_type = display::ImageTilingType::kLinear,
  });
  EXPECT_STATUS(display_engine_->ImportImage(kDisplayImageMetadata, kInvalidBufferCollectionId,
                                             /*buffer_index=*/0),
                zx::error(ZX_ERR_NOT_FOUND));

  // Invalid import: Invalid buffer collection index.
  constexpr uint64_t kInvalidBufferIndex = 100u;
  EXPECT_STATUS(
      display_engine_->ImportImage(kDisplayImageMetadata, kBufferCollectionId, kInvalidBufferIndex),
      zx::error(ZX_ERR_OUT_OF_RANGE));

  // Valid import.
  zx::result<display::DriverImageId> successful_import_result =
      display_engine_->ImportImage(kDisplayImageMetadata, kBufferCollectionId,
                                   /*buffer_index=*/0);
  ASSERT_TRUE(successful_import_result.is_ok());
  display::DriverImageId driver_image_id = std::move(successful_import_result).value();
  EXPECT_NE(driver_image_id, display::kInvalidDriverImageId);

  // Release the image.
  display_engine_->ReleaseImage(driver_image_id);

  EXPECT_OK(display_engine_->ReleaseBufferCollection(kBufferCollectionId));
}

TEST_F(FakeSysmemTest, ImportImageForCapture) {
  SetBufferCollectionBuilderForCapture();

  constexpr display::DriverBufferCollectionId kBufferCollectionId(1);
  fidl::ServerEnd<fuchsia_sysmem2::BufferCollectionToken> token_server =
      ImportBufferCollection(kBufferCollectionId);

  // Driver sets BufferCollection buffer memory constraints.
  static constexpr display::ImageBufferUsage kCaptureUsage({
      .tiling_type = display::ImageTilingType::kCapture,
  });
  EXPECT_OK(display_engine_->SetBufferCollectionConstraints(kCaptureUsage, kBufferCollectionId));

  // Invalid import: invalid buffer collection ID.
  const display::DriverBufferCollectionId kInvalidBufferCollectionId(100);
  EXPECT_STATUS(display_engine_->ImportImageForCapture(kInvalidBufferCollectionId,
                                                       /*buffer_index=*/0),
                zx::error(ZX_ERR_NOT_FOUND));

  // Invalid import: index out of range.
  const uint64_t kInvalidIndex = 100;
  EXPECT_STATUS(display_engine_->ImportImageForCapture(kBufferCollectionId, kInvalidIndex),
                zx::error(ZX_ERR_OUT_OF_RANGE));

  // Valid import.
  zx::result<display::DriverCaptureImageId> successful_import_result =
      display_engine_->ImportImageForCapture(kBufferCollectionId,
                                             /*buffer_index=*/0);
  ASSERT_OK(successful_import_result);
  display::DriverCaptureImageId capture_image_id = std::move(successful_import_result).value();
  EXPECT_NE(capture_image_id, display::kInvalidDriverCaptureImageId);

  // Release the image.
  EXPECT_OK(display_engine_->ReleaseCapture(capture_image_id));

  EXPECT_OK(display_engine_->ReleaseBufferCollection(kBufferCollectionId));
}

TEST_F(FakeSysmemTest, SysmemRequirements) {
  std::atomic<FakeBufferCollectionBase*> collection = nullptr;
  SetBufferCollectionBuilder([&collection] {
    const std::vector<fuchsia_images2::wire::PixelFormat> kPixelFormats = {
        fuchsia_images2::wire::PixelFormat::kB8G8R8A8,
        fuchsia_images2::wire::PixelFormat::kR8G8B8A8};
    auto new_buffer_collection = std::make_unique<FakeBufferCollection>(kPixelFormats);
    collection.store(new_buffer_collection.get());
    return new_buffer_collection;
  });

  constexpr display::DriverBufferCollectionId kBufferCollectionId(1);
  fidl::ServerEnd<fuchsia_sysmem2::BufferCollectionToken> token_server =
      ImportBufferCollection(kBufferCollectionId);

  EXPECT_TRUE(display::PollUntil([&] { return collection.load() != nullptr; }, zx::msec(5), 1000));

  static constexpr display::ImageBufferUsage kDisplayUsage({
      .tiling_type = display::ImageTilingType::kLinear,
  });
  EXPECT_OK(display_engine_->SetBufferCollectionConstraints(kDisplayUsage, kBufferCollectionId));

  EXPECT_TRUE(display::PollUntil(
      [&] {
        return driver_test_.RunInEnvironmentTypeContext<bool>(
            [&](AmlogicDisplayTestEnvironment& env) {
              FakeBufferCollectionBase* col = collection.load();
              return col != nullptr && col->set_constraints_called();
            });
      },
      zx::msec(5), 1000));

  EXPECT_TRUE(
      driver_test_.RunInEnvironmentTypeContext<bool>([&](AmlogicDisplayTestEnvironment& env) {
        FakeBufferCollectionBase* col = collection.load();
        return col != nullptr && col->set_name_called();
      }));
}

TEST_F(FakeSysmemTest, SysmemRequirements_BgraOnly) {
  std::atomic<FakeBufferCollectionBase*> collection = nullptr;
  SetBufferCollectionBuilder([&collection] {
    const std::vector<fuchsia_images2::wire::PixelFormat> kPixelFormats = {
        fuchsia_images2::wire::PixelFormat::kB8G8R8A8,
    };
    auto new_buffer_collection = std::make_unique<FakeBufferCollection>(kPixelFormats);
    collection.store(new_buffer_collection.get());
    return new_buffer_collection;
  });
  display_engine_->SetFormatSupportCheck(
      [](display::PixelFormat format) { return format == display::PixelFormat::kB8G8R8A8; });

  constexpr display::DriverBufferCollectionId kBufferCollectionId(1);
  fidl::ServerEnd<fuchsia_sysmem2::BufferCollectionToken> token_server =
      ImportBufferCollection(kBufferCollectionId);

  EXPECT_TRUE(display::PollUntil([&] { return collection.load() != nullptr; }, zx::msec(5), 1000));

  static constexpr display::ImageBufferUsage kDisplayUsage({
      .tiling_type = display::ImageTilingType::kLinear,
  });
  EXPECT_OK(display_engine_->SetBufferCollectionConstraints(kDisplayUsage, kBufferCollectionId));

  EXPECT_TRUE(display::PollUntil(
      [&] {
        return driver_test_.RunInEnvironmentTypeContext<bool>(
            [&](AmlogicDisplayTestEnvironment& env) {
              FakeBufferCollectionBase* col = collection.load();
              return col != nullptr && col->set_constraints_called();
            });
      },
      zx::msec(5), 1000));

  EXPECT_TRUE(
      driver_test_.RunInEnvironmentTypeContext<bool>([&](AmlogicDisplayTestEnvironment& env) {
        FakeBufferCollectionBase* col = collection.load();
        return col != nullptr && col->set_name_called();
      }));
}

TEST(AmlogicDisplay, FloatToFix3_10) {
  EXPECT_EQ(0x0000u, VideoInputUnit::FloatToFixed3_10(0.0f));
  EXPECT_EQ(0x0066u, VideoInputUnit::FloatToFixed3_10(0.1f));
  EXPECT_EQ(0x1f9au, VideoInputUnit::FloatToFixed3_10(-0.1f));
  // Test for maximum positive (<4)
  EXPECT_EQ(0x0FFFu, VideoInputUnit::FloatToFixed3_10(4.0f));
  EXPECT_EQ(0x0FFFu, VideoInputUnit::FloatToFixed3_10(40.0f));
  EXPECT_EQ(0x0FFFu, VideoInputUnit::FloatToFixed3_10(3.9999f));
  // Test for minimum negative (>= -4)
  EXPECT_EQ(0x1000u, VideoInputUnit::FloatToFixed3_10(-4.0f));
  EXPECT_EQ(0x1000u, VideoInputUnit::FloatToFixed3_10(-14.0f));
}

TEST(AmlogicDisplay, FloatToFixed2_10) {
  EXPECT_EQ(0x0000u, VideoInputUnit::FloatToFixed2_10(0.0f));
  EXPECT_EQ(0x0066u, VideoInputUnit::FloatToFixed2_10(0.1f));
  EXPECT_EQ(0x0f9au, VideoInputUnit::FloatToFixed2_10(-0.1f));
  // Test for maximum positive (<2)
  EXPECT_EQ(0x07FFu, VideoInputUnit::FloatToFixed2_10(2.0f));
  EXPECT_EQ(0x07FFu, VideoInputUnit::FloatToFixed2_10(20.0f));
  EXPECT_EQ(0x07FFu, VideoInputUnit::FloatToFixed2_10(1.9999f));
  // Test for minimum negative (>= -2)
  EXPECT_EQ(0x0800u, VideoInputUnit::FloatToFixed2_10(-2.0f));
  EXPECT_EQ(0x0800u, VideoInputUnit::FloatToFixed2_10(-14.0f));
}

TEST_F(FakeSysmemTest, NoLeakCaptureCanvas) {
  SetBufferCollectionBuilderForCapture();

  constexpr display::DriverBufferCollectionId kBufferCollectionId(1);
  fidl::ServerEnd<fuchsia_sysmem2::BufferCollectionToken> token_server =
      ImportBufferCollection(kBufferCollectionId);

  zx::result<display::DriverCaptureImageId> successful_import_result =
      display_engine_->ImportImageForCapture(kBufferCollectionId,
                                             /*buffer_index=*/0);
  ASSERT_OK(successful_import_result);
  display::DriverCaptureImageId capture_image_id = std::move(successful_import_result).value();
  EXPECT_OK(display_engine_->ReleaseCapture(capture_image_id));

  driver_test_.RunInEnvironmentTypeContext(
      [](AmlogicDisplayTestEnvironment& env) { env.canvas().CheckThatNoEntriesInUse(); });
}

}  // namespace

}  // namespace amlogic_display
