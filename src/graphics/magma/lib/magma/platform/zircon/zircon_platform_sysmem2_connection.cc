// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/fpromise/result.h>
#include <lib/magma/magma_common_defs.h>
#include <lib/magma/platform/platform_sysmem_connection.h>
#include <lib/magma/platform/platform_thread.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma/util/utils.h>
#include <lib/zx/channel.h>
#include <zircon/availability.h>

#include <limits>
#include <unordered_set>
#include <vector>

// sysmem2 was added to the SDK at 19
#if __Fuchsia_API_level__ >= 19

#include <lib/image-format/image_format.h>

using magma::Status;

namespace magma_sysmem {

namespace {
uint32_t SysmemToMagmaFormat(fuchsia_images2::PixelFormat format) {
  // The values are required to be identical.
  return static_cast<uint32_t>(format);
}

magma_status_t SysmemToMagmaError(fuchsia_sysmem2::Error error) {
  switch (error) {
    case fuchsia_sysmem2::Error::kInvalid:
      return MAGMA_STATUS_INTERNAL_ERROR;

    case fuchsia_sysmem2::Error::kUnspecified:
      return MAGMA_STATUS_INTERNAL_ERROR;

    case fuchsia_sysmem2::Error::kProtocolDeviation:
      return MAGMA_STATUS_INVALID_ARGS;

    case fuchsia_sysmem2::Error::kNotFound:
      return MAGMA_STATUS_INVALID_ARGS;

    case fuchsia_sysmem2::Error::kHandleAccessDenied:
      return MAGMA_STATUS_ACCESS_DENIED;

    case fuchsia_sysmem2::Error::kNoMemory:
      return MAGMA_STATUS_MEMORY_ERROR;

    case fuchsia_sysmem2::Error::kConstraintsIntersectionEmpty:
      return MAGMA_STATUS_CONSTRAINTS_INTERSECTION_EMPTY;

    case fuchsia_sysmem2::Error::kPending:
      return MAGMA_STATUS_BAD_STATE;

    case fuchsia_sysmem2::Error::kTooManyGroupChildCombinations:
      return MAGMA_STATUS_TOO_MANY_GROUP_CHILD_COMBINATIONS;

    default:
      return MAGMA_STATUS_INTERNAL_ERROR;
  }
}

static_assert(MAGMA_FORMAT_MODIFIER_INTEL_X_TILED ==
              fidl::ToUnderlying(fuchsia_images2::PixelFormatModifier::kIntelI915XTiled));
static_assert(MAGMA_FORMAT_MODIFIER_INTEL_Y_TILED ==
              fidl::ToUnderlying(fuchsia_images2::PixelFormatModifier::kIntelI915YTiled));
static_assert(MAGMA_FORMAT_MODIFIER_INTEL_YF_TILED ==
              fidl::ToUnderlying(fuchsia_images2::PixelFormatModifier::kIntelI915YfTiled));
static_assert(MAGMA_FORMAT_MODIFIER_INTEL_Y_TILED_CCS ==
              fidl::ToUnderlying(fuchsia_images2::PixelFormatModifier::kIntelI915YTiledCcs));
static_assert(MAGMA_FORMAT_MODIFIER_INTEL_YF_TILED_CCS ==
              fidl::ToUnderlying(fuchsia_images2::PixelFormatModifier::kIntelI915YfTiledCcs));

}  // namespace

class ZirconPlatformSysmem2BufferDescription : public PlatformBufferDescription {
 public:
  ZirconPlatformSysmem2BufferDescription(uint32_t buffer_count,
                                         fuchsia_sysmem2::SingleBufferSettings settings)
      : buffer_count_(buffer_count), settings_(std::move(settings)) {}
  ~ZirconPlatformSysmem2BufferDescription() override = default;

  bool IsValid() const {
    using fuchsia_sysmem2::CoherencyDomain;
    switch (*settings_.buffer_settings()->coherency_domain()) {
      case CoherencyDomain::kRam:
      case CoherencyDomain::kCpu:
      case CoherencyDomain::kInaccessible:
        break;

      default:
        return DRETF(false, "Unsupported coherency domain: %d",
                     static_cast<uint32_t>(*settings_.buffer_settings()->coherency_domain()));
    }
    return true;
  }

  bool is_secure() const override {
    DASSERT(settings_.buffer_settings().has_value());
    return *settings_.buffer_settings()->is_secure();
  }

