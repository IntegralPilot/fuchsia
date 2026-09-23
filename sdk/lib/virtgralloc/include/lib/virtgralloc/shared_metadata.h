// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_VIRTGRALLOC_SHARED_METADATA_H_
#define LIB_VIRTGRALLOC_SHARED_METADATA_H_

#include <inttypes.h>

#include <algorithm>
#include <atomic>
#include <cassert>
#include <cstddef>
#include <type_traits>

// Rather than including types from android headers from this header, this header defines struct
// templates. This header is in an sdk_source_set which results in a bazel target which we
// want/expect to be independently build-able without needing to include android header files in the
// fuchsia SDK. So by defining templates we defer specifying the specific android types until the
// include-er of this header can specify the android types defined in locally-available android
// headers. We check with static_assert()s that the underlying types are consistent with the intent
// of this header.

// We use a templatized SharedMetadataTraits to avoid some verbosity in client code. In client code
// it looks like this:
//
//     #include <lib/virtgralloc/shared_metadata.h>
//
//     #include <aidl/android/hardware/graphics/common/BlendMode.h>
//     #include <aidl/android/hardware/graphics/common/BufferUsage.h>
//     #include <aidl/android/hardware/graphics/common/Dataspace.h>
//     #include <aidl/android/hardware/graphics/common/ExtendableType.h>
//     #include <aidl/android/hardware/graphics/common/PixelFormat.h>
//     #include <aidl/android/hardware/graphics/common/StandardMetadataType.h>
//     #include <android/hardware/graphics/mapper/utils/IMapperMetadataTypes.h>
//     #include <android/hardware/graphics/mapper/4.0/IMapper.h>
//
//     using SharedMetadata = SharedMetadataTraits<
//         ::aidl::android::hardware::graphics::common::StandardMetadataType,
//         ::aidl::android::hardware::graphics::common::BlendMode,
//         ::aidl::android::hardware::graphics::common::BufferUsage,
//         ::aidl::android::hardware::graphics::common::Dataspace,
//         ::aidl::android::hardware::graphics::common::PixelFormat,
//         ::android::hardware::graphics::common::V1_2::BufferUsage,
//         ::android::hardware::graphics::common::V1_2::PixelFormat>;
//     SharedMetadata::Immmutable* immutable = ...;

// The last two types should be ::android::hardware::graphics::common::V1_2::BufferUsage and
// PixelFormat, unless the client code will not be using SerializedBufferDescriptor.
//
// TODO(b/306297561): The last two types are temporary; remove when possible.
template <typename StandardMetadataTypeT, typename BlendModeT, typename BufferUsageT,
          typename DataspaceT, typename PixelFormatT, typename hidl_BufferUsageT = uint64_t,
          typename hidl_PixelFormatT = int32_t>
struct SharedMetadataTraits final {
  using StandardMetadataType = StandardMetadataTypeT;
  using BlendMode = BlendModeT;
  using BufferUsage = BufferUsageT;
  using Dataspace = DataspaceT;
  using PixelFormat = PixelFormatT;
  using hidl_BufferUsage = hidl_BufferUsageT;
  using hidl_PixelFormat = hidl_PixelFormatT;

 private:
  // There is another public section further down with "using"s for the structs intended for use by
  // client code.
  //
  // We define the metadata struct types nested within SharedMetadataTraits to allow for
  // static_assert()s on fully defined types, including use of not-for-client-code-use templates to
  // implement some of the static_assert()s, while still within a syntactic scope that knows about
  // the template args above, and without needing a do_not_touch_this_namespace_even_though_visible
  // style namespace.
  static_assert(std::is_same_v<std::underlying_type_t<BlendMode>, int32_t>);
  static_assert(std::is_same_v<std::underlying_type_t<Dataspace>, int32_t>);
  static_assert(std::is_same_v<std::underlying_type_t<BufferUsage>, int64_t>);
  static_assert(std::is_same_v<std::underlying_type_t<PixelFormat>, int32_t>);
  static_assert(std::is_same_v<std::underlying_type_t<hidl_PixelFormat>, int32_t>);
  static_assert(std::is_same_v<std::underlying_type_t<hidl_BufferUsage>, uint64_t>);

  // Immutable metadata is created before the buffer is visible to any code outside the allocator
  // (before allocation is done) and is always mapped read-only after allocation, so immutable
  // metadata fields don't need to be atomic since they never change.
  //
  // Only standard immutable metadata fields go in this struct. See SharedImmutableMetadata for the
  // overall immutable metadata.
  //
  // Must be standard-layout (checked via static_assert()s below).
  //
  // This is not marked final because the gralloc allocator has a sub-class of this class, used to
  // initialize protected fields in-place when initially creating the immutable metadata VMO.
  class SharedImmutableStandardMetadataBase {
   public:
    static constexpr uint32_t kNameBufferLength = 129;
    static constexpr uint32_t kMaxNameLength = 128;

