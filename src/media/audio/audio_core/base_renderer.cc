// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/media/audio/audio_core/base_renderer.h"

#include <lib/fit/defer.h>
#include <lib/syslog/cpp/macros.h>

#include <algorithm>
#include <limits>

#include <ffl/string.h>

#include "src/media/audio/audio_core/logging_flags.h"
#include "src/media/audio/lib/clock/clone_mono.h"

namespace media::audio {
namespace {

// If client does not specify a ref_time for Play, pad it by this amount
constexpr zx::duration kPaddingForUnspecifiedRefTime = zx::msec(20);

// 4 slabs will allow each renderer to create >500 packets. Any client creating any more packets
// than this that are outstanding at the same time will be disconnected.
constexpr size_t kMaxPacketAllocatorSlabs = 4;

// Assert our implementation-defined limit is compatible with the FIDL limit.
static_assert(fuchsia::media::MAX_FRAMES_PER_RENDERER_PACKET <= Fixed::Max().Floor());

}  // namespace

BaseRenderer::BaseRenderer(
    fidl::InterfaceRequest<fuchsia::media::AudioRenderer> audio_renderer_request, Context* context)
    : AudioObject(Type::AudioRenderer),
      context_(*context),
      audio_renderer_binding_(this, std::move(audio_renderer_request)),
      pts_ticks_per_second_(kDefaultPtsTicksPerSecondNumerator,
                            kDefaultPtsTicksPerSecondDenominator),
      reference_clock_to_fractional_frames_(fbl::MakeRefCounted<VersionedTimelineFunction>()),
      packet_allocator_(kMaxPacketAllocatorSlabs, true),
      reporter_(Reporter::Singleton().CreateRenderer()) {
  TRACE_DURATION("audio", "BaseRenderer::BaseRenderer");
  FX_DCHECK(context);

  // Set the default immediately: don't require Reporter to maintain the default values.
  reporter_->SetPtsUnits(kDefaultPtsTicksPerSecondNumerator, kDefaultPtsTicksPerSecondDenominator);

  // Our default clock starts as an adjustable clone of MONOTONIC, but ultimately it will track the
  // clock of the device where the renderer is routed.
  SetAdjustableReferenceClock();

  audio_renderer_binding_.set_error_handler([this](zx_status_t status) {
    TRACE_DURATION("audio", "BaseRenderer::audio_renderer_binding_.error_handler", "zx_status",
                   status);
    FX_LOGS(DEBUG) << "Client disconnected";
    BeginShutdown();
  });
}

BaseRenderer::~BaseRenderer() {
  if (IsPlaying()) {
    // The child dtor should have already called ReportStopIfStarted() as needed.
    FX_LOGS(ERROR) << "~BaseRenderer: stream " << static_cast<fuchsia::media::AudioRenderer*>(this)
                   << " is still playing";
  }

  wav_writer_.Close();
  payload_buffers_.clear();
}

// Because a PacketQueue might need to outlive its Renderer, and because (in the future) there could
// be multiple destinations for a single renderer, we duplicate the underlying zx::clock here and
// send a new AudioClock object to each PacketQueue. If the client uses our clock (which is
// adjustable), then one PacketQueue will receive an AudioClock marked adjustable. All other
// PacketQueues receive AudioClocks that are non-adjustable.
fpromise::result<std::shared_ptr<ReadableStream>, zx_status_t> BaseRenderer::InitializeDestLink(
    const AudioObject& dest) {
  TRACE_DURATION("audio", "BaseRenderer::InitializeDestLink");

  // The PacketQueue uses our same clock.
  auto queue =
      std::make_shared<PacketQueue>(*format(), reference_clock_to_fractional_frames_, clock_);

  queue->SetUnderflowReporter([this](zx::duration underflow_duration) {
    auto now = zx::clock::get_monotonic();
    reporter_->PacketQueueUnderflow(now - underflow_duration, now);
  });
  auto stream_usage = usage();
  FX_DCHECK(stream_usage) << "A renderer cannot be linked without a usage";
  queue->set_usage(*stream_usage);
  packet_queues_.insert({&dest, queue});
  return fpromise::ok(std::move(queue));
}

void BaseRenderer::CleanupDestLink(const AudioObject& dest) {
  TRACE_DURATION("audio", "BaseRenderer::CleanupDestLink");
  auto it = packet_queues_.find(&dest);
  FX_DCHECK(it != packet_queues_.end());
  auto queue = std::move(it->second);
  packet_queues_.erase(it);

  // Flush this queue to:
  //
  //   1) Ensure we release any packet references in order.
  //   2) Hold a reference to self until the flush has completed. This is needed because the packets
  //      in the queue are allocated using a SlabAllocated owned by us, so we ensure we outlive
  //      our packets.
  //
  // It's okay to release the reference to |queue| since either the Flush will have completed
  // synchronously, or otherwise the mix job will hold a strong reference to the queue and perform
  // the flush at the end of the mix job when the packet queue buffers are unlocked.
  queue->Flush(PendingFlushToken::Create(context_.threading_model().FidlDomain().dispatcher(),
                                         [self = shared_from_this()] {}));
}

void BaseRenderer::RecomputeMinLeadTime() {
  TRACE_DURATION("audio", "BaseRenderer::RecomputeMinLeadTime");
  zx::duration cur_lead_time;
  for (const auto& [_, packet_queue] : packet_queues_) {
    cur_lead_time = std::max(cur_lead_time, packet_queue->GetPresentationDelay());
  }

  if constexpr (kLogPresentationDelay) {
    FX_LOGS(INFO) << "    (" << this << ") " << __FUNCTION__ << " calculated "
                  << cur_lead_time.to_nsecs() << " ns";
  }

  if (min_lead_time_ != cur_lead_time) {
    if (!min_lead_time_reported_) {
      reporter_->SetInitialMinLeadTime(cur_lead_time);
      min_lead_time_reported_ = true;
    } else {
      reporter_->UpdateMinLeadTime(cur_lead_time, zx::clock::get_monotonic());
    }
    min_lead_time_ = cur_lead_time;
    ReportNewMinLeadTime();
  }
}

// IsOperating is true any time we have any packets in flight. Configuration functions cannot be
// called any time we are operational.
bool BaseRenderer::IsOperating() {
  TRACE_DURATION("audio", "BaseRenderer::IsOperating");

  // If any packet queue is not empty, then this link _is_ operating.
  return std::ranges::any_of(packet_queues_,
                             [](const auto& pair) { return !pair.second->empty(); });
}

bool BaseRenderer::ValidateConfig() {
  TRACE_DURATION("audio", "BaseRenderer::ValidateConfig");
  if (config_validated_) {
    return true;
  }

  if (!format_valid() || payload_buffers_.empty()) {
    return false;
  }

  // Compute the number of fractional frames per PTS tick.
  Fixed frac_fps(format()->stream_type().frames_per_second);
  frac_frames_per_pts_tick_ =
      TimelineRate::Product(pts_ticks_per_second_.Inverse(), TimelineRate(frac_fps.raw_value(), 1));

  // Compute the PTS continuity threshold expressed in fractional input frames.
  if (!pts_continuity_threshold_set_) {
    // The user has not explicitly set a continuity threshold. Default to 1/2
    // of a PTS tick expressed in fractional input frames, rounded up.
    pts_continuity_threshold_frac_frame_ =
        Fixed::FromRaw((frac_frames_per_pts_tick_.Scale(1) + 1) >> 1);
  } else {
    pts_continuity_threshold_frac_frame_ =
        Fixed::FromRaw(static_cast<int64_t>(static_cast<double>(frac_fps.raw_value()) *
                                            static_cast<double>(pts_continuity_threshold_)));
  }

  FX_LOGS(DEBUG) << " threshold_set_: " << pts_continuity_threshold_set_
                 << ", thres_frac_frame_: " << ffl::String::DecRational
                 << pts_continuity_threshold_frac_frame_;

  // Compute the number of fractional frames per reference clock tick.
  // Later we reconcile the actual reference clock with CLOCK_MONOTONIC
  //
  frac_frames_per_ref_tick_ = TimelineRate(frac_fps.raw_value(), 1'000'000'000u);

  // TODO(mpuryear): Precompute anything else needed here. Adding links to other
  // outputs (and selecting resampling filters) might belong here as well.

  // Initialize the WavWriter here.
  wav_writer_.Initialize(
      nullptr, format()->stream_type().sample_format,
      static_cast<uint16_t>(format()->stream_type().channels),
      format()->stream_type().frames_per_second,
      static_cast<uint16_t>((format()->bytes_per_frame() * 8) / format()->stream_type().channels));

  config_validated_ = true;
  return true;
}

void BaseRenderer::ComputePtsToFracFrames(int64_t first_pts) {
  TRACE_DURATION("audio", "BaseRenderer::ComputePtsToFracFrames");
  // We should not be calling this, if transformation is already valid.
  FX_DCHECK(!pts_to_frac_frames_valid_);

  pts_to_frac_frames_ =
      TimelineFunction(next_frac_frame_pts_.raw_value(), first_pts, frac_frames_per_pts_tick_);
  pts_to_frac_frames_valid_ = true;

  FX_LOGS(DEBUG) << " (" << first_pts << ") => stime:" << pts_to_frac_frames_.subject_time()
                 << ", rtime:" << pts_to_frac_frames_.reference_time()
                 << ", sdelta:" << pts_to_frac_frames_.subject_delta()
                 << ", rdelta:" << pts_to_frac_frames_.reference_delta();
}

void BaseRenderer::AddPayloadBuffer(uint32_t id, zx::vmo payload_buffer) {
  AddPayloadBufferInternal(id, std::move(payload_buffer));
}

void BaseRenderer::AddPayloadBufferInternal(uint32_t id, zx::vmo payload_buffer) {
  TRACE_DURATION("audio", "BaseRenderer::AddPayloadBuffer");
  auto cleanup = fit::defer([this]() { context_.route_graph().RemoveRenderer(*this); });

  FX_LOGS(DEBUG) << " (id: " << id << ")";

  // TODO(https://fxbug.dev/42086172): Lift this restriction.
  if (IsOperating()) {
    FX_LOGS(ERROR) << "Attempted to set payload buffer while in operational mode.";
    return;
  }

  auto vmo_mapper = fbl::MakeRefCounted<RefCountedVmoMapper>();
  if (!payload_buffers_.emplace(id, vmo_mapper).second) {
    FX_LOGS(ERROR) << "Duplicate payload buffer id: " << id;
    return;
  }

  zx_status_t status;
  zx_info_vmo_t info;
  status = payload_buffer.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr);
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to get payload buffer info";
    return;
  }