  uint32_t count() const override { return buffer_count_; }
  uint32_t format() const override {
    return settings_.image_format_constraints().has_value()
               ? SysmemToMagmaFormat(*settings_.image_format_constraints()->pixel_format())
               : MAGMA_FORMAT_INVALID;
  }
  bool has_format_modifier() const override {
    return settings_.image_format_constraints().has_value() &&
           settings_.image_format_constraints()->pixel_format_modifier().has_value() &&
           *settings_.image_format_constraints()->pixel_format_modifier() !=
               fuchsia_images2::PixelFormatModifier::kLinear;
  }
  uint64_t format_modifier() const override {
    if (!settings_.image_format_constraints().has_value()) {
      return fidl::ToUnderlying(fuchsia_images2::PixelFormatModifier::kLinear);
    }
    const auto& ifc = *settings_.image_format_constraints();
    return fidl::ToUnderlying(ifc.pixel_format_modifier().has_value()
                                  ? *ifc.pixel_format_modifier()
                                  : fuchsia_images2::PixelFormatModifier::kLinear);
  }
  uint32_t coherency_domain() const override {
    DASSERT(settings_.buffer_settings().has_value());
    DASSERT(settings_.buffer_settings()->coherency_domain().has_value());
    using fuchsia_sysmem2::CoherencyDomain;
    switch (*settings_.buffer_settings()->coherency_domain()) {
      case CoherencyDomain::kRam:
        return MAGMA_COHERENCY_DOMAIN_RAM;

      case CoherencyDomain::kCpu:
        return MAGMA_COHERENCY_DOMAIN_CPU;

      case CoherencyDomain::kInaccessible:
        return MAGMA_COHERENCY_DOMAIN_INACCESSIBLE;

      default:
        // Checked by IsValid()
        DASSERT(false);
        return MAGMA_COHERENCY_DOMAIN_CPU;
    }
  }

  bool GetColorSpace(uint32_t* color_space_out) override {
    if (!settings_.image_format_constraints().has_value()) {
      return false;
    }
    if (!settings_.image_format_constraints()->color_spaces().has_value()) {
      return false;
    }
    // Only report first colorspace for now.
    if (settings_.image_format_constraints()->color_spaces()->size() < 1) {
      return false;
    }
    *color_space_out =
        fidl::ToUnderlying(settings_.image_format_constraints()->color_spaces()->at(0));
    return true;
  }

  bool GetPlanes(uint64_t width, uint64_t height, magma_image_plane_t* planes_out) const override {
    if (!settings_.image_format_constraints().has_value()) {
      return false;
    }

    for (uint32_t i = 0; i < MAGMA_MAX_IMAGE_PLANES; ++i) {
      planes_out[i].byte_offset = 0;
      planes_out[i].bytes_per_row = 0;
    }

    fpromise::result<fuchsia_images2::ImageFormat> image_format = ImageConstraintsToFormat(
        *settings_.image_format_constraints(), magma::to_uint32(width), magma::to_uint32(height));
    if (!image_format) {
      return DRETF(false, "Image format not valid");
    }
    for (uint32_t plane = 0; plane < MAGMA_MAX_IMAGE_PLANES; ++plane) {
      uint64_t offset;
      bool plane_valid = ImageFormatPlaneByteOffset(image_format.value(), plane, &offset);
      if (!plane_valid) {
        planes_out[plane].byte_offset = 0;
      } else {
        planes_out[plane].byte_offset = magma::to_uint32(offset);
      }
      uint32_t row_bytes;
      if (ImageFormatPlaneRowBytes(image_format.value(), plane, &row_bytes)) {
        planes_out[plane].bytes_per_row = row_bytes;
      } else {
        planes_out[plane].bytes_per_row = 0;
      }
    }
    return true;
  }

  bool GetFormatIndex(PlatformBufferConstraints* constraints, magma_bool_t* format_valid_out,
                      uint32_t format_valid_count) override;

 private:
  const uint32_t buffer_count_;
  const fuchsia_sysmem2::SingleBufferSettings settings_;
};

class ZirconPlatformSysmem2BufferConstraints : public PlatformBufferConstraints {
 public:
  ~ZirconPlatformSysmem2BufferConstraints() override = default;

  explicit ZirconPlatformSysmem2BufferConstraints(
      const magma_buffer_format_constraints_t* constraints) {
    if (constraints->count != 0) {
      constraints_.min_buffer_count() = constraints->count;
    }
    // Ignore input usage
    auto& usage = constraints_.usage().emplace();
    usage.vulkan() = fuchsia_sysmem2::kVulkanImageUsageTransientAttachment |
                     fuchsia_sysmem2::kVulkanImageUsageStencilAttachment |
                     fuchsia_sysmem2::kVulkanImageUsageInputAttachment |
                     fuchsia_sysmem2::kVulkanImageUsageColorAttachment |
                     fuchsia_sysmem2::kVulkanImageUsageTransferSrc |
                     fuchsia_sysmem2::kVulkanImageUsageTransferDst |
                     fuchsia_sysmem2::kVulkanImageUsageStorage |
                     fuchsia_sysmem2::kVulkanImageUsageSampled;
    auto& bmc = constraints_.buffer_memory_constraints().emplace();

    // No buffer constraints, except those passed directly through from the client. These two
    // are for whether this memory should be protected (e.g. usable for DRM content, the precise
    // definition depending on the system).
    if (constraints->secure_required) {
      bmc.secure_required() = constraints->secure_required;
    }
    // This must be true when secure_required is true.
    DASSERT(constraints->secure_permitted || !constraints->secure_required);
    bmc.inaccessible_domain_supported() = constraints->secure_permitted;

    bmc.ram_domain_supported() = constraints->ram_domain_supported;
    bmc.cpu_domain_supported() = constraints->cpu_domain_supported;

    if (constraints->min_size_bytes != 0) {
      bmc.min_size_bytes() = constraints->min_size_bytes;
    }
    ZX_ASSERT(!(constraints->options & ~(MAGMA_BUFFER_FORMAT_CONSTRAINT_OPTIONS_EXTRA_COUNTS)));
    if (constraints->options & MAGMA_BUFFER_FORMAT_CONSTRAINT_OPTIONS_EXTRA_COUNTS) {
      if (constraints->max_buffer_count != 0) {
        constraints_.max_buffer_count() = constraints->max_buffer_count;
      }
      if (constraints->min_buffer_count_for_camping != 0) {
        constraints_.min_buffer_count_for_camping() = constraints->min_buffer_count_for_camping;
      }
      if (constraints->min_buffer_count_for_dedicated_slack != 0) {
        constraints_.min_buffer_count_for_dedicated_slack() =
            constraints->min_buffer_count_for_dedicated_slack;
      }
      if (constraints->min_buffer_count_for_shared_slack != 0) {
        constraints_.min_buffer_count_for_shared_slack() =
            constraints->min_buffer_count_for_shared_slack;
      }
    }
  }