    // Pre-encoded fields are nominally arbitrary length. We append the pre-encoded payload of each
    // pre-encoded field after SharedImmutableMetadata in an out-of-band fashion. This in-band
    // struct has the offset to that data.
    struct PreEncodedField final {
     private:
      friend class SharedImmutableStandardMetadataBase;
      // Offset in bytes from start of SharedImmutableMetadata to the pre-encoded field data.
      // This value is always aligned to sizeof(max_align_t).
      uint32_t encoded_offset_;
      // Size in bytes of pre-encoded field data. This size is not aligned by any more than is
      // implied by the standard metadata encoding per StandardMetadataType field value (see
      // comments in StandardMetadataType for the per-enum-value encoding, or
      // IMapperMetadataTypes.h for encode/decode code).
      int32_t size_;
    };

    uint64_t buffer_id() const { return buffer_id_; }
    // always 0-terminated
    const char* _Nonnull name() const { return name_; }
    uint64_t width() const { return width_; }
    uint64_t height() const { return height_; }
    uint64_t layer_count() const { return layer_count_; }
    PixelFormat pixel_format_requested() const { return pixel_format_requested_; }
    uint32_t pixel_format_fourcc() const { return pixel_format_fourcc_; }
    uint64_t pixel_format_modifier() const { return pixel_format_modifier_; }
    BufferUsage usage() const { return usage_; }
    uint64_t allocation_size() const { return allocation_size_; }
    uint64_t protected_content() const { return protected_content_; }

    const PreEncodedField& pre_encoded_compression() const { return compression_; }
    const PreEncodedField& pre_encoded_interlaced() const { return interlaced_; }
    const PreEncodedField& pre_encoded_chroma_siting() const { return chroma_siting_; }
    const PreEncodedField& pre_encoded_plane_layouts() const { return plane_layouts_; }
    const PreEncodedField& pre_encoded_crop() const { return crop_; }

    const PreEncodedField* _Nullable GetPreEncodedField(StandardMetadataType type) const {
      switch (type) {
        case StandardMetadataType::COMPRESSION:
          return &pre_encoded_compression();
        case StandardMetadataType::INTERLACED:
          return &pre_encoded_interlaced();
        case StandardMetadataType::CHROMA_SITING:
          return &pre_encoded_chroma_siting();
        case StandardMetadataType::PLANE_LAYOUTS:
          return &pre_encoded_plane_layouts();
        case StandardMetadataType::CROP:
          return &pre_encoded_crop();
        default:
          return nullptr;
      }
    }

    // The SharedImmutableMetadata is initially created such that the half-open range
    // [shared_immutable_metadata + pre_encoded_field.encoded_offset_, shared_immutable_metadata +
    // pre_encoded_field.encoded_offset_ + pre_encoded_field.size_) has the pre-encoded data for the
    // specific pre_encoded_field, and the data is entirely within the mapped shared immutable
    // metadata VMO. We pass the shared_immutable_metadata_size to be able to assert this.
    //
    // The return value here is always positive bytes, never a negative error code.
    int32_t GetPreEncodedMetadata(const uint8_t* _Nonnull shared_immutable_metadata,
                                  uint32_t shared_immutable_metadata_size,
                                  const PreEncodedField& pre_encoded_field,
                                  void* _Nullable destBuffer, size_t destBufferSize) const {
      // Only used in asserts.
      (void)shared_immutable_metadata_size;

      assert((destBuffer != nullptr) == (destBufferSize != 0));

      // These asserts are asserts rather than error returns because the provenance of the FDs
      // specified by the native_handle_t will have been verified by this point, such that we'll be
      // able to rely on gralloc allocator having set up the shared immutable metadata buffer
      // correctly.
      //
      // TODO(b/291974637): Simplify above grammar once importBuffer verifies provenance.
      //
      // assert doesn't overflow
      assert(shared_immutable_metadata + shared_immutable_metadata_size >
             shared_immutable_metadata);
      // assert doesn't overflow
      assert(shared_immutable_metadata + pre_encoded_field.encoded_offset_ +
                 pre_encoded_field.size_ >
             shared_immutable_metadata);
      // assert within overall shared immutable metadata valid bytes
      assert(pre_encoded_field.encoded_offset_ + pre_encoded_field.size_ <
             shared_immutable_metadata_size);

      size_t bytes_to_copy = std::min(destBufferSize, static_cast<size_t>(pre_encoded_field.size_));

      // It's unlikely that any caller relies on being able to get some bytes without getting all
      // the bytes, but there's some vagueness in comments on getMetadata in IMapper.h, so go ahead
      // and copy as many bytes as we can here, up to all of them. We won't do any wasted copying in
      // the normal calling pattern where destBuffer is nullptr in the first call then
      // destBufferSize has enough room in the second call.
      if (bytes_to_copy != 0) {
        memcpy(destBuffer, shared_immutable_metadata + pre_encoded_field.encoded_offset_,
               bytes_to_copy);
      }

      // Return the overall needed bytes regardless of how many were copied.
      return pre_encoded_field.size_;
    }

