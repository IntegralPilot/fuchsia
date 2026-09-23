// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "vaapi_utils.h"

#include <fuchsia/media/cpp/fidl.h>
#include <lib/fdio/directory.h>
#include <lib/zx/channel.h>
#include <zircon/assert.h>

#include <filesystem>
#include <type_traits>
#include <utility>

#include <fbl/no_destructor.h>
#include <safemath/safe_conversions.h>
#include <va/va_magma.h>

#include "chromium_utils.h"
namespace {
std::unique_ptr<VADisplayWrapper> display_wrapper;
}

static void libva_error_callback(void* user_context, const char* message) {
  FX_LOG_KV(ERROR, "libva error", FX_KV("error_message", message));
}

static void libva_info_callback(void* user_context, const char* message) {
  FX_LOG_KV(INFO, "libva message", FX_KV("message", message));
}

// static
bool VADisplayWrapper::InitializeSingleton(uint64_t required_vendor_id) {
  ZX_ASSERT(!display_wrapper);

  auto new_display_wrapper = std::make_unique<VADisplayWrapper>();

  for (auto& p : std::filesystem::directory_iterator("/dev/class/gpu")) {
    {
      zx::channel local, remote;
      zx_status_t zx_status = zx::channel::create(0 /*flags*/, &local, &remote);
      ZX_ASSERT(zx_status == ZX_OK);
      zx_status = fdio_service_connect(p.path().c_str(), remote.release());
      ZX_ASSERT(zx_status == ZX_OK);
      magma_status_t status =
          magma_device_import(local.release(), &new_display_wrapper->magma_device_);
      ZX_ASSERT(status == MAGMA_STATUS_OK);
      if (status != MAGMA_STATUS_OK)
        continue;
    }
    {
      uint64_t vendor_id;
      magma_status_t magma_status = magma_device_query(new_display_wrapper->magma_device_,
                                                       MAGMA_QUERY_VENDOR_ID, nullptr, &vendor_id);
      if (magma_status == MAGMA_STATUS_OK && vendor_id == required_vendor_id) {
        break;
      }
    }

    magma_device_release(new_display_wrapper->magma_device_);
    new_display_wrapper->magma_device_ = {};
  }

  if (!new_display_wrapper->magma_device_)
    return false;

  if (!new_display_wrapper->Initialize()) {
    magma_device_release(new_display_wrapper->magma_device_);
    return false;
  }
  display_wrapper = std::move(new_display_wrapper);
  return true;
}

// static
bool VADisplayWrapper::InitializeSingletonForTesting() {
  auto new_display_wrapper = std::make_unique<VADisplayWrapper>();
  if (!new_display_wrapper->Initialize())
    return false;
  display_wrapper = std::move(new_display_wrapper);
  return true;
}

// static
bool VADisplayWrapper::DestroySingleton() {
  if (display_wrapper->Destroy()) {
    magma_device_release(display_wrapper->magma_device_);
    display_wrapper.reset();
    return true;
  }

  return false;
}

bool VADisplayWrapper::Initialize() {
  display_ = vaGetDisplayMagma(magma_device_);
  if (!display_)
    return false;

  vaSetErrorCallback(display_, libva_error_callback, nullptr);
  vaSetInfoCallback(display_, libva_info_callback, nullptr);

  int major_ver, minor_ver;
  VAStatus va_status = vaInitialize(display_, &major_ver, &minor_ver);
  if (va_status != VA_STATUS_SUCCESS) {
    return false;
  }
  return true;
}

bool VADisplayWrapper::Destroy() {
  VAStatus va_status = vaTerminate(display_);

  return (va_status == VA_STATUS_SUCCESS);
}

// static
VADisplayWrapper* VADisplayWrapper::GetSingleton() { return display_wrapper.get(); }

VASurface::VASurface(VASurfaceID va_surface_id, const gfx::Size& size, unsigned int format,
                     ReleaseCB release_cb)
    : va_surface_id_(va_surface_id),
      size_(size),
      format_(format),
      release_cb_(std::move(release_cb)) {
  DCHECK(release_cb_);
}

VASurface::~VASurface() { std::move(release_cb_)(va_surface_id_); }

