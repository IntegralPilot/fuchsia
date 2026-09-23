// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/drivers/tests/admin_test.h"

#include <fuchsia/hardware/audio/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/vmo.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/fidl.h>
#include <zircon/rights.h>
#include <zircon/time.h>

#include <cmath>
#include <cstring>
#include <numeric>
#include <optional>
#include <unordered_set>

#include <gtest/gtest.h>

#include "src/media/audio/drivers/tests/durations.h"

namespace fhasp = fuchsia::hardware::audio::signalprocessing;

namespace media::audio::drivers::test {

inline constexpr bool kTolerateNonTerminalDaiEndpoints = true;
inline constexpr bool kDisplayElementsAndTopologies = false;
inline constexpr bool kDisplayDaiFormatSets = false;

namespace {

void DisplayElements(const std::vector<fhasp::Element>& elements) {
  std::stringstream ss;
  ss << "Elements[" << elements.size() << "]:" << '\n';
  auto element_idx = 0u;
  for (const auto& element : elements) {
    ss << "        [" << element_idx++ << "] ID " << element.id() << ", type " << element.type()
       << (element.type() == fhasp::ElementType::DAI_INTERCONNECT ? " (DAI)" : "")
       << (element.type() == fhasp::ElementType::RING_BUFFER ? " (RING_BUFFER)" : "")
       << (element.type() == fhasp::ElementType::PACKET_STREAM ? " (PACKET_STREAM)" : "") << '\n';
  }
  printf("%s", ss.str().c_str());
}

void DisplayTopologies(const std::vector<fhasp::Topology>& topologies) {
  std::stringstream ss;
  ss << "Topologies[" << topologies.size() << "]:" << '\n';
  auto topology_idx = 0u;
  for (const auto& topology : topologies) {
    ss << "          [" << topology_idx++ << "] ID " << topology.id() << ", edges["
       << topology.processing_elements_edge_pairs().size() << "]:" << '\n';
    auto edge_idx = 0u;
    for (const auto& edge_pair : topology.processing_elements_edge_pairs()) {
      ss << "              [" << edge_idx++ << "] " << edge_pair.processing_element_id_from
         << " -> " << edge_pair.processing_element_id_to << '\n';
    }
  }
  printf("%s", ss.str().c_str());
}

}  // namespace

void AdminTest::TearDown() {
  DropSignalProcessing();
  DropRingBuffer();

  TestBase::TearDown();
}

void AdminTest::RequestCodecStartAndExpectResponse() {
  ASSERT_TRUE(device_entry().isCodec());

  zx_time_t received_start_time = ZX_TIME_INFINITE_PAST;
  zx_time_t pre_start_time = zx::clock::get_monotonic().get();
  codec()->Start(AddCallback("Codec::Start", [&received_start_time](int64_t start_time) {
    received_start_time = start_time;
  }));

  ExpectCallbacks();
  if (!HasFailure()) {
    EXPECT_GT(received_start_time, pre_start_time);
    EXPECT_LT(received_start_time, zx::clock::get_monotonic().get());
  }
}

void AdminTest::RequestCodecStopAndExpectResponse() {
  ASSERT_TRUE(device_entry().isCodec());

  zx_time_t received_stop_time = ZX_TIME_INFINITE_PAST;
  zx_time_t pre_stop_time = zx::clock::get_monotonic().get();
  codec()->Stop(AddCallback(
      "Codec::Stop", [&received_stop_time](int64_t stop_time) { received_stop_time = stop_time; }));

  ExpectCallbacks();
  if (!HasFailure()) {
    EXPECT_GT(received_stop_time, pre_stop_time);
    EXPECT_LT(received_stop_time, zx::clock::get_monotonic().get());
  }
}

// Request that the driver reset, expecting a response.
// TODO(https://fxbug.dev/42075676): Test Reset for Composite and Dai (Reset closes any RingBuffer).
// TODO(https://fxbug.dev/42077405): When SignalProcessing testing, Reset should change this state.
void AdminTest::ResetAndExpectResponse() {
  if (device_entry().isCodec()) {
    codec()->Reset(AddCallback("Codec::Reset"));
  } else {
    FAIL() << "Unexpected device type";
    __UNREACHABLE;
  }
  ExpectCallbacks();
}

// Is this ID a RingBuffer element?
bool AdminTest::ElementIsRingBuffer(fuchsia::hardware::audio::ElementId element_id) {
  return std::ranges::any_of(elements_, [element_id](const fhasp::Element& element) {
    return element.has_id() && element.id() == element_id && element.has_type() &&
           element.type() == fhasp::ElementType::RING_BUFFER;
  });
}
// Is this ID a PacketStream element?
bool AdminTest::ElementIsPacketStream(fuchsia::hardware::audio::ElementId element_id) {
  return std::ranges::any_of(elements_, [element_id](const fhasp::Element& element) {
    return element.has_id() && element.id() == element_id && element.has_type() &&
           element.type() == fhasp::ElementType::PACKET_STREAM;
  });
}

// Is this element Incoming (its contents are READ-ONLY) or Outgoing (also WRITABLE)?
// We walk the signalprocessing topologies. If (across all the topologies) this element ever has
// any incoming edges, then it is not itself an outgoing element. If this element ever has any
// outgoing edges, then it is not itself an incoming element.
// Note: This method is intended only for RingBuffer and PacketStream
// elements, not DAIs (which can also be terminal elements and thus
// could potentially be considered "outgoing" or "incoming").
std::optional<bool> AdminTest::ElementIsIncoming(
    std::optional<fuchsia::hardware::audio::ElementId> element_id) {
  if (!device_entry().isComposite()) {
    return TestBase::IsIncoming();
  }

  if (!element_id.has_value()) {
    ADD_FAILURE() << "element_id is not set";
    return std::nullopt;
  }

  if (!ElementIsRingBuffer(*element_id) && !ElementIsPacketStream(*element_id)) {
    ADD_FAILURE() << "element_id " << *element_id << " is not a RingBuffer or PacketStream element";
    return std::nullopt;
  }

  if (!current_topology_id_.has_value()) {
    ADD_FAILURE() << "current_topology_id_ is not set";
    return std::nullopt;
  }

  std::vector<fhasp::EdgePair> edge_pairs;
  for (const auto& t : topologies_) {
    if (t.has_id() && t.id() == *current_topology_id_) {
      edge_pairs = t.processing_elements_edge_pairs();
      break;
    }
  }
  if (edge_pairs.empty()) {
    ADD_FAILURE() << "could not find edge_pairs for current_topology_id " << *current_topology_id_;
    return std::nullopt;
  }
  bool has_outgoing =
      std::ranges::any_of(edge_pairs, [id = *element_id](const fhasp::EdgePair& edge_pair) {
        return (edge_pair.processing_element_id_from == id);
      });
  bool has_incoming =
      std::ranges::any_of(edge_pairs, [id = *element_id](const fhasp::EdgePair& edge_pair) {
        return (edge_pair.processing_element_id_to == id);
      });

  if (has_outgoing && !has_incoming) {
    return false;
  }
  if (!has_outgoing && has_incoming) {
    return true;
  }

  ADD_FAILURE() << "For element " << *element_id << ", has_outgoing and has_incoming are both "
                << (has_outgoing ? "TRUE" : "FALSE");
  return std::nullopt;
}

// For the channelization and sample_format that we've set for the ring buffer, determine the size
// of each frame. This method assumes that CreateRingBuffer has already been sent to the driver.
void AdminTest::CalculateRingBufferFrameSize() {
  EXPECT_LE(ring_buffer_pcm_format_.valid_bits_per_sample,
            ring_buffer_pcm_format_.bytes_per_sample * 8);
  frame_size_ =
      ring_buffer_pcm_format_.number_of_channels * ring_buffer_pcm_format_.bytes_per_sample;
}

void AdminTest::RequestRingBufferChannel() {
  ASSERT_FALSE(device_entry().isCodec());

  fuchsia::hardware::audio::Format rb_format = {};
  rb_format.set_pcm_format(ring_buffer_pcm_format_);

  fidl::InterfaceHandle<fuchsia::hardware::audio::RingBuffer> ring_buffer_handle;
  if (device_entry().isComposite()) {
    RequestTopologies();
    RetrieveInitialTopology();

    // If a ring_buffer_id exists, request it - but don't fail if the driver has no ring buffer.
    if (ring_buffer_id().has_value()) {
      fuchsia::hardware::audio::Format2 format2;
      format2.set_pcm_format(*rb_format.mutable_pcm_format());
      composite()->CreateRingBuffer(
          *ring_buffer_id(), std::move(format2), ring_buffer_handle.NewRequest(),
          // Unlike in other driver types, Composite::CreateRingBuffer has a completion. However,
          // a client need not actually wait for this completion before using the ring_buffer.
          // For sequences such as Composite::CreateRingBuffer > RingBuffer::GetProperties,
          // relax the required ordering of completion receipts.
          AddCallbackUnordered(
              "CreateRingBuffer",
              [](fuchsia::hardware::audio::Composite_CreateRingBuffer_Result result) {
                EXPECT_FALSE(result.is_err());
              }));
      if (!composite().is_bound()) {
        FAIL() << "Composite failed to get ring buffer channel";
      }
      SetRingBufferIncoming(ElementIsIncoming(ring_buffer_id()));
    }
  } else if (device_entry().isDai()) {
    fuchsia::hardware::audio::DaiFormat dai_format = {};
    EXPECT_EQ(fuchsia::hardware::audio::Clone(dai_format_, &dai_format), ZX_OK);
    dai()->CreateRingBuffer(std::move(dai_format), std::move(rb_format),
                            ring_buffer_handle.NewRequest());
    EXPECT_TRUE(dai().is_bound()) << "Dai failed to get ring buffer channel";
    SetRingBufferIncoming(IsIncoming());
  } else {
    stream_config()->CreateRingBuffer(std::move(rb_format), ring_buffer_handle.NewRequest());
    EXPECT_TRUE(stream_config().is_bound()) << "StreamConfig failed to get ring buffer channel";
    SetRingBufferIncoming(IsIncoming());
  }
  zx::channel channel = ring_buffer_handle.TakeChannel();
  ring_buffer_ =
      fidl::InterfaceHandle<fuchsia::hardware::audio::RingBuffer>(std::move(channel)).Bind();
  EXPECT_TRUE(ring_buffer_.is_bound()) << "Failed to get ring buffer channel";

  AddErrorHandler(ring_buffer_, "RingBuffer");

  CalculateRingBufferFrameSize();
}

// Request that driver set format to the lowest bit-rate/channelization of the ranges reported.
// This method assumes that the driver has already successfully responded to a GetFormats request.
void AdminTest::RequestRingBufferChannelWithMinFormat() {
  ASSERT_FALSE(device_entry().isCodec());

  if (ring_buffer_pcm_formats().empty() && device_entry().isComposite()) {
    GTEST_SKIP() << "*** this audio device returns no ring_buffer_formats. Skipping this test. ***";
    __UNREACHABLE;
  }
  ASSERT_GT(ring_buffer_pcm_formats().size(), 0u);

  ring_buffer_pcm_format_ = GetMinPcmFormat(ring_buffer_pcm_formats());
  if (device_entry().isDai() || device_entry().isComposite()) {
    if (dai_formats().empty()) {
      ASSERT_NO_FAILURE_OR_SKIP(RetrieveDaiFormats());
    }
    if (!dai_formats().empty()) {
      dai_format_ = GetMinDaiFormat(dai_formats());
    }
  }
  RequestRingBufferChannel();
}

// Request that driver set the highest bit-rate/channelization of the ranges reported.
// This method assumes that the driver has already successfully responded to a GetFormats request.
void AdminTest::RequestRingBufferChannelWithMaxFormat() {
  ASSERT_FALSE(device_entry().isCodec());

  if (ring_buffer_pcm_formats().empty() && device_entry().isComposite()) {
    GTEST_SKIP() << "*** this audio device returns no ring_buffer_formats. Skipping this test. ***";
    __UNREACHABLE;
  }
  ASSERT_GT(ring_buffer_pcm_formats().size(), 0u);

  ring_buffer_pcm_format_ = GetMaxPcmFormat(ring_buffer_pcm_formats());
  if (device_entry().isDai() || device_entry().isComposite()) {
    if (dai_formats().empty()) {
      ASSERT_NO_FAILURE_OR_SKIP(RetrieveDaiFormats());
    }
    if (!dai_formats().empty()) {
      dai_format_ = GetMaxDaiFormat(dai_formats());
    }
  }
  RequestRingBufferChannel();
}

// Construct an arbitrary PcmFormat from the first supported values.
fuchsia::hardware::audio::PcmFormat AdminTest::GetPcmFormat(
    const fuchsia::hardware::audio::PcmSupportedFormats& format_set) {
  fuchsia::hardware::audio::PcmFormat pcm_format;
  pcm_format.number_of_channels =
      static_cast<uint8_t>(format_set.channel_sets()[0].attributes().size());
  pcm_format.sample_format = format_set.sample_formats()[0];
  pcm_format.bytes_per_sample = format_set.bytes_per_sample()[0];
  pcm_format.valid_bits_per_sample = format_set.valid_bits_per_sample()[0];
  pcm_format.frame_rate = format_set.frame_rates()[0];
  return pcm_format;
}

fuchsia::hardware::audio::Encoding AdminTest::GetEncoding(
    const fuchsia::hardware::audio::SupportedEncodings& format_set) {
  fuchsia::hardware::audio::Encoding encoding;
  encoding.set_encoding_type(format_set.encoding_types()[0]);
  encoding.set_decoded_channel_count(
      static_cast<uint32_t>(format_set.decoded_channel_sets()[0].attributes().size()));

  uint32_t bitrate = 128000;
  if (format_set.has_min_encoding_bitrate()) {
    bitrate = std::max(bitrate, format_set.min_encoding_bitrate());
  }
  if (format_set.has_max_encoding_bitrate()) {
    bitrate = std::min(bitrate, format_set.max_encoding_bitrate());
  }
  encoding.set_average_encoding_bitrate(bitrate);
  return encoding;
}

void AdminTest::RequestPacketStreamChannelWithMinPcmFormat() {
  ASSERT_TRUE(device_entry().isComposite());

  if (packet_stream_pcm_formats().empty()) {
    GTEST_SKIP()
        << "*** this audio device returns no packet_stream_formats. Skipping this test. ***";
    __UNREACHABLE;
  }

  packet_stream_format_ = fuchsia::hardware::audio::Format2();
  packet_stream_format_->set_pcm_format(fidl::Clone(GetMinPcmFormat(packet_stream_pcm_formats())));
  RequestPacketStreamChannel();
}

void AdminTest::RequestPacketStreamChannelWithMaxPcmFormat() {
  ASSERT_TRUE(device_entry().isComposite());

  if (packet_stream_pcm_formats().empty()) {
    GTEST_SKIP()
        << "*** this audio device returns no packet_stream_formats. Skipping this test. ***";
    __UNREACHABLE;
  }

  packet_stream_format_ = fuchsia::hardware::audio::Format2();
  packet_stream_format_->set_pcm_format(fidl::Clone(GetMaxPcmFormat(packet_stream_pcm_formats())));
  RequestPacketStreamChannel();
}

void AdminTest::RequestPacketStreamChannelWithEncoding(
    fuchsia::hardware::audio::SupportedEncodings encoding_set) {
  ASSERT_TRUE(device_entry().isComposite());

  fuchsia::hardware::audio::Format2 format;
  format.set_encoding(GetEncoding(encoding_set));

  packet_stream_format_ = std::move(format);
  RequestPacketStreamChannel();
}

// Ring-buffer channel requests
//
// Request the RingBufferProperties, at the current format (relies on the ring buffer channel).
// Validate the four fields that might be returned (only one is currently required).
void AdminTest::RequestRingBufferProperties() {
  ASSERT_FALSE(device_entry().isCodec());

  ring_buffer_->GetProperties(AddCallback(
      "RingBuffer::GetProperties", [this](fuchsia::hardware::audio::RingBufferProperties props) {
        ring_buffer_props_ = std::move(props);
      }));
  ExpectCallbacks();
  if (HasFailure()) {
    return;
  }
  ASSERT_TRUE(ring_buffer_props_.has_value()) << "No RingBufferProperties table received";

  // This field is required.
  EXPECT_TRUE(ring_buffer_props_->has_needs_cache_flush_or_invalidate());

  if (ring_buffer_props_->has_turn_on_delay()) {
    // As a zx::duration, a negative value is theoretically possible, but this is disallowed.
    EXPECT_GE(ring_buffer_props_->turn_on_delay(), 0);
  }

  // This field is required, and must be non-zero.
  ASSERT_TRUE(ring_buffer_props_->has_driver_transfer_bytes());
  EXPECT_GT(ring_buffer_props_->driver_transfer_bytes(), 0u);
}

// Request the ring buffer's VMO handle, at the current format (relies on the ring buffer channel).
// `RequestRingBufferProperties` must be called before `RequestBuffer`.
void AdminTest::RequestBuffer(uint32_t min_ring_buffer_frames,
                              uint32_t notifications_per_ring = 0) {
  ASSERT_FALSE(device_entry().isCodec());

  ASSERT_TRUE(ring_buffer_props_.has_value())
      << "RequestBuffer was called before RequestRingBufferChannel";

  min_ring_buffer_frames_ = min_ring_buffer_frames;
  notifications_per_ring_ = notifications_per_ring;
  zx::vmo ring_buffer_vmo;
  ring_buffer_->GetVmo(
      min_ring_buffer_frames, notifications_per_ring,
      AddCallback("GetVmo", [this, &ring_buffer_vmo](
                                fuchsia::hardware::audio::RingBuffer_GetVmo_Result result) {
        ring_buffer_frames_ = result.response().num_frames;
        ring_buffer_vmo = std::move(result.response().ring_buffer);
        EXPECT_TRUE(ring_buffer_vmo.is_valid());
      }));
  ExpectCallbacks();
  if (HasFailure()) {
    return;
  }

  ASSERT_TRUE(ring_buffer_props_->has_driver_transfer_bytes());
  uint32_t driver_transfer_frames =
      (ring_buffer_props_->driver_transfer_bytes() + (frame_size_ - 1)) / frame_size_;
  EXPECT_GE(ring_buffer_frames_, min_ring_buffer_frames_ + driver_transfer_frames)
      << "Driver (returned " << ring_buffer_frames_
      << " frames) must add at least driver_transfer_bytes (" << driver_transfer_frames
      << " frames) to the client-requested ring buffer size (" << min_ring_buffer_frames_
      << " frames)";

  ring_buffer_mapper_.Unmap();
  const zx_vm_option_t option_flags = ZX_VM_PERM_READ | ZX_VM_PERM_WRITE;

  zx_info_handle_basic_t info;
  auto status =
      ring_buffer_vmo.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  ASSERT_EQ(status, ZX_OK) << "vmo.get_info returned error";

  const zx_rights_t required_rights =
      ring_buffer_is_incoming_.value_or(true) ? kRightsVmoReadOnly : kRightsVmoReadWrite;
  EXPECT_EQ((info.rights & required_rights), required_rights)
      << "VMO rights 0x" << std::hex << info.rights << " are insufficient (0x" << required_rights
      << " are required)";

  EXPECT_EQ(
      ring_buffer_mapper_.CreateAndMap(static_cast<uint64_t>(ring_buffer_frames_) * frame_size_,
                                       option_flags, nullptr, &ring_buffer_vmo, required_rights),
      ZX_OK);
}

void AdminTest::ActivateChannelsAndExpectOutcome(uint64_t active_channels_bitmask,
                                                 SetActiveChannelsOutcome expected_outcome) {
  zx_status_t status = ZX_OK;
  auto set_time = zx::time(0);
  auto send_time = zx::clock::get_monotonic();
  ring_buffer_->SetActiveChannels(
      active_channels_bitmask,
      AddCallback("SetActiveChannels",
                  [&status, &set_time](
                      fuchsia::hardware::audio::RingBuffer_SetActiveChannels_Result result) {
                    if (!result.is_err()) {
                      set_time = zx::time(result.response().set_time);
                    } else {
                      status = result.err();
                    }
                  }));
  ExpectCallbacks();

  if (status == ZX_ERR_NOT_SUPPORTED) {
    GTEST_SKIP() << "This driver does not support SetActiveChannels()";
    __UNREACHABLE;
  }

  SCOPED_TRACE(testing::Message() << "...during ring_buffer_fidl->SetActiveChannels(0x" << std::hex
                                  << active_channels_bitmask << ")");
  if (expected_outcome == SetActiveChannelsOutcome::FAILURE) {
    ASSERT_NE(status, ZX_OK) << "SetActiveChannels succeeded unexpectedly";
    EXPECT_EQ(status, ZX_ERR_INVALID_ARGS) << "Unexpected failure code";
  } else {
    ASSERT_EQ(status, ZX_OK) << "SetActiveChannels failed unexpectedly";
    if (expected_outcome == SetActiveChannelsOutcome::NO_CHANGE) {
      EXPECT_LT(set_time.get(), send_time.get());
    } else if (expected_outcome == SetActiveChannelsOutcome::CHANGE) {
      EXPECT_GT(set_time.get(), send_time.get());
    }
  }
}

void AdminTest::RetrieveRingBufferFormats() {
  if (!device_entry().isComposite()) {
    TestBase::RetrieveRingBufferFormats();
    return;
  }
  RequestTopologies();
  RetrieveInitialTopology();

  // If ring_buffer_id_ is set, then a ring-buffer element exists for this composite device.
  // Retrieve the supported ring buffer formats for that node.
  if (!ring_buffer_id_.has_value()) {
    // "No ring buffer" is valid (Composite can replace Codec); do nothing in that case.
    return;
  }
  composite()->GetRingBufferFormats(
      *ring_buffer_id_,
      AddCallback("GetRingBufferFormats",
                  [this](fuchsia::hardware::audio::Composite_GetRingBufferFormats_Result result) {
                    ASSERT_FALSE(result.is_err()) << static_cast<int32_t>(result.err());
                    auto& supported_formats = result.response().ring_buffer_formats;
                    EXPECT_FALSE(supported_formats.empty());

                    for (size_t i = 0; i < supported_formats.size(); ++i) {
                      SCOPED_TRACE(testing::Message()
                                   << "Composite supported_formats[" << i << "]");
                      ASSERT_TRUE(supported_formats[i].is_pcm_supported_formats());
                      auto& format_set = supported_formats[i].pcm_supported_formats();
                      ring_buffer_pcm_formats().push_back(std::move(format_set));
                    }
                  }));
  ExpectCallbacks();

  ValidatePcmSupportedFormats(ring_buffer_pcm_formats());
}

void AdminTest::RetrievePacketStreamFormats() {
  if (!device_entry().isComposite()) {
    return;
  }
  RequestTopologies();
  RetrieveInitialTopology();

  if (!packet_stream_id_.has_value()) {
    GTEST_SKIP() << "PacketStream element not found. Skipping this test.";
    return;
  }

  packet_stream_pcm_formats().clear();
  packet_stream_supported_encodings().clear();

  composite()->GetPacketStreamFormats(
      *packet_stream_id_,
      AddCallback("GetPacketStreamFormats",
                  [this](fuchsia::hardware::audio::Composite_GetPacketStreamFormats_Result result) {
                    ASSERT_FALSE(result.is_err()) << static_cast<int32_t>(result.err());
                    auto& supported_formats = result.response().packet_stream_formats;
                    EXPECT_FALSE(supported_formats.empty());

                    for (size_t i = 0; i < supported_formats.size(); ++i) {
                      SCOPED_TRACE(testing::Message()
                                   << "Composite packet_stream_formats[" << i << "]");
                      if (supported_formats[i].is_pcm_supported_formats()) {
                        auto& format_set = supported_formats[i].pcm_supported_formats();
                        packet_stream_pcm_formats().push_back(std::move(format_set));
                      } else if (supported_formats[i].is_supported_encodings()) {
                        auto& encoding_set = supported_formats[i].supported_encodings();
                        packet_stream_supported_encodings().push_back(std::move(encoding_set));
                      } else {
                        FAIL() << "Unknown format type";
                      }
                    }
                  }));
  ExpectCallbacks();

  if (!packet_stream_pcm_formats().empty()) {
    ValidatePcmSupportedFormats(packet_stream_pcm_formats());
  }

  if (!packet_stream_supported_encodings().empty()) {
    ValidateSupportedEncodings(packet_stream_supported_encodings());
  }
}

// Request that the driver return the format ranges that it supports.
void AdminTest::RetrieveDaiFormats() {
  if (!device_entry().isComposite()) {
    TestBase::RetrieveDaiFormats();
    return;
  }

  RequestTopologies();
  RetrieveInitialTopology();

  // If dai_id_ is set, request the DAI formats for this interconnect.
  if (dai_id_.has_value()) {
    composite()->GetDaiFormats(
        *dai_id_,
        AddCallback("Composite::GetDaiFormats",
                    [this](fuchsia::hardware::audio::Composite_GetDaiFormats_Result result) {
                      ASSERT_FALSE(result.is_err());
                      auto& supported_formats = result.response().dai_formats;
                      EXPECT_FALSE(supported_formats.empty());

                      for (size_t i = 0; i < supported_formats.size(); ++i) {
                        SCOPED_TRACE(testing::Message()
                                     << "Composite supported_formats[" << i << "]");
                        dai_formats().push_back(std::move(supported_formats[i]));
                      }
                    }));
  } else {
    // "No DAI" is also valid (Composite can replace StreamConfig); do nothing in that case.
    return;
  }

  ExpectCallbacks();

  ValidateDaiFormatSets(dai_formats());
}

void AdminTest::ValidateSupportedEncodings(
    const std::vector<fuchsia::hardware::audio::SupportedEncodings>& supported_encodings) {
  // We've already ensured `supported_encodings` is not empty, in `RetrievePacketStreamFormats()`.
  for (size_t i = 0; i < supported_encodings.size(); ++i) {
    SCOPED_TRACE(testing::Message() << "encoding[" << i << "]");
    auto& format_set = supported_encodings[i];

    ASSERT_TRUE(format_set.has_decoded_channel_sets());
    ASSERT_TRUE(format_set.has_encoding_types());

    ASSERT_FALSE(format_set.decoded_channel_sets().empty());
    ASSERT_FALSE(format_set.encoding_types().empty());

    // Ensure `decoded_channel_sets` are valid and listed in ascending order of channel count.
    for (size_t j = 0; j < format_set.decoded_channel_sets().size(); ++j) {
      SCOPED_TRACE(testing::Message() << "channel_set[" << j << "]");
      auto& channel_set = format_set.decoded_channel_sets()[j];

      ASSERT_TRUE(channel_set.has_attributes());
      ASSERT_FALSE(channel_set.attributes().empty());

      if (j > 0) {
        size_t prev_count = format_set.decoded_channel_sets()[j - 1].attributes().size();
        size_t curr_count = channel_set.attributes().size();
        EXPECT_GT(curr_count, prev_count)
            << "decoded_channel_sets[" << j << "] (" << curr_count << " channels) "
            << "must have more channels than decoded_channel_sets[" << j - 1 << "] (" << prev_count
            << " channels)";
      }
    }

    // Ensure frame_rates are unique and listed in ascending order.
    if (format_set.has_decoded_frame_rates()) {
      for (size_t j = 0; j < format_set.decoded_frame_rates().size(); ++j) {
        if (j > 0) {
          EXPECT_GT(format_set.decoded_frame_rates()[j], format_set.decoded_frame_rates()[j - 1])
              << "decoded_frame_rates[" << j << "] (" << format_set.decoded_frame_rates()[j]
              << ") must exceed decoded_frame_rates[" << j - 1 << "] ("
              << format_set.decoded_frame_rates()[j - 1] << ")";
        }
      }
    }

    // Ensure encoding_types are unique.
    for (size_t j = 0; j < format_set.encoding_types().size(); ++j) {
      for (size_t k = j + 1; k < format_set.encoding_types().size(); ++k) {
        EXPECT_NE(format_set.encoding_types()[j], format_set.encoding_types()[k])
            << "encoding_types[" << j << "] must not equal encoding_types[" << k << "]";
      }
    }
  }
}

void AdminTest::RequestPacketStreamChannel() {
  if (!device_entry().isComposite()) {
    // Only Composite supports PacketStream for now in this test suite context.
    return;
  }
  RequestTopologies();
  RetrieveInitialTopology();

  if (!packet_stream_id_.has_value()) {
    GTEST_SKIP() << "PacketStream element not found. Skipping this test.";
    return;
  }

  // If the test hasn't already specified one, select an arbitrary format.
  if (!packet_stream_format_.has_value()) {
    packet_stream_format_ = fuchsia::hardware::audio::Format2();

    if (!packet_stream_supported_encodings().empty()) {
      packet_stream_format_->set_encoding(GetEncoding(packet_stream_supported_encodings()[0]));
    } else if (!packet_stream_pcm_formats().empty()) {
      packet_stream_format_->set_pcm_format(GetPcmFormat(packet_stream_pcm_formats()[0]));
    } else {
      FAIL() << "No supported formats available for PacketStream";
    }
  }

  // Clone the format to pass to CreatePacketStream, keeping the member variable for test
  // verification.
  fuchsia::hardware::audio::Format2 format_to_use;
  packet_stream_format_->Clone(&format_to_use);

  fidl::InterfaceHandle<fuchsia::hardware::audio::PacketStreamControl> stream_handle;
  composite()->CreatePacketStream(
      *packet_stream_id_, std::move(format_to_use), stream_handle.NewRequest(),
      AddCallbackUnordered(
          "CreatePacketStream",
          [](fuchsia::hardware::audio::Composite_CreatePacketStream_Result result) {
            EXPECT_FALSE(result.is_err()) << result.err();
          }));

  packet_stream() = stream_handle.Bind();
  AddErrorHandler(packet_stream(), "PacketStreamControl");
}

void AdminTest::RequestPacketStreamProperties() {
  packet_stream()->GetProperties(AddCallback(
      "GetProperties",
      [this](fuchsia::hardware::audio::PacketStreamControl_GetProperties_Result result) {
        ASSERT_TRUE(result.is_response());
        packet_stream_props() = std::move(result.response().properties);
      }));
  ExpectCallbacks();
  ASSERT_TRUE(packet_stream_props().has_value());
  EXPECT_TRUE(packet_stream_props()->has_supported_buffer_types());
  EXPECT_TRUE(packet_stream_props()->has_needs_cache_flush_or_invalidate());
}

void AdminTest::RegisterPacketStreamVmos(uint32_t vmo_count, uint64_t vmo_size) {
  fuchsia::hardware::audio::RegisterVmosConfig config;
  std::vector<fuchsia::hardware::audio::VmoInfo> infos;
  // With `RegisterVmos()`, the client creates VMOs for the driver.
  for (uint32_t i = 0; i < vmo_count; ++i) {
    zx::vmo vmo;
    ASSERT_EQ(zx::vmo::create(vmo_size, 0, &vmo), ZX_OK);
    fuchsia::hardware::audio::VmoInfo info;
    info.set_id(i);  // simple ID
    zx::vmo dup;
    // The registered VMO handle must include ZX_RIGHT_TRANSFER, ZX_RIGHT_READ and ZX_RIGHT_MAP.
    // If the packet stream is an input stream, then the handle must also include ZX_RIGHT_WRITE.
    // (Note: 'Input' here means 'Input to the system' (Capture).
    //  If Capture, driver writes to VMO. We (Client) are registering VMO for Driver.
    //  So Driver needs ZX_RIGHT_WRITE. We give Driver 'kRightsVmoReadWrite'.
    //  If Playback, driver reads from VMO. We give Driver 'kRightsVmoReadOnly').
    //
    // Note: kRightsVmoReadOnly = READ | MAP | TRANSFER
    //       kRightsVmoReadWrite = READ | MAP | TRANSFER | WRITE
    ASSERT_TRUE(packet_stream_id_.has_value()) << "PacketStream element ID is not set";
    std::optional<bool> is_incoming = ElementIsIncoming(packet_stream_id_);
    ASSERT_TRUE(is_incoming.has_value())
        << "PacketStream element " << *packet_stream_id_ << " is not terminal";
    bool is_input = *is_incoming;
    zx_rights_t rights = is_input ? kRightsVmoReadWrite : kRightsVmoReadOnly;
    ASSERT_EQ(vmo.duplicate(rights, &dup), ZX_OK);
    info.set_vmo(std::move(dup));
    infos.push_back(std::move(info));
    packet_stream_vmos_.push_back(std::move(vmo));
  }
  config.set_vmo_infos(std::move(infos));
  packet_stream()->RegisterVmos(
      std::move(config),
      AddCallback("RegisterVmos",
                  [](fuchsia::hardware::audio::PacketStreamControl_RegisterVmos_Result result) {
                    EXPECT_FALSE(result.is_err());
                  }));
  ExpectCallbacks();
}

void AdminTest::UnregisterPacketStreamVmos() {
  packet_stream()->UnregisterVmos(AddCallback(
      "UnregisterVmos",
      [this](fuchsia::hardware::audio::PacketStreamControl_UnregisterVmos_Result result) {
        EXPECT_FALSE(result.is_err());
        packet_stream_vmos_.clear();
      }));
  ExpectCallbacks();
}

void AdminTest::AllocatePacketStreamVmos(uint32_t vmo_count, uint64_t vmo_size) {
  ASSERT_TRUE(packet_stream_id_.has_value()) << "PacketStream element ID is not set";
  std::optional<bool> is_incoming = ElementIsIncoming(packet_stream_id_);
  ASSERT_TRUE(is_incoming.has_value())
      << "PacketStream element " << *packet_stream_id_ << " is not terminal";
  const zx_rights_t required_rights = *is_incoming ? kRightsVmoReadOnly : kRightsVmoReadWrite;

  fuchsia::hardware::audio::AllocateVmosConfig config;
  config.set_vmo_count(vmo_count);
  config.set_min_vmo_size(vmo_size);

  // With `AllocateVmos()`, the driver creates VMOs for the client.
  packet_stream()->AllocateVmos(
      std::move(config),
      AddCallback("AllocateVmos",
                  [this, vmo_count, vmo_size, required_rights](
                      fuchsia::hardware::audio::PacketStreamControl_AllocateVmos_Result result) {
                    ASSERT_FALSE(result.is_err());
                    ASSERT_EQ(result.response().vmos.size(), vmo_count);

                    for (auto& info : result.response().vmos) {
                      zx_info_handle_basic_t basic_info;
                      ASSERT_EQ(info.mutable_vmo()->get_info(ZX_INFO_HANDLE_BASIC, &basic_info,
                                                             sizeof(basic_info), nullptr, nullptr),
                                ZX_OK);
                      EXPECT_EQ((basic_info.rights & required_rights), required_rights)
                          << "VMO rights 0x" << std::hex << basic_info.rights
                          << " are insufficient (0x" << required_rights << " are required)";

                      uint64_t actual_size;
                      ASSERT_EQ(info.mutable_vmo()->get_size(&actual_size), ZX_OK);
                      EXPECT_GE(actual_size, vmo_size);

                      packet_stream_vmos_.push_back(std::move(*info.mutable_vmo()));
                    }
                  }));
  ExpectCallbacks();
}

void AdminTest::DeallocatePacketStreamVmos() {
  packet_stream()->DeallocateVmos(AddCallback(
      "DeallocateVmos",
      [this](fuchsia::hardware::audio::PacketStreamControl_DeallocateVmos_Result result) {
        EXPECT_FALSE(result.is_err());
        packet_stream_vmos_.clear();
      }));
  ExpectCallbacks();
}

void AdminTest::RequestPacketStreamSink() {
  packet_stream()->GetPacketStreamSink(AddCallback(
      "GetPacketStreamSink",
      [this](fuchsia::hardware::audio::PacketStreamControl_GetPacketStreamSink_Result result) {
        ASSERT_FALSE(result.is_err()) << result.err();
        ASSERT_TRUE(result.response().has_stream());
        packet_stream_sink_ = result.response().mutable_stream()->Bind();
      }));
  ExpectCallbacks();
}

void AdminTest::RequestPacketStreamSinkAndExpectError(zx_status_t expected_error) {
  packet_stream()->GetPacketStreamSink(AddCallback(
      "GetPacketStreamSink",
      [expected_error](
          fuchsia::hardware::audio::PacketStreamControl_GetPacketStreamSink_Result result) {
        ASSERT_TRUE(result.is_err());
        EXPECT_EQ(result.err(), expected_error);
      }));
  ExpectCallbacks();
}

zx::time AdminTest::RequestPacketStreamStart() {
  auto send_time = zx::clock::get_monotonic();
  packet_stream()->Start(
      AddCallback("Start", [](fuchsia::hardware::audio::PacketStreamControl_Start_Result result) {
        EXPECT_FALSE(result.is_err()) << result.err();
      }));
  return send_time;
}

void AdminTest::RequestPacketStreamStartAndExpectCallback() {
  RequestPacketStreamStart();
  ExpectCallbacks();
  packet_stream_started_ = true;
}

void AdminTest::RequestPacketStreamStartAndExpectError(zx_status_t expected_error) {
  packet_stream()->Start(AddCallback(
      "Start", [expected_error](fuchsia::hardware::audio::PacketStreamControl_Start_Result result) {
        ASSERT_TRUE(result.is_err());
        EXPECT_EQ(result.err(), expected_error);
      }));
  ExpectCallbacks();
}

void AdminTest::RequestPacketStreamStopAndExpectCallback() {
  packet_stream()->Stop(
      AddCallback("Stop", [](fuchsia::hardware::audio::PacketStreamControl_Stop_Result result) {
        EXPECT_FALSE(result.is_err()) << result.err();
      }));
  ExpectCallbacks();
  packet_stream_started_ = false;
}

void AdminTest::RequestPacketStreamStopAndExpectError(zx_status_t expected_error) {
  packet_stream()->Stop(AddCallback(
      "Stop", [expected_error](fuchsia::hardware::audio::PacketStreamControl_Stop_Result result) {
        ASSERT_TRUE(result.is_err());
        EXPECT_EQ(result.err(), expected_error);
      }));
  ExpectCallbacks();
}

void AdminTest::PacketStreamPutPacket() {
  if (!packet_stream_sink_ || packet_stream_vmos_.empty())
    return;
  fuchsia::hardware::audio::PacketStreamSinkPutPacketRequest request;
  fuchsia::hardware::audio::VmoTransfer transfer;
  transfer.set_vmo_id(0);
  transfer.set_vmo_offset(0);
  transfer.set_payload_size(10);  // arbitrary small size
  fuchsia::hardware::audio::DataTransfer payload;
  payload.set_vmo_transfer(std::move(transfer));
  request.set_payload(std::move(payload));

  packet_stream_sink_->PutPacket(
      std::move(request),
      AddCallback("PutPacket",
                  [](fuchsia::hardware::audio::PacketStreamSink_PutPacket_Result result) {
                    EXPECT_FALSE(result.is_err()) << result.err();
                  }));
  ExpectCallbacks();
}

void AdminTest::PacketStreamPutPacketAndExpectError(
    fuchsia::hardware::audio::PacketStreamSinkPutPacketRequest request,
    zx_status_t expected_error) {
  packet_stream_sink_->PutPacket(
      std::move(request),
      AddCallback(
          "PutPacket",
          [expected_error](fuchsia::hardware::audio::PacketStreamSink_PutPacket_Result result) {
            ASSERT_TRUE(result.is_err());
            EXPECT_EQ(result.err(), expected_error);
          }));
  ExpectCallbacks();
}

void AdminTest::PacketStreamFlushPackets() {
  packet_stream_sink_->FlushPackets(AddCallback(
      "FlushPackets", [](fuchsia::hardware::audio::PacketStreamSink_FlushPackets_Result result) {
        EXPECT_FALSE(result.is_err()) << result.err();
      }));
  ExpectCallbacks();
}

// Request that the driver start the ring buffer engine, responding with the start_time.
// This method assumes that GetVmo has previously been called and we are not already started.
zx::time AdminTest::RequestRingBufferStart() {
  if (ring_buffer_frames_ == 0u) {
    ADD_FAILURE() << "GetVmo must be called before RingBuffer::Start()";
    return {};
  }

  // Any position notifications that arrive before RingBuffer::Start callback should cause failures.
  ExpectNoPositionNotifications();

  auto send_time = zx::clock::get_monotonic();
  ring_buffer_->Start(AddCallback("RingBuffer::Start", [this](int64_t start_time) {
    ExpectPositionNotifications();
    start_time_ = zx::time(start_time);
  }));
  return send_time;
}

// Request that the driver start the ring buffer engine, responding with the start_time.
// This method assumes that GetVmo has previously been called and we are not already started.
void AdminTest::RequestRingBufferStartAndExpectCallback() {
  auto send_time = RequestRingBufferStart();

  ExpectCallbacks();
  if (!HasFailure()) {
    EXPECT_GT(start_time_, send_time);
  }
}

// Request that the driver start the ring buffer engine, but expect disconnect rather than response.
void AdminTest::RequestRingBufferStartAndExpectDisconnect(zx_status_t expected_error) {
  ring_buffer_->Start(
      [](int64_t start_time) { FAIL() << "Received unexpected RingBuffer::Start response"; });

  ExpectError(ring_buffer(), expected_error);
  CooldownAfterRingBufferDisconnect();
}

// Ensure that the RingBuffer is already running before returning.
void AdminTest::WaitUntilAfterStartTime() {
  auto start = start_time();
  zx::nanosleep(start + zx::msec(1));
}

// Request that driver stop the ring buffer. This assumes that GetVmo has previously been called.
void AdminTest::RequestRingBufferStopAndExpectCallback() {
  ASSERT_GT(ring_buffer_frames_, 0u) << "GetVmo must be called before RingBuffer::Stop()";
  ring_buffer_->Stop(AddCallback("RingBuffer::Stop"));

  ExpectCallbacks();
}

// Request that the driver start the ring buffer engine, but expect disconnect rather than response.
// We would expect this if calling RingBuffer::Stop before GetVmo, for example.
void AdminTest::RequestRingBufferStopAndExpectDisconnect(zx_status_t expected_error) {
  ring_buffer_->Stop(AddUnexpectedCallback("RingBuffer::Stop - expected disconnect instead"));

  ExpectError(ring_buffer(), expected_error);
  CooldownAfterRingBufferDisconnect();
}

// After RingBuffer::Stop is called, no position notification should be received.
// To validate this without any race windows: from within the next position notification itself,
// we call RingBuffer::Stop and flag that subsequent position notifications should FAIL.
void AdminTest::RequestRingBufferStopAndExpectNoPositionNotifications() {
  ring_buffer_->Stop(
      AddCallback("RingBuffer::Stop", [this]() { ExpectNoPositionNotifications(); }));

  ExpectCallbacks();
}

void AdminTest::RequestPositionNotification() {
  ring_buffer()->WatchClockRecoveryPositionInfo(
      [this](fuchsia::hardware::audio::RingBufferPositionInfo position_info) {
        PositionNotificationCallback(position_info);
      });
}

void AdminTest::PositionNotificationCallback(
    fuchsia::hardware::audio::RingBufferPositionInfo position_info) {
  // If this is an unexpected callback, fail and exit.
  if (fail_on_position_notification_) {
    FAIL() << "Unexpected position notification";
    __UNREACHABLE;
  }
  ASSERT_GT(notifications_per_ring(), 0u)
      << "Position notification received: notifications_per_ring() cannot be zero";
}

void AdminTest::WatchDelayAndExpectUpdate() {
  ring_buffer_->WatchDelayInfo(AddCallback(
      "WatchDelayInfo", [this](fuchsia::hardware::audio::RingBuffer_WatchDelayInfo_Result result) {
        ASSERT_TRUE(result.is_response());
        delay_info_ = std::move(result.response().delay_info);
      }));
  ExpectCallbacks();

  ASSERT_TRUE(delay_info_.has_value()) << "No DelayInfo table received";
}

void AdminTest::WatchDelayAndExpectNoUpdate() {
  ring_buffer_->WatchDelayInfo(
      [](fuchsia::hardware::audio::RingBuffer_WatchDelayInfo_Result result) {
        FAIL() << "Unexpected delay update received";
      });
}

// We've already validated that we received an overall response.
// Internal delay must be present and non-negative.
void AdminTest::ValidateInternalDelay() {
  ASSERT_TRUE(delay_info_.has_value());
  ASSERT_TRUE(delay_info_->has_internal_delay());
  EXPECT_GE(delay_info_->internal_delay(), 0ll)
      << "WatchDelayInfo `internal_delay` (" << delay_info_->internal_delay()
      << ") cannot be negative";
}

// We've already validated that we received an overall response.
// External delay (if present) simply must be non-negative.
void AdminTest::ValidateExternalDelay() {
  ASSERT_TRUE(delay_info_.has_value());
  if (delay_info_->has_external_delay()) {
    EXPECT_GE(delay_info_->external_delay(), 0ll)
        << "WatchDelayInfo `external_delay` (" << delay_info_->external_delay()
        << ") cannot be negative";
  }
}

void AdminTest::DropRingBuffer() {
  if (ring_buffer_.is_bound()) {
    ring_buffer_.Unbind();
    ring_buffer_ = nullptr;
  }

  CooldownAfterRingBufferDisconnect();
}

// General signalprocessing methods
//
// All signalprocessing-related methods are in AdminTest, because only Composite drivers support the
// signalprocessing protocols, and Composite drivers have only AdminTest cases.
void AdminTest::SignalProcessingConnect() {
  if (signal_processing().is_bound()) {
    return;  // Already connected.
  }
  fidl::InterfaceHandle<fhasp::SignalProcessing> sp_client;
  fidl::InterfaceRequest<fhasp::SignalProcessing> sp_server = sp_client.NewRequest();
  composite()->SignalProcessingConnect(std::move(sp_server));
  signal_processing() = sp_client.Bind();

  AddErrorHandler(signal_processing(), "SignalProcessing");
}

void AdminTest::DropSignalProcessing() {
  if (signal_processing().is_bound()) {
    signal_processing().Unbind();
    signal_processing() = nullptr;

    CooldownAfterSignalProcessingDisconnect();
  }
}

// Element methods
//
// Retrieve the element list. If signalprocessing is not supported, exit early;
// otherwise save the ID of a RING_BUFFER element, and the ID of a DAI_INTERCONNECT element.
// We will use these IDs later, when performing Dai-specific and RingBuffer-specific checks.
void AdminTest::RequestElements() {
  SignalProcessingConnect();

  // If we've already checked for signalprocessing support, then no need to do it again.
  if (signalprocessing_is_supported_.has_value()) {
    return;
  }
  if (!elements_.empty()) {
    return;
  }

  zx_status_t status = ZX_OK;
  ring_buffer_id_.reset();
  dai_id_.reset();
  signal_processing()->GetElements(
      AddCallback("signalprocessing::Reader::GetElements",
                  [this, &status](fhasp::Reader_GetElements_Result result) {
                    status = result.is_err() ? result.err() : ZX_OK;
                    if (status == ZX_OK) {
                      elements_ = std::move(result.response().processing_elements);
                    } else {
                      signalprocessing_is_supported_ = false;
                    }
                  }));
  ExpectCallbacks();

  // Either we get elements or the API method is not supported.
  ASSERT_TRUE(status == ZX_OK || status == ZX_ERR_NOT_SUPPORTED);
  // We don't check for topologies if GetElements is not supported.
  if (status == ZX_ERR_NOT_SUPPORTED) {
    signalprocessing_is_supported_ = false;
    return;
  }

  // If supported, GetElements must return at least one element.
  if (elements_.empty()) {
    signalprocessing_is_supported_ = false;
    FAIL() << "elements list is empty";
  }

  signalprocessing_is_supported_ = true;

  std::unordered_set<fhasp::ElementId> element_ids;
  for (auto& element : elements_) {
    // All elements must have an ID and type
    ASSERT_TRUE(element.has_id());
    ASSERT_TRUE(element.has_type());
    if (element.type() == fhasp::ElementType::RING_BUFFER) {
      ring_buffer_id_.emplace(element.id());  // Override any previous.
    } else if (element.type() == fhasp::ElementType::DAI_INTERCONNECT) {
      dai_id_.emplace(element.id());  // Override any previous.
    } else if (element.type() == fhasp::ElementType::PACKET_STREAM) {
      packet_stream_id_.emplace(element.id());  // Override any previous.
    }

    // No element ID may be a duplicate.
    ASSERT_FALSE(element_ids.contains(element.id())) << "Duplicate element ID " << element.id();
    element_ids.insert(element.id());
  }
}

// For all elements, validate their non-type-specific fields.
void AdminTest::ValidateElements() {
  std::unordered_set<fhasp::ElementId> ids;
  for (const auto& element : elements()) {
    // id must be unique within the given array of elements.
    ASSERT_TRUE(element.has_id());
    ASSERT_FALSE(ids.contains(element.id()));
    ids.insert(element.id());

    ValidateElement(element);
  }
}

// For DAI_INTERCONNECT elements, validate their type-specific field.
void AdminTest::ValidateDaiElements() {
  bool found_one = false;
  for (const auto& element : elements()) {
    ASSERT_TRUE(element.has_type());
    if (element.type() == fhasp::ElementType::DAI_INTERCONNECT) {
      found_one = true;
      ValidateDaiElement(element);
    }
  }
  if (!found_one) {
    GTEST_SKIP() << "No DAI_INTERCONNECT elements found";
  }
}

// For DYNAMICS elements, validate their type-specific field.
void AdminTest::ValidateDynamicsElements() {
  bool found_one = false;
  for (const auto& element : elements()) {
    ASSERT_TRUE(element.has_type());
    if (element.type() == fhasp::ElementType::DYNAMICS) {
      found_one = true;
      ValidateDynamicsElement(element);
    }
  }
  if (!found_one) {
    GTEST_SKIP() << "No DYNAMICS elements found";
  }
}

// For EQUALIZER elements, validate their type-specific field.
void AdminTest::ValidateEqualizerElements() {
  bool found_one = false;
  for (const auto& element : elements()) {
    ASSERT_TRUE(element.has_type());
    if (element.type() == fhasp::ElementType::EQUALIZER) {
      found_one = true;
      ValidateEqualizerElement(element);
    }
  }
  if (!found_one) {
    GTEST_SKIP() << "No EQUALIZER elements found";
  }
}

// For GAIN elements, validate their type-specific field.
void AdminTest::ValidateGainElements() {
  bool found_one = false;
  for (const auto& element : elements()) {
    ASSERT_TRUE(element.has_type());
    if (element.type() == fhasp::ElementType::GAIN) {
      found_one = true;
      ValidateGainElement(element);
    }
  }
  if (!found_one) {
    GTEST_SKIP() << "No GAIN elements found";
  }
}

// For VENDOR_SPECIFIC elements, validate their type-specific field.
void AdminTest::ValidateVendorSpecificElements() {
  bool found_one = false;
  for (const auto& element : elements()) {
    ASSERT_TRUE(element.has_type());
    if (element.type() == fhasp::ElementType::VENDOR_SPECIFIC) {
      found_one = true;
      ValidateVendorSpecificElement(element);
    }
  }
  if (!found_one) {
    GTEST_SKIP() << "No VENDOR_SPECIFIC elements found";
  }
}

// Validate the non-type-specific portion of this Element.
void AdminTest::ValidateElement(const fhasp::Element& element) {
  // id: Required.
  ASSERT_TRUE(element.has_id());

  // type: Required.
  ASSERT_TRUE(element.has_type());

  // description: Optional, but cannot be "".
  EXPECT_FALSE(element.has_description() && element.description().empty());

  // type_specific: Required for 5 Element types, disallowed for the others.
  bool should_have_type_specific = (element.type() == fhasp::ElementType::DAI_INTERCONNECT ||
                                    element.type() == fhasp::ElementType::DYNAMICS ||
                                    element.type() == fhasp::ElementType::EQUALIZER ||
                                    element.type() == fhasp::ElementType::GAIN ||
                                    element.type() == fhasp::ElementType::VENDOR_SPECIFIC);
  EXPECT_EQ(element.has_type_specific(), should_have_type_specific)
      << "ElementTypeSpecific should " << (should_have_type_specific ? "" : "not")
      << " be set, for this ElementType";

  // can_bypass: Optional, but DAI_INTERCONNECT and RING_BUFFER elements must not set to true.
  EXPECT_FALSE((element.type() == fhasp::ElementType::DAI_INTERCONNECT ||
                element.type() == fhasp::ElementType::RING_BUFFER) &&
               element.has_can_bypass() && element.can_bypass())
      << "DAI_INTERCONNECT and RING_BUFFER elements must not set 'can_bypass'";
}

// Validate the type-specific portion of this DAI_INTERCONNECT Element.
void AdminTest::ValidateDaiElement(
    const fuchsia::hardware::audio::signalprocessing::Element& element) {
  // type_specific: Required (for this element type) and must be the DAI_INTERCONNECT variant.
  ASSERT_TRUE(element.has_type_specific());
  ASSERT_TRUE(element.type_specific().is_dai_interconnect());

  // plug_detect_capabilities: Required.
  EXPECT_TRUE(element.type_specific().dai_interconnect().has_plug_detect_capabilities());
}

// Validate the type-specific portion of this DYNAMICS Element.
void AdminTest::ValidateDynamicsElement(
    const fuchsia::hardware::audio::signalprocessing::Element& element) {
  // type_specific: Required (for this element type) and must be the DYNAMICS variant.
  ASSERT_TRUE(element.has_type_specific());
  ASSERT_TRUE(element.type_specific().is_dynamics());

  // bands: Required. Cannot be empty.
  ASSERT_TRUE(element.type_specific().dynamics().has_bands());
  ASSERT_FALSE(element.type_specific().dynamics().bands().empty());

  std::unordered_set<uint64_t> band_ids;
  for (const auto& band : element.type_specific().dynamics().bands()) {
    // id: Required. Must be unique within this element
    ASSERT_TRUE(band.has_id());
    EXPECT_FALSE(band_ids.contains(band.id()));
    band_ids.insert(band.id());
  }
}

// Validate the type-specific portion of this EQUALIZER Element.
void AdminTest::ValidateEqualizerElement(
    const fuchsia::hardware::audio::signalprocessing::Element& element) {
  // type_specific: Required (for this element type) and must be the EQUALIZER variant.
  ASSERT_TRUE(element.has_type_specific());
  ASSERT_TRUE(element.type_specific().is_equalizer());
  const auto& eq = element.type_specific().equalizer();

  // bands: Required. Cannot be empty.
  ASSERT_TRUE(eq.has_bands());
  ASSERT_FALSE(eq.bands().empty());

  std::unordered_set<uint64_t> band_ids;
  for (const auto& band : element.type_specific().equalizer().bands()) {
    // id: Required. Must be unique within this element
    ASSERT_TRUE(band.has_id());
    EXPECT_FALSE(band_ids.contains(band.id()));
    band_ids.insert(band.id());
  }

  // min_frequency, max_frequency: Required. min_frequency must <= max_frequency.
  ASSERT_TRUE(eq.has_min_frequency());
  ASSERT_TRUE(eq.has_max_frequency());
  EXPECT_LE(eq.min_frequency(), eq.max_frequency());

  // max_q: Optional. If present, must be finite (like all floats) and positive.
  EXPECT_TRUE(!eq.has_max_q() || (std::isfinite(eq.max_q()) && eq.max_q() > 0.0f));

  // min_gain_db, max_gain_db: Required for 3 suppported_controls values; disallowed otherwise.
  // min_gain_db must <= max_gain_db; both must be finite (like all floats).
  bool requires_gain_db_vals =
      eq.has_supported_controls() && (fhasp::EqualizerSupportedControls::SUPPORTS_TYPE_PEAK ||
                                      fhasp::EqualizerSupportedControls::SUPPORTS_TYPE_LOW_SHELF ||
                                      fhasp::EqualizerSupportedControls::SUPPORTS_TYPE_HIGH_SHELF);
  ASSERT_TRUE(!requires_gain_db_vals || (eq.has_min_gain_db() && std::isfinite(eq.min_gain_db())));
  ASSERT_TRUE(!requires_gain_db_vals || (eq.has_max_gain_db() && std::isfinite(eq.max_gain_db())));
  EXPECT_TRUE(!eq.has_min_gain_db() || !eq.has_max_gain_db() ||
              eq.min_gain_db() <= eq.max_gain_db());
}

// Validate the type-specific portion of this GAIN Element.
void AdminTest::ValidateGainElement(
    const fuchsia::hardware::audio::signalprocessing::Element& element) {
  // type_specific: Required (for this element type) and must be the GAIN variant.
  ASSERT_TRUE(element.has_type_specific());
  ASSERT_TRUE(element.type_specific().is_gain());

  // type: Required.
  ASSERT_TRUE(element.type_specific().gain().has_type());

  // min_gain: Required. Must be finite. Must be non-negative, if GainType is PERCENT.
  ASSERT_TRUE(element.type_specific().gain().has_min_gain());
  ASSERT_TRUE(std::isfinite(element.type_specific().gain().min_gain()));
  EXPECT_TRUE(element.type_specific().gain().type() != fhasp::GainType::PERCENT ||
              element.type_specific().gain().min_gain() >= 0.0);

  // max_gain: Required. Must be finite and >= min_gain. Cannot exceed 100, if GainType is PERCENT.
  ASSERT_TRUE(element.type_specific().gain().has_max_gain());
  ASSERT_TRUE(std::isfinite(element.type_specific().gain().max_gain()));
  EXPECT_GE(element.type_specific().gain().max_gain(), element.type_specific().gain().min_gain());
  EXPECT_TRUE(element.type_specific().gain().type() != fhasp::GainType::PERCENT ||
              element.type_specific().gain().max_gain() <= 100.0);

  // min_gain_step: Required. Must be non-negative. Cannot exceed (max_gain - min_gain).
  ASSERT_TRUE(element.type_specific().gain().has_min_gain_step());
  ASSERT_TRUE(std::isfinite(element.type_specific().gain().min_gain_step()));
  EXPECT_GE(element.type_specific().gain().min_gain_step(), 0.0f);
  EXPECT_LE(element.type_specific().gain().min_gain_step(),
            element.type_specific().gain().max_gain() - element.type_specific().gain().min_gain());
}

// Validate the type-specific portion of this VENDOR_SPECIFIC Element.
void AdminTest::ValidateVendorSpecificElement(
    const fuchsia::hardware::audio::signalprocessing::Element& element) {
  // type_specific: Required (for this element type) and must be the VENDOR_SPECIFIC variant.
  ASSERT_TRUE(element.has_type_specific());
  EXPECT_TRUE(element.type_specific().is_vendor_specific());
}

// Topology methods
//
// First retrieve the element list. If signalprocessing is not supported, exit early;
// otherwise save the ID of a RING_BUFFER element, and the ID of a DAI_INTERCONNECT element.
// We will use these IDs later, when performing Dai-specific and RingBuffer-specific checks.
void AdminTest::RequestTopologies() {
  SignalProcessingConnect();
  RequestElements();

  if (!signalprocessing_is_supported_.value_or(false)) {
    return;
  }
  if (!topologies_.empty()) {
    return;
  }

  signal_processing()->GetTopologies(AddCallback(
      "signalprocessing::Reader::GetTopologies", [this](fhasp::Reader_GetTopologies_Result result) {
        if (result.is_err()) {
          signalprocessing_is_supported_ = false;
          FAIL() << "GetTopologies returned err " << result.err();
          __UNREACHABLE;
        }
        topologies_ = std::move(result.response().topologies);
      }));
  ExpectCallbacks();

  // We only call GetTopologies if we have elements, so we must have at least one topology.
  if (topologies_.empty()) {
    signalprocessing_is_supported_ = false;
    FAIL() << "topologies list is empty";
    __UNREACHABLE;
  }

  std::unordered_set<fhasp::TopologyId> topology_ids;
  for (const auto& topology : topologies_) {
    // All topologies must have an ID and a non-empty list of edges.
    ASSERT_TRUE(topology.has_id());
    ASSERT_TRUE(topology.has_processing_elements_edge_pairs())
        << "Topology " << topology.id() << " processing_elements_edge_pairs is null";
    ASSERT_FALSE(topology.processing_elements_edge_pairs().empty())
        << "Topology " << topology.id() << " processing_elements_edge_pairs is empty";

    // No topology ID may be a duplicate.
    ASSERT_FALSE(topology_ids.contains(topology.id())) << "Duplicate topology ID " << topology.id();
    topology_ids.insert(topology.id());
  }
}

// Having obtained the supported topologies, retrieve the current topology (the default).
void AdminTest::RetrieveInitialTopology() {
  SignalProcessingConnect();
  RequestElements();
  RequestTopologies();

  ASSERT_TRUE(signalprocessing_is_supported_.value_or(false))
      << "signalprocessing is required for this test";

  if (initial_topology_id_.has_value()) {
    return;
  }

  signal_processing()->WatchTopology(AddCallback(
      "signalprocessing::Reader::WatchTopology", [this](fhasp::Reader_WatchTopology_Result result) {
        ASSERT_TRUE(result.is_response());
        initial_topology_id_ = result.response().topology_id;
        current_topology_id_ = initial_topology_id_;
      }));
  ExpectCallbacks();

  // We only call WatchTopology if we support signalprocessing, so a topology should be set now.
  ASSERT_TRUE(current_topology_id_.has_value());
  if (std::ranges::none_of(topologies_, [id = *current_topology_id_](const auto& topo) {
        return (topo.has_id() && topo.id() == id);
      })) {
    // This topology_id is not in the list returned earlier.
    signalprocessing_is_supported_ = false;
    FAIL() << "WatchTopology returned " << *current_topology_id_
           << " which is not in our topology list";
  }
}

void AdminTest::WatchForTopology(fhasp::TopologyId id) {
  ASSERT_TRUE(signalprocessing_is_supported_.value_or(false))
      << "signalprocessing is required for this test";
  // We only call WatchTopology if we support signalprocessing, so a topology should already be set.
  ASSERT_TRUE(current_topology_id_.has_value());
  // We should only expect a response if this represents a change. Don't call this method otherwise.
  ASSERT_NE(id, *current_topology_id_);

  // Make sure we're watching for a topology that is in the supported list.
  if (std::ranges::none_of(topologies_,
                           [id](const auto& t) { return (t.has_id() && t.id() == id); })) {
    // This topology_id is not in the list returned earlier.
    FAIL() << "We shouldn't wait for topology " << id << " which is not in our list";
    __UNREACHABLE;
  }

  signal_processing()->WatchTopology(
      AddCallbackUnordered("signalprocessing::Reader::WatchTopology(update)",
                           [this, id](fhasp::Reader_WatchTopology_Result result) {
                             ASSERT_TRUE(result.is_response());
                             ASSERT_EQ(id, result.response().topology_id);
                             current_topology_id_ = result.response().topology_id;
                           }));
}

// Request a notification if the topology changes -- and fail if we receive one.
void AdminTest::FailOnWatchTopologyCompletion() {
  signal_processing()->WatchTopology(AddUnexpectedCallback("unexpected WatchTopology completion"));
}

// Validate that all of the supported topologies can be set (and restore the default, when done).
void AdminTest::SetAllTopologies() {
  ASSERT_FALSE(topologies_.empty());
  ASSERT_TRUE(initial_topology_id_.has_value());
  ASSERT_TRUE(current_topology_id_.has_value());
  ASSERT_EQ(current_topology_id_, initial_topology_id_);

  if (topologies_.size() == 1) {
    GTEST_SKIP() << "Device has only one topology; we cannot test changing it";
    __UNREACHABLE;
  }

  // Ensure that all the supported topologies can be set.
  for (const auto& t : topologies_) {
    ASSERT_TRUE(t.has_id());
    // Try them all, except the current one (which _should_ be the initial topology).
    if (t.id() == *initial_topology_id_) {
      continue;
    }
    SetTopologyAndExpectCallback(t.id());
  }

  // Now restore the initial topology.
  SetTopologyAndExpectCallback(*initial_topology_id_);
}

// Validate that we can change the current topology and receive notification of the change.
void AdminTest::SetTopologyAndExpectCallback(fhasp::TopologyId id) {
  ASSERT_NE(id, current_topology_id_);
  ASSERT_FALSE(pending_set_topology_id_.has_value());
  pending_set_topology_id_ = id;
  signal_processing()->SetTopology(
      id, AddCallbackUnordered("SetTopology(topology " + std::to_string(id) + ")",
                               [this, id](fhasp::SignalProcessing_SetTopology_Result result) {
                                 ASSERT_FALSE(result.is_err())
                                     << "SetTopology(topology " << id << ") failed";
                                 ASSERT_TRUE(pending_set_topology_id_.has_value());
                                 ASSERT_EQ(*pending_set_topology_id_, id);
                                 pending_set_topology_id_.reset();
                               }));
  ASSERT_TRUE(signal_processing().is_bound()) << "SignalProcessing failed to set the topology";
  WatchForTopology(id);
  ExpectCallbacks();
}

// Issue a topology-change-notification request, but expect our binding to disconnect in response.
// This is used to validate correct behavior in the "Watch while already watching" case.
void AdminTest::WatchTopologyAndExpectDisconnect(zx_status_t expected_error) {
  FailOnWatchTopologyCompletion();
  ExpectError(signal_processing(), expected_error);
  CooldownAfterSignalProcessingDisconnect();
}

// Set the topology to an unsupported TopologyId, but expect our binding to disconnect in response.
void AdminTest::SetTopologyUnknownIdAndExpectError() {
  RequestTopologies();
  ASSERT_FALSE(pending_set_topology_id_.has_value());

  fhasp::TopologyId unknown_id = 0;
  while (std::ranges::any_of(topologies_, [unknown_id](const auto& topo) {
    return topo.has_id() && topo.id() == unknown_id;
  })) {
    ++unknown_id;
  }

  pending_set_topology_id_ = unknown_id;
  signal_processing()->SetTopology(
      unknown_id,
      AddCallback("SetTopology(topology " + std::to_string(unknown_id) + ")",
                  [this, unknown_id](fhasp::SignalProcessing_SetTopology_Result result) {
                    pending_set_topology_id_.reset();
                    ASSERT_TRUE(result.is_err())
                        << "SetTopology(topology " << unknown_id << ") did not fail";
                    EXPECT_EQ(result.err(), ZX_ERR_INVALID_ARGS);
                  }));
  ASSERT_TRUE(signal_processing().is_bound())
      << "SignalProcessing disconnected after setting an unknown topology";
  FailOnWatchTopologyCompletion();
  ExpectCallbacks();
}

void AdminTest::SetTopologyNoChangeAndExpectNoWatch() {
  ASSERT_TRUE(current_topology_id_.has_value());
  auto id = *current_topology_id_;

  FailOnWatchTopologyCompletion();

  signal_processing()->SetTopology(
      id, AddCallbackUnordered("SetTopology[no-change](topology " + std::to_string(id) + ")",
                               [id](fhasp::SignalProcessing_SetTopology_Result result) {
                                 EXPECT_FALSE(result.is_err())
                                     << "SetTopology[no-change](topology " << id << ") failed";
                               }));
  ASSERT_TRUE(signal_processing().is_bound()) << "SignalProcessing failed to set the topology";

  ExpectCallbacks();
}

// Validate that the collection of element IDs found in the topology list are complete and correct.
void AdminTest::ValidateElementTopologyClosure() {
  ASSERT_FALSE(elements().empty());
  std::unordered_set<fhasp::ElementId> all_element_ids;
  for (const auto& element : elements()) {
    ASSERT_FALSE(all_element_ids.contains(element.id()));
    all_element_ids.insert(element.id());
  }
  std::unordered_set<fhasp::ElementId> unused_element_ids = all_element_ids;

  ASSERT_FALSE(topologies().empty());
  for (const auto& topology : topologies()) {
    std::unordered_map<fhasp::ElementId, uint64_t> edge_source_id_counts, edge_dest_id_counts;
    ASSERT_FALSE(topology.processing_elements_edge_pairs().empty());
    for (const auto& edge_pair : topology.processing_elements_edge_pairs()) {
      // Only succeeds if the ElementId is not yet found in the map.
      edge_source_id_counts.insert({edge_pair.processing_element_id_from, 0});
      edge_dest_id_counts.insert({edge_pair.processing_element_id_to, 0});

      // Always succeeds.
      edge_source_id_counts[edge_pair.processing_element_id_from] += 1;
      edge_dest_id_counts[edge_pair.processing_element_id_to] += 1;
    }

    for (const auto& edge_pair : topology.processing_elements_edge_pairs()) {
      ASSERT_TRUE(all_element_ids.contains(edge_pair.processing_element_id_from))
          << "Topology " << topology.id() << " contains unknown element ID "
          << edge_pair.processing_element_id_from;
      ASSERT_TRUE(all_element_ids.contains(edge_pair.processing_element_id_to))
          << "Topology " << topology.id() << " contains unknown element ID "
          << edge_pair.processing_element_id_to;
      unused_element_ids.erase(edge_pair.processing_element_id_from);
      unused_element_ids.erase(edge_pair.processing_element_id_to);

      // An element can be topologically self-referential (have a 'to' edge and 'from' edge which
      // both point back to itself), but only if this is the only edgepair in the topology that
      // references the element, and only if this self-referencing element is a DAI_INTERCONNECT.
      if (edge_pair.processing_element_id_from == edge_pair.processing_element_id_to) {
        ASSERT_EQ(edge_source_id_counts[edge_pair.processing_element_id_from], 1u)
            << "Element in a self-referential edgepair must not also connect to another element";
        ASSERT_EQ(edge_dest_id_counts[edge_pair.processing_element_id_to], 1u)
            << "No other element may also connect to the element in a self-referential edgepair";

        for (const auto& element : elements()) {
          if (element.id() == edge_pair.processing_element_id_from) {
            ASSERT_EQ(element.type(), fhasp::ElementType::DAI_INTERCONNECT)
                << "Self-referential elements must be DAI_INTERCONNECT";
          }
        }
      }
    }
    for (const auto& [source_id, _count] : edge_source_id_counts) {
      fhasp::ElementType source_element_type;
      for (const auto& element : elements()) {
        if (element.id() == source_id) {
          source_element_type = element.type();
        }
      }
      if (edge_dest_id_counts.contains(source_id)) {
        if constexpr (!kTolerateNonTerminalDaiEndpoints) {
          ASSERT_NE(source_element_type, fhasp::ElementType::DAI_INTERCONNECT)
              << "Element " << source_id << " is not an endpoint in topology " << topology.id()
              << ", but is DAI_INTERCONNECT";
        }
        ASSERT_NE(source_element_type, fhasp::ElementType::RING_BUFFER)
            << "Element " << source_id << " is not an endpoint in topology " << topology.id()
            << ", but is RING_BUFFER";
        ASSERT_NE(source_element_type, fhasp::ElementType::PACKET_STREAM)
            << "Element " << source_id << " is not an endpoint in topology " << topology.id()
            << ", but is PACKET_STREAM";
        edge_dest_id_counts[source_id] -= 1;
        if (edge_dest_id_counts[source_id] == 0) {
          edge_dest_id_counts.erase(source_id);
        }
      } else {
        ASSERT_TRUE(source_element_type == fhasp::ElementType::DAI_INTERCONNECT ||
                    source_element_type == fhasp::ElementType::RING_BUFFER ||
                    source_element_type == fhasp::ElementType::PACKET_STREAM)
            << "Element " << source_id << " is a terminal (source) endpoint in topology "
            << topology.id() << ", but is neither DAI_INTERCONNECT, RING_BUFFER, nor PACKET_STREAM";
      }
    }
    for (const auto& [dest_id, _count] : edge_dest_id_counts) {
      fhasp::ElementType dest_element_type;
      for (const auto& element : elements()) {
        if (element.id() == dest_id) {
          dest_element_type = element.type();
        }
      }
      ASSERT_TRUE(dest_element_type == fhasp::ElementType::DAI_INTERCONNECT ||
                  dest_element_type == fhasp::ElementType::RING_BUFFER ||
                  dest_element_type == fhasp::ElementType::PACKET_STREAM)
          << "Element " << dest_id << " is a terminal (destination) endpoint in topology "
          << topology.id() << ", but is neither DAI_INTERCONNECT, RING_BUFFER, nor PACKET_STREAM";
    }
  }
  ASSERT_TRUE(unused_element_ids.empty())
      << unused_element_ids.size() << "elements (including ID " << *unused_element_ids.cbegin()
      << ") were not referenced in any topology";
}

// ElementState methods
//
void AdminTest::RetrieveInitialElementStates() {
  ASSERT_TRUE(signalprocessing_is_supported_.value_or(false))
      << "signalprocessing is required for this test";
  ASSERT_FALSE(elements_.empty());

  if (!initial_element_states_.empty()) {
    return;
  }

  for (const auto& e : elements_) {
    ASSERT_TRUE(e.has_id());

    fhasp::ElementState state;
    signal_processing()->WatchElementState(
        e.id(),
        AddCallback("initial WatchElementState(element " + std::to_string(e.id()) + ")",
                    [&state](const fhasp::ElementState& result) { state = fidl::Clone(result); }));
    ExpectCallbacks();

    initial_element_states_.emplace(e.id(), std::move(state));
  }
}

// Request a notification if this element's state changes -- and fail if we receive one.
void AdminTest::FailOnWatchElementStateCompletion(fhasp::ElementId id) {
  signal_processing()->WatchElementState(
      id, AddUnexpectedCallback("unexpected WatchElementState completion"));
}

// Request an element-state-change-notification, but expect our binding to disconnect in response.
// This is used to validate correct behavior in the "Watch while already watching" case.
void AdminTest::WatchElementStateAndExpectDisconnect(fhasp::ElementId id,
                                                     zx_status_t expected_error) {
  FailOnWatchElementStateCompletion(id);
  ExpectError(signal_processing(), expected_error);
  CooldownAfterSignalProcessingDisconnect();
}

// Request state-change notification for an unknown ElementId, expecting a disconnect in response.
void AdminTest::WatchElementStateUnknownIdAndExpectDisconnect(zx_status_t expected_error) {
  fhasp::ElementId unknown_id = 0;
  while (std::ranges::any_of(
      elements_, [unknown_id](const auto& el) { return el.has_id() && el.id() == unknown_id; })) {
    ++unknown_id;
  }

  signal_processing()->WatchElementState(
      unknown_id,
      AddUnexpectedCallback("WatchElementState(element " + std::to_string(unknown_id) + ")"));
  ASSERT_TRUE(signal_processing().is_bound())
      << "SignalProcessing disconnected after setting an unknown topology";
  ExpectError(signal_processing(), expected_error);
  CooldownAfterSignalProcessingDisconnect();
}

// Validate each ElementState, considering the Element that produced it.
void AdminTest::ValidateElementStates() {
  ASSERT_FALSE(initial_element_states_.empty());

  for (const auto& element : elements()) {
    ASSERT_TRUE(element.has_id());
    ASSERT_TRUE(initial_element_states_.contains(element.id()));
    const auto& state = initial_element_states_.at(element.id());
    ValidateElementState(element, state);
  }
}

// For DAI_INTERCONNECT elements, validate the type-specific portions of their ElementStates.
void AdminTest::ValidateDaiElementStates() {
  bool found_one = false;
  for (const auto& element : elements()) {
    ASSERT_TRUE(element.has_type());
    if (element.type() == fhasp::ElementType::DAI_INTERCONNECT) {
      found_one = true;
      ASSERT_TRUE(element.has_id());
      ASSERT_TRUE(initial_element_states_.contains(element.id()));
      const auto& state = initial_element_states_.at(element.id());
      ValidateDaiElementState(element, state);
    }
  }
  if (!found_one) {
    GTEST_SKIP() << "No DAI_INTERCONNECT Elements, cannot validate DAI_INTERCONNECT ElementState";
  }
}

// For DYNAMICS elements, validate the type-specific portions of their ElementStates.
void AdminTest::ValidateDynamicsElementStates() {
  bool found_one = false;
  for (const auto& element : elements()) {
    ASSERT_TRUE(element.has_type());
    if (element.type() == fhasp::ElementType::DYNAMICS) {
      found_one = true;
      ASSERT_TRUE(element.has_id());
      ASSERT_TRUE(initial_element_states_.contains(element.id()));
      const auto& state = initial_element_states_.at(element.id());
      ValidateDynamicsElementState(element, state);
    }
  }
  if (!found_one) {
    GTEST_SKIP() << "No DYNAMICS Elements, cannot validate DYNAMICS ElementState";
  }
}

// For EQUALIZER elements, validate the type-specific portions of their ElementStates.
void AdminTest::ValidateEqualizerElementStates() {
  bool found_one = false;
  for (const auto& element : elements()) {
    ASSERT_TRUE(element.has_type());
    if (element.type() == fhasp::ElementType::EQUALIZER) {
      found_one = true;
      ASSERT_TRUE(element.has_id());
      ASSERT_TRUE(initial_element_states_.contains(element.id()));
      const auto& state = initial_element_states_.at(element.id());
      ValidateEqualizerElementState(element, state);
    }
  }
  if (!found_one) {
    GTEST_SKIP() << "No EQUALIZER Elements, cannot validate EQUALIZER ElementState";
  }
}

// For GAIN elements, validate the type-specific portions of their ElementStates.
void AdminTest::ValidateGainElementStates() {
  bool found_one = false;
  for (const auto& element : elements()) {
    ASSERT_TRUE(element.has_type());
    if (element.type() == fhasp::ElementType::GAIN) {
      found_one = true;
      ASSERT_TRUE(element.has_id());
      ASSERT_TRUE(initial_element_states_.contains(element.id()));
      const auto& state = initial_element_states_.at(element.id());
      ValidateGainElementState(element, state);
    }
  }
  if (!found_one) {
    GTEST_SKIP() << "No GAIN Elements, cannot validate GAIN ElementState";
  }
}

// For VENDOR_SPECIFIC elements, validate the type-specific portions of their ElementStates.
void AdminTest::ValidateVendorSpecificElementStates() {
  bool found_one = false;
  for (const auto& element : elements()) {
    ASSERT_TRUE(element.has_type());
    if (element.type() == fhasp::ElementType::VENDOR_SPECIFIC) {
      found_one = true;
      ASSERT_TRUE(element.has_id());
      ASSERT_TRUE(initial_element_states_.contains(element.id()));
      const auto& state = initial_element_states_.at(element.id());
      ValidateVendorSpecificElementState(element, state);
    }
  }
  if (!found_one) {
    GTEST_SKIP() << "No VENDOR_SPECIFIC Elements, cannot validate VENDOR_SPECIFIC ElementState";
  }
}

// Validate the non-type-specific portion of this ElementState.
void AdminTest::ValidateElementState(const fhasp::Element& element,
                                     const fhasp::ElementState& state) {
  ASSERT_TRUE(element.has_type());

  // state: Required. If stopped then the element must support this.
  ASSERT_TRUE(state.has_started());
  EXPECT_TRUE(state.started() || (element.has_can_stop() && element.can_stop()));

  // type_specific: "Optional". Actually required for 5 element types, disallowed for the others.
  bool element_should_have_type_specific_state =
      (element.type() == fhasp::ElementType::DAI_INTERCONNECT ||
       element.type() == fhasp::ElementType::DYNAMICS ||
       element.type() == fhasp::ElementType::EQUALIZER ||
       element.type() == fhasp::ElementType::GAIN ||
       element.type() == fhasp::ElementType::VENDOR_SPECIFIC);
  EXPECT_TRUE(state.has_type_specific() == element_should_have_type_specific_state);

  // bypassed: Optional. If present and set, then the element must support this.
  EXPECT_TRUE(!state.has_bypassed() || !state.bypassed() || element.can_bypass());

  // delays: Optional. If present then cannot be negative.
  EXPECT_TRUE(!state.has_turn_on_delay() || state.turn_on_delay() >= 0);
  EXPECT_TRUE(!state.has_turn_off_delay() || state.turn_off_delay() >= 0);
  EXPECT_TRUE(!state.has_processing_delay() || state.processing_delay() >= 0);

  // vendor_specific_data: Optional (for ALL element types). If present, cannot be empty.
  EXPECT_FALSE(state.has_vendor_specific_data() && state.vendor_specific_data().empty());
}

// Validate the type-specific portion of the ElementState returned by a DAI_INTERCONNECT element.
void AdminTest::ValidateDaiElementState(const fhasp::Element& element,
                                        const fhasp::ElementState& state) {
  ASSERT_TRUE(element.type_specific().dai_interconnect().has_plug_detect_capabilities());

  // type_specific: Required. Must contain the variant that matches its Element (DAI_INTERCONNECT).
  ASSERT_TRUE(state.has_type_specific());
  ASSERT_TRUE(state.type_specific().is_dai_interconnect());

  // plug_state: Required.
  ASSERT_TRUE(state.type_specific().dai_interconnect().has_plug_state());

  // plug_state.plugged: Required. If unplugged, the element must support that.
  ASSERT_TRUE(state.type_specific().dai_interconnect().plug_state().has_plugged());
  EXPECT_TRUE(element.type_specific().dai_interconnect().plug_detect_capabilities() ==
                  fhasp::PlugDetectCapabilities::CAN_ASYNC_NOTIFY ||
              state.type_specific().dai_interconnect().plug_state().plugged());

  // plug_state.plug_state_time: Required. Must be >= 0
  ASSERT_TRUE(state.type_specific().dai_interconnect().plug_state().has_plug_state_time());
  EXPECT_GE(state.type_specific().dai_interconnect().plug_state().plug_state_time(), 0);

  // external_delay: Optional. If present, must be non-negative.
  EXPECT_TRUE(!state.type_specific().dai_interconnect().has_external_delay() ||
              state.type_specific().dai_interconnect().external_delay() >= 0);
}

// Validate the type-specific portion of the ElementState returned by a DYNAMICS element.
void AdminTest::ValidateDynamicsElementState(const fhasp::Element& element,
                                             const fhasp::ElementState& state) {
  ASSERT_TRUE(element.type_specific().dynamics().has_bands());
  ASSERT_FALSE(element.type_specific().dynamics().bands().empty());
  std::unordered_set<uint64_t> band_ids;
  for (const auto& band : element.type_specific().dynamics().bands()) {
    ASSERT_TRUE(band.has_id());
    band_ids.insert(band.id());
  }

  // type_specific: Required. Must contain the variant that matches its Element (DYNAMICS).
  ASSERT_TRUE(state.has_type_specific());
  ASSERT_TRUE(state.type_specific().is_dynamics());

  // band_states: Required. Cannot be empty.
  ASSERT_TRUE(state.type_specific().dynamics().has_band_states());
  ASSERT_FALSE(state.type_specific().dynamics().band_states().empty());

  for (const auto& band_state : state.type_specific().dynamics().band_states()) {
    // id: Required. Must be found in Element.TypeSpecific.Dynamics.bands.
    ASSERT_TRUE(band_state.has_id());
    ASSERT_TRUE(band_ids.contains(band_state.id())) << "Unknown dynamics band ID";
    (void)band_ids.extract(band_state.id());

    // min_frequency and max_frequency: Required. Min must <= max.
    ASSERT_TRUE(band_state.has_min_frequency());
    ASSERT_TRUE(band_state.has_max_frequency());
    EXPECT_LE(band_state.min_frequency(), band_state.max_frequency());

    // threshold_db: Required. Like all floats, must be finite.
    ASSERT_TRUE(band_state.has_threshold_db());
    EXPECT_TRUE(std::isfinite(band_state.threshold_db()));

    // threshold_type: Required.
    EXPECT_TRUE(band_state.has_threshold_type());

    // ratio: Required. Like all floats, it must be finite.
    ASSERT_TRUE(band_state.has_ratio());
    EXPECT_TRUE(std::isfinite(band_state.ratio()));

    // knee_width_db: Optional. If present, like all floats it must be finite.
    EXPECT_TRUE(!band_state.has_knee_width_db() || std::isfinite(band_state.knee_width_db()));

    // attack: Optional. If present, like all durations it must be non-negative.
    EXPECT_TRUE(!band_state.has_attack() || band_state.attack() >= 0);

    // release: Optional. If present, like all durations it must be non-negative.
    EXPECT_TRUE(!band_state.has_release() || band_state.release() >= 0);

    // output_gain_db: Optional. If present, like all floats it must be finite.
    EXPECT_TRUE(!band_state.has_output_gain_db() || std::isfinite(band_state.output_gain_db()));

    // input_gain_db: Optional. If present, like all floats it must be finite.
    EXPECT_TRUE(!band_state.has_input_gain_db() || std::isfinite(band_state.input_gain_db()));

    // lookahead: Optional. If present, like all durations it must be non-negative.
    EXPECT_TRUE(!band_state.has_lookahead() || band_state.lookahead() >= 0);
  }

  // band_states must contain every band_id in the element's TypeSpecific.Dynamics.bands.
  EXPECT_TRUE(band_ids.empty()) << "DynamicsState did not contain every band ID";
}

// Validate the type-specific portion of the ElementState returned by an EQUALIZER element.
void AdminTest::ValidateEqualizerElementState(const fhasp::Element& element,
                                              const fhasp::ElementState& state) {
  ASSERT_TRUE(element.type_specific().equalizer().has_bands());
  std::unordered_set<uint64_t> band_ids;
  for (const auto& band : element.type_specific().equalizer().bands()) {
    ASSERT_TRUE(band.has_id());
    band_ids.insert(band.id());
  }

  // type_specific: Required. Must contain the variant that matches its Element (EQUALIZER).
  ASSERT_TRUE(state.has_type_specific());
  ASSERT_TRUE(state.type_specific().is_equalizer());

  // band_states: Required. Cannot be empty.
  ASSERT_TRUE(state.type_specific().equalizer().has_band_states());
  ASSERT_FALSE(state.type_specific().equalizer().band_states().empty());

  for (const auto& band_state : state.type_specific().equalizer().band_states()) {
    // id: Required. Must be found in the element's TypeSpecific.Equalizer.bands.
    ASSERT_TRUE(band_state.has_id());
    ASSERT_TRUE(band_ids.contains(band_state.id())) << "Unknown EQ band ID";
    (void)band_ids.extract(band_state.id());

    // type: Required (Optional in FIDL definition, but without it a band has no function).
    ASSERT_TRUE(band_state.has_type());

    // frequency: Required (Optional in FIDL definition, but without it a band cannot function).
    EXPECT_TRUE(band_state.has_frequency());

    // q: Optional but must be positive and finite, if present
    EXPECT_TRUE(!band_state.has_q() || (std::isfinite(band_state.q()) && band_state.q() > 0.0));

    // gain_db: Required for peak/shelves, disallowed for notch/cuts.
    EXPECT_TRUE(!band_state.has_gain_db() || std::isfinite(band_state.gain_db()));
    EXPECT_TRUE(band_state.has_gain_db() ==
                (band_state.type() == fhasp::EqualizerBandType::PEAK ||
                 band_state.type() == fhasp::EqualizerBandType::LOW_SHELF ||
                 band_state.type() == fhasp::EqualizerBandType::HIGH_SHELF));
  }
  // band_states must contain every band_id in the element's TypeSpecific.Equalizer.bands.
  EXPECT_TRUE(band_ids.empty()) << "EqualizerState did not contain every band ID";
}

// Validate the type-specific portion of the ElementState returned by a GAIN element.
void AdminTest::ValidateGainElementState(const fhasp::Element& element,
                                         const fhasp::ElementState& state) {
  ASSERT_TRUE(element.type_specific().gain().has_min_gain());
  ASSERT_TRUE(element.type_specific().gain().has_max_gain());

  // type_specific: Required. Must contain the variant that matches its Element (GAIN).
  ASSERT_TRUE(state.has_type_specific());
  ASSERT_TRUE(state.type_specific().is_gain());

  // gain: Required. Must be finite and within [min_gain, max_gain].
  ASSERT_TRUE(state.type_specific().gain().has_gain());
  ASSERT_TRUE(std::isfinite(state.type_specific().gain().gain()));
  EXPECT_GE(state.type_specific().gain().gain(), element.type_specific().gain().min_gain());
  EXPECT_LE(state.type_specific().gain().gain(), element.type_specific().gain().max_gain());
}

// Validate the type-specific portion of the ElementState returned by a VENDOR_SPECIFIC element.
void AdminTest::ValidateVendorSpecificElementState(const fhasp::Element& element,
                                                   const fhasp::ElementState& state) {
  // type_specific: Required. Must contain the variant that matches its Element (VENDOR_SPECIFIC).
  ASSERT_TRUE(state.has_type_specific());
  EXPECT_TRUE(state.type_specific().is_vendor_specific());
}

// Validate the ability to change non-type-specific state, for all elements.
void AdminTest::SetAllElementStates() {
  ASSERT_FALSE(elements().empty());
  ASSERT_FALSE(initial_element_states_.empty());

  bool found_one = false;
  for (const auto& element : elements()) {
    ASSERT_TRUE(element.has_id());
    ASSERT_TRUE(initial_element_states_.contains(element.id()));
    const auto& initial_state = initial_element_states_.at(element.id());

    if ((!element.has_can_stop() || !element.can_stop()) &&
        (!element.has_can_bypass() || !element.can_bypass())) {
      // Element cannot be stopped or bypassed, so we can't test non-type-specific SetElementState
      // with this element. Keep trying the other elements, though.
      continue;
    }

    found_one = true;
    TestSetElementState(element, initial_state);
  }
  if (!found_one) {
    GTEST_SKIP() << "Can't stop/bypass any elements: cannot test non-type-specific SetElementState";
  }
}

// Validate the ability to change type-specific state, for DYNAMICS elements.
void AdminTest::SetAllDynamicsElementStates() {
  ASSERT_FALSE(elements().empty());
  ASSERT_FALSE(initial_element_states_.empty());

  bool found_one = false;
  for (const auto& element : elements()) {
    ASSERT_TRUE(element.has_type());
    if (element.type() == fhasp::ElementType::DYNAMICS) {
      found_one = true;
      ASSERT_TRUE(element.has_type_specific());
      ASSERT_TRUE(element.type_specific().is_dynamics());

      ASSERT_TRUE(element.has_id());
      ASSERT_TRUE(initial_element_states_.contains(element.id()));
      const auto& initial_state = initial_element_states_.at(element.id());
      ASSERT_TRUE(initial_state.has_type_specific());
      ASSERT_TRUE(initial_state.type_specific().is_dynamics());

      TestSetDynamicsElementState(element, initial_state);
    }
  }
  if (!found_one) {
    GTEST_SKIP() << "No DYNAMICS elements, cannot test type-specific ElementState changes";
  }
}

// Validate the ability to change type-specific state, for EQUALIZER elements.
void AdminTest::SetAllEqualizerElementStates() {
  ASSERT_FALSE(elements().empty());
  ASSERT_FALSE(initial_element_states_.empty());

  bool found_one = false;
  for (const auto& element : elements()) {
    ASSERT_TRUE(element.has_type());
    if (element.type() == fhasp::ElementType::EQUALIZER) {
      found_one = true;
      ASSERT_TRUE(element.has_type_specific());
      ASSERT_TRUE(element.type_specific().is_equalizer());

      ASSERT_TRUE(element.has_id());
      ASSERT_TRUE(initial_element_states_.contains(element.id()));
      const auto& initial_state = initial_element_states_.at(element.id());
      ASSERT_TRUE(initial_state.has_type_specific());
      ASSERT_TRUE(initial_state.type_specific().is_equalizer());

      TestSetEqualizerElementState(element, initial_state);
    }
  }
  if (!found_one) {
    GTEST_SKIP() << "No EQUALIZER elements, cannot test type-specific ElementState changes";
  }
}

// Validate the ability to change type-specific state, for GAIN elements.
void AdminTest::SetAllGainElementStates() {
  ASSERT_FALSE(elements().empty());
  ASSERT_FALSE(initial_element_states_.empty());

  bool found_one = false;
  for (const auto& element : elements()) {
    ASSERT_TRUE(element.has_type());
    if (element.type() == fhasp::ElementType::GAIN) {
      found_one = true;
      ASSERT_TRUE(element.has_type_specific());
      ASSERT_TRUE(element.type_specific().is_gain());

      ASSERT_TRUE(element.has_id());
      ASSERT_TRUE(initial_element_states_.contains(element.id()));
      const auto& initial_state = initial_element_states_.at(element.id());
      ASSERT_TRUE(initial_state.has_type_specific());
      ASSERT_TRUE(initial_state.type_specific().is_gain());

      TestSetGainElementState(element, initial_state);
    }
  }
  if (!found_one) {
    GTEST_SKIP() << "No GAIN elements, cannot test type-specific ElementState changes";
  }
}

// Validate the ability to change type-specific state, for GAIN elements.
void AdminTest::SetAllGainElementStatesNoChange() {
  ASSERT_FALSE(elements().empty());
  ASSERT_FALSE(initial_element_states_.empty());

  bool found_one = false;
  for (const auto& element : elements()) {
    ASSERT_TRUE(element.has_type());
    if (element.type() == fhasp::ElementType::GAIN) {
      found_one = true;
      ASSERT_TRUE(element.has_type_specific());
      ASSERT_TRUE(element.type_specific().is_gain());

      ASSERT_TRUE(element.has_id());
      ASSERT_TRUE(initial_element_states_.contains(element.id()));
      const auto& initial_state = initial_element_states_.at(element.id());
      ASSERT_TRUE(initial_state.has_type_specific());
      ASSERT_TRUE(initial_state.type_specific().is_gain());
      ASSERT_TRUE(initial_state.type_specific().gain().has_gain());

      TestSetGainElementStateNoChange(element.id(), initial_state.type_specific().gain().gain());
    }
  }
  if (!found_one) {
    GTEST_SKIP() << "No GAIN elements, cannot test type-specific ElementState changes";
  }
}

// Validate the ability to change type-specific state, for GAIN elements.
void AdminTest::SetAllGainElementStatesInvalidGainShouldError() {
  ASSERT_FALSE(elements().empty());
  ASSERT_FALSE(initial_element_states_.empty());

  bool found_one = false;
  for (const auto& element : elements()) {
    ASSERT_TRUE(element.has_type());
    if (element.type() == fhasp::ElementType::GAIN) {
      found_one = true;
      ASSERT_TRUE(element.has_type_specific());
      ASSERT_TRUE(element.type_specific().is_gain());

      ASSERT_TRUE(element.has_id());
      ASSERT_TRUE(initial_element_states_.contains(element.id()));
      const auto& initial_state = initial_element_states_.at(element.id());
      ASSERT_TRUE(initial_state.has_type_specific());
      ASSERT_TRUE(initial_state.type_specific().is_gain());

      TestSetGainElementStateInvalidGain(element.id());
    }
  }
  if (!found_one) {
    GTEST_SKIP() << "No GAIN elements, cannot test type-specific ElementState changes";
  }
}

// Validate that a SetElementState with no-change does not trigger a WatchElementState completion.
void AdminTest::SetAllElementStatesNoChange() {
  ASSERT_FALSE(elements().empty());
  ASSERT_FALSE(initial_element_states_.empty());

  for (const auto& element : elements()) {
    FailOnWatchElementStateCompletion(element.id());
    SetElementStateNoChange(element.id());
  }
}

void AdminTest::SetElementStateNoChange(fhasp::ElementId id) {
  signal_processing()->SetElementState(
      id, fhasp::SettableElementState{},
      AddCallbackUnordered("SetElementState[no-change](element " + std::to_string(id) + ")",
                           [id](fhasp::SignalProcessing_SetElementState_Result result) {
                             EXPECT_FALSE(result.is_err())
                                 << "SetElementState[no-change](element " << id << ") failed";
                           }));
  EXPECT_TRUE(signal_processing().is_bound()) << "SignalProcessing failed to set the ElementState";
}

// SetElementState for an unsupported ElementId, and expect our completion to contain an error.
// However, our signalprocessing binding should not disconnect as a result.
void AdminTest::SetElementStateUnknownIdAndExpectError() {
  fhasp::ElementId unknown_id = 0;
  while (std::ranges::any_of(elements_, [&unknown_id](const auto& element) {
    return element.has_id() && element.id() == unknown_id;
  })) {
    ++unknown_id;
  }

  signal_processing()->SetElementState(
      unknown_id, fuchsia::hardware::audio::signalprocessing::SettableElementState{},
      AddCallback("SetElementState[unknown](element " + std::to_string(unknown_id) + ")",
                  [unknown_id](fhasp::SignalProcessing_SetElementState_Result result) {
                    ASSERT_TRUE(result.is_err())
                        << "SetElementState[unknown](element " << unknown_id << ") did not fail";
                    EXPECT_EQ(result.err(), ZX_ERR_INVALID_ARGS);
                  }));
  ASSERT_TRUE(signal_processing().is_bound())
      << "SignalProcessing disconnected after trying to set element state for an unknown id";
  ExpectCallbacks();
}

// Validate the ability to change non-type-specific state for this specific element.
void AdminTest::TestSetElementState(const fhasp::Element& element,
                                    const fhasp::ElementState& initial_state) {
  ASSERT_TRUE(element.has_id());
  // Change what we can, in the base (non-TypeSpecific) SettableElementState.
  // Don't use vendor_specific_data since we cannot predict how a driver uses this.
  {
    bool can_stop = element.has_can_stop() && element.can_stop();
    bool can_bypass = element.has_can_bypass() && element.can_bypass();
    // We only call this method for elements that can stop or bypass.
    ASSERT_TRUE(can_stop || can_bypass) << "element_id " << element.id();

    fhasp::SettableElementState state;
    if (can_stop) {
      // Alter 'started' state, else WatchElementState call will not return.
      state.set_started(!initial_state.has_started() || !initial_state.started());
    } else {
      state.set_started(true);
    }
    if (can_bypass) {
      state.set_bypassed(!(initial_state.has_bypassed() && initial_state.bypassed()));
    } else {
      state.set_bypassed(false);
    }
    bool new_started = state.started();
    bool new_bypassed = state.bypassed();

    signal_processing()->WatchElementState(
        element.id(), AddCallbackUnordered(
                          "WatchElementState[1](element " + std::to_string(element.id()) + ")",
                          [&element, new_started, new_bypassed](const fhasp::ElementState& result) {
                            ValidateElementState(element, result);

                            EXPECT_EQ(new_started, !result.has_started() || result.started())
                                << "element_id " << element.id();
                            EXPECT_EQ(new_bypassed, result.has_bypassed() && result.bypassed())
                                << "element_id " << element.id();
                          }));

    signal_processing()->SetElementState(
        element.id(), std::move(state),
        AddCallbackUnordered(
            "SetElementState[1](element " + std::to_string(element.id()) + ")",
            [id = element.id()](fhasp::SignalProcessing_SetElementState_Result result) {
              EXPECT_FALSE(result.is_err()) << "SetElementState(element " << id << ") failed";
            }));
    ASSERT_TRUE(signal_processing().is_bound())
        << "SignalProcessing failed to set the ElementState(id " << element.id() << ")";
    ExpectCallbacks();
  }

  // Now restore the initial element state.
  {
    fhasp::SettableElementState state;
    state.set_started(!initial_state.has_started() || initial_state.started());
    state.set_bypassed(initial_state.has_bypassed() && initial_state.bypassed());

    signal_processing()->WatchElementState(
        element.id(),
        AddCallbackUnordered("WatchElementState[2](element " + std::to_string(element.id()) + ")",
                             [&element, &initial_state](const fhasp::ElementState& result) {
                               ValidateElementState(element, result);

                               EXPECT_EQ(!initial_state.has_started() || initial_state.started(),
                                         !result.has_started() || result.started())
                                   << "element_id " << element.id();
                               EXPECT_EQ(initial_state.has_bypassed() && initial_state.bypassed(),
                                         result.has_bypassed() && result.bypassed())
                                   << "element_id " << element.id();
                             }));

    signal_processing()->SetElementState(
        element.id(), std::move(state),
        AddCallbackUnordered(
            "SetElementState[2](element " + std::to_string(element.id()) + ")",
            [id = element.id()](fhasp::SignalProcessing_SetElementState_Result result) {
              EXPECT_FALSE(result.is_err()) << "SetElementState(element " << id << ") failed";
            }));
    ASSERT_TRUE(signal_processing().is_bound())
        << "SignalProcessing failed to restore the ElementState(id " << element.id() << ")";
    ExpectCallbacks();
  }
}

// Validate the ability to change type-specific state for this specific DYNAMICS element.
void AdminTest::TestSetDynamicsElementState(const fhasp::Element& element,
                                            const fhasp::ElementState& initial_state) {
  ASSERT_TRUE(element.has_type_specific());
  ASSERT_TRUE(element.type_specific().is_dynamics());
  ASSERT_TRUE(element.type_specific().dynamics().has_bands() &&
              !element.type_specific().dynamics().bands().empty());

  // Change what we can, in SettableElementState::DynamicsElementState
  {
    std::vector<::fuchsia::hardware::audio::signalprocessing::DynamicsBandState> new_band_states;
    for (const auto& old_band_state : initial_state.type_specific().dynamics().band_states()) {
      fhasp::DynamicsBandState new_band_state;
      uint32_t band_id = static_cast<uint32_t>(old_band_state.id());
      new_band_state.set_id(band_id);
      new_band_state.set_min_frequency(20 + band_id);
      new_band_state.set_max_frequency(10'000 + band_id);
      new_band_state.set_threshold_db(static_cast<float>(band_id));
      new_band_state.set_ratio(2.0f + static_cast<float>(band_id));

      if (element.type_specific().dynamics().has_supported_controls() &&
          element.type_specific().dynamics().supported_controls() &
              fuchsia::hardware::audio::signalprocessing::DynamicsSupportedControls::
                  THRESHOLD_TYPE) {
        new_band_state.set_threshold_type(fhasp::ThresholdType::ABOVE);
      }
      if (element.type_specific().dynamics().has_supported_controls() &&
          element.type_specific().dynamics().supported_controls() &
              fuchsia::hardware::audio::signalprocessing::DynamicsSupportedControls::KNEE_WIDTH) {
        new_band_state.set_knee_width_db(1.0f + static_cast<float>(band_id));
      }
      if (element.type_specific().dynamics().has_supported_controls() &&
          element.type_specific().dynamics().supported_controls() &
              fuchsia::hardware::audio::signalprocessing::DynamicsSupportedControls::ATTACK) {
        new_band_state.set_attack(1'000'000 + band_id);
      }
      if (element.type_specific().dynamics().has_supported_controls() &&
          element.type_specific().dynamics().supported_controls() &
              fuchsia::hardware::audio::signalprocessing::DynamicsSupportedControls::RELEASE) {
        new_band_state.set_release(5'000'000 + band_id);
      }
      if (element.type_specific().dynamics().has_supported_controls() &&
          element.type_specific().dynamics().supported_controls() &
              fuchsia::hardware::audio::signalprocessing::DynamicsSupportedControls::OUTPUT_GAIN) {
        new_band_state.set_output_gain_db(-3.0f + static_cast<float>(band_id));
      }
      if (element.type_specific().dynamics().has_supported_controls() &&
          element.type_specific().dynamics().supported_controls() &
              fuchsia::hardware::audio::signalprocessing::DynamicsSupportedControls::INPUT_GAIN) {
        new_band_state.set_input_gain_db(-5.0f + static_cast<float>(band_id));
      }
      if (element.type_specific().dynamics().has_supported_controls() &&
          element.type_specific().dynamics().supported_controls() &
              fuchsia::hardware::audio::signalprocessing::DynamicsSupportedControls::LEVEL_TYPE) {
        new_band_state.set_level_type(fhasp::LevelType::PEAK);
      }
      if (element.type_specific().dynamics().has_supported_controls() &&
          element.type_specific().dynamics().supported_controls() &
              fuchsia::hardware::audio::signalprocessing::DynamicsSupportedControls::LOOKAHEAD) {
        new_band_state.set_lookahead(2'000'000 + band_id);
      }
      if (element.type_specific().dynamics().has_supported_controls() &&
          element.type_specific().dynamics().supported_controls() &
              fuchsia::hardware::audio::signalprocessing::DynamicsSupportedControls::
                  LINKED_CHANNELS) {
        new_band_state.set_linked_channels(true);
      }

      new_band_states.emplace_back(std::move(new_band_state));
    }
    fhasp::DynamicsElementState dyn_state;
    dyn_state.set_band_states(std::move(new_band_states));
    fhasp::DynamicsElementState dyn_copy = fidl::Clone(dyn_state);
    fhasp::SettableElementState new_state;
    new_state.set_type_specific(
        fhasp::SettableTypeSpecificElementState::WithDynamics(std::move(dyn_state)));

    signal_processing()->WatchElementState(
        element.id(),
        AddCallbackUnordered(
            "WatchElementState[dyn 1](element " + std::to_string(element.id()) + ")",
            [&element, &dyn_copy](const fhasp::ElementState& received_state) {
              ValidateElementState(element, received_state);
              ValidateDynamicsElementState(element, received_state);

              ASSERT_TRUE(received_state.has_type_specific());
              ASSERT_TRUE(received_state.type_specific().is_dynamics());
              ASSERT_TRUE(received_state.type_specific().dynamics().has_band_states());
              const auto& rs = received_state.type_specific().dynamics().band_states();
              ASSERT_TRUE(!rs.empty());
              for (auto idx = 0u; idx < rs.size(); ++idx) {
                uint32_t band_id = static_cast<uint32_t>(dyn_copy.band_states()[idx].id());
                ASSERT_TRUE(rs[idx].has_id());
                EXPECT_EQ(rs[idx].id(), band_id);

                ASSERT_TRUE(rs[idx].has_min_frequency());
                EXPECT_EQ(rs[idx].min_frequency(), 20 + band_id);

                ASSERT_TRUE(rs[idx].has_max_frequency());
                EXPECT_EQ(rs[idx].max_frequency(), 10'000 + band_id);

                ASSERT_TRUE(rs[idx].has_threshold_db());
                EXPECT_EQ(rs[idx].threshold_db(), static_cast<float>(band_id));

                if (element.type_specific().dynamics().supported_controls() &
                    fuchsia::hardware::audio::signalprocessing::DynamicsSupportedControls::
                        THRESHOLD_TYPE) {
                  ASSERT_TRUE(rs[idx].has_threshold_type());
                  EXPECT_EQ(rs[idx].threshold_type(), fhasp::ThresholdType::ABOVE);
                } else {
                  EXPECT_FALSE(rs[idx].has_threshold_type());
                }

                ASSERT_TRUE(rs[idx].has_ratio());
                EXPECT_EQ(rs[idx].ratio(), 2.0f + static_cast<float>(band_id));

                if (element.type_specific().dynamics().supported_controls() &
                    fuchsia::hardware::audio::signalprocessing::DynamicsSupportedControls::
                        KNEE_WIDTH) {
                  ASSERT_TRUE(rs[idx].has_knee_width_db());
                  EXPECT_EQ(rs[idx].knee_width_db(), 1.0f + static_cast<float>(band_id));
                } else {
                  EXPECT_FALSE(rs[idx].has_knee_width_db());
                }

                if (element.type_specific().dynamics().supported_controls() &
                    fuchsia::hardware::audio::signalprocessing::DynamicsSupportedControls::ATTACK) {
                  ASSERT_TRUE(rs[idx].has_attack());
                  EXPECT_EQ(rs[idx].attack(), 1'000'000 + band_id);
                } else {
                  EXPECT_FALSE(rs[idx].has_attack());
                }

                if (element.type_specific().dynamics().supported_controls() &
                    fuchsia::hardware::audio::signalprocessing::DynamicsSupportedControls::
                        RELEASE) {
                  ASSERT_TRUE(rs[idx].has_release());
                  EXPECT_EQ(rs[idx].release(), 5'000'000 + band_id);
                } else {
                  EXPECT_FALSE(rs[idx].has_release());
                }

                if (element.type_specific().dynamics().supported_controls() &
                    fuchsia::hardware::audio::signalprocessing::DynamicsSupportedControls::
                        OUTPUT_GAIN) {
                  ASSERT_TRUE(rs[idx].has_output_gain_db());
                  EXPECT_EQ(rs[idx].output_gain_db(), -3.0f + static_cast<float>(band_id));
                } else {
                  EXPECT_FALSE(rs[idx].has_output_gain_db());
                }

                if (element.type_specific().dynamics().supported_controls() &
                    fuchsia::hardware::audio::signalprocessing::DynamicsSupportedControls::
                        INPUT_GAIN) {
                  ASSERT_TRUE(rs[idx].has_input_gain_db());
                  EXPECT_EQ(rs[idx].input_gain_db(), -5.0f + static_cast<float>(band_id));
                } else {
                  EXPECT_FALSE(rs[idx].has_input_gain_db());
                }

                if (element.type_specific().dynamics().supported_controls() &
                    fuchsia::hardware::audio::signalprocessing::DynamicsSupportedControls::
                        LEVEL_TYPE) {
                  ASSERT_TRUE(rs[idx].has_level_type());
                  EXPECT_EQ(rs[idx].level_type(), fhasp::LevelType::PEAK);
                } else {
                  EXPECT_FALSE(rs[idx].has_level_type());
                }

                if (element.type_specific().dynamics().supported_controls() &
                    fuchsia::hardware::audio::signalprocessing::DynamicsSupportedControls::
                        LOOKAHEAD) {
                  ASSERT_TRUE(rs[idx].has_lookahead());
                  EXPECT_EQ(rs[idx].lookahead(), 2'000'000 + band_id);
                } else {
                  EXPECT_FALSE(rs[idx].has_lookahead());
                }

                if (element.type_specific().dynamics().supported_controls() &
                    fuchsia::hardware::audio::signalprocessing::DynamicsSupportedControls::
                        LINKED_CHANNELS) {
                  ASSERT_TRUE(rs[idx].has_linked_channels());
                  EXPECT_EQ(rs[idx].linked_channels(), true);
                } else {
                  EXPECT_FALSE(rs[idx].has_linked_channels());
                }
              }
            }));

    signal_processing()->SetElementState(
        element.id(), std::move(new_state),
        AddCallbackUnordered(
            "SetElementState[dyn](element " + std::to_string(element.id()) + ")",
            [id = element.id()](fhasp::SignalProcessing_SetElementState_Result result) {
              EXPECT_FALSE(result.is_err())
                  << "SetElementState[dyn 1](element " << id << ") failed";
            }));
    ASSERT_TRUE(signal_processing().is_bound()) << "SetElementState failed. Unbound";
    ExpectCallbacks();
  }

  // Now restore the initial element state.
  {
    fhasp::SettableElementState restored_state;
    restored_state.set_type_specific(fhasp::SettableTypeSpecificElementState::WithDynamics(
        fidl::Clone(initial_state.type_specific().dynamics())));

    signal_processing()->WatchElementState(
        element.id(),
        AddCallbackUnordered(
            "WatchElementState[dyn 2](element " + std::to_string(element.id()) + ")",
            [&element, &initial_state](const fhasp::ElementState& received_state) {
              ValidateElementState(element, received_state);
              ValidateDynamicsElementState(element, received_state);

              ASSERT_TRUE(received_state.has_type_specific());
              ASSERT_TRUE(received_state.type_specific().is_dynamics());
              ASSERT_TRUE(received_state.type_specific().dynamics().has_band_states());
              const auto& rs = received_state.type_specific().dynamics().band_states();
              ASSERT_TRUE(!rs.empty());
              const auto& is = initial_state.type_specific().dynamics().band_states();
              for (auto idx = 0u; idx < rs.size(); ++idx) {
                ASSERT_TRUE(rs[idx].has_id());
                ASSERT_EQ(rs[idx].has_min_frequency(), is[idx].has_min_frequency());
                ASSERT_EQ(rs[idx].has_max_frequency(), is[idx].has_max_frequency());
                ASSERT_EQ(rs[idx].has_threshold_db(), is[idx].has_threshold_db());
                ASSERT_EQ(rs[idx].has_threshold_type(), is[idx].has_threshold_type());
                ASSERT_EQ(rs[idx].has_ratio(), is[idx].has_ratio());
                ASSERT_EQ(rs[idx].has_knee_width_db(), is[idx].has_knee_width_db());
                ASSERT_EQ(rs[idx].has_attack(), is[idx].has_attack());
                ASSERT_EQ(rs[idx].has_release(), is[idx].has_release());
                ASSERT_EQ(rs[idx].has_output_gain_db(), is[idx].has_output_gain_db());
                ASSERT_EQ(rs[idx].has_input_gain_db(), is[idx].has_input_gain_db());
                ASSERT_EQ(rs[idx].has_level_type(), is[idx].has_level_type());
                ASSERT_EQ(rs[idx].has_lookahead(), is[idx].has_lookahead());
                ASSERT_EQ(rs[idx].has_linked_channels(), is[idx].has_linked_channels());

                EXPECT_EQ(rs[idx].id(), is[idx].id());
                if (is[idx].has_min_frequency()) {
                  EXPECT_EQ(rs[idx].min_frequency(), is[idx].min_frequency());
                }
                if (is[idx].has_max_frequency()) {
                  EXPECT_EQ(rs[idx].max_frequency(), is[idx].max_frequency());
                }
                if (is[idx].has_threshold_db()) {
                  EXPECT_EQ(rs[idx].threshold_db(), is[idx].threshold_db());
                }
                if (is[idx].has_threshold_type()) {
                  EXPECT_EQ(rs[idx].threshold_type(), is[idx].threshold_type());
                }
                if (is[idx].has_ratio()) {
                  EXPECT_EQ(rs[idx].ratio(), is[idx].ratio());
                }
                if (is[idx].has_knee_width_db()) {
                  EXPECT_EQ(rs[idx].knee_width_db(), is[idx].knee_width_db());
                }
                if (is[idx].has_attack()) {
                  EXPECT_EQ(rs[idx].attack(), is[idx].attack());
                }
                if (is[idx].has_release()) {
                  EXPECT_EQ(rs[idx].release(), is[idx].release());
                }
                if (is[idx].has_output_gain_db()) {
                  EXPECT_EQ(rs[idx].output_gain_db(), is[idx].output_gain_db());
                }
                if (is[idx].has_input_gain_db()) {
                  EXPECT_EQ(rs[idx].input_gain_db(), is[idx].input_gain_db());
                }
                if (is[idx].has_level_type()) {
                  EXPECT_EQ(rs[idx].level_type(), is[idx].level_type());
                }
                if (is[idx].has_lookahead()) {
                  EXPECT_EQ(rs[idx].lookahead(), is[idx].lookahead());
                }
                if (is[idx].has_linked_channels()) {
                  EXPECT_EQ(rs[idx].linked_channels(), is[idx].linked_channels());
                }
              }
            }));

    signal_processing()->SetElementState(
        element.id(), std::move(restored_state),
        AddCallbackUnordered(
            "SetElementState[dyn](element " + std::to_string(element.id()) + ")",
            [id = element.id()](fhasp::SignalProcessing_SetElementState_Result result) {
              EXPECT_FALSE(result.is_err())
                  << "SetElementState[dyn 2](element " << id << ") failed";
            }));
    ASSERT_TRUE(signal_processing().is_bound()) << "SetElementState failed (restoral). Unbound";
    ExpectCallbacks();
  }
}

// Validate the ability to change type-specific state for this specific EQUALIZER element.
void AdminTest::TestSetEqualizerElementState(const fhasp::Element& element,
                                             const fhasp::ElementState& initial_state) {
  ASSERT_TRUE(element.has_type_specific());
  ASSERT_TRUE(element.type_specific().is_equalizer());
  ASSERT_TRUE(element.type_specific().equalizer().has_bands() &&
              !element.type_specific().equalizer().bands().empty());

  // Change what we can, in SettableElementState::EqualizerElementState
  {
    std::vector<::fuchsia::hardware::audio::signalprocessing::EqualizerBandState> new_band_states;
    for (const auto& old_band_state : initial_state.type_specific().equalizer().band_states()) {
      uint32_t band_id = static_cast<uint32_t>(old_band_state.id());
      fhasp::EqualizerBandState new_band_state;
      new_band_state.set_id(band_id);
      if (element.type_specific().equalizer().has_supported_controls() &&
          element.type_specific().equalizer().supported_controls() &
              fuchsia::hardware::audio::signalprocessing::EqualizerSupportedControls::
                  SUPPORTS_TYPE_PEAK) {
        new_band_state.set_type(fhasp::EqualizerBandType::PEAK);
      } else if (element.type_specific().equalizer().has_supported_controls() &&
                 element.type_specific().equalizer().supported_controls() &
                     fuchsia::hardware::audio::signalprocessing::EqualizerSupportedControls::
                         SUPPORTS_TYPE_LOW_SHELF) {
        new_band_state.set_type(fhasp::EqualizerBandType::LOW_SHELF);
      } else if (element.type_specific().equalizer().has_supported_controls() &&
                 element.type_specific().equalizer().supported_controls() &
                     fuchsia::hardware::audio::signalprocessing::EqualizerSupportedControls::
                         SUPPORTS_TYPE_HIGH_SHELF) {
        new_band_state.set_type(fhasp::EqualizerBandType::HIGH_SHELF);
      }
      if (element.type_specific().equalizer().has_supported_controls() &&
          element.type_specific().equalizer().supported_controls() &
              fuchsia::hardware::audio::signalprocessing::EqualizerSupportedControls::
                  CAN_CONTROL_FREQUENCY) {
        // Set this value based on the band_id.
        new_band_state.set_frequency(400 + band_id);
      }
      if (element.type_specific().equalizer().has_supported_controls() &&
          element.type_specific().equalizer().supported_controls() &
              fuchsia::hardware::audio::signalprocessing::EqualizerSupportedControls::
                  CAN_CONTROL_Q) {
        // Set this value based on the band_id.
        new_band_state.set_q(static_cast<float>(band_id));
      }
      // Set this value based on the band_id.
      new_band_state.set_gain_db(10.0f + static_cast<float>(band_id));
      new_band_state.set_enabled(true);
      new_band_states.emplace_back(std::move(new_band_state));
    }
    fhasp::EqualizerElementState eq_state;
    eq_state.set_band_states(std::move(new_band_states));
    fhasp::EqualizerElementState eq_copy = fidl::Clone(eq_state);
    fhasp::SettableElementState new_state;
    new_state.set_type_specific(
        fhasp::SettableTypeSpecificElementState::WithEqualizer(std::move(eq_state)));

    signal_processing()->WatchElementState(
        element.id(),
        AddCallbackUnordered(
            "WatchElementState[EQ 1](element " + std::to_string(element.id()) + ")",
            [&element, &eq_copy](const fhasp::ElementState& received_state) {
              ValidateElementState(element, received_state);
              ValidateEqualizerElementState(element, received_state);

              ASSERT_TRUE(received_state.has_type_specific());
              ASSERT_TRUE(received_state.type_specific().is_equalizer());
              ASSERT_TRUE(received_state.type_specific().equalizer().has_band_states());
              const auto& rs = received_state.type_specific().equalizer().band_states();
              ASSERT_TRUE(!rs.empty());
              for (auto idx = 0u; idx < rs.size(); ++idx) {
                ASSERT_TRUE(rs[idx].has_id());
                EXPECT_EQ(rs[idx].id(), eq_copy.band_states()[idx].id());
                const auto& band_id = rs[idx].id();

                if (element.type_specific().equalizer().has_supported_controls() &&
                    element.type_specific().equalizer().supported_controls() &
                        fuchsia::hardware::audio::signalprocessing::EqualizerSupportedControls::
                            SUPPORTS_TYPE_PEAK) {
                  ASSERT_TRUE(rs[idx].has_type());
                  EXPECT_EQ(rs[idx].type(), fhasp::EqualizerBandType::PEAK);
                } else if (element.type_specific().equalizer().has_supported_controls() &&
                           element.type_specific().equalizer().supported_controls() &
                               fuchsia::hardware::audio::signalprocessing::
                                   EqualizerSupportedControls::SUPPORTS_TYPE_LOW_SHELF) {
                  ASSERT_TRUE(rs[idx].has_type());
                  EXPECT_EQ(rs[idx].type(), fhasp::EqualizerBandType::LOW_SHELF);
                } else if (element.type_specific().equalizer().has_supported_controls() &&
                           element.type_specific().equalizer().supported_controls() &
                               fuchsia::hardware::audio::signalprocessing::
                                   EqualizerSupportedControls::SUPPORTS_TYPE_HIGH_SHELF) {
                  ASSERT_TRUE(rs[idx].has_type());
                  EXPECT_EQ(rs[idx].type(), fhasp::EqualizerBandType::HIGH_SHELF);
                }
                if (element.type_specific().equalizer().has_supported_controls() &&
                    element.type_specific().equalizer().supported_controls() &
                        fuchsia::hardware::audio::signalprocessing::EqualizerSupportedControls::
                            CAN_CONTROL_FREQUENCY) {
                  ASSERT_TRUE(rs[idx].has_frequency());
                  // This value was set based on the band_id.
                  EXPECT_EQ(rs[idx].frequency(), 400 + static_cast<uint32_t>(band_id));
                }
                if (element.type_specific().equalizer().has_supported_controls() &&
                    element.type_specific().equalizer().supported_controls() &
                        fuchsia::hardware::audio::signalprocessing::EqualizerSupportedControls::
                            CAN_CONTROL_Q) {
                  ASSERT_TRUE(rs[idx].has_q());
                  // This value was set based on the band_id.
                  EXPECT_EQ(rs[idx].q(), static_cast<float>(band_id));
                }
                ASSERT_TRUE(rs[idx].has_gain_db());
                // This value was set based on the band_id.
                EXPECT_EQ(rs[idx].gain_db(), 10.0f + static_cast<float>(band_id));
                ASSERT_TRUE(rs[idx].has_enabled());
                EXPECT_TRUE(rs[idx].enabled());
              }
            }));

    signal_processing()->SetElementState(
        element.id(), std::move(new_state),
        AddCallbackUnordered(
            "SetElementState[EQ 1](element " + std::to_string(element.id()) + ")",
            [id = element.id()](fhasp::SignalProcessing_SetElementState_Result result) {
              EXPECT_FALSE(result.is_err()) << "SetElementState[EQ](element " << id << ") failed";
            }));
    ASSERT_TRUE(signal_processing().is_bound()) << "SetElementState failed. Unbound";
    ExpectCallbacks();
  }

  // Now restore the initial element state.
  {
    fhasp::SettableElementState restored_state;
    restored_state.set_type_specific(fhasp::SettableTypeSpecificElementState::WithEqualizer(
        fidl::Clone(initial_state.type_specific().equalizer())));

    signal_processing()->WatchElementState(
        element.id(),
        AddCallbackUnordered(
            "WatchElementState[EQ 2](element " + std::to_string(element.id()) + ")",
            [&element, &initial_state](const fhasp::ElementState& received_state) {
              ValidateElementState(element, received_state);
              ValidateEqualizerElementState(element, received_state);

              ASSERT_TRUE(received_state.has_type_specific());
              ASSERT_TRUE(received_state.type_specific().is_equalizer());
              ASSERT_TRUE(received_state.type_specific().equalizer().has_band_states());
              const auto& rs = received_state.type_specific().equalizer().band_states();
              ASSERT_TRUE(!rs.empty());
              const auto& is = initial_state.type_specific().equalizer().band_states();
              for (auto idx = 0u; idx < rs.size(); ++idx) {
                ASSERT_TRUE(rs[idx].has_id());
                ASSERT_EQ(rs[idx].has_type(), is[idx].has_type());
                ASSERT_EQ(rs[idx].has_frequency(), is[idx].has_frequency());
                ASSERT_EQ(rs[idx].has_q(), is[idx].has_q());
                ASSERT_EQ(rs[idx].has_gain_db(), is[idx].has_gain_db());
                ASSERT_EQ(rs[idx].has_enabled(), is[idx].has_enabled());

                EXPECT_EQ(rs[idx].id(), is[idx].id());
                if (is[idx].has_type()) {
                  EXPECT_EQ(rs[idx].type(), is[idx].type());
                }
                if (is[idx].has_frequency()) {
                  EXPECT_EQ(rs[idx].frequency(), is[idx].frequency());
                }
                if (is[idx].has_q()) {
                  EXPECT_EQ(rs[idx].q(), is[idx].q());
                }
                if (is[idx].has_gain_db()) {
                  EXPECT_EQ(rs[idx].gain_db(), is[idx].gain_db());
                }
                if (is[idx].has_enabled()) {
                  EXPECT_EQ(rs[idx].enabled(), is[idx].enabled());
                }
              }
            }));

    signal_processing()->SetElementState(
        element.id(), std::move(restored_state),
        AddCallbackUnordered(
            "SetElementState[EQ 2](element " + std::to_string(element.id()) + ")",
            [id = element.id()](fhasp::SignalProcessing_SetElementState_Result result) {
              ASSERT_FALSE(result.is_err()) << "SetElementState[EQ](element " << id << ") failed";
            }));
    ASSERT_TRUE(signal_processing().is_bound()) << "SetElementState failed (restoral). Unbound";
    ExpectCallbacks();
  }
}

// Validate the ability to change type-specific state for this specific GAIN element.
void AdminTest::TestSetGainElementState(const fhasp::Element& element,
                                        const fhasp::ElementState& initial_state) {
  const auto& gain_caps = element.type_specific().gain();

  if (gain_caps.min_gain() == gain_caps.max_gain()) {
    GTEST_SKIP() << "Cannot test GainElementState for element " << element.id()
                 << ": min_gain == max_gain";
  }

  // Change what we can, in SettableElementState::GainElementState
  {
    fhasp::GainElementState gain_state;
    if (initial_state.type_specific().gain().gain() == gain_caps.max_gain()) {
      gain_state.set_gain(gain_caps.min_gain());
    } else {
      gain_state.set_gain(gain_caps.max_gain());
    }
    float expected_gain = gain_state.gain();
    fhasp::SettableElementState state;
    state.set_type_specific(
        fhasp::SettableTypeSpecificElementState::WithGain(std::move(gain_state)));

    signal_processing()->WatchElementState(
        element.id(), AddCallbackUnordered(
                          "WatchElementState[gain 1](element " + std::to_string(element.id()) + ")",
                          [&element, expected_gain](const fhasp::ElementState& result) {
                            ValidateElementState(element, result);
                            ValidateGainElementState(element, result);

                            EXPECT_EQ(expected_gain, result.type_specific().gain().gain());
                          }));

    signal_processing()->SetElementState(
        element.id(), std::move(state),
        AddCallbackUnordered(
            "SetElementState[gain 1](element " + std::to_string(element.id()) + ")",
            [id = element.id()](fhasp::SignalProcessing_SetElementState_Result result) {
              ASSERT_FALSE(result.is_err()) << "SetElementState[gain](element " << id << ") failed";
            }));
    ASSERT_TRUE(signal_processing().is_bound())
        << "SignalProcessing failed to set the ElementState";
    ExpectCallbacks();
  }

  // Now restore the initial element state.
  {
    fhasp::GainElementState gain_state;
    gain_state.set_gain(initial_state.type_specific().gain().gain());
    fhasp::SettableElementState state;
    state.set_type_specific(
        fhasp::SettableTypeSpecificElementState::WithGain(std::move(gain_state)));

    signal_processing()->WatchElementState(
        element.id(),
        AddCallback("WatchElementState[gain 2](element " + std::to_string(element.id()) + ")",
                    [&element, expected_gain = initial_state.type_specific().gain().gain()](
                        const fhasp::ElementState& result) {
                      ValidateElementState(element, result);
                      ValidateGainElementState(element, result);

                      EXPECT_EQ(expected_gain, result.type_specific().gain().gain());
                    }));

    signal_processing()->SetElementState(
        element.id(), std::move(state),
        AddCallbackUnordered(
            "SetElementState[gain 2](element " + std::to_string(element.id()) + ")",
            [id = element.id()](fhasp::SignalProcessing_SetElementState_Result result) {
              EXPECT_FALSE(result.is_err()) << "SetElementState[gain](element " << id << ") failed";
            }));
    ASSERT_TRUE(signal_processing().is_bound())
        << "SignalProcessing failed to restore the ElementState";
    ExpectCallbacks();
  }
}

// Validate SetElementState for this specific GAIN element, when there is no change.
// This also covers the edge case where min_gain == max_gain: we still validate SetElementState.
void AdminTest::TestSetGainElementStateNoChange(fhasp::ElementId element_id, float current_gain) {
  // Define a value in SettableElementState::GainElementState, but the same as current state.
  fhasp::GainElementState gain_state;
  gain_state.set_gain(current_gain);
  fhasp::SettableElementState state;
  state.set_type_specific(fhasp::SettableTypeSpecificElementState::WithGain(std::move(gain_state)));

  FailOnWatchElementStateCompletion(element_id);

  signal_processing()->SetElementState(
      element_id, std::move(state),
      AddCallback("SetElementState[gain no-change](element " + std::to_string(element_id) + ")",
                  [element_id](fhasp::SignalProcessing_SetElementState_Result result) {
                    EXPECT_FALSE(result.is_err())
                        << "SetElementState[gain no-change](element " << element_id << ") failed";
                  }));
  ASSERT_TRUE(signal_processing().is_bound()) << "SignalProcessing failed to set the ElementState";
  ExpectCallbacks();
}

// Validate this specific GAIN element, when SetElementState contains invalid type-specific data.
void AdminTest::TestSetGainElementStateInvalidGain(fhasp::ElementId element_id) {
  fhasp::GainElementState gain_state;
  gain_state.set_gain(NAN);
  fhasp::SettableElementState state;
  state.set_type_specific(fhasp::SettableTypeSpecificElementState::WithGain(std::move(gain_state)));

  FailOnWatchElementStateCompletion(element_id);

  signal_processing()->SetElementState(
      element_id, std::move(state),
      AddCallback("SetElementState[gain NAN](element " + std::to_string(element_id) + ")",
                  [element_id](fhasp::SignalProcessing_SetElementState_Result result) {
                    ASSERT_TRUE(result.is_err())
                        << "SetElementState[gain NAN](element " << element_id << ") did not fail";
                    EXPECT_EQ(result.err(), ZX_ERR_INVALID_ARGS);
                  }));
  ASSERT_TRUE(signal_processing().is_bound())
      << "SignalProcessing unbound upon the errant SetElementState";
  ExpectCallbacks();
}

#define DEFINE_ADMIN_TEST_CLASS(CLASS_NAME, CODE)                               \
  class CLASS_NAME : public AdminTest {                                         \
   public:                                                                      \
    explicit CLASS_NAME(const DeviceEntry& dev_entry) : AdminTest(dev_entry) {} \
    void TestBody() override { CODE }                                           \
  }

//
// Test cases that target each of the various admin commands
//
// Any case not ending in disconnect/error should WaitForError, in case the channel disconnects.

// Verify the driver responds to the GetHealthState query.
DEFINE_ADMIN_TEST_CLASS(CompositeHealth, { RequestHealthAndExpectHealthy(); });

// Verify the driver does not close the channel when an unknown method is called.
DEFINE_ADMIN_TEST_CLASS(CompositeUnknownMethod, {
  // Unbind to get raw channel
  auto channel = composite().Unbind().TakeChannel();

  // Send unknown method
  fidl_message_header_t header = {};
  header.txid = 0;  // One way call
  header.at_rest_flags[0] = FIDL_MESSAGE_HEADER_AT_REST_FLAGS_0_USE_VERSION_V2;
  header.magic_number = kFidlWireFormatMagicNumberInitial;
  // Select an ordinal with the high bit set (0x8...) to indicate a flexible method.
  // This ensures the runtime treats it as an "Unknown Flexible" method rather than
  // a "Strict Protocol Violation", allowing handle_unknown_method to be called.
  header.ordinal = 0x9234567812345678;
  header.dynamic_flags |= FIDL_MESSAGE_HEADER_DYNAMIC_FLAGS_FLEXIBLE_METHOD;

  ASSERT_EQ(channel.write(0, &header, sizeof(header), nullptr, 0), ZX_OK);

  // Rebind and re-add error handler
  composite().Bind(std::move(channel));
  AddErrorHandler(composite(), "Composite");

  // Verify connection is still alive using a valid method
  RequestHealthAndExpectHealthy();
});

// Verify a valid unique_id, manufacturer, product are successfully received.
DEFINE_ADMIN_TEST_CLASS(CompositeProperties, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());

  ValidateProperties();
  WaitForError();
});

// Display the DAI Format Sets, for debugging purposes only.
DEFINE_ADMIN_TEST_CLASS(DisplayDaiFormatSets, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveDaiFormats());