    // For most pixel formats the stride is in pixels, for a few it's in bytes, per android
    // semantics for this field. The gralloc code always has ImageFormat.bytes_per_row in bytes per
    // fuchsia semantics for that field, even when this android stride field is in pixels.
    uint32_t stride() const { return stride_; }

    // To be standard-layout, all member vars must have same access control, so we use accessors
    // above, which also avoids any accidental attempts to write to immutable fields, which would
    // fault.
   protected:
    void SetPreEncodedField(PreEncodedField& field, uint32_t encoded_offset, uint32_t size) {
      field.encoded_offset_ = encoded_offset;
      field.size_ = size;
    }

    uint64_t buffer_id_;

    // This string is 0-terminated with immutability enforced by read-only VMO, and with validity of
    // native_handle_t checked by each accessing process before that process reads this string (or
    // any other part of SharedImmutableMetadata), so the reading process can safely assume that
    // this string won't change during reading and will be 0-terminated.
    //
    // TODO(b/291974637): Make the previous paragraph fully true (validity checked).
    char name_[kNameBufferLength];

    uint64_t width_;
    uint64_t height_;

    // Currently always 1.
    uint64_t layer_count_;

    PixelFormat pixel_format_requested_;

    uint32_t pixel_format_fourcc_;

    uint64_t pixel_format_modifier_;

    // "The usage is a uint64_t bit field of android.hardware.graphics.common@1.2::BufferUsage's."
    BufferUsage usage_;

    uint64_t allocation_size_;

    // "In future versions, this field will be extended to expose more information about the type
    // of protected content in the buffer."
    uint64_t protected_content_;

    // At least for now, this is always set to "android.hardware.graphics.common.Compression" and
    // android.hardware.graphics.common.Compression.NONE, even if/when pixel_format_modifier_ is set
    // indicating framebuffer compression.
    PreEncodedField compression_;

    // At least for now, this will always be the pre-encoded form of AIDL
    // "android.hardware.graphics.common.Interlaced" and
    // android.hardware.graphics.common.Interlaced.NONE.
    PreEncodedField interlaced_;

    // At least For now, this will be the pre-encoded form of AIDL
    // "android.hardware.graphics.common.ChromaSiting" and
    // android.hardware.graphics.common.ChromaSiting.UNKNOWN or
    // android.hardware.graphics.common.ChromaSiting.NONE depending on chroma-subsampled YUV or not,
    // respectively.
    PreEncodedField chroma_siting_;

    // The PlaneLayoutComponent.type field will have name field set to
    // "android.hardware.graphics.common.PlaneLayoutComponentType" and value set appropriately.
    PreEncodedField plane_layouts_;

    // At least for now, this will specify the entire plane (no cropping).
    PreEncodedField crop_;

    uint32_t stride_;
  };
  class SharedImmutableStandardMetadata final : public SharedImmutableStandardMetadataBase {};

  static_assert(std::is_standard_layout_v<SharedImmutableStandardMetadata>);

  struct PrivateSharedImmutableStandardMetadata final : public SharedImmutableStandardMetadataBase {
    static constexpr bool is_expected_field_offsets() {
      static_assert(std::is_standard_layout_v<PrivateSharedImmutableStandardMetadata>);
      // Don't change existing offsets.
      static_assert(offsetof(PrivateSharedImmutableStandardMetadata, buffer_id_) == 0);
      static_assert(offsetof(PrivateSharedImmutableStandardMetadata, name_) == 8);
      static_assert(offsetof(PrivateSharedImmutableStandardMetadata, width_) == 144);
      static_assert(offsetof(PrivateSharedImmutableStandardMetadata, height_) == 152);
      static_assert(offsetof(PrivateSharedImmutableStandardMetadata, layer_count_) == 160);
      static_assert(offsetof(PrivateSharedImmutableStandardMetadata, pixel_format_requested_) ==
                    168);
      static_assert(offsetof(PrivateSharedImmutableStandardMetadata, pixel_format_fourcc_) == 172);
      static_assert(offsetof(PrivateSharedImmutableStandardMetadata, pixel_format_modifier_) ==
                    176);
      static_assert(offsetof(PrivateSharedImmutableStandardMetadata, usage_) == 184);
      static_assert(offsetof(PrivateSharedImmutableStandardMetadata, allocation_size_) == 192);
      static_assert(offsetof(PrivateSharedImmutableStandardMetadata, protected_content_) == 200);
      static_assert(offsetof(PrivateSharedImmutableStandardMetadata, compression_) == 208);
      static_assert(offsetof(PrivateSharedImmutableStandardMetadata, interlaced_) == 216);
      static_assert(offsetof(PrivateSharedImmutableStandardMetadata, chroma_siting_) == 224);
      static_assert(offsetof(PrivateSharedImmutableStandardMetadata, plane_layouts_) == 232);
      static_assert(offsetof(PrivateSharedImmutableStandardMetadata, crop_) == 240);
      static_assert(offsetof(PrivateSharedImmutableStandardMetadata, stride_) == 248);
      return true;
    }
  };
  // without this, the static_assert()s in is_expected_field_offsets() are ignored
  static_assert(PrivateSharedImmutableStandardMetadata::is_expected_field_offsets());

