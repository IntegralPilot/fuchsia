// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/drivers/aml-g12-tdm/composite-server.h"

#include <fidl/fuchsia.hardware.audio.signalprocessing/cpp/common_types.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/memory_barriers/memory_barriers.h>
#include <lib/trace/event_args.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>

#include <algorithm>
#include <numeric>
#include <string>

#include <fbl/algorithm.h>

#include "src/media/audio/drivers/lib/inspect/recorder.h"

namespace audio::aml_g12 {

// In C++23 we can remove this and just use std::ranges::contains.
template <typename Container, typename T>
bool contains(Container c, T v) {
  return std::find(std::begin(c), std::end(c), v) != std::end(c);
}

// The Composite interface returns DriverError upon error, not zx_status_t, so convert any error.
fuchsia_hardware_audio::DriverError ZxStatusToDriverError(zx_status_t status) {
  switch (status) {
    case ZX_ERR_NOT_SUPPORTED:
      return fuchsia_hardware_audio::DriverError::kNotSupported;
    case ZX_ERR_INVALID_ARGS:
      return fuchsia_hardware_audio::DriverError::kInvalidArgs;
    case ZX_ERR_WRONG_TYPE:
      return fuchsia_hardware_audio::DriverError::kWrongType;
    case ZX_ERR_SHOULD_WAIT:
      return fuchsia_hardware_audio::DriverError::kShouldWait;
    default:
      return fuchsia_hardware_audio::DriverError::kInternalError;
  }
}

AudioCompositeServer::AudioCompositeServer(
    std::array<std::optional<fdf::MmioBuffer>, kNumberOfTdmEngines> mmios, zx::bti bti,
    async_dispatcher_t* dispatcher, metadata::AmlVersion aml_version,
    fidl::WireSyncClient<fuchsia_hardware_clock::Clock> clock_gate_client,
    fidl::WireSyncClient<fuchsia_hardware_clock::Clock> pll_client,
    std::vector<SclkPin> sclk_clients, std::unique_ptr<Recorder> recorder)
    : dispatcher_(dispatcher),
      bti_(std::move(bti)),
      clock_gate_(std::move(clock_gate_client)),
      pll_(std::move(pll_client)),
      sclk_clients_(std::move(sclk_clients)),
      recorder_(std::move(recorder)) {
  for (const fuchsia_hardware_audio::ElementId& dai : kDaiIds) {
    element_completers_[dai].first_response_sent = false;
    element_completers_[dai].completer = {};
  }
  for (const fuchsia_hardware_audio::ElementId& ring_buffer : kRingBufferIds) {
    element_completers_[ring_buffer].first_response_sent = false;
    element_completers_[ring_buffer].completer = {};
  }
  topology_completer_.first_response_sent = false;

  for (size_t i = 0; i < kNumberOfPipelines; ++i) {
    // Default supported DAI formats.
    supported_dai_formats_[i].number_of_channels(
        AmlTdmConfigDevice::GetSupportedNumberOfChannels());
    supported_dai_formats_[i].sample_formats(AmlTdmConfigDevice::GetFidlSupportedSampleFormats());
    supported_dai_formats_[i].frame_formats(AmlTdmConfigDevice::GetFidlSupportedFrameFormats());
    supported_dai_formats_[i].frame_rates(AmlTdmConfigDevice::GetSupportedFrameRates());
    supported_dai_formats_[i].bits_per_slot(AmlTdmConfigDevice::GetSupportedBitsPerSlot());
    supported_dai_formats_[i].bits_per_sample(AmlTdmConfigDevice::GetSupportedBitsPerSample());

    // Take values from supported list to define current DAI format.
    current_dai_formats_[i].emplace(
        supported_dai_formats_[i].number_of_channels()[0],
        (1 << supported_dai_formats_[i].number_of_channels()[0]) - 1,  // enable all channels.
        supported_dai_formats_[i].sample_formats()[0], supported_dai_formats_[i].frame_formats()[0],
        supported_dai_formats_[i].frame_rates()[0], supported_dai_formats_[i].bits_per_slot()[0],
        supported_dai_formats_[i].bits_per_sample()[0]);
  }

  ZX_ASSERT(StartSocPower(/*wait_for_completion*/ true) == ZX_OK);

  // Output engines.
  ZX_ASSERT(ConfigEngine(0, 0, false, std::move(mmios[0].value()), aml_version) == ZX_OK);
  ZX_ASSERT(ConfigEngine(1, 1, false, std::move(mmios[1].value()), aml_version) == ZX_OK);
  ZX_ASSERT(ConfigEngine(2, 2, false, std::move(mmios[2].value()), aml_version) == ZX_OK);

  // Input engines.
  ZX_ASSERT(ConfigEngine(3, 0, true, std::move(mmios[3].value()), aml_version) == ZX_OK);
  ZX_ASSERT(ConfigEngine(4, 1, true, std::move(mmios[4].value()), aml_version) == ZX_OK);
  ZX_ASSERT(ConfigEngine(5, 2, true, std::move(mmios[5].value()), aml_version) == ZX_OK);
  // Unconditional reset on construction.
  for (size_t i = 0; i < kNumberOfTdmEngines; ++i) {
    ZX_ASSERT(ResetEngine(i) == ZX_OK);
  }

  // Make sure the DMAs are stopped before releasing quarantine.
  for (auto& engine : engines_) {
    engine.device->Stop();
  }
  // Make sure that all reads/writes have gone through.
  BarrierBeforeRelease();
  ZX_ASSERT(bti_.release_quarantine() == ZX_OK);

  // We may not need to StartSocPower above -- it might suffice to just power-down here.
  ZX_ASSERT(StopSocPower() == ZX_OK);
}

zx_status_t AudioCompositeServer::ConfigEngine(size_t index, size_t dai_index, bool input,
                                               fdf::MmioBuffer mmio,
                                               metadata::AmlVersion aml_version) {
  // Common configuration.
  engines_[index].config = {};
  engines_[index].config.version = aml_version;
  engines_[index].config.mClockDivFactor = 10;
  engines_[index].config.sClockDivFactor = 25;

  // Supported ring buffer formats.
  // We take some values from supported DAI formats.
  fuchsia_hardware_audio::PcmSupportedFormats pcm_formats;

  // Vector with number_of_channels empty attributes supported in Ring Buffer
  // equal to the number of channels supported on DAI.
  std::vector<fuchsia_hardware_audio::ChannelSet> channel_sets;
  for (auto& number_of_channels : supported_dai_formats_[dai_index].number_of_channels()) {
    std::vector<fuchsia_hardware_audio::ChannelAttributes> attributes(number_of_channels);
    fuchsia_hardware_audio::ChannelSet channel_set;
    channel_set.attributes(std::move(attributes));
    channel_sets.emplace_back(std::move(channel_set));
  }
  pcm_formats.channel_sets(std::move(channel_sets));

  // Frame rates supported on DAI are supported in Ring Buffer.
  pcm_formats.frame_rates(supported_dai_formats_[dai_index].frame_rates());

  // Sample format is PCM signed.
  pcm_formats.sample_formats(std::vector{fuchsia_hardware_audio::SampleFormat::kPcmSigned});

  // Bits per slot supported on Ring Buffer.
  pcm_formats.bytes_per_sample(AmlTdmConfigDevice::GetSupportedRingBufferBytesPerSlot());

  // Valid bits per sample supported on Ring Buffer.
  auto v = AmlTdmConfigDevice::GetSupportedRingBufferBytesPerSlot();
  std::transform(v.begin(), v.end(), v.begin(),
                 [](const uint8_t bytes_per_slot) -> uint8_t { return bytes_per_slot * 8; });
  pcm_formats.valid_bits_per_sample(v);

  supported_ring_buffer_formats_[index] = std::move(pcm_formats);

  engines_[index].ring_buffer_index = index;
  engines_[index].dai_index = dai_index;
  engines_[index].config.is_input = input;

  switch (dai_index) {
      // clang-format off
    case 0: engines_[index].config.bus = metadata::AmlBus::TDM_A; break;
    case 1: engines_[index].config.bus = metadata::AmlBus::TDM_B; break;
    case 2: engines_[index].config.bus = metadata::AmlBus::TDM_C; break;
      // clang-format on
    default:
      return ZX_ERR_INVALID_ARGS;
  }

  engines_[index].device.emplace(engines_[index].config, std::move(mmio));

  // Unconditional reset on configuration.
  return ResetEngine(index);
}

zx_status_t AudioCompositeServer::ResetEngine(size_t index) {
  // Resets engine using AmlTdmConfigDevice, so we need to translate the current state
  // into parameters used by AmlTdmConfigDevice.
  ZX_ASSERT(engines_[index].dai_index < kNumberOfPipelines);
  ZX_ASSERT(current_dai_formats_[engines_[index].dai_index].has_value());
  const fuchsia_hardware_audio::DaiFormat& dai_format =
      *current_dai_formats_[engines_[index].dai_index];
  if (dai_format.sample_format() != fuchsia_hardware_audio::DaiSampleFormat::kPcmSigned) {
    fdf::error("Sample format not supported");
    return ZX_ERR_NOT_SUPPORTED;
  }
  auto& dai_type = engines_[index].config.dai.type;
  using StandardFormat = fuchsia_hardware_audio::DaiFrameFormatStandard;
  using DaiType = metadata::DaiType;
  if (dai_format.frame_format().frame_format_standard().has_value()) {
    switch (dai_format.frame_format().frame_format_standard().value()) {
        // clang-format off
      case StandardFormat::kI2S:        dai_type = DaiType::I2s;                 break;
      case StandardFormat::kStereoLeft: dai_type = DaiType::StereoLeftJustified; break;
      case StandardFormat::kTdm1:       dai_type = DaiType::Tdm1;                break;
      case StandardFormat::kTdm2:       dai_type = DaiType::Tdm2;                break;
      case StandardFormat::kTdm3:       dai_type = DaiType::Tdm3;                break;
        // clang-format on
      case StandardFormat::kNone:
        [[fallthrough]];
      case StandardFormat::kStereoRight:
        [[fallthrough]];
      default:
        fdf::error("Frame format not supported");
        return ZX_ERR_NOT_SUPPORTED;
    }
  } else if (dai_format.frame_format().frame_format_custom().has_value()) {
    dai_type = DaiType::Custom;
    if (!dai_format.frame_format().frame_format_custom()->left_justified()) {
      fdf::error("Non-left justified custom formats not supported");
      return ZX_ERR_NOT_SUPPORTED;
    }
    engines_[index].config.dai.custom_sclk_on_raising =
        dai_format.frame_format().frame_format_custom()->sclk_on_raising();

    engines_[index].config.dai.custom_frame_sync_sclks_offset =
        dai_format.frame_format().frame_format_custom()->frame_sync_sclks_offset();
    if (engines_[index].config.dai.custom_frame_sync_sclks_offset !=
        AmlTdmConfigDevice::GetSupportedCustomFrameSyncSclksOffset()) {
      fdf::error("Sync sclks offset not supported");
      return ZX_ERR_NOT_SUPPORTED;
    }

    engines_[index].config.dai.custom_frame_sync_size =
        dai_format.frame_format().frame_format_custom()->frame_sync_size();
    if (engines_[index].config.dai.custom_frame_sync_size !=
        AmlTdmConfigDevice::GetSupportedCustomFrameSyncSize()) {
      fdf::error("Frame sync size not supported");
      return ZX_ERR_NOT_SUPPORTED;
    }
  } else {
    fdf::error("No standard or custom frame format");
    return ZX_ERR_NOT_SUPPORTED;
  }
  engines_[index].config.dai.bits_per_sample = dai_format.bits_per_sample();
  engines_[index].config.dai.bits_per_slot = dai_format.bits_per_slot();
  engines_[index].config.dai.number_of_channels =
      static_cast<uint8_t>(dai_format.number_of_channels());
  engines_[index].config.ring_buffer.number_of_channels =
      engines_[index].config.dai.number_of_channels;
  // AMLogic allows channel swapping with a channel number per nibble in registers like
  // EE_AUDIO_TDMOUT_A_SWAP and EE_AUDIO_TDMIN_A_SWAP.
  constexpr uint32_t kNoSwaps = 0x76543210;  // Default channel numbers for no swaps.
  engines_[index].config.swaps = kNoSwaps;
  size_t lane = engines_[index].config.is_input ? 1 : 0;
  engines_[index].config.lanes_enable_mask[lane] =
      (1 << dai_format.number_of_channels()) - 1;  // enable all channels.

  zx_status_t status = AmlTdmConfigDevice::Normalize(engines_[index].config);
  if (status != ZX_OK) {
    fdf::error("Failed to normalize config: {}", zx_status_get_string(status));
    return status;
  }

  {
    auto stop_if_temporarily_started = TemporarilyStartSocPower();
    status = engines_[index].device->InitHW(
        engines_[index].config, dai_format.channels_to_use_bitmask(), dai_format.frame_rate());
    if (status != ZX_OK) {
      fdf::error("Failed to init hardware: {}", zx_status_get_string(status));
    }
  }
  return status;
}

void AudioCompositeServer::Reset(ResetCompleter::Sync& completer) {
  for (size_t i = 0; i < kNumberOfTdmEngines; ++i) {
    if (zx_status_t status = ResetEngine(i); status != ZX_OK) {
      completer.Reply(zx::error(ZxStatusToDriverError(status)));
      return;
    }
  }
  completer.Reply(zx::ok());
}

void AudioCompositeServer::GetProperties(
    fidl::Server<fuchsia_hardware_audio::Composite>::GetPropertiesCompleter::Sync& completer) {
  fuchsia_hardware_audio::CompositeProperties props{{
      .manufacturer = "Amlogic",
      .product = "g12",
      .clock_domain = fuchsia_hardware_audio::kClockDomainMonotonic,
  }};
  completer.Reply(std::move(props));
}

void AudioCompositeServer::GetHealthState(GetHealthStateCompleter::Sync& completer) {
  completer.Reply(fuchsia_hardware_audio::HealthState{}.healthy(true));
}

// Note that if already bound, we close the NEW channel (not the one on which we were called).
void AudioCompositeServer::SignalProcessingConnect(
    SignalProcessingConnectRequest& request, SignalProcessingConnectCompleter::Sync& completer) {
  if (signal_) {
    request.protocol().Close(ZX_ERR_ALREADY_BOUND);
    return;
  }

  // Reset all completion state related to signalprocessing.
  topology_completer_.completer.reset();
  topology_completer_.first_response_sent = false;
  for (auto& element_entry : element_completers_) {
    element_entry.second.completer.reset();
    element_entry.second.first_response_sent = false;
  }

  signal_.emplace(dispatcher(), std::move(request.protocol()), this,
                  std::mem_fn(&AudioCompositeServer::OnSignalProcessingClosed));
}

void AudioCompositeServer::OnSignalProcessingClosed(fidl::UnbindInfo info) {
  if (info.is_peer_closed()) {
    fdf::info("Client disconnected");
  } else if (!info.is_user_initiated()) {
    // Do not log canceled cases; these happen particularly frequently in certain test cases.
    if (info.status() != ZX_ERR_CANCELED) {
      fdf::error("Client connection unbound: {}", info.status_string());
    }
  }
  if (signal_) {
    signal_.reset();
  }
}

void AudioCompositeServer::GetRingBufferFormats(GetRingBufferFormatsRequest& request,
                                                GetRingBufferFormatsCompleter::Sync& completer) {
  auto ring_buffer =
      std::find(kRingBufferIds.begin(), kRingBufferIds.end(), request.processing_element_id());
  if (ring_buffer == kRingBufferIds.end()) {
    fdf::error("Unknown Ring Buffer id ({}) for format retrieval", request.processing_element_id());
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }
  size_t ring_buffer_index = ring_buffer - kRingBufferIds.begin();
  ZX_ASSERT(ring_buffer_index < kNumberOfTdmEngines);

  auto formats_entry = fuchsia_hardware_audio::SupportedFormats2::WithPcmSupportedFormats(
      supported_ring_buffer_formats_[ring_buffer_index]);
  completer.Reply(zx::ok(std::vector{std::move(formats_entry)}));
}

void AudioCompositeServer::CreateRingBuffer(CreateRingBufferRequest& request,
                                            CreateRingBufferCompleter::Sync& completer) {
  auto ring_buffer =
      std::find(kRingBufferIds.begin(), kRingBufferIds.end(), request.processing_element_id());
  if (ring_buffer == kRingBufferIds.end()) {
    fdf::error("Unknown Ring Buffer id ({}) for creation", request.processing_element_id());
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }
  size_t ring_buffer_index = ring_buffer - kRingBufferIds.begin();
  ZX_ASSERT(ring_buffer_index < kNumberOfTdmEngines);
  auto& supported = supported_ring_buffer_formats_[ring_buffer_index];
  if (request.format().Which() != fuchsia_hardware_audio::Format2::Tag::kPcmFormat) {
    fdf::error("No PCM formats provided for Ring Buffer id ({})", request.processing_element_id());
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }

  ZX_ASSERT(supported.channel_sets().has_value());
  ZX_ASSERT(supported.sample_formats().has_value());
  ZX_ASSERT(supported.bytes_per_sample().has_value());
  ZX_ASSERT(supported.valid_bits_per_sample().has_value());
  ZX_ASSERT(supported.frame_rates().has_value());

  auto& requested = request.format().pcm_format().value();

  bool number_of_channels_found = false;
  for (auto& supported_channel_set : *supported.channel_sets()) {
    ZX_ASSERT(supported_channel_set.attributes().has_value());
    if (supported_channel_set.attributes()->size() == requested.number_of_channels()) {
      number_of_channels_found = true;
      break;
    }
  }
  if (!number_of_channels_found) {
    fdf::error("Ring Buffer number of channels for Ring Buffer id ({}) not supported",
               request.processing_element_id());
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }

  if (!contains(*supported.sample_formats(), requested.sample_format())) {
    fdf::error("Ring Buffer sample format for Ring Buffer id ({}) not supported",
               request.processing_element_id());
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }

  if (!contains(*supported.bytes_per_sample(), requested.bytes_per_sample())) {
    fdf::error("Ring Buffer bytes per sample for Ring Buffer id ({}) not supported",
               request.processing_element_id());
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }

  if (!contains(*supported.valid_bits_per_sample(), requested.valid_bits_per_sample())) {
    fdf::error("Ring Buffer valid bits per sample for Ring Buffer id ({}) not supported",
               request.processing_element_id());
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }

  if (!contains(*supported.frame_rates(), requested.frame_rate())) {
    fdf::error("Ring Buffer frame rate for Ring Buffer id ({}) not supported",
               request.processing_element_id());
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }

  auto& engine = engines_[ring_buffer_index];
  if (engine.ring_buffer) {
    // If it exists, we unbind the previous ring buffer channel with a ZX_ERR_NO_RESOURCES
    // epitaph to convey that the previous ring buffer resource is not there anymore.
    engine.ring_buffer->Unbind(ZX_ERR_NO_RESOURCES);
  }

  // RingBuffer::SetActiveChannels is optional; we must work well for clients who _never_ call it.
  //
  // The hardware is put into the powered-off state when initially configured, until the first
  // RingBuffer is created. At that point the hardware powers-up automatically, so that clients who
  // do not use SetActiveChannels get the expected behavior.
  ZX_ASSERT(StartSocPower(/*wait_for_completion*/ true) == ZX_OK);

  // Engine is on by default.
  engines_on_.set(ring_buffer_index, true);
  engine.ring_buffer_format = request.format();
  engine.ring_buffer = RingBufferServer::CreateRingBufferServer(
      dispatcher(), *this, ring_buffer_index, std::move(request.ring_buffer()),
      request.processing_element_id());
  completer.Reply(zx::ok());
}

void AudioCompositeServer::GetDaiFormats(GetDaiFormatsRequest& request,
                                         GetDaiFormatsCompleter::Sync& completer) {
  auto dai = std::find(kDaiIds.begin(), kDaiIds.end(), request.processing_element_id());
  if (dai == kDaiIds.end()) {
    fdf::error("Unknown DAI id ({}) for GetDaiFormats", request.processing_element_id());
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }
  size_t dai_index = dai - kDaiIds.begin();
  ZX_ASSERT(dai_index < kNumberOfPipelines);
  completer.Reply(zx::ok(std::vector{supported_dai_formats_[dai_index]}));
}

void AudioCompositeServer::SetDaiFormat(SetDaiFormatRequest& request,
                                        SetDaiFormatCompleter::Sync& completer) {
  auto dai = std::find(kDaiIds.begin(), kDaiIds.end(), request.processing_element_id());
  if (dai == kDaiIds.end()) {
    fdf::error("Unknown DAI id ({}) for SetDaiFormat", request.processing_element_id());
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }
  size_t dai_index = dai - kDaiIds.begin();
  ZX_ASSERT(dai_index < kNumberOfPipelines);
  auto& supported = supported_dai_formats_[dai_index];

  if (!contains(supported.number_of_channels(), request.format().number_of_channels())) {
    fdf::error("DAI format number of channels for DAI id ({}) not supported",
               request.processing_element_id());
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }

  if (request.format().channels_to_use_bitmask() >= (1u << request.format().number_of_channels())) {
    fdf::error("DAI format channels-to-use 0x{:x} out of range, for DAI id ({})",
               request.format().channels_to_use_bitmask(), request.processing_element_id());
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }

  if (!contains(supported.sample_formats(), request.format().sample_format())) {
    fdf::error("DAI format sample format for DAI id ({}) not supported",
               request.processing_element_id());
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }

  if (!contains(supported.frame_formats(), request.format().frame_format())) {
    fdf::error("DAI format frame format for DAI id ({}) not supported",
               request.processing_element_id());
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }

  if (!contains(supported.frame_rates(), request.format().frame_rate())) {
    fdf::error("DAI format frame rate for DAI id ({}) not supported",
               request.processing_element_id());
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }

  if (!contains(supported.bits_per_slot(), request.format().bits_per_slot())) {
    fdf::error("DAI format bits per slot for DAI id ({}) not supported",
               request.processing_element_id());
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }

  if (!contains(supported.bits_per_sample(), request.format().bits_per_sample())) {
    fdf::error("DAI format bits per sample for DAI id ({}) not supported",
               request.processing_element_id());
    completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kInvalidArgs));
    return;
  }
  current_dai_formats_[dai_index] = request.format();
  for (size_t i = 0; i < kNumberOfTdmEngines; ++i) {
    if (engines_[i].dai_index == dai_index) {
      if (zx_status_t status = ResetEngine(i); status != ZX_OK) {
        completer.Reply(zx::error(ZxStatusToDriverError(status)));
        return;
      }
    }
  }
  completer.Reply(zx::ok());
}