static bool SupportsEntrypointForProfile(VAProfile profile, VAEntrypoint required_entrypoint) {
  VADisplay display = VADisplayWrapper::GetSingleton()->display();
  size_t max_entrypoints = safemath::checked_cast<size_t>(vaMaxNumEntrypoints(display));

  int num_entrypoints = 0;
  std::vector<VAEntrypoint> entrypoints(max_entrypoints);

  VAStatus status =
      vaQueryConfigEntrypoints(display, profile, entrypoints.data(), &num_entrypoints);
  if (status != VA_STATUS_SUCCESS) {
    FX_LOG_KV(ERROR, "vaQueryConfigEntrypoints failed", FX_KV("error_str", vaErrorStr(status)));
    return false;
  }

  int vld_entrypoint;
  for (vld_entrypoint = 0; vld_entrypoint < num_entrypoints; vld_entrypoint++) {
    if (entrypoints[vld_entrypoint] == required_entrypoint) {
      return true;
    }
  }

  return false;
}

static bool SupportsAttribsForProfile(VAProfile profile, VAEntrypoint required_entrypoint,
                                      const std::vector<VAConfigAttrib>& required_attribs) {
  VADisplay display = VADisplayWrapper::GetSingleton()->display();
  std::vector<VAConfigAttrib> config_attrib = required_attribs;
  for (size_t i = 0; i < config_attrib.size(); i += 1) {
    config_attrib[i].value = 0u;
  }

  VAStatus status =
      vaGetConfigAttributes(display, profile, required_entrypoint, config_attrib.data(),
                            safemath::checked_cast<int>(config_attrib.size()));
  if (status != VA_STATUS_SUCCESS) {
    FX_LOG_KV(ERROR, "vaGetConfigAttributes failed", FX_KV("error_str", vaErrorStr(status)));
    return false;
  }

  for (size_t i = 0; i < required_attribs.size(); i += 1) {
    if ((config_attrib[i].type != required_attribs[i].type) ||
        ((config_attrib[i].value & required_attribs[i].value) != required_attribs[i].value)) {
      using UnderlyingType = std::underlying_type_t<decltype(required_attribs[i].type)>;
      FX_LOG_KV(DEBUG, "Unsupported attribute",
                FX_KV("type", static_cast<UnderlyingType>(required_attribs[i].type)),
                FX_KV("required_value", required_attribs[i].value),
                FX_KV("actual_value", config_attrib[i].value));
      return false;
    }
  }

  return true;
}

bool SupportsProfile(VAProfile profile, VAEntrypoint required_entrypoint,
                     const std::vector<VAConfigAttrib>& required_attribs) {
  if (!SupportsEntrypointForProfile(profile, required_entrypoint)) {
    return false;
  }

  if (!SupportsAttribsForProfile(profile, required_entrypoint, required_attribs)) {
    return false;
  }

  return true;
}

std::optional<ProfileDescription> GetProfileDescription(
    const VAProfile& profile, VAEntrypoint required_entrypoint,
    std::vector<VAConfigAttrib>& required_attribs) {
  VADisplay display = VADisplayWrapper::GetSingleton()->display();

  if (!SupportsProfile(profile, required_entrypoint, required_attribs)) {
    return std::nullopt;
  }

  VAConfigID config_id;
  VAStatus status =
      vaCreateConfig(display, profile, required_entrypoint, required_attribs.data(),
                     safemath::checked_cast<int>(required_attribs.size()), &config_id);
  if (status != VA_STATUS_SUCCESS) {
    FX_LOG_KV(ERROR, "vaCreateConfig failed", FX_KV("error_str", vaErrorStr(status)));
    return std::nullopt;
  }
  ScopedContextID scoped_context(config_id);

  unsigned int num_attribs = 0;
  status = vaQuerySurfaceAttributes(display, config_id, nullptr, &num_attribs);
  if (status != VA_STATUS_SUCCESS) {
    FX_LOG_KV(ERROR, "vaQuerySurfaceAttributes failed", FX_KV("error_str", vaErrorStr(status)));
    return std::nullopt;
  }

  std::vector<VASurfaceAttrib> attrib_list(safemath::checked_cast<size_t>(num_attribs));
  status = vaQuerySurfaceAttributes(display, profile, attrib_list.data(), &num_attribs);
  if (status != VA_STATUS_SUCCESS) {
    FX_LOG_KV(ERROR, "vaQuerySurfaceAttributes failed", FX_KV("error_str", vaErrorStr(status)));
    return std::nullopt;
  }

  ProfileDescription description;
  description.profile = profile;
  description.entrypoint = required_entrypoint;
  for (const auto& attrib : attrib_list) {
    if (attrib.type == VASurfaceAttribMinWidth) {
      ZX_DEBUG_ASSERT(!description.min_width.has_value());
      description.min_width = attrib.value.value.i;
    } else if (attrib.type == VASurfaceAttribMaxWidth) {
      ZX_DEBUG_ASSERT(!description.max_width.has_value());
      description.max_width = attrib.value.value.i;
    } else if (attrib.type == VASurfaceAttribMinHeight) {
      ZX_DEBUG_ASSERT(!description.min_height.has_value());
      description.min_height = attrib.value.value.i;
    } else if (attrib.type == VASurfaceAttribMaxHeight) {
      ZX_DEBUG_ASSERT(!description.max_height.has_value());
      description.max_height = attrib.value.value.i;
    }
  }

  return description;
}

