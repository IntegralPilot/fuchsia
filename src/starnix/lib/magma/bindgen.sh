#!/bin/sh
# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -e

if [[ ! -f sdk/lib/magma_client/include/lib/magma/magma.h ]]; then
  echo 'Please run this script from the root of your Fuchsia source tree.'
  exit 1
fi

readonly RAW_LINES="// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use zerocopy::{IntoBytes, FromBytes, Immutable};"

# Type/define pairs, used to generate a list of variables to work around
# https://github.com/rust-lang/rust-bindgen/issues/316
# The list below is generated using regex:
# sed -rn 's/^#define\s*(\S*)\s*\(\(([^\)]*).*$/\2,\1/p' magma_common_defs.h
readonly define_list=(
magma_query_t,MAGMA_QUERY_VENDOR_ID
magma_query_t,MAGMA_QUERY_DEVICE_ID
magma_query_t,MAGMA_QUERY_VENDOR_VERSION
magma_query_t,MAGMA_QUERY_IS_TOTAL_TIME_SUPPORTED
magma_query_t,MAGMA_QUERY_MAXIMUM_INFLIGHT_PARAMS
magma_query_t,MAGMA_QUERY_VENDOR_PARAM_0
magma_query_t,MAGMA_QUERY_TOTAL_TIME
uint64_t,MAGMA_INVALID_OBJECT_ID
uint64_t,MAGMA_COMMAND_BUFFER_VENDOR_FLAGS_0
magma_status_t,MAGMA_STATUS_OK
magma_status_t,MAGMA_STATUS_INTERNAL_ERROR
magma_status_t,MAGMA_STATUS_INVALID_ARGS
magma_status_t,MAGMA_STATUS_ACCESS_DENIED
magma_status_t,MAGMA_STATUS_MEMORY_ERROR
magma_status_t,MAGMA_STATUS_CONTEXT_KILLED
magma_status_t,MAGMA_STATUS_CONNECTION_LOST
magma_status_t,MAGMA_STATUS_TIMED_OUT
magma_status_t,MAGMA_STATUS_UNIMPLEMENTED
magma_status_t,MAGMA_STATUS_BAD_STATE
magma_status_t,MAGMA_STATUS_CONSTRAINTS_INTERSECTION_EMPTY
magma_status_t,MAGMA_STATUS_TOO_MANY_GROUP_CHILD_COMBINATIONS
magma_cache_operation_t,MAGMA_CACHE_OPERATION_CLEAN
magma_cache_operation_t,MAGMA_CACHE_OPERATION_CLEAN_INVALIDATE
magma_cache_policy_t,MAGMA_CACHE_POLICY_CACHED
magma_cache_policy_t,MAGMA_CACHE_POLICY_WRITE_COMBINING
magma_cache_policy_t,MAGMA_CACHE_POLICY_UNCACHED
uint32_t,MAGMA_DUMP_TYPE_NORMAL
uint32_t,MAGMA_PERF_COUNTER_RESULT_DISCONTINUITY
uint64_t,MAGMA_IMPORT_SEMAPHORE_ONE_SHOT
magma_format_t,MAGMA_FORMAT_INVALID
magma_format_t,MAGMA_FORMAT_R8G8B8A8
magma_format_t,MAGMA_FORMAT_BGRA32
magma_format_t,MAGMA_FORMAT_I420
magma_format_t,MAGMA_FORMAT_M420
magma_format_t,MAGMA_FORMAT_NV12
magma_format_t,MAGMA_FORMAT_YUY2
magma_format_t,MAGMA_FORMAT_MJPEG
magma_format_t,MAGMA_FORMAT_YV12
magma_format_t,MAGMA_FORMAT_BGR24
magma_format_t,MAGMA_FORMAT_RGB565
magma_format_t,MAGMA_FORMAT_RGB332
magma_format_t,MAGMA_FORMAT_RGB2220
magma_format_t,MAGMA_FORMAT_L8
magma_format_t,MAGMA_FORMAT_R8
magma_format_t,MAGMA_FORMAT_R8G8
magma_format_t,MAGMA_FORMAT_A2R10G10B10
magma_format_t,MAGMA_FORMAT_A2B10G10R10
magma_format_t,MAGMA_FORMAT_P010
magma_format_t,MAGMA_FORMAT_R8G8B8
magma_format_modifier_t,MAGMA_FORMAT_MODIFIER_LINEAR
magma_format_modifier_t,MAGMA_FORMAT_MODIFIER_INTEL_X_TILED
magma_format_modifier_t,MAGMA_FORMAT_MODIFIER_INTEL_Y_TILED
magma_format_modifier_t,MAGMA_FORMAT_MODIFIER_INTEL_YF_TILED
magma_format_modifier_t,MAGMA_FORMAT_MODIFIER_INTEL_Y_TILED_CCS
magma_format_modifier_t,MAGMA_FORMAT_MODIFIER_INTEL_YF_TILED_CCS
magma_format_modifier_t,MAGMA_FORMAT_MODIFIER_ARM_YUV_BIT
magma_format_modifier_t,MAGMA_FORMAT_MODIFIER_ARM_SPLIT_BLOCK_BIT
magma_format_modifier_t,MAGMA_FORMAT_MODIFIER_ARM_SPARSE_BIT
magma_format_modifier_t,MAGMA_FORMAT_MODIFIER_ARM_BCH_BIT
magma_format_modifier_t,MAGMA_FORMAT_MODIFIER_ARM_TE_BIT
magma_format_modifier_t,MAGMA_FORMAT_MODIFIER_ARM_TILED_HEADER_BIT
magma_format_modifier_t,MAGMA_FORMAT_MODIFIER_ARM
magma_format_modifier_t,MAGMA_FORMAT_MODIFIER_ARM_AFBC_16X16
magma_format_modifier_t,MAGMA_FORMAT_MODIFIER_ARM_AFBC_32X8
magma_format_modifier_t,MAGMA_FORMAT_MODIFIER_ARM_LINEAR_TE
magma_format_modifier_t,MAGMA_FORMAT_MODIFIER_ARM_AFBC_16X16_TE
magma_format_modifier_t,MAGMA_FORMAT_MODIFIER_ARM_AFBC_32X8_TE
magma_colorspace_t,MAGMA_COLORSPACE_INVALID
magma_colorspace_t,MAGMA_COLORSPACE_SRGB
magma_colorspace_t,MAGMA_COLORSPACE_REC601_NTSC
magma_colorspace_t,MAGMA_COLORSPACE_REC601_NTSC_FULL_RANGE
magma_colorspace_t,MAGMA_COLORSPACE_REC601_PAL
magma_colorspace_t,MAGMA_COLORSPACE_REC601_PAL_FULL_RANGE
magma_colorspace_t,MAGMA_COLORSPACE_REC709
magma_colorspace_t,MAGMA_COLORSPACE_REC2020
magma_colorspace_t,MAGMA_COLORSPACE_REC2100
magma_coherency_domain_t,MAGMA_COHERENCY_DOMAIN_CPU
magma_coherency_domain_t,MAGMA_COHERENCY_DOMAIN_RAM
magma_coherency_domain_t,MAGMA_COHERENCY_DOMAIN_INACCESSIBLE
uint32_t,MAGMA_POLL_TYPE_SEMAPHORE
uint32_t,MAGMA_POLL_TYPE_HANDLE
uint32_t,MAGMA_POLL_CONDITION_READABLE
uint32_t,MAGMA_POLL_CONDITION_SIGNALED
magma_buffer_range_op_t,MAGMA_BUFFER_RANGE_OP_POPULATE_TABLES
magma_buffer_range_op_t,MAGMA_BUFFER_RANGE_OP_COMMIT
magma_buffer_range_op_t,MAGMA_BUFFER_RANGE_OP_DEPOPULATE_TABLES
magma_buffer_range_op_t,MAGMA_BUFFER_RANGE_OP_DECOMMIT
uint32_t,MAGMA_SYSMEM_FLAG_PROTECTED
uint32_t,MAGMA_SYSMEM_FLAG_FOR_CLIENT
uint32_t,MAGMA_MAX_IMAGE_PLANES
uint32_t,MAGMA_MAX_DRM_FORMAT_MODIFIERS
uint64_t,MAGMA_MAP_FLAG_VENDOR_SHIFT
uint64_t,MAGMA_MAP_FLAG_READ
uint64_t,MAGMA_MAP_FLAG_WRITE
uint64_t,MAGMA_MAP_FLAG_EXECUTE
uint64_t,MAGMA_MAP_FLAG_GROWABLE
uint64_t,MAGMA_MAP_FLAG_VENDOR_0
uint32_t,MAGMA_IMAGE_CREATE_FLAGS_PRESENTABLE
uint32_t,MAGMA_IMAGE_CREATE_FLAGS_VULKAN_USAGE
uint64_t,MAGMA_PRIORITY_LOW
uint64_t,MAGMA_PRIORITY_MEDIUM
uint64_t,MAGMA_PRIORITY_HIGH
uint64_t,MAGMA_PRIORITY_REALTIME
)