  // Always aligned to a page boundary.
  //
  // While it's possible to sub-class to gain direct access to all the fields, the intent is that
  // only gralloc code does that, while filling out the fields initially. This way other code will
  // get a compile error if any non-gralloc code accidentally tries to mutate a field that'll be
  // mapped read-only and would fault if written.
  //
  // This is the first part of the shared immutable metadata buffer, but
  // SharedImmutableStandardMetadata::PreEncodedField data goes "out of band" after this struct, in
  // arbitrary order, with offsets (relative to start of this struct / start of the shared immutable
  // metadata buffer) and sizes specified (at run-time) by the fields of each
  // SharedImmutableStandardMetadata::PreEncodedField.
  //
  // This is not marked final because the gralloc allocator has a sub-class of this class, used to
  // initialize protected fields in-place when initially creating the immutable metadata VMO.
  // Clients other than the gralloc allocator should not sub-class.
  class SharedImmutableMetadataBase {
   public:
    static constexpr uint64_t kMagic = 0x74756d6d692d6766;  // 'fg-immut' in LE char dump

    uint64_t magic() const { return magic_; }

    // How many bytes of the immutable metadata buffer are valid.
    uint32_t size() const { return size_; }

    // The offset from start of mutable metadata buffer to the reserved region stored in the mutable
    // metadata buffer. This offset is always page aligned (not just aligned to
    // sizeof(max_align_t)), since the types stored in the reserved region may potentially include
    // architecture-specific types, potentially not accounted for in max_align_t.
    uint32_t reserved_region_offset() const { return reserved_region_offset_; }
    uint32_t reserved_region_size() const { return reserved_region_size_; }

    const SharedImmutableStandardMetadata& shared_immutable_standard_metadata() const {
      // gralloc allocator needs to set this before accessing via
      // shared_immutable_standard_metadata(); the mapper will never hit this assert because
      // shared_immutable_standard_metadata_offset will always be set by the time
      // SharedImmutableMetadata is visible to the mapper
      assert(shared_immutable_standard_metadata_offset_ != 0);
      // in gralloc allocator code they'll be equal; in mapper code the immutable standard metadata
      // must not be colliding with known-to-mapper-code metadata fields above located before the
      // immutable standard metadata
      assert(reinterpret_cast<const SharedImmutableStandardMetadata*>(
                 reinterpret_cast<const uint8_t*>(this) +
                 shared_immutable_standard_metadata_offset_) >=
             &shared_immutable_standard_metadata_storage_);
      return *reinterpret_cast<const SharedImmutableStandardMetadata*>(
          reinterpret_cast<const uint8_t*>(this) + shared_immutable_standard_metadata_offset_);
    }

    // To be standard-layout (static_assert'ed below), all non-static fields must have same access
    // control, so we use non-virtual accessors for all field access above, even for fields where
    // we don't need to compute the offset of the field within SharedImmutableMetadata.
   protected:
    // Only gralloc allocator code should do any sub-classing.

    // This struct is placement-new'ed at the start of the shared immutable metadata buffer while
    // the buffer is still mutable to gralloc allocator code, so field defaults here can work, but
    // any that should default to 0 will already be zero due to fuchsia VMOs starting 0 (but not
    // necessarily backed by physical pages).
    const uint64_t magic_ = kMagic;

    // The overall size of the meaningful bytes of the shared immutable metadata buffer, including
    // the out-of-band PreEncodedField(s). This size is not page aligned (unless by coincidence).
    // This field exists mainly for asserts. After this size, remaining bytes are zero up to the
    // page aligned size of the shared immutable metadata VMO.
    uint32_t size_;

    // Offset from start of SharedImmutableMetadata to shared_immutable_standard_metadata_storage_.
    //
    // Code other than the field initializer should access SharedImmutableStandardMetadata only via
    // shared_immutable_standard_metadata().
    //
    // By having this offset field, and always accessing ...storage_ accounting for the value in
    // this field, gralloc allocator changes can introduce additional immutable metadata fields
    // before the shared_immutable_standard_metadata_storage_ without needing any mapper repo CL(s)
    // first.
    uint32_t shared_immutable_standard_metadata_offset_ =
        offsetof(SharedImmutableMetadata, shared_immutable_standard_metadata_storage_);

    // The offset from start of mutable metadata buffer to start of reserved region. This is needed
    // to allow for adding a mutable metadata field in the gralloc repo before the mapper repo knows
    // about the new field, including when the mutable metadata size is bumped up past a page
    // boundary.
    uint32_t reserved_region_offset_;
    uint32_t reserved_region_size_;