  LogDaiFormatSets(dai_formats());
  WaitForError();
});

// Verify that a valid element list is successfully received.
// Validate the base Element properties, for all elements.
DEFINE_ADMIN_TEST_CLASS(GetElements, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestElements());
  if (kDisplayElementsAndTopologies) {
    DisplayElements(elements());
  }

  ASSERT_NO_FAILURE_OR_SKIP(ValidateElements());
  WaitForError();
});

// There are five element types that have additional type-specific element properties.
//
// Validate Element.type_specific, for DAI elements.
DEFINE_ADMIN_TEST_CLASS(DaiElements, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestElements());

  ASSERT_NO_FAILURE_OR_SKIP(ValidateDaiElements());
  WaitForError();
});

// Validate Element.type_specific, for Dynamics elements.
DEFINE_ADMIN_TEST_CLASS(DynamicsElements, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestElements());

  ASSERT_NO_FAILURE_OR_SKIP(ValidateDynamicsElements());
  WaitForError();
});

// Validate Element.type_specific, for Equalizer elements.
DEFINE_ADMIN_TEST_CLASS(EqualizerElements, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestElements());

  ASSERT_NO_FAILURE_OR_SKIP(ValidateEqualizerElements());
  WaitForError();
});

// Validate Element.type_specific, for Gain elements.
DEFINE_ADMIN_TEST_CLASS(GainElements, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestElements());

  ASSERT_NO_FAILURE_OR_SKIP(ValidateGainElements());
  WaitForError();
});

