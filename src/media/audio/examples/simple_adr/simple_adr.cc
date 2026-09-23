// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/examples/simple_adr/simple_adr.h"

#include <fidl/fuchsia.hardware.audio/cpp/natural_types.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/wire/internal/transport_channel.h>
#include <lib/fzl/vmar-manager.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>

#include <cassert>
#include <cmath>
#include <cstdlib>
#include <iomanip>
#include <iostream>
#include <ostream>
#include <string>
#include <utility>

#include <src/lib/fxl/strings/string_printf.h>

namespace examples {

namespace {

inline std::ostream& operator<<(
    std::ostream& out,
    const std::optional<fuchsia_audio_device::PlugDetectCapabilities>& plug_caps) {
  if (plug_caps.has_value()) {
    switch (*plug_caps) {
      case fuchsia_audio_device::PlugDetectCapabilities::kHardwired:
        return (out << "kHardwired");
      case fuchsia_audio_device::PlugDetectCapabilities::kPluggable:
        return (out << "kPluggable");
      default:
        return (out << "unknown PlugDetectCapabilities enum");
    }
  }
  return (out << "NONE");
}

inline std::ostream& operator<<(std::ostream& out,
                                const std::optional<fuchsia_audio_device::PlugState>& plug_state) {
  if (plug_state.has_value()) {
    switch (*plug_state) {
      case fuchsia_audio_device::PlugState::kPlugged:
        return (out << "kPlugged");
      case fuchsia_audio_device::PlugState::kUnplugged:
        return (out << "kUnplugged");
      default:
        return (out << "unknown PlugState enum");
    }
  }
  return (out << "NONE (non-compliant)");
}

inline std::ostream& operator<<(std::ostream& out,
                                const std::optional<fuchsia_audio::SampleType>& sample_type) {
  if (sample_type.has_value()) {
    switch (*sample_type) {
      case fuchsia_audio::SampleType::kUint8:
        return (out << "kUint8");
      case fuchsia_audio::SampleType::kInt16:
        return (out << "kInt16");
      case fuchsia_audio::SampleType::kInt32:
        return (out << "kInt32");
      case fuchsia_audio::SampleType::kFloat32:
        return (out << "kFloat32");
      case fuchsia_audio::SampleType::kFloat64:
        return (out << "kFloat64");
      default:
        return (out << "unknown SampleType enum");
    }
  }
  return (out << "NONE (non-compliant)");
}

inline std::ostream& operator<<(std::ostream& out,
                                const std::optional<fuchsia_audio::ChannelLayout>& channel_layout) {
  if (channel_layout.has_value()) {
    switch (channel_layout->config().value()) {
      case fuchsia_audio::ChannelConfig::kMono:
        return (out << "kMono");
      case fuchsia_audio::ChannelConfig::kStereo:
        return (out << "kStereo");
      case fuchsia_audio::ChannelConfig::kQuad:
        return (out << "kQuad");
      case fuchsia_audio::ChannelConfig::kSurround3:
        return (out << "kSurround3");
      case fuchsia_audio::ChannelConfig::kSurround4:
        return (out << "kSurround4");
      case fuchsia_audio::ChannelConfig::kSurround51:
        return (out << "kSurround51");
      default:
        return (out << "unknown ChannelConfig enum");
    }
  }
  return (out << "NONE");
}

inline std::ostream& operator<<(std::ostream& out,
                                const fuchsia_audio_device::DeviceType& device_type) {
  switch (device_type) {
    case fuchsia_audio_device::DeviceType::kCodec:
      return (out << "kCodec");
    case fuchsia_audio_device::DeviceType::kComposite:
      return (out << "kComposite");
    default:
      return (out << "unknown DeviceType enum");
  }
}

}  // namespace

// fidl::AsyncEventHandler<> implementation, called when the server disconnects its channel.
template <typename ProtocolT>
void FidlHandler<ProtocolT>::on_fidl_error(fidl::UnbindInfo error) {
  if (!error.is_user_initiated() && !error.is_peer_closed() && !error.is_dispatcher_shutdown()) {
    std::cout << name_ << ":" << __func__ << ", shutting down... " << error << '\n';
  }
  parent_->Shutdown();
}

// static
std::optional<fidl::Client<fuchsia_audio_device::Registry>> MediaApp::registry_client_ =
    std::nullopt;
std::optional<fidl::SyncClient<fuchsia_audio_device::ControlCreator>>
    MediaApp::control_creator_client_ = std::nullopt;

MediaApp::MediaApp(async::Loop& loop, fit::closure quit_callback)
    : loop_(loop), quit_callback_(std::move(quit_callback)) {
  assert(quit_callback_);
}

void MediaApp::Run() {
  ConnectToRegistry();
  WaitForFirstAudioDevice();
}

void MediaApp::ConnectToRegistry() {
  if (!registry_client_.has_value()) {
    zx::result client_end = component::Connect<fuchsia_audio_device::Registry>();
    registry_client_ = fidl::Client<fuchsia_audio_device::Registry>(
        std::move(*client_end), loop_.dispatcher(), &reg_handler_);
  }
}

void MediaApp::WaitForFirstAudioDevice() {
  if (!registry_client_.has_value()) {
    Shutdown();
    return;
  }
  (*registry_client_)
      ->WatchDevicesAdded()
      .Then([this](fidl::Result<fuchsia_audio_device::Registry::WatchDevicesAdded>& result) {
        if (result.is_error()) {
          std::cout << "Registry/WatchDevicesAdded error: "
                    << result.error_value().FormatDescription() << '\n';
          Shutdown();
          return;
        }

        // Now that we have been notified of a device, log its information and configure the first
        // ring_buffer with the first supported ring_buffer_format.
        for (const auto& device : *result->devices()) {
          device_token_id_ = *device.token_id();

          if constexpr (kLogDeviceInfo) {
            std::cout << "Connecting to audio device:" << '\n';
            std::cout << "    token_id                  " << device_token_id_ << '\n';
            std::cout << "    device_type               " << *device.device_type() << '\n';
            std::cout << "    device_name               " << *device.device_name() << '\n';
            std::cout << "    manufacturer              " << device.manufacturer().value_or("NONE")
                      << '\n';
            std::cout << "    product                   " << device.product().value_or("NONE")
                      << '\n';

            std::string uid_str;
            if (!device.unique_instance_id()) {
              uid_str = "NONE";
            } else {
              uid_str = fxl::StringPrintf(
                  "0x%02X%02X%02X%02X%02X%02X%02X%02X%02X%02X%02X%02X%02X%02X%02X%02X",
                  (*device.unique_instance_id())[0], (*device.unique_instance_id())[1],
                  (*device.unique_instance_id())[2], (*device.unique_instance_id())[3],
                  (*device.unique_instance_id())[4], (*device.unique_instance_id())[5],
                  (*device.unique_instance_id())[6], (*device.unique_instance_id())[7],
                  (*device.unique_instance_id())[8], (*device.unique_instance_id())[9],
                  (*device.unique_instance_id())[10], (*device.unique_instance_id())[11],
                  (*device.unique_instance_id())[12], (*device.unique_instance_id())[13],
                  (*device.unique_instance_id())[14], (*device.unique_instance_id())[15]);
            }
            std::cout << "    unique_instance_id        " << uid_str << '\n';

            // Log ring_buffer_format_sets details
            std::cout << "    ring_buffer_format_sets[" << device.ring_buffer_format_sets()->size()
                      << "]" << '\n';
            for (auto idx = 0u; idx < device.ring_buffer_format_sets()->size(); ++idx) {
              auto element_ring_buffer_format_set = device.ring_buffer_format_sets()->at(idx);
              std::cout << "        [" << idx << "]   element_id  "
                        << *element_ring_buffer_format_set.element_id() << '\n';
              std::cout << "              format_set ["
                        << element_ring_buffer_format_set.format_sets()->size() << "]" << '\n';
              for (auto idx = 0u; idx < element_ring_buffer_format_set.format_sets()->size();
                   ++idx) {
                auto ring_buffer_format_set = element_ring_buffer_format_set.format_sets()->at(idx);
                std::cout << "                  [" << idx << "]  channel_sets ["
                          << ring_buffer_format_set.channel_sets()->size() << "]" << '\n';
                for (auto cs = 0u; cs < ring_buffer_format_set.channel_sets()->size(); ++cs) {
                  auto channel_set = (*ring_buffer_format_set.channel_sets())[cs];
                  std::cout << "                [" << cs << "]   attributes["
                            << channel_set.attributes()->size() << "]" << '\n';
                  for (auto a = 0u; a < channel_set.attributes()->size(); ++a) {
                    auto attribs = (*channel_set.attributes())[a];
                    std::cout << "                        [" << a << "]   min_frequency  "
                              << (attribs.min_frequency().has_value()
                                      ? std::to_string(*attribs.min_frequency())
                                      : "NONE")
                              << '\n';
                    std::cout << "                              max_frequency  "
                              << (attribs.max_frequency().has_value()
                                      ? std::to_string(*attribs.max_frequency())
                                      : "NONE")
                              << '\n';
                  }
                }
                std::cout << "            sample_types["
                          << ring_buffer_format_set.sample_types()->size() << "]" << '\n';
                for (auto st = 0u; st < ring_buffer_format_set.sample_types()->size(); ++st) {
                  std::cout << "                [" << st << "]     "
                            << (*ring_buffer_format_set.sample_types())[st] << '\n';
                }
                std::cout << "            frame_rates ["
                          << ring_buffer_format_set.frame_rates()->size() << "]" << '\n';
                for (auto fr = 0u; fr < ring_buffer_format_set.frame_rates()->size(); ++fr) {
                  std::cout << "                [" << fr << "]     "
                            << (*ring_buffer_format_set.frame_rates())[fr] << '\n';
                }
              }
            }

            if (device.dai_format_sets().has_value()) {
              std::cout << "    dai_format_sets           [" << device.dai_format_sets()->size()
                        << "]" << '\n';
              // Include more `dai_format_sets` info here.
            } else {
              std::cout << "    dai_format_sets           NONE" << '\n';
            }

            if (device.is_input().has_value()) {
              std::cout << "    is_input                  "
                        << (*device.is_input() ? "true" : "false") << '\n';
            } else {
              std::cout << "    is_input                  UNSPECIFIED" << '\n';
            }

            std::cout << "    plug_caps                 " << device.plug_detect_caps() << '\n';

            std::string clk_domain_str;
            if (!device.clock_domain()) {
              clk_domain_str = "unspecified (CLOCK_DOMAIN_EXTERNAL)";
            } else if (*device.clock_domain() == 0xFFFFFFFF) {
              clk_domain_str = "CLOCK_DOMAIN_EXTERNAL";
            } else if (*device.clock_domain() == 0) {
              clk_domain_str = "CLOCK_DOMAIN_MONOTONIC";
            } else {
              clk_domain_str = std::to_string(*device.clock_domain()) + " (not MONOTONIC)";
            }
            std::cout << "    clock_domain              " << clk_domain_str << '\n';

            if (device.signal_processing_elements().has_value()) {
              std::cout << "    signal_processing_elements ["
                        << device.signal_processing_elements()->size() << "]" << '\n';
              // Include more `signal_processing_elements` info here.
            } else {
              std::cout << "    signal_processing_elements NONE" << '\n';
            }

            if (device.signal_processing_topologies().has_value()) {
              std::cout << "    signal_processing_topologies ["
                        << device.signal_processing_topologies()->size() << "]" << '\n';
              // Include more `signal_processing_topologies` info here.
            } else {
              std::cout << "    signal_processing_topologies NONE" << '\n';
            }
          }

          // Now determine the format we will use, and connect more deeply to the device.
          if (!device.ring_buffer_format_sets() || device.ring_buffer_format_sets()->empty() ||
              !device.ring_buffer_format_sets()->front().format_sets() ||
              device.ring_buffer_format_sets()->front().format_sets()->empty() ||
              !device.ring_buffer_format_sets()->front().format_sets()->front().channel_sets() ||
              device.ring_buffer_format_sets()
                  ->front()
                  .format_sets()
                  ->front()
                  .channel_sets()
                  ->empty() ||
              !device.ring_buffer_format_sets()
                   ->front()
                   .format_sets()
                   ->front()
                   .channel_sets()
                   ->front()
                   .attributes() ||
              device.ring_buffer_format_sets()
                  ->front()
                  .format_sets()
                  ->front()
                  .channel_sets()
                  ->front()
                  .attributes()
                  ->empty()) {
            std::cout
                << "Cannot determine a channel-count from ring_buffer_format_sets (missing/empty)"
                << '\n';
            Shutdown();
            return;
          }
          // For convenience, just use the first channel configuration that the device listed.
          channels_per_frame_ = device.ring_buffer_format_sets()
                                    ->front()
                                    .format_sets()
                                    ->front()
                                    .channel_sets()
                                    ->front()
                                    .attributes()
                                    ->size();

          // If we didn't get a valid overall format, then we shouldn't continue onward.
          if (!channels_per_frame_) {
            return;
          }

          ObserveDevice();
          ConnectToControlCreator();
          if (ConnectToControl()) {
            ConnectToRingBuffer();
          }
          break;  // Only do this for the first device found.
        }
      });
}

void MediaApp::ObserveDevice() {
  if (!registry_client_.has_value()) {
    Shutdown();
    return;
  }
  zx::channel server_end, client_end;
  zx::channel::create(0, &server_end, &client_end);
  observer_client_ = fidl::Client<fuchsia_audio_device::Observer>(
      fidl::ClientEnd<fuchsia_audio_device::Observer>(std::move(client_end)), loop_.dispatcher(),
      &obs_handler_);

  (*registry_client_)
      ->CreateObserver({{
          .token_id = device_token_id_,
          .observer_server = fidl::ServerEnd<fuchsia_audio_device::Observer>(std::move(server_end)),
      }})
      .Then([this](fidl::Result<fuchsia_audio_device::Registry::CreateObserver>& result) {
        if (!result.is_ok()) {
          std::cout << "Registry/CreateObserver error: " << result.error_value().FormatDescription()
                    << '\n';
          Shutdown();
          return;
        }

        observer_client_->WatchPlugState().Then(
            [this](fidl::Result<fuchsia_audio_device::Observer::WatchPlugState>& result) {
              if (!result.is_ok()) {
                std::cout << "Observer/WatchPlugState error: "
                          << result.error_value().FormatDescription() << '\n';
                Shutdown();
              }

              std::cout << "PlugState" << '\n';
              std::cout << "    state:                    " << result->state() << '\n';
              std::cout << "    plug_time (nsec):         "
                        << (result->plug_time() ? std::to_string(*result->plug_time())
                                                : "NONE (non-compliant)")
                        << '\n';
            });
      });
}

// static
void MediaApp::ConnectToControlCreator() {
  if (!control_creator_client_.has_value()) {
    zx::result client_end = component::Connect<fuchsia_audio_device::ControlCreator>();
    control_creator_client_ =
        fidl::SyncClient<fuchsia_audio_device::ControlCreator>(std::move(*client_end));
  }
}

bool MediaApp::ConnectToControl() {
  if (!control_creator_client_.has_value()) {
    Shutdown();
    return false;
  }
  zx::channel server_end, client_end;
  zx::channel::create(0, &server_end, &client_end);
  control_client_ = fidl::Client<fuchsia_audio_device::Control>(
      fidl::ClientEnd<fuchsia_audio_device::Control>(std::move(client_end)), loop_.dispatcher(),
      &ctl_handler_);

  auto result = (*control_creator_client_)
                    ->Create({{
                        .token_id = device_token_id_,
                        .control_server =
                            fidl::ServerEnd<fuchsia_audio_device::Control>(std::move(server_end)),
                    }});
  if (!result.is_ok()) {
    std::cout << "ControlCreator/Create error: " << result.error_value().FormatDescription()
              << '\n';
    Shutdown();
    return false;
  }

  return true;
}

// Create a RingBuffer and play a tone in that format to that RingBuffer.
void MediaApp::ConnectToRingBuffer() {
  zx::channel server_end, client_end;
  zx::channel::create(0, &server_end, &client_end);
  ring_buffer_client_ = fidl::Client<fuchsia_audio_device::RingBuffer>(
      fidl::ClientEnd<fuchsia_audio_device::RingBuffer>(std::move(client_end)), loop_.dispatcher(),
      &rb_handler_);

  control_client_
      ->CreateRingBuffer({{
          .options = fuchsia_audio_device::RingBufferOptions{{
              .format = fuchsia_audio::Format{{
                  .sample_type = kSampleFormat,
                  .channel_count = channels_per_frame_,
                  .frames_per_second = kFrameRate,
              }},
              .ring_buffer_min_bytes = 4096,
          }},
          .ring_buffer_server =
              fidl::ServerEnd<fuchsia_audio_device::RingBuffer>(std::move(server_end)),
      }})
      .Then([this](fidl::Result<fuchsia_audio_device::Control::CreateRingBuffer>& result) {
        if (!result.is_ok()) {
          std::cout << "Control/CreateRingBuffer error: "
                    << result.error_value().FormatDescription() << '\n';
          Shutdown();
          return;
        }
        assert(result->properties() && result->ring_buffer());
        assert(result->properties()->valid_bits_per_sample() &&
               result->properties()->turn_on_delay());
        assert(result->ring_buffer()->buffer() && result->ring_buffer()->format() &&
               result->ring_buffer()->producer_bytes() && result->ring_buffer()->consumer_bytes() &&
               result->ring_buffer()->reference_clock());

        ring_buffer_ = std::move(*result->ring_buffer());
        const auto fmt = ring_buffer_.format();

        std::cout << "properties:" << '\n';
        std::cout << "    valid_bits_per_sample:    "
                  << static_cast<uint32_t>(*result->properties()->valid_bits_per_sample()) << '\n';
        std::cout << "    turn_on_delay (nsec):     " << *result->properties()->turn_on_delay()
                  << '\n';
        std::cout << "ring_buffer:" << '\n';
        std::cout << "    buffer:" << '\n';
        std::cout << "        vmo (handle):         0x" << std::hex
                  << ring_buffer_.buffer()->vmo().get() << std::dec << '\n';
        std::cout << "        size:                 " << ring_buffer_.buffer()->size() << '\n';
        std::cout << "    format:" << '\n';
        std::cout << "        sample_type:          " << fmt->sample_type() << '\n';
        std::cout << "        channel_count:        " << *fmt->channel_count() << '\n';
        std::cout << "        frames_per_second:    " << *fmt->frames_per_second() << '\n';
        std::cout << "        channel_layout:       " << fmt->channel_layout() << '\n';
        std::cout << "    producer_bytes:           " << *ring_buffer_.producer_bytes() << '\n';
        std::cout << "    consumer_bytes:           " << *ring_buffer_.consumer_bytes() << '\n';
        std::cout << "    reference_clock (handle): 0x" << std::hex
                  << ring_buffer_.reference_clock()->get() << std::dec << '\n';
        std::string clk_domain_str;
        if (!ring_buffer_.reference_clock_domain()) {
          clk_domain_str = "unspecified (CLOCK_DOMAIN_EXTERNAL)";
        } else if (*ring_buffer_.reference_clock_domain() == 0xFFFFFFFF) {
          clk_domain_str = "CLOCK_DOMAIN_EXTERNAL";
        } else if (*ring_buffer_.reference_clock_domain() == 0) {
          clk_domain_str = "CLOCK_DOMAIN_MONOTONIC";
        } else {
          clk_domain_str =
              std::to_string(*ring_buffer_.reference_clock_domain()) + " (not MONOTONIC)";
        }
        std::cout << "    reference_clock_domain:   " << clk_domain_str << '\n';

        if (!MapRingBufferVmo()) {
          Shutdown();
          return;
        }
        if constexpr (kAutoplaySinusoid) {
          WriteAudioToVmo();
        }
        StartRingBuffer();
      });
}

namespace {

fbl::RefPtr<fzl::VmarManager>* CreateVmarManager() {
  constexpr size_t kSize = 16ull * 1024 * 1024 * 1024;
  constexpr zx_vm_option_t kFlags = ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_ALIGN_1GB;

  auto ptr = new fbl::RefPtr<fzl::VmarManager>;
  *ptr = fzl::VmarManager::Create(kSize, nullptr, kFlags);
  return ptr;
}
const fbl::RefPtr<fzl::VmarManager>* const vmar_manager = CreateVmarManager();

}  // namespace

// Validate and map the VMO.
bool MediaApp::MapRingBufferVmo() {
  auto buffer = std::move(*ring_buffer_.buffer());
  ring_buffer_size_ = buffer.size();

  zx_info_vmo_t info;
  if (auto status = buffer.vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr);
      status != ZX_OK) {
    std::cout << "WARNING: vmo.get_info failed: " << std::to_string(status) << '\n';
    return false;
  }
  if ((info.flags & ZX_INFO_VMO_RESIZABLE) != 0) {
    std::cout << "WARNING: vmo is resizable, which is not permittted" << '\n';
    return false;
  }
  if ((info.flags & ZX_INFO_VMO_PAGER_BACKED) != 0) {
    std::cout << "WARNING: vmo is pager-backed, which is not permitted" << '\n';
    return false;
  }