  Status SetImageFormatConstraints(
      uint32_t index, const magma_image_format_constraints2_t* format_constraints) override {
    using fuchsia_images2::ColorSpace;
    using fuchsia_images2::PixelFormat;

    if (index != raw_image_constraints_.size()) {
      return DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "Format constraint gaps or changes not allowed");
    }
    if (merge_result_) {
      return DRET_MSG(MAGMA_STATUS_INVALID_ARGS,
                      "Setting format constraints on merged constraints.");
    }

    fuchsia_sysmem2::ImageFormatConstraints constraints;
    // constraints.min_size() = {0, 0};
    constraints.max_size() = {16384, 16384};
    if (format_constraints->min_bytes_per_row != 0) {
      constraints.min_bytes_per_row() = format_constraints->min_bytes_per_row;
    }
    if (format_constraints->width > 0 || format_constraints->height > 0) {
      constraints.required_max_size() = {format_constraints->width, format_constraints->height};
    }
#if FUCHSIA_API_LEVEL_AT_LEAST(31)
    if (format_constraints->pad_for_block_width > 0 ||
        format_constraints->pad_for_block_height > 0) {
      constraints.pad_for_block_size() = {format_constraints->pad_for_block_width,
                                          format_constraints->pad_for_block_height};
    }
#endif

    bool is_yuv = false;
    static const magma_format_t yuv_formats[] = {
        MAGMA_FORMAT_NV12, MAGMA_FORMAT_I420,  MAGMA_FORMAT_M420, MAGMA_FORMAT_YV12,
        MAGMA_FORMAT_YUY2, MAGMA_FORMAT_MJPEG, MAGMA_FORMAT_P010,
    };
    static const magma_format_t rgb_formats[] = {
        MAGMA_FORMAT_R8G8B8A8, MAGMA_FORMAT_BGRA32, MAGMA_FORMAT_RGB565,     MAGMA_FORMAT_BGR24,
        MAGMA_FORMAT_RGB565,   MAGMA_FORMAT_RGB332, MAGMA_FORMAT_RGB2220,    MAGMA_FORMAT_L8,
        MAGMA_FORMAT_R8,       MAGMA_FORMAT_R8G8,   MAGMA_FORMAT_A2B10G10R10};
    if (std::find(std::begin(yuv_formats), std::end(yuv_formats),
                  format_constraints->image_format) != std::end(yuv_formats)) {
      constraints.pixel_format() =
          static_cast<fuchsia_images2::PixelFormat>(format_constraints->image_format);
      is_yuv = true;
    } else if (std::find(std::begin(rgb_formats), std::end(rgb_formats),
                         format_constraints->image_format) != std::end(rgb_formats)) {
      constraints.pixel_format() =
          static_cast<fuchsia_images2::PixelFormat>(format_constraints->image_format);
    } else {
      return DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "Invalid format: %d",
                      format_constraints->image_format);
    }
    auto& color_spaces = constraints.color_spaces().emplace();
    if (is_yuv) {
      // This is the full list of formats currently supported by
      // VkSamplerYcbcrModelConversion and VkSamplerYcbcrRange as of vulkan 1.1,
      // restricted to 8-bit-per-component formats.
      color_spaces.emplace_back(ColorSpace::kRec601Ntsc);
      color_spaces.emplace_back(ColorSpace::kRec601NtscFullRange);
      color_spaces.emplace_back(ColorSpace::kRec601Pal);
      color_spaces.emplace_back(ColorSpace::kRec601PalFullRange);
      color_spaces.emplace_back(ColorSpace::kRec709);
    } else {
      color_spaces.emplace_back(ColorSpace::kSrgb);
    }

    if (format_constraints->has_format_modifier) {
      constraints.pixel_format_modifier() =
          static_cast<fuchsia_images2::PixelFormatModifier>(format_constraints->format_modifier);
    }
    if (format_constraints->bytes_per_row_divisor > 1) {
      constraints.bytes_per_row_divisor() = format_constraints->bytes_per_row_divisor;
    }
    raw_image_constraints_.push_back(std::move(constraints));

