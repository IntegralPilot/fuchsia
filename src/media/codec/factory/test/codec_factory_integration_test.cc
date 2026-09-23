// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/gpu/magma/cpp/fidl_test_base.h>
#include <fuchsia/hardware/mediacodec/cpp/fidl.h>
#include <fuchsia/media/cpp/fidl.h>
#include <fuchsia/media/cpp/fidl_test_base.h>
#include <fuchsia/mediacodec/cpp/fidl.h>
#include <fuchsia/sysinfo/cpp/fidl_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/fit/defer.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/vfs/cpp/pseudo_dir.h>
#include <lib/vfs/cpp/remote_dir.h>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/real_loop_fixture.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/service.h"
#include "src/storage/lib/vfs/cpp/synchronous_vfs.h"

// NOLINTNEXTLINE
using namespace component_testing;

class FakeSysInfoDevice : public fuchsia::sysinfo::testing::SysInfo_TestBase {
 public:
  void NotImplemented_(const std::string& name) override {
    fprintf(stderr, "FakeSysInfoDevice doing notimplemented with %s\n", name.c_str());
  }
  void GetBoardName(GetBoardNameCallback callback) override { callback(ZX_OK, "FakeBoard"); }
  fidl::InterfaceRequestHandler<fuchsia::sysinfo::SysInfo> GetHandler() {
    return bindings_.GetHandler(this);
  }

 private:
  fidl::BindingSet<fuchsia::sysinfo::SysInfo> bindings_;
};

class MockSysInfoComponent : public LocalComponentImpl {
 public:
  void OnStart() override { outgoing()->AddPublicService(sysinfo_device_.GetHandler()); }

 private:
  FakeSysInfoDevice sysinfo_device_;
  std::unique_ptr<LocalComponentHandles> handles_;
};

class FakeMagmaDevice : public fuchsia::gpu::magma::testing::CombinedDevice_TestBase {
 public:
  void NotImplemented_(const std::string& name) override {
    fprintf(stderr, "Magma doing notimplemented with %s\n", name.c_str());
  }

  void GetIcdList(GetIcdListCallback callback) override {
    std::vector<fuchsia::gpu::magma::IcdInfo> vec;
    if (has_icds_) {
      fuchsia::gpu::magma::IcdInfo info;
      info.set_component_url("#meta/fake_codec_factory.cm");
      info.set_flags(fuchsia::gpu::magma::IcdFlags::SUPPORTS_MEDIA_CODEC_FACTORY);
      vec.push_back(std::move(info));
    }
    callback(std::move(vec));
  }

  fidl::InterfaceRequestHandler<fuchsia::gpu::magma::CombinedDevice> GetHandler() {
    return bindings_.GetHandler(this);
  }

  void CloseAll() { bindings_.CloseAll(); }

  void set_has_icds(bool has_icds) { has_icds_ = has_icds; }

 private:
  fidl::BindingSet<fuchsia::gpu::magma::CombinedDevice> bindings_;
  bool has_icds_ = true;
};

class FakeStreamProcessor : public fuchsia::media::testing::StreamProcessor_TestBase {
 public:
  void Bind(fidl::InterfaceRequest<fuchsia::media::StreamProcessor> request) {
    if (binding_.is_bound()) {
      binding_.Unbind();
    }
    binding_.Bind(std::move(request));
    binding_.events().OnInputConstraints(fuchsia::media::StreamBufferConstraints());
  }

  void NotImplemented_(const std::string& name) override {}

 private:
  fidl::Binding<fuchsia::media::StreamProcessor> binding_{this};
};

class FakeCodecFactory : public fuchsia::mediacodec::CodecFactory {
 public:
  void Bind(fidl::InterfaceRequest<fuchsia::mediacodec::CodecFactory> request) {
    bindings_.AddBinding(this, std::move(request));
  }