// Validate Element.type_specific, for Vendor-specific elements.
DEFINE_ADMIN_TEST_CLASS(VendorSpecificElements, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestElements());

  ASSERT_NO_FAILURE_OR_SKIP(ValidateVendorSpecificElements());
  WaitForError();
});

// Verify that a valid topology list is successfully received.
DEFINE_ADMIN_TEST_CLASS(GetTopologies, {
  RequestTopologies();
  if (kDisplayElementsAndTopologies) {
    DisplayTopologies(topologies());
  }
  WaitForError();
});

// Verify that a valid topology is successfully received.
DEFINE_ADMIN_TEST_CLASS(InitialTopology, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestTopologies());

  RetrieveInitialTopology();
  WaitForError();
});

// Verify that a valid topology is successfully received.
DEFINE_ADMIN_TEST_CLASS(SetTopology, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestTopologies());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialTopology());

  SetAllTopologies();
  WaitForError();
});

// All elements should be in at least one topology, all topology elements should be known.
DEFINE_ADMIN_TEST_CLASS(ElementTopologyClosure, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestElements());
  ASSERT_NO_FAILURE_OR_SKIP(RequestTopologies());

  ValidateElementTopologyClosure();
  WaitForError();
});

// Verify that SetTopology with the current topology does not cause WatchTopology to trigger.
DEFINE_ADMIN_TEST_CLASS(SetTopologyNoChange, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestElements());
  ASSERT_NO_FAILURE_OR_SKIP(RequestTopologies());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialTopology());

  SetTopologyNoChangeAndExpectNoWatch();
  WaitForError();
});

