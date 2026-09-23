// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "codec_factory_app.h"

#include <fuchsia/media/cpp/fidl.h>
#include <fuchsia/mediacodec/cpp/fidl.h>
#include <fuchsia/metrics/cpp/fidl.h>
#include <fuchsia/sysinfo/cpp/fidl.h>
#include <lib/fdio/directory.h>
#include <lib/fidl/cpp/interface_request.h>
#include <lib/fidl/cpp/string.h>
#include <lib/fidl/cpp/vector.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/vfs/cpp/pseudo_dir.h>
#include <unistd.h>
#include <zircon/assert.h>
#include <zircon/status.h>

#include <algorithm>
#include <random>

#include <fbl/string_printf.h>

#include "src/media/codec/factory/codec_factory_impl.h"
#include "src/media/codec/factory/codec_isolate.h"
#include "src/media/codec/factory/codec_specifier.h"

namespace {

// Device instances exposing either of these two paths are also handled, but via
// temporary migration code elsewhere. See
// src/devices/bin/driver_manager/devfs/class_names.h and related. The path
// names are kept in this comment for now in case it helps with searchability
// until we've migrated all codec and magma devices to serve without going
// through devfs.
//
// constexpr char kDeviceClass[] = "/dev/class/media-codec";
//
// constexpr char kGpuDeviceClass[] = "/dev/class/gpu";

const char* kLogTag = "CodecFactoryApp";

struct SwVideoDecoderInfo {
  const char* mime_type;
  fuchsia::media::CodecProfile profile;
  uint32_t min_width;
  uint32_t min_height;
  uint32_t max_width;
  uint32_t max_height;
  std::string codec_specifier;
};
const std::vector<SwVideoDecoderInfo> kSwVideoDecoderInfos = {
    {
        .mime_type = "video/h264",  // VIDEO_ENCODING_H264
        .profile = fuchsia::media::CodecProfile::H264PROFILE_HIGH,
        .min_width = 32,
        .min_height = 32,
        // The decode performance will likely not be realtime at these dimensions.
        .max_width = 3840,
        .max_height = 2160,
        .codec_specifier = CreateRandomCodecSpecifier(),
    },
};

const char* CodecProfileToString(fuchsia::media::CodecProfile profile) {
  switch (profile) {
    case fuchsia::media::CodecProfile::H264PROFILE_BASELINE:
      return "H264PROFILE_BASELINE";
    case fuchsia::media::CodecProfile::H264PROFILE_MAIN:
      return "H264PROFILE_MAIN";
    case fuchsia::media::CodecProfile::H264PROFILE_EXTENDED:
      return "H264PROFILE_EXTENDED";
    case fuchsia::media::CodecProfile::H264PROFILE_HIGH:
      return "H264PROFILE_HIGH";
    case fuchsia::media::CodecProfile::H264PROFILE_HIGH10PROFILE:
      return "H264PROFILE_HIGH10PROFILE";
    case fuchsia::media::CodecProfile::H264PROFILE_HIGH422PROFILE:
      return "H264PROFILE_HIGH422PROFILE";
    case fuchsia::media::CodecProfile::H264PROFILE_HIGH444PREDICTIVEPROFILE:
      return "H264PROFILE_HIGH444PREDICTIVEPROFILE";
    case fuchsia::media::CodecProfile::H264PROFILE_SCALABLEBASELINE:
      return "H264PROFILE_SCALABLEBASELINE";
    case fuchsia::media::CodecProfile::H264PROFILE_SCALABLEHIGH:
      return "H264PROFILE_SCALABLEHIGH";
    case fuchsia::media::CodecProfile::H264PROFILE_STEREOHIGH:
      return "H264PROFILE_STEREOHIGH";
    case fuchsia::media::CodecProfile::H264PROFILE_MULTIVIEWHIGH:
      return "H264PROFILE_MULTIVIEWHIGH";
    case fuchsia::media::CodecProfile::VP8PROFILE_ANY:
      return "VP8PROFILE_ANY";
    case fuchsia::media::CodecProfile::VP9PROFILE_PROFILE0:
      return "VP9PROFILE_PROFILE0";
    case fuchsia::media::CodecProfile::VP9PROFILE_PROFILE1:
      return "VP9PROFILE_PROFILE1";
    case fuchsia::media::CodecProfile::VP9PROFILE_PROFILE2:
      return "VP9PROFILE_PROFILE2";
    case fuchsia::media::CodecProfile::VP9PROFILE_PROFILE3:
      return "VP9PROFILE_PROFILE3";
    case fuchsia::media::CodecProfile::HEVCPROFILE_MAIN:
      return "HEVCPROFILE_MAIN";
    case fuchsia::media::CodecProfile::HEVCPROFILE_MAIN10:
      return "HEVCPROFILE_MAIN10";
    case fuchsia::media::CodecProfile::HEVCPROFILE_MAIN_STILL_PICTURE:
      return "HEVCPROFILE_MAIN_STILL_PICTURE";
    case fuchsia::media::CodecProfile::MJPEG_BASELINE:
      return "MJPEG_BASELINE";
    default:
      return "UNKNOWN";
  }
}

fuchsia::mediacodec::CodecDescription CreateDeprecatedCodecDescriptionFromDetailedCodecDescription(
    fuchsia::mediacodec::DetailedCodecDescription& detailed) {
  fuchsia::mediacodec::CodecDescription deprecated;
  deprecated.codec_type = detailed.codec_type();
  deprecated.mime_type = detailed.mime_type();
  deprecated.is_hw = detailed.is_hw();
  switch (detailed.codec_type()) {
    case fuchsia::mediacodec::CodecType::DECODER:
      // Default to more-capable, but if any per-profile info indicates less-capable, aggregate by
      // marking less-capable.
      deprecated.can_stream_bytes_input = true;
      deprecated.can_find_start = true;
      deprecated.can_re_sync = true;
      deprecated.will_report_all_detected_errors = true;
      deprecated.split_header_handling = true;
      for (auto& profile : detailed.profile_descriptions().decoder_profile_descriptions()) {
        if (!profile.has_can_stream_bytes_input() || !profile.can_stream_bytes_input()) {
          deprecated.can_stream_bytes_input = false;
        }
        if (!profile.has_can_find_start() || !profile.can_find_start()) {
          deprecated.can_find_start = false;
        }
        if (!profile.has_can_re_sync() || !profile.can_re_sync()) {
          deprecated.can_re_sync = false;
        }
        if (!profile.has_will_report_all_detected_errors() ||
            !profile.will_report_all_detected_errors()) {
          deprecated.will_report_all_detected_errors = false;
        }
        if (!profile.has_split_header_handling() || !profile.split_header_handling()) {
          deprecated.split_header_handling = false;
        }
      }
      break;
    case fuchsia::mediacodec::CodecType::ENCODER:
      // These don't make sense for encoders, so set them all to false.  The ambiguity of these
      // fields appearing to apply to encoders when really they make no sense for encoders is
      // among the reasons we're deprecating OnCodecList.
      deprecated.can_stream_bytes_input = false;
      deprecated.can_find_start = false;
      deprecated.can_re_sync = false;
      deprecated.will_report_all_detected_errors = false;
      deprecated.split_header_handling = false;
  }
  return deprecated;
}

}  // namespace