  void GetDetailedCodecDescriptions(
      fuchsia::mediacodec::CodecFactory::GetDetailedCodecDescriptionsCallback callback) override {
    std::vector<fuchsia::mediacodec::DetailedCodecDescription> descriptions;
    {
      fuchsia::mediacodec::DetailedCodecDescription description;
      description.set_codec_type(fuchsia::mediacodec::CodecType::DECODER);
      description.set_mime_type("video/hevc");
      description.set_is_hw(false);

      fuchsia::mediacodec::DecoderProfileDescription profile;
      profile.set_profile(fuchsia::media::CodecProfile::HEVCPROFILE_MAIN);
      profile.set_min_image_size({16, 16});
      profile.set_max_image_size({3840, 2160});
      fuchsia::mediacodec::ProfileDescriptions profile_descriptions;
      std::vector<fuchsia::mediacodec::DecoderProfileDescription> profiles;
      profiles.emplace_back(std::move(profile));
      profile_descriptions.set_decoder_profile_descriptions(std::move(profiles));
      description.set_profile_descriptions(std::move(profile_descriptions));

      descriptions.push_back(std::move(description));
    }
    fuchsia::mediacodec::CodecFactoryGetDetailedCodecDescriptionsResponse response;
    response.set_codecs(std::move(descriptions));
    callback(std::move(response));
  }

  void CreateDecoder(fuchsia::mediacodec::CreateDecoder_Params params,
                     fidl::InterfaceRequest<fuchsia::media::StreamProcessor> decoder) override {
    stream_processor_.Bind(std::move(decoder));
  }

  void CreateEncoder(fuchsia::mediacodec::CreateEncoder_Params params,
                     fidl::InterfaceRequest<fuchsia::media::StreamProcessor> encoder) override {}

  void AttachLifetimeTracking(zx::eventpair codec_end) override {}

  void handle_unknown_method(uint64_t ordinal, bool method_has_response) override {}

 private:
  fidl::BindingSet<fuchsia::mediacodec::CodecFactory> bindings_;
  FakeStreamProcessor stream_processor_;
};

class FakeMediaCodecDevice : public fuchsia::hardware::mediacodec::Device {
 public:
  void GetCodecFactory(zx::channel request) override {
    codec_factory_.Bind(
        fidl::InterfaceRequest<fuchsia::mediacodec::CodecFactory>(std::move(request)));
  }

  void SetAuxServiceDirectory(
      fidl::InterfaceHandle<fuchsia::io::Directory> service_directory) override {}

  void handle_unknown_method(uint64_t ordinal, bool method_has_response) override {}

  fidl::InterfaceRequestHandler<fuchsia::hardware::mediacodec::Device> GetHandler() {
    return bindings_.GetHandler(this);
  }

  void CloseAll() { bindings_.CloseAll(); }

 private:
  fidl::BindingSet<fuchsia::hardware::mediacodec::Device> bindings_;
  FakeCodecFactory codec_factory_;
};

class MockGpuComponent : public LocalComponentImpl {
 public:
  explicit MockGpuComponent(async_dispatcher_t* dispatcher, FakeMagmaDevice& magma_device,
                            FakeMediaCodecDevice& mediacodec_device)
      : magma_device_(magma_device),
        mediacodec_device_(mediacodec_device),
        gpu_vfs_(dispatcher),
        mediacodec_vfs_(dispatcher) {}