static std::vector<ProfileDescription> H264DecoderDescriptions() {
  std::vector<ProfileDescription> descriptions;

  std::vector<VAConfigAttrib> attribs = {
      {.type = VAConfigAttribRTFormat, .value = VA_RT_FORMAT_YUV420},
  };

  // Currently we only support the |VAProfileH264High| profile
  auto high_profile_description =
      GetProfileDescription(VAProfileH264High, VAEntrypointVLD, attribs);

  if (high_profile_description.has_value()) {
    descriptions.push_back(high_profile_description.value());
  }

  return descriptions;
}

static std::vector<ProfileDescription> VP9DecoderDescriptions() {
  std::vector<ProfileDescription> descriptions;

  std::vector<VAConfigAttrib> attribs = {
      {.type = VAConfigAttribRTFormat, .value = VA_RT_FORMAT_YUV420},
  };

  // Currently we only support the |VAProfileVP9Profile0| profile
  auto profile0_description = GetProfileDescription(VAProfileVP9Profile0, VAEntrypointVLD, attribs);

  if (profile0_description.has_value()) {
    descriptions.push_back(profile0_description.value());
  }

  return descriptions;
}

static bool SupportsH264Encoder() {
  std::vector<VAConfigAttrib> attribs = {
      {.type = VAConfigAttribRTFormat, .value = VA_RT_FORMAT_YUV420},
  };

  return SupportsProfile(VAProfileH264High, VAEntrypointEncSliceLP, attribs);
}

std::vector<VAProfile> GetHardwareSupportedProfiles() {
  VADisplay display = VADisplayWrapper::GetSingleton()->display();
  size_t max_num_profiles = safemath::checked_cast<size_t>(vaMaxNumProfiles(display));

  std::vector<VAProfile> supported_profiles(max_num_profiles, VAProfileNone);
  int num_profiles = 0;
  VAStatus status = vaQueryConfigProfiles(display, supported_profiles.data(), &num_profiles);
  if (status != VA_STATUS_SUCCESS) {
    FX_LOG_KV(ERROR, "vaQueryConfigProfiles failed", FX_KV("error_str", vaErrorStr(status)));
    return {};
  }

  supported_profiles.resize(safemath::checked_cast<size_t>(num_profiles));
  return supported_profiles;
}

static bool SupportsJPEGDecoder() {
  std::vector<VAConfigAttrib> attribs = {
      {.type = VAConfigAttribRTFormat, .value = VA_RT_FORMAT_YUV420},
  };

  return SupportsProfile(VAProfileJPEGBaseline, VAEntrypointVLD, attribs);
}

std::vector<fuchsia::mediacodec::CodecDescription> GetDeprecatedCodecList() {
  std::vector<fuchsia::mediacodec::CodecDescription> descriptions;

  if (SupportsJPEGDecoder()) {
    fuchsia::mediacodec::CodecDescription description;
    description.codec_type = fuchsia::mediacodec::CodecType::DECODER;
    description.mime_type = "video/x-motion-jpeg";
    descriptions.push_back(std::move(description));
  }

  if (!H264DecoderDescriptions().empty()) {
    fuchsia::mediacodec::CodecDescription description;
    description.codec_type = fuchsia::mediacodec::CodecType::DECODER;
    description.mime_type = "video/h264";
    descriptions.push_back(std::move(description));
  }

  if (!VP9DecoderDescriptions().empty()) {
    fuchsia::mediacodec::CodecDescription description;
    description.codec_type = fuchsia::mediacodec::CodecType::DECODER;
    description.mime_type = "video/vp9";
    descriptions.push_back(std::move(description));
  }

  if (SupportsH264Encoder()) {
    fuchsia::mediacodec::CodecDescription description;
    description.codec_type = fuchsia::mediacodec::CodecType::ENCODER;
    description.mime_type = "video/h264";
    descriptions.push_back(std::move(description));
  }

  return descriptions;
}

