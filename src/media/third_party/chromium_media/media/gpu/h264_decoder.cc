// Copyright 2012 The Chromium Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "media/gpu/h264_decoder.h"

#include <lib/stdcompat/variant.h>
#include <algorithm>
#include <limits>
#include <memory>
#include <optional>
#include <variant>

#include "media/parsers/h264_level_limits.h"
#include "media/parsers/h26x_parser.h"

namespace media {
namespace {

template <class... Ts>
struct Overloaded : Ts... {
  using Ts::operator()...;
};

// This heuristic is not conformant to the h264 spec.  It is very possible to
// construct an h264 stream that will cause this heuristic to output frames in
// the wrong order near the start of the stream, and potential interaction with
// no_output_of_prior_pics_flag == 1 has not been fully evaluated.
//
// Unfortunately, in some situations, we don't have access to
// max_num_reorder_frames due to HW/FW limitations, so without this heuristic
// we'd be stuck with higher frame decode delay than we'd expect of a decoder
// that pays attention to max_num_reorder_frames == 0.
//
// Even with this heuristic, for a stream with only even POCs, we still need a
// few frames at the start to determine (make a fairly reasonable guess) that
// there aren't any odd POCs, so those frames experience delay if the stream has
// only even POCs.
//
// If the stream POC skips by more than 2 per output frame, the heuristic
// doesn't help reduce latency.
constexpr bool kEnableQuickOutputHeuristic = true;
constexpr uint64_t kNumPocSeenToAssumeOnlyEvenPocUnlessProvenOtherwise = 3;

bool ParseBitDepth(const H264SPS& sps, uint8_t& bit_depth) {
  // Spec 7.4.2.1.1
  if (sps.bit_depth_luma_minus8 != sps.bit_depth_chroma_minus8) {
    FX_LOGS(DEBUG)
        << "H264Decoder doesn't support different bit depths between luma"
        << "and chroma, bit_depth_luma_minus8=" << sps.bit_depth_luma_minus8
        << ", bit_depth_chroma_minus8=" << sps.bit_depth_chroma_minus8;
    return false;
  }
  DCHECK_GE(sps.bit_depth_luma_minus8, 0);
  DCHECK_LE(sps.bit_depth_luma_minus8, 6);
  switch (sps.bit_depth_luma_minus8) {
    case 0:
      bit_depth = 8u;
      break;
    case 2:
      bit_depth = 10u;
      break;
    case 4:
      bit_depth = 12u;
      break;
    case 6:
      bit_depth = 14u;
      break;
    default:
      FX_LOGS(DEBUG) << "Invalid bit depth: "
                     << base::checked_cast<int>(sps.bit_depth_luma_minus8 + 8);
      return false;
  }
  return true;
}

bool IsValidBitDepth(uint8_t bit_depth, VideoCodecProfile profile) {
  // Spec A.2.
  switch (profile) {
    case H264PROFILE_BASELINE:
    case H264PROFILE_MAIN:
    case H264PROFILE_EXTENDED:
    case H264PROFILE_HIGH:
      return bit_depth == 8u;
    case H264PROFILE_HIGH10PROFILE:
    case H264PROFILE_HIGH422PROFILE:
      return bit_depth == 8u || bit_depth == 10u;
    case H264PROFILE_HIGH444PREDICTIVEPROFILE:
      return bit_depth == 8u || bit_depth == 10u || bit_depth == 12u ||
             bit_depth == 14u;
    case H264PROFILE_SCALABLEBASELINE:
    case H264PROFILE_SCALABLEHIGH:
      // Spec G.10.1.
      return bit_depth == 8u;
    case H264PROFILE_STEREOHIGH:
    case H264PROFILE_MULTIVIEWHIGH:
      // Spec H.10.1.1 and H.10.1.2.
      return bit_depth == 8u;
    default:
      NOTREACHED();
  }
}

}  // namespace

H264Decoder::H264Accelerator::H264Accelerator() = default;

H264Decoder::H264Accelerator::~H264Accelerator() = default;

scoped_refptr<H264Picture>
H264Decoder::H264Accelerator::CreateH264PictureSecure(uint64_t secure_handle) {
  return nullptr;
}

void H264Decoder::H264Accelerator::ProcessSPS(
    const H264SPS* sps,
    base::span<const uint8_t> sps_nalu_data) {}

void H264Decoder::H264Accelerator::ProcessPPS(
    const H264PPS* pps,
    base::span<const uint8_t> pps_nalu_data) {}

H264Decoder::H264Accelerator::Status
H264Decoder::H264Accelerator::ParseEncryptedSliceHeader(
    const std::vector<base::span<const uint8_t>>& data,
    const std::vector<SubsampleEntry>& subsamples,
    uint64_t secure_handle,
    H264SliceHeader* slice_header_out) {
  return H264Decoder::H264Accelerator::Status::kNotSupported;
}

H264Decoder::H264Accelerator::Status H264Decoder::H264Accelerator::SetStream(
    base::span<const uint8_t> stream,
    const DecryptConfig* decrypt_config) {
  return H264Decoder::H264Accelerator::Status::kNotSupported;
}

// Fuchsia change: By default in Fuchsia, H264 accelerators require reference
// lists.
bool H264Decoder::H264Accelerator::RequiresRefLists() {
  return true;
}

H264Decoder::H264Decoder(std::unique_ptr<H264Accelerator> accelerator,
                         VideoCodecProfile profile,
                         const VideoColorSpace& container_color_space)
    : state_(State::kNeedStreamMetadata),
      container_color_space_(container_color_space),
      max_frame_num_(0),
      max_pic_num_(0),
      max_long_term_frame_idx_(0),
      max_num_reorder_frames_(0),
      // TODO(hiroh): Set profile to UNKNOWN.
      profile_(profile),
      accelerator_(std::move(accelerator)),
      requires_ref_lists_(accelerator_->RequiresRefLists()) {
  DCHECK(accelerator_);
  Reset();
}

H264Decoder::~H264Decoder() = default;

void H264Decoder::Reset() {
  curr_pic_ = nullptr;
  curr_nalu_.reset();
  curr_slice_hdr_ = nullptr;
  FX_LOGS(DEBUG) << "curr_sps_id_ = -1";
  curr_sps_id_ = -1;
  curr_pps_id_ = -1;

  prev_frame_num_ = -1;
  prev_ref_frame_num_ = -1;
  prev_frame_num_offset_ = -1;
  prev_has_memmgmnt5_ = false;

  prev_ref_has_memmgmnt5_ = false;
  prev_ref_top_field_order_cnt_ = -1;
  prev_ref_pic_order_cnt_msb_ = -1;
  prev_ref_pic_order_cnt_lsb_ = -1;
  prev_ref_field_ = H264Picture::FIELD_NONE;

  ref_pic_list_p0_.clear();
  ref_pic_list_b0_.clear();
  ref_pic_list_b1_.clear();
  dpb_.Clear();
  parser_.Reset();
  accelerator_->Reset();
  last_output_poc_ = std::numeric_limits<int>::min();

  prior_cencv1_nalus_.clear();
  prior_cencv1_subsamples_.clear();

  recovery_frame_num_.reset();
  recovery_frame_cnt_.reset();

  num_poc_seen_ = 0;
  odd_poc_seen_ = false;

  decoder_buffer_.reset();
  secure_handle_ = 0;

  // If we have already parsed stream metadata, we can resume without processing
  // an SPS. The state becomes kDecoding again, (1) at the first IDR slice or
  // (2) at the first slice after the recovery point SEI.
  if (state_ != State::kNeedStreamMetadata && state_ != State::kError) {
    state_ = State::kAfterReset;
  }
}

void H264Decoder::PrepareRefPicLists() {
  ConstructReferencePicListsP();
  ConstructReferencePicListsB();
}

bool H264Decoder::ModifyReferencePicLists(const H264SliceHeader* slice_hdr,
                                          H264Picture::Vector* ref_pic_list0,
                                          H264Picture::Vector* ref_pic_list1) {
  ref_pic_list0->clear();
  ref_pic_list1->clear();

  // Fill reference picture lists for B and S/SP slices.
  if (slice_hdr->IsPSlice() || slice_hdr->IsSPSlice()) {
    *ref_pic_list0 = ref_pic_list_p0_;
    FX_LOGS(DEBUG) << "ModifyReferencePicLists P case";
    return ModifyReferencePicList(slice_hdr, 0, ref_pic_list0);
  } else if (slice_hdr->IsBSlice()) {
    *ref_pic_list0 = ref_pic_list_b0_;
    *ref_pic_list1 = ref_pic_list_b1_;
    FX_LOGS(DEBUG) << "ModifyReferencePicLists B case";
    return ModifyReferencePicList(slice_hdr, 0, ref_pic_list0) &&
           ModifyReferencePicList(slice_hdr, 1, ref_pic_list1);
  }

  return true;
}

void H264Decoder::StoreDPBPicForTesting(scoped_refptr<H264Picture> p) {
  dpb_.set_max_num_pics(dpb_.max_num_pics() + 1);
  dpb_.StorePic(p);
}

bool H264Decoder::ModifyReferencePicListsForTesting(
    const H264SliceHeader* slice_hdr,
    H264Picture::Vector* ref_pic_list0,
    H264Picture::Vector* ref_pic_list1) {
  if (slice_hdr->IsPSlice() || slice_hdr->IsSPSlice()) {
    ref_pic_list_p0_ = *ref_pic_list0;
  } else {
    ref_pic_list_b0_ = *ref_pic_list0;
    ref_pic_list_b1_ = *ref_pic_list1;
  }
  return ModifyReferencePicLists(slice_hdr, ref_pic_list0, ref_pic_list1);
}

H264Decoder::H264Accelerator::Status H264Decoder::DecodePicture() {
  DCHECK(curr_pic_.get());

  return accelerator_->SubmitDecode(curr_pic_);
}

bool H264Decoder::InitNonexistingPicture(scoped_refptr<H264Picture> pic,
                                         int frame_num) {
  pic->nonexisting = true;
  pic->nal_ref_idc = 1;
  pic->frame_num = pic->pic_num = frame_num;
  pic->adaptive_ref_pic_marking_mode_flag = false;
  pic->ref = true;
  pic->long_term_reference_flag = false;
  pic->field = H264Picture::FIELD_NONE;

  return CalculatePicOrderCounts(pic);
}

bool H264Decoder::InitCurrPicture(const H264SliceHeader* slice_hdr) {
  FX_LOGS(DEBUG) << "curr_sps_id_: " << curr_sps_id_;
  if (!FillH264PictureFromSliceHeader(parser_.GetSPS(curr_sps_id_), *slice_hdr,
                                      curr_pic_.get())) {
    return false;
  }

  if (!CalculatePicOrderCounts(curr_pic_))
    return false;

  curr_pic_->long_term_reference_flag = slice_hdr->long_term_reference_flag;
  curr_pic_->adaptive_ref_pic_marking_mode_flag =
      slice_hdr->adaptive_ref_pic_marking_mode_flag;

  // If the slice header indicates we will have to perform reference marking
  // process after this picture is decoded, store required data for that
  // purpose.
  if (slice_hdr->adaptive_ref_pic_marking_mode_flag) {
    curr_pic_->ref_pic_marking = slice_hdr->ref_pic_marking;
  }

  curr_pic_->set_visible_rect(visible_rect_);
  curr_pic_->set_bitstream_id(stream_id_);

  return true;
}

bool H264Decoder::CalculatePicOrderCounts(scoped_refptr<H264Picture> pic) {
  FX_LOGS(DEBUG) << "curr_sps_id_: " << curr_sps_id_;
  const H264SPS* sps = parser_.GetSPS(curr_sps_id_);
  if (!sps)
    return false;

  switch (pic->pic_order_cnt_type) {
    case 0: {
      // See spec 8.2.1.1.
      int prev_pic_order_cnt_msb, prev_pic_order_cnt_lsb;

      if (pic->idr) {
        prev_pic_order_cnt_msb = prev_pic_order_cnt_lsb = 0;
      } else {
        if (prev_ref_has_memmgmnt5_) {
          prev_pic_order_cnt_msb = 0;
          prev_pic_order_cnt_lsb = prev_ref_top_field_order_cnt_;
        } else {
          prev_pic_order_cnt_msb = prev_ref_pic_order_cnt_msb_;
          prev_pic_order_cnt_lsb = prev_ref_pic_order_cnt_lsb_;
        }
      }

      int max_pic_order_cnt_lsb =
          1 << (sps->log2_max_pic_order_cnt_lsb_minus4 + 4);
      DCHECK_NE(max_pic_order_cnt_lsb, 0);
      base::CheckedNumeric<int> pic_order_cnt_msb = prev_pic_order_cnt_msb;
      if ((pic->pic_order_cnt_lsb < prev_pic_order_cnt_lsb) &&
          (prev_pic_order_cnt_lsb - pic->pic_order_cnt_lsb >=
           max_pic_order_cnt_lsb / 2)) {
        pic_order_cnt_msb += max_pic_order_cnt_lsb;
      } else if ((pic->pic_order_cnt_lsb > prev_pic_order_cnt_lsb) &&
                 (pic->pic_order_cnt_lsb - prev_pic_order_cnt_lsb >
                  max_pic_order_cnt_lsb / 2)) {
        pic_order_cnt_msb -= max_pic_order_cnt_lsb;
      }

      base::CheckedNumeric<int> top_field_order_cnt =
          pic_order_cnt_msb + pic->pic_order_cnt_lsb;
      base::CheckedNumeric<int> bottom_field_order_cnt =
          top_field_order_cnt + pic->delta_pic_order_cnt_bottom;

      if (!top_field_order_cnt.IsValid() || !bottom_field_order_cnt.IsValid()) {
        DVLOG(1) << "Invalid pic_order_cnt (type 0).";
        return false;
      }

      pic->pic_order_cnt_msb = pic_order_cnt_msb.ValueOrDie();
      pic->top_field_order_cnt = top_field_order_cnt.ValueOrDie();
      pic->bottom_field_order_cnt = bottom_field_order_cnt.ValueOrDie();

      break;
    }

    case 1: {
      // See spec 8.2.1.2.
      if (prev_has_memmgmnt5_)
        prev_frame_num_offset_ = 0;

      if (pic->idr)
        pic->frame_num_offset = 0;
      else if (prev_frame_num_ > pic->frame_num)
        pic->frame_num_offset = prev_frame_num_offset_ + max_frame_num_;
      else
        pic->frame_num_offset = prev_frame_num_offset_;

      int abs_frame_num = 0;
      if (sps->num_ref_frames_in_pic_order_cnt_cycle != 0)
        abs_frame_num = pic->frame_num_offset + pic->frame_num;
      else
        abs_frame_num = 0;

      if (pic->nal_ref_idc == 0 && abs_frame_num > 0)
        --abs_frame_num;

      base::CheckedNumeric<int> expected_pic_order_cnt = 0;
      if (abs_frame_num > 0) {
        if (sps->num_ref_frames_in_pic_order_cnt_cycle == 0) {
          FX_LOGS(DEBUG) << "Invalid num_ref_frames_in_pic_order_cnt_cycle "
                         << "in stream";
          return false;
        }

        int pic_order_cnt_cycle_cnt =
            (abs_frame_num - 1) / sps->num_ref_frames_in_pic_order_cnt_cycle;
        int frame_num_in_pic_order_cnt_cycle =
            (abs_frame_num - 1) % sps->num_ref_frames_in_pic_order_cnt_cycle;

        expected_pic_order_cnt =
            base::CheckedNumeric<int>(pic_order_cnt_cycle_cnt) *
            sps->expected_delta_per_pic_order_cnt_cycle;

        // frame_num_in_pic_order_cnt_cycle is verified < 255 in parser
        for (int i = 0; i <= frame_num_in_pic_order_cnt_cycle; ++i)
          expected_pic_order_cnt += sps->offset_for_ref_frame[i];
      }

      if (!pic->nal_ref_idc)
        expected_pic_order_cnt += sps->offset_for_non_ref_pic;

      base::CheckedNumeric<int> top_field_order_cnt =
          expected_pic_order_cnt + pic->delta_pic_order_cnt0;
      base::CheckedNumeric<int> bottom_field_order_cnt =
          top_field_order_cnt + sps->offset_for_top_to_bottom_field +
          pic->delta_pic_order_cnt1;

      if (!top_field_order_cnt.IsValid()) {
        DVLOG(1) << "Invalid top_field_order_cnt.";
        return false;
      }

      if (!bottom_field_order_cnt.IsValid()) {
        DVLOG(1) << "Invalid bottom_field_order_cnt.";
        return false;
      }

      pic->top_field_order_cnt = top_field_order_cnt.ValueOrDie();
      pic->bottom_field_order_cnt = bottom_field_order_cnt.ValueOrDie();
      break;
    }

    case 2: {
      // See spec 8.2.1.3.
      if (prev_has_memmgmnt5_)
        prev_frame_num_offset_ = 0;

      if (pic->idr)
        pic->frame_num_offset = 0;
      else if (prev_frame_num_ > pic->frame_num)
        pic->frame_num_offset = prev_frame_num_offset_ + max_frame_num_;
      else
        pic->frame_num_offset = prev_frame_num_offset_;

      int temp_pic_order_cnt;
      if (pic->idr) {
        temp_pic_order_cnt = 0;
      } else if (!pic->nal_ref_idc) {
        temp_pic_order_cnt = 2 * (pic->frame_num_offset + pic->frame_num) - 1;
      } else {
        temp_pic_order_cnt = 2 * (pic->frame_num_offset + pic->frame_num);
      }

      pic->top_field_order_cnt = temp_pic_order_cnt;
      pic->bottom_field_order_cnt = temp_pic_order_cnt;

      break;
    }

    default:
      FX_LOGS(DEBUG) << "Invalid pic_order_cnt_type: "
                     << sps->pic_order_cnt_type;
      return false;
  }

  pic->pic_order_cnt =
      std::min(pic->top_field_order_cnt, pic->bottom_field_order_cnt);

  ++num_poc_seen_;
  if (pic->pic_order_cnt & 0x1) {
    odd_poc_seen_ = true;
  }

  return true;
}

void H264Decoder::UpdatePicNums(int frame_num) {
  for (auto& pic : dpb_) {
    if (!pic->ref)
      continue;

    // 8.2.4.1. Assumes non-interlaced stream.
    DCHECK_EQ(pic->field, H264Picture::FIELD_NONE);
    if (pic->long_term) {
      pic->long_term_pic_num = pic->long_term_frame_idx;
    } else {
      if (pic->frame_num > frame_num)
        pic->frame_num_wrap = pic->frame_num - max_frame_num_;
      else
        pic->frame_num_wrap = pic->frame_num;

      pic->pic_num = pic->frame_num_wrap;
    }
  }
}

struct PicNumDescCompare {
  bool operator()(const scoped_refptr<H264Picture>& a,
                  const scoped_refptr<H264Picture>& b) const {
    return a->pic_num > b->pic_num;
  }
};

struct LongTermPicNumAscCompare {
  bool operator()(const scoped_refptr<H264Picture>& a,
                  const scoped_refptr<H264Picture>& b) const {
    return a->long_term_pic_num < b->long_term_pic_num;
  }
};

void H264Decoder::ConstructReferencePicListsP() {
  // RefPicList0 (8.2.4.2.1) [[1] [2]], where:
  // [1] shortterm ref pics sorted by descending pic_num,
  // [2] longterm ref pics by ascending long_term_pic_num.
  ref_pic_list_p0_.clear();

  // First get the short ref pics...
  dpb_.GetShortTermRefPicsAppending(&ref_pic_list_p0_);
  size_t num_short_refs = ref_pic_list_p0_.size();

  // and sort them to get [1].
  std::sort(ref_pic_list_p0_.begin(), ref_pic_list_p0_.end(),
            PicNumDescCompare());

  // Now get long term pics and sort them by long_term_pic_num to get [2].
  dpb_.GetLongTermRefPicsAppending(&ref_pic_list_p0_);
  std::sort(ref_pic_list_p0_.begin() + num_short_refs, ref_pic_list_p0_.end(),
            LongTermPicNumAscCompare());
}

struct POCAscCompare {
  bool operator()(const scoped_refptr<H264Picture>& a,
                  const scoped_refptr<H264Picture>& b) const {
    return a->pic_order_cnt < b->pic_order_cnt;
  }