define_text=""
for define in ${define_list[@]}; do
  TYPE=${define%,*};
  NAME=${define#*,};

  # Create a variable with the same name as the define, so bindgen can use its value.
  define_text+="
const $TYPE _$NAME = $NAME;
#undef $NAME
const $TYPE $NAME = _$NAME;"
done

temp_include_dir=$(mktemp -d)

function cleanup {
  rm -rf "${temp_include_dir}"
}

trap cleanup EXIT

echo "${define_text}" > "${temp_include_dir}/missing_includes.h"

PATH="$PWD/scripts:$PATH" \
src/graphics/lib/magma/include/virtio/virtio_magma_h_gen.py \
  fuchsia \
  src/graphics/lib/magma/include/magma/magma.json \
  "${temp_include_dir}/virtio_magma.h"

PATH="$PWD/prebuilt/third_party/rust/linux-x64/bin:$PATH" \
./prebuilt/third_party/rust_bindgen/linux-x64/bindgen \
  --no-layout-tests \
  --with-derive-default \
  --explicit-padding \
  --raw-line "${RAW_LINES}" \
  --allowlist-function 'magma_.*' \
  --allowlist-type 'magma_.*' \
  --allowlist-type 'virtmagma_.*' \
  --allowlist-type 'virtio_magma_.*' \
  --allowlist-var 'MAGMA_.*' \
  -o src/starnix/lib/magma/src/magma.rs \
  src/starnix/lib/magma/wrapper.h \
  -- \
  -I zircon/system/public \
  -I src/graphics/lib/magma/src \
  -I src/graphics/lib/magma/include \
  -I sdk/lib/magma_client/include \
  -I sdk/lib/magma_common/include \
  -I $temp_include_dir \
  -I $(pwd)

# TODO: Figure out how to get bindgen to derive IntoBytes, FromBytes, Immutable.
#       See https://github.com/rust-lang/rust-bindgen/issues/1089
sed -i \
  's/derive(Debug, Default, Copy, Clone)/derive(Debug, Default, Copy, Clone, IntoBytes, FromBytes, Immutable)/' \
  src/starnix/lib/magma/src/magma.rs