  void OnStart() override {
    // Use fs:: versions because they support device watcher.
    {
      fidl::InterfaceHandle<fuchsia::io::Directory> io_dir;
      auto gpu_root = fbl::MakeRefCounted<fs::PseudoDir>();
      EXPECT_EQ(ZX_OK, gpu_vfs_.ServeDirectory(gpu_root, fidl::ServerEnd<fuchsia_io::Directory>(
                                                             io_dir.NewRequest().TakeChannel())));
      gpu_root->AddEntry(
          "000", fbl::MakeRefCounted<fs::Service>([this](zx::channel channel) {
            magma_device_.GetHandler()(
                fidl::InterfaceRequest<fuchsia::gpu::magma::CombinedDevice>(std::move(channel)));
            return ZX_OK;
          }));

      EXPECT_EQ(ZX_OK, outgoing()->root_dir()->AddEntry(
                           "dev-gpu", std::make_unique<vfs::RemoteDir>(io_dir.TakeChannel())));
    }

    {
      fidl::InterfaceHandle<fuchsia::io::Directory> io_dir;
      auto gpu_root = fbl::MakeRefCounted<fs::PseudoDir>();
      EXPECT_EQ(ZX_OK,
                mediacodec_vfs_.ServeDirectory(gpu_root, fidl::ServerEnd<fuchsia_io::Directory>(
                                                             io_dir.NewRequest().TakeChannel())));

      EXPECT_EQ(ZX_OK,
                outgoing()->root_dir()->AddEntry(
                    "dev-mediacodec", std::make_unique<vfs::RemoteDir>(io_dir.TakeChannel())));
    }

    {
      sys::ServiceHandler magma_service_handler;
      fuchsia::gpu::magma::Service::Handler magma_handler(&magma_service_handler);
      zx_status_t status = magma_handler.add_device(magma_device_.GetHandler());
      EXPECT_EQ(ZX_OK, status);
      status =
          outgoing()->AddService<fuchsia::gpu::magma::Service>(std::move(magma_service_handler));
      EXPECT_EQ(ZX_OK, status);
    }

    {
      sys::ServiceHandler mediacodec_service_handler;
      fuchsia::hardware::mediacodec::Service::Handler mediacodec_handler(
          &mediacodec_service_handler);
      zx_status_t status = mediacodec_handler.add_device(mediacodec_device_.GetHandler());
      EXPECT_EQ(ZX_OK, status);
      status = outgoing()->AddService<fuchsia::hardware::mediacodec::Service>(
          std::move(mediacodec_service_handler));
      EXPECT_EQ(ZX_OK, status);
    }
  }

 private:
  FakeMagmaDevice& magma_device_;
  FakeMediaCodecDevice& mediacodec_device_;
  fs::SynchronousVfs gpu_vfs_;
  fs::SynchronousVfs mediacodec_vfs_;
};

constexpr auto kCodecFactoryName = "codec_factory";
constexpr auto kMockGpuName = "mock_gpu";
constexpr auto kSysInfoName = "mock_sys_info";

class Integration : public gtest::RealLoopFixture {
 protected:
  Integration() = default;

  void InitializeRoutes(RealmBuilder& builder, bool route_magma = true,
                        bool route_mediacodec = true) {
    builder.AddChild(kCodecFactoryName, "#meta/codec_factory.cm");
    builder.AddRoute(Route{
        .capabilities = {Protocol{"fuchsia.logger.LogSink"}, Dictionary{"diagnostics"}},
        .source = ParentRef(),
        .targets = {ChildRef{kCodecFactoryName}},
    });
    builder.AddRoute(Route{
        .capabilities = {Protocol{"fuchsia.mediacodec.CodecFactory"}},
        .source = ChildRef{kCodecFactoryName},
        .targets = {ParentRef()},
    });
    builder.AddLocalChild(kMockGpuName,
                          [d = dispatcher(), &m = magma_device_, &mc = mediacodec_device_] {
                            return std::make_unique<MockGpuComponent>(d, m, mc);
                          });
    builder.AddLocalChild(kSysInfoName, [] { return std::make_unique<MockSysInfoComponent>(); });
    builder.AddRoute(Route{
        .capabilities = {Protocol{"fuchsia.sysinfo.SysInfo"}},
        .source = ChildRef{kSysInfoName},
        .targets = {ChildRef{kCodecFactoryName}},
    });

    builder.AddRoute(Route{
        .capabilities =
            {
                Directory{
                    .name = "dev-gpu",
                    .rights = fuchsia::io::R_STAR_DIR,
                    .path = "/dev-gpu",
                },
                Directory{
                    .name = "dev-mediacodec",
                    .rights = fuchsia::io::R_STAR_DIR,
                    .path = "/dev-mediacodec",
                },
            },
        .source = ChildRef{kMockGpuName},
        .targets = {ChildRef{kCodecFactoryName}},
    });

    std::vector<Capability> services;
    if (route_magma) {
      services.push_back(component_testing::Service{"fuchsia.gpu.magma.Service"});
    }
    if (route_mediacodec) {
      services.push_back(component_testing::Service{"fuchsia.hardware.mediacodec.Service"});
    }
    if (!services.empty()) {
      builder.AddRoute(Route{
          .capabilities = std::move(services),
          .source = ChildRef{kMockGpuName},
          .targets = {ChildRef{kCodecFactoryName}},
      });
    }
  }

