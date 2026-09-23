// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "usb-audio-stream.h"

#include <lib/ddk/device.h>
#include <lib/fit/defer.h>
#include <lib/zx/clock.h>
#include <lib/zx/vmar.h>
#include <string.h>
#include <zircon/process.h>
#include <zircon/threads.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <algorithm>
#include <cmath>
#include <limits>
#include <memory>
#include <utility>

#include <audio-proto-utils/format-utils.h>
#include <fbl/algorithm.h>
#include <fbl/auto_lock.h>
#include <usb/audio.h>
#include <usb/usb-request.h>

#include "src/lib/digest/digest.h"
#include "usb-audio-device.h"
#include "usb-audio-stream-interface.h"
#include "usb-audio.h"

namespace audio::usb {

namespace audio_fidl = fuchsia_hardware_audio;

static constexpr uint32_t MAX_OUTSTANDING_REQ = 6;

UsbAudioStream::UsbAudioStream(UsbAudioDevice* parent, std::unique_ptr<UsbAudioStreamInterface> ifc)
    : UsbAudioStreamBase(parent->zxdev()),
      AudioStreamProtocol(ifc->direction() == Direction::Input),
      parent_(*parent),
      ifc_(std::move(ifc)),
      create_time_(zx::clock::get_monotonic().get()),
      loop_(&kAsyncLoopConfigNeverAttachToThread),
      dispatcher_loop_(&kAsyncLoopConfigNeverAttachToThread) {
  snprintf(log_prefix_, sizeof(log_prefix_), "UsbAud %04hx:%04hx %s-%03d", parent_.vid(),
           parent_.pid(), is_input() ? "input" : "output", ifc_->term_link());
  loop_.StartThread("usb-audio-stream-loop");

  root_ = inspect_.GetRoot().CreateChild("usb_audio_stream");
  state_ = root_.CreateString("state", "created");
  number_of_stream_channels_ = root_.CreateUint("number_of_stream_channels", 0);
  start_time_ = root_.CreateInt("start_time", 0);
  position_request_time_ = root_.CreateInt("position_request_time", 0);
  position_reply_time_ = root_.CreateInt("position_reply_time", 0);
  frames_requested_ = root_.CreateUint("frames_requested", 0);
  ring_buffer_size2_ = root_.CreateUint("ring_buffer_size", 0);
  usb_requests_sent_ = root_.CreateUint("usb_requests_sent", 0);
  usb_requests_outstanding_ = root_.CreateUint("usb_requests_outstanding", 0);

  frame_rate_ = root_.CreateUint("frame_rate", 0);
  bits_per_slot_ = root_.CreateUint("bits_per_slot", 0);
  bits_per_sample_ = root_.CreateUint("bits_per_sample", 0);
  sample_format_ = root_.CreateString("sample_format", "not_set");

  size_t number_of_formats = ifc_->formats().size();
  supported_min_number_of_channels_ =
      root_.CreateUintArray("supported_min_number_of_channels", number_of_formats);
  supported_max_number_of_channels_ =
      root_.CreateUintArray("supported_max_number_of_channels", number_of_formats);
  supported_min_frame_rates_ =
      root_.CreateUintArray("supported_min_frame_rates", number_of_formats);
  supported_max_frame_rates_ =
      root_.CreateUintArray("supported_max_frame_rates", number_of_formats);
  supported_bits_per_slot_ = root_.CreateUintArray("supported_bits_per_slot", number_of_formats);
  supported_bits_per_sample_ =
      root_.CreateUintArray("supported_bits_per_sample", number_of_formats);
  supported_sample_formats_ =
      root_.CreateStringArray("supported_sample_formats", number_of_formats);

  size_t count = 0;
  for (auto i : ifc_->formats()) {
    supported_min_number_of_channels_.Set(count, i.range_.min_channels);
    supported_max_number_of_channels_.Set(count, i.range_.max_channels);
    supported_min_frame_rates_.Set(count, i.range_.min_frames_per_second);
    supported_max_frame_rates_.Set(count, i.range_.max_frames_per_second);
    std::vector<utils::Format> formats = utils::GetAllFormats(i.range_.sample_formats);
    // Each UsbAudioStreamInterface formats() entry only reports one format.
    ZX_ASSERT(formats.size() == 1);
    utils::Format format = formats[0];
    supported_bits_per_slot_.Set(count, format.bytes_per_sample * 8ul);
    supported_bits_per_sample_.Set(count, format.valid_bits_per_sample);
    switch (format.format) {
      case audio_fidl::wire::SampleFormat::kPcmSigned:
        supported_sample_formats_.Set(count, "PCM_signed");
        break;
      case audio_fidl::wire::SampleFormat::kPcmUnsigned:
        supported_sample_formats_.Set(count, "PCM_unsigned");
        break;
      case audio_fidl::wire::SampleFormat::kPcmFloat:
        supported_sample_formats_.Set(count, "PCM_float");
        break;
    }
    count++;
  }
}

fbl::RefPtr<UsbAudioStream> UsbAudioStream::Create(UsbAudioDevice* parent,
                                                   std::unique_ptr<UsbAudioStreamInterface> ifc) {
  ZX_DEBUG_ASSERT(parent != nullptr);
  ZX_DEBUG_ASSERT(ifc != nullptr);

  fbl::AllocChecker ac;
  auto stream = fbl::AdoptRef(new (&ac) UsbAudioStream(parent, std::move(ifc)));
  if (!ac.check()) {
    LOG_EX(ERROR, *parent, "Out of memory while attempting to allocate UsbAudioStream");
    return nullptr;
  }

  stream->ComputePersistentUniqueId();

  return stream;
}

zx_status_t UsbAudioStream::Bind() {
  char name[64];
  snprintf(name, sizeof(name), "usb-audio-%s-%03d", is_input() ? "input" : "output",
           ifc_->term_link());

  auto status =
      UsbAudioStreamBase::DdkAdd(ddk::DeviceAddArgs(name).set_inspect_vmo(inspect_.DuplicateVmo()));
  if (status == ZX_OK) {
    // If bind/setup has succeeded, then the devmgr now holds a reference to us.
    // Manually increase our reference count to account for this.
    this->AddRef();
  } else {
    LOG(ERROR, "Failed to publish UsbAudioStream device node (name \"%s\", status %d)", name,
        status);
  }

  thrd_t tmp_thrd;
  dispatcher_loop_.StartThread("usb-audio-stream-dispatcher-loop", &tmp_thrd);
  // TODO(johngro) : See https://fxbug.dev/42105800.  Eliminate this as soon as we have a more
  // official way of meeting real-time latency requirements.
  constexpr char role_name[] = "fuchsia.devices.usb.audio";
  const size_t role_name_size = strlen(role_name);
  status = device_set_profile_by_role(this->zxdev(), thrd_get_zx_handle(tmp_thrd), role_name,
                                      role_name_size);
  if (status != ZX_OK) {
    zxlogf(WARNING,
           "Failed to apply role \"%s\" to the USB audio callback thread.  Service will be best "
           "effort.\n",
           role_name);
  }

  return status;
}

void UsbAudioStream::ComputePersistentUniqueId() {
  // Do the best that we can to generate a persistent ID unique to this audio
  // stream by blending information from a number of sources.  In particular,
  // consume...
  //
  // 1) This USB device's top level device descriptor (this contains the
  //    VID/PID of the device, among other things)
  // 2) The contents of the descriptor list used to describe the control and
  //    streaming interfaces present in the device.
  // 3) The manufacturer, product, and serial number string descriptors (if
  //    present)
  // 4) The stream interface ID.
  //
  // The goal here is to produce something like a UUID which is as unique to a
  // specific instance of a specific device as we can make it, but which
  // should persist across boots even in the presence of driver updates an
  // such.  Even so, upper levels of code will still need to deal with the sad
  // reality that some types of devices may end up looking the same between
  // two different instances.  If/when this becomes an issue, we may need to
  // pursue other options.  One choice might be to change the way devices are
  // enumerated in the USB section of the device tree so that their path has
  // only to do with physical topology, and has no runtime enumeration order
  // dependencies.  At that point in time, adding the topology into the hash
  // should do the job, but would imply that the same device plugged into two
  // different ports will have a different unique ID for the purposes of
  // saving and restoring driver settings (as it does in some operating
  // systems today).
  //
  uint16_t vid = parent_.desc().id_vendor;
  uint16_t pid = parent_.desc().id_product;
  audio_stream_unique_id_t fallback_id{
      .data =
          {
              'U',
              'S',
              'B',
              ' ',
              static_cast<uint8_t>(vid >> 8),
              static_cast<uint8_t>(vid),
              static_cast<uint8_t>(pid >> 8),
              static_cast<uint8_t>(pid),
              ifc_->iid(),
          },
  };
  persistent_unique_id_ = fallback_id;

  digest::Digest sha;
  sha.Init();

  // #1: Top level descriptor.
  sha.Update(&parent_.desc(), sizeof(parent_.desc()));

  // #2: The descriptor list
  const auto& desc_list = parent_.desc_list();
  ZX_DEBUG_ASSERT((desc_list != nullptr) && (desc_list->size() > 0));
  sha.Update(desc_list->data(), desc_list->size());

  // #3: The various descriptor strings which may exist.
  const fbl::Array<uint8_t>* desc_strings[] = {
      &parent_.mfr_name(),
      &parent_.prod_name(),
      &parent_.serial_num(),
  };
  for (const auto str : desc_strings) {
    if (!str->empty()) {
      sha.Update(str->data(), str->size());
    }
  }

  // #4: The stream interface's ID.
  auto iid = ifc_->iid();
  sha.Update(&iid, sizeof(iid));

  // Finish the SHA and attempt to copy as much of the results to our internal
  // cached representation as we can.
  sha.Final();
  sha.CopyTruncatedTo(persistent_unique_id_.data, sizeof(persistent_unique_id_.data));
}

void UsbAudioStream::ReleaseRingBufferLocked() {
  if (ring_buffer_virt_ != nullptr) {
    ZX_DEBUG_ASSERT(ring_buffer_size_ != 0);
    zx::vmar::root_self()->unmap(reinterpret_cast<uintptr_t>(ring_buffer_virt_), ring_buffer_size_);
    ring_buffer_virt_ = nullptr;
    ring_buffer_size_ = 0;
  }
  ring_buffer_vmo_.reset();
}

void UsbAudioStream::Connect(ConnectRequestView request, ConnectCompleter::Sync& completer) {
  fbl::AutoLock lock(&lock_);
  if (shutting_down_) {
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  // Attempt to allocate a new driver channel and bind it to us.  If we don't
  // already have an stream_channel_, flag this channel is the privileged
  // connection (The connection which is allowed to do things like change
  // formats).
  bool privileged = (stream_channel_ == nullptr);

  auto stream_channel = StreamChannel::Create<StreamChannel>(this);
  if (stream_channel == nullptr) {
    completer.Close(ZX_ERR_NO_MEMORY);
    return;
  }
  stream_channels_.push_back(stream_channel);
  number_of_stream_channels_.Add(1);
  fidl::OnUnboundFn<fidl::WireServer<audio_fidl::StreamConfig>> on_unbound =
      [this, stream_channel](fidl::WireServer<audio_fidl::StreamConfig>*, fidl::UnbindInfo,
                             fidl::ServerEnd<fuchsia_hardware_audio::StreamConfig>) {
        fbl::AutoLock channel_lock(&lock_);
        this->DeactivateStreamChannelLocked(stream_channel.get());
      };

  stream_channel->BindServer(fidl::BindServer(loop_.dispatcher(), std::move(request->protocol),
                                              stream_channel.get(), std::move(on_unbound)));

  if (privileged) {
    ZX_DEBUG_ASSERT(stream_channel_ == nullptr);
    stream_channel_ = std::move(stream_channel);
  }
}

void UsbAudioStream::DdkUnbind(ddk::UnbindTxn txn) {
  {
    fbl::AutoLock lock(&lock_);
    shutting_down_ = true;
    rb_vmo_fetched_ = false;
  }
  dispatcher_loop_.Shutdown();
  // We stop the loop so we can safely deactivate channels via RAII via DdkRelease.
  loop_.Shutdown();

  // Unpublish our device node.
  txn.Reply();
}

void UsbAudioStream::DdkRelease() {
  // Reclaim our reference from the driver framework and let it go out of
  // scope.  If this is our last reference (it should be), we will destruct
  // immediately afterwards.
  auto stream = fbl::ImportFromRawPtr(this);

  // Make sure that our parent is no longer holding a reference to us.
  parent_.RemoveAudioStream(stream);
}

bool UsbAudioStream::is_async() const {
  return ifc_ && ifc_->ep_sync_type() == EndpointSyncType::Async;
}

void UsbAudioStream::GetSupportedFormats(
    StreamChannel::GetSupportedFormatsCompleter::Sync& completer) {
  const fbl::Vector<UsbAudioStreamInterface::FormatMapEntry>& formats = ifc_->formats();
  if (formats.size() > std::numeric_limits<uint16_t>::max()) {
    LOG(ERROR, "Too many formats (%zu) to send during AUDIO_STREAM_CMD_GET_FORMATS request!",
        formats.size());
    return;
  }

  // Build formats compatible with FIDL from a vector of audio_stream_format_range_t.
  // Needs to be alive until the reply is sent.
  struct FidlCompatibleFormats {
    fbl::Vector<uint8_t> number_of_channels;
    fbl::Vector<audio_fidl::wire::SampleFormat> sample_formats;
    fbl::Vector<uint32_t> frame_rates;
    fbl::Vector<uint8_t> valid_bits_per_sample;
    fbl::Vector<uint8_t> bytes_per_sample;
  };
  fbl::Vector<FidlCompatibleFormats> fidl_compatible_formats;
  for (const UsbAudioStreamInterface::FormatMapEntry& i : formats) {
    std::vector<utils::Format> formats = audio::utils::GetAllFormats(i.range_.sample_formats);
    ZX_ASSERT(!formats.empty());
    for (utils::Format& j : formats) {
      fbl::Vector<uint32_t> rates;
      audio::utils::FrameRateEnumerator enumerator(i.range_);
      for (uint32_t rate : enumerator) {
        rates.push_back(rate);
      }

      fbl::Vector<uint8_t> number_of_channels;
      for (uint8_t j = i.range_.min_channels; j <= i.range_.max_channels; ++j) {
        number_of_channels.push_back(j);
      }

      fidl_compatible_formats.push_back({
          .number_of_channels = std::move(number_of_channels),
          .sample_formats = {j.format},
          .frame_rates = std::move(rates),
          .valid_bits_per_sample = {j.valid_bits_per_sample},
          .bytes_per_sample = {j.bytes_per_sample},
      });
    }
  }

  fidl::Arena allocator;
  fidl::VectorView<audio_fidl::wire::SupportedFormats> fidl_formats(allocator,
                                                                    fidl_compatible_formats.size());
  // Build formats compatible with FIDL for all the formats.
  // Needs to be alive until the reply is sent.
  for (size_t i = 0; i < fidl_compatible_formats.size(); ++i) {
    FidlCompatibleFormats& src = fidl_compatible_formats[i];
    audio_fidl::wire::SupportedFormats& dst = fidl_formats[i];

    audio_fidl::wire::PcmSupportedFormats formats;
    formats.Allocate(allocator);
    fidl::VectorView<audio_fidl::wire::ChannelSet> channel_sets(allocator,
                                                                src.number_of_channels.size());

    for (size_t j = 0; j < src.number_of_channels.size(); ++j) {
      fidl::VectorView<audio_fidl::wire::ChannelAttributes> attributes(allocator,
                                                                       src.number_of_channels[j]);
      channel_sets[j].Allocate(allocator);
      channel_sets[j].set_attributes(allocator, attributes);
    }
    formats.set_channel_sets(allocator, channel_sets);
    formats.set_sample_formats(allocator,
                               ::fidl::VectorView<audio_fidl::wire::SampleFormat>::FromExternal(
                                   src.sample_formats.data(), src.sample_formats.size()));
    formats.set_frame_rates(allocator, ::fidl::VectorView<uint32_t>::FromExternal(
                                           src.frame_rates.data(), src.frame_rates.size()));
    formats.set_bytes_per_sample(
        allocator, ::fidl::VectorView<uint8_t>::FromExternal(src.bytes_per_sample.data(),
                                                             src.bytes_per_sample.size()));
    formats.set_valid_bits_per_sample(
        allocator, ::fidl::VectorView<uint8_t>::FromExternal(src.valid_bits_per_sample.data(),
                                                             src.valid_bits_per_sample.size()));

    dst.Allocate(allocator);
    dst.set_pcm_supported_formats(allocator, formats);
  }

  completer.Reply(fidl_formats);
}

void UsbAudioStream::CreateRingBuffer(StreamChannel* channel, audio_fidl::wire::Format format,
                                      ::fidl::ServerEnd<audio_fidl::RingBuffer> ring_buffer,
                                      StreamChannel::CreateRingBufferCompleter::Sync& completer) {
  // Only the privileged stream channel is allowed to change the format.
  {
    fbl::AutoLock channel_lock(&lock_);
    if (channel != stream_channel_.get()) {
      LOG(ERROR, "Unprivileged channel cannot set the format");
      completer.Close(ZX_ERR_INVALID_ARGS);
      return;
    }
  }

  auto req = format.pcm_format();

  audio_sample_format_t sample_format =
      audio::utils::GetSampleFormat(req.valid_bits_per_sample, 8 * req.bytes_per_sample);

  if (sample_format == 0) {
    LOG(ERROR, "Unsupported format: Invalid bits per sample (%u/%u)", req.valid_bits_per_sample,
        8 * req.bytes_per_sample);
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  if (req.sample_format == audio_fidl::wire::SampleFormat::kPcmFloat) {
    sample_format = AUDIO_SAMPLE_FORMAT_32BIT_FLOAT;
    if (req.valid_bits_per_sample != 32 || req.bytes_per_sample != 4) {
      LOG(ERROR, "Unsupported format: Not 32 per sample/channel for float");
      completer.Close(ZX_ERR_INVALID_ARGS);
      return;
    }
  }

  if (req.sample_format == audio_fidl::wire::SampleFormat::kPcmUnsigned) {
    sample_format |= AUDIO_SAMPLE_FORMAT_FLAG_UNSIGNED;
  }

  // Look up the details about the interface and the endpoint which will be
  // used for the requested format.
  size_t format_ndx;
  zx_status_t status =
      ifc_->LookupFormat(req.frame_rate, req.number_of_channels, sample_format, &format_ndx);
  if (status != ZX_OK) {
    LOG(ERROR, "Could not find a suitable format");
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  // Determine the frame size needed for this requested format, then compute
  // the size of our short packets, and the constants used to generate the
  // short/long packet cadence.  For now, assume that we will be operating at
  // a 1mSec isochronous rate.
  //
  // Make sure that we can fit our longest payload length into one of our
  // usb requests.
  //
  // Store the results of all of these calculations in local variables.  Do
  // not commit them to member variables until we are certain that we are
  // going to go ahead with this format change.
  //
  // TODO(johngro) : Unless/until we can find some way to set the USB bus
  // driver to perform direct DMA to/from the Ring Buffer VMO without the need
  // for software intervention, we may want to expose ways to either increase
  // the isochronous interval (to minimize load) or to use USB 2.0 125uSec
  // sub-frame timing (to decrease latency) if possible.
  uint32_t frame_size;
  frame_size =
      audio::utils::ComputeFrameSize(static_cast<uint16_t>(req.number_of_channels), sample_format);
  if (!frame_size) {
    LOG(ERROR, "Failed to compute frame size (ch %hu fmt 0x%08x)", req.number_of_channels,
        sample_format);
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  ZX_DEBUG_ASSERT(format_ndx < ifc_->formats().size());

  static constexpr uint32_t iso_packet_rate = 1000;
  uint32_t bytes_per_packet, fractional_bpp_inc, long_payload_len;
  if (is_input() && is_async()) {
    // For async inputs, give them as much room as we can, on every
    // single packet.  Make sure it's a multiple of the frame size;
    // it probably should already be a multiple, but if so, we're
    // not losing anything by checking it.
    bytes_per_packet = ifc_->formats()[format_ndx].max_req_size_;
    bytes_per_packet -= bytes_per_packet % frame_size;
    fractional_bpp_inc = 0;
    long_payload_len = bytes_per_packet;
  } else {
    bytes_per_packet = (req.frame_rate / iso_packet_rate) * frame_size;
    fractional_bpp_inc = (req.frame_rate % iso_packet_rate);
    long_payload_len = bytes_per_packet + (fractional_bpp_inc ? frame_size : 0);
  }

  if (long_payload_len > ifc_->formats()[format_ndx].max_req_size_) {
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  // Deny the format change request if the ring buffer is not currently stopped.
  {
    // TODO(johngro) : If the ring buffer is running, should we automatically
    // stop it instead of returning bad state?
    fbl::AutoLock req_lock(&req_lock_);
    if (ring_buffer_state_ != RingBufferState::STOPPED) {
      completer.Close(ZX_ERR_BAD_STATE);
      return;
    }
  }

  fbl::AutoLock req_lock(&lock_);
  if (shutting_down_) {
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  // Looks like we are going ahead with this format change.  Tear down any
  // exiting ring buffer interface before proceeding.
  if (rb_channel_ != nullptr) {
    rb_channel_->UnbindServer();
  }

  // Record the details of our cadence and format selection
  selected_format_ndx_ = format_ndx;
  selected_frame_rate_ = req.frame_rate;
  frame_size_ = frame_size;
  iso_packet_rate_ = iso_packet_rate;
  bytes_per_packet_ = bytes_per_packet;
  fractional_bpp_inc_ = fractional_bpp_inc;

  // Compute the effective fifo depth for this stream.  Right now, we are in a
  // situation where, for an output, we need to memcpy payloads from the mixer
  // ring buffer into the jobs that we send to the USB host controller.  For an
  // input, when the jobs complete, we need to copy the data from the completed
  // job into the ring buffer.
  //
  // This gives us two different "fifo" depths we may need to report.  For an
  // input, if job X just completed, we will be copying the data sometime during
  // job X+1, assuming that we are hitting our callback targets.  Because of
  // this, we should be safe to report our fifo depth as being 2 times the size
  // of a single maximum sized job.
  //
  // For output, we are attempting to stay MAX_OUTSTANDING_REQ ahead, and we are
  // copying the data from the mixer ring buffer as we go.  Because of this, our
  // reported fifo depth is going to be MAX_OUTSTANDING_REQ maximum sized jobs
  // ahead of the nominal read pointer.
  fifo_bytes_ = bytes_per_packet_ * (is_input() ? 2 : MAX_OUTSTANDING_REQ);

  // If we have no fractional portion to accumulate, we always send
  // short packets.  If our fractional portion is <= 1/2 of our
  // isochronous rate, then we will never send two long packets back
  // to back.
  if (fractional_bpp_inc_) {
    fifo_bytes_ += frame_size_;
    if (fractional_bpp_inc_ > (iso_packet_rate_ >> 1)) {
      fifo_bytes_ += frame_size_;
    }
  }
  if (req.frame_rate == 0) {
    LOG(ERROR, "Bad (zero) frame rate");
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  if (frame_size == 0) {
    LOG(ERROR, "Bad (zero) frame size");
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  internal_delay_nsec_ = 0;  // No internal delay known, so we report 0.

  // Create a new ring buffer channel which can be used to move bulk data and
  // bind it to us.
  rb_channel_ = Channel::Create<RingBufferChannel>();

  number_of_channels_.Set(req.number_of_channels);
  frame_rate_.Set(req.frame_rate);
  bits_per_slot_.Set(req.bytes_per_sample * 8ul);
  bits_per_sample_.Set(req.valid_bits_per_sample);
  // clang-format off
  switch (req.sample_format) {
    case audio_fidl::wire::SampleFormat::kPcmSigned:   sample_format_.Set("PCM_signed");   break;
    case audio_fidl::wire::SampleFormat::kPcmUnsigned: sample_format_.Set("PCM_unsigned"); break;
    case audio_fidl::wire::SampleFormat::kPcmFloat:    sample_format_.Set("PCM_float");    break;
  }
  // clang-format on

  fidl::OnUnboundFn<fidl::WireServer<audio_fidl::RingBuffer>> on_unbound =
      [this](fidl::WireServer<audio_fidl::RingBuffer>*, fidl::UnbindInfo,
             fidl::ServerEnd<fuchsia_hardware_audio::RingBuffer>) {
        fbl::AutoLock lock(&lock_);
        this->DeactivateRingBufferChannelLocked(rb_channel_.get());
      };

  rb_channel_->BindServer(
      fidl::BindServer(loop_.dispatcher(), std::move(ring_buffer), this, std::move(on_unbound)));
}

void UsbAudioStream::WatchGainState(StreamChannel* channel,
                                    StreamChannel::WatchGainStateCompleter::Sync& completer) {
  if (channel->gain_completer_) {
    completer.Close(ZX_ERR_BAD_STATE);
    channel->gain_completer_.reset();
    channel->last_reported_gain_state_ = {};
    return;
  }

  channel->gain_completer_ = completer.ToAsync();

  ZX_DEBUG_ASSERT(ifc_->path() != nullptr);
  const auto& path = *(ifc_->path());

  audio_proto::GainState cur_gain_state = {};
  cur_gain_state.cur_mute = path.cur_mute();
  cur_gain_state.cur_agc = path.cur_agc();
  cur_gain_state.cur_gain = path.cur_gain();
  cur_gain_state.can_mute = path.has_mute();
  cur_gain_state.can_agc = path.has_agc();
  cur_gain_state.min_gain = path.min_gain();
  cur_gain_state.max_gain = path.max_gain();
  cur_gain_state.gain_step = path.gain_res();
  // Reply is delayed if there is no change since the last reported gain state.
  if (channel->last_reported_gain_state_ != cur_gain_state) {
    fidl::Arena allocator;
    audio_fidl::wire::GainState gain_state(allocator);
    if (cur_gain_state.can_mute) {
      gain_state.set_muted(cur_gain_state.cur_mute);
    }
    if (cur_gain_state.can_agc) {
      gain_state.set_agc_enabled(cur_gain_state.cur_agc);
    }
    gain_state.set_gain_db(cur_gain_state.cur_gain);
    channel->last_reported_gain_state_ = cur_gain_state;
    channel->gain_completer_->Reply(gain_state);
    channel->gain_completer_.reset();
  }
}

void UsbAudioStream::WatchClockRecoveryPositionInfo(
    WatchClockRecoveryPositionInfoCompleter::Sync& completer) {
  fbl::AutoLock req_lock(&req_lock_);

  if (position_completer_) {
    completer.Close(ZX_ERR_BAD_STATE);
    position_completer_.reset();
    return;
  }

  position_completer_ = completer.ToAsync();
  position_request_time_.Set(zx::clock::get_monotonic().get());
}

void UsbAudioStream::WatchDelayInfo(WatchDelayInfoCompleter::Sync& completer) {
  if (delay_completer_) {
    completer.Close(ZX_ERR_BAD_STATE);
    delay_completer_.reset();
    return;
  }

  if (!delay_info_updated_) {
    delay_info_updated_ = true;
    fidl::Arena allocator;
    auto delay_info = audio_fidl::wire::DelayInfo::Builder(allocator);
    // No external delay information is provided by this driver.
    // TODO(johngro): Report the actual external delay.
    delay_info.internal_delay(internal_delay_nsec_);
    completer.Reply(delay_info.Build());
    return;
  }
  // All completers must either Reply, Close, or ToAsync(+persist until Unbind).
  delay_completer_ = completer.ToAsync();
}

void UsbAudioStream::SetGain(audio_fidl::wire::GainState state,
                             StreamChannel::SetGainCompleter::Sync& completer) {
  // TODO(johngro): Actually perform the set operation on our audio path.
  ZX_DEBUG_ASSERT(ifc_->path() != nullptr);
  auto& path = *(ifc_->path());
  bool illegal_mute = state.has_muted() && state.muted() && !path.has_mute();
  bool illegal_agc = state.has_agc_enabled() && state.agc_enabled() && !path.has_agc();
  bool illegal_gain =
      state.has_gain_db() &&
      (
          // If the path has no gain control, then setting gain_db to a non-zero value is illegal.
          (!path.has_gain() && state.gain_db() != 0) ||
          // It is also illegal to set it to NAN or +/-INFINITY...
          !std::isfinite(state.gain_db()) ||
          // ...or to set it out-of-range...
          state.gain_db() > path.max_gain() || state.gain_db() < path.min_gain());

  if (illegal_mute || illegal_agc || illegal_gain) {
    // If this request is illegal in any way, make no changes.
    return;
  }

  if (state.has_muted()) {
    state.muted() = path.SetMute(parent_.usb_proto(), state.muted());
  }

  if (state.has_agc_enabled()) {
    state.agc_enabled() = path.SetAgc(parent_.usb_proto(), state.agc_enabled());
  }

  if (state.has_gain_db()) {
    state.gain_db() = path.SetGain(parent_.usb_proto(), state.gain_db());
  }

  fbl::AutoLock channel_lock(&lock_);
  for (auto& channel : stream_channels_) {
    if (channel.gain_completer_) {
      channel.gain_completer_->Reply(state);
      channel.gain_completer_.reset();
    }
  }
}

void UsbAudioStream::WatchPlugState(StreamChannel* channel,
                                    StreamChannel::WatchPlugStateCompleter::Sync& completer) {
  if (channel->plug_completer_) {
    completer.Close(ZX_ERR_BAD_STATE);
    channel->plug_completer_.reset();
    channel->last_reported_plugged_state_ = StreamChannel::Plugged::kNotReported;
    return;
  }

  channel->plug_completer_ = completer.ToAsync();

  // As long as the usb device is present, we are plugged. A second reply is delayed indefinitely
  // since there will be no change from the last reported plugged state.
  if (channel->last_reported_plugged_state_ == StreamChannel::Plugged::kNotReported ||
      (channel->last_reported_plugged_state_ != StreamChannel::Plugged::kPlugged)) {
    fidl::Arena allocator;
    audio_fidl::wire::PlugState plug_state(allocator);
    plug_state.set_plugged(true).set_plug_state_time(allocator, create_time_);
    channel->last_reported_plugged_state_ = StreamChannel::Plugged::kPlugged;
    channel->plug_completer_->Reply(plug_state);
    channel->plug_completer_.reset();
  }
}

void UsbAudioStream::GetProperties(StreamChannel::GetPropertiesCompleter::Sync& completer) {
  fidl::Arena allocator;
  audio_fidl::wire::StreamProperties stream_properties(allocator);
  stream_properties.set_unique_id(allocator);
  for (size_t i = 0; i < audio_fidl::wire::kUniqueIdSize; ++i) {
    stream_properties.unique_id().data_[i] = persistent_unique_id_.data[i];
  }

  const auto& path = *ifc_->path();

  auto product = fidl::StringView::FromExternal(
      reinterpret_cast<const char*>(parent_.prod_name().begin()), parent_.prod_name().size());
  auto manufacturer = fidl::StringView::FromExternal(
      reinterpret_cast<const char*>(parent_.mfr_name().begin()), parent_.mfr_name().size());

  stream_properties.set_is_input(is_input())
      .set_can_mute(path.has_mute())
      .set_can_agc(path.has_agc())
      .set_min_gain_db(path.min_gain())
      .set_max_gain_db(path.max_gain())
      .set_gain_step_db(path.gain_res())
      .set_product(allocator, product)
      .set_manufacturer(allocator, manufacturer)
      .set_clock_domain(clock_domain_)
      .set_plug_detect_capabilities(audio_fidl::wire::PlugDetectCapabilities::kHardwired);

  completer.Reply(stream_properties);
}

void UsbAudioStream::GetProperties(GetPropertiesCompleter::Sync& completer) {
  fidl::Arena allocator;
  audio_fidl::wire::RingBufferProperties ring_buffer_properties(allocator);
  ring_buffer_properties.set_driver_transfer_bytes(fifo_bytes_)
      .set_needs_cache_flush_or_invalidate(true);
  completer.Reply(ring_buffer_properties);
}

void UsbAudioStream::GetVmo(GetVmoRequestView request, GetVmoCompleter::Sync& completer) {
  zx::vmo client_rb_handle;
  uint32_t map_flags, client_rights;
  frames_requested_.Set(request->min_frames);

  {
    // We cannot create a new ring buffer if we are not currently stopped.
    fbl::AutoLock req_lock(&req_lock_);
    if (ring_buffer_state_ != RingBufferState::STOPPED) {
      LOG(ERROR, "Tried to get VMO in non-stopped state");
      return;
    }
  }

  // Unmap and release any previous ring buffer.
  {
    fbl::AutoLock req_lock(&lock_);
    ReleaseRingBufferLocked();
  }

  auto cleanup = fit::defer([&completer, this]() {
    {
      fbl::AutoLock req_lock(&this->lock_);
      this->ReleaseRingBufferLocked();
    }
    completer.ReplyError(audio_fidl::wire::GetVmoError::kInternalError);
  });

  // Compute the ring buffer size.  It needs to be at least as big
  // as min_frames + the virtual fifo depth.
  ZX_DEBUG_ASSERT(frame_size_ && ((fifo_bytes_ % frame_size_) == 0));
  uint64_t ring_buffer_size =
      (static_cast<uint64_t>(request->min_frames) * frame_size_) + fifo_bytes_;
  if (ring_buffer_size > std::numeric_limits<uint32_t>::max()) {
    LOG(ERROR, "Ring buffer size overflow (%lu)", ring_buffer_size);
    return;
  }
  ring_buffer_size_ = static_cast<uint32_t>(ring_buffer_size);

  // Set up our state for generating notifications.
  if (request->clock_recovery_notifications_per_ring) {
    bytes_per_notification_ = ring_buffer_size_ / request->clock_recovery_notifications_per_ring;
  } else {
    bytes_per_notification_ = 0;
  }

  // Create the ring buffer vmo we will use to share memory with the client.
  zx_status_t status = zx::vmo::create(ring_buffer_size_, 0, &ring_buffer_vmo_);
  if (status != ZX_OK) {
    LOG(ERROR, "Failed to create ring buffer (size %u, res %d)", ring_buffer_size_, status);
    return;
  }

  // Map the VMO into our address space.
  //
  // TODO(johngro): skip this step when APIs in the USB bus driver exist to
  // DMA directly from the VMO.
  map_flags = ZX_VM_PERM_READ;
  if (is_input())
    map_flags |= ZX_VM_PERM_WRITE;

  status = zx::vmar::root_self()->map(map_flags, 0, ring_buffer_vmo_, 0, ring_buffer_size_,
                                      reinterpret_cast<uintptr_t*>(&ring_buffer_virt_));
  if (status != ZX_OK) {
    LOG(ERROR, "Failed to map ring buffer (size %u, res %d)", ring_buffer_size_, status);
    return;
  }

  // Create the client's handle to the ring buffer vmo and set it back to them.
  client_rights = ZX_RIGHT_TRANSFER | ZX_RIGHT_MAP | ZX_RIGHT_READ;
  if (!is_input())
    client_rights |= ZX_RIGHT_WRITE;

  status = ring_buffer_vmo_.duplicate(client_rights, &client_rb_handle);
  if (status != ZX_OK) {
    LOG(ERROR, "Failed to duplicate ring buffer handle (res %d)", status);
    return;
  }

  uint32_t num_ring_buffer_frames = ring_buffer_size_ / frame_size_;

  cleanup.cancel();
  {
    fbl::AutoLock lock(&lock_);
    rb_vmo_fetched_ = true;
  }
  ring_buffer_size2_.Set(ring_buffer_size_);
  completer.ReplySuccess(num_ring_buffer_frames, std::move(client_rb_handle));
}

void UsbAudioStream::Start(StartCompleter::Sync& completer) {
  fbl::AutoLock lock(&lock_);
  fbl::AutoLock req_lock(&req_lock_);

  {
    if (!rb_vmo_fetched_) {
      zxlogf(ERROR, "Did not start, VMO not fetched");
      completer.Close(ZX_ERR_BAD_STATE);
      return;
    }
  }

  if (ring_buffer_state_ != RingBufferState::STOPPED) {
    // The ring buffer is running, do not linger in the lock while we send
    // the error code back to the user.
    LOG(ERROR, "Attempt to start an already started ring buffer");
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  // We are idle, all of our usb requests should be sitting in the free list.
  ZX_DEBUG_ASSERT(ep_.RequestsFull());

  // Activate the format.
  zx_status_t status = ifc_->ActivateFormat(selected_format_ndx_, selected_frame_rate_);
  if (status != ZX_OK) {
    LOG(ERROR, "Failed to activate format %d", status);
    completer.Reply(zx::clock::get_monotonic().get());
    return;
  }

  // Initialize the counters used to...
  // 1) generate the short/long packet cadence.
  // 2) generate notifications.
  // 3) track the position in the ring buffer.
  fractional_bpp_acc_ = 0;
  notification_acc_ = 0;
  ring_buffer_offset_ = 0;
  ring_buffer_pos_ = 0;

  // Flag ourselves as being in the starting state, then queue up all of our
  // transactions.
  ring_buffer_state_ = RingBufferState::STARTING;
  state_.Set("starting");
  status = StartRequests();
  if (status != ZX_OK) {
    zxlogf(ERROR, "Could not start Request Thread %d", status);
    completer.Close(status);
    return;
  }

  start_completer_.emplace(completer.ToAsync());
}

void UsbAudioStream::Stop(StopCompleter::Sync& completer) {
  fbl::AutoLock lock(&lock_);
  fbl::AutoLock req_lock(&req_lock_);

  {
    if (!rb_vmo_fetched_) {
      zxlogf(ERROR, "Did not stop, VMO not fetched");
      completer.Close(ZX_ERR_BAD_STATE);
      return;
    }
  }

  // TODO(johngro): Fix this to use the cancel transaction capabilities added
  // to the USB bus driver.
  //
  // Also, investigate whether or not the cancel interface is synchronous or
  // whether we will need to maintain an intermediate stopping state.
  if (ring_buffer_state_ != RingBufferState::STARTED) {
    LOG(INFO, "Attempt to stop a not started ring buffer");
    completer.Reply();
    return;
  }

  ring_buffer_state_ = RingBufferState::STOPPING;
  state_.Set("stopping_requested");
  stop_completer_.emplace(completer.ToAsync());
}

void UsbAudioStream::RequestBatchComplete(
    std::vector<fuchsia_hardware_usb_endpoint::Completion> completion) {
  for (auto& req : completion) {
    RequestComplete(std::move(req));
  }
}

void UsbAudioStream::RequestComplete(fuchsia_hardware_usb_endpoint::Completion completion) {
  enum class Action : uint8_t {
    NONE,
    SIGNAL_STARTED,
    SIGNAL_STOPPED,
    NOTIFY_POSITION,
    HANDLE_UNPLUG,
  };

  audio_fidl::wire::RingBufferPositionInfo position_info = {};

  usb_requests_outstanding_.Subtract(1);

  zx_time_t complete_time = zx::clock::get_monotonic().get();
  Action when_finished = Action::NONE;

  {
    fbl::AutoLock req_lock(&req_lock_);

    // Cache the status and length of this usb request.
    zx_status_t req_status = *completion.status();

    // Complete the usb request.  This will return the transaction to the free
    // list and (in the case of an input stream) copy the payload to the
    // ring buffer, and update the ring buffer position.
    //
    // TODO(johngro): copying the payload out of the ring buffer is an
    // operation which goes away when we get to the zero copy world.
    auto req_length = CompleteRequestLocked(std::move(completion));

    // Did the transaction fail because the device was unplugged?  If so,
    // enter the stopping state and close the connections to our clients.
    if (req_status == ZX_ERR_IO_NOT_PRESENT) {
      ring_buffer_state_ = RingBufferState::STOPPING_AFTER_UNPLUG;
      state_.Set("stopping_after_unplug");
    } else {
      // If we are supposed to be delivering notifications, check to see
      // if it is time to do so.
      if (bytes_per_notification_) {
        notification_acc_ += req_length;

        if ((ring_buffer_state_ == RingBufferState::STARTED) &&
            (notification_acc_ >= bytes_per_notification_)) {
          when_finished = Action::NOTIFY_POSITION;
          notification_acc_ = (notification_acc_ % bytes_per_notification_);
          position_info.timestamp = zx::clock::get_monotonic().get();
          position_info.position = ring_buffer_pos_;
        }
      }
    }

    switch (ring_buffer_state_) {
      case RingBufferState::STOPPING:
        if (ep_.RequestsFull()) {
          when_finished = Action::SIGNAL_STOPPED;
        }
        break;

      case RingBufferState::STOPPING_AFTER_UNPLUG:
        if (ep_.RequestsFull()) {
          when_finished = Action::HANDLE_UNPLUG;
        }
        break;

      case RingBufferState::STARTING:
        when_finished = Action::SIGNAL_STARTED;
        [[fallthrough]];

      case RingBufferState::STARTED: {
        auto status = QueueRequestLocked();
        if (status != ZX_OK) {
          // QueueRequestLocked may fail if FIDL connection has dropped.
          LOG(ERROR, "QueueRequestLocked failed %d", status);
          when_finished = Action::SIGNAL_STOPPED;
        }
      } break;

      case RingBufferState::STOPPED:
      default:
        LOG(ERROR, "Invalid state (%u)", static_cast<uint32_t>(ring_buffer_state_));
        ZX_DEBUG_ASSERT(false);
        break;
    }
  }

  if (when_finished != Action::NONE) {
    fbl::AutoLock lock(&lock_);
    switch (when_finished) {
      case Action::SIGNAL_STARTED:
        if (rb_channel_ != nullptr) {
          // TODO(johngro) : this start time estimate is not as good as it
          // could be.  We really need to have the USB bus driver report
          // the relationship between the USB frame counter and the system
          // tick counter (and track the relationship in the case that the
          // USB oscillator is not derived from the system oscillator).
          // Then we can accurately report the start time as the time of
          // the tick on which we scheduled the first transaction.
          fbl::AutoLock req_lock(&req_lock_);
          start_completer_->Reply(zx_time_sub_duration(complete_time, ZX_MSEC(1)));
          start_completer_.reset();
        }
        {
          fbl::AutoLock req_lock(&req_lock_);
          ring_buffer_state_ = RingBufferState::STARTED;
          state_.Set("started");
          start_time_.Set(zx::clock::get_monotonic().get());
        }
        break;

      case Action::HANDLE_UNPLUG:
        if (rb_channel_ != nullptr) {
          rb_channel_->UnbindServer();
        }

        if (stream_channel_ != nullptr) {
          stream_channel_->UnbindServer();
        }

        {
          fbl::AutoLock req_lock(&req_lock_);
          ring_buffer_state_ = RingBufferState::STOPPED;
          state_.Set("stopped_handle_unplug");
        }
        break;

      case Action::SIGNAL_STOPPED:
        if (rb_channel_ != nullptr) {
          fbl::AutoLock req_lock(&req_lock_);
          stop_completer_->Reply();
          stop_completer_.reset();
        }
        {
          fbl::AutoLock req_lock(&req_lock_);
          ring_buffer_state_ = RingBufferState::STOPPED;
          state_.Set("stopped_after_signal");
          ifc_->ActivateIdleFormat();
        }
        break;

      case Action::NOTIFY_POSITION: {
        fbl::AutoLock req_lock(&req_lock_);
        if (position_completer_) {
          position_completer_->Reply(position_info);
          position_completer_.reset();
          position_reply_time_.Set(zx::clock::get_monotonic().get());
        }
      } break;

      default:
        ZX_DEBUG_ASSERT(false);
        break;
    }
  }
}

zx_status_t UsbAudioStream::StartRequests() {
  auto client = DdkConnectFidlProtocol<fuchsia_hardware_usb::UsbService::Device>(parent_.parent());
  if (client.is_error()) {
    zxlogf(ERROR, "Failed to connect fidl protocol");
    return client.error_value();
  }
  auto status = ep_.Init(ifc_->ep_addr(), *client, dispatcher_loop_.dispatcher());
  if (status != ZX_OK) {
    zxlogf(ERROR, "Could not init endpoint %d", status);
    return status;
  }

  auto actual = ep_.AddRequests(MAX_OUTSTANDING_REQ, ifc_->max_req_size(),
                                fuchsia_hardware_usb_request::Buffer::Tag::kVmoId);
  if (actual == 0) {
    zxlogf(ERROR, "Could not add any requests!");
    return ZX_ERR_INTERNAL;
  }
  if (actual != MAX_OUTSTANDING_REQ) {
    zxlogf(WARNING, "Wanted %d requests, got %zu requests", MAX_OUTSTANDING_REQ, actual);
  }

  // Schedule the frame number which the first transaction will go out on.
  usb_frame_num_ = usb_get_current_frame(&parent_.usb_proto());

  status = QueueRequestLocked();
  if (status != ZX_OK) {
    LOG(ERROR, "QueueRequestLocked failed %d", status);
    return status;
  }

  return ZX_OK;
}

zx_status_t UsbAudioStream::QueueRequestLocked() {
  ZX_DEBUG_ASSERT((ring_buffer_state_ == RingBufferState::STARTING) ||
                  (ring_buffer_state_ == RingBufferState::STARTED));

  std::vector<fuchsia_hardware_usb_request::Request> request;
  while (auto req = ep_.GetRequest()) {
    // Figure out how much we want to send or receive this time (short or long
    // packet)
    uint32_t todo = bytes_per_packet_;
    fractional_bpp_acc_ += fractional_bpp_inc_;
    if (fractional_bpp_acc_ >= iso_packet_rate_) {
      fractional_bpp_acc_ -= iso_packet_rate_;
      todo += frame_size_;
      ZX_DEBUG_ASSERT(fractional_bpp_acc_ < iso_packet_rate_);
    }

    // If this is an output stream, copy our data into the usb request.
    // TODO(johngro): eliminate this when we can get to a zero-copy world.
    if (!is_input()) {
      req->clear_buffers();

      RingBufferCopy(*req, true, todo);
      ring_buffer_offset_ = (ring_buffer_offset_ + todo) % ring_buffer_size_;

      req->CacheFlush(ep_.GetMapped());
    } else {
      req->reset_buffers(ep_.GetMapped());
      req->CacheFlushInvalidate(ep_.GetMapped());
    }

    // Schedule this packet to be sent out on the next frame.
    (*req)->information()->isochronous()->frame_id(++usb_frame_num_);

    request.emplace_back(req->take_request());
  }

  usb_requests_sent_.Add(request.size());
  usb_requests_outstanding_.Add(request.size());
  auto result = ep_->QueueRequests(std::move(request));
  if (result.is_error()) {
    zxlogf(ERROR, "QueueRequests failed %s", result.error_value().FormatDescription().c_str());
    return result.error_value().status();
  }
  return ZX_OK;
}

size_t UsbAudioStream::CompleteRequestLocked(fuchsia_hardware_usb_endpoint::Completion completion) {
  auto request = ::usb::FidlRequest(std::move(*completion.request()));
  auto req_length = request.length();

  // If we are an input stream, copy the payload into the ring buffer.
  uint32_t todo = std::min<uint32_t>(*completion.transfer_size(), ring_buffer_size_);
  if (is_input()) {
    // TODO(https://fxbug.dev/42106932): for async inputs, measure and report the device's
    // observed sampling rate to the client.  If the device is falling
    // behind the nominal sampling rate, it's probably a minor quality
    // issue; but if they're running faster than the nominal sampling rate,
    // the client will be seeing older and older data as time goes on, and
    // the audio will fall further and further behind realtime.
    uint32_t avail = ring_buffer_size_ - ring_buffer_offset_;
    ZX_DEBUG_ASSERT(ring_buffer_offset_ < ring_buffer_size_);
    ZX_DEBUG_ASSERT((avail % frame_size_) == 0);

    if (completion.status() == ZX_OK) {
      RingBufferCopy(request, false, todo);
    } else {
      uint32_t amt = std::min(avail, todo);
      uint8_t* dst = reinterpret_cast<uint8_t*>(ring_buffer_virt_) + ring_buffer_offset_;

      // TODO(johngro): filling with zeros is only the proper thing to do
      // for signed formats.  USB does support unsigned 8-bit audio; if
      // that is our format, we should fill with 0x80 instead in order to
      // fill with silence.
      memset(dst, 0, amt);
      if (amt < todo) {
        memset(ring_buffer_virt_, 0, todo - amt);
      }
    }
  }

  // Check if the actual transfer length looks reasonable.
  // It's reasonable if it's exactly what we requested, or if we're dealing
  // with an async input, or if we got an error.  (If there's an error,
  // reporting the length mismatch is probably superfluous.)
  const bool actual_length_reasonable = (completion.transfer_size() == request.length()) ||
                                        (is_async() && is_input()) ||
                                        (completion.status() != ZX_OK);

  // If not reasonable, log a warning.
  if (!actual_length_reasonable) {
    // Rate limit warnings to no more than one every 3 seconds.
    if (zx::clock::get_monotonic() >= allow_length_warnings_) {
      allow_length_warnings_ = zx::deadline_after(zx::sec(3));
      zxlogf(WARNING, "%s: Audio transfer mismatch; asked for %lu bytes, actual %lu bytes",
             parent_.log_prefix(), request.length(), *completion.transfer_size());
    }
  }

  // Update the ring buffer position using 64-bit arithmetic to prevent overflow before modulo.
  uint64_t new_pos = static_cast<uint64_t>(ring_buffer_pos_) + todo;
  if (ring_buffer_size_ > 0) {
    ring_buffer_pos_ = static_cast<uint32_t>(new_pos % ring_buffer_size_);
  } else {
    ring_buffer_pos_ = 0;
  }

  // If this is an input stream, the ring buffer offset should always be equal
  // to the stream position.
  if (is_input()) {
    ring_buffer_offset_ = ring_buffer_pos_;
  }

  // Return the transaction to the free list.
  ep_.PutRequest(std::move(request));
  return req_length;
}

void UsbAudioStream::DeactivateStreamChannelLocked(StreamChannel* channel) {
  if (stream_channel_.get() == channel) {
    //  Any pending completers MUST be released, for the StreamConfig channel to cleanly unwind.
    stream_channel_->gain_completer_.reset();
    stream_channel_->plug_completer_.reset();

    stream_channel_ = nullptr;
  }
  stream_channels_.erase(*channel);
  number_of_stream_channels_.Subtract(1);
}

void UsbAudioStream::DeactivateRingBufferChannelLocked(const Channel* channel) {
  ZX_DEBUG_ASSERT(stream_channel_.get() != channel);
  ZX_DEBUG_ASSERT(rb_channel_.get() == channel);

  {
    fbl::AutoLock req_lock(&req_lock_);
    if (ring_buffer_state_ != RingBufferState::STOPPED) {
      ring_buffer_state_ = RingBufferState::STOPPING;
      state_.Set("stopping_deactivate");
    }
    rb_vmo_fetched_ = false;
    delay_info_updated_ = false;

    //  Any pending completers MUST be released, for the RingBuffer channel to cleanly unwind.
    start_completer_.reset();
    stop_completer_.reset();
    position_completer_.reset();
    delay_completer_.reset();
  }

  rb_channel_.reset();
}

void UsbAudioStream::RingBufferCopy(::usb::FidlRequest& req, bool copy_to, uint32_t todo) {
  if (ring_buffer_size_ == 0 || ring_buffer_offset_ >= ring_buffer_size_) {
    return;
  }
  todo = static_cast<uint32_t>(std::min<uint64_t>({todo, ring_buffer_size_, ifc_->max_req_size()}));
  uint32_t avail = ring_buffer_size_ - ring_buffer_offset_;
  ZX_DEBUG_ASSERT(ring_buffer_offset_ < ring_buffer_size_);
  ZX_DEBUG_ASSERT((avail % frame_size_) == 0);
  uint32_t amt = std::min(avail, todo);

  uint8_t* ptr = reinterpret_cast<uint8_t*>(ring_buffer_virt_) + ring_buffer_offset_;

  uint64_t vmo_size = ifc_->max_req_size();
  size_t cp_size = std::min(vmo_size, static_cast<uint64_t>(amt));
  auto actual = copy_to ? req.CopyTo(0, ptr, cp_size, ep_.GetMapped())
                        : req.CopyFrom(0, ptr, cp_size, ep_.GetMapped());
  size_t total = 0;
  for (uint32_t i = 0; i < actual.size(); i++) {
    total += actual[i];
    if (copy_to) {
      req->data()->at(i).size(actual[i]);
    }
  }
  if (total != cp_size) {
    zxlogf(WARNING, "Wanted to copy %zu bytes, but actually copied %zu bytes", cp_size, total);
  }
  if (amt < todo) {
    cp_size = std::min(vmo_size - amt, static_cast<uint64_t>(todo - amt));
    actual = copy_to ? req.CopyTo(amt, ring_buffer_virt_, cp_size, ep_.GetMapped())
                     : req.CopyFrom(amt, ring_buffer_virt_, cp_size, ep_.GetMapped());
    total = 0;
    for (uint32_t i = 0; i < actual.size(); i++) {
      total += actual[i];
      if (copy_to) {
        req->data()->at(i).size(*req->data()->at(i).size() + actual[i]);
      }
    }
    if (total != cp_size) {
      zxlogf(WARNING, "Wanted to copy %zu bytes, but actually copied %zu bytes", cp_size, total);
    }
  }
}

}  // namespace audio::usb