void AudioCompositeServer::GetPacketStreamFormats(
    GetPacketStreamFormatsRequest& request, GetPacketStreamFormatsCompleter::Sync& completer) {
  // Packet streams are not supported.
  completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kNotSupported));
}

void AudioCompositeServer::CreatePacketStream(CreatePacketStreamRequest& request,
                                              CreatePacketStreamCompleter::Sync& completer) {
  // Packet streams are not supported.
  completer.Reply(zx::error(fuchsia_hardware_audio::DriverError::kNotSupported));
}

zx_status_t AudioCompositeServer::StartSocPower(bool wait_for_completion) {
  TRACE_DURATION("power-audio", "aml-g12-audio-composite::StartSocPower");
  auto call_time = zx::clock::get_monotonic();

  // Only if needed (not done previously) so voting on relevant clock ids is not repeated.
  // Each driver instance (audio or any other) may vote independently.
  if (soc_power_started_) {
    TRACE_INSTANT("power-audio", "aml-g12-audio-composite::StartSocPower exit", TRACE_SCOPE_PROCESS,
                  "status", ZX_OK, "reason", "Already started");
    fdf::info("SoC power already started");
    return ZX_OK;
  }
  fdf::info("Starting SoC power");
  fidl::WireResult clock_gate_result = clock_gate_->Enable();
  if (!clock_gate_result.ok()) {
    fdf::error("Failed to send request to enable clock gate: {}",
               clock_gate_result.status_string());
    TRACE_INSTANT("power-audio", "aml-g12-audio-composite::StartSocPower exit", TRACE_SCOPE_PROCESS,
                  "status", clock_gate_result.status(), "reason",
                  "Could not send request to enable clock gate");
    return clock_gate_result.status();
  }
  if (clock_gate_result->is_error()) {
    fdf::error("Send request to enable clock gate error: {}",
               zx_status_get_string(clock_gate_result->error_value()));
    TRACE_INSTANT("power-audio", "aml-g12-audio-composite::StartSocPower exit", TRACE_SCOPE_PROCESS,
                  "status", clock_gate_result->error_value(), "reason",
                  "Could not enable clock gate");
    return clock_gate_result->error_value();
  }
  fidl::WireResult pll_result = pll_->Enable();
  if (!pll_result.ok()) {
    fdf::error("Failed to send request to enable PLL: {}", pll_result.status_string());
    TRACE_INSTANT("power-audio", "aml-g12-audio-composite::StartSocPower exit", TRACE_SCOPE_PROCESS,
                  "status", pll_result.status(), "reason", "Could not send request to enable PLL");
    return pll_result.status();
  }
  if (pll_result->is_error()) {
    fdf::error("Send request to enable PLL error: {}",
               zx_status_get_string(pll_result->error_value()));
    TRACE_INSTANT("power-audio", "aml-g12-audio-composite::StartSocPower exit", TRACE_SCOPE_PROCESS,
                  "status", pll_result->error_value(), "reason", "Could not enable PLL");
    return pll_result->error_value();
  }
  if (wait_for_completion) {
    zx::nanosleep(zx::deadline_after(kTimeToStabilizePll));
  }

  constexpr uint32_t kSclkAltFunction = 1;

  fidl::Arena arena;
  const auto sclk_function_config =
      fuchsia_hardware_pin::wire::Configuration::Builder(arena).function(kSclkAltFunction).Build();

  for (auto& sclk_client : sclk_clients_) {
    fidl::WireResult result = sclk_client.pin->Configure(sclk_function_config);
    if (!result.ok()) {
      fdf::error("Failed to send request to set GPIO function: {}", result.status_string());
      TRACE_INSTANT("power-audio", "aml-g12-audio-composite::StartSocPower exit",
                    TRACE_SCOPE_PROCESS, "status", result.status(), "reason",
                    "Could not set GPIO function");
      return result.status();
    }
  }
  TRACE_ASYNC_END("aml-g12-composite", "suspend", trace_async_id_);
  TRACE_INSTANT("power-audio", "aml-g12-audio-composite::StartSocPower exit", TRACE_SCOPE_PROCESS,
                "status", ZX_OK, "reason", "Successfully powered up");
  soc_power_started_ = true;
  last_started_time_ = zx::clock::get_monotonic();
  device_inspect()->RecordSocPowerUp(call_time, last_started_time_);

  return ZX_OK;
}