  FakeMagmaDevice magma_device_;
  FakeMediaCodecDevice mediacodec_device_;
};

TEST_F(Integration, MagmaDevice) {
  auto builder = RealmBuilder::Create();
  InitializeRoutes(builder);
  auto realm = builder.Build(dispatcher());
  auto cleanup = fit::defer([&]() {
    bool complete = false;
    realm.Teardown([&](fit::result<fuchsia::component::Error> result) { complete = true; });
    RunLoopUntil([&]() { return complete; });
  });
  auto factory = realm.component().Connect<fuchsia::mediacodec::CodecFactory>();

  factory.set_error_handler([&](zx_status_t status) { FAIL() << zx_status_get_string(status); });

  fuchsia::mediacodec::CreateDecoder_Params params;
  fuchsia::media::FormatDetails input_details;
  input_details.set_mime_type("video/h264");
  params.set_input_details(std::move(input_details));
  params.set_require_hw(true);
  fuchsia::media::StreamProcessorPtr processor;
  factory->CreateDecoder(std::move(params), processor.NewRequest());
  processor.set_error_handler([&](zx_status_t status) { FAIL() << zx_status_get_string(status); });

  bool on_input_constraints_called = false;
  processor.events().OnInputConstraints = [&](fuchsia::media::StreamBufferConstraints constraints) {
    on_input_constraints_called = true;
    processor.Unbind();
  };

  RunLoopUntil([&]() { return on_input_constraints_called || HasFailure(); });

  magma_device_.CloseAll();

  // Eventually codecs from the device should disappear.
  while (true) {
    fuchsia::mediacodec::CreateDecoder_Params params;
    fuchsia::media::FormatDetails input_details;
    input_details.set_mime_type("video/h264");
    params.set_input_details(std::move(input_details));
    params.set_require_hw(true);
    fuchsia::media::StreamProcessorPtr processor;
    factory->CreateDecoder(std::move(params), processor.NewRequest());

    bool processor_failed = false;
    processor.set_error_handler([&](zx_status_t status) { processor_failed = true; });

    bool on_input_constraints_called = false;
    processor.events().OnInputConstraints =
        [&](fuchsia::media::StreamBufferConstraints constraints) {
          on_input_constraints_called = true;
          processor.Unbind();
        };
    RunLoopUntil([&]() { return processor_failed || on_input_constraints_called; });
    if (processor_failed) {
      break;
    }
    // Ignore this success and try again.
  }
}

// If the Magma Device doesn't list any ICDs, creating a hardware codec should fail but not hang.
TEST_F(Integration, MagmaDeviceNoIcd) {
  auto builder = RealmBuilder::Create();
  InitializeRoutes(builder);
  magma_device_.set_has_icds(false);

  auto realm = builder.Build(dispatcher());
  auto cleanup = fit::defer([&]() {
    bool complete = false;
    realm.Teardown([&](fit::result<fuchsia::component::Error> result) { complete = true; });
    RunLoopUntil([&]() { return complete; });
  });
  auto factory = realm.component().Connect<fuchsia::mediacodec::CodecFactory>();

  factory.set_error_handler([&](zx_status_t status) { FAIL() << zx_status_get_string(status); });

  fuchsia::mediacodec::CreateDecoder_Params params;
  fuchsia::media::FormatDetails input_details;
  input_details.set_mime_type("video/h264");
  params.set_input_details(std::move(input_details));
  params.set_require_hw(true);
  fuchsia::media::StreamProcessorPtr processor;
  factory->CreateDecoder(std::move(params), processor.NewRequest());
  bool processor_failed = false;
  processor.set_error_handler([&](zx_status_t status) {
    // This should error out.
    processor_failed = true;
  });

  processor.events().OnInputConstraints = [&](fuchsia::media::StreamBufferConstraints constraints) {
    if (constraints.has_buffer_constraints_version_ordinal()) {
      FAIL() << constraints.buffer_constraints_version_ordinal();
    }
    FAIL() << "fuchsia::media::StreamBufferConstraints{}";
  };

  RunLoopUntil([&]() { return processor_failed || HasFailure(); });
}

