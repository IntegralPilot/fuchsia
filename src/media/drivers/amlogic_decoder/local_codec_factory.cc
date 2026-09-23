// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "local_codec_factory.h"

#include <lib/fidl/cpp/clone.h>
#include <lib/fit/function.h>
#include <lib/media/codec_impl/codec_admission_control.h>

#include <optional>

#include "codec_adapter_h264_multi.h"
#include "codec_adapter_mpeg2.h"
#include "codec_adapter_vp9.h"
#include "device_ctx.h"

namespace amlogic_decoder {

namespace {

// Mirrors |fuchisa.mediacodec.DetailedCodecDescription| and
// |fuchsia.mediacodec.DecoderProfileDescription|. Used so that we can construct values statically.
struct DecoderDescription {
  struct ProfileDescription {
    fuchsia::media::CodecProfile profile;
    fuchsia::math::SizeU min_coded_size;
    fuchsia::math::SizeU max_coded_size;
    bool allow_encryption;
    bool require_encryption;
    bool allow_input_protection;
    bool require_input_protection;
  };

  std::string mime_type;
  bool is_hw;
  std::vector<ProfileDescription> profiles;
};

// List of decoder descriptions supported by this local codec factory.
const DecoderDescription kDecoderDescriptions[] = {
    {.mime_type = "video/h264",
     .is_hw = true,
     .profiles = {{
         .profile = fuchsia::media::CodecProfile::H264PROFILE_MAIN,
         .min_coded_size = {.width = 16u, .height = 16u},
         .max_coded_size = {.width = 4096u, .height = 4096u},
         // The decoder itself does not yet support decryption. Rather the decoder supports
         // receiving input in protected memory.
         .allow_encryption = false,
         .require_encryption = false,
         .allow_input_protection = true,
         .require_input_protection = false,
     }}},
    {.mime_type = "video/h264",
     .is_hw = true,
     .profiles = {{
         .profile = fuchsia::media::CodecProfile::H264PROFILE_HIGH,
         .min_coded_size = {.width = 16u, .height = 16u},
         .max_coded_size = {.width = 4096u, .height = 4096u},
         // The decoder itself does not yet support decryption. Rather the decoder supports
         // receiving input in protected memory.
         .allow_encryption = false,
         .require_encryption = false,
         .allow_input_protection = true,
         .require_input_protection = false,
     }}},
    {.mime_type = "video/vp9",
     .is_hw = true,
     .profiles = {{
         .profile = fuchsia::media::CodecProfile::VP9PROFILE_PROFILE0,
         .min_coded_size = {.width = 2u, .height = 2u},
         .max_coded_size = {.width = 4096u, .height = 4096u},
         // The decoder itself does not yet support decryption. Rather the decoder supports
         // receiving input in protected memory.
         .allow_encryption = false,
         .require_encryption = false,
         .allow_input_protection = true,
         .require_input_protection = false,
     }}},
};

struct CodecAdapterFactory {
  bool is_enabled;
  fuchsia::mediacodec::CodecDescription description;

  // This typedef is just for local readability here, not for use outside this
  // struct.
  using CreateFunction = fit::function<std::unique_ptr<CodecAdapter>(
      std::mutex& lock, CodecAdapterEvents*, DeviceCtx*)>;

  bool multi_instance;

