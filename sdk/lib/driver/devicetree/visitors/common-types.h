// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_COMMON_TYPES_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_COMMON_TYPES_H_

#include <lib/devicetree/devicetree.h>

#include <cstdint>

namespace fdf_devicetree {

using Uint32ArrayElement = devicetree::PropEncodedArrayElement<1>;
class Uint32Array : public devicetree::PropEncodedArray<Uint32ArrayElement> {
 public:
  explicit constexpr Uint32Array(devicetree::ByteView data) : PropEncodedArray(data, 1) {}

  uint32_t operator[](size_t index) const {
    return static_cast<uint32_t>(*PropEncodedArray::operator[](index)[0]);
  }
};

using Uint64ArrayElement = devicetree::PropEncodedArrayElement<1>;
class Uint64Array : public devicetree::PropEncodedArray<Uint64ArrayElement> {
 public:
  explicit constexpr Uint64Array(devicetree::ByteView data) : PropEncodedArray(data, 2) {}

  uint64_t operator[](size_t index) const { return *PropEncodedArray::operator[](index)[0]; }
};

}  // namespace fdf_devicetree

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_COMMON_TYPES_H_