TEST_F(Integration, MagmaEncoder) {
  auto builder = RealmBuilder::Create();
  InitializeRoutes(builder);
  auto realm = builder.Build(dispatcher());
  auto cleanup = fit::defer([&]() {
    bool complete = false;
    realm.Teardown([&](fit::result<fuchsia::component::Error> result) { complete = true; });
    RunLoopUntil([&]() { return complete; });
  });
  auto factory = realm.component().Connect<fuchsia::mediacodec::CodecFactory>();

  factory.set_error_handler([&](zx_status_t status) { FAIL() << zx_status_get_string(status); });

  fuchsia::mediacodec::CreateEncoder_Params params;
  fuchsia::media::FormatDetails input_details;
  input_details.set_mime_type("video/h264");
  fuchsia::media::EncoderSettings encoder_settings;
  encoder_settings.set_h264({});
  input_details.set_encoder_settings(std::move(encoder_settings));
  params.set_input_details(std::move(input_details));
  params.set_require_hw(true);
  fuchsia::media::StreamProcessorPtr processor;
  factory->CreateEncoder(std::move(params), processor.NewRequest());
  processor.set_error_handler([&](zx_status_t status) { FAIL() << zx_status_get_string(status); });

  bool on_input_constraints_called = false;
  processor.events().OnInputConstraints = [&](fuchsia::media::StreamBufferConstraints constraints) {
    on_input_constraints_called = true;
    processor.Unbind();
  };

  RunLoopUntil([&]() { return on_input_constraints_called || HasFailure(); });

  magma_device_.CloseAll();

  // Eventually codecs from the device should disappear.
  while (true) {
    fuchsia::mediacodec::CreateEncoder_Params params;
    fuchsia::media::FormatDetails input_details;
    input_details.set_mime_type("video/h264");
    fuchsia::media::EncoderSettings encoder_settings;
    encoder_settings.set_h264({});
    input_details.set_encoder_settings(std::move(encoder_settings));
    params.set_input_details(std::move(input_details));
    params.set_require_hw(true);
    fuchsia::media::StreamProcessorPtr processor;
    factory->CreateEncoder(std::move(params), processor.NewRequest());

    bool processor_failed = false;
    processor.set_error_handler([&](zx_status_t status) { processor_failed = true; });

    bool on_input_constraints_called = false;
    processor.events().OnInputConstraints =
        [&](fuchsia::media::StreamBufferConstraints constraints) {
          on_input_constraints_called = true;
          processor.Unbind();
        };
    RunLoopUntil([&]() { return processor_failed || on_input_constraints_called; });
    if (processor_failed) {
      break;
    }
    // Ignore this success and try again.
  }
}

TEST_F(Integration, NoMediaCodecService) {
  auto builder = RealmBuilder::Create();
  InitializeRoutes(builder, /*route_magma=*/true, /*route_mediacodec=*/false);
  auto realm = builder.Build(dispatcher());
  auto cleanup = fit::defer([&]() {
    bool complete = false;
    realm.Teardown([&](fit::result<fuchsia::component::Error> result) { complete = true; });
    RunLoopUntil([&]() { return complete; });
  });
  auto factory = realm.component().Connect<fuchsia::mediacodec::CodecFactory>();

  factory.set_error_handler([&](zx_status_t status) { FAIL() << zx_status_get_string(status); });

  bool got_description = false;
  factory->GetDetailedCodecDescriptions(
      [&](fuchsia::mediacodec::CodecFactoryGetDetailedCodecDescriptionsResponse response) {
        got_description = true;
      });

  RunLoopUntil([&]() { return got_description || HasFailure(); });
}