  // Resizable VMOs are disallowed because we map the VMO: shrinking the VMO size can cause a crash.
  if ((info.flags & ZX_INFO_VMO_RESIZABLE) != 0) {
    FX_LOGS(ERROR) << "Resizable payload buffers not supported";
    return;
  }

  // Pager-backed VMOs are disallowed because page faults on audio threads can stall or terminate
  // audio_core.
  if ((info.flags & ZX_INFO_VMO_PAGER_BACKED) != 0) {
    FX_LOGS(ERROR) << "Pager-backed payload buffers not supported";
    return;
  }

  // If the VMO is discardable, lock it to ensure the pages are not reclaimed until they are
  // unmapped from this process.
  if ((info.flags & ZX_INFO_VMO_DISCARDABLE) != 0) {
    zx_vmo_lock_state_t ls;
    status = payload_buffer.op_range(ZX_VMO_OP_LOCK, 0, info.size_bytes, &ls, sizeof(ls));
    if (status != ZX_OK) {
      FX_PLOGS(ERROR, status) << "Failed to lock payload buffer";
      return;
    }
    if (ls.discarded_size) {
      FX_LOGS(INFO) << "Mapping discardable buffer: " << ls.discarded_size << "/" << ls.size
                    << " bytes were previously discarded";
    }
  }

  // ZX_VM_ALLOW_FAULTS is required to support discardable VMOs.
  status = vmo_mapper->Map(payload_buffer, 0, 0,
                           ZX_VM_PERM_READ | ZX_VM_ALLOW_FAULTS | ZX_VM_MAP_RANGE, context_.vmar());
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to map payload buffer";
    return;
  }

  // Locking a discardable VMO ensures the mappings won't be discarded, but doesn't eagerly map the
  // pages. If a buggy client sends us a discardable VMO, whose contents have been discarded, and
  // that client doesn't write to or COMMIT the VMO, then there won't be any pages allocated to the
  // VMO. By reading from each page, we ensure that pages are allocated for the entire VMAR, which
  // ensures we won't take a page fault later when reading from these pages.
  if ((info.flags & ZX_INFO_VMO_DISCARDABLE) != 0) {
    const auto page_size = zx_system_get_page_size();
    for (size_t offset = 0; offset < info.size_bytes; offset += page_size) {
      // Use a volatile pointer to ensure the load is not optimized out.
      auto p = static_cast<const volatile char*>(vmo_mapper->start()) + offset;
      *p;
    }
  }

  reporter_->AddPayloadBuffer(id, vmo_mapper->size());

  // Things went well, cancel the cleanup hook. If our config had been validated previously, it will
  // have to be revalidated as we move into the operational phase of our life.
  InvalidateConfiguration();
  cleanup.cancel();
}

void BaseRenderer::RemovePayloadBuffer(uint32_t id) { RemovePayloadBufferInternal(id); }