// board_name_ initialization requires startup_context_ already initialized.
// policy_ initialization requires board_name_ already initialized.
CodecFactoryApp::CodecFactoryApp(async_dispatcher_t* dispatcher, ProdOrTest prod_or_test)
    : dispatcher_(dispatcher),
      prod_or_test_(prod_or_test),
      startup_context_(sys::ComponentContext::Create()),
      board_name_(GetBoardName()),
      policy_(this) {
  inspector_ =
      std::make_unique<inspect::ComponentInspector>(dispatcher_, inspect::PublishOptions{});
  inspector_->Health().StartingUp();
  hardware_factory_nodes_ = inspector_->root().CreateChild("hardware_factory_nodes");
  // Don't publish service or outgoing()->ServeFromStartupInfo() until after initial discovery is
  // done, else the pumping of the loop will drop the incoming request for CodecFactory before
  // AddPublicService() below has had a chance to register for it.

  zx_status_t status =
      outgoing_codec_aux_service_directory_parent_
          .AddPublicService<fuchsia::metrics::MetricEventLoggerFactory>(
              [this](fidl::InterfaceRequest<fuchsia::metrics::MetricEventLoggerFactory> request) {
                ZX_DEBUG_ASSERT(startup_context_);
                FX_LOG_KV(INFO, "codec_factory handling request for MetricEventLoggerFactory",
                          FX_KV("tag", kLogTag), FX_KV("handle value", request.channel().get()));
                startup_context_->svc()->Connect(std::move(request));
              });
  outgoing_codec_aux_service_directory_ =
      outgoing_codec_aux_service_directory_parent_.GetOrCreateDirectory("svc");

  // Else codec_factory won't be able to provide what codecs expect to be able to rely on.
  ZX_ASSERT(status == ZX_OK);

  DiscoverMagmaCodecDriversAndListenForMoreAsync();
  DiscoverMediaCodecDriversAndListenForMoreAsync();
}