  bool operator()(const H264Picture* a, const H264Picture* b) const {
    return a->pic_order_cnt < b->pic_order_cnt;
  }

  bool operator()(const scoped_refptr<H264Picture>& a,
                  const H264Picture* b) const {
    return a->pic_order_cnt < b->pic_order_cnt;
  }

  bool operator()(const H264Picture* a,
                  const scoped_refptr<H264Picture>& b) const {
    return a->pic_order_cnt < b->pic_order_cnt;
  }
};

struct POCDescCompare {
  bool operator()(const scoped_refptr<H264Picture>& a,
                  const scoped_refptr<H264Picture>& b) const {
    return a->pic_order_cnt > b->pic_order_cnt;
  }
  bool operator()(const H264Picture* a, const H264Picture* b) const {
    return a->pic_order_cnt > b->pic_order_cnt;
  }
  bool operator()(const scoped_refptr<H264Picture>& a,
                  const H264Picture* b) const {
    return a->pic_order_cnt > b->pic_order_cnt;
  }
  bool operator()(const H264Picture* a,
                  const scoped_refptr<H264Picture>& b) const {
    return a->pic_order_cnt > b->pic_order_cnt;
  }
};

void H264Decoder::ConstructReferencePicListsB() {
  // RefPicList0 (8.2.4.2.3) [[1] [2] [3]], where:
  // [1] shortterm ref pics with POC < curr_pic's POC sorted by descending POC,
  // [2] shortterm ref pics with POC > curr_pic's POC by ascending POC,
  // [3] longterm ref pics by ascending long_term_pic_num.
  ref_pic_list_b0_.clear();
  ref_pic_list_b1_.clear();
  dpb_.GetShortTermRefPicsAppending(&ref_pic_list_b0_);
  size_t num_short_refs = ref_pic_list_b0_.size();

  // First sort ascending, this will put [1] in right place and finish [2].
  std::sort(ref_pic_list_b0_.begin(), ref_pic_list_b0_.end(), POCAscCompare());

  // Find first with POC > curr_pic's POC to get first element in [2]...
  H264Picture::Vector::iterator iter;
  iter = std::upper_bound(ref_pic_list_b0_.begin(), ref_pic_list_b0_.end(),
                          curr_pic_.get(), POCAscCompare());

  // and sort [1] descending, thus finishing sequence [1] [2].
  std::sort(ref_pic_list_b0_.begin(), iter, POCDescCompare());

  // Now add [3] and sort by ascending long_term_pic_num.
  dpb_.GetLongTermRefPicsAppending(&ref_pic_list_b0_);
  std::sort(ref_pic_list_b0_.begin() + num_short_refs, ref_pic_list_b0_.end(),
            LongTermPicNumAscCompare());

  // RefPicList1 (8.2.4.2.4) [[1] [2] [3]], where:
  // [1] shortterm ref pics with POC > curr_pic's POC sorted by ascending POC,
  // [2] shortterm ref pics with POC < curr_pic's POC by descending POC,
  // [3] longterm ref pics by ascending long_term_pic_num.

  dpb_.GetShortTermRefPicsAppending(&ref_pic_list_b1_);
  num_short_refs = ref_pic_list_b1_.size();

  // First sort by descending POC.
  std::sort(ref_pic_list_b1_.begin(), ref_pic_list_b1_.end(), POCDescCompare());

  // Find first with POC < curr_pic's POC to get first element in [2]...
  iter = std::upper_bound(ref_pic_list_b1_.begin(), ref_pic_list_b1_.end(),
                          curr_pic_.get(), POCDescCompare());

  // and sort [1] ascending.
  std::sort(ref_pic_list_b1_.begin(), iter, POCAscCompare());

  // Now add [3] and sort by ascending long_term_pic_num
  dpb_.GetLongTermRefPicsAppending(&ref_pic_list_b1_);
  std::sort(ref_pic_list_b1_.begin() + num_short_refs, ref_pic_list_b1_.end(),
            LongTermPicNumAscCompare());

  // If lists identical, swap first two entries in RefPicList1 (spec 8.2.4.2.3)
  if (ref_pic_list_b1_.size() > 1 &&
      std::ranges::equal(ref_pic_list_b0_, ref_pic_list_b1_)) {
    std::swap(ref_pic_list_b1_[0], ref_pic_list_b1_[1]);
  }
}

// See 8.2.4
int H264Decoder::PicNumF(const H264Picture& pic) {
  if (!pic.long_term)
    return pic.pic_num;
  else
    return max_pic_num_;
}

// See 8.2.4
int H264Decoder::LongTermPicNumF(const H264Picture& pic) {
  if (pic.ref && pic.long_term)
    return pic.long_term_pic_num;
  else
    return 2 * (max_long_term_frame_idx_ + 1);
}

// Shift elements on the |v| starting from |from| to |to|, inclusive,
// one position to the right and insert pic at |from|.
static void ShiftRightAndInsert(H264Picture::Vector* v,
                                int from,
                                int to,
                                scoped_refptr<H264Picture> pic) {
  // Security checks, do not disable in Debug mode.
  //
  // TODO(https://fxbug.dev/42084549): These need to not abort() - instead just
  // failing the stream.  While we're at it, we should fix the comment just
  // above.
  CHECK(from <= to);
  CHECK(to <= std::numeric_limits<int>::max() - 2);
  // Additional checks. Debug mode ok.
  DCHECK(v);
  DCHECK(pic);
  DCHECK((to + 1 == static_cast<int>(v->size())) ||
         (to + 2 == static_cast<int>(v->size())));

  v->resize(to + 2);

  for (int i = to + 1; i > from; --i)
    (*v)[i] = (*v)[i - 1];

  (*v)[from] = std::move(pic);
}

bool H264Decoder::ModifyReferencePicList(const H264SliceHeader* slice_hdr,
                                         int list,
                                         H264Picture::Vector* ref_pic_listx) {
  FX_LOGS(DEBUG) << "ModifyReferencePicList list: " << list;
  base::span<const H264ModificationOfPicNum> list_mod;
  bool ref_pic_list_modification_flag_lX;
  int num_ref_idx_lX_active_minus1;

  // This can process either ref_pic_list0 or ref_pic_list1, depending on
  // the list argument. Set up pointers to proper list to be processed here.
  if (list == 0) {
    ref_pic_list_modification_flag_lX =
        slice_hdr->ref_pic_list_modification_flag_l0;
    num_ref_idx_lX_active_minus1 = slice_hdr->num_ref_idx_l0_active_minus1;
    list_mod = slice_hdr->ref_list_l0_modifications;
  } else {
    ref_pic_list_modification_flag_lX =
        slice_hdr->ref_pic_list_modification_flag_l1;
    num_ref_idx_lX_active_minus1 = slice_hdr->num_ref_idx_l1_active_minus1;
    list_mod = slice_hdr->ref_list_l1_modifications;
  }
  FX_LOGS(DEBUG) << "ref_pic_list_modification_flag_lX: "
                 << ref_pic_list_modification_flag_lX
                 << " num_ref_idx_lX_active_minus1: "
                 << num_ref_idx_lX_active_minus1;

  // Resize the list to the size requested in the slice header.
  // Note that per 8.2.4.2 it's possible for num_ref_idx_lX_active_minus1 to
  // indicate there should be more ref pics on list than we constructed.
  // Those superfluous ones should be treated as non-reference and will be
  // initialized to nullptr, which must be handled by clients.
  DCHECK_GE(num_ref_idx_lX_active_minus1, 0);
  ref_pic_listx->resize(num_ref_idx_lX_active_minus1 + 1);

  if (!ref_pic_list_modification_flag_lX)
    return true;

  // Spec 8.2.4.3:
  // Reorder pictures on the list in a way specified in the stream.
  int pic_num_lx_pred = curr_pic_->pic_num;
  int ref_idx_lx = 0;
  int pic_num_lx_no_wrap;
  int pic_num_lx;
  bool done = false;
  scoped_refptr<H264Picture> pic;
  for (size_t i = 0; i < list_mod.size() && !done; ++i) {
    const auto& mod = list_mod[i];
    FX_LOGS(DEBUG) << " list_mod->modification_of_pic_nums_idc: "
                   << mod.modification_of_pic_nums_idc << " i: " << i;
    switch (mod.modification_of_pic_nums_idc) {
      case 0:
      case 1:
        // Modify short reference picture position.
        if (mod.modification_of_pic_nums_idc == 0) {
          // Subtract given value from predicted PicNum.
          pic_num_lx_no_wrap =
              pic_num_lx_pred -
              (static_cast<int>(mod.abs_diff_pic_num_minus1) + 1);
          FX_LOGS(DEBUG) << "idc 0: pic_num_lx_no_wrap: " << pic_num_lx_no_wrap
                         << " pic_num_lx_pred: " << pic_num_lx_pred
                         << " abs_diff_pic_num_minus1: "
                         << mod.abs_diff_pic_num_minus1;
          // Wrap around max_pic_num_ if it becomes < 0 as result
          // of subtraction.
          if (pic_num_lx_no_wrap < 0)
            pic_num_lx_no_wrap += max_pic_num_;
          FX_LOGS(DEBUG) << "idc 0: pic_num_lx_no_wrap: " << pic_num_lx_no_wrap;
        } else {
          // Add given value to predicted PicNum.
          pic_num_lx_no_wrap =
              pic_num_lx_pred +
              (static_cast<int>(mod.abs_diff_pic_num_minus1) + 1);
          FX_LOGS(DEBUG) << "idc 1: pic_num_lx_no_wrap: " << pic_num_lx_no_wrap
                         << " pic_num_lx_pred: " << pic_num_lx_pred
                         << " abs_diff_pic_num_minus1: "
                         << mod.abs_diff_pic_num_minus1;
          // Wrap around max_pic_num_ if it becomes >= max_pic_num_ as result
          // of the addition.
          if (pic_num_lx_no_wrap >= max_pic_num_)
            pic_num_lx_no_wrap -= max_pic_num_;
          FX_LOGS(DEBUG) << "idc 0: pic_num_lx_no_wrap: " << pic_num_lx_no_wrap;
        }

        // For use in next iteration.
        pic_num_lx_pred = pic_num_lx_no_wrap;

        if (pic_num_lx_no_wrap > curr_pic_->pic_num)
          pic_num_lx = pic_num_lx_no_wrap - max_pic_num_;
        else
          pic_num_lx = pic_num_lx_no_wrap;
        FX_LOGS(DEBUG) << "pic_num_lx: " << pic_num_lx;

        DCHECK_LT(num_ref_idx_lX_active_minus1 + 1,
                  H264SliceHeader::kRefListModSize);
        pic = dpb_.GetShortRefPicByPicNum(pic_num_lx);
        if (!pic) {
          FX_LOGS(DEBUG) << "Malformed stream, no pic num " << pic_num_lx;
          return false;
        }

        if (ref_idx_lx > num_ref_idx_lX_active_minus1) {
          FX_LOGS(DEBUG) << "Bounds mismatch: expected " << ref_idx_lx
                         << " <= " << num_ref_idx_lX_active_minus1;
          return false;
        }

        ShiftRightAndInsert(ref_pic_listx, ref_idx_lx,
                            num_ref_idx_lX_active_minus1, pic);
        ref_idx_lx++;

        for (int src = ref_idx_lx, dst = ref_idx_lx;
             src <= num_ref_idx_lX_active_minus1 + 1; ++src) {
          auto* src_pic = (*ref_pic_listx)[src].get();
          int src_pic_num_lx = src_pic ? PicNumF(*src_pic) : -1;
          if (src_pic_num_lx != pic_num_lx)
            (*ref_pic_listx)[dst++] = (*ref_pic_listx)[src];
        }
        break;

      case 2:
        // Modify long term reference picture position.
        DCHECK_LT(num_ref_idx_lX_active_minus1 + 1,
                  H264SliceHeader::kRefListModSize);
        FX_LOGS(DEBUG) << "idc 2: list_mod->long_term_pic_num: "
                       << mod.long_term_pic_num;
        pic = dpb_.GetLongRefPicByLongTermPicNum(mod.long_term_pic_num);
        if (!pic) {
          FX_LOGS(DEBUG) << "Malformed stream, no pic num "
                         << mod.long_term_pic_num;
          return false;
        }
        ShiftRightAndInsert(ref_pic_listx, ref_idx_lx,
                            num_ref_idx_lX_active_minus1, pic);
        ref_idx_lx++;

        for (int src = ref_idx_lx, dst = ref_idx_lx;
             src <= num_ref_idx_lX_active_minus1 + 1; ++src) {
          auto* picptr = (*ref_pic_listx)[src].get();
          if (!picptr || (LongTermPicNumF(*picptr) !=
                          static_cast<int>(mod.long_term_pic_num))) {
            (*ref_pic_listx)[dst++] = (*ref_pic_listx)[src];
          }
        }
        break;

      case 3:
        // End of modification list.
        FX_LOGS(DEBUG) << "idc 3 done - i: " << i;
        done = true;
        break;

      default:
        // May be recoverable.
        FX_LOGS(DEBUG) << "Invalid modification_of_pic_nums_idc="
                       << mod.modification_of_pic_nums_idc << " in position "
                       << i;
        break;
    }
  }

  // Per NOTE 2 in 8.2.4.3.2, the ref_pic_listx size in the above loop is
  // temporarily made one element longer than the required final list.
  // Resize the list back to its required size.
  ref_pic_listx->resize(num_ref_idx_lX_active_minus1 + 1);

  return true;
}

bool H264Decoder::OutputPic(scoped_refptr<H264Picture> pic) {
  DCHECK(!pic->outputted);
  pic->outputted = true;

  VideoColorSpace colorspace_for_frame = container_color_space_;
  FX_LOGS(DEBUG) << "curr_sps_id_: " << curr_sps_id_;
  const H264SPS* sps = parser_.GetSPS(curr_sps_id_);
  if (sps && sps->GetColorSpace().IsSpecified())
    colorspace_for_frame = sps->GetColorSpace();
  pic->set_colorspace(colorspace_for_frame);

  if (pic->nonexisting) {
    FX_LOGS(DEBUG) << "Skipping output, non-existing frame_num: "
                   << pic->frame_num;
    return true;
  }
  last_output_poc_ = pic->pic_order_cnt;

  FX_LOGS(DEBUG) << "Posting output task for POC: " << pic->pic_order_cnt;
  return accelerator_->OutputPicture(pic);
}

void H264Decoder::ClearDPB() {
  // Clear DPB contents, marking the pictures as unused first.
  dpb_.Clear();
  last_output_poc_ = std::numeric_limits<int>::min();
}

bool H264Decoder::OutputAllRemainingPics() {
  // Output all pictures that are waiting to be outputted.
  if (FinishPrevFrameIfPresent() != H264Accelerator::Status::kOk)
    return false;

  H264Picture::Vector to_output;
  dpb_.GetNotOutputtedPicsAppending(&to_output);
  // Sort them by ascending POC to output in order.
  std::sort(to_output.begin(), to_output.end(), POCAscCompare());

  for (auto& pic : to_output) {
    if (!OutputPic(pic))
      return false;
  }
  return true;
}

bool H264Decoder::Flush() {
  FX_LOGS(DEBUG) << "Decoder flush";

  if (!OutputAllRemainingPics())
    return false;

  ClearDPB();
  FX_LOGS(DEBUG) << "Decoder flush finished";
  return true;
}

H264Decoder::H264Accelerator::Status H264Decoder::StartNewFrame(
    const H264SliceHeader* slice_hdr) {
  // TODO posciak: add handling of max_num_ref_frames per spec.
  CHECK(curr_pic_.get());
  DCHECK(slice_hdr);

  curr_pps_id_ = slice_hdr->pic_parameter_set_id;
  FX_LOGS(DEBUG) << "curr_pps_id_ = " << curr_pps_id_;
  const H264PPS* pps = parser_.GetPPS(curr_pps_id_);
  if (!pps)
    return H264Accelerator::Status::kFail;

  curr_sps_id_ = pps->seq_parameter_set_id;
  FX_LOGS(DEBUG) << "curr_sps_id_ = " << curr_sps_id_;
  FX_LOGS(DEBUG) << "curr_sps_id_: " << curr_sps_id_;
  const H264SPS* sps = parser_.GetSPS(curr_sps_id_);
  if (!sps)
    return H264Accelerator::Status::kFail;

  max_frame_num_ = 1 << (sps->log2_max_frame_num_minus4 + 4);
  int frame_num = slice_hdr->frame_num;
  if (slice_hdr->idr_pic_flag)
    prev_ref_frame_num_ = 0;

  // 7.4.3
  if (frame_num != prev_ref_frame_num_ &&
      frame_num != (prev_ref_frame_num_ + 1) % max_frame_num_) {
    H264Accelerator::Status handle_gap_status = HandleFrameNumGap(frame_num);
    if (handle_gap_status != H264Accelerator::Status::kOk) {
      return handle_gap_status;
    }
  }

  if (!InitCurrPicture(slice_hdr))
    return H264Accelerator::Status::kFail;

  UpdatePicNums(frame_num);

  if (requires_ref_lists_) {
    PrepareRefPicLists();
  }

  return accelerator_->SubmitFrameMetadata(sps, pps, dpb_, ref_pic_list_p0_,
                                           ref_pic_list_b0_, ref_pic_list_b1_,
                                           curr_pic_);
}

bool H264Decoder::HandleMemoryManagementOps(scoped_refptr<H264Picture> pic) {
  // 8.2.5.4
  for (size_t i = 0; i < std::size(pic->ref_pic_marking); ++i) {
    // Code below does not support interlaced stream (per-field pictures).
    H264DecRefPicMarking* ref_pic_marking = &pic->ref_pic_marking[i];
    scoped_refptr<H264Picture> to_mark;
    int pic_num_x;

    switch (ref_pic_marking->memory_mgmnt_control_operation) {
      case 0:
        // Normal end of operations' specification.
        return true;

      case 1:
        // Mark a short term reference picture as unused so it can be removed
        // if outputted.
        pic_num_x =
            pic->pic_num - (ref_pic_marking->difference_of_pic_nums_minus1 + 1);
        to_mark = dpb_.GetShortRefPicByPicNum(pic_num_x);
        if (to_mark) {
          to_mark->ref = false;
        } else {
          // |to_mark| may be null for a variety of reasons. For example, the
          // video frame it refers to may have been dropped by the network, or
          // the bitstream is non-conformant and the frame it refers to is
          // already marked as "unused for reference," etc. In any case, it
          // should be safe to ignore this case and continue processing further
          // memory management control operations since the frame won't be used
          // for reference after this in any case.
          //
          // In real life, this case was observed in https://crbug.com/1394965.
          FX_LOGS(DEBUG) << "Invalid short ref pic num to unmark";
        }
        break;

      case 2:
        // Mark a long term reference picture as unused so it can be removed
        // if outputted.
        to_mark = dpb_.GetLongRefPicByLongTermPicNum(
            ref_pic_marking->long_term_pic_num);
        if (to_mark) {
          to_mark->ref = false;
        } else {
          // TODO(crbug.com/40251206): consider doing the same for mmco 2 when
          // we can have testing for it, as how we handle missing |to_mark| for
          // mmco 1.
          FX_LOGS(DEBUG) << "Invalid long term ref pic num to unmark";
          return false;
        }
        break;

      case 3:
        // Mark a short term reference picture as long term reference.
        pic_num_x =
            pic->pic_num - (ref_pic_marking->difference_of_pic_nums_minus1 + 1);
        to_mark = dpb_.GetShortRefPicByPicNum(pic_num_x);
        if (to_mark) {
          DCHECK(to_mark->ref && !to_mark->long_term);

          scoped_refptr<H264Picture> long_term_mark =
              dpb_.GetLongRefPicByLongTermIdx(
                  ref_pic_marking->long_term_frame_idx);
          if (long_term_mark) {
            long_term_mark->ref = false;
          }

          to_mark->long_term = true;
          to_mark->long_term_frame_idx = ref_pic_marking->long_term_frame_idx;
        } else {
          FX_LOGS(DEBUG)
              << "Invalid short term ref pic num to mark as long ref";
          return false;
        }
        break;

      case 4: {
        // Unmark all reference pictures with long_term_frame_idx over new max.
        max_long_term_frame_idx_ =
            ref_pic_marking->max_long_term_frame_idx_plus1 - 1;
        H264Picture::Vector long_terms;
        dpb_.GetLongTermRefPicsAppending(&long_terms);
        for (size_t long_term = 0; long_term < long_terms.size(); ++long_term) {
          scoped_refptr<H264Picture>& long_term_pic = long_terms[long_term];
          DCHECK(long_term_pic->ref && long_term_pic->long_term);
          // Ok to cast, max_long_term_frame_idx is much smaller than 16bit.
          if (long_term_pic->long_term_frame_idx >
              static_cast<int>(max_long_term_frame_idx_))
            long_term_pic->ref = false;
        }
        break;
      }

      case 5:
        // Unmark all reference pictures.
        dpb_.MarkAllUnusedForRef();
        max_long_term_frame_idx_ = -1;
        pic->mem_mgmt_5 = true;
        break;

      case 6: {
        // Replace long term reference pictures with current picture.
        // First unmark if any existing with this long_term_frame_idx...
        H264Picture::Vector long_terms;
        dpb_.GetLongTermRefPicsAppending(&long_terms);
        for (size_t long_term = 0; long_term < long_terms.size(); ++long_term) {
          scoped_refptr<H264Picture>& long_term_pic = long_terms[long_term];
          DCHECK(long_term_pic->ref && long_term_pic->long_term);
          // Ok to cast, long_term_frame_idx is much smaller than 16bit.
          if (long_term_pic->long_term_frame_idx ==
              static_cast<int>(ref_pic_marking->long_term_frame_idx))
            long_term_pic->ref = false;
        }

        // and mark the current one instead.
        pic->ref = true;
        pic->long_term = true;
        pic->long_term_frame_idx = ref_pic_marking->long_term_frame_idx;
        break;
      }

      default:
        // Would indicate a bug in parser.
        NOTREACHED();
    }
  }

  return true;
}

// This method ensures that DPB does not overflow, either by removing
// reference pictures as specified in the stream, or using a sliding window
// procedure to remove the oldest one.
// It also performs marking and unmarking pictures as reference.
// See spac 8.2.5.1.
bool H264Decoder::ReferencePictureMarking(scoped_refptr<H264Picture> pic) {
  // If the current picture is an IDR, all reference pictures are unmarked.
  if (pic->idr) {
    dpb_.MarkAllUnusedForRef();

    if (pic->long_term_reference_flag) {
      pic->long_term = true;
      pic->long_term_frame_idx = 0;
      max_long_term_frame_idx_ = 0;
    } else {
      pic->long_term = false;
      max_long_term_frame_idx_ = -1;
    }

    return true;
  }

  // Not an IDR. If the stream contains instructions on how to discard pictures
  // from DPB and how to mark/unmark existing reference pictures, do so.
  // Otherwise, fall back to default sliding window process.
  if (pic->adaptive_ref_pic_marking_mode_flag) {
    DCHECK(!pic->nonexisting);
    return HandleMemoryManagementOps(pic);
  } else {
    return SlidingWindowPictureMarking();
  }
}

bool H264Decoder::SlidingWindowPictureMarking() {
  FX_LOGS(DEBUG) << "curr_sps_id_: " << curr_sps_id_;
  const H264SPS* sps = parser_.GetSPS(curr_sps_id_);
  if (!sps)
    return false;

  // 8.2.5.3. Ensure the DPB doesn't overflow by discarding the oldest picture.
  int num_ref_pics = dpb_.CountRefPics();
  int effective_max_num_ref_frames = std::max<int>(sps->max_num_ref_frames, 1);
  if (num_ref_pics == effective_max_num_ref_frames) {
    // Max number of reference pics reached, need to remove one of the short
    // term ones. Find smallest frame_num_wrap short reference picture and mark
    // it as unused.
    scoped_refptr<H264Picture> to_unmark =
        dpb_.GetLowestFrameNumWrapShortRefPic();
    if (!to_unmark) {
      FX_LOGS(DEBUG) << "Couldn't find a short ref picture to unmark";
      return false;
    }

    to_unmark->ref = false;
  } else if (num_ref_pics > effective_max_num_ref_frames) {
    FX_LOGS(DEBUG) << "Too many reference pictures in DPB";
    return false;
  }

  return true;
}

bool H264Decoder::FinishPicture(scoped_refptr<H264Picture> pic) {
  // Finish processing the picture.
  // Start by storing previous picture data for later use.
  if (pic->ref) {
    if (!ReferencePictureMarking(pic)) {
      return false;
    }

    prev_ref_has_memmgmnt5_ = pic->mem_mgmt_5;
    prev_ref_top_field_order_cnt_ = pic->top_field_order_cnt;
    prev_ref_pic_order_cnt_msb_ = pic->pic_order_cnt_msb;
    prev_ref_pic_order_cnt_lsb_ = pic->pic_order_cnt_lsb;
    prev_ref_field_ = pic->field;
    prev_ref_frame_num_ = pic->frame_num;
  }
  prev_frame_num_ = pic->frame_num;
  prev_has_memmgmnt5_ = pic->mem_mgmt_5;
  prev_frame_num_offset_ = pic->frame_num_offset;

  // Remove unused (for reference or later output) pictures from DPB, marking
  // them as such.
  dpb_.DeleteUnused();

  FX_LOGS(DEBUG) << "Finishing picture frame_num: " << pic->frame_num
                 << ", entries in DPB: " << dpb_.size();
  if (recovery_frame_cnt_) {
    // This is the first picture after the recovery point SEI message. Validate
    // `recovery_frame_cnt_` now that we are certain to have max_frame_num_.
    if (*recovery_frame_cnt_ >= max_frame_num_) {
      FX_LOGS(DEBUG) << "Invalid recovery_frame_cnt=" << *recovery_frame_cnt_
                     << " (must be less than or equal to max_frame_num-1="
                     << (max_frame_num_ - 1) << ")";
      return false;
    }

    // Compute the frame_num of the first frame that should be output (D.2.8).
    recovery_frame_num_ =
        (*recovery_frame_cnt_ + pic->frame_num) % max_frame_num_;
    FX_LOGS(DEBUG) << "recovery_frame_num_=" << *recovery_frame_num_;
    recovery_frame_cnt_.reset();
  }

  if (pic->idr && recovery_frame_num_) {
    // The pictures after the IDR picture in decode order is guaranteed to be
    // correct. We don't recover at the recovery frame even if it's before the
    // IDR picture (i.e. dropping correct frames before the IDR frame) for
    // simplifying the implementation.
    // As the frames in |dpb_| will not be output so we drop them here. It's
    // safe to clear DPB because IDR and later frames don't reference frames
    // before IDR.
    recovery_frame_num_.reset();
    dpb_.Clear();
  }

  // The ownership of pic will either be transferred to DPB - if the picture is
  // still needed (for output and/or reference) - or we will release it
  // immediately if we manage to output it here and won't have to store it for
  // future reference.

  // Get all pictures that haven't been outputted yet.
  H264Picture::Vector not_outputted;
  dpb_.GetNotOutputtedPicsAppending(&not_outputted);
  // Include the one we've just decoded.
  not_outputted.push_back(pic);

  // Sort in output order.
  std::sort(not_outputted.begin(), not_outputted.end(), POCAscCompare());

  // Try to output as many pictures as we can. A picture can be output,
  // if the number of decoded and not yet outputted pictures that would remain
  // in DPB afterwards would at least be equal to max_num_reorder_frames.
  // If the outputted picture is not a reference picture, it doesn't have
  // to remain in the DPB and can be removed.
  auto output_candidate = not_outputted.begin();
  size_t num_remaining = not_outputted.size();
  DCHECK_EQ(!!num_remaining, (output_candidate != not_outputted.end()));
  while (
      num_remaining &&
      (num_remaining > max_num_reorder_frames_ ||
       // If the condition below is used, this is an invalid stream. We should
       // not be forced to output beyond max_num_reorder_frames in order to
       // make room in DPB to store the current picture (if we need to do so).
       // However, if this happens, ignore max_num_reorder_frames and try
       // to output more. This may cause out-of-order output, but is not
       // fatal, and better than failing instead.
       (dpb_.IsFull() && (!pic->outputted || pic->ref)) ||
       // See comments above definition of kEnableQuickOutputHeuristic.
       IsQuickOutputHeuristicSatisfied((*output_candidate)->pic_order_cnt))) {
    if (num_remaining <= max_num_reorder_frames_) {
      FX_LOGS(DEBUG) << "Invalid stream: max_num_reorder_frames not preserved";
    }
    if (!recovery_frame_num_ ||
        // If we are decoding ahead to reach a SEI recovery point, skip
        // outputting all pictures before it, to avoid outputting corrupted
        // frames.
        (*output_candidate)->frame_num == *recovery_frame_num_) {
      recovery_frame_num_ = std::nullopt;
      if (!OutputPic(*output_candidate))
        return false;
    }

    if (!(*output_candidate)->ref) {
      // Current picture hasn't been inserted into DPB yet, so don't remove it
      // if we managed to output it immediately.
      if (*output_candidate != pic)
        dpb_.Delete(*output_candidate);
    }

    ++output_candidate;
    --num_remaining;
    DCHECK_EQ(!!num_remaining, (output_candidate != not_outputted.end()));
  }

  // If we haven't managed to output the picture that we just decoded, or if
  // it's a reference picture, we have to store it in DPB.
  if (!pic->outputted || pic->ref) {
    if (dpb_.IsFull()) {
      // If we haven't managed to output anything to free up space in DPB
      // to store this picture, it's an error in the stream.
      FX_LOGS(DEBUG) << "Could not free up space in DPB!";
      return false;
    }

    dpb_.StorePic(std::move(pic));
  }

  return true;
}

bool H264Decoder::IsQuickOutputHeuristicSatisfied(int pic_order_cnt) {
  if (!kEnableQuickOutputHeuristic) {
    // heuristic disabled
    return false;
  }
  if (max_num_reorder_frames_ < dpb_.max_num_pics()) {
    // The heuristic is disabled if we have a real max_num_reorder_frames_ from
    // the stream, because in that case the stream knows best.
    return false;
  }
  if (last_output_poc_ == std::numeric_limits<int>::min()) {
    // Assume first frame is ok to output immediately.  For most normal streams
    // this is correct, but it _may_ be possible to construct a valid stream for
    // which this guess isn't correct.
    //
    // This also has the nice effect of providing pixels to put on the screen
    // ASAP, even if we aren't sure (enough) for a few frames whether we're
    // likely to see any odd POC values.
    return true;
  }
  if (last_output_poc_ + 1 == pic_order_cnt) {
    // If the POC is exactly one more than previous outputted POC, then output
    // early because no other POC to be decoded after can fit between the two
    // frames.  I believe this is always a completely valid thing to do.
    // However, not all streams increment their POC values by 1 unfortunately.
    return true;
  }
  if (num_poc_seen_ < kNumPocSeenToAssumeOnlyEvenPocUnlessProvenOtherwise) {
    // We aren't sure (enough) yet regarding whether there are any odd POCs.
    return false;
  }
  if (!odd_poc_seen_ && (last_output_poc_ + 2 == pic_order_cnt)) {
    // We haven't seen any odd POCs yet.  So far it looks like POC values are
    // only even in this stream which is common (apparently starting with a
    // reference encoder from way back).  So until we see an odd POC, assume
    // there are only even POCs and allow output of this "next" POC since we can
    // "reasonably" guess that there won't be any odd POC that shows up between
    // the previous POC and this POC.  While this guess may be wrong near the
    // start of a stream, and even may be wrong _far_ into a
    // specially-constructed stream, it's unlikely (enough) to be a wrong guess
    // for normal streams if kNumPocSeenToAssumeOnlyEvenPocUnlessProvenOtherwise
    // is large enough.  As for exactly how large
    // kNumPocSeenToAssumeOnlyEvenPocUnlessProvenOtherwise needs to be to handle
    // all the normal streams we encounter, that's a tuning thing.
    return true;
  }
  // Wait until a more suitable lowest-POC frame is decoded.
  //
  // Or, if the stream isn't being helpful with it's POC values, wait until
  // max_num_reorder_frames_ is satisfied even though max_num_reorder_frames_
  // may be larger than the actual max_num_reorder_frames from the stream,
  // possibly due to lack of availability of max_num_reorder_frames from FW.  In
  // such cases it may be worth asking the sender of the stream to consider
  // encoding with POC values that start at 0 and increment by 1 each frame
  // (better), or start at 0 and increment by 2 each frame (still works but with
  // a tad more decoder delay at first).
  return false;
}

bool H264Decoder::UpdateMaxNumReorderFrames(const H264SPS* sps) {
  if (sps->vui_parameters_present_flag && sps->bitstream_restriction_flag) {
    max_num_reorder_frames_ =
        base::checked_cast<size_t>(sps->max_num_reorder_frames);
    if (max_num_reorder_frames_ > dpb_.max_num_pics()) {
      FX_LOGS(DEBUG)
          << "max_num_reorder_frames present, but larger than MaxDpbFrames ("
          << max_num_reorder_frames_ << " > " << dpb_.max_num_pics() << ")";
      max_num_reorder_frames_ = 0;
      return false;
    }
    return true;
  }

  // max_num_reorder_frames not present, infer from profile/constraints
  // (see VUI semantics in spec).
  if (sps->constraint_set3_flag) {
    switch (sps->profile_idc) {
      case 44:
      case 86:
      case 100:
      case 110:
      case 122:
      case 244:
        max_num_reorder_frames_ = 0;
        break;
      default:
        max_num_reorder_frames_ = dpb_.max_num_pics();
        break;
    }
  } else {
    max_num_reorder_frames_ = dpb_.max_num_pics();
  }

  return true;
}

bool H264Decoder::ProcessPPSAndSPS(int pps_id, bool* need_new_buffers) {
  FX_LOGS(DEBUG) << "Processing PPS id: " << pps_id;

  const H264PPS* pps = parser_.GetPPS(pps_id);
  if (!pps) {
    return false;
  }

  const H264SPS* sps = parser_.GetSPS(pps->seq_parameter_set_id);
  if (!sps)
    return false;

  *need_new_buffers = false;

  if (sps->frame_mbs_only_flag == 0) {
    FX_LOGS(ERROR) << "Interlacing is not supported (frame_mbs_only_flag != 1)";
    return false;
  }

  gfx::Size new_pic_size = sps->GetCodedSize().value_or(gfx::Size());
  if (new_pic_size.IsEmpty() || !gfx::Rect(new_pic_size).IsValid()) {
    FX_LOGS(DEBUG) << "Invalid picture size";
    return false;
  }

  gfx::Rect new_visible_rect =
      sps->GetVisibleRect().value_or(gfx::Rect(new_pic_size));
  if (!new_visible_rect.IsValid()) {
    FX_LOGS(DEBUG) << "Invalid visible rect";
    return false;
  }

  int width_mb = new_pic_size.width() / 16;
  int height_mb = new_pic_size.height() / 16;

  // Verify that the values are not too large before multiplying.
  if (std::numeric_limits<int>::max() / width_mb < height_mb) {
    FX_LOGS(DEBUG) << "Picture size is too big: " << new_pic_size.ToString();
    return false;
  }

  // Spec A.3.1 and A.3.2
  // For Baseline, Constrained Baseline and Main profile, the indicated level is
  // Level 1b if level_idc is equal to 11 and constraint_set3_flag is equal to 1
  uint8_t level = base::checked_cast<uint8_t>(sps->level_idc);
  if ((sps->profile_idc == H264SPS::kProfileIDCBaseline ||
       sps->profile_idc == H264SPS::kProfileIDCConstrainedBaseline ||
       sps->profile_idc == H264SPS::kProfileIDCMain) &&
      level == 11 && sps->constraint_set3_flag) {
    level = 9;  // Level 1b
  }
  int max_dpb_mbs = base::checked_cast<int>(H264LevelToMaxDpbMbs(level));
  if (max_dpb_mbs == 0)
    return false;

  // MaxDpbFrames from level limits per spec.
  size_t max_dpb_frames = std::min(max_dpb_mbs / (width_mb * height_mb),
                                   static_cast<int>(H264DPB::kDPBMaxSize));
  FX_LOGS(DEBUG) << "MaxDpbFrames: " << max_dpb_frames
                 << ", max_num_ref_frames: " << sps->max_num_ref_frames
                 << ", max_dec_frame_buffering: "
                 << sps->max_dec_frame_buffering;

  // Set DPB size to at least the level limit, or what the stream requires.
  size_t max_dpb_size =
      std::max(static_cast<int>(max_dpb_frames),
               std::max(sps->max_num_ref_frames, sps->max_dec_frame_buffering));
  if (max_dpb_size > max_dpb_frames) {
    FX_LOGS(DEBUG) << "Invalid stream, DPB size > MaxDpbFrames";
  }
  // Some non-conforming streams specify more frames are needed than the current
  // level limit. Allow this, but only up to the maximum number of reference
  // frames allowed per spec.
  if (max_dpb_size == 0 || max_dpb_size > H264DPB::kDPBMaxSize) {
    FX_LOGS(DEBUG) << "Invalid DPB size: " << max_dpb_size;
    return false;
  }
  VideoChromaSampling new_chroma_sampling = sps->GetChromaSampling();
  if (new_chroma_sampling != chroma_sampling_) {
    chroma_sampling_ = new_chroma_sampling;
  }

  if (chroma_sampling_ != VideoChromaSampling::k420) {
    FX_LOGS(DEBUG) << "Only YUV 4:2:0 is supported";
    return false;
  }

  gfx::Size sar_size(sps->sar_width, sps->sar_height);

  VideoCodecProfile new_profile =
      H264Parser::ProfileIDCToVideoCodecProfile(sps->profile_idc);
  if (new_profile == VIDEO_CODEC_PROFILE_UNKNOWN) {
    return false;
  }
  uint8_t new_bit_depth = 0;
  if (!ParseBitDepth(*sps, new_bit_depth)) {
    return false;
  }
  if (!IsValidBitDepth(new_bit_depth, new_profile)) {
    FX_LOGS(DEBUG) << "Invalid bit depth="
                   << base::strict_cast<int>(new_bit_depth)
                   << ", profile=" << GetProfileName(new_profile);
    return false;
  }

  VideoColorSpace new_color_space;
  if (sps && sps->GetColorSpace().IsSpecified()) {
    new_color_space = sps->GetColorSpace();
  } else if (container_color_space_.IsSpecified()) {
    new_color_space = container_color_space_;
  }

  if (new_color_space.matrix() == VideoColorSpace::MatrixID::RGB) {
    // Some H.264 videos contain a VUI that specifies a color matrix of GBR,
    // when they are actually ordinary YUV. H264 only supports 4:2:0 subsampling
    // and BGR should only be used with 4:4:4, hence default to Rec709. See
    // crbug.com/341266991.
    CHECK_NE(chroma_sampling_, VideoChromaSampling::k444);
    new_color_space = VideoColorSpace::REC709();
  }

#if CHROMIUM_CODE
  bool is_color_space_change = false;
  if (base::FeatureList::IsEnabled(kAVDColorSpaceChanges)) {
    is_color_space_change = new_color_space.IsSpecified() &&
                            new_color_space != picture_color_space_;
  }
#endif
  if (pic_size_ != new_pic_size || dpb_.max_num_pics() != max_dpb_size ||
      profile_ != new_profile || bit_depth_ != new_bit_depth ||
      new_color_space != picture_color_space_ || sar_size_ != sar_size) {
    if (!Flush())
      return false;
#if CHROMIUM_CODE
    FX_LOGS(DEBUG) << "Codec profile: " << GetProfileName(new_profile)
                   << ", level: " << base::strict_cast<int>(level)
                   << ", DPB size: " << max_dpb_size
                   << ", Picture size: " << new_pic_size.ToString()
                   << ", bit depth: " << base::strict_cast<int>(new_bit_depth);
#endif
    *need_new_buffers = true;
    profile_ = new_profile;
    bit_depth_ = new_bit_depth;
    pic_size_ = new_pic_size;
    sar_size_ = sar_size;
    picture_color_space_ = new_color_space;
    dpb_.set_max_num_pics(max_dpb_size);
  }

  ZX_ASSERT(gfx::Rect(new_pic_size).IsValid());
  ZX_ASSERT(new_visible_rect.IsValid());
  if (!gfx::Rect(new_pic_size).Contains(new_visible_rect)) {
    DVLOG(1) << "Visible rect " << new_visible_rect.ToString()
             << " exceeds pic size " << new_pic_size.ToString()
             << ", resetting to pic size";
    new_visible_rect = gfx::Rect(new_pic_size);
  }
  if (visible_rect_ != new_visible_rect) {
    FX_LOGS(DEBUG) << "New visible rect: " << new_visible_rect.ToString();
    visible_rect_ = new_visible_rect;
  }

  if (!UpdateMaxNumReorderFrames(sps))
    return false;
  FX_LOGS(DEBUG) << "max_num_reorder_frames: " << max_num_reorder_frames_;

  if (*need_new_buffers) {
    ref_pic_list_p0_.clear();
    ref_pic_list_b0_.clear();
    ref_pic_list_b1_.clear();
  }
  return true;
}

H264Decoder::H264Accelerator::Status H264Decoder::FinishPrevFrameIfPresent() {
  // If we already have a frame waiting to be decoded, decode it and finish.
  if (curr_pic_) {
    H264Accelerator::Status result = DecodePicture();
    if (result != H264Accelerator::Status::kOk)
      return result;

    scoped_refptr<H264Picture> pic = curr_pic_;
    curr_pic_ = nullptr;
    if (!FinishPicture(pic))
      return H264Accelerator::Status::kFail;
  }

  return H264Accelerator::Status::kOk;
}

H264Decoder::H264Accelerator::Status H264Decoder::HandleFrameNumGap(
    int frame_num) {
  FX_LOGS(DEBUG) << "curr_sps_id_: " << curr_sps_id_;
  const H264SPS* sps = parser_.GetSPS(curr_sps_id_);
  if (!sps)
    return H264Decoder::H264Accelerator::Status::kFail;

  if (!sps->gaps_in_frame_num_value_allowed_flag) {
    FX_LOGS(DEBUG) << "Invalid frame_num: " << frame_num;
    // TODO(b:129119729, b:146914440): Youtube android app sometimes sends an
    // invalid frame number after a seek. The sequence goes like:
    // Seek, SPS, PPS, IDR-frame, non-IDR, ... non-IDR with invalid number.
    // The only way to work around this reliably is to ignore this error.
    // Video playback is not affected, no artefacts are visible.
    return H264Decoder::H264Accelerator::Status::kOk;
  }

  FX_LOGS(DEBUG) << "Handling frame_num gap: " << prev_ref_frame_num_ << "->"
                 << frame_num;

  // 7.4.3/7-23
  int unused_short_term_frame_num = (prev_ref_frame_num_ + 1) % max_frame_num_;

  const int gap_size =
      (frame_num - unused_short_term_frame_num + max_frame_num_) %
      max_frame_num_;
  if (gap_size > H264DPB::kDPBMaxSize) {
    // A gap larger than the DPB size will inevitably push out all existing
    // frames. Instead of allocating thousands of dummy pictures to fill the
    // gap (which can cause a timeout/OOM), fast-forward the state to just
    // before the last `H264DPB::kDPBMaxSize` frames. The while loop below
    // will then naturally fill the DPB with those dummy frames.

    int skip_count = gap_size - H264DPB::kDPBMaxSize;

    if (recovery_frame_num_) {
      int recovery_offset = (*recovery_frame_num_ -
                             unused_short_term_frame_num + max_frame_num_) %
                            max_frame_num_;
      if (recovery_offset < skip_count) {
        // The recovery frame was skipped. Stop dropping frames.
        recovery_frame_num_ = std::nullopt;
      }
    }

    if (prev_has_memmgmnt5_) {
      prev_frame_num_offset_ = 0;
      prev_has_memmgmnt5_ = false;
    }

    int wrap_count = 0;
    if (prev_frame_num_ > unused_short_term_frame_num) {
      wrap_count++;
    }
    if (unused_short_term_frame_num + skip_count > max_frame_num_) {
      wrap_count++;
    }

    if (wrap_count > 0) {
      if (!safemath::CheckAdd(prev_frame_num_offset_,
                              wrap_count * max_frame_num_)
               .AssignIfValid(&prev_frame_num_offset_)) {
        FX_LOGS(DEBUG) << "Frame num offset overflow";
        return H264Accelerator::Status::kFail;
      }
    }

    int advanced_frame_num =
        (unused_short_term_frame_num + skip_count) % max_frame_num_;
    prev_ref_frame_num_ =
        (advanced_frame_num - 1 + max_frame_num_) % max_frame_num_;
    prev_frame_num_ = prev_ref_frame_num_;
    prev_ref_has_memmgmnt5_ = false;

    unused_short_term_frame_num = advanced_frame_num;
  }

  while (unused_short_term_frame_num != frame_num) {
    scoped_refptr<H264Picture> pic =
        accelerator_->CreateH264Picture(/*is_for_output*/ false);
    if (!pic) {
      // CreateH264Picture() returning no pic is like kTryAgain from
      // accelerator_.
      return H264Decoder::H264Accelerator::Status::kTryAgain;
    }
    if (!InitNonexistingPicture(pic, unused_short_term_frame_num))
      return H264Decoder::H264Accelerator::Status::kFail;

    UpdatePicNums(unused_short_term_frame_num);

    if (!FinishPicture(pic))
      return H264Decoder::H264Accelerator::Status::kFail;

    unused_short_term_frame_num++;
    unused_short_term_frame_num %= max_frame_num_;
  }

  return H264Decoder::H264Accelerator::Status::kOk;
}

H264Decoder::H264Accelerator::Status H264Decoder::ProcessEncryptedSliceHeader(
    const std::vector<SubsampleEntry>& subsamples) {
  DCHECK(curr_nalu_);
  DCHECK(curr_slice_hdr_);
  std::vector<base::span<const uint8_t>> spans;
  spans.reserve(prior_cencv1_nalus_.size() + 1);
  spans.insert(spans.end(), prior_cencv1_nalus_.begin(),
               prior_cencv1_nalus_.end());
  spans.emplace_back(curr_nalu_->data);
  std::vector<SubsampleEntry> all_subsamples;
  all_subsamples.reserve(prior_cencv1_subsamples_.size() + subsamples.size());
  all_subsamples.insert(all_subsamples.end(), prior_cencv1_subsamples_.begin(),
                        prior_cencv1_subsamples_.end());
  all_subsamples.insert(all_subsamples.end(), subsamples.begin(),
                        subsamples.end());
  auto rv = accelerator_->ParseEncryptedSliceHeader(
      spans, all_subsamples, secure_handle_, curr_slice_hdr_.get());
  // Return now if this isn't fully processed and don't store the NALU info
  // since we will get called again in the kTryAgain case, and on an error we
  // want to exist.
  if (rv != H264Accelerator::Status::kOk)
    return rv;

  // Insert this encrypted slice data as well in case this is a multi-slice
  // picture.
  prior_cencv1_nalus_.emplace_back(curr_nalu_->data);
  prior_cencv1_subsamples_.insert(prior_cencv1_subsamples_.end(),
                                  subsamples.begin(), subsamples.end());
  return rv;
}

H264Decoder::H264Accelerator::Status H264Decoder::PreprocessCurrentSlice() {
  const H264SliceHeader* slice_hdr = curr_slice_hdr_.get();
  DCHECK(slice_hdr);

  FX_LOGS(DEBUG) << "curr_sps_id_: " << curr_sps_id_;
  if (IsNewPrimaryCodedPicture(curr_pic_.get(), curr_pps_id_,
                               parser_.GetSPS(curr_sps_id_), *slice_hdr)) {
    // New picture, so first finish the previous one before processing it.
    H264Accelerator::Status result = FinishPrevFrameIfPresent();
    if (result != H264Accelerator::Status::kOk)
      return result;

    DCHECK(!curr_pic_);

    if (slice_hdr->first_mb_in_slice != 0) {
      FX_LOGS(DEBUG) << "ASO/invalid stream, first_mb_in_slice: "
                     << slice_hdr->first_mb_in_slice;
      return H264Accelerator::Status::kFail;
    }

    // If the new picture is an IDR, flush DPB.
    if (slice_hdr->idr_pic_flag) {
      // Output all remaining pictures, unless we are explicitly instructed
      // not to do so.
      if (!slice_hdr->no_output_of_prior_pics_flag) {
        if (!Flush())
          return H264Accelerator::Status::kFail;
      }
      dpb_.Clear();
      last_output_poc_ = std::numeric_limits<int>::min();
    }
  }

  return H264Accelerator::Status::kOk;
}

H264Decoder::H264Accelerator::Status H264Decoder::ProcessCurrentSlice() {
  DCHECK(curr_pic_);

  const H264SliceHeader* slice_hdr = curr_slice_hdr_.get();
  DCHECK(slice_hdr);

  if (slice_hdr->field_pic_flag == 0)
    max_pic_num_ = max_frame_num_;
  else
    max_pic_num_ = 2 * max_frame_num_;

  H264Picture::Vector ref_pic_list0, ref_pic_list1;
  // If we are using full sample encryption then we do not have the information
  // we need to update the ref pic lists here, but that's OK because the
  // accelerator doesn't actually need to submit them in this case.
  if (!slice_hdr->full_sample_encryption && requires_ref_lists_ &&
      !ModifyReferencePicLists(slice_hdr, &ref_pic_list0, &ref_pic_list1)) {
    return H264Accelerator::Status::kFail;
  }

  const H264PPS* pps = parser_.GetPPS(curr_pps_id_);
  if (!pps)
    return H264Accelerator::Status::kFail;

  DCHECK(nalu_injection_mode_ != NaluInjectionMode::kUnknown);
  DCHECK(nalu_injection_mode_ == NaluInjectionMode::kOff ||
         nalu_injection_mode_ == NaluInjectionMode::kOn);
  return accelerator_->SubmitSlice(
      pps, slice_hdr, ref_pic_list0, ref_pic_list1, curr_pic_,
      slice_hdr->nalu_data, slice_hdr->nalu_size,
      nalu_injection_mode_ == NaluInjectionMode::kOff
          ? parser_.GetCurrentSubsamples()
          : std::vector<SubsampleEntry>());
}

#define SET_ERROR_AND_RETURN()               \
  do {                                       \
    FX_LOGS(DEBUG) << "Error during decode"; \
    state_ = State::kError;                  \
    return H264Decoder::kDecodeError;        \
  } while (0)

#define CHECK_ACCELERATOR_RESULT(func)                 \
  do {                                                 \
    H264Accelerator::Status result = (func);           \
    switch (result) {                                  \
      case H264Accelerator::Status::kOk:               \
        break;                                         \
      case H264Accelerator::Status::kTryAgain:         \
        FX_LOGS(DEBUG) << #func " needs to try again"; \
        return H264Decoder::kTryAgain;                 \
      case H264Accelerator::Status::kFail:             \
      case H264Accelerator::Status::kNotSupported:     \
        SET_ERROR_AND_RETURN();                        \
    }                                                  \
  } while (0)

void H264Decoder::SetStream(int32_t id,
                            scoped_refptr<DecoderBuffer> decoder_buffer) {
  ZX_DEBUG_ASSERT(nalu_injection_mode_ != NaluInjectionMode::kOn);
  nalu_injection_mode_ = NaluInjectionMode::kOff;

  CHECK(decoder_buffer);
  prior_cencv1_nalus_.clear();
  prior_cencv1_subsamples_.clear();
  curr_nalu_.reset();
  curr_slice_hdr_.reset();
  // Keep the old buffer alive until the end of this function to ensure
  // that any active spans in the parser are cleared before the memory is freed.
  auto outgoing_decoder_buffer = std::move(decoder_buffer_);
  decoder_buffer_ = std::move(decoder_buffer);
  const DecryptConfig* decrypt_config = decoder_buffer_->decrypt_config();

  FX_LOGS(DEBUG) << "New input stream id: " << id
                 << " size: " << decoder_buffer_->size();
  stream_id_ = id;
  decoder_buffer_has_been_changed_ = true;
  if (decrypt_config) {
#if CHROMIUM_CODE
    parser_.SetEncryptedStream(decoder_buffer_->data(), decoder_buffer_->size(),
                               decrypt_config->subsamples());
    current_decrypt_config_ = decrypt_config->Clone();
#endif
  } else {
    parser_.SetStream(decoder_buffer_->as_span());
    current_decrypt_config_ = nullptr;
  }
}

H264Decoder::DecodeResult H264Decoder::Decode() {
  if (state_ == State::kError) {
    FX_LOGS(DEBUG) << "Decoder in error state";
    return kDecodeError;
  }

  if (nalu_injection_mode_ == NaluInjectionMode::kUnknown) {
    nalu_injection_mode_ = NaluInjectionMode::kOff;
  }
  ZX_DEBUG_ASSERT(nalu_injection_mode_ != NaluInjectionMode::kUnknown);
  ZX_DEBUG_ASSERT(!decoder_buffer_has_been_changed_ ||
                  nalu_injection_mode_ == NaluInjectionMode::kOff);

  if (decoder_buffer_has_been_changed_) {
    // Calling H264Accelerator::SetStream() here instead of when the stream is
    // originally set in case the accelerator needs to return kTryAgain.
    H264Accelerator::Status result = accelerator_->SetStream(
        *decoder_buffer_, current_decrypt_config_.get());
    switch (result) {
      case H264Accelerator::Status::kOk:
      case H264Accelerator::Status::kNotSupported:
        // kNotSupported means the accelerator can't handle this stream,
        // so everything will be done through the parser.
        break;
      case H264Accelerator::Status::kTryAgain:
        FX_LOGS(DEBUG) << "SetStream() needs to try again";
        return H264Decoder::kTryAgain;
      case H264Accelerator::Status::kFail:
        SET_ERROR_AND_RETURN();
    }

    // Reset the flag so that this is only called again next time SetStream()
    // is called.
    decoder_buffer_has_been_changed_ = false;
  }

  while (true) {
    H264Parser::Result par_res;

    if (!curr_nalu_) {
      if (nalu_injection_mode_ == NaluInjectionMode::kOff) {
        curr_nalu_.emplace();
        par_res = parser_.AdvanceToNextNALU(&*curr_nalu_);
        if (par_res == H264Parser::kEOStream) {
          CHECK_ACCELERATOR_RESULT(FinishPrevFrameIfPresent());
          return kRanOutOfStreamData;
        } else if (par_res != H264Parser::kOk) {
          SET_ERROR_AND_RETURN();
        }

        FX_LOGS(DEBUG) << "New NALU: "
                       << static_cast<int>(curr_nalu_->nal_unit_type);
      } else {
        ZX_DEBUG_ASSERT(nalu_injection_mode_ == NaluInjectionMode::kOn);
        if (preparsed_nalus_.empty()) {
          return kRanOutOfStreamData;
        }
        curr_nalu_ = std::move(preparsed_nalus_.front());
        preparsed_nalus_.pop_front();
        FX_LOGS(DEBUG) << "New preparsed NALU: "
                       << static_cast<int>(curr_nalu_->nal_unit_type);
      }
    }

    switch (curr_nalu_->nal_unit_type) {
      case H264NALU::kNonIDRSlice:
        // We can't resume from a non-IDR slice unless recovery point SEI
        // process is going.
        if (state_ == State::kError ||
            (state_ == State::kAfterReset && !recovery_frame_cnt_))
          break;

        [[fallthrough]];
      case H264NALU::kIDRSlice: {
        // TODO(posciak): the IDR may require an SPS that we don't have
        // available. For now we'd fail if that happens, but ideally we'd like
        // to keep going until the next SPS in the stream.
        if (state_ == State::kNeedStreamMetadata) {
          // We need an SPS, skip this IDR and keep looking.
          break;
        }

        // If after reset or waiting for a key, we should be able to recover
        // from an IDR. |state_|, |curr_slice_hdr_|, and |curr_pic_| are used
        // to keep track of what has previously been attempted, so that after
        // a retryable result is returned, subsequent calls to Decode() retry
        // the call that failed previously. If it succeeds (it may not if no
        // additional key has been provided, for example), then the remaining
        // steps will be executed.
        if (!curr_slice_hdr_) {
          if (nalu_injection_mode_ == NaluInjectionMode::kOff) {
            curr_slice_hdr_ = std::make_unique<H264SliceHeader>();
            state_ = State::kParseSliceHeader;
          } else {
            ZX_DEBUG_ASSERT(nalu_injection_mode_ == NaluInjectionMode::kOn);
            curr_slice_hdr_ =
                std::move(std::get<std::unique_ptr<H264SliceHeader>>(
                    curr_nalu_->preparsed_header));
            state_ = State::kTryPreprocessCurrentSlice;
          }
        }

        if (state_ == State::kParseSliceHeader) {
          // Check if the slice header is encrypted.
          bool parsed_header = false;
          if (current_decrypt_config_) {
            const std::vector<SubsampleEntry>& subsamples =
                parser_.GetCurrentSubsamples();
            // There is only a single clear byte for the NALU information for
            // full sample encryption, and the rest is encrypted.
            if (!subsamples.empty() && subsamples[0].clear_bytes == 1) {
#if CHROMIUM_CODE
              if (base::FeatureList::IsEnabled(
                      kVaapiEarlyPPSParsingForCENCv1)) {
                // On ChromeOS for CENCv1 on some devices, we need to wait until
                // after the decoder buffers are created to be able to process
                // the encrypted slice header.
                bool need_new_buffers = false;
                if (!ProcessPPSAndSPS(last_parsed_pps_id_, &need_new_buffers)) {
                  SET_ERROR_AND_RETURN();
                }

                if (need_new_buffers) {
                  // Yield `kConfigChange` to the client so they can allocate
                  // new surfaces. We do not advance `state_` or clear
                  // `curr_nalu_`. When Decode() resumes, it will re-enter this
                  // block, but ProcessPPSAndSPS will evaluate `need_new_buffers
                  // = false` and proceed.
                  return kConfigChange;
                }
              }
#endif
              CHECK_ACCELERATOR_RESULT(ProcessEncryptedSliceHeader(subsamples));
              parsed_header = true;
              curr_slice_hdr_->pic_parameter_set_id = last_parsed_pps_id_;
            }
          }
          if (!parsed_header) {
            par_res =
                parser_.ParseSliceHeader(*curr_nalu_, curr_slice_hdr_.get());
            if (par_res != H264Parser::kOk)
              SET_ERROR_AND_RETURN();
          }
          state_ = State::kTryPreprocessCurrentSlice;
        }

        if (state_ == State::kTryPreprocessCurrentSlice) {
          CHECK_ACCELERATOR_RESULT(PreprocessCurrentSlice());
          state_ = State::kEnsurePicture;
        }

        if (state_ == State::kEnsurePicture) {
          if (curr_pic_) {
            // |curr_pic_| already exists, so skip to ProcessCurrentSlice().
            state_ = State::kTryCurrentSlice;
          } else {
            // When kVaapiEarlyPPSParsingForCENCv1 is enabled and the stream is
            // CENCv1, ProcessPPSAndSPS will always set `need_new_buffers` to
            // false, since the configuration change gets handled in the
            // ProcessPPSAndSPS call above.
            bool need_new_buffers = false;
            if (!ProcessPPSAndSPS(curr_slice_hdr_->pic_parameter_set_id,
                                  &need_new_buffers)) {
              SET_ERROR_AND_RETURN();
            }

            if (need_new_buffers) {
              // Yield `kConfigChange` to the client so they can allocate new
              // surfaces. We do not advance `state_` or clear `curr_nalu_`.
              // When Decode() resumes, it will re-enter this block, but
              // ProcessPPSAndSPS will evaluate `need_new_buffers = false` and
              // proceed.
              return kConfigChange;
            }

            // New picture/finished previous one, try to start a new one
            // or tell the client we need more surfaces.
            if (secure_handle_) {
              curr_pic_ = accelerator_->CreateH264PictureSecure(secure_handle_);
            } else {
              curr_pic_ =
                  accelerator_->CreateH264Picture(/*is_for_output=*/true);
            }
            if (!curr_pic_)
              return kRanOutOfSurfaces;
            if (current_decrypt_config_)
              curr_pic_->set_decrypt_config(current_decrypt_config_->Clone());
            curr_pic_->SetDynamicHdrMetadata(hdr_metadata_bitstream_,
                                             decoder_buffer_.get());

            state_ = State::kTryNewFrame;
          }
        }

        if (state_ == State::kTryNewFrame) {
          CHECK_ACCELERATOR_RESULT(StartNewFrame(curr_slice_hdr_.get()));
          state_ = State::kTryCurrentSlice;
        }

        DCHECK_EQ(state_, State::kTryCurrentSlice);
        CHECK_ACCELERATOR_RESULT(ProcessCurrentSlice());
        curr_slice_hdr_ = nullptr;
        state_ = State::kDecoding;
        break;
      }

      case H264NALU::kSPS: {
        int sps_id;

        CHECK_ACCELERATOR_RESULT(FinishPrevFrameIfPresent());
        if (nalu_injection_mode_ == NaluInjectionMode::kOff) {
          par_res = parser_.ParseSPS(&sps_id);
        } else {
          ZX_DEBUG_ASSERT(nalu_injection_mode_ == NaluInjectionMode::kOn);
          par_res = parser_.AcceptPreparsedSPS(
              std::move(std::get<std::unique_ptr<H264SPS>>(
                  curr_nalu_->preparsed_header)),
              &sps_id);
        }
        if (par_res != H264Parser::kOk)
          SET_ERROR_AND_RETURN();

        accelerator_->ProcessSPS(parser_.GetSPS(sps_id), curr_nalu_->data);

        if (state_ == State::kNeedStreamMetadata)
          state_ = State::kAfterReset;
        break;
      }

      case H264NALU::kPPS: {
        CHECK_ACCELERATOR_RESULT(FinishPrevFrameIfPresent());
        if (nalu_injection_mode_ == NaluInjectionMode::kOff) {
          par_res = parser_.ParsePPS(&last_parsed_pps_id_);
        } else {
          ZX_DEBUG_ASSERT(nalu_injection_mode_ == NaluInjectionMode::kOn);
          par_res = parser_.AcceptPreparsedPPS(
              std::move(std::get<std::unique_ptr<H264PPS>>(
                  curr_nalu_->preparsed_header)),
              &last_parsed_pps_id_);
        }
        if (par_res != H264Parser::kOk)
          SET_ERROR_AND_RETURN();
        accelerator_->ProcessPPS(parser_.GetPPS(last_parsed_pps_id_),
                                 curr_nalu_->data);
        break;
      }

      case H264NALU::kAUD:
      case H264NALU::kEOSeq:
      case H264NALU::kEOStream:
        if (state_ != State::kDecoding)
          break;

        CHECK_ACCELERATOR_RESULT(FinishPrevFrameIfPresent());
        break;

      case H264NALU::kSEIMessage: {
        if (current_decrypt_config_) {
          // If there are encrypted SEI NALUs as part of CENCv1, then we also
          // need to save those so we can send them into the accelerator so it
          // can decrypt the sample properly (otherwise it would be starting
          // partway into a block).
          const std::vector<SubsampleEntry>& subsamples =
              parser_.GetCurrentSubsamples();
          if (!subsamples.empty()) {
            prior_cencv1_nalus_.emplace_back(curr_nalu_->data);
            DCHECK_EQ(1u, subsamples.size());
            prior_cencv1_subsamples_.push_back(subsamples[0]);
            // Since the SEI is encrypted, do not try to parse it below as it
            // may fail or yield incorrect results.
            DVLOG(3) << "Skipping parsing of encrypted SEI NALU";
            break;
          }
        }
        H264SEI sei;
        if (parser_.ParseSEI(&sei) != H264Parser::kOk)
          break;

        for (const auto& sei_msg : sei.msgs) {
          if (!std::visit(
                  Overloaded{
                      [this](const H264SEIRecoveryPoint& recovery_point) {
                        // If we are after reset, we can also resume from a SEI
                        // recovery point (spec D.2.8) if one is present.
                        // However, if we are already in the process of handling
                        // one, skip any subsequent ones until we are done
                        // processing.
                        if (state_ == State::kAfterReset &&
                            !recovery_frame_cnt_ && !recovery_frame_num_) {
                          recovery_frame_cnt_ =
                              recovery_point.recovery_frame_cnt;

                          if (0 > *recovery_frame_cnt_) {
                            FX_LOGS(DEBUG) << "Invalid recovery_frame_cnt="
                                           << *recovery_frame_cnt_
                                           << " (it must not be less than 0)";
                            return false;
                          }
                          FX_LOGS(DEBUG) << "Recovery point SEI is found, "
                                            "recovery_frame_cnt_="
                                         << *recovery_frame_cnt_;
                        }
                        return true;
                      },
                      [this](const H26xSEIContentLightLevelInfo& info) {
                        hdr_metadata_bitstream_.cta_861_3 = info.ToGfx();
                        return true;
                      },
                      [this](const H26xSEIMasteringDisplayInfo& info) {
                        hdr_metadata_bitstream_.smpte_st_2086 = info.ToGfx();
                        return true;
                      },
                      [](const H26xSEIUserDataRegisteredT35& info) {
                        return true;
                      },
                      [](const std::monostate) { return true; }},
                  sei_msg)) {
            SET_ERROR_AND_RETURN();
          }
        }

        break;
      }

      default:
        FX_LOGS(DEBUG) << "Skipping NALU type: " << curr_nalu_->nal_unit_type;
        break;
    }

    FX_LOGS(DEBUG) << "NALU done";
    curr_nalu_.reset();
  }
}

void H264Decoder::SetStreamId(int32_t id) {
  ZX_DEBUG_ASSERT(nalu_injection_mode_ != NaluInjectionMode::kOff);
  nalu_injection_mode_ = NaluInjectionMode::kOn;

  stream_id_ = id;
}

void H264Decoder::QueuePreparsedNalu(H264NALU nalu) {
  ZX_DEBUG_ASSERT(nalu_injection_mode_ != NaluInjectionMode::kOff);
  nalu_injection_mode_ = NaluInjectionMode::kOn;

  preparsed_nalus_.emplace_back(std::move(nalu));
}

gfx::Size H264Decoder::GetPicSize() const {
  return pic_size_;
}

gfx::Rect H264Decoder::GetVisibleRect() const {
  return visible_rect_;
}

VideoCodecProfile H264Decoder::GetProfile() const {
  return profile_;
}

uint8_t H264Decoder::GetBitDepth() const {
  return bit_depth_;
}

VideoChromaSampling H264Decoder::GetChromaSampling() const {
  return chroma_sampling_;
}

VideoColorSpace H264Decoder::GetVideoColorSpace() const {
  return picture_color_space_;
}

size_t H264Decoder::GetRequiredNumOfPictures() const {
  constexpr size_t kPicsInPipeline = limits::kMaxVideoFrames + 1;
  return GetNumReferenceFrames() + kPicsInPipeline;
}

bool H264Decoder::IsCurrentFrameKeyframe() const {
  return curr_slice_hdr_ && curr_slice_hdr_->idr_pic_flag;
}

size_t H264Decoder::GetNumReferenceFrames() const {
  // Use the maximum number of pictures in the Decoded Picture Buffer.
  return dpb_.max_num_pics();
}

// static
bool H264Decoder::FillH264PictureFromSliceHeader(
    const H264SPS* sps,
    const H264SliceHeader& slice_hdr,
    H264Picture* pic) {
  DCHECK(pic);

  pic->idr = slice_hdr.idr_pic_flag;
  if (pic->idr)
    pic->idr_pic_id = slice_hdr.idr_pic_id;

  if (!slice_hdr.field_pic_flag) {
    pic->field = H264Picture::FIELD_NONE;
  } else {
    FX_LOGS(DEBUG) << "Interlaced video not supported.";
    return false;
  }

  pic->nal_ref_idc = slice_hdr.nal_ref_idc;
  pic->ref = slice_hdr.nal_ref_idc != 0;
  // This assumes non-interlaced stream.
  pic->frame_num = pic->pic_num = slice_hdr.frame_num;

  if (!sps)
    return false;

  pic->pic_order_cnt_type = sps->pic_order_cnt_type;
  switch (pic->pic_order_cnt_type) {
    case 0:
      pic->pic_order_cnt_lsb = slice_hdr.pic_order_cnt_lsb;
      pic->delta_pic_order_cnt_bottom = slice_hdr.delta_pic_order_cnt_bottom;
      break;

    case 1:
      pic->delta_pic_order_cnt0 = slice_hdr.delta_pic_order_cnt0;
      pic->delta_pic_order_cnt1 = slice_hdr.delta_pic_order_cnt1;
      break;

    case 2:
      break;

    default:
      NOTREACHED();
  }
  return true;
}

// static
bool H264Decoder::IsNewPrimaryCodedPicture(const H264Picture* curr_pic,
                                           int curr_pps_id,
                                           const H264SPS* sps,
                                           const H264SliceHeader& slice_hdr) {
  if (!curr_pic)
    return true;

  // 7.4.1.2.4, assumes non-interlaced.
  if (slice_hdr.frame_num != curr_pic->frame_num ||
      slice_hdr.pic_parameter_set_id != curr_pps_id ||
      slice_hdr.nal_ref_idc != curr_pic->nal_ref_idc ||
      slice_hdr.idr_pic_flag != curr_pic->idr ||
      (slice_hdr.idr_pic_flag &&
       (slice_hdr.idr_pic_id != curr_pic->idr_pic_id ||
        // If we have two consecutive IDR slices, and the second one has
        // first_mb_in_slice == 0, treat it as a new picture.
        // Per spec, idr_pic_id should not be equal in this case (and we should
        // have hit the condition above instead, see spec 7.4.3 on idr_pic_id),
        // but some encoders neglect changing idr_pic_id for two consecutive
        // IDRs. Work around this by checking if the next slice contains the
        // zeroth macroblock, i.e. data that belongs to the next picture.
        // Do not perform this check for CENCv1 encrypted content as the
        // first_mb_in_slice field is not correctly populated in that case.
        (slice_hdr.first_mb_in_slice == 0 &&
         !slice_hdr.full_sample_encryption))))
    return true;

  if (!sps)
    return false;

  if (sps->pic_order_cnt_type == curr_pic->pic_order_cnt_type) {
    if (curr_pic->pic_order_cnt_type == 0) {
      if (slice_hdr.pic_order_cnt_lsb != curr_pic->pic_order_cnt_lsb ||
          slice_hdr.delta_pic_order_cnt_bottom !=
              curr_pic->delta_pic_order_cnt_bottom)
        return true;
    } else if (curr_pic->pic_order_cnt_type == 1) {
      if (slice_hdr.delta_pic_order_cnt0 != curr_pic->delta_pic_order_cnt0 ||
          slice_hdr.delta_pic_order_cnt1 != curr_pic->delta_pic_order_cnt1)
        return true;
    }
  }

  return false;
}

}  // namespace media