void BaseRenderer::RemovePayloadBufferInternal(uint32_t id) {
  TRACE_DURATION("audio", "BaseRenderer::RemovePayloadBuffer");
  auto cleanup = fit::defer([this]() { context_.route_graph().RemoveRenderer(*this); });

  FX_LOGS(DEBUG) << " (id: " << id << ")";

  // TODO(https://fxbug.dev/42086172): Lift this restriction.
  if (IsOperating()) {
    FX_LOGS(ERROR) << "Attempted to remove payload buffer while in the operational mode.";
    return;
  }

  if (payload_buffers_.erase(id) != 1) {
    FX_LOGS(ERROR) << "Invalid payload buffer id";
    return;
  }

  reporter_->RemovePayloadBuffer(id);
  cleanup.cancel();
}

void BaseRenderer::SetPtsUnits(uint32_t tick_per_second_numerator,
                               uint32_t tick_per_second_denominator) {
  TRACE_DURATION("audio", "BaseRenderer::SetPtsUnits");
  auto cleanup = fit::defer([this]() { context_.route_graph().RemoveRenderer(*this); });

  FX_LOGS(DEBUG) << "PTS ticks per sec: " << tick_per_second_numerator << " / "
                 << tick_per_second_denominator;

  if (IsOperating()) {
    FX_LOGS(ERROR) << "PTS ticks per second cannot be set while in operational mode.";
    return;
  }

  if (!tick_per_second_numerator || !tick_per_second_denominator) {
    FX_LOGS(ERROR) << "Bad PTS ticks per second (" << tick_per_second_numerator << "/"
                   << tick_per_second_denominator
                   << "): both numerator and denominator must be non-zero";
    return;
  }

  auto pts_ticks_per_sec = TimelineRate(tick_per_second_numerator, tick_per_second_denominator);

  // Sanity checks to ensure that Scale() operations cannot overflow.
  // Must have at most 1 tick per nanosecond. Ticks should not have higher resolution than clocks.
  static_assert(TimelineRate::kOverflow > 1'000'000'000,
                "AudioCore logic expects TimelineRate::kOverflow to exceed 10^9");
  if (auto t = pts_ticks_per_sec.Scale(1, TimelineRate::RoundingMode::Ceiling);
      t > 1'000'000'000  // includes 't == TimelineRate::kOverflow' check
  ) {
    FX_LOGS(ERROR) << "PTS ticks per second too high (" << tick_per_second_numerator << "/"
                   << tick_per_second_denominator << ")";
    return;
  }
  // Must have at least 1 tick per minute. This limit is somewhat arbitrary. We need *some* limit
  // here and we expect this is way more headroom than will be needed in practice.
  if (auto t = pts_ticks_per_sec.Scale(60); t == 0) {
    FX_LOGS(ERROR) << "PTS ticks per second too low (" << tick_per_second_numerator << "/"
                   << tick_per_second_denominator << ")";
    return;
  }

  reporter_->SetPtsUnits(tick_per_second_numerator, tick_per_second_denominator);

  pts_ticks_per_second_ = pts_ticks_per_sec;

  // Things went well, cancel the cleanup hook. If our config had been validated previously, it will
  // have to be revalidated as we move into the operational phase of our life.
  InvalidateConfiguration();
  cleanup.cancel();
}

void BaseRenderer::SetPtsContinuityThreshold(float threshold_seconds) {
  TRACE_DURATION("audio", "BaseRenderer::SetPtsContinuityThreshold");
  auto cleanup = fit::defer([this]() { context_.route_graph().RemoveRenderer(*this); });

  FX_LOGS(DEBUG) << "PTS continuity threshold: " << threshold_seconds << " sec";

  if (IsOperating()) {
    FX_LOGS(ERROR) << "PTS continuity threshold cannot be set while in operational mode.";
    return;
  }

  if (!isnormal(threshold_seconds) && threshold_seconds != 0.0) {
    FX_LOGS(ERROR) << "PTS continuity threshold (" << threshold_seconds << ") must be normal or 0";
    return;
  }

  if (threshold_seconds < 0.0) {
    FX_LOGS(ERROR) << "PTS continuity threshold (" << threshold_seconds << ") cannot be negative";
    return;
  }

  reporter_->SetPtsContinuityThreshold(threshold_seconds);

  pts_continuity_threshold_ = threshold_seconds;
  pts_continuity_threshold_set_ = true;

  // Things went well, cancel the cleanup hook. If our config had been validated previously, it will
  // have to be revalidated as we move into the operational phase of our life.
  InvalidateConfiguration();
  cleanup.cancel();
}

// This method returns true if the entire packet contents are digital silence. The checker enforces
// this only to the extent supported by the sample format. For samples in the SIGNED_24_IN_32
// format, it ignores the contents of the padding byte. For FLOAT samples, it considers the range
// [-numeric_limits<float>::epsilon(), numeric_limits<float>::epsilon()] as equivalent to 0.0f.
//
// Only ever called if tracing is started and the specific tracing category "audio" is enabled.
bool BaseRenderer::CheckForSilentPacket(const void* packet_buffer, int64_t frame_count) {
  switch (format()->stream_type().sample_format) {
    case fuchsia::media::AudioSampleFormat::FLOAT:
      return SilentPacketChecker::IsSilenceFloat32(packet_buffer,
                                                   frame_count * format()->stream_type().channels);
    case fuchsia::media::AudioSampleFormat::SIGNED_24_IN_32:
      return SilentPacketChecker::IsSilenceInt24In32(
          packet_buffer, frame_count * format()->stream_type().channels);
    case fuchsia::media::AudioSampleFormat::SIGNED_16:
      return SilentPacketChecker::IsSilenceInt16(packet_buffer,
                                                 frame_count * format()->stream_type().channels);
    case fuchsia::media::AudioSampleFormat::UNSIGNED_8:
      return SilentPacketChecker::IsSilenceUint8(packet_buffer,
                                                 frame_count * format()->stream_type().channels);
  }
}

void BaseRenderer::SendPacket(fuchsia::media::StreamPacket packet, SendPacketCallback callback) {
  SendPacketInternal(packet, std::move(callback));
}

