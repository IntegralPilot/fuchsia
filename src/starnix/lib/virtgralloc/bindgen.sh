#!/bin/sh
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -e

if [[ ! -f sdk/lib/virtgralloc/include/lib/virtgralloc/virtgralloc_ioctl.h ]]; then
  echo 'Please run this script from the root of your Fuchsia source tree.'
  exit 1
fi

readonly RAW_LINES="// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use zerocopy::{AsBytes, FromBytes, FromZeros};"

# Type/define pairs, used to generate a list of variables to work around
# https://github.com/rust-lang/rust-bindgen/issues/316
readonly define_list=(
rust_bindgen_string,VIRTGRALLOC_DEVICE_NAME
virtgralloc_VulkanMode,VIRTGRALLOC_VULKAN_MODE_INVALID
virtgralloc_VulkanMode,VIRTGRALLOC_VULKAN_MODE_SWIFTSHADER
virtgralloc_VulkanMode,VIRTGRALLOC_VULKAN_MODE_MAGMA
virtgralloc_SetVulkanModeResult,VIRTGRALLOC_SET_VULKAN_MODE_RESULT_INVALID
virtgralloc_SetVulkanModeResult,VIRTGRALLOC_SET_VULKAN_MODE_RESULT_SUCCESS
uint32_t,VIRTGRALLOC_IOCTL_SET_VULKAN_MODE
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
  rm -rf "$temp_include_dir"
}

trap cleanup EXIT

echo "$define_text" > $temp_include_dir/missing_includes.h
PATH="$PWD/prebuilt/third_party/rust/linux-x64/bin:$PATH" \
./prebuilt/third_party/rust_bindgen/linux-x64/bindgen \
  --no-layout-tests \
  --with-derive-default \
  --explicit-padding \
  --raw-line "${RAW_LINES}" \
  --allowlist-function 'virtgralloc_.*' \
  --allowlist-type 'virtgralloc_.*' \
  --allowlist-var 'VIRTGRALLOC_.*' \
  -o src/starnix/lib/virtgralloc/src/virtgralloc.rs \
  src/starnix/lib/virtgralloc/wrapper.h \
  -- \
  -I sdk/lib/virtgralloc/include \
  -I $temp_include_dir \
  -I $(pwd)

# TODO: Figure out how to get bindgen to derive AsBytes, FromBytes, and FromZeros.
#       See https://github.com/rust-lang/rust-bindgen/issues/1089
sed -i \
  's/derive(Debug, Default, Copy, Clone)/derive(Debug, Default, Copy, Clone, AsBytes, FromBytes, FromZeros, zerocopy::NoCell)/' \
  src/starnix/lib/virtgralloc/src/virtgralloc.rs

# TODO: Revisit once we're using a rust-bindgen that generates CStr instead of
#       byte arrays.
# Remove "pub type rust_bindgen_string = *const ::std::os::raw::c_char;" line.
sed -i \
  's/.*rust_bindgen_string.*//' \
  src/starnix/lib/virtgralloc/src/virtgralloc.rs