    return MAGMA_STATUS_OK;
  }

  magma::Status SetColorSpaces(uint32_t index, uint32_t color_space_count,
                               const uint32_t* color_spaces_param) override {
    if (index >= raw_image_constraints_.size()) {
      return DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "Format constraints must be set first");
    }
    if (color_space_count > fuchsia_sysmem2::kMaxCountImageFormatConstraintsColorSpaces) {
      return DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "Too many color spaces: %d", color_space_count);
    }
    auto& color_spaces = raw_image_constraints_[index].color_spaces().emplace();
    color_spaces.reserve(color_space_count);
    for (uint32_t i = 0; i < color_space_count; i++) {
      color_spaces.emplace_back(static_cast<fuchsia_images2::ColorSpace>(color_spaces_param[i]));
    }
    return MAGMA_STATUS_OK;
  }

  magma::Status AddAdditionalConstraints(
      const magma_buffer_format_additional_constraints_t* additional) override {
    if (additional->max_buffer_count != 0) {
      constraints_.max_buffer_count() = additional->max_buffer_count;
    }
    if (additional->min_buffer_count_for_camping != 0) {
      constraints_.min_buffer_count_for_camping() = additional->min_buffer_count_for_camping;
    }
    if (additional->min_buffer_count_for_dedicated_slack != 0) {
      constraints_.min_buffer_count_for_dedicated_slack() =
          additional->min_buffer_count_for_dedicated_slack;
    }
    if (additional->min_buffer_count_for_shared_slack != 0) {
      constraints_.min_buffer_count_for_shared_slack() =
          additional->min_buffer_count_for_shared_slack;
    }
    return MAGMA_STATUS_OK;
  }

  // Merge image format constraints with identical pixel formats, since sysmem can't handle
  // duplicate pixel formats in this list.
  bool MergeRawConstraints() {
    if (merge_result_) {
      return *merge_result_;
    }
    if (!constraints_.image_format_constraints().has_value()) {
      constraints_.image_format_constraints().emplace();
    }
    for (const auto& in_constraints : raw_image_constraints_) {
      uint32_t j = 0;
      for (; j < constraints_.image_format_constraints()->size(); j++) {
        auto& out_constraints = constraints_.image_format_constraints()->at(j);
        fuchsia_images2::PixelFormatModifier out_modifier =
            out_constraints.pixel_format_modifier().has_value()
                ? *out_constraints.pixel_format_modifier()
                : fuchsia_images2::PixelFormatModifier::kLinear;
        fuchsia_images2::PixelFormatModifier in_modifier =
            in_constraints.pixel_format_modifier().has_value()
                ? *in_constraints.pixel_format_modifier()
                : fuchsia_images2::PixelFormatModifier::kLinear;
        if (*in_constraints.pixel_format() == *out_constraints.pixel_format() &&
            in_modifier == out_modifier) {
          break;
        }
      }
      if (j == constraints_.image_format_constraints()->size()) {
        if (constraints_.image_format_constraints()->size() >=
            fuchsia_sysmem2::kMaxCountBufferCollectionConstraintsImageFormatConstraints) {
          merge_result_.emplace(false);
          return DRETF(false, "Too many input image format constraints to merge");
        }
        // in_constraints must be preserved for later calls to GetFormatIndex, so we clone/copy here
        // not move
        constraints_.image_format_constraints()->push_back(in_constraints);
        continue;
      }
      auto& out_constraints = constraints_.image_format_constraints()->at(j);
      // In these constraints we generally want the most restrictive option, because being more
      // restrictive won't generally cause the allocation to fail, it will just cause it to be a bit
      // bigger than necessary.
      if (in_constraints.min_bytes_per_row().has_value()) {
        if (!out_constraints.min_bytes_per_row().has_value()) {
          out_constraints.min_bytes_per_row() = 0;
        }
        out_constraints.min_bytes_per_row() =
            std::max(*out_constraints.min_bytes_per_row(), *in_constraints.min_bytes_per_row());
      }
      if (in_constraints.required_max_size().has_value()) {
        if (!out_constraints.required_max_size().has_value()) {
          out_constraints.required_max_size() = {0, 0};
        }
        auto& out_max_size = *out_constraints.required_max_size();
        out_max_size.width() =
            std::max(in_constraints.required_max_size()->width(), out_max_size.width());
        out_max_size.height() =
            std::max(in_constraints.required_max_size()->height(), out_max_size.height());
      }
      if (in_constraints.bytes_per_row_divisor().has_value()) {
        if (!out_constraints.bytes_per_row_divisor().has_value()) {
          out_constraints.bytes_per_row_divisor() = 0;
        }
        out_constraints.bytes_per_row_divisor() = std::max(
            *in_constraints.bytes_per_row_divisor(), *out_constraints.bytes_per_row_divisor());
      }

      // Union the sets of color spaces to ensure that they're all still legal.
      std::unordered_set<uint32_t> color_spaces;
      if (out_constraints.color_spaces().has_value()) {
        for (uint32_t j = 0; j < out_constraints.color_spaces()->size(); j++) {
          color_spaces.insert(fidl::ToUnderlying(out_constraints.color_spaces()->at(j)));
        }
      }
      if (in_constraints.color_spaces().has_value()) {
        for (uint32_t j = 0; j < in_constraints.color_spaces()->size(); j++) {
          color_spaces.insert(fidl::ToUnderlying(in_constraints.color_spaces()->at(j)));
        }
      }
      if (color_spaces.size() > fuchsia_sysmem2::kMaxCountImageFormatConstraintsColorSpaces) {
        merge_result_.emplace(false);
        return DRETF(false, "Too many input color spaces to merge");
      }

      out_constraints.color_spaces().emplace();
      out_constraints.color_spaces()->reserve(color_spaces.size());
      for (auto color_space : color_spaces) {
        out_constraints.color_spaces()->emplace_back(color_space);
      }
    }
    merge_result_.emplace(true);
    return true;
  }

  fuchsia_sysmem2::BufferCollectionConstraints take_constraints() {
    DASSERT(merge_result_);
    DASSERT(*merge_result_);
    DASSERT(!constraints_taken_);
    constraints_taken_ = true;
    auto result = std::move(constraints_);
    // avoid leaving around any pesky moved-out std::optional<>(s) that'd still claim has_value()
    constraints_ = fuchsia_sysmem2::BufferCollectionConstraints{};
    return result;
  }

  const std::vector<fuchsia_sysmem2::ImageFormatConstraints>& raw_image_constraints() {
    return raw_image_constraints_;
  }

 private:
  std::optional<bool> merge_result_;
  bool constraints_taken_ = false;
  fuchsia_sysmem2::BufferCollectionConstraints constraints_;
  std::vector<fuchsia_sysmem2::ImageFormatConstraints> raw_image_constraints_;
};