// Verify that an unknown topology causes an error but does not disconnect.
DEFINE_ADMIN_TEST_CLASS(SetTopologyUnknownIdShouldError, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestTopologies());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialTopology());

  SetTopologyUnknownIdAndExpectError();  // ... but we should remain connected.
  WaitForError();
});

// Verify that calling WatchTopology while a previous call is still pending causes a disconnect.
DEFINE_ADMIN_TEST_CLASS(WatchTopologyWhilePendingShouldDisconnect, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestTopologies());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialTopology());

  ASSERT_NO_FAILURE_OR_SKIP(FailOnWatchTopologyCompletion());
  ASSERT_NO_FAILURE_OR_SKIP(WaitForError());

  WatchTopologyAndExpectDisconnect(ZX_ERR_BAD_STATE);
});

// Validate the base ElementState for all elements.
DEFINE_ADMIN_TEST_CLASS(InitialElementStates, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestElements());
  ASSERT_NO_FAILURE_OR_SKIP(RequestTopologies());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialTopology());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialElementStates());

  ValidateElementStates();
  WaitForError();
});

// There are five element types that have additional type-specific element state.
//
// Validate ElementState.type_specific, for DAI elements.
DEFINE_ADMIN_TEST_CLASS(DaiElementStates, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestElements());
  ASSERT_NO_FAILURE_OR_SKIP(RequestTopologies());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialTopology());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialElementStates());

  ValidateDaiElementStates();
  WaitForError();
});