  // The VMO must allow mapping with appropriate permissions.
  zx_rights_t expected_rights = ZX_RIGHT_READ | ZX_RIGHT_MAP | ZX_RIGHT_WRITE;
  if ((info.handle_rights & expected_rights) != expected_rights) {
    std::cout << "WARNING: invalid rights = 0x" << std::hex << std::setw(4) << std::setfill('0')
              << info.handle_rights << " (required rights = 0x" << std::setw(4) << std::setfill('0')
              << expected_rights << ")" << std::dec << '\n';
    return false;
  }
  std::cout << "Mapping a mem.Buffer/size of " << ring_buffer_size_ << '\n';

  // Map.
  zx_vm_option_t flags = ZX_VM_PERM_READ | ZX_VM_PERM_WRITE;
  if (auto status =
          ring_buffer_mapper_.Map(buffer.vmo(), 0, ring_buffer_size_, flags, *vmar_manager);
      status != ZX_OK) {
    std::cout << "WARNING: VmpMapper.Map failed with status=" << std::to_string(status) << '\n';
    return false;
  }

  void* vmo_start = ring_buffer_mapper_.start();
  void* vmo_end = static_cast<char*>(vmo_start) + ring_buffer_size_;
  rb_start_ = static_cast<int16_t*>(vmo_start);
  std::cout << "Mapped a VMO of size: 0x" << std::hex << ring_buffer_size_ << ", to [0x"
            << vmo_start << ", 0x" << vmo_end << ")" << std::dec << '\n';
  return true;
}