class ZirconPlatformSysmem2BufferCollection : public PlatformBufferCollection {
 public:
  ~ZirconPlatformSysmem2BufferCollection() override {
    if (collection_) {
      [[maybe_unused]] auto result = collection_->Release();
    }
  }

  Status Bind(fidl::SyncClient<fuchsia_sysmem2::Allocator>& allocator, uint32_t token_handle) {
    DASSERT(!collection_);
    auto endpoints = fidl::CreateEndpoints<fuchsia_sysmem2::BufferCollection>();
    if (!endpoints.is_ok()) {
      return DRET_MSG(MAGMA_STATUS_INTERNAL_ERROR, "Failed to create channels: %d",
                      endpoints.status_value());
    }

    fidl::Request<fuchsia_sysmem2::Allocator::BindSharedCollection> bind_shared_request;
    bind_shared_request.token() =
        fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>(zx::channel(token_handle));
    bind_shared_request.buffer_collection_request() = std::move(endpoints->server);
    auto bind_shared_result = allocator->BindSharedCollection(std::move(bind_shared_request));
    if (bind_shared_result.is_error()) {
      return DRET_MSG(MAGMA_STATUS_CONNECTION_LOST, "Internal error: %s",
                      bind_shared_result.error_value().FormatDescription().c_str());
    }

    collection_ = fidl::SyncClient(std::move(endpoints->client));

    return MAGMA_STATUS_OK;
  }

  Status SetConstraints(PlatformBufferConstraints* constraints) override {
    auto platform_constraints = static_cast<ZirconPlatformSysmem2BufferConstraints*>(constraints);
    if (!platform_constraints->MergeRawConstraints()) {
      return DRET_MSG(MAGMA_STATUS_INTERNAL_ERROR, "Merging constraints failed.");
    }
    auto llcpp_constraints = platform_constraints->take_constraints();

    auto& bmc = *llcpp_constraints.buffer_memory_constraints();
    bool secure_required = bmc.secure_required().has_value() && *bmc.secure_required();
    const char* buffer_name =
        secure_required ? "MagmaProtectedSysmemShared" : "MagmaUnprotectedSysmemShared";
    // These names are very generic, so set a low priority so it's easy to override them.
    constexpr uint32_t kVulkanPriority = 5;
    fidl::Request<fuchsia_sysmem2::BufferCollection::SetName> set_name_request;
    set_name_request.priority() = kVulkanPriority;
    set_name_request.name() = buffer_name;
    auto set_name_result = collection_->SetName(std::move(set_name_request));
    if (set_name_result.is_error()) {
      return DRET_MSG(MAGMA_STATUS_CONNECTION_LOST, "Error setting name: %s",
                      set_name_result.error_value().FormatDescription().c_str());
    }

    fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constrants_request;
    set_constrants_request.constraints() = std::move(llcpp_constraints);
    auto set_constraints_result = collection_->SetConstraints(std::move(set_constrants_request));
    if (set_constraints_result.is_error()) {
      return DRET_MSG(MAGMA_STATUS_CONNECTION_LOST, "Error setting constraints: %s",
                      set_constraints_result.error_value().FormatDescription().c_str());
    }
    return MAGMA_STATUS_OK;
  }