    // All access to this field must be via shared_immutable_standard_metadata(), in both gralloc
    // and mapper code. In mapper code, this address of this field is not guaranteed to be the same
    // as the address of the reference returned by shared_immutable_standard_metadata().
    //
    // This field should remain last in this structure.
    //
    // Currently, the PreEncodedField(s) payload data chunks go after this storage beyond the end of
    // SharedImmutableMetadata, but the mapper code can deal with PreEncodedField payloads before
    // SharedImmutableStandardMetadata if the gralloc code set up the shared immutable metadata
    // buffer that way (without requiring any mapper CLs in advance).
    alignas(sizeof(std::max_align_t))
        SharedImmutableStandardMetadata shared_immutable_standard_metadata_storage_;
  };
  class SharedImmutableMetadata final : public SharedImmutableMetadataBase {};

  static_assert(std::is_standard_layout_v<SharedImmutableMetadata>);

  struct PrivateSharedImmutableMetadata final : public SharedImmutableMetadataBase {
    static constexpr bool is_expected_field_offsets() {
      static_assert(std::is_standard_layout_v<PrivateSharedImmutableMetadata>);
      // Don't change existing offsets.
      static_assert(offsetof(PrivateSharedImmutableMetadata, magic_) == 0);
      static_assert(offsetof(PrivateSharedImmutableMetadata, size_) == 8);
      static_assert(offsetof(PrivateSharedImmutableMetadata,
                             shared_immutable_standard_metadata_offset_) == 12);
      static_assert(offsetof(PrivateSharedImmutableMetadata, reserved_region_offset_) == 16);
      static_assert(offsetof(PrivateSharedImmutableMetadata, reserved_region_size_) == 20);
      return true;
    }
  };
  // without this, the static_assert()s in is_expected_field_offsets() are ignored
  static_assert(PrivateSharedImmutableMetadata::is_expected_field_offsets());

  // Mutable metadata follows the usual gralloc metadata synchronization strategy. This is a
  // cooperative model where permission to write to the main buffer implies permission to write to
  // mutable metadata. Synchronization within a process that may be writing to mutable metadata from
  // more than one thread is up to the code initiating those writes, not the mapper code.
  // Cross-process synchronization is left to mechanisms separate from the gralloc interfaces
  // (separate from IAllocator and IMapper).
  //
  // Despite synchronization among well-behaved code being separate from these structures, to avoid
  // undefined behavior under an assumption of potentially hostile/compromised processes with the
  // ReadWriteMetadata mapped writable, all well-behaved accesses of mutable metadata must be atomic
  // ("relaxed" memory ordering is fine).
  //
  // Each mutable scalar field is atomic, but a multi-field logical metadata item is not atomic as a
  // whole (assuming a badly-behaved process). For a multi-field logical metadata item, per-field
  // atomic reads still prevent UB and still prevent read, validate, read, use problems. However, a
  // multi-field logical metadata item can be "torn", and if torn, will be seen as torn by the
  // validate step. The torn metadata item can only happen if a process is misbehaving.
  //
  // Always aligned to a page boundary.
  struct SharedMutableMetadataBase {
   public:
    std::atomic<Dataspace> data_space;
    std::atomic<BlendMode> blend_mode;

    // We store Cta861_3 (and other multi-field structs below) as separate fields so that each field
    // can be lock-free, since cross process locking with a lock in shared memory generally isn't ok
    // for DoS reasons at least. This means the overall Cta861_3 info can be torn, but only if a
    // thread is writing when it shouldn't. In that case we mitigate read, verify, read, use, and we
    // avoid UB, but we don't prevent the torn Cta861_3.

    // non-zero corresponds to std::optional<>.has_value() (see StandardMetadata<SMPTE2086>).
    std::atomic<uint32_t> smpte2086__has_value;
    std::atomic<float> smpte2086__primaryRed__x;
    std::atomic<float> smpte2086__primaryRed__y;
    std::atomic<float> smpte2086__primaryGreen__x;
    std::atomic<float> smpte2086__primaryGreen__y;
    std::atomic<float> smpte2086__primaryBlue__x;
    std::atomic<float> smpte2086__primaryBlue__y;
    std::atomic<float> smpte2086__whitePoint__x;
    std::atomic<float> smpte2086__whitePoint__y;
    std::atomic<float> smpte2086__maxLuminance;
    std::atomic<float> smpte2086__minLuminance;

    // non-zero corresponds to std::optional<>.has_value() (see StandardMetadata<CTA861_3>).
    std::atomic<uint32_t> cta861_3__has_value;
    std::atomic<float> cta861_3__maxContentLightLevel;
    std::atomic<float> cta861_3__maxFrameAverageLightLevel;

    // SMPTE2094_10 and SMPTE2094_40 and SMPTE2094_50
    //
    // For consistency with how StandardMetadataType.aidl says it encodes these into a byte stream,
    // and consistent with the std::optional<std::vector<uint8_t>> type used (see
    // StandardMetadata<>), this can represent a has_value true zero length vector differently from
    // !has_value.
    //
    // TODO(b/291974637): It may be possible to trim down these MaxSizeBytes values. Consider adding
    // a specific max length requirement to comments in StandardMetadataTypes.aidl (padded or not)
    // so it's more obvious to gralloc implementers how many bytes is really needed for these (at
    // most).