// Write a .125-amplitude sinusoid that can loop around the ring buffer.
void MediaApp::WriteAudioToVmo() {
  // Floor this to integer, to get a perfectly continuous signal across ring-buffer wraparound.
  auto cycles_per_buffer =
      static_cast<uint16_t>(static_cast<float>(ring_buffer_size_) / kApproxFramesPerCycle /
                            static_cast<float>(kBytesPerSample * channels_per_frame_));

  std::cout << "Writing " << cycles_per_buffer << " cycles in this " << ring_buffer_size_
            << "-byte buffer: a "
            << (static_cast<double>(channels_per_frame_ * kFrameRate * cycles_per_buffer *
                                    kBytesPerSample) /
                static_cast<double>(ring_buffer_size_))
            << "-hz tone (estimated " << kApproxToneFrequency << "-hz)" << '\n';
  for (size_t idx = 0; idx < ring_buffer_size_ / (kBytesPerSample * channels_per_frame_); ++idx) {
    auto val = static_cast<int16_t>(
        32768.0f * kToneAmplitude *
        std::sin(static_cast<double>(idx * 2 * (kBytesPerSample * channels_per_frame_) *
                                     cycles_per_buffer) *
                 M_PI / static_cast<double>(ring_buffer_size_)));
    for (size_t chan = 0; chan < channels_per_frame_; ++chan) {
      rb_start_[(idx * channels_per_frame_) + chan] = val;
    }
  }
}