  Status GetBufferDescription(
      std::unique_ptr<PlatformBufferDescription>* description_out) override {
    auto result = collection_->WaitForAllBuffersAllocated();
    if (result.is_error()) {
      if (result.error_value().is_framework_error()) {
        return DRET_MSG(MAGMA_STATUS_CONNECTION_LOST, "Failed wait for allocation: %s",
                        result.error_value().FormatDescription().c_str());
      } else {
        return DRET_MSG(SysmemToMagmaError(result.error_value().domain_error()),
                        "WaitForBuffersAllocated failed: %s",
                        result.error_value().FormatDescription().c_str());
      }
    }
    auto buffer_collection_info = std::move(*result->buffer_collection_info());
    result->buffer_collection_info().reset();

    auto description = std::make_unique<ZirconPlatformSysmem2BufferDescription>(
        buffer_collection_info.buffers()->size(), std::move(*buffer_collection_info.settings()));
    buffer_collection_info.settings().reset();

    if (!description->IsValid()) {
      return DRET(MAGMA_STATUS_INTERNAL_ERROR);
    }

    *description_out = std::move(description);
    return MAGMA_STATUS_OK;
  }

  Status GetBufferHandle(uint32_t index, uint32_t* handle_out, uint32_t* offset_out) override {
    auto result = collection_->WaitForAllBuffersAllocated();
    if (result.is_error()) {
      if (result.error_value().is_framework_error()) {
        return DRET_MSG(MAGMA_STATUS_CONNECTION_LOST, "Failed wait for allocation: %s",
                        result.error_value().FormatDescription().c_str());
      } else {
        return DRET_MSG(SysmemToMagmaError(result.error_value().domain_error()),
                        "WaitForBuffersAllocated failed: %s",
                        result.error_value().FormatDescription().c_str());
      }
    }
    auto buffer_collection_info = std::move(*result->buffer_collection_info());

    if (buffer_collection_info.buffers()->size() < index) {
      return DRET(MAGMA_STATUS_INVALID_ARGS);
    }

    *handle_out = (*buffer_collection_info.buffers()->at(index).vmo()).release();
    *offset_out = magma::to_uint32(*buffer_collection_info.buffers()->at(index).vmo_usable_start());
    return MAGMA_STATUS_OK;
  }

 private:
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> collection_;
};

class ZirconPlatformSysmem2Connection : public PlatformSysmemConnection {
 public:
  explicit ZirconPlatformSysmem2Connection(fidl::SyncClient<fuchsia_sysmem2::Allocator> allocator)
      : sysmem_allocator_(std::move(allocator)) {
    const std::string process_name = magma::PlatformProcessHelper::GetCurrentProcessName();
    const size_t max_length =
        std::min(process_name.length(),
                 static_cast<size_t>(fuchsia_sysmem2::kMaxClientNameLength - 16 /* padding */));
    std::string debug_name = std::string("(2) magma[") + process_name.substr(0, max_length) + "]";
    fuchsia_sysmem2::AllocatorSetDebugClientInfoRequest set_debug_request;
    set_debug_request.name().emplace(debug_name);
    set_debug_request.id().emplace(magma::PlatformProcessHelper::GetCurrentProcessId());
    [[maybe_unused]] auto result =
        sysmem_allocator_->SetDebugClientInfo(std::move(set_debug_request));
  }

  magma_status_t AllocateBuffer(uint32_t flags, size_t size,
                                std::unique_ptr<magma::PlatformBuffer>* buffer_out) override {
    DASSERT(size != 0);
    fuchsia_sysmem2::BufferCollectionConstraints constraints;
    auto& usage = constraints.usage().emplace();
    usage.vulkan() = fuchsia_sysmem2::kVulkanImageUsageTransientAttachment |
                     fuchsia_sysmem2::kVulkanImageUsageStencilAttachment |
                     fuchsia_sysmem2::kVulkanImageUsageInputAttachment |
                     fuchsia_sysmem2::kVulkanImageUsageColorAttachment |
                     fuchsia_sysmem2::kVulkanImageUsageTransferSrc |
                     fuchsia_sysmem2::kVulkanImageUsageTransferDst |
                     fuchsia_sysmem2::kVulkanImageUsageStorage |
                     fuchsia_sysmem2::kVulkanImageUsageSampled;
    // sysmem2 has no VideoUsageHwProtected flag (at least not as of this comment) - secure_required
    // is below and is the actual way that both sysmem2 and sysmem(1) convey that info
    constraints.min_buffer_count_for_camping() = 1;
    auto& bmc = constraints.buffer_memory_constraints().emplace();
    bmc.min_size_bytes() = magma::to_uint32(size);
    // It's always ok to support inaccessible domain, though this does imply that CPU access will
    // potentially not be possible.
    bmc.inaccessible_domain_supported() = true;
    if (flags & MAGMA_SYSMEM_FLAG_PROTECTED) {
      bmc.secure_required() = true;
      // This defaults to true so we have to set it to false, since it's not allowed to specify
      // secure_required and cpu_domain_supported at the same time.
      bmc.cpu_domain_supported() = false;
      // This must be false if secure_required is true (either explicitly or via sysmem2 default);
      // go ahead and set explicitly to false here.
      bmc.ram_domain_supported() = false;
    }
    DASSERT(!constraints.image_format_constraints().has_value());

    std::string buffer_name =
        (flags & MAGMA_SYSMEM_FLAG_PROTECTED) ? "MagmaProtectedSysmem" : "MagmaUnprotectedSysmem";
    if (flags & MAGMA_SYSMEM_FLAG_FOR_CLIENT) {
      // Signal that the memory was allocated for a vkAllocateMemory that the client asked for
      // directly.
      buffer_name += "ForClient";
    }
    fuchsia_sysmem2::BufferCollectionInfo info;
    magma_status_t result = AllocateBufferCollection(std::move(constraints), buffer_name, &info);
    if (result != MAGMA_STATUS_OK) {
      return DRET(result);
    }

    if (info.buffers()->size() != 1) {
      return DRET(MAGMA_STATUS_INTERNAL_ERROR);
    }

    if (!info.buffers()->at(0).vmo().has_value() || !info.buffers()->at(0).vmo()->is_valid()) {
      return DRET(MAGMA_STATUS_INTERNAL_ERROR);
    }

    *buffer_out = magma::PlatformBuffer::Import(info.buffers()->at(0).vmo()->release());
    if (!buffer_out) {
      return DRET_MSG(MAGMA_STATUS_INTERNAL_ERROR, "PlatformBuffer::Import failed");
    }

    return MAGMA_STATUS_OK;
  }