TEST_F(Integration, NoMagmaCodecService) {
  auto builder = RealmBuilder::Create();
  InitializeRoutes(builder, /*route_magma=*/false, /*route_mediacodec=*/true);
  auto realm = builder.Build(dispatcher());
  auto cleanup = fit::defer([&]() {
    bool complete = false;
    realm.Teardown([&](fit::result<fuchsia::component::Error> result) { complete = true; });
    RunLoopUntil([&]() { return complete; });
  });
  auto factory = realm.component().Connect<fuchsia::mediacodec::CodecFactory>();

  factory.set_error_handler([&](zx_status_t status) { FAIL() << zx_status_get_string(status); });

  bool got_description = false;
  factory->GetDetailedCodecDescriptions(
      [&](fuchsia::mediacodec::CodecFactoryGetDetailedCodecDescriptionsResponse response) {
        got_description = true;
      });

  RunLoopUntil([&]() { return got_description || HasFailure(); });
}

TEST_F(Integration, NoMagmaOrMediaCodecService) {
  auto builder = RealmBuilder::Create();
  InitializeRoutes(builder, /*route_magma=*/false, /*route_mediacodec=*/false);
  auto realm = builder.Build(dispatcher());
  auto cleanup = fit::defer([&]() {
    bool complete = false;
    realm.Teardown([&](fit::result<fuchsia::component::Error> result) { complete = true; });
    RunLoopUntil([&]() { return complete; });
  });
  auto factory = realm.component().Connect<fuchsia::mediacodec::CodecFactory>();

  factory.set_error_handler([&](zx_status_t status) { FAIL() << zx_status_get_string(status); });

  bool got_description = false;
  factory->GetDetailedCodecDescriptions(
      [&](fuchsia::mediacodec::CodecFactoryGetDetailedCodecDescriptionsResponse response) {
        got_description = true;
      });

  RunLoopUntil([&]() { return got_description || HasFailure(); });
}

TEST_F(Integration, MediaCodecDevice) {
  auto builder = RealmBuilder::Create();
  InitializeRoutes(builder);
  auto realm = builder.Build(dispatcher());
  auto cleanup = fit::defer([&]() {
    bool complete = false;
    realm.Teardown([&](fit::result<fuchsia::component::Error> result) { complete = true; });
    RunLoopUntil([&]() { return complete; });
  });
  auto factory = realm.component().Connect<fuchsia::mediacodec::CodecFactory>();

  factory.set_error_handler([&](zx_status_t status) { FAIL() << zx_status_get_string(status); });

  bool got_hevc = false;
  for (int attempt = 0; attempt < 50; ++attempt) {
    bool got_description = false;
    factory->GetDetailedCodecDescriptions(
        [&](fuchsia::mediacodec::CodecFactoryGetDetailedCodecDescriptionsResponse response) {
          got_description = true;
          for (const auto& codec : response.codecs()) {
            if (codec.mime_type() == "video/hevc") {
              got_hevc = true;
            }
          }
        });
    RunLoopUntil([&]() { return got_description || HasFailure(); });
    if (HasFailure()) {
      break;
    }
    if (got_hevc) {
      break;
    }
    zx::nanosleep(zx::deadline_after(zx::msec(50)));
  }

  EXPECT_TRUE(got_hevc);

  fuchsia::mediacodec::CreateDecoder_Params params;
  fuchsia::media::FormatDetails input_details;
  input_details.set_mime_type("video/hevc");
  params.set_input_details(std::move(input_details));
  params.set_require_hw(true);
  fuchsia::media::StreamProcessorPtr processor;
  factory->CreateDecoder(std::move(params), processor.NewRequest());
  processor.set_error_handler([&](zx_status_t status) { FAIL() << zx_status_get_string(status); });

  bool on_input_constraints_called = false;
  processor.events().OnInputConstraints = [&](fuchsia::media::StreamBufferConstraints constraints) {
    on_input_constraints_called = true;
    processor.Unbind();
  };

  RunLoopUntil([&]() { return on_input_constraints_called || HasFailure(); });
}