void CodecFactoryApp::PublishService() {
  // We delay doing this until we're completely ready to add services.
  // We _rely_ on the driver to either fail the channel or respond to GetDetailedCodecDescriptions.
  ZX_DEBUG_ASSERT(existing_devices_discovered_);
  zx_status_t status =
      startup_context_->outgoing()->AddPublicService<fuchsia::mediacodec::CodecFactory>(
          [this](fidl::InterfaceRequest<fuchsia::mediacodec::CodecFactory> request) {
            // The CodecFactoryImpl is self-owned and will self-delete when the
            // channel closes or an error occurs.
            CodecFactoryImpl::CreateSelfOwned(this, startup_context_.get(), std::move(request));
          });
  // else this codec_factory is useless
  ZX_ASSERT(status == ZX_OK);
  inspector_->Health().Ok();
  if (prod_or_test_ == ProdOrTest::kProduction) {
    status = startup_context_->outgoing()->ServeFromStartupInfo();
    ZX_ASSERT(status == ZX_OK);
  }
}

// All of the current supported hardware and software decoders, randomly shuffled
// so as to avoid clients depending on the order.
// TODO(schottm): send encoders as well
std::vector<fuchsia::mediacodec::CodecDescription> CodecFactoryApp::MakeCodecList() const {
  std::vector<fuchsia::mediacodec::CodecDescription> codecs;

  for (const auto& info : kSwVideoDecoderInfos) {
    codecs.push_back({
        .codec_type = fuchsia::mediacodec::CodecType::DECODER,
        .mime_type = info.mime_type,

        // TODO(schottm): can some of these be true?
        .can_stream_bytes_input = false,
        .can_find_start = false,
        .can_re_sync = false,
        .will_report_all_detected_errors = false,

        .is_hw = false,
        .split_header_handling = true,
    });
  }

  for (const auto& factory : hw_factories_) {
    for (const auto& codec : factory->hw_codecs) {
      codecs.push_back(codec->deprecated_codec_description);
    }
  }

  auto rng = std::default_random_engine();
  std::shuffle(codecs.begin(), codecs.end(), rng);

  return codecs;
}

std::vector<fuchsia::mediacodec::DetailedCodecDescription>
CodecFactoryApp::MakeDetailedCodecDescriptions() const {
  std::vector<fuchsia::mediacodec::DetailedCodecDescription> codec_descriptions;

  for (const auto& info : kSwVideoDecoderInfos) {
    fuchsia::mediacodec::DetailedCodecDescription description;
    description.set_codec_type(fuchsia::mediacodec::CodecType::DECODER);
    description.set_mime_type(info.mime_type);
    description.set_is_hw(false);
    // This is correct so far.
    description.set_supports_dynamic_buffers(false);
    description.set_codec_specifier(info.codec_specifier);
    fuchsia::mediacodec::DecoderProfileDescription profile_description;
    profile_description.set_profile(info.profile);
    profile_description.set_min_image_size({info.min_width, info.min_height});
    profile_description.set_max_image_size({info.max_width, info.max_height});
    // We leave the rest of the fields un-set intentionally, since clients must accept a non-set
    // field and know that means what the comments on the field in the FIDL file say it means (no
    // programmatic field defaults).
    std::vector<fuchsia::mediacodec::DecoderProfileDescription> profiles;
    profiles.push_back(std::move(profile_description));
    fuchsia::mediacodec::ProfileDescriptions profile_descriptions;
    profile_descriptions.set_decoder_profile_descriptions(std::move(profiles));
    description.set_profile_descriptions(std::move(profile_descriptions));
    codec_descriptions.push_back(std::move(description));
  }

  for (const auto& factory : hw_factories_) {
    for (const auto& codec : factory->hw_codecs) {
      // Need to clone the |DetailedCodecDescription| since it is not copyable
      fuchsia::mediacodec::DetailedCodecDescription detailed_description;
      ZX_ASSERT(codec->detailed_description.Clone(&detailed_description) == ZX_OK);
      if (!detailed_description.has_supports_dynamic_buffers()) {
        detailed_description.set_supports_dynamic_buffers(false);
      }
      codec_descriptions.push_back(std::move(detailed_description));
    }
  }

  return codec_descriptions;
}

const fuchsia::mediacodec::CodecFactoryPtr* CodecFactoryApp::FindHwCodec(
    fit::function<bool(const fuchsia::mediacodec::DetailedCodecDescription&)> is_match) {
  for (auto& hw_factory : hw_factories_) {
    // HW codecs are connected to using factory, not by launching a component using the URL.
    if (!hw_factory->component_url.empty()) {
      continue;
    }

    auto iter = std::find_if(hw_factory->hw_codecs.begin(), hw_factory->hw_codecs.end(),
                             [&is_match](const std::unique_ptr<CodecListEntry>& entry) -> bool {
                               return is_match(entry->detailed_description);
                             });

    if (iter != hw_factory->hw_codecs.end()) {
      return hw_factory->codec_factory.get();
    }
  }

  return nullptr;
}