zx_status_t AudioCompositeServer::StopSocPower() {
  TRACE_DURATION("power-audio", "aml-g12-audio-composite::StopSocPower");
  auto call_time = zx::clock::get_monotonic();

  // Only if needed (not done previously) so voting on relevant clock ids is not repeated.
  // Each driver instance (audio or any other) may vote independently.
  if (!soc_power_started_) {
    TRACE_INSTANT("power-audio", "aml-g12-audio-composite::StopSocPower exit", TRACE_SCOPE_PROCESS,
                  "status", ZX_OK, "reason", "Already stopped");
    fdf::info("SoC power already stopped");
    return ZX_OK;
  }
  fdf::info("Stopping SoC power");

  constexpr uint32_t kGpioAltFunction = 0;

  fidl::Arena arena;
  const auto gpio_function_config =
      fuchsia_hardware_pin::wire::Configuration::Builder(arena).function(kGpioAltFunction).Build();

  for (auto& sclk_client : sclk_clients_) {
    fidl::WireResult alt_function_result = sclk_client.pin->Configure(gpio_function_config);
    if (!alt_function_result.ok()) {
      fdf::error("Failed to send request to set GPIO function: {}",
                 alt_function_result.status_string());
      TRACE_INSTANT("power-audio", "aml-g12-audio-composite::StopSocPower exit",
                    TRACE_SCOPE_PROCESS, "status", alt_function_result.status(), "reason",
                    "Could not set GPIO function");
      return alt_function_result.status();
    }
    fidl::WireResult config_out_result =
        sclk_client.gpio->SetBufferMode(fuchsia_hardware_gpio::BufferMode::kOutputLow);
    if (!config_out_result.ok()) {
      fdf::error("Failed to send request to set GPIO output: {}",
                 config_out_result.status_string());
      TRACE_INSTANT("power-audio", "aml-g12-audio-composite::StopSocPower exit",
                    TRACE_SCOPE_PROCESS, "status", config_out_result.status(), "reason",
                    "Could not set GPIO output");
      return config_out_result.status();
    }
  }

  // MMIO access is still valid after clock gating the audio subsystem.
  fidl::WireResult clock_gate_result = clock_gate_->Disable();
  if (!clock_gate_result.ok()) {
    fdf::error("Failed to send request to disable clock gate: {}",
               clock_gate_result.status_string());
    TRACE_INSTANT("power-audio", "aml-g12-audio-composite::StopSocPower exit", TRACE_SCOPE_PROCESS,
                  "status", clock_gate_result.status(), "reason",
                  "Could not send request to disable clock gate");
    return clock_gate_result.status();
  }
  if (clock_gate_result->is_error()) {
    fdf::error("Send request to disable clock gate error: {}",
               zx_status_get_string(clock_gate_result->error_value()));
    TRACE_INSTANT("power-audio", "aml-g12-audio-composite::StopSocPower exit", TRACE_SCOPE_PROCESS,
                  "status", clock_gate_result->error_value(), "reason",
                  "Could not disable clock gate");
    return clock_gate_result->error_value();
  }
  // MMIO access is still valid after disabling the PLL used.
  fidl::WireResult pll_result = pll_->Disable();
  if (!pll_result.ok()) {
    fdf::error("Failed to send request to disable PLL: {}", pll_result.status_string());
    TRACE_INSTANT("power-audio", "aml-g12-audio-composite::StopSocPower exit", TRACE_SCOPE_PROCESS,
                  "status", pll_result.status(), "reason", "Could not send request to disable PLL");
    return pll_result.status();
  }
  if (pll_result->is_error()) {
    fdf::error("Send request to disable PLL error: {}",
               zx_status_get_string(pll_result->error_value()));
    TRACE_INSTANT("power-audio", "aml-g12-audio-composite::StopSocPower exit", TRACE_SCOPE_PROCESS,
                  "status", pll_result->error_value(), "reason", "Could not disable PLL");
    return pll_result->error_value();
  }
  TRACE_INSTANT("power-audio", "aml-g12-audio-composite::StopSocPower exit", TRACE_SCOPE_PROCESS,
                "status", ZX_OK, "reason", "Successfully powered down");
  trace_async_id_ = TRACE_NONCE();
  TRACE_ASYNC_BEGIN("aml-g12-composite", "suspend", trace_async_id_);
  soc_power_started_ = false;
  last_stopped_time_ = zx::clock::get_monotonic();
  device_inspect()->RecordSocPowerDown(call_time, last_stopped_time_);

  return ZX_OK;
}