void BaseRenderer::SendPacketInternal(fuchsia::media::StreamPacket packet,
                                      SendPacketCallback callback) {
  TRACE_DURATION("audio", "BaseRenderer::SendPacket", "pts", packet.pts);
  auto cleanup = fit::defer([this]() { context_.route_graph().RemoveRenderer(*this); });

  // It is an error to attempt to send a packet before we have established at least a minimum valid
  // configuration. IOW - the format must have been configured, and we must have an established
  // payload buffer.
  if (!ValidateConfig()) {
    FX_LOGS(ERROR) << "Failed to validate configuration during SendPacket";
    return;
  }

  // Lookup our payload buffer.
  auto it = payload_buffers_.find(packet.payload_buffer_id);
  if (it == payload_buffers_.end()) {
    FX_LOGS(ERROR) << "Invalid payload_buffer_id (" << packet.payload_buffer_id << ")";
    return;
  }
  auto payload_buffer = it->second;

  // Start by making sure that the region we are receiving is made from an integral number of audio
  // frames. Count the total number of frames in the process.
  uint32_t frame_size = format()->bytes_per_frame();
  FX_DCHECK(frame_size != 0);
  if (packet.payload_size % frame_size) {
    FX_LOGS(ERROR) << "Region length (" << packet.payload_size
                   << ") is not divisible by by audio frame size (" << frame_size << ")";
    return;
  }

  // Make sure that we don't exceed the maximum permissible frames-per-packet.
  int64_t frame_count = static_cast<int64_t>(packet.payload_size / frame_size);
  if (frame_count > fuchsia::media::MAX_FRAMES_PER_RENDERER_PACKET) {
    FX_LOGS(ERROR) << "Audio frame count (" << frame_count << ") exceeds maximum allowed ("
                   << fuchsia::media::MAX_FRAMES_PER_RENDERER_PACKET << ")";
    return;
  }

  // Make sure that the packet offset/size exists entirely within the payload buffer.
  FX_DCHECK(payload_buffer != nullptr);
  uint64_t start = packet.payload_offset;
  uint64_t end = start + packet.payload_size;
  uint64_t pb_size = payload_buffer->size();
  if ((start >= pb_size) || (end > pb_size)) {
    FX_LOGS(ERROR) << "Bad packet range [" << start << ", " << end << "). Payload buffer size is "
                   << pb_size;
    return;
  }

  reporter_->SendPacket(packet);

  // Compute the PTS values for this packet applying our interpolation and continuity thresholds as
  // we go. Start by checking to see if this our PTS to frames transformation needs to be computed
  // (this should be needed after startup, and after each flush operation).
  if (!pts_to_frac_frames_valid_) {
    ComputePtsToFracFrames((packet.pts == fuchsia::media::NO_TIMESTAMP) ? 0 : packet.pts);
  }

  // We need to translate this packet's PTS into a fractional input frame value. If the packet did
  // not contain a PTS, we will first need to determine that.
  Fixed start_pts;
  Fixed packet_ffpts{0};
  if (packet.pts == fuchsia::media::NO_TIMESTAMP) {
    // Packet has no timestamp. Use the next expected PTS: the end of the previous packet.
    start_pts = next_frac_frame_pts_;

    auto ref_now = clock_->now();
    zx::time deadline = ref_now + min_lead_time_;
    auto first_valid_frame =
        Fixed::FromRaw(reference_clock_to_fractional_frames_->Apply(deadline.get()));

    // Our packet has NO_TIMESTAMP. If it ALSO has STREAM_PACKET_FLAG_DISCONTINUITY, then we will
    // "nudge" it forward in time if needed, so that the entire packet can safely play. If it does
    // NOT have this flag, then we will place it continuous with the end of the previous packet.
    if (packet.flags & fuchsia::media::STREAM_PACKET_FLAG_DISCONTINUITY) {
      // We reset frames_received_ on a DISCONTINUITY packet, whether or not we "nudge" the pkt.
      frames_received_ = 0;

      if (start_pts < first_valid_frame) {
        // We received this NO_TIMESTAMP packet too late to safely play it on time, when considering
        // min_lead_time. The packet sets FLAG_DISCONTINUITY, so we nudge it forward by a "safety"
        // duration: not only up to the exact "safe" threshold, but adding a "padding" duration.
        zx::time padded_start_ref = deadline + kPaddingForUnspecifiedRefTime;
        auto padded_start_pts =
            Fixed::FromRaw(reference_clock_to_fractional_frames_->Apply(padded_start_ref.get()));
        TRACE_INSTANT("audio", "Renderer-DiscontinuityNudge", TRACE_SCOPE_THREAD,
                      "prev_packet_end_frac", next_frac_frame_pts_.raw_value(), "safety_frac",
                      (first_valid_frame.raw_value() - next_frac_frame_pts_.raw_value()),
                      "safety_ref_ns",
                      frac_frames_per_ref_tick_.Inverse().Scale(first_valid_frame.raw_value() -
                                                                next_frac_frame_pts_.raw_value()),
                      "padded_start_frac", start_pts.raw_value(), "padded_start_ref_ns",
                      padded_start_ref.get());

        FX_LOGS(TRACE) << "RENDER CONTINUITY UNDERFLOW is prevented: FLAG_DISCONTINUITY is set. "
                       << "prev packet ended at " << ffl::String::DecRational << start_pts
                       << ", but first safe PTS for this NO_TIMESTAMP packet is "
                       << first_valid_frame << " (ref_time " << deadline.get() << ") -- late by "
                       << Fixed(first_valid_frame - start_pts)
                       << ". After 'nudging' by safety factor, pkt starts at " << padded_start_pts
                       << " (ref_time " << padded_start_ref.get() << ").";
        start_pts = padded_start_pts;
      } else {
        // Packet sets FLAG_DISCONTINUITY, but it can play continuously (no nudge is needed).
        auto safety_interval_frac = Fixed(start_pts - first_valid_frame);
        zx::duration safety_internal_ref =
            zx::nsec(frac_frames_per_ref_tick_.Inverse().Scale(safety_interval_frac.raw_value()));
        TRACE_INSTANT("audio", "Renderer-DiscontinuityNoNudge", TRACE_SCOPE_THREAD,
                      "prev_packet_end_frac", start_pts.raw_value(), "safety_interval_frac",
                      safety_interval_frac.raw_value(), "safety_interval_ref_ns",
                      safety_internal_ref.get(), "retained_start_ref_ns",
                      (deadline - safety_internal_ref).get());

        FX_LOGS(TRACE)
            << "RENDER CONTINUITY: NO_TS packet set FLAG_DISCONTINUITY but is in time (early by "
            << ffl::String::DecRational << safety_interval_frac << " frames: ref_time "
            << safety_internal_ref.get() << " ns); applying a safe PTS of " << start_pts
            << " (ref_time " << deadline.get() - safety_internal_ref.get() << ").";
      }
    } else if (start_pts < first_valid_frame) {
      // NO_TIMESTAMP packet is late without STREAM_PACKET_FLAG_DISCONTINUITY. This represents a
      // break in continuity, and data will be lost. Report this to those who are concerned.
      // For description of "Render Continuity Underflows", see logging_flags.h.
      ReportContinuityUnderflow(start_pts, first_valid_frame, deadline);
    }
  } else {
    // The packet has an explicit PTS. Translate to frames and apply continuity threshold rules.
    packet_ffpts = Fixed::FromRaw(pts_to_frac_frames_.Apply(packet.pts));
    Fixed delta = packet_ffpts - next_frac_frame_pts_;
    Fixed abs_delta = delta.Absolute();

    // As a starting assumption, we expect this timestamp to be continuous with the previous packet.
    // I.e. if not, we expect any overlap/gap to be smaller than our continuity threshold.
    start_pts = next_frac_frame_pts_;

    if (delta < 0) {
      if (abs_delta >= pts_continuity_threshold_frac_frame_) {
        // Timestamped packet overlaps with previous packet by the continuity threshold or more, and
        // FLAG_DISCONTINUITY is not set. Overlapping frames in this packet will be lost. Report
        // this to various parties (Inspect, Tracing, SysLog, perhaps Cobalt) who care.
        // For description of "Render Timestamp Underflows", see logging_flags.h.
        ReportTimestampUnderflow(packet_ffpts, next_frac_frame_pts_);
        start_pts = packet_ffpts;  // Use the explicitly-stated packet PTS.
      } else {
        // Timestamped packet overlapped with previous packet, but by less than the continuity
        // threshold. We will adjust its timing ("nudge" it) to play continuously (no data loss).
        TRACE_INSTANT("audio", "Renderer-TimestampOverlapNudge", TRACE_SCOPE_THREAD,
                      "prev_packet_end_frac", next_frac_frame_pts_.raw_value(), "actual_pts_frac",
                      packet_ffpts.raw_value(), "threshold_frac",
                      pts_continuity_threshold_frac_frame_.raw_value(), "threshold_ref_ns",
                      static_cast<int32_t>(pts_continuity_threshold_ * 1'000'000'000.0),
                      "actual_overlap_frac", delta.raw_value(), "actual_overlap_ref_ns",
                      frac_frames_per_ref_tick_.Inverse().Scale(delta.raw_value()));

        FX_LOGS(TRACE) << "RENDER TIMESTAMP UNDERFLOW is prevented: overlap ("
                       << ffl::String::DecRational << abs_delta
                       << " frames) is less than discontinuity threshold ("
                       << pts_continuity_threshold_frac_frame_ << " frames)";
      }
    } else if (delta > 0) {
      if (abs_delta >= pts_continuity_threshold_frac_frame_) {
        // Gap exists between end of previous packet and this timestamped packet. This gap equals/
        // exceeds the continuity threshold, and thus the packet's timing will NOT be adjusted
        // (restated: we will insert silence between previous-packet-end and this-packet-start).
        TRACE_INSTANT("audio", "Renderer-TimestampGapNoNudge", TRACE_SCOPE_THREAD,
                      "prev_packet_end_frac", next_frac_frame_pts_.raw_value(), "actual_pts_frac",
                      packet_ffpts.raw_value(), "threshold_frac",
                      pts_continuity_threshold_frac_frame_.raw_value(), "threshold_ref_ns",
                      static_cast<int32_t>(pts_continuity_threshold_ * 1'000'000'000.0),
                      "actual_overlap_frac", delta.raw_value(), "actual_overlap_ref_ns",
                      frac_frames_per_ref_tick_.Inverse().Scale(delta.raw_value()));

        // Some clients do this intentionally, and 20+ times per second! ("sparse packets")
        // If changing this to >= INFO, consider wrapping the logging with a conditional such as:
        //   `if (usage()->render_usage() != RenderUsage::ULTRASOUND) { FX_LOGS... }`
        FX_LOGS(TRACE) << "RENDER TIMESTAMP OVERFLOW: gap (" << ffl::String::DecRational
                       << abs_delta << " frames) exceeds/equals discontinuity threshold ("
                       << pts_continuity_threshold_frac_frame_ << ")";
        start_pts = packet_ffpts;  // Use the explicitly-stated packet PTS.
      } else {
        // The gap between end of previous packet and this timestamped packet is less than the
        // continuity threshold, so this packet's timing will be adjusted to make it continuous.
        TRACE_INSTANT("audio", "Renderer-TimestampGapNudge", TRACE_SCOPE_THREAD,
                      "prev_packet_end_frac", next_frac_frame_pts_.raw_value(), "actual_pts_frac",
                      packet_ffpts.raw_value(), "threshold_frac",
                      pts_continuity_threshold_frac_frame_.raw_value(), "threshold_ref_ns",
                      static_cast<int32_t>(pts_continuity_threshold_ * 1'000'000'000.0),
                      "actual_overlap_frac", delta.raw_value(), "actual_overlap_ref_ns",
                      frac_frames_per_ref_tick_.Inverse().Scale(delta.raw_value()));

        FX_LOGS(TRACE) << "RENDER TIMESTAMP OVERFLOW is prevented: gap ("
                       << ffl::String::DecRational << abs_delta
                       << " frames) is less than discontinuity threshold ("
                       << pts_continuity_threshold_frac_frame_ << ")";
      }
      // Otherwise (delta == 0), the packet PTS is perfectly continuous.
    }
  }
  frames_received_ += frame_count;

  FX_DCHECK(packet.payload_offset / frame_size <= std::numeric_limits<uint32_t>::max());
  uint32_t frame_offset = static_cast<uint32_t>(packet.payload_offset / frame_size);
  FX_LOGS(TRACE) << " [pkt " << ffl::String::DecRational << packet_ffpts << ", now "
                 << next_frac_frame_pts_ << "] => " << start_pts << " - "
                 << Fixed(start_pts + Fixed::FromRaw(pts_to_frac_frames_.Apply(frame_count)))
                 << ", offset " << Fixed::FromRaw(pts_to_frac_frames_.Apply(frame_offset));

  auto packet_buff = reinterpret_cast<uint8_t*>(payload_buffer->start()) + packet.payload_offset;
  if (TRACE_CATEGORY_ENABLED("audio")) {
    if (CheckForSilentPacket(reinterpret_cast<const void*>(packet_buff), frame_count)) {
      TRACE_INSTANT("audio", "Renderer-SilentPacket", TRACE_SCOPE_THREAD, "packet_start_frac",
                    start_pts.raw_value(), "frame_count", frame_count, "channel_count",
                    format()->channels(), "bytes_per_sample", format()->bytes_per_sample(),
                    "sample_format_ordinal", fidl::ToUnderlying(format()->sample_format()));
    }
  }

  // Regardless of timing, capture this data to file.
  FX_DCHECK(packet.payload_size <= std::numeric_limits<uint32_t>::max());
  wav_writer_.Write(packet_buff, static_cast<uint32_t>(packet.payload_size));
  wav_writer_.UpdateHeader();

  // Snap the starting pts to an input frame boundary.
  //
  // TODO(https://fxbug.dev/42083626): Don't do this. If a user wants to write an explicit timestamp
  // on a source packet which schedules the packet to start at a fractional position on the source
  // time line, we should probably permit this. We need to make sure that the mixer cores are ready
  // to handle this case before proceeding, however.
  start_pts = Fixed(start_pts.Floor());

  // Create the packet.
  auto packet_ref = packet_allocator_.New(
      payload_buffer, packet.payload_offset, frame_count, start_pts,
      context_.threading_model().FidlDomain().dispatcher(), std::move(callback));
  if (!packet_ref) {
    FX_LOGS(ERROR) << "Client created too many concurrent Packets; Allocator has created "
                   << packet_allocator_.obj_count() << " / " << packet_allocator_.max_obj_count()
                   << " max allocations";
    return;
  }

  // The end pts is the value we will use for the next packet's start PTS, if the user does not
  // provide an explicit PTS.
  next_frac_frame_pts_ = packet_ref->end();

  // Distribute our packet to all our dest links
  for (auto& [_, packet_queue] : packet_queues_) {
    packet_queue->PushPacket(packet_ref);
  }

  // Things went well, cancel the cleanup hook.
  cleanup.cancel();
}

// This NO_TIMESTAMP packet is too late to safely play continuously; expect a dropout.
// For description of "Render Continuity Underflows", see logging_flags.h.
void BaseRenderer::ReportContinuityUnderflow(Fixed implied_pts, Fixed first_safe_pts,
                                             zx::time first_safe_ref) {
  zx::duration duration_late{frac_frames_per_ref_tick_.Inverse().Scale(first_safe_pts.raw_value() -
                                                                       implied_pts.raw_value())};
  TRACE_INSTANT("audio", "RendererContinuityUnderflow", TRACE_SCOPE_THREAD, "implied_start_frac",
                implied_pts.raw_value(), "late_by_frac",
                (first_safe_pts.raw_value() - implied_pts.raw_value()), "late_by_ref_ns",
                duration_late.get(), "first_safe_frac", first_safe_pts.raw_value(),
                "first_safe_ref_ns", first_safe_ref.get());
  continuity_underflow_count_++;

  reporter_->ContinuityUnderflow(first_safe_ref - duration_late, first_safe_ref);

#define LOG_CONTINUITY_UNDERFLOW(where, interval)                                               \
  FX_LOGS(where) << "RENDER CONTINUITY UNDERFLOW #" << continuity_underflow_count_ << " (1/"    \
                 << (interval) << "): prev packet ended at " << ffl::String::DecRational        \
                 << implied_pts << ", but first safe PTS for this NO_TIMESTAMP packet is "      \
                 << first_safe_pts << " (ref_time " << first_safe_ref.get()                     \
                 << "). Untimestamped packet arrived late by " << continuity_gap << " frames (" \
                 << continuity_gap_ref_ns << " ns)"

  if constexpr (kLogRendererUnderflow) {
    Fixed continuity_gap = first_safe_pts - implied_pts;
    auto continuity_gap_ref_ns =
        frac_frames_per_ref_tick_.Inverse().Scale(Fixed(first_safe_pts - implied_pts).raw_value());

    if (kRendererContinuityUnderflowWarningInterval > 0 &&
        (continuity_underflow_count_ - 1) % kRendererContinuityUnderflowWarningInterval == 0) {
      LOG_CONTINUITY_UNDERFLOW(WARNING, kRendererContinuityUnderflowWarningInterval);
    } else if (kRendererContinuityUnderflowInfoInterval > 0 &&
               (continuity_underflow_count_ - 1) % kRendererContinuityUnderflowInfoInterval == 0) {
      LOG_CONTINUITY_UNDERFLOW(INFO, kRendererContinuityUnderflowInfoInterval);
    } else {
      LOG_CONTINUITY_UNDERFLOW(TRACE, 1);
    }
  }
#undef LOG_CONTINUITY_UNDERFLOW
}

// This timestamped packet is too late to play without payload truncation; expect a dropout.
// For description of "Render Timestamp Underflows", see logging_flags.h.
void BaseRenderer::ReportTimestampUnderflow(Fixed packet_pts, Fixed prev_packet_end_pts) {
  zx::duration duration_late{frac_frames_per_ref_tick_.Inverse().Scale(
      prev_packet_end_pts.raw_value() - packet_pts.raw_value())};
  TRACE_INSTANT("audio", "RendererTimestampUnderflow", TRACE_SCOPE_THREAD, "implied_start_frac",
                packet_pts.raw_value(), "late_by_frac",
                (prev_packet_end_pts.raw_value() - packet_pts.raw_value()), "late_by_ref_ns",
                duration_late.get(), "first_safe_frac", prev_packet_end_pts.raw_value());
  timestamp_underflow_count_++;

  auto now = zx::clock::get_monotonic();
  reporter_->TimestampUnderflow(now - duration_late, now);

#define LOG_TIMESTAMP_UNDERFLOW(where, interval)                                                \
  FX_LOGS(where) << "RENDER TIMESTAMP UNDERFLOW #" << timestamp_underflow_count_ << " (1/"      \
                 << (interval) << "): prev packet ended at " << ffl::String::DecRational        \
                 << prev_packet_end_pts << ", but PTS for this packet is " << packet_pts        \
                 << ". Packet overlaps with previous packet by "                                \
                 << Fixed(prev_packet_end_pts - packet_pts) << " frames ("                      \
                 << frac_frames_per_ref_tick_.Inverse().Scale(prev_packet_end_pts.raw_value() - \
                                                              packet_pts.raw_value())           \
                 << " ns)"

  if constexpr (kLogRendererUnderflow) {
    if (kRendererTimestampUnderflowWarningInterval > 0 &&
        (timestamp_underflow_count_ - 1) % kRendererTimestampUnderflowWarningInterval == 0) {
      LOG_TIMESTAMP_UNDERFLOW(WARNING, kRendererTimestampUnderflowWarningInterval);
    } else if (kRendererTimestampUnderflowInfoInterval > 0 &&
               (timestamp_underflow_count_ - 1) % kRendererTimestampUnderflowInfoInterval == 0) {
      LOG_TIMESTAMP_UNDERFLOW(INFO, kRendererTimestampUnderflowInfoInterval);
    } else {
      LOG_TIMESTAMP_UNDERFLOW(TRACE, 1);
    }
  }
#undef LOG_TIMESTAMP_UNDERFLOW
}

void BaseRenderer::SendPacketNoReply(fuchsia::media::StreamPacket packet) {
  TRACE_DURATION("audio", "BaseRenderer::SendPacketNoReply");
  SendPacket(packet, nullptr);
}

void BaseRenderer::EndOfStream() {
  TRACE_DURATION("audio", "BaseRenderer::EndOfStream");

  // Today we do nothing, but in the future this could be used by clients to indicate intentional
  // gaps in a sequence of packets.
}

void BaseRenderer::DiscardAllPackets(DiscardAllPacketsCallback callback) {
  DiscardAllPacketsInternal(std::move(callback));
}

void BaseRenderer::DiscardAllPacketsInternal(DiscardAllPacketsCallback callback) {
  TRACE_DURATION("audio", "BaseRenderer::DiscardAllPackets");

  // If the user has requested a callback, create the flush token we will use to invoke the callback
  // at the proper time.
  fbl::RefPtr<PendingFlushToken> flush_token;
  if (callback != nullptr) {
    flush_token = PendingFlushToken::Create(context_.threading_model().FidlDomain().dispatcher(),
                                            std::move(callback));
  }

  // Tell each link to flush. If link is currently processing pending data, it will take a reference
  // to the flush token and ensure a callback is queued at the proper time (after all pending
  // packet-complete callbacks are queued).
  for (auto& [_, packet_queue] : packet_queues_) {
    packet_queue->Flush(flush_token);
  }
  frames_received_ = 0;
}

void BaseRenderer::DiscardAllPacketsNoReply() {
  TRACE_DURATION("audio", "BaseRenderer::DiscardAllPacketsNoReply");
  DiscardAllPackets(nullptr);
}

void BaseRenderer::Play(int64_t reference_time, int64_t media_time, PlayCallback callback) {
  PlayInternal(zx::time(reference_time), zx::time(media_time), std::move(callback));
}

void BaseRenderer::PlayInternal(zx::time reference_time, zx::time media_time,
                                PlayCallback callback) {
  TRACE_DURATION("audio", "BaseRenderer::Play");
  FX_LOGS(DEBUG) << "Request (ref: "
                 << (reference_time.get() == fuchsia::media::NO_TIMESTAMP ? -1
                                                                          : reference_time.get())
                 << ", media: "
                 << (media_time.get() == fuchsia::media::NO_TIMESTAMP ? -1 : media_time.get())
                 << ")";

  auto cleanup = fit::defer([this]() { context_.route_graph().RemoveRenderer(*this); });

  if (!ValidateConfig()) {
    FX_LOGS(ERROR) << "Failed to validate configuration during Play";
    return;
  }

  // Ensure we have enough headroom so that a renderer can play continuously for one year.
  constexpr zx::duration kMaxRendererDuration = zx::hour(24) * 365;
  const auto kMaxRendererFrames =
      Fixed::FromRaw(format()->frames_per_ns().Scale(kMaxRendererDuration.get()));

  auto over_or_underflow = [](int64_t x) {
    return x == TimelineRate::kOverflow || x == TimelineRate::kUnderflow;
  };

  auto timeline_function_overflows = [over_or_underflow](const TimelineFunction& f, int64_t t,
                                                         int64_t max_duration) {
    // Check if we overflow when applying this function or its inverse.
    auto x = f.Apply(t);
    if (over_or_underflow(x) || over_or_underflow(f.ApplyInverse(x))) {
      return true;
    }

    // Check if we have enough headroom for max_duration time steps.
    if (__builtin_add_overflow(t, max_duration, &x)) {
      return true;
    }
    x = f.Apply(t + max_duration);
    return (over_or_underflow(x) || over_or_underflow(f.ApplyInverse(x)));
  };

  // TODO(mpuryear): What do we want to do here if we are already playing?

  // Did the user supply a reference time? If not, figure out a safe starting time based on the
  // outputs we are currently linked to.
  if (reference_time.get() == fuchsia::media::NO_TIMESTAMP) {
    // TODO(mpuryear): How much more than the minimum clock lead time do we want to pad this by?
    // Also, if/when lead time requirements change, do we want to introduce a discontinuity?
    //
    // We could consider an explicit mode (make it default) where timing across outputs is treated
    // as "loose". Specifically, make no effort to account for external latency, nor to synchronize
    // streams across multiple parallel outputs. In this mode we must update lead time upon changes
    // in internal interconnect requirements, but impact should be small since internal lead time
    // factors tend to be small, while external factors can be huge.

    auto ref_now = clock_->now();
    reference_time = ref_now + min_lead_time_ + kPaddingForUnspecifiedRefTime;
  }

  // If no media time was specified, use the first pending packet's media time.
  //
  // Note: users specify the units for media time by calling SetPtsUnits(), or nanoseconds if this
  // is never called. Internally we use fractional input frames, on the timeline defined when
  // transitioning to operational mode.
  Fixed frac_frame_media_time;

  if (media_time.get() == fuchsia::media::NO_TIMESTAMP) {
    // Are we resuming from pause?
    if (pause_time_frac_frames_valid_) {
      frac_frame_media_time = pause_time_frac_frames_;
    } else {
      // TODO(mpuryear): peek the first PTS of the pending queue.
      frac_frame_media_time = Fixed(0);
    }

    // If we do not know the pts_to_frac_frames relationship yet, compute one.
    if (!pts_to_frac_frames_valid_) {
      next_frac_frame_pts_ = frac_frame_media_time;
      ComputePtsToFracFrames(0);
    }

    media_time = zx::time{pts_to_frac_frames_.ApplyInverse(frac_frame_media_time.raw_value())};
  } else {
    // If we do not know the pts_to_frac_frames relationship yet, compute one.
    if (!pts_to_frac_frames_valid_) {
      ComputePtsToFracFrames(media_time.get());
      frac_frame_media_time = next_frac_frame_pts_;
    } else {
      frac_frame_media_time = Fixed::FromRaw(pts_to_frac_frames_.Apply(media_time.get()));
    }
    // Sanity check media_time: ensure we have enough headroom to not overflow.
    if (over_or_underflow(frac_frame_media_time.raw_value()) ||
        timeline_function_overflows(pts_to_frac_frames_.Inverse(),
                                    frac_frame_media_time.raw_value(),
                                    kMaxRendererFrames.raw_value())) {
      FX_LOGS(ERROR) << "Overflow in Play: media_time too large: " << media_time.get();
      return;
    }
  }

  // Update our transformation.
  //
  // TODO(mpuryear): if we need to trigger a remix for our outputs, do it here.
  //
  auto ref_clock_to_frac_frames = TimelineFunction(frac_frame_media_time.raw_value(),
                                                   reference_time.get(), frac_frames_per_ref_tick_);
  reference_clock_to_fractional_frames_->Update(ref_clock_to_frac_frames);

  // Sanity check reference_time: ensure we have enough headroom to not overflow.
  if (timeline_function_overflows(ref_clock_to_frac_frames, reference_time.get(),
                                  kMaxRendererDuration.get())) {
    FX_LOGS(ERROR) << "Overflow in Play: reference_time too large: " << reference_time.get();
    return;
  }

  // Sanity check: ensure media_time is not so far in the past that it underflows reference_time.
  if (timeline_function_overflows(ref_clock_to_frac_frames.Inverse(),
                                  frac_frame_media_time.raw_value(),
                                  kMaxRendererFrames.raw_value())) {
    FX_LOGS(ERROR) << "Underflow in Play: media_time too small: " << media_time.get();
    return;
  }

  FX_LOGS(DEBUG) << "Actual: (ref: " << reference_time.get() << ", media: " << media_time.get()
                 << ")";
  FX_LOGS(DEBUG) << "frac_frame_media_time: " << ffl::String::DecRational << frac_frame_media_time;

  // If the user requested a callback, invoke it now.
  if (callback != nullptr) {
    if constexpr (kLogRendererPlayCalls) {
      FX_LOGS(INFO) << "BaseRenderer(" << this << ")::PlayInternal about to call Play callback";
    }
    callback(reference_time.get(), media_time.get());
  }

  ReportStartIfStopped();

  // Things went well, cancel the cleanup hook.
  cleanup.cancel();
}

void BaseRenderer::Pause(PauseCallback callback) {
  TRACE_DURATION("audio", "BaseRenderer::Pause");
  auto cleanup = fit::defer([this]() { context_.route_graph().RemoveRenderer(*this); });

  if (!ValidateConfig()) {
    FX_LOGS(ERROR) << "Failed to validate configuration during Pause";
    return;
  }

  if (IsPlaying()) {
    PauseInternal(std::move(callback));
  } else {
    FX_LOGS(WARNING) << "Renderer::Pause called when not playing";
    if (callback != nullptr) {
      if constexpr (kLogRendererPauseCalls) {
        FX_LOGS(INFO) << "BaseRenderer(" << this
                      << ")::Pause about to call Pause callback (not Playing!)";
      }
      // Return the previously-reported timestamp values, to preserve idempotency.
      if (pause_reference_time_.has_value() && pause_media_time_.has_value()) {
        callback(pause_reference_time_->get(), pause_media_time_->get());
      } else {
        callback(fuchsia::media::NO_TIMESTAMP, fuchsia::media::NO_TIMESTAMP);
      }
    }
  }

  // Things went well, cancel the cleanup hook.
  cleanup.cancel();
}

void BaseRenderer::PauseInternal(PauseCallback callback) {
  TRACE_DURATION("audio", "BaseRenderer::PauseInternal");
  pause_reference_time_ = clock_->now();

  // Update our reference clock to fractional frame transformation, keeping it 1st order continuous.
  pause_time_frac_frames_ =
      Fixed::FromRaw(reference_clock_to_fractional_frames_->Apply(pause_reference_time_->get()));
  pause_time_frac_frames_valid_ = true;

  reference_clock_to_fractional_frames_->Update(
      TimelineFunction(pause_time_frac_frames_.raw_value(), pause_reference_time_->get(), {0, 1}));

  // If we do not know the pts_to_frac_frames relationship yet, compute one.
  if (!pts_to_frac_frames_valid_) {
    next_frac_frame_pts_ = pause_time_frac_frames_;
    ComputePtsToFracFrames(0);
  }

  pause_media_time_ =
      zx::time(pts_to_frac_frames_.ApplyInverse(pause_time_frac_frames_.raw_value()));

  // If the user requested a callback, figure out the media time that we paused at and report back.
  FX_LOGS(DEBUG) << ". Actual (ref: " << pause_reference_time_->get()
                 << ", media: " << pause_media_time_->get() << ")";

  ReportStopIfStarted();

  if (callback != nullptr) {
    if constexpr (kLogRendererPauseCalls) {
      FX_LOGS(INFO) << "BaseRenderer(" << this << ")::PauseInternal about to call Pause callback";
    }
    callback(pause_reference_time_->get(), pause_media_time_->get());
  }
}

void BaseRenderer::ReportStartIfStopped() {
  if (!IsPlaying()) {
    ReportStart();
  }
}

void BaseRenderer::ReportStopIfStarted() {
  if (IsPlaying()) {
    ReportStop();
  }
}

void BaseRenderer::ReportStart() {
  reporter_->StartSession(zx::clock::get_monotonic());
  state_ = State::Playing;
}

void BaseRenderer::ReportStop() {
  reporter_->StopSession(zx::clock::get_monotonic());
  state_ = State::Paused;
}

void BaseRenderer::BeginShutdown() { context_.route_graph().RemoveRenderer(*this); }

void BaseRenderer::OnLinkAdded() { RecomputeMinLeadTime(); }

void BaseRenderer::EnableMinLeadTimeEvents(bool enabled) {
  EnableMinLeadTimeEventsInternal(enabled);
}

void BaseRenderer::EnableMinLeadTimeEventsInternal(bool enabled) {
  TRACE_DURATION("audio", "BaseRenderer::EnableMinLeadTimeEvents");

  min_lead_time_events_enabled_ = enabled;
  if (enabled) {
    ReportNewMinLeadTime();
  }
}

void BaseRenderer::GetMinLeadTime(GetMinLeadTimeCallback callback) {
  GetMinLeadTimeInternal(std::move(callback));
}

void BaseRenderer::GetMinLeadTimeInternal(GetMinLeadTimeCallback callback) {
  TRACE_DURATION("audio", "BaseRenderer::GetMinLeadTime");

  callback(min_lead_time_.to_nsecs());
}

void BaseRenderer::ReportNewMinLeadTime() {
  TRACE_DURATION("audio", "BaseRenderer::ReportNewMinLeadTime");
  if (min_lead_time_events_enabled_) {
    auto& lead_time_event = audio_renderer_binding_.events();
    lead_time_event.OnMinLeadTimeChanged(min_lead_time_.to_nsecs());
    if constexpr (kLogPresentationDelay) {
      // This need not be logged every time since we also log this in RecomputeMinLeadTime.
      FX_LOGS(DEBUG) << "    (" << this << ") " << __FUNCTION__ << " reported "
                     << min_lead_time_.to_nsecs() << " ns";
    }
  }
}

// Use our adjustable clock as the default. This starts as an adjustable clone of MONOTONIC, but
// will track the clock of the device where the renderer is routed.
zx_status_t BaseRenderer::SetAdjustableReferenceClock() {
  TRACE_DURATION("audio", "BaseRenderer::SetAdjustableReferenceClock");
  clock_ =
      context_.clock_factory()->CreateClientAdjustable(audio::clock::AdjustableCloneOfMonotonic());
  return ZX_OK;
}

// Ensure that the clock has appropriate rights.
zx_status_t BaseRenderer::SetCustomReferenceClock(zx::clock ref_clock) {
  constexpr auto kRequiredClockRights = ZX_RIGHT_DUPLICATE | ZX_RIGHT_TRANSFER | ZX_RIGHT_READ;
  auto status = ref_clock.replace(kRequiredClockRights, &ref_clock);
  if (status != ZX_OK || !ref_clock.is_valid()) {
    FX_PLOGS(ERROR, status) << "Could not set rights on client-submitted reference clock";
    return ZX_ERR_INVALID_ARGS;
  }
  clock_ = context_.clock_factory()->CreateClientFixed(std::move(ref_clock));
  return ZX_OK;
}

// Regardless of the source of the reference clock, we can duplicate and return it here.
void BaseRenderer::GetReferenceClock(GetReferenceClockCallback callback) {
  TRACE_DURATION("audio", "BaseRenderer::GetReferenceClock");

  // If something goes wrong, hang up the phone and shutdown.
  auto cleanup = fit::defer([this]() { context_.route_graph().RemoveRenderer(*this); });

  // Regardless of whether clock_ is writable, this strips off the WRITE right.
  auto clock_result = clock_->DuplicateZxClockReadOnly();
  if (!clock_result) {
    FX_LOGS(ERROR) << "DuplicateZxClockReadOnly failed, will not return reference clock!";
    return;
  }

  callback(std::move(*clock_result));
  cleanup.cancel();
}

}  // namespace media::audio
