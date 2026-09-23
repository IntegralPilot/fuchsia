// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/gpu/magma/cpp/fidl.h>
#include <fuchsia/media/cpp/fidl_test_base.h>
#include <fuchsia/mediacodec/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/io.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>

#include <filesystem>

#include "src/lib/fxl/command_line.h"
#include "src/lib/fxl/log_settings_command_line.h"

namespace {

const char* kMimeType = "video/h264";

std::optional<std::string> FindGpuDevice(const std::string dir_name) {
  std::optional<std::string> device;

  // Directory_iterator skips dot and dot-dot so we don't have to.
  for (auto const& entry : std::filesystem::directory_iterator(dir_name)) {
    if (entry.is_regular_file()) {
      device = entry.path();
      break;
    }
  }

  return device;
}
}  // namespace

class StreamProcessorImpl final : public fuchsia::media::testing::StreamProcessor_TestBase {
  void NotImplemented_(const std::string& name) override {
    fprintf(stderr, "Streamprocessor doing notimplemented with %s\n", name.c_str());
  }
};

class CodecFactoryImpl final : public fuchsia::mediacodec::CodecFactory {
 public:
  void Bind(std::unique_ptr<CodecFactoryImpl> factory,
            fidl::InterfaceRequest<fuchsia::mediacodec::CodecFactory> request) {
    binding_.Bind(std::move(request));
    binding_.set_error_handler([factory = std::move(factory)](zx_status_t status) {});
  }

  void GetDetailedCodecDescriptions(GetDetailedCodecDescriptionsCallback callback) override {
    std::vector<fuchsia::mediacodec::DetailedCodecDescription> descriptions;
    {
      fuchsia::mediacodec::DetailedCodecDescription description;
      description.set_codec_type(fuchsia::mediacodec::CodecType::DECODER);
      description.set_mime_type(kMimeType);
      description.set_is_hw(false);

      fuchsia::mediacodec::DecoderProfileDescription profile;
      profile.set_profile(fuchsia::media::CodecProfile::H264PROFILE_HIGH);
      profile.set_min_image_size({16, 16});
      profile.set_max_image_size({3840, 2160});
      fuchsia::mediacodec::ProfileDescriptions profile_descriptions;
      std::vector<fuchsia::mediacodec::DecoderProfileDescription> profiles;
      profiles.emplace_back(std::move(profile));
      profile_descriptions.set_decoder_profile_descriptions(std::move(profiles));
      description.set_profile_descriptions(std::move(profile_descriptions));

      descriptions.push_back(std::move(description));
    }
    {
      fuchsia::mediacodec::DetailedCodecDescription description;
      description.set_codec_type(fuchsia::mediacodec::CodecType::ENCODER);
      description.set_mime_type(kMimeType);
      description.set_is_hw(false);

      fuchsia::mediacodec::EncoderProfileDescription profile;
      profile.set_profile(fuchsia::media::CodecProfile::H264PROFILE_HIGH);
      fuchsia::mediacodec::ProfileDescriptions profile_descriptions;
      std::vector<fuchsia::mediacodec::EncoderProfileDescription> profiles;
      profiles.emplace_back(std::move(profile));
      profile_descriptions.set_encoder_profile_descriptions(std::move(profiles));
      description.set_profile_descriptions(std::move(profile_descriptions));

      descriptions.push_back(std::move(description));
    }
    fuchsia::mediacodec::CodecFactoryGetDetailedCodecDescriptionsResponse response;
    response.set_codecs(std::move(descriptions));
    callback(std::move(response));
  }

  void CreateDecoder(fuchsia::mediacodec::CreateDecoder_Params params,
                     fidl::InterfaceRequest<fuchsia::media::StreamProcessor> decoder) override {
    StreamProcessorImpl impl;

    fidl::Binding<fuchsia::media::StreamProcessor> processor(&impl);
    processor.Bind(std::move(decoder));
    processor.events().OnInputConstraints(fuchsia::media::StreamBufferConstraints());
  }

  void CreateEncoder(
      fuchsia::mediacodec::CreateEncoder_Params encoder_params,
      fidl::InterfaceRequest<fuchsia::media::StreamProcessor> encoder_request) override {
    StreamProcessorImpl impl;

    fidl::Binding<fuchsia::media::StreamProcessor> processor(&impl);
    processor.Bind(std::move(encoder_request));
    processor.events().OnInputConstraints(fuchsia::media::StreamBufferConstraints());
  }

  void AttachLifetimeTracking(zx::eventpair codec_end) override {}

  // All CodecFactory servers implement this way, so for consistency we implement the same way here.
  void handle_unknown_method(uint64_t ordinal, bool method_has_response) override {
    // See src/media/codec/factory/codec_factory_impl.cc handle_unknown_method for why we handle
    // this way.
    if (!method_has_response) {
      // As a client author, if you see this message, please read the comments
      // in src/media/codec/factory/codec_factory_impl.cc handle_unknown_method.
      FX_LOGS(WARNING) << "Unrecognized one-way message - ordinal: " << std::hex << ordinal;
    } else {
      FX_LOGS(INFO) << "Unrecognized two-way message - ordinal: " << std::hex << ordinal;
    }
  }

 private:
  fidl::Binding<fuchsia::mediacodec::CodecFactory> binding_{this};
};

int main(int argc, const char* const* argv) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  auto command_line = fxl::CommandLineFromArgcArgv(argc, argv);
  fxl::SetLogSettingsFromCommandLine(command_line);
  auto context = sys::ComponentContext::CreateAndServeOutgoingDirectory();

  // Validate that /dev/class/gpu is accessible.
  std::optional<std::string> device_name = FindGpuDevice("/dev/class/gpu");
  if (!device_name) {
    fprintf(stderr, "No GPU devices found\n");
    return -1;
  }

  fuchsia::gpu::magma::IcdLoaderDeviceSyncPtr device;
  fdio_service_connect(device_name->c_str(), device.NewRequest().TakeChannel().release());
  std::vector<fuchsia::gpu::magma::IcdInfo> list;
  zx_status_t status = device->GetIcdList(&list);
  if (status != ZX_OK) {
    fprintf(stderr, "Failed to call GetIcdList\n");
    return -1;
  }
  if (list.size() != 1) {
    fprintf(stderr, "Incorrect ICD list size %lu\n", list.size());
    return -1;
  }

  context->outgoing()->AddPublicService(
      fidl::InterfaceRequestHandler<fuchsia::mediacodec::CodecFactory>(
          [](fidl::InterfaceRequest<fuchsia::mediacodec::CodecFactory> request) {
            auto factory = std::make_unique<CodecFactoryImpl>();
            factory->Bind(std::move(factory), std::move(request));
          }));
  loop.Run();
  return 0;
}