// static
std::unique_ptr<RingBufferServer> RingBufferServer::CreateRingBufferServer(
    async_dispatcher_t* dispatcher, AudioCompositeServer& owner, size_t engine_index,
    fidl::ServerEnd<fuchsia_hardware_audio::RingBuffer> ring_buffer, uint64_t element_id) {
  return std::make_unique<RingBufferServer>(dispatcher, owner, engine_index, std::move(ring_buffer),
                                            element_id);
}

RingBufferServer::RingBufferServer(async_dispatcher_t* dispatcher, AudioCompositeServer& owner,
                                   size_t engine_index,
                                   fidl::ServerEnd<fuchsia_hardware_audio::RingBuffer> ring_buffer,
                                   uint64_t element_id)
    : engine_(owner.engines_[engine_index]),
      engine_index_(engine_index),
      dispatcher_(dispatcher),
      owner_(owner),
      binding_(fidl::ServerBinding<fuchsia_hardware_audio::RingBuffer>(
          dispatcher, std::move(ring_buffer), this,
          std::mem_fn(&RingBufferServer::OnRingBufferClosed))),
      ring_buffer_recorder_(
          device_inspect()->CreateRingBufferInstance(element_id, zx::clock::get_monotonic())) {
  ResetRingBuffer();
}

void RingBufferServer::OnRingBufferClosed(fidl::UnbindInfo info) {
  if (info.is_peer_closed()) {
    fdf::info("Client disconnected");
  } else if (!info.is_user_initiated()) {
    // Do not log canceled cases; these happen particularly frequently in certain test cases.
    if (info.status() != ZX_ERR_CANCELED) {
      fdf::error("Client connection unbound: {}", info.status_string());
    }
  }
  ring_buffer_inspect().RecordDestructionTime(zx::clock::get_monotonic());
  ResetRingBuffer();
}