// Validate ElementState.type_specific, for Dynamics elements.
DEFINE_ADMIN_TEST_CLASS(DynamicsElementStates, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestElements());
  ASSERT_NO_FAILURE_OR_SKIP(RequestTopologies());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialTopology());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialElementStates());

  ValidateDynamicsElementStates();
  WaitForError();
});

// Validate ElementState.type_specific, for Equalizer elements.
DEFINE_ADMIN_TEST_CLASS(EqualizerElementStates, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestElements());
  ASSERT_NO_FAILURE_OR_SKIP(RequestTopologies());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialTopology());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialElementStates());

  ValidateEqualizerElementStates();
  WaitForError();
});

// Validate ElementState.type_specific, for Gain elements.
DEFINE_ADMIN_TEST_CLASS(GainElementStates, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestElements());
  ASSERT_NO_FAILURE_OR_SKIP(RequestTopologies());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialTopology());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialElementStates());

  ValidateGainElementStates();
  WaitForError();
});

// Validate ElementState.type_specific, for Vendor-specific elements.
DEFINE_ADMIN_TEST_CLASS(VendorSpecificElementStates, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestElements());
  ASSERT_NO_FAILURE_OR_SKIP(RequestTopologies());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialElementStates());

  ValidateVendorSpecificElementStates();
  WaitForError();
});