using ProfileMapType = std::map<fuchsia::media::CodecProfile, VAProfile>;
static ProfileMapType GetCodecProfileTranslation() {
  static const fbl::NoDestructor<ProfileMapType> kCodecProfileToVaProfileMap({
      {fuchsia::media::CodecProfile::H264PROFILE_BASELINE, VAProfileH264Baseline},
      {fuchsia::media::CodecProfile::H264PROFILE_MAIN, VAProfileH264Main},
      {fuchsia::media::CodecProfile::H264PROFILE_HIGH, VAProfileH264High},
      {fuchsia::media::CodecProfile::VP9PROFILE_PROFILE0, VAProfileVP9Profile0},
      {fuchsia::media::CodecProfile::VP9PROFILE_PROFILE1, VAProfileVP9Profile1},
      {fuchsia::media::CodecProfile::VP9PROFILE_PROFILE2, VAProfileVP9Profile2},
      {fuchsia::media::CodecProfile::VP9PROFILE_PROFILE3, VAProfileVP9Profile3},
  });

  return *kCodecProfileToVaProfileMap;
}

using VaProfileMapType = std::map<VAProfile, fuchsia::media::CodecProfile>;
static VaProfileMapType GetVaProfileTranslation() {
  static const fbl::NoDestructor<VaProfileMapType> kVaProfileToMap({
      {VAProfileH264Baseline, fuchsia::media::CodecProfile::H264PROFILE_BASELINE},
      {VAProfileH264Main, fuchsia::media::CodecProfile::H264PROFILE_MAIN},
      {VAProfileH264High, fuchsia::media::CodecProfile::H264PROFILE_HIGH},
      {VAProfileVP9Profile0, fuchsia::media::CodecProfile::VP9PROFILE_PROFILE0},
      {VAProfileVP9Profile1, fuchsia::media::CodecProfile::VP9PROFILE_PROFILE1},
      {VAProfileVP9Profile2, fuchsia::media::CodecProfile::VP9PROFILE_PROFILE2},
      {VAProfileVP9Profile3, fuchsia::media::CodecProfile::VP9PROFILE_PROFILE3},
  });

  return *kVaProfileToMap;
}

// TODO(https://fxbug.dev/42073599): Remove [[maybe_unused]] once used, or potentially remove the
// method if no longer expect to need to convert the other direction.
[[maybe_unused]] static VAProfile CodecProfileToVaProfile(fuchsia::media::CodecProfile profile) {
  const auto& profile_map = GetCodecProfileTranslation();
  const auto& profile_itr = profile_map.find(profile);

  if (profile_itr != profile_map.end()) {
    return profile_itr->second;
  }

  return VAProfileNone;
}

static fuchsia::media::CodecProfile VaProfileToCodecProfile(VAProfile profile) {
  const auto& profile_map = GetVaProfileTranslation();
  const auto& profile_itr = profile_map.find(profile);

  if (profile_itr != profile_map.end()) {
    return profile_itr->second;
  }

  return fuchsia::media::CodecProfile::Unknown();
}