  CreateFunction create;
};

// TODO(dustingreen): Fix up this list to correspond to what
// CodecImpl+AmlogicVideo can actually handle so far, once there's at least one
// format in that list.  For now this list is here to allow covering some
// LocalCodecFactory code.
const CodecAdapterFactory kCodecFactories[] = {
    {
        true,  // is_enabled
        fuchsia::mediacodec::CodecDescription{
            .codec_type = fuchsia::mediacodec::CodecType::DECODER,
            // TODO(dustingreen): See TODO comments on this field in
            // codec_common.fidl.
            .mime_type = "video/h264",

            // TODO(https://fxbug.dev/42084549): Make more of these true by implementing in
            // CodecAdapterH264Multi.
            .can_stream_bytes_input = true,
            .can_find_start = false,
            .can_re_sync = false,
            .will_report_all_detected_errors = false,

            .is_hw = true,

            // TODO(dustingreen): Determine if this claim of "true" is actually
            // the case.
            .split_header_handling = true,
        },
        true,  // multi_instance
        [](std::mutex& lock, CodecAdapterEvents* events, DeviceCtx* device) {
          return std::make_unique<CodecAdapterH264Multi>(lock, events, device);
        },
    },
    {
        false,  // is_enabled
        fuchsia::mediacodec::CodecDescription{
            .codec_type = fuchsia::mediacodec::CodecType::DECODER,
            // TODO(dustingreen): See TODO comments on this field in
            // codec_common.fidl.
            .mime_type = "video/mpeg2",

            // TODO(dustingreen): Determine which of these can safely indicate
            // more capability.
            .can_stream_bytes_input = false,
            .can_find_start = false,
            .can_re_sync = false,
            .will_report_all_detected_errors = false,

            .is_hw = true,

            // TODO(dustingreen): Determine if this claim of "true" is actually
            // the case.
            .split_header_handling = true,
        },
        false,  // multi_instance
        [](std::mutex& lock, CodecAdapterEvents* events, DeviceCtx* device) {
          return std::make_unique<CodecAdapterMpeg2>(lock, events, device);
        },
    },
    {
        true,  // is_enabled
        fuchsia::mediacodec::CodecDescription{
            .codec_type = fuchsia::mediacodec::CodecType::DECODER,
            // TODO(dustingreen): See TODO comments on this field in
            // codec_common.fidl.
            .mime_type = "video/vp9",

            // TODO(dustingreen): Determine which of these can safely indicate
            // more capability.
            .can_stream_bytes_input = false,
            .can_find_start = false,
            .can_re_sync = false,
            .will_report_all_detected_errors = false,

            .is_hw = true,

            // TODO(dustingreen): Determine if this claim of "true" is actually
            // the case.
            .split_header_handling = true,
        },
        true,  // multi_instance
        [](std::mutex& lock, CodecAdapterEvents* events, DeviceCtx* device) {
          return std::make_unique<CodecAdapterVp9>(lock, events, device);
        },
    },
    {
        true,  // is_enabled
        fuchsia::mediacodec::CodecDescription{
            .codec_type = fuchsia::mediacodec::CodecType::DECODER,
            // TODO(dustingreen): See TODO comments on this field in
            // codec_common.fidl.
            .mime_type = "video/h264-multi/test/force-context-save-restore",

            // TODO(dustingreen): Determine which of these can safely indicate
            // more capability.
            .can_stream_bytes_input = false,
            .can_find_start = false,
            .can_re_sync = false,
            .will_report_all_detected_errors = false,

            .is_hw = true,

            // TODO(dustingreen): Determine if this claim of "true" is actually
            // the case.
            .split_header_handling = true,
        },
        true,  // multi_instance
        [](std::mutex& lock, CodecAdapterEvents* events, DeviceCtx* device) {
          auto decoder = std::make_unique<CodecAdapterH264Multi>(lock, events, device);
          AmlogicDecoderTestHooks test_hooks = {
              .force_context_save_restore = true,
          };
          decoder->set_test_hooks(test_hooks);
          return decoder;
        },
    },
    {
        true,  // is_enabled
        fuchsia::mediacodec::CodecDescription{
            .codec_type = fuchsia::mediacodec::CodecType::DECODER,
            // TODO(dustingreen): See TODO comments on this field in
            // codec_common.fidl.
            .mime_type = "video/vp9/test/force-context-save-restore",

            // TODO(dustingreen): Determine which of these can safely indicate
            // more capability.
            .can_stream_bytes_input = false,
            .can_find_start = false,
            .can_re_sync = false,
            .will_report_all_detected_errors = false,

            .is_hw = true,

            // TODO(dustingreen): Determine if this claim of "true" is actually
            // the case.
            .split_header_handling = true,
        },
        true,  // multi_instance
        [](std::mutex& lock, CodecAdapterEvents* events, DeviceCtx* device) {
          auto decoder = std::make_unique<CodecAdapterVp9>(lock, events, device);
          AmlogicDecoderTestHooks test_hooks = {
              .force_context_save_restore = true,
          };
          decoder->set_test_hooks(test_hooks);
          return decoder;
        },
    },
};

}  // namespace

// device - associated device.
LocalCodecFactory::LocalCodecFactory(DeviceCtx* device) : device_(device), factory_binding_(this) {
  // nothing else to do here
}

LocalCodecFactory::~LocalCodecFactory() {
  // We need ~factory_binding_ to run on shared_fidl_thread() else it's not safe
  // to un-bind unilaterally (without the channel closing).  Unless not bound in
  // the first place.
  ZX_DEBUG_ASSERT(thrd_current() == device_->driver()->shared_fidl_thread() ||
                  !factory_binding_.is_bound());

  // ~factory_binding_ here + fact that we're running on shared_fidl_thread()
  // (if Bind() previously called) means error_handler won't be running
  // concurrently with ~LocalCodecFactory and won't run after ~factory_binding_
  // here.
}

void LocalCodecFactory::SetErrorHandler(fit::closure error_handler) {
  ZX_DEBUG_ASSERT(!factory_binding_.is_bound());
  factory_binding_.set_error_handler(
      [this, error_handler = std::move(error_handler)](zx_status_t status) mutable {
        ZX_DEBUG_ASSERT(thrd_current() == device_->driver()->shared_fidl_thread());
        // This queues after the similar posting in CreateDecoder() (via
        // TryAddCodec()), so that LocalCodecFactory won't get deleted until
        // after previously-started TryAddCodec()s are done.
        device_->codec_admission_control()->PostAfterPreviouslyStartedClosesDone(
            [this, error_handler = std::move(error_handler)] {
              ZX_DEBUG_ASSERT(thrd_current() == device_->driver()->shared_fidl_thread());
              error_handler();
              // "this" is gone
            });
      });
  is_error_handler_set_ = true;
}

void LocalCodecFactory::Bind(zx::channel server_endpoint) {
  ZX_DEBUG_ASSERT(is_error_handler_set_);
  ZX_DEBUG_ASSERT(!factory_binding_.is_bound());

  // Go!  (immediately - if Bind() is called on IOCTL thread, this can result in
  // _immediate_ dispatching over on shared_fidl_thread()).
  factory_binding_.Bind(std::move(server_endpoint),
                        device_->driver()->shared_fidl_loop()->dispatcher());
}

void LocalCodecFactory::GetDetailedCodecDescriptions(
    GetDetailedCodecDescriptionsCallback callback) {
  std::vector<fuchsia::mediacodec::DetailedCodecDescription> fidl_codec_descriptions;

  for (const auto& decoder_description : kDecoderDescriptions) {
    fuchsia::mediacodec::DetailedCodecDescription fidl_codec_description;
    fidl_codec_description.set_codec_type(::fuchsia::mediacodec::CodecType::DECODER);
    fidl_codec_description.set_mime_type(decoder_description.mime_type);
    fidl_codec_description.set_is_hw(decoder_description.is_hw);

    std::vector<fuchsia::mediacodec::DecoderProfileDescription> fidl_profile_description_vector;
    for (const auto& decoder_profile : decoder_description.profiles) {
      fuchsia::mediacodec::DecoderProfileDescription profile_description;
      profile_description.set_profile(decoder_profile.profile);
      profile_description.set_min_image_size(decoder_profile.min_coded_size);
      profile_description.set_max_image_size(decoder_profile.max_coded_size);
      profile_description.set_allow_encryption(decoder_profile.allow_encryption);
      profile_description.set_require_encryption(decoder_profile.require_encryption);
      profile_description.set_allow_input_protection(decoder_profile.allow_input_protection);
      profile_description.set_require_input_protection(decoder_profile.require_input_protection);
      fidl_profile_description_vector.push_back(std::move(profile_description));
    }

    fuchsia::mediacodec::ProfileDescriptions fidl_profile_descriptions;
    fidl_profile_descriptions.set_decoder_profile_descriptions(
        std::move(fidl_profile_description_vector));
    fidl_codec_description.set_profile_descriptions(std::move(fidl_profile_descriptions));

    fidl_codec_descriptions.push_back(std::move(fidl_codec_description));
  }

  fuchsia::mediacodec::CodecFactoryGetDetailedCodecDescriptionsResponse fidl_response;
  fidl_response.set_codecs(std::move(fidl_codec_descriptions));
  callback(std::move(fidl_response));
}

void LocalCodecFactory::CreateDecoder(
    fuchsia::mediacodec::CreateDecoder_Params video_decoder_params,
    ::fidl::InterfaceRequest<fuchsia::media::StreamProcessor> video_decoder) {
  if (!video_decoder_params.has_input_details()) {
    return;
  }

  if (!video_decoder_params.input_details().has_mime_type()) {
    // Without mime_type we cannot search for a decoder.
    return;
  }

  const CodecAdapterFactory* factory = nullptr;
  for (const CodecAdapterFactory& candidate_factory : kCodecFactories) {
    if (!candidate_factory.is_enabled) {
      continue;
    }
    if (candidate_factory.description.mime_type ==
        video_decoder_params.input_details().mime_type()) {
      factory = &candidate_factory;
      break;
    }
  }
  if (!factory) {
    // This shouldn't really happen since the main CodecFactory shouldn't be
    // asking this LocalCodecFactory for a codec fitting a description that's
    // not a description this factory previously delivered to the main
    // CodecFactory via GetDetailedCodecDescriptions().
    //
    // TODO(dustingreen): epitaph for video_decoder.
    //
    // ~video_decoder here will take care of closing the channel
    return;
  }

  // We also post to the same queue in the set_error_handler() lambda, so that
  // we know the LocalCodecFactory will remain alive until after this lambda
  // completes.
  //
  // The factory pointer remains valid for whole lifetime of this devhost
  // process.
  device_->codec_admission_control()->TryAddCodec(
      factory->multi_instance, [this, video_decoder_params = std::move(video_decoder_params),
                                video_decoder = std::move(video_decoder),
                                factory](std::unique_ptr<CodecAdmission> codec_admission) mutable {
        if (!codec_admission) {
          // We can't create another Codec presently.
          //
          // ~video_decoder will take care of closing the channel.
          return;
        }

        zx::result allocator_client_result = device_->video()->ConnectToSysmem();
        if (allocator_client_result.is_error()) {
          return;
        }
        auto sysmem = std::move(*allocator_client_result);
        if (!sysmem) {
          return;
        }

        auto codec =
            std::make_unique<CodecImpl>(std::move(sysmem), std::move(codec_admission),
                                        device_->driver()->shared_fidl_loop()->dispatcher(),
                                        device_->driver()->shared_fidl_thread(),
                                        std::move(video_decoder_params), std::move(video_decoder));

        codec->SetLifetimeTracking(std::move(lifetime_tracking_));
        ZX_DEBUG_ASSERT(lifetime_tracking_.empty());

        codec->SetCodecMetrics(&device_->metrics());
        auto core_codec = factory->create(codec->lock(), codec.get(), device_);
        // Don't wait for CodecAdapter sub-class to call LogEvent().  Verify this is not
        // std::nullopt here, as any CodecAdapter sub-class in amlogic_video_decoder is allowed to
        // call LogEvent() and must override CoreCodecMetricsImplementation().
        ZX_DEBUG_ASSERT(core_codec->CoreCodecMetricsImplementation());
        codec->SetCoreCodecAdapter(std::move(core_codec));

        codec->SetCodecDiagnostics(&device_->diagnostics());

        device_->device_fidl()->BindCodecImpl(std::move(codec));
      });
}

void LocalCodecFactory::CreateEncoder(
    fuchsia::mediacodec::CreateEncoder_Params encoder_params,
    ::fidl::InterfaceRequest<fuchsia::media::StreamProcessor> encoder_request) {
  // We have no encoders to provide.
  lifetime_tracking_.clear();
  // ~encoder_request
}

void LocalCodecFactory::AttachLifetimeTracking(zx::eventpair codec_end) {
  ZX_DEBUG_ASSERT(lifetime_tracking_.size() <=
                  fuchsia::mediacodec::CODEC_FACTORY_LIFETIME_TRACKING_EVENTPAIR_PER_CREATE_MAX);
  if (lifetime_tracking_.size() >=
      fuchsia::mediacodec::CODEC_FACTORY_LIFETIME_TRACKING_EVENTPAIR_PER_CREATE_MAX) {
    factory_binding_.Close(ZX_ERR_BAD_STATE);
    device_->device_fidl()->DeleteFactory(this);
    return;
  }
  lifetime_tracking_.emplace_back(std::move(codec_end));
}

void LocalCodecFactory::handle_unknown_method(uint64_t ordinal, bool method_has_response) {
  // See src/media/codec/factory/codec_factory_impl.cc handle_unknown_method for why we handle
  // this way.
  if (!method_has_response) {
    // As a client author, if you see this message, please read the comments
    // in src/media/codec/factory/codec_factory_impl.cc handle_unknown_method.
    LOG(WARNING, "unrecognized one-way message - ordinal: %" PRIx64, ordinal);
  } else {
    LOG(INFO, "unrecognized two-way message - ordinal: %" PRIx64, ordinal);
  }
}

}  // namespace amlogic_decoder