// Verify that calling WatchTopology while a previous call is still pending causes a disconnect.
DEFINE_ADMIN_TEST_CLASS(WatchElementStateWhilePendingShouldDisconnect, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestElements());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialElementStates());

  fhasp::ElementId id = elements().front().id();
  ASSERT_NO_FAILURE_OR_SKIP(FailOnWatchElementStateCompletion(id));
  ASSERT_NO_FAILURE_OR_SKIP(WaitForError());

  WatchElementStateAndExpectDisconnect(id, ZX_ERR_BAD_STATE);
});

// Verify that an unknown element_id causes a disconnect.
DEFINE_ADMIN_TEST_CLASS(WatchElementStateUnknownIdShouldDisconnect, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestElements());

  WatchElementStateUnknownIdAndExpectDisconnect(ZX_ERR_INVALID_ARGS);
});

// For all elements, change non-type-specific portions of SettableElementState that can be set.
DEFINE_ADMIN_TEST_CLASS(SetElementState, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestElements());
  ASSERT_NO_FAILURE_OR_SKIP(RequestTopologies());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialTopology());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialElementStates());

  SetAllElementStates();
  WaitForError();
});

// There are four element types that have additional settable type-specific element state.
//
// Validate SettableTypeSpecificElementState, for Dynamics elements.
DEFINE_ADMIN_TEST_CLASS(SetDynamicsElementState, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestElements());
  ASSERT_NO_FAILURE_OR_SKIP(RequestTopologies());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialTopology());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialElementStates());

  SetAllDynamicsElementStates();
  WaitForError();
});

// Validate SettableTypeSpecificElementState, for Equalizer elements.
DEFINE_ADMIN_TEST_CLASS(SetEqualizerElementState, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestElements());
  ASSERT_NO_FAILURE_OR_SKIP(RequestTopologies());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialTopology());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialElementStates());

  SetAllEqualizerElementStates();
  WaitForError();
});

// Validate SettableTypeSpecificElementState, for Gain elements.
DEFINE_ADMIN_TEST_CLASS(SetGainElementState, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestElements());
  ASSERT_NO_FAILURE_OR_SKIP(RequestTopologies());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialTopology());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialElementStates());

  SetAllGainElementStates();
  WaitForError();
});

// Validate SettableTypeSpecificElementState, for Gain elements.
DEFINE_ADMIN_TEST_CLASS(SetGainElementStateNoChange, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestElements());
  ASSERT_NO_FAILURE_OR_SKIP(RequestTopologies());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialTopology());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialElementStates());

  SetAllGainElementStatesNoChange();
  WaitForError();
});

// Validate SettableTypeSpecificElementState, for Gain elements.
DEFINE_ADMIN_TEST_CLASS(SetGainElementStateInvalidGain, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestElements());
  ASSERT_NO_FAILURE_OR_SKIP(RequestTopologies());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialTopology());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialElementStates());

  SetAllGainElementStatesInvalidGainShouldError();  // ... but we should remain connected.
  WaitForError();
});

DEFINE_ADMIN_TEST_CLASS(SetElementStateNoChange, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestElements());
  ASSERT_NO_FAILURE_OR_SKIP(RequestTopologies());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialTopology());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialElementStates());

  SetAllElementStatesNoChange();
  WaitForError();
});

// Verify that an unknown topology causes an error but does not disconnect.
DEFINE_ADMIN_TEST_CLASS(SetElementStateUnknownIdShouldError, {
  ASSERT_NO_FAILURE_OR_SKIP(RequestElements());
  ASSERT_NO_FAILURE_OR_SKIP(RequestTopologies());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialTopology());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveInitialElementStates());

  SetElementStateUnknownIdAndExpectError();  // ... but we should remain connected.
  WaitForError();
});

// Verify that format-retrieval responses are successfully received and are complete and valid.
DEFINE_ADMIN_TEST_CLASS(CompositeRingBufferFormats, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());

  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  WaitForError();
});

// Verify that format-retrieval responses are successfully received and are complete and valid.
DEFINE_ADMIN_TEST_CLASS(CompositeDaiFormats, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());

  ASSERT_NO_FAILURE_OR_SKIP(RetrieveDaiFormats());
  WaitForError();
});

// Verify that a Reset() returns a valid completion.
DEFINE_ADMIN_TEST_CLASS(Reset, {
  ResetAndExpectResponse();
  WaitForError();
});

// Start-while-started should always succeed, so we test this twice.
DEFINE_ADMIN_TEST_CLASS(CodecStart, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RequestCodecStartAndExpectResponse());

  RequestCodecStartAndExpectResponse();
  WaitForError();
});

// Stop-while-stopped should always succeed, so we call Stop twice.
DEFINE_ADMIN_TEST_CLASS(CodecStop, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RequestCodecStopAndExpectResponse());

  RequestCodecStopAndExpectResponse();
  WaitForError();
});

// Verify valid responses: ring buffer properties
DEFINE_ADMIN_TEST_CLASS(GetRingBufferProperties, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMaxFormat());

  RequestRingBufferProperties();
  WaitForError();
});

// Verify valid responses: get ring buffer VMO.
DEFINE_ADMIN_TEST_CLASS(GetRingBufferVmo, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMinFormat());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferProperties());

  RequestBuffer(100);
  WaitForError();
});

// Clients request minimum VMO sizes for their requirements, and drivers must respond with VMOs that
// satisfy those requests as well as their own constraints for proper operation. A driver or device
// reads/writes a ring buffer in batches, so it must reserve part of the ring buffer for safe
// copying. This test case validates that drivers set aside a non-zero amount of their ring buffers.
//
// Many drivers automatically "round up" their VMO to a memory page boundary, regardless of space
// needed for proper DMA. To factor this out, here the client requests enough frames to exactly fill
// an integral number of memory pages. The driver should nonetheless return a larger buffer.
DEFINE_ADMIN_TEST_CLASS(DriverReservesRingBufferSpace, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMaxFormat());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferProperties());

  uint32_t page_frame_aligned_rb_frames =
      std::lcm<uint32_t>(frame_size(), zx_system_get_page_size()) / frame_size();
  FX_LOGS(DEBUG) << "frame_size is " << frame_size() << ", requesting a ring buffer of "
                 << page_frame_aligned_rb_frames << " frames";
  RequestBuffer(page_frame_aligned_rb_frames);
  WaitForError();

  // Calculate the driver's needed ring-buffer space, from retrieved fifo_size|safe_offset values.
  EXPECT_GT(ring_buffer_frames(), page_frame_aligned_rb_frames);
});

// Verify valid responses: set active channels returns a set_time after the call is made.
DEFINE_ADMIN_TEST_CLASS(SetActiveChannelsChange, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMaxFormat());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferProperties());

  uint64_t all_channels_mask = (1 << ring_buffer_pcm_format().number_of_channels) - 1;
  ASSERT_NO_FAILURE_OR_SKIP(
      ActivateChannelsAndExpectOutcome(all_channels_mask, SetActiveChannelsOutcome::SUCCESS));

  ASSERT_NO_FAILURE_OR_SKIP(RequestBuffer(8000));
  ASSERT_NO_FAILURE_OR_SKIP(ActivateChannelsAndExpectOutcome(0, SetActiveChannelsOutcome::CHANGE));

  WaitForError();
});

// If no change, the previous set-time should be returned.
DEFINE_ADMIN_TEST_CLASS(SetActiveChannelsNoChange, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMaxFormat());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RequestBuffer(100));

  uint64_t all_channels_mask = (1 << ring_buffer_pcm_format().number_of_channels) - 1;
  ASSERT_NO_FAILURE_OR_SKIP(
      ActivateChannelsAndExpectOutcome(all_channels_mask, SetActiveChannelsOutcome::SUCCESS));

  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferStartAndExpectCallback());
  ASSERT_NO_FAILURE_OR_SKIP(
      ActivateChannelsAndExpectOutcome(all_channels_mask, SetActiveChannelsOutcome::NO_CHANGE));

  RequestRingBufferStopAndExpectCallback();
  WaitForError();
});

// Verify an invalid input (out of range) for SetActiveChannels.
DEFINE_ADMIN_TEST_CLASS(SetActiveChannelsTooHigh, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMaxFormat());

  auto channel_mask_too_high = (1 << ring_buffer_pcm_format().number_of_channels);
  ActivateChannelsAndExpectOutcome(channel_mask_too_high, SetActiveChannelsOutcome::FAILURE);
});

// Verify that valid start responses are received.
DEFINE_ADMIN_TEST_CLASS(RingBufferStart, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMinFormat());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RequestBuffer(32000));

  RequestRingBufferStartAndExpectCallback();
  WaitForError();
});

// ring-buffer FIDL channel should disconnect, with ZX_ERR_BAD_STATE
DEFINE_ADMIN_TEST_CLASS(RingBufferStartBeforeGetVmoShouldDisconnect, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMinFormat());

  RequestRingBufferStartAndExpectDisconnect(ZX_ERR_BAD_STATE);
});

// ring-buffer FIDL channel should disconnect, with ZX_ERR_BAD_STATE
DEFINE_ADMIN_TEST_CLASS(RingBufferStartWhileStartingShouldDisconnect, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMaxFormat());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RequestBuffer(8000));

  // Don't wait for the reported start time, or even the callback itself...
  auto _ = RequestRingBufferStart();
  // ...just immediately call again.
  RequestRingBufferStartAndExpectDisconnect(ZX_ERR_BAD_STATE);
});

// ring-buffer FIDL channel should disconnect, with ZX_ERR_BAD_STATE
DEFINE_ADMIN_TEST_CLASS(RingBufferStartWhileStartedShouldDisconnect, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMaxFormat());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RequestBuffer(8000));
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferStartAndExpectCallback());
  ASSERT_NO_FAILURE_OR_SKIP(WaitUntilAfterStartTime());

  RequestRingBufferStartAndExpectDisconnect(ZX_ERR_BAD_STATE);
});

// Verify that valid stop responses are received.
DEFINE_ADMIN_TEST_CLASS(RingBufferStop, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMaxFormat());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RequestBuffer(100));
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferStartAndExpectCallback());

  RequestRingBufferStopAndExpectCallback();
  WaitForError();
});

// ring-buffer FIDL channel should disconnect, with ZX_ERR_BAD_STATE
DEFINE_ADMIN_TEST_CLASS(RingBufferStopBeforeGetVmoShouldDisconnect, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMinFormat());

  RequestRingBufferStopAndExpectDisconnect(ZX_ERR_BAD_STATE);
});

DEFINE_ADMIN_TEST_CLASS(RingBufferStopWhileStoppedIsPermitted, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMinFormat());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RequestBuffer(100));
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferStopAndExpectCallback());

  RequestRingBufferStopAndExpectCallback();
  WaitForError();
});

// Verify valid WatchDelayInfo internal_delay responses.
DEFINE_ADMIN_TEST_CLASS(InternalDelayIsValid, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMaxFormat());

  WatchDelayAndExpectUpdate();
  ValidateInternalDelay();
  WaitForError();
});