std::vector<fuchsia::mediacodec::DetailedCodecDescription> GetCodecDescriptions() {
  std::vector<fuchsia::mediacodec::DetailedCodecDescription> codec_descriptions;

  fit::function<void(const std::string&, const std::vector<ProfileDescription>&)>
      add_decoder_profiles = [&codec_descriptions](
                                 const std::string& mime_type,
                                 const std::vector<ProfileDescription>& profile_descriptions) {
        fuchsia::mediacodec::DetailedCodecDescription codec_description;
        codec_description.set_codec_type(fuchsia::mediacodec::CodecType::DECODER);
        codec_description.set_mime_type(mime_type);
        codec_description.set_is_hw(true);

        // Iterate over each of the |ProfileDescription| and add the corresponding
        // |DecoderProfileDescription| to the list of supported profiles.
        std::vector<fuchsia::mediacodec::DecoderProfileDescription> decoder_descriptions;
        for (const auto& profile_description : profile_descriptions) {
          fuchsia::mediacodec::DecoderProfileDescription decoder_description;

          auto codec_profile = VaProfileToCodecProfile(profile_description.profile);
          if (codec_profile.IsUnknown()) {
            FX_LOG_KV(ERROR, "Unknown translation from VAProfile to CodecProfile",
                      FX_KV("va_profile", safemath::strict_cast<std::underlying_type_t<VAProfile>>(
                                              profile_description.profile)));
            continue;
          }
          decoder_description.set_profile(codec_profile);

          if (profile_description.min_width.has_value() &&
              profile_description.min_height.has_value()) {
            decoder_description.set_min_image_size(
                {profile_description.min_width.value(), profile_description.min_height.value()});
          }

          if (profile_description.max_width.has_value() &&
              profile_description.max_height.has_value()) {
            decoder_description.set_max_image_size(
                {profile_description.max_width.value(), profile_description.max_height.value()});
          }

          // VA-API currently does not support encrypted inputs.
          decoder_description.set_allow_encryption(false);
          decoder_description.set_require_encryption(false);

          decoder_descriptions.push_back(std::move(decoder_description));
        }

        // Ensure there is at least one supported decoder profile before adding the codec_profile.
        // It really doesn't make much sense to have a |DetailedCodecDescription| with no supported
        // profiles. If this is the case, then just return without adding the it to the list of
        // codec descriptions.
        if (decoder_descriptions.empty()) {
          return;
        }

        fuchsia::mediacodec::ProfileDescriptions decoder_profile_descriptions;
        decoder_profile_descriptions.set_decoder_profile_descriptions(
            std::move(decoder_descriptions));
        codec_description.set_profile_descriptions(std::move(decoder_profile_descriptions));

        codec_descriptions.push_back(std::move(codec_description));
      };

  add_decoder_profiles("video/h264", H264DecoderDescriptions());
  add_decoder_profiles("video/vp9", VP9DecoderDescriptions());

  if (SupportsJPEGDecoder()) {
    fuchsia::mediacodec::DetailedCodecDescription jpeg_description;
    jpeg_description.set_codec_type(fuchsia::mediacodec::CodecType::DECODER);
    jpeg_description.set_mime_type("video/x-motion-jpeg");
    jpeg_description.set_is_hw(true);

    fuchsia::mediacodec::DecoderProfileDescription decoder_description;
    decoder_description.set_profile(::fuchsia::media::CodecProfile::MJPEG_BASELINE);
    decoder_description.set_min_image_size({1, 1});
    // TBV whether we can successfully decode a max-dimensions jpeg via HW.
    decoder_description.set_max_image_size({65535, 65535});

    ::std::vector<::fuchsia::mediacodec::DecoderProfileDescription> decoder_descriptions;
    decoder_descriptions.emplace_back(std::move(decoder_description));
    fuchsia::mediacodec::ProfileDescriptions descriptions;
    descriptions.set_decoder_profile_descriptions(std::move(decoder_descriptions));
    jpeg_description.set_profile_descriptions(std::move(descriptions));
    codec_descriptions.emplace_back(std::move(jpeg_description));
  }

  if (SupportsH264Encoder()) {
    fuchsia::mediacodec::DetailedCodecDescription h264_description;
    h264_description.set_codec_type(fuchsia::mediacodec::CodecType::ENCODER);
    h264_description.set_mime_type("video/h264");
    h264_description.set_is_hw(true);

    fuchsia::mediacodec::EncoderProfileDescription encoder_description;
    encoder_description.set_profile(::fuchsia::media::CodecProfile::H264PROFILE_HIGH);

    ::std::vector<::fuchsia::mediacodec::EncoderProfileDescription> encoder_descriptions;
    encoder_descriptions.emplace_back(std::move(encoder_description));
    fuchsia::mediacodec::ProfileDescriptions descriptions;
    descriptions.set_encoder_profile_descriptions(std::move(encoder_descriptions));
    h264_description.set_profile_descriptions(std::move(descriptions));
    codec_descriptions.emplace_back(std::move(h264_description));
  }

  return codec_descriptions;
}