    static constexpr uint32_t kSmpte2094_10_MaxSizeBytes = 1024;
    // non-zero if std::optional.has_value() (see StandardMetadata<SMPTE2094_10>).
    std::atomic<uint32_t> smpte2094_10__has_value;
    // Validated to be <= kSmpte2094_10_MaxSizeBytes.
    std::atomic<uint32_t> smpte2094_10__size_bytes;
    // Will only ever read up to kSmpte2094_10_MaxSizeBytes regardless of smpte2094_10__size_bytes.
    std::atomic<uint8_t> smpte2094_10__bytes[kSmpte2094_10_MaxSizeBytes];

    static constexpr uint32_t kSmpte2094_40_MaxSizeBytes = 1024;
    // non-zero if std::optional.has_value() (see StandardMetadata<SMPTE2094_40>).
    std::atomic<uint32_t> smpte2094_40__has_value;
    // Validated to be <= kSmpte2094_40_MaxSizeBytes.
    std::atomic<uint32_t> smpte2094_40__size_bytes;
    // Will only ever read up to kSmpte2094_40_MaxSizeBytes regardless of smpte2094_40__size_bytes.
    std::atomic<uint8_t> smpte2094_40__bytes[kSmpte2094_40_MaxSizeBytes];

    // The minimum allowed value for kSmpte2094_50_MaxSizeBytes comes from
    // GraphicsMapperStableCTests#GetSetSmpte2094_50 as the "10 KB" value defined as 10 * 1028 (1028
    // not 1024). It's TBD if any valid kSmpte2094_50 data will ever be this large. The "SMPTE ST
    // 2094-50" spec allows future versions of smpte2094_50 to have unspecified arbitrary size (see
    // "application_version" and "minimum_application_version"). The good news is that VMO pages
    // aren't committed until actually touched.
    //
    // This value is chosen to allow the next field after smpte2094_50__bytes to start at offset 16
    // KiB which is a 4KiB page boundary.
    //
    // This value can safely be increased (server first, then clients) until the field after
    // smpte2094_50__bytes is added. This value cannot safely be decreased.
    static constexpr uint32_t kSmpte2094_50_MaxSizeBytes = (16 * 1024) - 2136;
    // see comment above; this static_assert does not imply that kSmpte2094_50_MaxSizeBytes can be
    // decreased from its current value just above, as that would require soft transition steps
    static_assert(kSmpte2094_50_MaxSizeBytes >= 10 * 1028);
    // non-zero if std::optional.has_value() (see StandardMetadata<SMPTE2094_50>).
    std::atomic<uint32_t> smpte2094_50__has_value;
    // Validated to be <= kSmpte2094_50_MaxSizeBytes (from reader's point of view).
    std::atomic<uint32_t> smpte2094_50__size_bytes;
    // Will only ever read up to kSmpte2094_50_MaxSizeBytes (from reader's point of view) regardless
    // of smpte2094_50__size_bytes.
    //
    // As of this comment, typically the offset of smpte2094_50__bytes[smpte2094_50__size_bytes-1]
    // will be within the first 4KiB page.
    std::atomic<uint8_t> smpte2094_50__bytes[kSmpte2094_50_MaxSizeBytes];

    // The next field after this is at offset 16 KiB. This maximises the page packing efficiency of
    // metadata beyond smpte2094_50, given that the 4KiB page containing the last part of
    // smpte2094_50__bytes is unlikely to be committed.
    //
    // If adding a new field, this author recommends reading the most recent version of "SMPTE ST
    // 2094-50" to determine if kSmpte2094_50_MaxSizeBytes should be increased before adding the
    // next field, because if/when "SMPTE ST 2094-50" allows exceeding kSmpte2094_50_MaxSizeBytes
    // after we have a subsequent field, we'd need to split the smpte2094_50__bytes across multiple
    // fields and/or migrate to a new FD with new layout.
    //
    // Any new field here should be defined such that the initial all-zeroes when a page is not yet
    // committed will be the valid default value per StandardMetadataType.aidl comments. For most
    // image buffers, this may avoid allocating a physical page for pages of this metadata struct/FD
    // beyond page 0. For most non-image buffers, this may allow this metadata struct/FD to consume
    // zero physical pages.
  };
  struct SharedMutableMetadata final : public SharedMutableMetadataBase {};

  static_assert(std::is_standard_layout_v<SharedMutableMetadata>);

