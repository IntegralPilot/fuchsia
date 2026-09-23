// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/camera/drivers/controller/sherlock/common_util.h"

#include <fidl/fuchsia.sysmem/cpp/hlcpp_conversion.h>
#include <lib/sysmem-version/sysmem-version.h>

#include <sstream>

namespace camera {

fuchsia::camera2::StreamProperties GetStreamProperties(fuchsia::camera2::CameraStreamType type) {
  fuchsia::camera2::StreamProperties ret{};
  ret.set_stream_type(type);
  return ret;
}

fuchsia::sysmem2::BufferCollectionConstraints InvalidConstraints() {
  StreamConstraints stream_constraints;
  stream_constraints.set_bytes_per_row_divisor(0);
  stream_constraints.set_contiguous(false);
  stream_constraints.AddImageFormat(0, 0, fuchsia::images2::PixelFormat::NV12);
  stream_constraints.set_buffer_count_for_camping(0);
  return stream_constraints.MakeBufferCollectionConstraints();
}

fuchsia::sysmem::BufferCollectionConstraints CopyConstraintsWithNewCampingBufferCount(
    const fuchsia::sysmem::BufferCollectionConstraints& original,
    uint32_t new_camping_buffer_count) {
  auto constraints = original;
  constraints.min_buffer_count_for_camping = new_camping_buffer_count;
  return constraints;
}

fuchsia::sysmem2::BufferCollectionConstraints CopyConstraintsWithOverrides(
    const fuchsia::sysmem::BufferCollectionConstraints& v1_original,
    ConstraintsOverrides overrides) {
  auto v1_original_natural = fidl::HLCPPToNatural(fidl::Clone(v1_original));
  auto v2_original_constraints_result =
      sysmem::V2CopyFromV1BufferCollectionConstraints(&v1_original_natural);
  ZX_ASSERT(v2_original_constraints_result.is_ok());
  auto original = fidl::NaturalToHLCPP(std::move(v2_original_constraints_result.value()));

  auto modified = fidl::Clone(original);
  if (overrides.min_buffer_count_for_camping.has_value()) {
    modified.set_min_buffer_count_for_camping(*overrides.min_buffer_count_for_camping);
  }
  if (overrides.min_buffer_count.has_value()) {
    modified.set_min_buffer_count(*overrides.min_buffer_count);
  }
  return modified;
}

}  // namespace camera