std::optional<std::string> CodecFactoryApp::FindHwIsolate(
    fit::function<bool(const fuchsia::mediacodec::DetailedCodecDescription&)> is_match) {
  for (auto& hw_factory : hw_factories_) {
    // Isolate codecs are connected by launching a component using the URL.
    if (hw_factory->component_url.empty()) {
      continue;
    }

    auto iter = std::find_if(hw_factory->hw_codecs.begin(), hw_factory->hw_codecs.end(),
                             [&is_match](const std::unique_ptr<CodecListEntry>& entry) -> bool {
                               return is_match(entry->detailed_description);
                             });

    if (iter != hw_factory->hw_codecs.end()) {
      return hw_factory->component_url;
    }
  }

  return std::nullopt;
}

void CodecFactoryApp::IdledCodecDiscovery() {
  ZX_ASSERT(num_codec_discoveries_in_flight_ >= 1);
  if (--num_codec_discoveries_in_flight_ == 0) {
    // The idle_callback indicates that all pre-existing devices have been
    // seen, and by the time this item reaches the front of the discovery
    // queue, all pre-existing devices have all been processed.
    device_discovery_queue_.emplace_back(std::make_unique<DeviceDiscoveryEntry>());
    PostDiscoveryQueueProcessing();
  }
}

void CodecFactoryApp::DiscoverMediaCodecDriversAndListenForMoreAsync() {
  num_codec_discoveries_in_flight_++;

  // Pre-check if fuchsia.hardware.mediacodec.Service is routed to codec_factory.
  // We can do this by checking if the service directory is present in /svc.
  // If it is not routed, we immediately report we are "idle" for media codec discovery.
  // This allows discovery to proceed without hardware media codec codecs and avoids
  // a ZX_PANIC due to codec_service_watcher_.Begin failing.
  std::string service_path = std::string("/svc/") + fuchsia_hardware_mediacodec::Service::Name;
  if (access(service_path.c_str(), F_OK) != 0) {
    FX_LOGS(INFO) << "fuchsia.hardware.mediacodec.Service is not routed to codec_factory. "
                  << "Continuing without hardware media codec codecs.";
    async::PostTask(dispatcher_, [this] { IdledCodecDiscovery(); });
    return;
  }

  // We don't want to start serving CodecFactory until we've discovered and processed all existing
  // media-codec devices.  That way, the first time a client requests a HW-backed codec, we robustly
  // consider all codecs provided by pre-existing devices.  The request for a HW-backed Codec will
  // have a much higher probability of succeeding vs. if we just discovered pre-existing devices
  // async.  This doesn't prevent the possibility that the device might not exist at the moment the
  // CodecFactory is started, but as long as the device does exist by then, this will ensure the
  // device's codecs are considered, including for the first client request.
  auto begin_result = codec_service_watcher_.Begin(
      dispatcher_,
      [this](fidl::ClientEnd<fuchsia_hardware_mediacodec::Device> codec_device_client) {
        auto device_interface = fidl::SyncClient(std::move(codec_device_client));

        auto [aux_svc_client, aux_svc_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
        fidl::InterfaceHandle<fuchsia::io::Directory> aux_service_directory;
        if (zx_status_t status = outgoing_codec_aux_service_directory_->Serve(
                fuchsia_io::wire::kPermReadable | fuchsia_io::wire::kPermWritable,
                std::move(aux_svc_server), dispatcher_);
            status != ZX_OK) {
          FX_PLOGS(ERROR, status) << "outgoing_codec_aux_service_directory_.Serve() failed";
          return;
        }

        auto discovery_entry = std::make_unique<DeviceDiscoveryEntry>();
        discovery_entry->codec_factory = std::make_shared<fuchsia::mediacodec::CodecFactoryPtr>();

        fuchsia_hardware_mediacodec::DeviceSetAuxServiceDirectoryRequest set_aux_request;
        set_aux_request.service_directory() = std::move(aux_svc_client);
        auto set_aux_result = device_interface->SetAuxServiceDirectory(std::move(set_aux_request));
        if (!set_aux_result.is_ok()) {
          FX_LOGS(WARNING) << "device_interface->SetAuxServiceDirectory() failed: "
                           << set_aux_result.error_value().FormatDescription();
          return;
        }
        fuchsia_hardware_mediacodec::DeviceGetCodecFactoryRequest get_factory_request;
        get_factory_request.request() =
            discovery_entry->codec_factory->NewRequest(dispatcher()).TakeChannel();
        auto get_factory_result = device_interface->GetCodecFactory(std::move(get_factory_request));
        if (!get_factory_result.is_ok()) {
          FX_LOGS(WARNING) << "device_interface->GetCodecFactory() failed: "
                           << get_factory_result.error_value().FormatDescription();
          return;
        }

        // From here on in the current lambda, we're doing stuff that can't fail
        // here locally (at least, not without exiting the whole process).  The
        // error handler will handle channel error async.

        discovery_entry->codec_factory->set_error_handler(
            [this, factory = discovery_entry->codec_factory](zx_status_t status) {
              // Any given factory won't be in both lists, but will be in one or
              // the other by the time this error handler runs. We must keep
              // factory alive until the end of this lambda, else ambiguity of
              // the pointer value would cause trouble.
              device_discovery_queue_.remove_if(
                  [&factory](const std::unique_ptr<DeviceDiscoveryEntry>& entry) {
                    return factory.get() == entry->codec_factory.get();
                  });
              // Perhaps the removed discovery item was the first item in the
              // list; maybe now the new first item in the list can be
              // processed.
              PostDiscoveryQueueProcessing();

              hw_factories_.remove_if(
                  [&factory](const std::unique_ptr<CodecFactoryEntry>& factory_entry) {
                    return factory.get() == factory_entry->codec_factory.get();
                  });

              // ~factory
            });

        // Queue up a call to |GetDetailedCodecDescriptions()| and store the output in the
        // discovery_entry.
        FX_LOGS(INFO) << "Calling GetDetailedCodecDescriptions";
        (*discovery_entry->codec_factory)
            ->GetDetailedCodecDescriptions(
                [this, discovery_entry = discovery_entry.get()](
                    fuchsia::mediacodec::CodecFactoryGetDetailedCodecDescriptionsResponse
                        response) {
                  ZX_ASSERT(response.has_codecs());
                  ZX_ASSERT(response.mutable_codecs());
                  FX_LOGS(INFO) << "GetDetailedCodecDescriptions response received";
                  discovery_entry->detailed_codec_descriptions =
                      std::move(*response.mutable_codecs());
                  // In case discovery_entry is the first item which is now ready to
                  // process, process the discovery queue.
                  PostDiscoveryQueueProcessing();
                });

        device_discovery_queue_.emplace_back(std::move(discovery_entry));
      },
      [this] { IdledCodecDiscovery(); });
  if (!begin_result.is_ok()) {
    // The CodecFactoryApp won't serve any codecs until codec discovery is
    // complete (whether any are found or not), so failure to run the discovery
    // is fatal for now.
    ZX_PANIC("codec_service_watcher_.Begin failed: %s", begin_result.status_string());
  }
}

void CodecFactoryApp::TeardownMagmaCodec(
    const std::shared_ptr<fuchsia::gpu::magma::IcdLoaderDevicePtr>& magma_device) {
  // Any given magma device won't be in both lists, but will be in one or
  // the other by the time this error handler runs.
  device_discovery_queue_.remove_if(
      [&magma_device](const std::unique_ptr<DeviceDiscoveryEntry>& entry) {
        return magma_device.get() == entry->magma_device.get();
      });

  hw_factories_.remove_if([&magma_device](const std::unique_ptr<CodecFactoryEntry>& factory_entry) {
    return magma_device.get() == factory_entry->magma_device.get();
  });

  // Perhaps the removed discovery item was the first item in the
  // list; maybe now the new first item in the list can be
  // processed.
  PostDiscoveryQueueProcessing();
}

void CodecFactoryApp::DiscoverMagmaCodecDriversAndListenForMoreAsync() {
  num_codec_discoveries_in_flight_++;

  // Pre-check if fuchsia.gpu.magma.Service is routed to codec_factory.
  // We can do this by checking if the service directory is present in /svc.
  // If it is not routed, we immediately report we are "idle" for magma discovery.
  // This allows discovery to proceed without magma codecs and avoids a ZX_PANIC
  // due to magma_service_watcher_.Begin failing.
  std::string service_path = std::string("/svc/") + fuchsia_gpu_magma::Service::Name;
  if (access(service_path.c_str(), F_OK) != 0) {
    FX_LOGS(INFO) << "fuchsia.gpu.magma.Service is not routed to codec_factory. "
                  << "Continuing without magma codecs.";
    async::PostTask(dispatcher_, [this] { IdledCodecDiscovery(); });
    return;
  }

  auto begin_result = magma_service_watcher_.Begin(
      dispatcher_,
      [this](fidl::ClientEnd<fuchsia_gpu_magma::CombinedDevice> magma_device_client) {
        // The fuchsia_gpu_magma::Device is an aggregated protocol; this client only needs to use
        // the fuchsia_gpu_magma::IcdLoaderDevice protocol, so we convert as early as possible here.
        auto magma_device = std::make_shared<fuchsia::gpu::magma::IcdLoaderDevicePtr>();
        magma_device->Bind(magma_device_client.TakeChannel(), dispatcher());

        auto discovery_entry = std::make_unique<DeviceDiscoveryEntry>();
        discovery_entry->magma_device = magma_device;
        discovery_entry->magma_device->set_error_handler(
            [this, magma_device](zx_status_t status) { TeardownMagmaCodec(magma_device); });

        (*magma_device)
            ->GetIcdList([this, discovery_entry = discovery_entry.get(), magma_device](
                             const std::vector<fuchsia::gpu::magma::IcdInfo>& icd_infos) {
              bool found_media_icd = false;
              for (auto& icd_entry : icd_infos) {
                if (!icd_entry.has_flags() || !icd_entry.has_component_url())
                  continue;
                if (!(icd_entry.flags() &
                      fuchsia::gpu::magma::IcdFlags::SUPPORTS_MEDIA_CODEC_FACTORY)) {
                  continue;
                }
                discovery_entry->component_url = icd_entry.component_url();
                ForwardToIsolate(
                    icd_entry.component_url(), IsolateType::kMagma, startup_context_.get(),
                    [this, magma_device](fuchsia::mediacodec::CodecFactoryPtr ptr) {
                      auto it = std::find_if(
                          device_discovery_queue_.begin(), device_discovery_queue_.end(),
                          [magma_device](const std::unique_ptr<DeviceDiscoveryEntry>& entry) {
                            return magma_device.get() == entry->magma_device.get();
                          });
                      if (it == device_discovery_queue_.end()) {
                        // Device was removed from the queue due to the magma error handler running.
                        return;
                      }
                      auto discovery_entry = it->get();

                      discovery_entry->codec_factory =
                          std::make_shared<fuchsia::mediacodec::CodecFactoryPtr>();
                      discovery_entry->codec_factory->Bind(ptr.Unbind());
                      discovery_entry->codec_factory->set_error_handler(
                          [this, magma_device](zx_status_t status) {
                            TeardownMagmaCodec(magma_device);
                          });
                      // Queue up a call to |GetDetailedCodecDescriptions()| and store the output
                      // in the discovery_entry.
                      FX_LOGS(INFO) << "Calling GetDetailedCodecDescriptions for "
                                    << discovery_entry->component_url;
                      (*discovery_entry->codec_factory)
                          ->GetDetailedCodecDescriptions(
                              [this, discovery_entry](
                                  fuchsia::mediacodec::
                                      CodecFactoryGetDetailedCodecDescriptionsResponse response) {
                                ZX_ASSERT(response.has_codecs());
                                ZX_ASSERT(response.mutable_codecs());
                                FX_LOGS(INFO)
                                    << "GetDetailedCodecDescriptions response received for "
                                    << discovery_entry->component_url;
                                discovery_entry->detailed_codec_descriptions =
                                    std::move(*response.mutable_codecs());
                                // In case discovery_entry is the first item which is now ready to
                                // process, process the discovery queue.
                                PostDiscoveryQueueProcessing();
                              });
                    },
                    [this, magma_device]() { TeardownMagmaCodec(magma_device); });
                found_media_icd = true;
                // Only support a single codec factory per magma device.
                break;
              }
              if (!found_media_icd) {
                TeardownMagmaCodec(magma_device);
              }
            });
        device_discovery_queue_.emplace_back(std::move(discovery_entry));
      },
      [this] { IdledCodecDiscovery(); });
  if (!begin_result.is_ok()) {
    // The CodecFactoryApp won't serve any codecs until magma codec discovery is
    // complete (whether any are found or not), so failure to run the discovery
    // is fatal for now.
    ZX_PANIC("magma_service_watcher_.Begin failed: %s", begin_result.status_string());
  }
}

void CodecFactoryApp::PostDiscoveryQueueProcessing() {
  async::PostTask(dispatcher_, fit::bind_member<&CodecFactoryApp::ProcessDiscoveryQueue>(this));
}

void CodecFactoryApp::ProcessDiscoveryQueue() {
  // Both startup and steady-state use this processing loop.
  //
  // In startup, we care about ordering of the discovery queue because we want
  // to allow serving of CodecFactory as soon as all pre-existing devices are
  // done processing.  We care that pre-existing devices are before
  // newly-discovered devices in the queue.  As far as startup is concerned,
  // there are other ways we could track this without using a queue, but a queue
  // works, and using the queue allows startup to share code with steady-state.
  //
  // In steady-state, we care (a little) about ordering of the discovery queue
  // because we want (to a limited degree, for now) to prefer a
  // more-recently-discovered device over a less-recently-discovered device (for
  // now at least), so to make that robust, we preserve the device discovery
  // order through the codec discovery sequence, to account for the possibility
  // that an previously discovered device may have only just recently responded
  // to GetDetailedCodecDescriptions before failing; without the
  // device_discovery_queue_ that previously-discovered device's response could
  // re-order vs. the replacement device's response.
  //
  // The device_discovery_queue_ marginally increases the odds of a client
  // request picking up a replacement devhost instead of an old devhost that
  // failed quickly and which we haven't yet noticed is gone.  This devhost
  // replacement case is the main motivation for caring about the device
  // discovery order in the first place (at least for now), since it should be
  // robustly the case that discovery of the old devhost happens before
  // discovery of the replacement devhost.
  //
  // The ordering of the hw_codec_ list is the main way in which
  // more-recently-discovered codecs are preferred over less-recently-discovered
  // codecs.  The device_discovery_queue_ just makes the hw_codec_ ordering
  // exactly correspond to the device discovery order (reversed) even when
  // devices are discovered near each other in time.
  //
  // None of this changes the fact that a replacement devhost's arrival can race
  // with a client's request, so if a devhost fails and is replaced, it's quite
  // possible the client will see the Codec interface just fail.  Even if this
  // were mitigated, it wouldn't change the fact that a devhost failure later
  // would result in Codec interface failure at that time, so failures near the
  // start aren't really much different than async failures later. It can make
  // sense for a client to retry a low number of times (if the client wants to
  // work despite a devhost not always fully working), even if the Codec failure
  // happens quite early.
  while (!device_discovery_queue_.empty()) {
    std::unique_ptr<DeviceDiscoveryEntry>& discovered_device = device_discovery_queue_.front();
    if (!discovered_device->codec_factory && !discovered_device->magma_device) {
      // All pre-existing devices have been processed.
      //
      // Now the CodecFactory can begin serving (shortly).
      if (!existing_devices_discovered_) {
        existing_devices_discovered_ = true;
        FX_LOGS(INFO) << "CodecFactory calling PublishService()";
        PublishService();
      }
      // The marker has done its job, so remove the marker.
      device_discovery_queue_.pop_front();
      return;
    }

    // Wait for detailed_codec_descriptions.  We don't wait for driver_codec_list, which is
    // generated from detailed_codec_descriptions below.
    if (!discovered_device->detailed_codec_descriptions) {
      // The first item is not yet ready. The current method will get re-posted
      // when the first item is potentially ready.
      return;
    }

    if (!discovered_device->component_url.empty()) {
      // If there's a component URL then a new instance will be launched for every codec, so
      // codec_factory won't be used anymore.
      discovered_device->codec_factory = {};
    }

    auto inspect_name = std::format("device_{}", next_inspect_ordinal_++);

    // Create the hardware codec factory that these codecs will belong to
    auto hw_codec_factory = std::make_unique<CodecFactoryEntry>();
    hw_codec_factory->component_url = discovered_device->component_url;
    hw_codec_factory->codec_factory = discovered_device->codec_factory;
    hw_codec_factory->magma_device = discovered_device->magma_device;

    // Device paths are always unique and must be provided for hardware factories
    hw_codec_factory->factory_node = hardware_factory_nodes_.CreateChild(inspect_name);

    hw_codec_factory->factory_node.RecordString("inspect_name", inspect_name);

    const std::string& component_url =
        !discovered_device->component_url.empty() ? discovered_device->component_url : "N/A";
    hw_codec_factory->factory_node.RecordString("component_url", component_url);

    // All codecs must implement GetDetailedCodecDescriptions.  We ignore any OnCodecList from each
    // codec in favor of building an OnCodecList from GetDetailedCodecDescriptions responses, so
    // that each codec (roughly speaking) can stop sending the deprecated OnCodecList before all
    // clients have stopped listening to OnCodecList.
    FX_CHECK(discovered_device->detailed_codec_descriptions);

    while (!discovered_device->detailed_codec_descriptions->empty()) {
      auto detailed_description = std::move(discovered_device->detailed_codec_descriptions->back());
      discovered_device->detailed_codec_descriptions->pop_back();

      FX_LOGS(INFO) << "Registering "
                    << (detailed_description.codec_type() == fuchsia::mediacodec::CodecType::DECODER
                            ? "decoder"
                            : "encoder")
                    << ", mime_type: " << detailed_description.mime_type()
                    << ", inspect_name: " << inspect_name
                    << ", component url: " << discovered_device->component_url;

      if (!detailed_description.has_profile_descriptions()) {
        FX_LOGS(WARNING) << "!has_profile_descriptions for " << detailed_description.mime_type()
                         << " "
                         << ((detailed_description.codec_type() ==
                              fuchsia::mediacodec::CodecType::DECODER)
                                 ? "DECODER"
                                 : "ENCODER");
        continue;
      }

      // If a local CodecFactory doesn't provide a codec_specifier, we assign one here.
      // Newer/updated codec servers will provide their own codec_specifier(s) so they can fully
      // plumb the specifier. A specifiier assigned here will ensure we route to the correct codec
      // server, but the final routing is achieved by the rest of the CreateDecoder/CreateDecoder
      // params, which isn't quite as rigorous as the codec server setting codec_specifier but will
      // work for all currently-existing old codec servers under normal use by clients, since the
      // rest of the params are sufficiently unambiguous to route the rest of the way.
      if (!detailed_description.has_codec_specifier()) {
        detailed_description.set_codec_specifier(CreateRandomCodecSpecifier());
      }

      auto deprecated_codec_description =
          CreateDeprecatedCodecDescriptionFromDetailedCodecDescription(detailed_description);

      auto& parent_node = hw_codec_factory->factory_node;
      inspect::Node node = parent_node.CreateChild(detailed_description.mime_type());
      node.RecordString("type",
                        detailed_description.codec_type() == fuchsia::mediacodec::CodecType::DECODER
                            ? "decoder"
                            : "encoder");
      node.RecordString("mime_type", detailed_description.mime_type());

      // Populate the detailed inspect values, which is different for decoders vs. encoders.
      switch (detailed_description.codec_type()) {
        case fuchsia::mediacodec::CodecType::DECODER:
          node.RecordChild("supported_profiles", [&detailed_description](
                                                     inspect::Node& supported_profile_nodes) {
            if (!detailed_description.has_profile_descriptions()) {
              // A warning was alraedy printed above.
              return;
            }
            if (!detailed_description.profile_descriptions().is_decoder_profile_descriptions()) {
              FX_LOGS(WARNING) << "DECODER !is_decoder_profile_descriptions for "
                               << detailed_description.mime_type();
              return;
            }
            uint32_t profile_ordinal = 0u;
            const auto& profiles =
                detailed_description.profile_descriptions().decoder_profile_descriptions();
            for (const auto& profile : profiles) {
              supported_profile_nodes.RecordChild(
                  std::to_string(profile_ordinal), [&profile](inspect::Node& profile_node) {
                    FX_DCHECK(profile.has_profile());
                    profile_node.RecordString("profile", CodecProfileToString(profile.profile()));

                    if (profile.has_min_image_size()) {
                      const auto& min_size = profile.min_image_size();
                      profile_node.RecordUint("min_width", min_size.width);
                      profile_node.RecordUint("min_height", min_size.height);
                    }

                    if (profile.has_max_image_size()) {
                      const auto& max_size = profile.max_image_size();
                      profile_node.RecordUint("max_width", max_size.width);
                      profile_node.RecordUint("max_height", max_size.height);
                    }
                  });
              profile_ordinal += 1u;
            }
          });
          break;
        case fuchsia::mediacodec::CodecType::ENCODER:
          node.RecordChild("supported_profiles", [&detailed_description](
                                                     inspect::Node& supported_profile_nodes) {
            if (!detailed_description.has_profile_descriptions()) {
              // A warning was already printed above.
              return;
            }
            if (!detailed_description.profile_descriptions().is_encoder_profile_descriptions()) {
              FX_LOGS(WARNING) << "ENCODER !is_encoder_profile_descriptions for "
                               << detailed_description.mime_type();
              return;
            }
            uint32_t profile_ordinal = 0u;
            const auto& profiles =
                detailed_description.profile_descriptions().encoder_profile_descriptions();
            for (const auto& profile : profiles) {
              supported_profile_nodes.RecordChild(
                  std::to_string(profile_ordinal), [&profile](inspect::Node& profile_node) {
                    FX_DCHECK(profile.has_profile());
                    profile_node.RecordString("profile", CodecProfileToString(profile.profile()));
                  });
              profile_ordinal += 1u;
            }
          });
          break;
      }

      hw_codec_factory->hw_codecs.emplace_front(std::make_unique<CodecListEntry>(CodecListEntry{
          .detailed_description = std::move(detailed_description),
          .codec_node = std::move(node),
          .deprecated_codec_description = std::move(deprecated_codec_description),
      }));
    }

    hw_factories_.push_back(std::move(hw_codec_factory));
    device_discovery_queue_.pop_front();
  }
}

// This is called during field initialization portion of the constructor, so needs to avoid reading
// any fields that are not yet initialized.
std::string CodecFactoryApp::GetBoardName() {
  fuchsia::sysinfo::SysInfoSyncPtr sysinfo;
  zx_status_t status =
      startup_context_->svc()->Connect<fuchsia::sysinfo::SysInfo>(sysinfo.NewRequest());
  // CodecFactoryApp's process can't necessarily work correctly without the board name.
  ZX_ASSERT(status == ZX_OK);
  fidl::StringPtr board_name;
  zx_status_t fidl_status = sysinfo->GetBoardName(&status, &board_name);
  if (fidl_status != ZX_OK || status != ZX_OK) {
    // This path is only taken if CodecFactory can't contact fuchsia.sysinfo.SysInfo.  Most often
    // this happens in tests that don't correctly route the capability for protocol
    // fuchsia.sysinfo.SysInfo.
    FX_LOGS(WARNING) << "#############################";
    FX_LOGS(WARNING) << "sysinfo->GetBoardName() failed.  "
                        "CodecFactoryApp needs access to fuchsia.sysinfo.SysInfo.  fidl_status: "
                     << fidl_status << " status: " << status;
    FX_LOGS(WARNING) << "#############################";
    return "<UNKNOWN>";
  }
  ZX_ASSERT(fidl_status == ZX_OK && status == ZX_OK);
  return board_name.value();
}