  struct PrivateSharedMutableMetadata final : public SharedMutableMetadataBase {
    static constexpr bool is_expected_field_offsets() {
      static_assert(std::is_standard_layout_v<PrivateSharedImmutableMetadata>);
      // Existing fields above are constrained to their existing offsets, short of a soft transition
      // to a different layout (and/or an additional FD with new layout scheme).
      static_assert(offsetof(PrivateSharedMutableMetadata, data_space) == 0);
      static_assert(offsetof(PrivateSharedMutableMetadata, blend_mode) == 4);
      static_assert(offsetof(PrivateSharedMutableMetadata, smpte2086__has_value) == 8);
      static_assert(offsetof(PrivateSharedMutableMetadata, smpte2086__primaryRed__x) == 12);
      static_assert(offsetof(PrivateSharedMutableMetadata, smpte2086__primaryRed__y) == 16);
      static_assert(offsetof(PrivateSharedMutableMetadata, smpte2086__primaryGreen__x) == 20);
      static_assert(offsetof(PrivateSharedMutableMetadata, smpte2086__primaryGreen__y) == 24);
      static_assert(offsetof(PrivateSharedMutableMetadata, smpte2086__primaryBlue__x) == 28);
      static_assert(offsetof(PrivateSharedMutableMetadata, smpte2086__primaryBlue__y) == 32);
      static_assert(offsetof(PrivateSharedMutableMetadata, smpte2086__whitePoint__x) == 36);
      static_assert(offsetof(PrivateSharedMutableMetadata, smpte2086__whitePoint__y) == 40);
      static_assert(offsetof(PrivateSharedMutableMetadata, smpte2086__maxLuminance) == 44);
      static_assert(offsetof(PrivateSharedMutableMetadata, smpte2086__minLuminance) == 48);
      static_assert(offsetof(PrivateSharedMutableMetadata, cta861_3__has_value) == 52);
      static_assert(offsetof(PrivateSharedMutableMetadata, cta861_3__maxContentLightLevel) == 56);
      static_assert(offsetof(PrivateSharedMutableMetadata, cta861_3__maxFrameAverageLightLevel) ==
                    60);
      static_assert(offsetof(PrivateSharedMutableMetadata, smpte2094_10__has_value) == 64);
      static_assert(offsetof(PrivateSharedMutableMetadata, smpte2094_10__size_bytes) == 68);
      static_assert(offsetof(PrivateSharedMutableMetadata, smpte2094_10__bytes) == 72);
      static_assert(offsetof(PrivateSharedMutableMetadata, smpte2094_40__has_value) == 1096);
      static_assert(offsetof(PrivateSharedMutableMetadata, smpte2094_40__size_bytes) == 1100);
      static_assert(offsetof(PrivateSharedMutableMetadata, smpte2094_40__bytes) == 1104);
      static_assert(offsetof(PrivateSharedMutableMetadata, smpte2094_50__has_value) == 2128);
      static_assert(offsetof(PrivateSharedMutableMetadata, smpte2094_50__size_bytes) == 2132);
      static_assert(offsetof(PrivateSharedMutableMetadata, smpte2094_50__bytes) == 2136);
      // This size is allowed to increase if done in a backward-compatible way. This is the expected
      // current value.
      static_assert(sizeof(PrivateSharedMutableMetadata) == 16ull * 1024);
      return true;
    }
  };
  // without this, the static_assert()s in is_expected_field_offsets() are ignored
  static_assert(PrivateSharedMutableMetadata::is_expected_field_offsets());

  // This is essentially AIDL BufferDescriptorInfo, but with everything in-band.
  //
  // This can be used by a HIDL mapper's createDescriptor and a HIDL IAllocator's allocate and/or an
  // AIDL IAllocator allocate (in contrast to an AIDL IAllocator allocate2 which doesn't need this).
  //
  // TODO(b/306297561): This is temporary; remove when the HIDL mapper has been removed, which may
  // not be possible for a while due to Stable-C mapper currently requiring use of out-of-proc AIDL
  // to discover the name of the Stable-C mapper.fuchsia-starnix-mapper.so, which is inconvenient
  // for Vulkan ICDs for example.
  struct SerializedBufferDescriptorBase {
   public:
    static constexpr uint32_t kNameBufferLength =
        SharedImmutableStandardMetadata::kNameBufferLength;
    static constexpr uint32_t kMaxNameLength = SharedImmutableStandardMetadata::kMaxNameLength;

    // We preserve as much of the name as SharedImmutableStandardMetadata preserves. This is always
    // 0-terminated _if_ the calling code is well-behaved, but the allocator can't assume that.
    char name[kNameBufferLength];
    uint32_t width;
    uint32_t height;
    uint32_t layerCount;
    hidl_PixelFormat format;
    hidl_BufferUsage usage;
    uint64_t reservedSize;
  };
  struct SerializedBufferDescriptor : public SerializedBufferDescriptorBase {};

  static_assert(std::is_standard_layout_v<SerializedBufferDescriptor>);

  struct PrivateSerializedBufferDescriptor final : public SerializedBufferDescriptorBase {
    static constexpr bool is_expected_field_offsets() {
      static_assert(std::is_standard_layout_v<PrivateSerializedBufferDescriptor>);
      // Don't change existing offsets.
      static_assert(offsetof(PrivateSerializedBufferDescriptor, name) == 0);
      static_assert(offsetof(PrivateSerializedBufferDescriptor, width) == 132);
      static_assert(offsetof(PrivateSerializedBufferDescriptor, height) == 136);
      static_assert(offsetof(PrivateSerializedBufferDescriptor, layerCount) == 140);
      static_assert(offsetof(PrivateSerializedBufferDescriptor, format) == 144);
      static_assert(offsetof(PrivateSerializedBufferDescriptor, usage) == 152);
      static_assert(offsetof(PrivateSerializedBufferDescriptor, reservedSize) == 160);
      return true;
    }
  };
  // without this, the static_assert()s in is_expected_field_offsets() are ignored
  static_assert(PrivateSerializedBufferDescriptor::is_expected_field_offsets());