void MediaApp::StartRingBuffer() {
  ring_buffer_client_->Start({}).Then(
      [this](fidl::Result<fuchsia_audio_device::RingBuffer::Start>& result) {
        if (!result.is_ok()) {
          std::cout << "RingBuffer/Start error: " << result.error_value().FormatDescription()
                    << '\n';
          Shutdown();
          return;
        }
        std::cout << "RingBuffer/Start is_ok, playback has begun" << '\n';
      });
}

void MediaApp::StopRingBuffer() {
  ring_buffer_client_->Stop({}).Then(
      [this](fidl::Result<fuchsia_audio_device::RingBuffer::Stop>& result) {
        if (!result.is_ok()) {
          std::cout << "RingBuffer/Stop error: " << result.error_value().FormatDescription()
                    << '\n';
        } else {
          std::cout << "RingBuffer/Stop is_ok: success!" << '\n';
        }

        Shutdown();
      });
}

// Unmap memory, quit message loop (FIDL interfaces auto-delete upon ~MediaApp).
void MediaApp::Shutdown() { quit_callback_(); }

}  // namespace examples

int main(int argc, const char** argv) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  auto startup_context = sys::ComponentContext::CreateAndServeOutgoingDirectory();

  examples::MediaApp media_app(
      loop, [&loop]() { async::PostTask(loop.dispatcher(), [&loop]() { loop.Quit(); }); });
  media_app.Run();

  loop.Run();  // Now wait for the message loop to return...

  return 0;
}