// Verify valid WatchDelayInfo external_delay response.
DEFINE_ADMIN_TEST_CLASS(ExternalDelayIsValid, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMaxFormat());

  WatchDelayAndExpectUpdate();
  ValidateExternalDelay();
  WaitForError();
});

// Verify valid responses: WatchDelayInfo does NOT respond a second time.
DEFINE_ADMIN_TEST_CLASS(GetDelayInfoSecondTimeNoResponse, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMaxFormat());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferProperties());

  WatchDelayAndExpectUpdate();
  WatchDelayAndExpectNoUpdate();

  ASSERT_NO_FAILURE_OR_SKIP(RequestBuffer(8000));
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferStartAndExpectCallback());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferStopAndExpectCallback());

  WaitForError();
});

// Verify that valid WatchDelayInfo responses are received, even after RingBufferStart().
DEFINE_ADMIN_TEST_CLASS(GetDelayInfoAfterStart, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMaxFormat());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RequestBuffer(100));
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferStartAndExpectCallback());

  WatchDelayAndExpectUpdate();
  WaitForError();
});

DEFINE_ADMIN_TEST_CLASS(PositionNotifyBeforeStart, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMinFormat());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RequestBuffer(8000));

  ASSERT_NO_FAILURE_OR_SKIP(ExpectNoPositionNotifications());

  RequestPositionNotification();
  WaitForError();
});

DEFINE_ADMIN_TEST_CLASS(PositionNotifyNone, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMaxFormat());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RequestBuffer(8000, 0));

  ASSERT_NO_FAILURE_OR_SKIP(ExpectNoPositionNotifications());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferStartAndExpectCallback());

  RequestPositionNotification();
  WaitForError();
});

DEFINE_ADMIN_TEST_CLASS(PositionNotifyAfterStop, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMaxFormat());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RequestBuffer(8000));

  ASSERT_NO_FAILURE_OR_SKIP(ExpectNoPositionNotifications());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferStartAndExpectCallback());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferStopAndExpectCallback());

  RequestPositionNotification();
  WaitForError();
});

// Create a RingBuffer, drop it, recreate it, then interact with it in any way (e.g. GetProperties).
DEFINE_ADMIN_TEST_CLASS(GetRingBufferPropertiesAfterDroppingFirstRingBuffer, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMaxFormat());
  ASSERT_NO_FAILURE_OR_SKIP(DropRingBuffer());

  // Dropped first ring buffer, creating second one.
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMaxFormat());

  RequestRingBufferProperties();
  WaitForError();
});

// Create RingBuffer, fully exercise it, drop it, recreate it, then validate GetDelayInfo.
DEFINE_ADMIN_TEST_CLASS(GetDelayInfoAfterDroppingFirstRingBuffer, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMaxFormat());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferProperties());
  ASSERT_NO_FAILURE_OR_SKIP(WatchDelayAndExpectUpdate());
  ASSERT_NO_FAILURE_OR_SKIP(RequestBuffer(100));
  ASSERT_NO_FAILURE_OR_SKIP(WatchDelayAndExpectNoUpdate());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferStartAndExpectCallback());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferStopAndExpectCallback());
  ASSERT_NO_FAILURE_OR_SKIP(DropRingBuffer());

  // Dropped first ring buffer, creating second one, reverifying WatchDelayInfo.
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMaxFormat());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RequestBuffer(100));
  ASSERT_NO_FAILURE_OR_SKIP(WatchDelayAndExpectUpdate());

  WatchDelayAndExpectNoUpdate();
  WaitForError();
});

// Create RingBuffer, fully exercise it, drop it, recreate it, then validate SetActiveChannels.
DEFINE_ADMIN_TEST_CLASS(SetActiveChannelsAfterDroppingFirstRingBuffer, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RetrieveRingBufferFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMaxFormat());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RequestBuffer(100));

  uint64_t all_channels_mask = (1 << ring_buffer_pcm_format().number_of_channels) - 1;
  ASSERT_NO_FAILURE_OR_SKIP(
      ActivateChannelsAndExpectOutcome(all_channels_mask, SetActiveChannelsOutcome::SUCCESS));
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferStartAndExpectCallback());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferStopAndExpectCallback());
  ASSERT_NO_FAILURE_OR_SKIP(DropRingBuffer());

  // Dropped first ring buffer, creating second one, reverifying SetActiveChannels.
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferChannelWithMaxFormat());
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferProperties());
  ASSERT_NO_FAILURE_OR_SKIP(RequestBuffer(100));
  ASSERT_NO_FAILURE_OR_SKIP(RequestRingBufferStartAndExpectCallback());
  ASSERT_NO_FAILURE_OR_SKIP(ActivateChannelsAndExpectOutcome(0, SetActiveChannelsOutcome::SUCCESS));

  RequestRingBufferStopAndExpectCallback();
  WaitForError();
});

// Register separate test case instances for each enumerated device.
//
// See googletest/docs/advanced.md for details.
#define REGISTER_ADMIN_TEST(CLASS_NAME, DEVICE)                                              \
  testing::RegisterTest("AdminTest", TestNameForEntry(#CLASS_NAME, DEVICE).c_str(), nullptr, \
                        DevNameForEntry(DEVICE).c_str(), __FILE__, __LINE__,                 \
                        [&]() -> AdminTest* { return new CLASS_NAME(DEVICE); })

#define REGISTER_DISABLED_ADMIN_TEST(CLASS_NAME, DEVICE)                                       \
  testing::RegisterTest(                                                                       \
      "AdminTest", (std::string("DISABLED_") + TestNameForEntry(#CLASS_NAME, DEVICE)).c_str(), \
      nullptr, DevNameForEntry(DEVICE).c_str(), __FILE__, __LINE__,                            \
      [&]() -> AdminTest* { return new CLASS_NAME(DEVICE); })

// Verify PacketStream properties.
DEFINE_ADMIN_TEST_CLASS(PacketStreamGetProperties, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());

  RequestPacketStreamProperties();
});

// Verify PacketStream formats.
DEFINE_ADMIN_TEST_CLASS(PacketStreamGetFormats, { RetrievePacketStreamFormats(); });

// Verify PacketStream Create with min format.
DEFINE_ADMIN_TEST_CLASS(PacketStreamCreateWithMinFormat, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  RequestPacketStreamChannelWithMinPcmFormat();
  WaitForError();
});

// Verify PacketStream Create with max format.
DEFINE_ADMIN_TEST_CLASS(PacketStreamCreateWithMaxFormat, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  RequestPacketStreamChannelWithMaxPcmFormat();
  WaitForError();
});

// Verify PacketStream Create with encoding formats.
DEFINE_ADMIN_TEST_CLASS(PacketStreamCreateWithEncoding, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());

  if (packet_stream_supported_encodings().empty()) {
    GTEST_SKIP() << "Driver does not support any encoding formats";
  }

  for (const auto& encoding_set : packet_stream_supported_encodings()) {
    RequestPacketStreamChannelWithEncoding(fidl::Clone(encoding_set));
    WaitForError();
    ASSERT_NO_FAILURE_OR_SKIP(packet_stream().Unbind());
  }
});

// Verify PacketStream AllocateVmos.
DEFINE_ADMIN_TEST_CLASS(PacketStreamAllocateVmos, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamProperties());

  if (!(packet_stream_props()->supported_buffer_types() &
        fuchsia::hardware::audio::BufferType::DRIVER_OWNED)) {
    GTEST_SKIP() << "Driver does not support DRIVER_OWNED buffers";
  }

  AllocatePacketStreamVmos(1, 4096);
});

// Verify PacketStream AllocateVmos after RegisterVmos should error.
DEFINE_ADMIN_TEST_CLASS(PacketStreamAllocateAfterRegisterShouldError, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamProperties());

  if (!(packet_stream_props()->supported_buffer_types() &
        fuchsia::hardware::audio::BufferType::CLIENT_OWNED)) {
    GTEST_SKIP() << "Driver does not support CLIENT_OWNED buffers";
  }
  if (!(packet_stream_props()->supported_buffer_types() &
        fuchsia::hardware::audio::BufferType::DRIVER_OWNED)) {
    GTEST_SKIP() << "Driver does not support DRIVER_OWNED buffers";
  }

  ASSERT_NO_FAILURE_OR_SKIP(RegisterPacketStreamVmos(1, 4096));

  fuchsia::hardware::audio::AllocateVmosConfig config;
  config.set_vmo_count(1);
  config.set_min_vmo_size(4096);
  packet_stream()->AllocateVmos(
      std::move(config),
      AddCallback("AllocateVmos",
                  [](fuchsia::hardware::audio::PacketStreamControl_AllocateVmos_Result result) {
                    ASSERT_TRUE(result.is_err());
                    EXPECT_EQ(result.err(), ZX_ERR_BAD_STATE);
                  }));
  ExpectCallbacks();
});

// Verify PacketStream AllocateVmos with zero VMO count should error.
DEFINE_ADMIN_TEST_CLASS(PacketStreamAllocateVmosZeroCountShouldError, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamProperties());

  if (!(packet_stream_props()->supported_buffer_types() &
        fuchsia::hardware::audio::BufferType::DRIVER_OWNED)) {
    GTEST_SKIP() << "Driver does not support DRIVER_OWNED buffers";
  }

  fuchsia::hardware::audio::AllocateVmosConfig config;
  config.set_vmo_count(0);  // INVALID
  config.set_min_vmo_size(4096);
  packet_stream()->AllocateVmos(
      std::move(config),
      AddCallback("AllocateVmos",
                  [](fuchsia::hardware::audio::PacketStreamControl_AllocateVmos_Result result) {
                    ASSERT_TRUE(result.is_err());
                    EXPECT_EQ(result.err(), ZX_ERR_INVALID_ARGS);
                  }));
  ExpectCallbacks();
});

// Verify PacketStream AllocateVmos with zero VMO size should error.
DEFINE_ADMIN_TEST_CLASS(PacketStreamAllocateVmosZeroSizeShouldError, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamProperties());

  if (!(packet_stream_props()->supported_buffer_types() &
        fuchsia::hardware::audio::BufferType::DRIVER_OWNED)) {
    GTEST_SKIP() << "Driver does not support DRIVER_OWNED buffers";
  }

  fuchsia::hardware::audio::AllocateVmosConfig config;
  config.set_vmo_count(1);
  config.set_min_vmo_size(0);  // INVALID
  packet_stream()->AllocateVmos(
      std::move(config),
      AddCallback("AllocateVmos",
                  [](fuchsia::hardware::audio::PacketStreamControl_AllocateVmos_Result result) {
                    ASSERT_TRUE(result.is_err());
                    EXPECT_EQ(result.err(), ZX_ERR_INVALID_ARGS);
                  }));
  ExpectCallbacks();
});

// Verify PacketStream AllocateVmos when DRIVER_OWNED not supported should error.
DEFINE_ADMIN_TEST_CLASS(PacketStreamAllocateVmosNoDriverOwnedSupportShouldError, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamProperties());

  if ((packet_stream_props()->supported_buffer_types() &
       fuchsia::hardware::audio::BufferType::DRIVER_OWNED)) {
    GTEST_SKIP() << "Driver SUPPORTS DRIVER_OWNED buffers";
  }

  fuchsia::hardware::audio::AllocateVmosConfig config;
  config.set_vmo_count(1);
  config.set_min_vmo_size(4096);
  packet_stream()->AllocateVmos(
      std::move(config),
      AddCallback("AllocateVmos",
                  [](fuchsia::hardware::audio::PacketStreamControl_AllocateVmos_Result result) {
                    ASSERT_TRUE(result.is_err());
                    EXPECT_EQ(result.err(), ZX_ERR_NOT_SUPPORTED);
                  }));
  ExpectCallbacks();
});

// Verify PacketStream RegisterVmos.
DEFINE_ADMIN_TEST_CLASS(PacketStreamRegisterVmos, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamProperties());

  if (!(packet_stream_props()->supported_buffer_types() &
        fuchsia::hardware::audio::BufferType::CLIENT_OWNED)) {
    GTEST_SKIP() << "Driver does not support CLIENT_OWNED buffers";
  }

  RegisterPacketStreamVmos(1, 4096);
});

// Verify PacketStream RegisterVmos with duplicate IDs should error.
DEFINE_ADMIN_TEST_CLASS(PacketStreamRegisterVmosDuplicateIdsShouldError, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamProperties());

  if (!(packet_stream_props()->supported_buffer_types() &
        fuchsia::hardware::audio::BufferType::CLIENT_OWNED)) {
    GTEST_SKIP() << "Driver does not support CLIENT_OWNED buffers";
  }

  fuchsia::hardware::audio::RegisterVmosConfig config;
  std::vector<fuchsia::hardware::audio::VmoInfo> infos;
  for (int i = 0; i < 2; ++i) {
    zx::vmo vmo;
    ASSERT_EQ(zx::vmo::create(4096, 0, &vmo), ZX_OK);
    fuchsia::hardware::audio::VmoInfo info;
    info.set_id(123);  // DUPLICATE ID
    info.set_vmo(std::move(vmo));
    infos.push_back(std::move(info));
  }
  config.set_vmo_infos(std::move(infos));

  packet_stream()->RegisterVmos(
      std::move(config),
      AddCallback("RegisterVmos",
                  [](fuchsia::hardware::audio::PacketStreamControl_RegisterVmos_Result result) {
                    ASSERT_TRUE(result.is_err());
                    EXPECT_EQ(result.err(), ZX_ERR_INVALID_ARGS);
                  }));
  ExpectCallbacks();
});

// Verify PacketStream RegisterVmos with invalid VMO handle should error.
DEFINE_ADMIN_TEST_CLASS(PacketStreamRegisterVmosInvalidHandleShouldError, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamProperties());

  if (!(packet_stream_props()->supported_buffer_types() &
        fuchsia::hardware::audio::BufferType::CLIENT_OWNED)) {
    GTEST_SKIP() << "Driver does not support CLIENT_OWNED buffers";
  }

  fuchsia::hardware::audio::RegisterVmosConfig config;
  std::vector<fuchsia::hardware::audio::VmoInfo> infos;
  fuchsia::hardware::audio::VmoInfo info;
  info.set_id(1);
  // No VMO set -> invalid handle
  infos.push_back(std::move(info));
  config.set_vmo_infos(std::move(infos));

  packet_stream()->RegisterVmos(
      std::move(config),
      AddCallback("RegisterVmos",
                  [](fuchsia::hardware::audio::PacketStreamControl_RegisterVmos_Result result) {
                    ASSERT_TRUE(result.is_err());
                    EXPECT_EQ(result.err(), ZX_ERR_INVALID_ARGS);
                  }));
  ExpectCallbacks();
});

// Verify PacketStream RegisterVmos with insufficient rights should error.
DEFINE_ADMIN_TEST_CLASS(PacketStreamRegisterVmosInsufficientRightsShouldError, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamProperties());

  if (!(packet_stream_props()->supported_buffer_types() &
        fuchsia::hardware::audio::BufferType::CLIENT_OWNED)) {
    GTEST_SKIP() << "Driver does not support CLIENT_OWNED buffers";
  }

  fuchsia::hardware::audio::RegisterVmosConfig config;
  std::vector<fuchsia::hardware::audio::VmoInfo> infos;
  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(4096, 0, &vmo), ZX_OK);

  // Remove READ right
  zx::vmo limited_vmo;
  ASSERT_EQ(vmo.replace(ZX_RIGHT_TRANSFER | ZX_RIGHT_MAP, &limited_vmo), ZX_OK);

  fuchsia::hardware::audio::VmoInfo info;
  info.set_id(1);
  info.set_vmo(std::move(limited_vmo));
  infos.push_back(std::move(info));
  config.set_vmo_infos(std::move(infos));

  packet_stream()->RegisterVmos(
      std::move(config),
      AddCallback("RegisterVmos",
                  [](fuchsia::hardware::audio::PacketStreamControl_RegisterVmos_Result result) {
                    ASSERT_TRUE(result.is_err());
                    EXPECT_EQ(result.err(), ZX_ERR_ACCESS_DENIED);
                  }));
  ExpectCallbacks();
});

// Verify PacketStream RegisterVmos after AllocateVmos should error.
DEFINE_ADMIN_TEST_CLASS(PacketStreamRegisterAfterAllocateShouldError, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamProperties());

  if (!(packet_stream_props()->supported_buffer_types() &
        fuchsia::hardware::audio::BufferType::CLIENT_OWNED)) {
    GTEST_SKIP() << "Driver does not support CLIENT_OWNED buffers";
  }
  if (!(packet_stream_props()->supported_buffer_types() &
        fuchsia::hardware::audio::BufferType::DRIVER_OWNED)) {
    GTEST_SKIP() << "Driver does not support DRIVER_OWNED buffers";
  }

  ASSERT_NO_FAILURE_OR_SKIP(AllocatePacketStreamVmos(1, 4096));

  fuchsia::hardware::audio::RegisterVmosConfig config;
  std::vector<fuchsia::hardware::audio::VmoInfo> infos;
  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(4096, 0, &vmo), ZX_OK);
  fuchsia::hardware::audio::VmoInfo info;
  info.set_id(99);
  info.set_vmo(std::move(vmo));
  infos.push_back(std::move(info));
  config.set_vmo_infos(std::move(infos));

  packet_stream()->RegisterVmos(
      std::move(config),
      AddCallback("RegisterVmos",
                  [](fuchsia::hardware::audio::PacketStreamControl_RegisterVmos_Result result) {
                    ASSERT_TRUE(result.is_err());
                    EXPECT_EQ(result.err(), ZX_ERR_BAD_STATE);
                  }));
  ExpectCallbacks();
});

// Verify PacketStream RegisterVmos with zero size VMO should error.
DEFINE_ADMIN_TEST_CLASS(PacketStreamRegisterVmosZeroSizeShouldError, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamProperties());

  if (!(packet_stream_props()->supported_buffer_types() &
        fuchsia::hardware::audio::BufferType::CLIENT_OWNED)) {
    GTEST_SKIP() << "Driver does not support CLIENT_OWNED buffers";
  }

  fuchsia::hardware::audio::RegisterVmosConfig config;
  std::vector<fuchsia::hardware::audio::VmoInfo> infos;
  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(0, 0, &vmo), ZX_OK);

  fuchsia::hardware::audio::VmoInfo info;
  info.set_id(1);
  info.set_vmo(std::move(vmo));
  infos.push_back(std::move(info));
  config.set_vmo_infos(std::move(infos));

  packet_stream()->RegisterVmos(
      std::move(config),
      AddCallback("RegisterVmos",
                  [](fuchsia::hardware::audio::PacketStreamControl_RegisterVmos_Result result) {
                    ASSERT_TRUE(result.is_err());
                    EXPECT_EQ(result.err(), ZX_ERR_INVALID_ARGS);
                  }));
  ExpectCallbacks();
});

// Verify PacketStream RegisterVmos when CLIENT_OWNED not supported should error.
DEFINE_ADMIN_TEST_CLASS(PacketStreamRegisterVmosNoClientOwnedSupportShouldError, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamProperties());

  if ((packet_stream_props()->supported_buffer_types() &
       fuchsia::hardware::audio::BufferType::CLIENT_OWNED)) {
    GTEST_SKIP() << "Driver SUPPORTS CLIENT_OWNED buffers";
  }

  fuchsia::hardware::audio::RegisterVmosConfig config;
  std::vector<fuchsia::hardware::audio::VmoInfo> infos;
  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(4096, 0, &vmo), ZX_OK);
  fuchsia::hardware::audio::VmoInfo info;
  info.set_id(1);
  info.set_vmo(std::move(vmo));
  infos.push_back(std::move(info));
  config.set_vmo_infos(std::move(infos));

  packet_stream()->RegisterVmos(
      std::move(config),
      AddCallback("RegisterVmos",
                  [](fuchsia::hardware::audio::PacketStreamControl_RegisterVmos_Result result) {
                    ASSERT_TRUE(result.is_err());
                    EXPECT_EQ(result.err(), ZX_ERR_NOT_SUPPORTED);
                  }));
  ExpectCallbacks();
});

// Verify PacketStream DeallocateVmos while not allocated should error.
DEFINE_ADMIN_TEST_CLASS(PacketStreamDeallocateVmosBeforeAllocateShouldError, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamProperties());

  packet_stream()->DeallocateVmos(
      AddCallback("DeallocateVmos",
                  [](fuchsia::hardware::audio::PacketStreamControl_DeallocateVmos_Result result) {
                    ASSERT_TRUE(result.is_err());
                    EXPECT_EQ(result.err(), ZX_ERR_BAD_STATE);
                  }));
  ExpectCallbacks();
});

// Verify PacketStream DeallocateVmos while started should error.
DEFINE_ADMIN_TEST_CLASS(PacketStreamDeallocateVmosWhileStartedShouldError, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamProperties());

  if (!(packet_stream_props()->supported_buffer_types() &
        fuchsia::hardware::audio::BufferType::DRIVER_OWNED)) {
    GTEST_SKIP() << "Driver does not support DRIVER_OWNED buffers";
  }

  ASSERT_NO_FAILURE_OR_SKIP(AllocatePacketStreamVmos(1, 4096));
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamStartAndExpectCallback());

  packet_stream()->DeallocateVmos(
      AddCallback("DeallocateVmos",
                  [](fuchsia::hardware::audio::PacketStreamControl_DeallocateVmos_Result result) {
                    ASSERT_TRUE(result.is_err());
                    EXPECT_EQ(result.err(), ZX_ERR_BAD_STATE);
                  }));
  ExpectCallbacks();
});

// Verify PacketStream UnregisterVmos while not registered should error.
DEFINE_ADMIN_TEST_CLASS(PacketStreamUnregisterVmosBeforeRegisterShouldError, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());

  packet_stream()->UnregisterVmos(
      AddCallback("UnregisterVmos",
                  [](fuchsia::hardware::audio::PacketStreamControl_UnregisterVmos_Result result) {
                    ASSERT_TRUE(result.is_err());
                    EXPECT_EQ(result.err(), ZX_ERR_BAD_STATE);
                  }));
  ExpectCallbacks();
});

// Verify PacketStream UnregisterVmos while started should error.
DEFINE_ADMIN_TEST_CLASS(PacketStreamUnregisterVmosWhileStartedShouldError, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamProperties());

  if (!(packet_stream_props()->supported_buffer_types() &
        fuchsia::hardware::audio::BufferType::CLIENT_OWNED)) {
    GTEST_SKIP() << "Driver does not support CLIENT_OWNED buffers";
  }

  ASSERT_NO_FAILURE_OR_SKIP(RegisterPacketStreamVmos(1, 4096));
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamStartAndExpectCallback());

  packet_stream()->UnregisterVmos(
      AddCallback("UnregisterVmos",
                  [](fuchsia::hardware::audio::PacketStreamControl_UnregisterVmos_Result result) {
                    ASSERT_TRUE(result.is_err());
                    EXPECT_EQ(result.err(), ZX_ERR_BAD_STATE);
                  }));
  ExpectCallbacks();
});

// Verify PacketStream Start without buffers should error.
DEFINE_ADMIN_TEST_CLASS(PacketStreamStartBeforeBuffersShouldError, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamProperties());

  if (packet_stream_props()->supported_buffer_types() &
      fuchsia::hardware::audio::BufferType::INLINE) {
    GTEST_SKIP() << "Driver supports INLINE buffers, Start might be permitted";
  }

  packet_stream()->Start(
      AddCallback("Start", [](fuchsia::hardware::audio::PacketStreamControl_Start_Result result) {
        ASSERT_TRUE(result.is_err());
        EXPECT_EQ(result.err(), ZX_ERR_BAD_STATE);
      }));
  ExpectCallbacks();
});

// Verify Start() errors if the driver receives it before responding to a previous Start().
DEFINE_ADMIN_TEST_CLASS(PacketStreamStartWhileStartingShouldError, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamProperties());

  if (packet_stream_props()->supported_buffer_types() &
      fuchsia::hardware::audio::BufferType::CLIENT_OWNED) {
    ASSERT_NO_FAILURE_OR_SKIP(RegisterPacketStreamVmos(1, 4096));
  } else if (packet_stream_props()->supported_buffer_types() &
             fuchsia::hardware::audio::BufferType::DRIVER_OWNED) {
    ASSERT_NO_FAILURE_OR_SKIP(AllocatePacketStreamVmos(1, 4096));
  } else if (!(packet_stream_props()->supported_buffer_types() &
               fuchsia::hardware::audio::BufferType::INLINE)) {
    GTEST_SKIP() << "Driver supports no buffer types?";
  }

  // Don't wait for the callback...
  auto _ = RequestPacketStreamStart();
  // ...just immediately call again.
  RequestPacketStreamStartAndExpectError(ZX_ERR_BAD_STATE);
});