std::unique_ptr<Recorder>& RingBufferServer::device_inspect() { return owner_.device_inspect(); }

void RingBufferServer::ResetRingBuffer() {
  delay_completer_.first_response_sent = false;
  delay_completer_.completer = {};

  position_completer_.reset();
  expected_notifications_per_ring_ = 0;
  notify_timer_.Cancel();
  notification_period_ = {};

  fetched_ = false;
  {
    auto stop_if_temporarily_started = owner_.TemporarilyStartSocPower();
    engine_.device->Stop();
  }
  BarrierBeforeRelease();
  started_ = false;
  pinned_ring_buffer_.Unpin();
}

void RingBufferServer::GetProperties(
    fidl::Server<fuchsia_hardware_audio::RingBuffer>::GetPropertiesCompleter::Sync& completer) {
  fuchsia_hardware_audio::RingBufferProperties properties;
  properties.needs_cache_flush_or_invalidate(true)
      .driver_transfer_bytes(engine_.device->fifo_depth())
      .turn_on_delay(std::make_optional(owner_.kTimeToStabilizePll.get()));
  completer.Reply(std::move(properties));
}

void RingBufferServer::GetVmo(
    GetVmoRequest& request,
    fidl::Server<fuchsia_hardware_audio::RingBuffer>::GetVmoCompleter::Sync& completer) {
  if (started_) {
    fdf::error("GetVmo failed, ring buffer started");
    binding_.Close(ZX_ERR_BAD_STATE);
    return;
  }
  if (fetched_) {
    fdf::error("GetVmo failed, VMO already retrieved");
    binding_.Close(ZX_ERR_BAD_STATE);
    return;
  }
  uint32_t frame_size = engine_.ring_buffer_format.pcm_format().value().number_of_channels() *
                        engine_.ring_buffer_format.pcm_format().value().bytes_per_sample();
  size_t ring_buffer_size = fbl::round_up<size_t, size_t>(
      request.min_frames() * frame_size + engine_.device->fifo_depth(),
      std::lcm(frame_size, engine_.device->GetBufferAlignment()));
  size_t out_frames = ring_buffer_size / frame_size;
  if (out_frames > std::numeric_limits<uint32_t>::max()) {
    fdf::error("out frames too big: {}", out_frames);
    completer.Close(ZX_ERR_INTERNAL);
    return;
  }
  zx_status_t status = InitBuffer(ring_buffer_size);
  if (status != ZX_OK) {
    fdf::error("failed to init buffer: {}", zx_status_get_string(status));
    completer.Close(status);
    return;
  }

  constexpr uint32_t rights = ZX_RIGHT_READ | ZX_RIGHT_WRITE | ZX_RIGHT_MAP | ZX_RIGHT_TRANSFER;
  zx::vmo buffer;
  status = ring_buffer_vmo_.duplicate(rights, &buffer);
  if (status != ZX_OK) {
    fdf::error("GetVmo failed, could not duplicate VMO: {}", zx_status_get_string(status));
    completer.Close(status);
    return;
  }

  {
    auto stop_if_temporarily_started = owner_.TemporarilyStartSocPower();
    status = engine_.device->SetBuffer(pinned_ring_buffer_.region(0).phys_addr, ring_buffer_size);
  }
  if (status != ZX_OK) {
    fdf::error("failed to set buffer: {}", zx_status_get_string(status));
    completer.Close(status);
    return;
  }

  expected_notifications_per_ring_.store(request.clock_recovery_notifications_per_ring());
  fetched_ = true;
  // This is safe because of the overflow check we made above.
  auto out_num_rb_frames = static_cast<uint32_t>(out_frames);
  completer.Reply(zx::ok(
      fuchsia_hardware_audio::RingBufferGetVmoResponse(out_num_rb_frames, std::move(buffer))));
}