  // If one of these static_assert()s starts failing for a new/updated toolchain (assuming
  // correctly-specified template arguments), that might indicate that we need to switch from
  // std::atomic<> to using common/shared .S assembly per supported architecture. Note that
  // essentially the same caveat would apply if we used c11 stdatomic.h -- for example if two
  // toolchains targeting the same architecture were to disagree about ATOMIC_LONG_LOCK_FREE. For
  // supported architectures and relevant toolchains, the std::atomic<> types used in this file are
  // expected (in a pragmatic sense) to be lock free and compatible in practice. These
  // static_assert()s are here to try to catch some easily detectable incompatibilities in case
  // expectations prove wrong, but these static_assert()s should not be misunderstood to be capable
  // of detecting the entire space of possible incompatibilities downstream from all possible
  // permitted implementations of std::atomic<T> from a C++ spec perspective -- these are just a
  // best-effort attempt at detecting an incmoplete set of potential compatibility problems.

  template <typename T>
  struct check_gralloc_writable_metadata_field_type final {
    // Regarding is_always_lock_free, std::atomic<T> is allowed to have alignof(std::atomic<T>) ==
    // alignof(T), and the compiler can assume proper alignment consistent with alignof(). This is
    // how is_always_lock_free can be true even if a not-naturally-aligned case would not be
    // supported in lock-free fashion by the arch.
    static constexpr bool value =
        !std::is_enum_v<T> && std::is_standard_layout_v<T> &&
        std::is_standard_layout_v<std::atomic<T>> && (sizeof(T) == sizeof(std::atomic<T>)) &&
        (alignof(T) == alignof(std::atomic<T>)) && std::atomic<T>::is_always_lock_free;
  };

  template <typename T>
  static constexpr bool check_gralloc_writable_metadata_field_type_v =
      check_gralloc_writable_metadata_field_type<T>::value;

  template <typename T>
  struct check_gralloc_writable_metadata_enum_field_type final {
    static constexpr bool value =
        std::is_enum_v<T> &&
        check_gralloc_writable_metadata_field_type_v<std::underlying_type_t<T>> &&
        (sizeof(std::atomic<T>) == sizeof(std::atomic<std::underlying_type_t<T>>)) &&
        (sizeof(T) == sizeof(std::atomic<T>)) && (alignof(T) == alignof(std::atomic<T>)) &&
        std::atomic<T>::is_always_lock_free;
  };

  template <typename T>
  static constexpr bool check_gralloc_writable_metadata_enum_field_type_v =
      check_gralloc_writable_metadata_enum_field_type<T>::value;

  static_assert(check_gralloc_writable_metadata_field_type_v<uint8_t>);
  static_assert(check_gralloc_writable_metadata_field_type_v<int8_t>);
  // char is always 1 byte
  static_assert(check_gralloc_writable_metadata_field_type_v<char>);
  // unsigned char is always 1 byte
  static_assert(check_gralloc_writable_metadata_field_type_v<unsigned char>);
  static_assert(check_gralloc_writable_metadata_field_type_v<uint32_t>);
  static_assert(check_gralloc_writable_metadata_field_type_v<uint64_t>);
  static_assert(check_gralloc_writable_metadata_field_type_v<int32_t>);
  static_assert(check_gralloc_writable_metadata_field_type_v<int64_t>);

  static_assert(sizeof(float) == 4);
  static_assert(sizeof(double) == 8);
  static_assert(check_gralloc_writable_metadata_field_type_v<float>);
  static_assert(check_gralloc_writable_metadata_field_type_v<double>);

  static_assert(check_gralloc_writable_metadata_enum_field_type_v<BlendMode>);
  static_assert(check_gralloc_writable_metadata_enum_field_type_v<Dataspace>);

 public:
  using ImmutableStandard = SharedImmutableStandardMetadata;
  using Immutable = SharedImmutableMetadata;
  using Mutable = SharedMutableMetadata;

  // TODO(b/306297561): This is temporary; remove when possible.
  using BufferDescriptor = SerializedBufferDescriptor;

  // Normal clients should not use these types, to avoid accidentally trying to write to immutable
  // metadata which is mapped read only and would cause a fault on attempt to write. Tests and the
  // gralloc allocator can use these types as base classes to allow writing to immutable metadata
  // fields (assuming the backing virtual page is mapped as writable).
  struct ForInitializationOnly final {
    using ImmutableStandardBase = SharedImmutableStandardMetadataBase;
    using ImmutableBase = SharedImmutableMetadataBase;
  };
};

#endif  // LIB_VIRTGRALLOC_SHARED_METADATA_H_