// Verify PacketStream Start while started should error.
DEFINE_ADMIN_TEST_CLASS(PacketStreamStartWhileStartedShouldError, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamProperties());

  if (packet_stream_props()->supported_buffer_types() &
      fuchsia::hardware::audio::BufferType::CLIENT_OWNED) {
    ASSERT_NO_FAILURE_OR_SKIP(RegisterPacketStreamVmos(1, 4096));
  } else if (packet_stream_props()->supported_buffer_types() &
             fuchsia::hardware::audio::BufferType::DRIVER_OWNED) {
    ASSERT_NO_FAILURE_OR_SKIP(AllocatePacketStreamVmos(1, 4096));
  } else if (!(packet_stream_props()->supported_buffer_types() &
               fuchsia::hardware::audio::BufferType::INLINE)) {
    GTEST_SKIP() << "Driver supports no buffer types?";
  }

  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamStartAndExpectCallback());

  RequestPacketStreamStartAndExpectError(ZX_ERR_BAD_STATE);
});

// Verify PacketStream Stop while stopped should error.
DEFINE_ADMIN_TEST_CLASS(PacketStreamStopWhileStoppedShouldError, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());

  packet_stream()->Stop(
      AddCallback("Stop", [](fuchsia::hardware::audio::PacketStreamControl_Stop_Result result) {
        ASSERT_TRUE(result.is_err());
        EXPECT_EQ(result.err(), ZX_ERR_BAD_STATE);
      }));
  ExpectCallbacks();
});

// Verify PacketStream GetPacketStreamSink for input stream should error.
DEFINE_ADMIN_TEST_CLASS(PacketStreamGetSinkForInputShouldError, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());

  std::optional<bool> is_incoming = ElementIsIncoming(packet_stream_id());
  ASSERT_TRUE(is_incoming.has_value());
  if (!*is_incoming) {
    GTEST_SKIP() << "This is an output stream, skipping negative test for input";
  }

  RequestPacketStreamSinkAndExpectError(ZX_ERR_NOT_SUPPORTED);
  WaitForError();
});

// Verify PacketStream SetPacketStreamSink for output stream should error.
DEFINE_ADMIN_TEST_CLASS(PacketStreamSetSinkForOutputShouldError, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());

  std::optional<bool> is_incoming = ElementIsIncoming(packet_stream_id());
  ASSERT_TRUE(is_incoming.has_value());
  if (*is_incoming) {
    GTEST_SKIP() << "This is an input stream, skipping negative test for output";
  }

  fidl::InterfaceHandle<fuchsia::hardware::audio::PacketStreamSink> sink_handle;
  auto sink_request = sink_handle.NewRequest();
  fuchsia::hardware::audio::PacketStreamControlSetPacketStreamSinkRequest request;
  request.set_stream(std::move(sink_handle));

  packet_stream()->SetPacketStreamSink(
      std::move(request),
      AddCallback(
          "SetPacketStreamSink",
          [](fuchsia::hardware::audio::PacketStreamControl_SetPacketStreamSink_Result result) {
            ASSERT_TRUE(result.is_err());
            EXPECT_EQ(result.err(), ZX_ERR_NOT_SUPPORTED);
          }));
  ExpectCallbacks();
});

// Verify PacketStreamPutPacket with invalid VMO ID should error.
DEFINE_ADMIN_TEST_CLASS(PacketStreamPutPacketInvalidVmoIdShouldError, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamProperties());

  if (!(packet_stream_props()->supported_buffer_types() &
        (fuchsia::hardware::audio::BufferType::CLIENT_OWNED |
         fuchsia::hardware::audio::BufferType::DRIVER_OWNED))) {
    GTEST_SKIP() << "Driver does not support VMO buffers";
  }

  if (packet_stream_props()->supported_buffer_types() &
      fuchsia::hardware::audio::BufferType::CLIENT_OWNED) {
    ASSERT_NO_FAILURE_OR_SKIP(RegisterPacketStreamVmos(1, 4096));
  } else {
    ASSERT_NO_FAILURE_OR_SKIP(AllocatePacketStreamVmos(1, 4096));
  }

  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamSink());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamStartAndExpectCallback());

  fuchsia::hardware::audio::PacketStreamSinkPutPacketRequest request;
  fuchsia::hardware::audio::VmoTransfer transfer;
  transfer.set_vmo_id(999);  // invalid ID
  transfer.set_vmo_offset(0);
  transfer.set_payload_size(10);
  fuchsia::hardware::audio::DataTransfer payload;
  payload.set_vmo_transfer(std::move(transfer));
  request.set_payload(std::move(payload));

  PacketStreamPutPacketAndExpectError(std::move(request), ZX_ERR_INVALID_ARGS);
  WaitForError();
});

// Verify PacketStreamPutPacket with out of bounds VMO offset/size should error.
DEFINE_ADMIN_TEST_CLASS(PacketStreamPutPacketOutOfBoundsShouldError, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamProperties());

  if (!(packet_stream_props()->supported_buffer_types() &
        (fuchsia::hardware::audio::BufferType::CLIENT_OWNED |
         fuchsia::hardware::audio::BufferType::DRIVER_OWNED))) {
    GTEST_SKIP() << "Driver does not support VMO buffers";
  }

  const uint64_t kVmoSize = 4096;
  if (packet_stream_props()->supported_buffer_types() &
      fuchsia::hardware::audio::BufferType::CLIENT_OWNED) {
    ASSERT_NO_FAILURE_OR_SKIP(RegisterPacketStreamVmos(1, kVmoSize));
  } else {
    ASSERT_NO_FAILURE_OR_SKIP(AllocatePacketStreamVmos(1, kVmoSize));
  }

  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamSink());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamStartAndExpectCallback());

  fuchsia::hardware::audio::PacketStreamSinkPutPacketRequest request;
  fuchsia::hardware::audio::VmoTransfer transfer;
  transfer.set_vmo_id(0);
  transfer.set_vmo_offset(kVmoSize - 5);
  transfer.set_payload_size(10);  // 5 bytes out of bounds
  fuchsia::hardware::audio::DataTransfer payload;
  payload.set_vmo_transfer(std::move(transfer));
  request.set_payload(std::move(payload));

  PacketStreamPutPacketAndExpectError(std::move(request), ZX_ERR_INVALID_ARGS);
  WaitForError();
});

// Verify PacketStream FlushPackets.
DEFINE_ADMIN_TEST_CLASS(PacketStreamFlushPacketsTest, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamProperties());

  if (packet_stream_props()->supported_buffer_types() &
      fuchsia::hardware::audio::BufferType::CLIENT_OWNED) {
    ASSERT_NO_FAILURE_OR_SKIP(RegisterPacketStreamVmos(1, 4096));
  } else if (packet_stream_props()->supported_buffer_types() &
             fuchsia::hardware::audio::BufferType::DRIVER_OWNED) {
    ASSERT_NO_FAILURE_OR_SKIP(AllocatePacketStreamVmos(1, 4096));
  }

  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamSink());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamStartAndExpectCallback());

  PacketStreamFlushPackets();
  WaitForError();
});

// Create a PacketStream, drop it, recreate it, then interact with it in any way (e.g.
// GetProperties).
DEFINE_ADMIN_TEST_CLASS(PacketStreamGetPropertiesAfterDroppingFirstPacketStream, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamProperties());

  packet_stream().Unbind();
  packet_stream() = nullptr;

  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());
  RequestPacketStreamProperties();
  WaitForError();
});

// Verify PacketStream basic flow: Channel, Properties, VMOs, Sink, Start, Output, Stop.
DEFINE_ADMIN_TEST_CLASS(PacketStreamOutputBasic, {
  ASSERT_NO_FAILURE_OR_SKIP(RetrievePacketStreamFormats());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamChannel());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamProperties());

  std::optional<bool> is_incoming = ElementIsIncoming(packet_stream_id());
  ASSERT_TRUE(is_incoming.has_value());
  if (*is_incoming) {
    GTEST_SKIP() << "Driver supports input only";
  }

  if (packet_stream_props()->supported_buffer_types() &
      fuchsia::hardware::audio::BufferType::CLIENT_OWNED) {
    ASSERT_NO_FAILURE_OR_SKIP(RegisterPacketStreamVmos(1, 4096));
  } else if (packet_stream_props()->supported_buffer_types() &
             fuchsia::hardware::audio::BufferType::DRIVER_OWNED) {
    ASSERT_NO_FAILURE_OR_SKIP(AllocatePacketStreamVmos(1, 4096));
  }

  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamSink());
  ASSERT_NO_FAILURE_OR_SKIP(RequestPacketStreamStartAndExpectCallback());

  ASSERT_NO_FAILURE_OR_SKIP(PacketStreamPutPacket());

  RequestPacketStreamStopAndExpectCallback();
});

void RegisterAdminTestsForDevice(const DeviceEntry& device_entry) {
  if (device_entry.isCodec()) {
    REGISTER_ADMIN_TEST(Reset, device_entry);

    REGISTER_ADMIN_TEST(CodecStop, device_entry);
    REGISTER_ADMIN_TEST(CodecStart, device_entry);
  } else if (device_entry.isComposite()) {
    // Composite test cases
    //
    REGISTER_ADMIN_TEST(CompositeHealth, device_entry);
    REGISTER_ADMIN_TEST(CompositeUnknownMethod, device_entry);
    REGISTER_ADMIN_TEST(CompositeProperties, device_entry);
    REGISTER_ADMIN_TEST(CompositeRingBufferFormats, device_entry);

    // Add this for debugging purposes, to show what the driver is exposing:
    if constexpr (kDisplayDaiFormatSets) {
      REGISTER_ADMIN_TEST(DisplayDaiFormatSets, device_entry);
    }
    REGISTER_ADMIN_TEST(CompositeDaiFormats, device_entry);

    // TODO(https://fxbug.dev/42075676): Add Composite testing (e.g. Reset, SetDaiFormat).
    // REGISTER_ADMIN_TEST(SetDaiFormat, device_entry); // test all DAIs, not just the first.
    // Reset should close RingBuffers and revert SetTopology, SetElementState and SetDaiFormat.
    // REGISTER_ADMIN_TEST(CompositeReset, device_entry);

    // signalprocessing Element test cases
    //
    REGISTER_ADMIN_TEST(GetElements, device_entry);
    // type-specific cases
    REGISTER_ADMIN_TEST(DaiElements, device_entry);
    REGISTER_ADMIN_TEST(DynamicsElements, device_entry);
    REGISTER_ADMIN_TEST(EqualizerElements, device_entry);
    REGISTER_ADMIN_TEST(GainElements, device_entry);
    REGISTER_ADMIN_TEST(VendorSpecificElements, device_entry);

    // signalprocessing Topology test cases
    //
    REGISTER_ADMIN_TEST(GetTopologies, device_entry);
    REGISTER_ADMIN_TEST(ElementTopologyClosure, device_entry);
    REGISTER_ADMIN_TEST(InitialTopology, device_entry);
    REGISTER_ADMIN_TEST(SetTopology, device_entry);
    REGISTER_ADMIN_TEST(SetTopologyUnknownIdShouldError, device_entry);
    REGISTER_ADMIN_TEST(WatchTopologyWhilePendingShouldDisconnect, device_entry);

    // signalprocessing ElementState test cases
    //
    REGISTER_ADMIN_TEST(InitialElementStates, device_entry);
    // type-specific cases
    REGISTER_ADMIN_TEST(DaiElementStates, device_entry);
    REGISTER_ADMIN_TEST(DynamicsElementStates, device_entry);
    REGISTER_ADMIN_TEST(EqualizerElementStates, device_entry);
    REGISTER_ADMIN_TEST(GainElementStates, device_entry);
    REGISTER_ADMIN_TEST(VendorSpecificElementStates, device_entry);

    REGISTER_ADMIN_TEST(WatchElementStateWhilePendingShouldDisconnect, device_entry);
    REGISTER_ADMIN_TEST(WatchElementStateUnknownIdShouldDisconnect, device_entry);

    REGISTER_ADMIN_TEST(SetElementState, device_entry);
    REGISTER_ADMIN_TEST(SetElementStateNoChange, device_entry);
    REGISTER_ADMIN_TEST(SetElementStateUnknownIdShouldError, device_entry);
    // type-specific cases
    REGISTER_ADMIN_TEST(SetDynamicsElementState, device_entry);
    REGISTER_ADMIN_TEST(SetEqualizerElementState, device_entry);
    REGISTER_ADMIN_TEST(SetGainElementState, device_entry);
    REGISTER_ADMIN_TEST(SetGainElementStateNoChange, device_entry);
    REGISTER_ADMIN_TEST(SetGainElementStateInvalidGain, device_entry);
    // Cannot verify SettableVendorSpecificElementState unless we apply structure to it....

    // RingBuffer test cases
    //
    // TODO(https://fxbug.dev/42075676): Add Composite testing (all RingBuffers, not just first).
    REGISTER_ADMIN_TEST(GetRingBufferProperties, device_entry);
    REGISTER_ADMIN_TEST(GetRingBufferVmo, device_entry);
    REGISTER_ADMIN_TEST(DriverReservesRingBufferSpace, device_entry);

    REGISTER_ADMIN_TEST(InternalDelayIsValid, device_entry);
    REGISTER_ADMIN_TEST(ExternalDelayIsValid, device_entry);
    REGISTER_ADMIN_TEST(GetDelayInfoSecondTimeNoResponse, device_entry);

    REGISTER_ADMIN_TEST(SetActiveChannelsChange, device_entry);
    REGISTER_ADMIN_TEST(SetActiveChannelsTooHigh, device_entry);
    REGISTER_ADMIN_TEST(SetActiveChannelsNoChange, device_entry);

    REGISTER_ADMIN_TEST(PositionNotifyBeforeStart, device_entry);
    REGISTER_ADMIN_TEST(PositionNotifyNone, device_entry);
    REGISTER_ADMIN_TEST(PositionNotifyAfterStop, device_entry);

    REGISTER_ADMIN_TEST(RingBufferStart, device_entry);
    REGISTER_ADMIN_TEST(RingBufferStartBeforeGetVmoShouldDisconnect, device_entry);
    REGISTER_ADMIN_TEST(RingBufferStartWhileStartingShouldDisconnect, device_entry);
    REGISTER_ADMIN_TEST(RingBufferStartWhileStartedShouldDisconnect, device_entry);
    REGISTER_ADMIN_TEST(GetDelayInfoAfterStart, device_entry);

    REGISTER_ADMIN_TEST(RingBufferStop, device_entry);
    REGISTER_ADMIN_TEST(RingBufferStopBeforeGetVmoShouldDisconnect, device_entry);
    REGISTER_ADMIN_TEST(RingBufferStopWhileStoppedIsPermitted, device_entry);

    REGISTER_ADMIN_TEST(GetRingBufferPropertiesAfterDroppingFirstRingBuffer, device_entry);
    REGISTER_ADMIN_TEST(GetDelayInfoAfterDroppingFirstRingBuffer, device_entry);
    REGISTER_ADMIN_TEST(SetActiveChannelsAfterDroppingFirstRingBuffer, device_entry);

    // PacketStream test cases
    REGISTER_ADMIN_TEST(PacketStreamGetProperties, device_entry);
    REGISTER_ADMIN_TEST(PacketStreamGetFormats, device_entry);

    REGISTER_ADMIN_TEST(PacketStreamCreateWithMinFormat, device_entry);
    REGISTER_ADMIN_TEST(PacketStreamCreateWithMaxFormat, device_entry);
    REGISTER_ADMIN_TEST(PacketStreamCreateWithEncoding, device_entry);

    REGISTER_ADMIN_TEST(PacketStreamAllocateVmos, device_entry);
    REGISTER_ADMIN_TEST(PacketStreamAllocateAfterRegisterShouldError, device_entry);
    REGISTER_ADMIN_TEST(PacketStreamAllocateVmosZeroCountShouldError, device_entry);
    REGISTER_ADMIN_TEST(PacketStreamAllocateVmosZeroSizeShouldError, device_entry);
    REGISTER_ADMIN_TEST(PacketStreamAllocateVmosNoDriverOwnedSupportShouldError, device_entry);

    REGISTER_ADMIN_TEST(PacketStreamRegisterVmos, device_entry);
    REGISTER_ADMIN_TEST(PacketStreamRegisterVmosDuplicateIdsShouldError, device_entry);
    REGISTER_ADMIN_TEST(PacketStreamRegisterVmosInvalidHandleShouldError, device_entry);
    REGISTER_ADMIN_TEST(PacketStreamRegisterVmosInsufficientRightsShouldError, device_entry);
    REGISTER_ADMIN_TEST(PacketStreamRegisterAfterAllocateShouldError, device_entry);
    REGISTER_ADMIN_TEST(PacketStreamRegisterVmosZeroSizeShouldError, device_entry);
    REGISTER_ADMIN_TEST(PacketStreamRegisterVmosNoClientOwnedSupportShouldError, device_entry);

    REGISTER_ADMIN_TEST(PacketStreamDeallocateVmosBeforeAllocateShouldError, device_entry);
    REGISTER_ADMIN_TEST(PacketStreamDeallocateVmosWhileStartedShouldError, device_entry);

    REGISTER_ADMIN_TEST(PacketStreamUnregisterVmosBeforeRegisterShouldError, device_entry);
    REGISTER_ADMIN_TEST(PacketStreamUnregisterVmosWhileStartedShouldError, device_entry);

    REGISTER_ADMIN_TEST(PacketStreamStartBeforeBuffersShouldError, device_entry);
    REGISTER_ADMIN_TEST(PacketStreamStartWhileStartingShouldError, device_entry);
    REGISTER_ADMIN_TEST(PacketStreamStartWhileStartedShouldError, device_entry);

    REGISTER_ADMIN_TEST(PacketStreamStopWhileStoppedShouldError, device_entry);

    REGISTER_ADMIN_TEST(PacketStreamGetSinkForInputShouldError, device_entry);
    REGISTER_ADMIN_TEST(PacketStreamSetSinkForOutputShouldError, device_entry);

    REGISTER_ADMIN_TEST(PacketStreamPutPacketInvalidVmoIdShouldError, device_entry);
    REGISTER_ADMIN_TEST(PacketStreamPutPacketOutOfBoundsShouldError, device_entry);

    REGISTER_ADMIN_TEST(PacketStreamFlushPacketsTest, device_entry);

    REGISTER_ADMIN_TEST(PacketStreamGetPropertiesAfterDroppingFirstPacketStream, device_entry);

    REGISTER_ADMIN_TEST(PacketStreamOutputBasic, device_entry);
  } else if (device_entry.isDai()) {
    REGISTER_ADMIN_TEST(GetRingBufferProperties, device_entry);
    REGISTER_ADMIN_TEST(GetRingBufferVmo, device_entry);
    REGISTER_ADMIN_TEST(DriverReservesRingBufferSpace, device_entry);

    REGISTER_ADMIN_TEST(InternalDelayIsValid, device_entry);
    REGISTER_ADMIN_TEST(ExternalDelayIsValid, device_entry);
    REGISTER_ADMIN_TEST(GetDelayInfoSecondTimeNoResponse, device_entry);

    REGISTER_ADMIN_TEST(SetActiveChannelsChange, device_entry);
    REGISTER_ADMIN_TEST(SetActiveChannelsTooHigh, device_entry);
    REGISTER_ADMIN_TEST(SetActiveChannelsNoChange, device_entry);

    REGISTER_ADMIN_TEST(PositionNotifyBeforeStart, device_entry);
    REGISTER_ADMIN_TEST(PositionNotifyNone, device_entry);
    REGISTER_ADMIN_TEST(PositionNotifyAfterStop, device_entry);

    REGISTER_ADMIN_TEST(RingBufferStart, device_entry);
    REGISTER_ADMIN_TEST(RingBufferStartBeforeGetVmoShouldDisconnect, device_entry);
    REGISTER_ADMIN_TEST(RingBufferStartWhileStartingShouldDisconnect, device_entry);
    REGISTER_ADMIN_TEST(RingBufferStartWhileStartedShouldDisconnect, device_entry);
    REGISTER_ADMIN_TEST(GetDelayInfoAfterStart, device_entry);

    REGISTER_ADMIN_TEST(RingBufferStop, device_entry);
    REGISTER_ADMIN_TEST(RingBufferStopBeforeGetVmoShouldDisconnect, device_entry);
    REGISTER_ADMIN_TEST(RingBufferStopWhileStoppedIsPermitted, device_entry);

    REGISTER_ADMIN_TEST(GetRingBufferPropertiesAfterDroppingFirstRingBuffer, device_entry);
    REGISTER_ADMIN_TEST(GetDelayInfoAfterDroppingFirstRingBuffer, device_entry);
    REGISTER_ADMIN_TEST(SetActiveChannelsAfterDroppingFirstRingBuffer, device_entry);
  } else if (device_entry.isStreamConfig()) {
    // TODO(https://fxbug.dev/445731230): reenable once `core.vim3-vg-hwasan` timing issue is fixed
    if (!device_entry.isVirtual()) {
      REGISTER_ADMIN_TEST(GetRingBufferProperties, device_entry);
      REGISTER_ADMIN_TEST(GetRingBufferVmo, device_entry);
      REGISTER_ADMIN_TEST(DriverReservesRingBufferSpace, device_entry);

      REGISTER_ADMIN_TEST(InternalDelayIsValid, device_entry);
      REGISTER_ADMIN_TEST(ExternalDelayIsValid, device_entry);
      REGISTER_ADMIN_TEST(GetDelayInfoSecondTimeNoResponse, device_entry);

      REGISTER_ADMIN_TEST(SetActiveChannelsChange, device_entry);
      REGISTER_ADMIN_TEST(SetActiveChannelsTooHigh, device_entry);
      REGISTER_ADMIN_TEST(SetActiveChannelsNoChange, device_entry);

      REGISTER_ADMIN_TEST(PositionNotifyBeforeStart, device_entry);
      REGISTER_ADMIN_TEST(PositionNotifyNone, device_entry);
      REGISTER_ADMIN_TEST(PositionNotifyAfterStop, device_entry);

      REGISTER_ADMIN_TEST(RingBufferStart, device_entry);
      REGISTER_ADMIN_TEST(RingBufferStartBeforeGetVmoShouldDisconnect, device_entry);
      REGISTER_ADMIN_TEST(RingBufferStartWhileStartingShouldDisconnect, device_entry);
      REGISTER_ADMIN_TEST(RingBufferStartWhileStartedShouldDisconnect, device_entry);
      REGISTER_ADMIN_TEST(GetDelayInfoAfterStart, device_entry);

      REGISTER_ADMIN_TEST(RingBufferStop, device_entry);
      REGISTER_ADMIN_TEST(RingBufferStopBeforeGetVmoShouldDisconnect, device_entry);
      REGISTER_ADMIN_TEST(RingBufferStopWhileStoppedIsPermitted, device_entry);

      REGISTER_ADMIN_TEST(GetRingBufferPropertiesAfterDroppingFirstRingBuffer, device_entry);
      REGISTER_ADMIN_TEST(GetDelayInfoAfterDroppingFirstRingBuffer, device_entry);
      REGISTER_ADMIN_TEST(SetActiveChannelsAfterDroppingFirstRingBuffer, device_entry);
    }
  } else {
    FAIL() << "Unknown device type";
  }
}

// TODO(https://fxbug.dev/302704556): Add Watch-while-still-pending tests for delay and position.

// TODO(https://fxbug.dev/42075676): Add testing for Composite protocol methods.
// SetDaiFormatUnsupported
//    Codec::SetDaiFormat with bad format returns the expected ZX_ERR_INVALID_ARGS.
//    Codec should still be usable (protocol channel still open), after an error is returned.
// SetDaiFormatWhileUnplugged (not testable in automated environment)

// TODO(https://fxbug.dev/42077405): Add remaining testing for SignalProcessing methods.
//
// Proposed remaining test cases for fuchsia.hardware.audio.signalprocessing listed below:
//
// SetElementStateBadValues
//    SetElementState(badVal) returns ZX_ERR_INVALID_ARGS, not close channel. Fail on other error.
//    Bad val could be out-of-range, inf/nan, other malformed, or setting something unsettable.
//    Test this for more type-specific variants of SettableElementState than just GAIN w/NAN?
//
//
// Once drivers can accept a config change that invalidates the current signalprocessing state:
//
// SetTopologyInvalidated
//    First make a change that invalidates the SignalProcessing configuration, then
//    (hanging) WatchTopology should ... return ZX_ERR_BAD_STATE and not close channel?
//    SetTopology should return ZX_ERR_BAD_STATE and not close channel.
// SetTopologyReconfigured
//    First invalidate the SignalProcessing configuration, then retrieve the new topologies.
//    SetTopology returns callback (does not fail or close channel).
//    WatchTopology acknowledges the change made by SetTopology.
// SetElementStateInvalidated
//    First make a change that invalidates the SignalProcessing configuration, then
//    SetElementState should return ZX_ERR_BAD_STATE and not close channel.
// SetElementStateReconfigured
//    First invalidate the SignalProcessing configuration, then retrieve new elements/topologies.
//    SetElementState returns callback (does not fail or close channel).

// TODO(https://fxbug.dev/481768946): Add remaining exhaustive testing for PacketStream methods.

}  // namespace media::audio::drivers::test