void RingBufferServer::Start(StartCompleter::Sync& completer) {
  int64_t start_time = 0;
  if (started_) {
    fdf::error("Could not start: already started");
    binding_.Close(ZX_ERR_BAD_STATE);
    return;
  }
  if (!fetched_) {
    fdf::error("Could not start: first, GetVmo must successfully complete");
    binding_.Close(ZX_ERR_BAD_STATE);
    return;
  }

  {
    auto stop_if_temporarily_started = owner_.TemporarilyStartSocPower();
    start_time = engine_.device->Start();
  }
  started_ = true;

  uint32_t notifs = expected_notifications_per_ring_.load();
  if (notifs) {
    uint32_t frame_size =
        engine_.config.ring_buffer.number_of_channels * engine_.config.ring_buffer.bytes_per_sample;
    ZX_ASSERT(engine_.dai_index < kNumberOfPipelines);
    uint32_t frame_rate = owner_.current_dai_formats(engine_.dai_index).frame_rate();

    // Notification period in usecs scaling by 1'000s to provide good enough resolution in the
    // integer calculation.
    notification_period_ = zx::usec(1'000 * pinned_ring_buffer_.region(0).size /
                                    (frame_size * frame_rate / 1'000 * notifs));
    notify_timer_.PostDelayed(dispatcher_, notification_period_);
  } else {
    notification_period_ = {};
  }
  completer.Reply(start_time);
  ring_buffer_inspect().RecordStartTime(zx::time(start_time));
}

void RingBufferServer::Stop(StopCompleter::Sync& completer) {
  if (!fetched_) {
    fdf::error("GetVmo must successfully complete before calling Start or Stop");
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }
  if (!started_) {
    fdf::debug("Stop called while stopped; this is allowed");
  }

  notify_timer_.Cancel();
  notification_period_ = {};

  {
    auto stop_if_temporarily_started = owner_.TemporarilyStartSocPower();
    engine_.device->Stop();
  }
  auto stop_time = zx::clock::get_monotonic();
  started_ = false;

  completer.Reply();
  ring_buffer_inspect().RecordStopTime(stop_time);
}

zx_status_t RingBufferServer::InitBuffer(size_t size) {
  pinned_ring_buffer_.Unpin();
  const size_t vmo_size = fbl::round_up<size_t, size_t>(size, zx_system_get_page_size());
  zx_status_t status = zx_vmo_create_contiguous(owner_.bti().get(), vmo_size, 0,
                                                ring_buffer_vmo_.reset_and_get_address());
  if (status != ZX_OK) {
    fdf::error("failed to allocate ring buffer vmo: {}", zx_status_get_string(status));
    return status;
  }

  status =
      pinned_ring_buffer_.Pin(ring_buffer_vmo_, owner_.bti(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE);
  if (status != ZX_OK) {
    fdf::error("failed to pin ring buffer vmo: {}", zx_status_get_string(status));
    return status;
  }
  return ZX_OK;
}

void RingBufferServer::ProcessRingNotification() {
  if (notification_period_.get()) {
    notify_timer_.PostDelayed(dispatcher_, notification_period_);
  } else {
    notify_timer_.Cancel();
    return;
  }
  if (position_completer_) {
    fuchsia_hardware_audio::RingBufferPositionInfo info;
    info.position(engine_.device->GetRingPosition());
    info.timestamp(zx::clock::get_monotonic().get());
    position_completer_->Reply(std::move(info));
    position_completer_.reset();
  }
}

void RingBufferServer::WatchClockRecoveryPositionInfo(
    WatchClockRecoveryPositionInfoCompleter::Sync& completer) {
  if (position_completer_) {
    // The client called WatchClockRecoveryPositionInfo when another hanging get was pending.
    // This is an error condition and hence we unbind the channel.
    fdf::error(
        "WatchClockRecoveryPositionInfo was re-called while the previous call was still"
        " pending");
    completer.Close(ZX_ERR_BAD_STATE);
  } else {
    // This completer is kept and responded in ProcessRingNotification.
    position_completer_.emplace(completer.ToAsync());
  }
}

void RingBufferServer::WatchDelayInfo(WatchDelayInfoCompleter::Sync& completer) {
  if (!delay_completer_.first_response_sent) {
    delay_completer_.first_response_sent = true;

    fuchsia_hardware_audio::DelayInfo delay_info = {};
    // No external delay information is provided by this driver.
    delay_info.internal_delay(internal_delay_.to_nsecs());
    completer.Reply(std::move(delay_info));
  } else if (delay_completer_.completer) {
    // The client called WatchDelayInfo when another hanging get was pending.
    // This is an error condition and hence we unbind the channel.
    fdf::error("WatchDelayInfo was re-called while the previous call was still pending");
    completer.Close(ZX_ERR_BAD_STATE);
  } else {
    // This completer is kept but never used since we are not updating the delay info.
    delay_completer_.completer.emplace(completer.ToAsync());
  }
}

void RingBufferServer::SetActiveChannels(
    fuchsia_hardware_audio::RingBufferSetActiveChannelsRequest& request,
    SetActiveChannelsCompleter::Sync& completer) {
  TRACE_DURATION("power-audio", "aml-g12-audio-composite::SetActiveChannels", "bitmask",
                 request.active_channels_bitmask());
  auto call_time = zx::clock::get_monotonic();
  // Check if bitmask activating channels go beyond the current number of channels.
  if (request.active_channels_bitmask() &
      ~((1 << engine_.ring_buffer_format.pcm_format().value().number_of_channels()) - 1)) {
    fdf::error("Bitmask: 0x{:X} activating channels beyond the current number of channels: {}",
               request.active_channels_bitmask(),
               engine_.ring_buffer_format.pcm_format().value().number_of_channels());
    TRACE_INSTANT("power-audio", "aml-g12-audio-composite::SetActiveChannels exit",
                  TRACE_SCOPE_PROCESS, "status", ZX_ERR_INVALID_ARGS, "reason",
                  "Bitmask out of range");
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  // Any active channel requires a particular engine to be on.
  owner_.engines_on_.set(engine_index_, request.active_channels_bitmask());

  // Any engine that is on requires SoC power.
  if (owner_.engines_on_.any()) {
    // Per the audio drivers API at //sdk/fidl/fuchsia.hardware.audio/ring_buffer.fidl
    // SetActiveChannels does not delay the reply waiting for the hardware to actually turn on,
    // we reply with a time indicating when the hardware configuration was completed.
    zx_status_t status = owner_.StartSocPower(/*wait_for_completion*/ false);
    if (status != ZX_OK) {
      TRACE_INSTANT("power-audio", "aml-g12-audio-composite::SetActiveChannels exit",
                    TRACE_SCOPE_PROCESS, "status", status, "reason", "StartSocPower failed");
      completer.Reply(zx::error(status));
      return;
    }
    TRACE_INSTANT("power-audio", "aml-g12-audio-composite::SetActiveChannels exit",
                  TRACE_SCOPE_PROCESS, "status", ZX_OK, "reason", "Successfully enabled channels");
    completer.Reply(zx::ok(owner_.last_started_time_.get()));
    ring_buffer_inspect().RecordActiveChannelsCall(request.active_channels_bitmask(), call_time,
                                                   owner_.last_started_time_);
    return;
  }

  zx_status_t status = owner_.StopSocPower();
  if (status != ZX_OK) {
    TRACE_INSTANT("power-audio", "aml-g12-audio-composite::SetActiveChannels exit",
                  TRACE_SCOPE_PROCESS, "status", status, "reason", "StopSocPower failed");
    completer.Reply(zx::error(status));
    return;
  }
  TRACE_INSTANT("power-audio", "aml-g12-audio-composite::SetActiveChannels exit",
                TRACE_SCOPE_PROCESS, "status", ZX_OK, "reason", "Successfully disabled channels");
  completer.Reply(zx::ok(owner_.last_stopped_time_.get()));
  ring_buffer_inspect().RecordActiveChannelsCall(request.active_channels_bitmask(), call_time,
                                                 owner_.last_stopped_time_);
}

void RingBufferServer::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_audio::RingBuffer> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error("RingBufferServer::handle_unknown_method (RingBuffer) ordinal {}",
             metadata.method_ordinal);
}

void AudioCompositeServer::GetElements(GetElementsCompleter::Sync& completer) {
  std::vector<fuchsia_hardware_audio_signalprocessing::Element> elements;

  // One ring buffer per TDM engine.
  for (size_t i = 0; i < kNumberOfTdmEngines; ++i) {
    fuchsia_hardware_audio_signalprocessing::Element ring_buffer;
    ring_buffer.id(kRingBufferIds[i])
        .type(fuchsia_hardware_audio_signalprocessing::ElementType::kRingBuffer)
        .description(std::string(engines_[i].config.is_input ? "Incoming" : "Outgoing") +
                     " ring buffer, id " + std::to_string(kRingBufferIds[i]))
        .can_stop(false)
        .can_bypass(false);
    elements.push_back(std::move(ring_buffer));
  }
  // One DAI per pipeline.
  for (size_t i = 0; i < kNumberOfPipelines; ++i) {
    fuchsia_hardware_audio_signalprocessing::Element dai;
    fuchsia_hardware_audio_signalprocessing::DaiInterconnect dai_interconnect;
    dai_interconnect.plug_detect_capabilities(
        fuchsia_hardware_audio_signalprocessing::PlugDetectCapabilities::kHardwired);
    dai.id(kDaiIds[i])
        .type(fuchsia_hardware_audio_signalprocessing::ElementType::kDaiInterconnect)
        .type_specific(
            fuchsia_hardware_audio_signalprocessing::TypeSpecificElement::WithDaiInterconnect(
                std::move(dai_interconnect)))
        .description("DAI interconnect, id " + std::to_string(kDaiIds[i]))
        .can_stop(false)
        .can_bypass(false);
    elements.push_back(std::move(dai));
  }

  completer.Reply(zx::ok(elements));
}

void AudioCompositeServer::WatchElementState(WatchElementStateRequest& request,
                                             WatchElementStateCompleter::Sync& completer) {
  auto element_completer = element_completers_.find(request.processing_element_id());
  if (element_completer == element_completers_.end()) {
    fdf::error("Unknown process element id ({}) for WatchElementState",
               request.processing_element_id());
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  auto& element = element_completer->second;
  if (!element.first_response_sent) {
    element.first_response_sent = true;

    fuchsia_hardware_audio_signalprocessing::ElementState element_state;
    // For the DAI elements, we need to return type-specific state as well.
    if (std::find(kDaiIds.begin(), kDaiIds.end(), request.processing_element_id()) !=
        kDaiIds.end()) {
      // All DAI elements are hardwired hence plugged at time 0.
      fuchsia_hardware_audio_signalprocessing::PlugState plug_state;
      plug_state.plugged(true).plug_state_time(0);
      fuchsia_hardware_audio_signalprocessing::DaiInterconnectElementState dai_state;
      dai_state.plug_state(std::move(plug_state));

      // Would be nice to specify dai_state.external_delay as well, if we knew it.

      element_state.type_specific(
          fuchsia_hardware_audio_signalprocessing::TypeSpecificElementState::WithDaiInterconnect(
              std::move(dai_state)));
    }
    element_state.started(true);
    completer.Reply(std::move(element_state));
  } else if (element.completer) {
    // The client called WatchElementState when another hanging-get was pending.
    // This is an error condition and hence we unbind the channel.
    fdf::error("WatchElementState was re-called while the previous call was still pending");
    completer.Close(ZX_ERR_BAD_STATE);
  } else {
    // This completer is kept but never used since all our ElementStates will never change.
    element.completer = completer.ToAsync();
  }
}

void AudioCompositeServer::SetElementState(SetElementStateRequest& request,
                                           SetElementStateCompleter::Sync& completer) {
  auto element_completer = element_completers_.find(request.processing_element_id());
  if (element_completer == element_completers_.end()) {
    fdf::error("Unknown process element id ({}) for SetElementState",
               request.processing_element_id());
    // Return an error, but no need to close down the entire protocol channel.
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  // All elements are interconnects or ring_buffers that cannot be stopped or bypassed.
  if (!request.state().started().value_or(true) || request.state().bypassed().value_or(false)) {
    // Return an error, but no need to close down the entire protocol channel.
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  // This driver does not expect or handle vendor_specific_data from clients.
  if (request.state().vendor_specific_data().has_value()) {
    fdf::warn("SetElementState({}): ignoring {} bytes of vendor_specific_data (unsupported)",
              request.processing_element_id(), request.state().vendor_specific_data()->size());
  }
  // No field is acted upon, so if we get this far then we can declare success.
  completer.Reply(zx::ok());
}

void AudioCompositeServer::GetTopologies(GetTopologiesCompleter::Sync& completer) {
  fuchsia_hardware_audio_signalprocessing::Topology topology;
  topology.id(kTopologyId);
  std::vector<fuchsia_hardware_audio_signalprocessing::EdgePair> edges;
  for (size_t i = 0; i < kNumberOfTdmEngines; ++i) {
    if (!engines_[i].config.is_input) {
      //                        +-------------+    +---------+
      //        Source       -> | RING_BUFFER | -> +   DAI   | ->     Destination
      //      from client       +-------------+    +---------+    e.g. Bluetooth chip
      fuchsia_hardware_audio_signalprocessing::EdgePair pair;
      pair.processing_element_id_from(kRingBufferIds[i])
          .processing_element_id_to(kDaiIds[engines_[i].dai_index]);
      edges.push_back(std::move(pair));
    } else {
      //                        +---------+    +-------------+
      //       Source        -> |   DAI   | -> + RING_BUFFER | ->     Destination
      // e.g. Bluetooth chip    +---------+    +-------------+         to client
      fuchsia_hardware_audio_signalprocessing::EdgePair pair;
      pair.processing_element_id_from(kDaiIds[engines_[i].dai_index])
          .processing_element_id_to(kRingBufferIds[i]);
      edges.push_back(std::move(pair));
    }
  }

  topology.processing_elements_edge_pairs(edges);
  completer.Reply(zx::ok(std::vector{std::move(topology)}));
}

void AudioCompositeServer::WatchTopology(WatchTopologyCompleter::Sync& completer) {
  if (!topology_completer_.first_response_sent) {
    topology_completer_.first_response_sent = true;
    completer.Reply(kTopologyId);
  } else if (topology_completer_.completer.has_value()) {
    // The client called WatchTopology when another hanging get was pending.
    // This is an error condition and hence we unbind the channel.
    fdf::error("WatchTopology was re-called while the previous call was still pending");
    completer.Close(ZX_ERR_BAD_STATE);
  } else {
    // This completer is kept but never used since we are not updating the topology.
    topology_completer_.completer.emplace(completer.ToAsync());
  }
}

// This device has only one signalprocessing topology.
void AudioCompositeServer::SetTopology(SetTopologyRequest& request,
                                       SetTopologyCompleter::Sync& completer) {
  if (request.topology_id() == kTopologyId) {
    completer.Reply(zx::ok());
  } else {
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
  }
}

void AudioCompositeServer::handle_unknown_method(
    fidl::UnknownMethodMetadata<typename fuchsia_hardware_audio_signalprocessing::SignalProcessing>
        metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error("AudioCompositeServer::handle_unknown_method (SignalProcessing) ordinal {}",
             metadata.method_ordinal);
}

void AudioCompositeServer::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_audio::Composite> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error("AudioCompositeServer::handle_unknown_method (Composite) ordinal {}",
             metadata.method_ordinal);
}

}  // namespace audio::aml_g12