  Status CreateBufferCollectionToken(uint32_t* handle_out) override {
    auto endpoints = fidl::CreateEndpoints<fuchsia_sysmem2::BufferCollectionToken>();
    if (!endpoints.is_ok()) {
      return DRET_MSG(MAGMA_STATUS_INTERNAL_ERROR, "Failed to create channels: %d",
                      endpoints.status_value());
    }

    fuchsia_sysmem2::AllocatorAllocateSharedCollectionRequest allocate_shared_request;
    allocate_shared_request.token_request() = std::move(endpoints->server);
    auto result = sysmem_allocator_->AllocateSharedCollection(std::move(allocate_shared_request));
    if (result.is_error()) {
      return DRET_MSG(MAGMA_STATUS_CONNECTION_LOST, "AllocateSharedCollection failed: %s",
                      result.error_value().FormatDescription().c_str());
    }

    *handle_out = endpoints->client.TakeChannel().release();
    return MAGMA_STATUS_OK;
  }

  Status ImportBufferCollection(
      uint32_t handle, std::unique_ptr<PlatformBufferCollection>* collection_out) override {
    auto collection = std::make_unique<ZirconPlatformSysmem2BufferCollection>();
    Status status = collection->Bind(sysmem_allocator_, handle);
    if (!status.ok()) {
      return DRET(status.get());
    }

    *collection_out = std::move(collection);
    return MAGMA_STATUS_OK;
  }

  Status CreateBufferConstraints(
      const magma_buffer_format_constraints_t* constraints,
      std::unique_ptr<PlatformBufferConstraints>* constraints_out) override {
    *constraints_out = std::make_unique<ZirconPlatformSysmem2BufferConstraints>(constraints);
    return MAGMA_STATUS_OK;
  }

 private:
  magma_status_t AllocateBufferCollection(fuchsia_sysmem2::BufferCollectionConstraints constraints,
                                          const std::string& name,
                                          fuchsia_sysmem2::BufferCollectionInfo* info_out) {
    auto endpoints = fidl::CreateEndpoints<fuchsia_sysmem2::BufferCollection>();
    if (!endpoints.is_ok()) {
      return DRET_MSG(MAGMA_STATUS_INTERNAL_ERROR, "Failed to create channels: %d",
                      endpoints.status_value());
    }

    fuchsia_sysmem2::AllocatorAllocateNonSharedCollectionRequest allocate_non_shared_request;
    allocate_non_shared_request.collection_request() = std::move(endpoints->server);
    auto allocate_non_shared_result =
        sysmem_allocator_->AllocateNonSharedCollection(std::move(allocate_non_shared_request));
    if (allocate_non_shared_result.is_error()) {
      return DRET_MSG(MAGMA_STATUS_CONNECTION_LOST, "Failed to allocate buffer: %s",
                      allocate_non_shared_result.error_value().FormatDescription().c_str());
    }

    fidl::SyncClient<fuchsia_sysmem2::BufferCollection> collection(std::move(endpoints->client));

    if (!name.empty()) {
      fuchsia_sysmem2::NodeSetNameRequest set_name_request;
      set_name_request.priority() = 10;
      set_name_request.name() = name;
      [[maybe_unused]] auto set_name_result = collection->SetName(std::move(set_name_request));
    }

    fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
    set_constraints_request.constraints() = std::move(constraints);
    auto set_constraints_result = collection->SetConstraints(std::move(set_constraints_request));
    if (set_constraints_result.is_error()) {
      return DRET_MSG(MAGMA_STATUS_CONNECTION_LOST, "Failed to set constraints: %s",
                      set_constraints_result.error_value().FormatDescription().c_str());
    }

    auto wait_result = collection->WaitForAllBuffersAllocated();

    {
      // Ignore failure - this just prevents unnecessary logged errors.
      [[maybe_unused]] auto release_result = collection->Release();
    }

    if (wait_result.is_error()) {
      if (wait_result.error_value().is_framework_error()) {
        return DRET_MSG(MAGMA_STATUS_CONNECTION_LOST, "Failed wait for allocation: %s",
                        wait_result.error_value().FormatDescription().c_str());
      } else {
        return DRET_MSG(SysmemToMagmaError(wait_result.error_value().domain_error()),
                        "WaitForBuffersAllocated failed: %s",
                        wait_result.error_value().FormatDescription().c_str());
      }
    }

    *info_out = std::move(*wait_result->buffer_collection_info());
    return MAGMA_STATUS_OK;
  }

