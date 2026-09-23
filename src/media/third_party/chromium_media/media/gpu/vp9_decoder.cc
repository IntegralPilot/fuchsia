// Copyright 2015 The Chromium Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "media/gpu/vp9_decoder.h"

#include <memory>

// Fuchsia change: Remove libraries in favor of "chromium_utils.h"
#include "chromium_utils.h"

namespace media {

namespace {
bool GetSpatialLayerFrameSize(const DecoderBuffer& decoder_buffer,
                              std::vector<uint32_t>& frame_sizes) {
  frame_sizes.clear();

  const uint32_t* cue_data =
      reinterpret_cast<const uint32_t*>(decoder_buffer.side_data());
  if (!cue_data)
    return true;

// On windows, currently only d3d11 supports decoding VP9 kSVC stream, we
// shouldn't combine the switch kD3D11Vp9kSVCHWDecoding to kVp9kSVCHWDecoding
// due to we want keep returning false to MediaCapability.
// Fuchsia change: Don't support windows or chromeos
#if 0
  const bool enable_vp9_ksvc =
#if BUILDFLAG(IS_WIN)
      base::FeatureList::IsEnabled(media::kD3D11Vp9kSVCHWDecoding);
#elif BUILDFLAG(IS_CHROMEOS) && defined(ARCH_CPU_ARM_FAMILY)
      // V4L2 stateless API decoder is not capable of decoding VP9 k-SVC stream.
      false;
#else
      base::FeatureList::IsEnabled(media::kVp9kSVCHWDecoding);
#endif

  if (!enable_vp9_ksvc) {
    DLOG(ERROR) << "Vp9 k-SVC hardware decoding is disabled";
    return false;
  }
#endif

  size_t num_of_layers = decoder_buffer.side_data_size() / sizeof(uint32_t);
  if (num_of_layers > 3u) {
    DLOG(WARNING) << "The maximum number of spatial layers in VP9 is three";
    return false;
  }

  frame_sizes = std::vector<uint32_t>(cue_data, cue_data + num_of_layers);
  return true;
}

VideoCodecProfile VP9ProfileToVideoCodecProfile(uint8_t profile) {
  switch (profile) {
    case 0:
      return VP9PROFILE_PROFILE0;
    case 1:
      return VP9PROFILE_PROFILE1;
    case 2:
      return VP9PROFILE_PROFILE2;
    case 3:
      return VP9PROFILE_PROFILE3;
    default:
      return VIDEO_CODEC_PROFILE_UNKNOWN;
  }
}

bool IsValidBitDepth(uint8_t bit_depth, VideoCodecProfile profile) {
  // Spec 7.2.
  switch (profile) {
    case VP9PROFILE_PROFILE0:
    case VP9PROFILE_PROFILE1:
      return bit_depth == 8u;
    case VP9PROFILE_PROFILE2:
    case VP9PROFILE_PROFILE3:
      return bit_depth == 10u || bit_depth == 12u;
    default:
      NOTREACHED();
  }
}

VideoChromaSampling GetVP9ChromaSampling(const Vp9FrameHeader& frame_header) {
  // Spec section 7.2.2
  uint8_t subsampling_x = frame_header.subsampling_x;
  uint8_t subsampling_y = frame_header.subsampling_y;
  if (subsampling_x == 0 && subsampling_y == 0) {
    return VideoChromaSampling::k444;
  } else if (subsampling_x == 1u && subsampling_y == 0u) {
    return VideoChromaSampling::k422;
  } else if (subsampling_x == 1u && subsampling_y == 1u) {
    return VideoChromaSampling::k420;
  } else {
    FX_LOGS(DEBUG) << "Unknown chroma sampling format.";
    return VideoChromaSampling::kUnknown;
  }
}
}  // namespace

VP9Decoder::VP9Accelerator::VP9Accelerator() {}

VP9Decoder::VP9Accelerator::~VP9Accelerator() {}

scoped_refptr<VP9Picture> VP9Decoder::VP9Accelerator::CreateVP9PictureSecure(
    uint64_t secure_handle) {
  return nullptr;
}

VP9Decoder::VP9Decoder(std::unique_ptr<VP9Accelerator> accelerator,
                       VideoCodecProfile profile,
                       const VideoColorSpace& container_color_space)
    : state_(kNeedStreamMetadata),
      container_color_space_(container_color_space),
      // TODO(hiroh): Set profile to UNKNOWN.
      profile_(profile),
      accelerator_(std::move(accelerator)) {}

VP9Decoder::~VP9Decoder() = default;

void VP9Decoder::SetStream(int32_t id,
                           scoped_refptr<DecoderBuffer> decoder_buffer) {
  CHECK(decoder_buffer);
  // Keep the old buffer alive until the end of this function to ensure
  // that any active spans in the parser are cleared before the memory is freed.
  auto outgoing_decoder_buffer = std::move(decoder_buffer_);
  decoder_buffer_ = std::move(decoder_buffer);
  const DecryptConfig* decrypt_config = decoder_buffer_->decrypt_config();

  FX_LOGS(DEBUG) << "New input stream id: " << id
                 << " size: " << decoder_buffer_->size();
  stream_id_ = id;
  std::vector<uint32_t> frame_sizes;
  if (!GetSpatialLayerFrameSize(*decoder_buffer_, frame_sizes)) {
    SetError();
    return;
  }
  // Fuchsia change: Fuchsia does not use secure_handle_ for DRM playback.
  secure_handle_ = 0;

  parser_.SetStream(*decoder_buffer_, frame_sizes,
                    decrypt_config ? decrypt_config->Clone() : nullptr);
}

bool VP9Decoder::Flush() {
  FX_LOGS(DEBUG) << "Decoder flush";
  Reset();
  return true;
}

void VP9Decoder::Reset() {
  curr_frame_hdr_ = nullptr;
  decrypt_config_.reset();
  pending_pic_.reset();

  ref_frames_.Clear();

  parser_.Reset();
  decoder_buffer_.reset();

  secure_handle_ = 0;

  if (state_ == kDecoding) {
    state_ = kAfterReset;
  }
}

VP9Decoder::DecodeResult VP9Decoder::Decode() {
  while (true) {
    if (state_ == kError)
      return kDecodeError;

    // If we have a pending picture to decode, try that first.
    if (pending_pic_) {
      VP9Accelerator::Status status =
          DecodeAndOutputPicture(std::move(pending_pic_));
      if (status == VP9Accelerator::Status::kFail) {
        SetError();
        return kDecodeError;
      }
      if (status == VP9Accelerator::Status::kTryAgain)
        return kTryAgain;
    }

    // Read a new frame header if one is not awaiting decoding already.
    if (!curr_frame_hdr_) {
      gfx::Size allocate_size;
      std::unique_ptr<Vp9FrameHeader> hdr(new Vp9FrameHeader());
      Vp9Parser::Result res =
          parser_.ParseNextFrame(hdr.get(), &allocate_size, &decrypt_config_);
      switch (res) {
        case Vp9Parser::kOk:
          curr_frame_hdr_ = std::move(hdr);
          curr_frame_size_ = allocate_size;
          break;

        case Vp9Parser::kEOStream:
          // Fuchsia change: Release the decoder buffer as soon as all frames in
          // the current bitstream buffer have been parsed and submitted. Note
          // that parser_.Reset() must not be called here so that reference
          // slots and segmentation context persist across frames.
          ZX_DEBUG_ASSERT(parser_.is_stream_empty());
          if (!pending_pic_) {
            decoder_buffer_.reset();
          }
          return kRanOutOfStreamData;

        case Vp9Parser::kInvalidStream:
          FX_LOGS(DEBUG) << "Error parsing stream";
          SetError();
          return kDecodeError;
      }
    }

    if (state_ != kDecoding) {
      // Not kDecoding, so we need a resume point (a keyframe), as we are after
      // reset or at the beginning of the stream. Drop anything that is not
      // a keyframe in such case, and continue looking for a keyframe.
      // Only exception is when the stream/sequence starts with an Intra only
      // frame.
      if (curr_frame_hdr_->IsKeyframe() ||
          (curr_frame_hdr_->IsIntra() && pic_size_.IsEmpty())) {
        state_ = kDecoding;
      } else {
        curr_frame_hdr_.reset();
        decrypt_config_.reset();
        continue;
      }
    }

    if (curr_frame_hdr_->show_existing_frame) {
      // This frame header only instructs us to display one of the
      // previously-decoded frames, but has no frame data otherwise. Display
      // and continue decoding subsequent frames.
      size_t frame_to_show = curr_frame_hdr_->frame_to_show_map_idx;
      if (frame_to_show >= kVp9NumRefFrames ||
          !ref_frames_.GetFrame(frame_to_show)) {
        FX_LOGS(DEBUG) << "Request to show an invalid frame";
        SetError();
        return kDecodeError;
      }

      // Duplicate the VP9Picture and set the current bitstream id to keep the
      // correct timestamp.
      scoped_refptr<VP9Picture> pic =
          ref_frames_.GetFrame(frame_to_show)->Duplicate();
      if (pic == nullptr) {
        FX_LOGS(DEBUG) << "Failed to duplicate the VP9Picture.";
        SetError();
        return kDecodeError;
      }
      pic->set_bitstream_id(stream_id_);
      pic->frame_hdr = std::move(curr_frame_hdr_);
      if (!accelerator_->OutputPicture(std::move(pic))) {
        SetError();
        return kDecodeError;
      }

      decrypt_config_.reset();
      continue;
    }

    gfx::Size new_pic_size = curr_frame_size_;
    gfx::Rect new_render_rect(curr_frame_hdr_->render_width,
                              curr_frame_hdr_->render_height);
    const gfx::Rect frame_size(curr_frame_hdr_->frame_width,
                               curr_frame_hdr_->frame_height);
    if (!new_render_rect.IsValid()) {
      FX_LOGS(DEBUG) << "Invalid render rect: " << new_render_rect.ToString();
      SetError();
      return kDecodeError;
    }
    if (!frame_size.IsValid()) {
      FX_LOGS(DEBUG) << "Invalid frame size: " << frame_size.ToString();
      SetError();
      return kDecodeError;
    }
    ZX_ASSERT(frame_size.IsValid());
    ZX_ASSERT(new_render_rect.IsValid());
    if (!frame_size.Contains(new_render_rect)) {
      // For safety, check the validity of render size or leave it as the actual
      // per-frame decoded size. In the k-SVC path |curr_frame_size_| is the
      // *maximum* layer size (Vp9Parser::ParseSVCFrame stamps allocate_size
      // onto every layer), so clamping against it would allow render_rect to
      // extend into the undecoded region of the surface (b/149727823).
      FX_LOGS(DEBUG) << "Render size exceeds frame size. render size: "
                     << new_render_rect.ToString()
                     << ", frame size: " << frame_size.ToString();
      new_render_rect = frame_size;
    }
    VideoCodecProfile new_profile =
        VP9ProfileToVideoCodecProfile(curr_frame_hdr_->profile);
    if (new_profile == VIDEO_CODEC_PROFILE_UNKNOWN) {
      FX_LOGS(DEBUG) << "Invalid profile: " << curr_frame_hdr_->profile;
      return kDecodeError;
    }
    if (!IsValidBitDepth(curr_frame_hdr_->bit_depth, new_profile)) {
      FX_LOGS(DEBUG) << "Invalid bit depth="
                     << base::strict_cast<int>(curr_frame_hdr_->bit_depth)
                     << ", profile=" << GetProfileName(new_profile);
      return kDecodeError;
    }
    VideoChromaSampling new_chroma_sampling =
        GetVP9ChromaSampling(*curr_frame_hdr_);
    if (new_chroma_sampling != chroma_sampling_) {
      chroma_sampling_ = new_chroma_sampling;
    }

    if (chroma_sampling_ != VideoChromaSampling::k420) {
      FX_LOGS(DEBUG) << "Only YUV 4:2:0 is supported";
      return kDecodeError;
    }

    VideoColorSpace new_color_space;
    // For VP9, container color spaces override video stream color spaces.
    if (container_color_space_.IsSpecified()) {
      new_color_space = container_color_space_;
    } else if (curr_frame_hdr_->GetColorSpace().IsSpecified()) {
      new_color_space = curr_frame_hdr_->GetColorSpace();
    }

    DCHECK(!new_pic_size.IsEmpty());
    const bool is_pic_size_different = new_pic_size != pic_size_;
    const bool is_pic_size_larger = new_pic_size.width() > pic_size_.width() ||
                                    new_pic_size.height() > pic_size_.height();
    const bool is_new_configuration_different_enough =
        (ignore_resolution_changes_to_smaller_for_testing_
             ? is_pic_size_larger
             : is_pic_size_different) ||
        new_profile != profile_ || curr_frame_hdr_->bit_depth != bit_depth_;

    if (is_new_configuration_different_enough) {
      FX_LOGS(DEBUG) << "New profile: " << GetProfileName(new_profile)
                     << ", New resolution: " << new_pic_size.ToString()
                     << ", New bit depth: "
                     << base::strict_cast<int>(curr_frame_hdr_->bit_depth);

      // If frame is a keyframe, reset the decoding process by releasing all the
      // reference frames
      if (curr_frame_hdr_->IsKeyframe()) {
        ref_frames_.Clear();
      }

      pic_size_ = new_pic_size;
      visible_rect_ = new_render_rect;
      profile_ = new_profile;
      bit_depth_ = curr_frame_hdr_->bit_depth;
      picture_color_space_ = new_color_space;
      size_change_failure_counter_ = 0;
      return kConfigChange;
    }

    scoped_refptr<VP9Picture> pic;
    if (secure_handle_) {
      pic = accelerator_->CreateVP9PictureSecure(secure_handle_);
    } else {
      pic = accelerator_->CreateVP9Picture();
    }
    if (!pic) {
      return kRanOutOfSurfaces;
    }
    FX_LOGS(DEBUG) << "Render resolution: " << new_render_rect.ToString();

    pic->set_visible_rect(new_render_rect);
    pic->set_bitstream_id(stream_id_);

    pic->set_decrypt_config(std::move(decrypt_config_));

    // For VP9, container color spaces override video stream color spaces.
    if (container_color_space_.IsSpecified())
      pic->set_colorspace(container_color_space_);
    else if (curr_frame_hdr_)
      pic->set_colorspace(curr_frame_hdr_->GetColorSpace());

    // VP9 only supports HDR metadata from the container.
    pic->SetDynamicHdrMetadata(decoder_buffer_.get());

    // VP9 only supports HDR metadata from the container.
    pic->SetDynamicHdrMetadata(decoder_buffer_.get());

    pic->frame_hdr = std::move(curr_frame_hdr_);

    VP9Accelerator::Status status = DecodeAndOutputPicture(std::move(pic));
    if (status == VP9Accelerator::Status::kFail) {
      SetError();
      return kDecodeError;
    }
    if (status == VP9Accelerator::Status::kTryAgain)
      return kTryAgain;
  }
}

VP9Decoder::VP9Accelerator::Status VP9Decoder::DecodeAndOutputPicture(
    scoped_refptr<VP9Picture> pic) {
  DCHECK(!pic_size_.IsEmpty());
  DCHECK(pic->frame_hdr);

  const Vp9Parser::Context& context = parser_.context();
  VP9Accelerator::Status status = accelerator_->SubmitDecode(
      pic, context.segmentation(), context.loop_filter(), ref_frames_);
  if (status != VP9Accelerator::Status::kOk) {
    if (status == VP9Accelerator::Status::kTryAgain)
      pending_pic_ = std::move(pic);
    return status;
  }

  if (pic->frame_hdr->show_frame) {
    if (!accelerator_->OutputPicture(pic))
      return VP9Accelerator::Status::kFail;
  }

  ref_frames_.Refresh(std::move(pic));
  return status;
}

void VP9Decoder::SetError() {
  Reset();
  state_ = kError;
}

gfx::Size VP9Decoder::GetPicSize() const {
  return pic_size_;
}

gfx::Rect VP9Decoder::GetVisibleRect() const {
  return visible_rect_;
}

VideoCodecProfile VP9Decoder::GetProfile() const {
  return profile_;
}

uint8_t VP9Decoder::GetBitDepth() const {
  return bit_depth_;
}

VideoChromaSampling VP9Decoder::GetChromaSampling() const {
  return chroma_sampling_;
}

VideoColorSpace VP9Decoder::GetVideoColorSpace() const {
  return picture_color_space_;
}

size_t VP9Decoder::GetRequiredNumOfPictures() const {
  constexpr size_t kPicsInPipeline = limits::kMaxVideoFrames + 1;
  return kPicsInPipeline + GetNumReferenceFrames();
}

size_t VP9Decoder::GetNumReferenceFrames() const {
  // Maximum number of reference frames
  return kVp9NumRefFrames;
}

bool VP9Decoder::IsCurrentFrameKeyframe() const {
  return curr_frame_hdr_ && curr_frame_hdr_->IsKeyframe();
}

}  // namespace media