  fidl::SyncClient<fuchsia_sysmem2::Allocator> sysmem_allocator_;
};

// static
std::unique_ptr<PlatformSysmemConnection> PlatformSysmemConnection::Import2(uint32_t handle) {
  zx::channel channel = zx::channel(handle);
  fidl::SyncClient<fuchsia_sysmem2::Allocator> sysmem_allocator(
      fidl::ClientEnd<fuchsia_sysmem2::Allocator>(std::move(channel)));
  return std::make_unique<ZirconPlatformSysmem2Connection>(std::move(sysmem_allocator));
}

bool ZirconPlatformSysmem2BufferDescription::GetFormatIndex(PlatformBufferConstraints* constraints,
                                                            magma_bool_t* format_valid_out,
                                                            uint32_t format_valid_count) {
  auto* zircon_constraints = static_cast<ZirconPlatformSysmem2BufferConstraints*>(constraints);

  const auto& llcpp_constraints = zircon_constraints->raw_image_constraints();
  if (format_valid_count < llcpp_constraints.size()) {
    return DRETF(false, "format_valid_count %d < image_format_constraints_count %ld",
                 format_valid_count, llcpp_constraints.size());
  }
  for (uint32_t i = 0; i < format_valid_count; i++) {
    format_valid_out[i] = false;
  }
  if (!settings_.image_format_constraints().has_value()) {
    DMESSAGE("!settings_.image_format_constraints().has_value()");
    return true;
  }
  const auto& out = *settings_.image_format_constraints();

  for (uint32_t i = 0; i < llcpp_constraints.size(); ++i) {
    const auto& in = llcpp_constraints[i];
    // These checks are sorted in order of how often they're expected to mismatch, from most likely
    // to least likely. They aren't always equality comparisons, since sysmem may change some values
    // in compatible ways on behalf of the other participants.
    DASSERT(out.pixel_format().has_value());
    DASSERT(in.pixel_format().has_value());
    if (*out.pixel_format() != *in.pixel_format()) {
      continue;
    }
    auto in_format_modifier = in.pixel_format_modifier().has_value()
                                  ? *in.pixel_format_modifier()
                                  : fuchsia_images2::PixelFormatModifier::kLinear;
    auto out_format_modifier = out.pixel_format_modifier().has_value()
                                   ? *out.pixel_format_modifier()
                                   : fuchsia_images2::PixelFormatModifier::kLinear;
    if (out_format_modifier != in_format_modifier) {
      continue;
    }
    auto out_min_bytes_per_row = out.min_bytes_per_row().has_value() ? *out.min_bytes_per_row() : 0;
    auto in_min_bytes_per_row = in.min_bytes_per_row().has_value() ? *in.min_bytes_per_row() : 0;
    if (out_min_bytes_per_row < in_min_bytes_per_row) {
      continue;
    }
    auto out_required_max_size =
        out.required_max_size().has_value() ? *out.required_max_size() : fuchsia_math::SizeU{0, 0};
    auto in_required_max_size =
        in.required_max_size().has_value() ? *in.required_max_size() : fuchsia_math::SizeU{0, 0};
    if (out_required_max_size.width() < in_required_max_size.width()) {
      continue;
    }
    if (out_required_max_size.height() < in_required_max_size.height()) {
      continue;
    }
    auto out_bytes_per_row_divisor =
        out.bytes_per_row_divisor().has_value() ? *out.bytes_per_row_divisor() : 1;
    auto in_bytes_per_row_divisor =
        in.bytes_per_row_divisor().has_value() ? *in.bytes_per_row_divisor() : 1;
    if (out_bytes_per_row_divisor % in_bytes_per_row_divisor != 0) {
      continue;
    }
    // Check if the out colorspaces are a subset of the in color spaces.
    bool all_color_spaces_found = true;
    if (out.color_spaces().has_value()) {
      for (uint32_t j = 0; j < out.color_spaces()->size(); j++) {
        bool found_matching_color_space = false;
        if (in.color_spaces().has_value()) {
          for (uint32_t k = 0; k < in.color_spaces()->size(); k++) {
            if (out.color_spaces()->at(j) == in.color_spaces()->at(k)) {
              found_matching_color_space = true;
              break;
            }
          }
        }
        if (!found_matching_color_space) {
          all_color_spaces_found = false;
          break;
        }
      }
    }
    if (!all_color_spaces_found) {
      continue;
    }
    format_valid_out[i] = true;
  }

  return true;
}

}  // namespace magma_sysmem

#endif  // __Fuchsia_API_level__ >= 19
