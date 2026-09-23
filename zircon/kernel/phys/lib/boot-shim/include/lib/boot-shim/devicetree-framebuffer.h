// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_DEVICETREE_FRAMEBUFFER_H_
#define ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_DEVICETREE_FRAMEBUFFER_H_

#include <lib/boot-shim/devicetree.h>
#include <lib/zbi-format/graphics.h>
#include <algorithm>

namespace boot_shim {

class DevicetreeFramebufferItem
    : public DevicetreeItemBase<DevicetreeFramebufferItem, 1>,
      public SingleOptionalItem<zbi_swfb_t, ZBI_TYPE_FRAMEBUFFER> {
 public:
  template <class Shim>
  void Init(const Shim& shim) {
    DevicetreeItemBase::Init(shim);
    mmio_observer_ = &shim.mmio_observer();
  }

  devicetree::ScanState OnScan() { return devicetree::ScanState::kDone; }

  devicetree::ScanState OnNode(const devicetree::NodePath& path,
                             const devicetree::PropertyDecoder& decoder) {
    if (!path.IsDescendentOf("/chosen")) {
      return devicetree::ScanState::kActive;
    }
    auto compatible =
        decoder.FindAndDecodeProperty<&devicetree::PropertyValue::AsStringList>("compatible");
    if (!compatible ||
        std::find(compatible->begin(), compatible->end(), "simple-framebuffer") == compatible->end()) {
      return devicetree::ScanState::kActive;
    }
    auto status = decoder.FindAndDecodeProperty<&devicetree::PropertyValue::AsString>("status");
    if (status && *status != "okay" && *status != "ok") {
      return devicetree::ScanState::kActive;
    }
    auto width = decoder.FindAndDecodeProperty<&devicetree::PropertyValue::AsUint32>("width");
    auto height = decoder.FindAndDecodeProperty<&devicetree::PropertyValue::AsUint32>("height");
    auto stride = decoder.FindAndDecodeProperty<&devicetree::PropertyValue::AsUint32>("stride");
    auto format = decoder.FindAndDecodeProperty<&devicetree::PropertyValue::AsString>("format");
    auto property = decoder.FindProperty("reg");
    auto reg = property ? property->AsReg(decoder) : std::nullopt;
    if (!width || !height || !stride || !format || !reg || reg->size() != 1 ||
        !(*reg)[0].address() || !(*reg)[0].size() || !*width || !*height ||
        *width > INT32_MAX || *height > INT32_MAX || *stride % 4 ||
        *stride / 4 < *width) {
      OnError("Ignoring invalid simple framebuffer layout");
      return devicetree::ScanState::kDone;
    }
    auto base = decoder.TranslateAddress(*(*reg)[0].address());
    const uint64_t size = uint64_t{*stride} * *height;
    if (!base || !*base || size > UINT32_MAX || size > *(*reg)[0].size() ||
        *base > UINT64_MAX - size - 4095) {
      OnError("Ignoring invalid simple framebuffer memory range");
      return devicetree::ScanState::kDone;
    }
    zbi_pixel_format_t pixel_format;
    if (*format == "x8r8g8b8" || *format == "a8r8g8b8") {
      pixel_format = ZBI_PIXEL_FORMAT_ARGB_8888;
    } else if (*format == "x8b8g8r8" || *format == "a8b8g8r8") {
      pixel_format = ZBI_PIXEL_FORMAT_ABGR_8888;
    } else if (*format == "x2r10g10b10" || *format == "a2r10g10b10") {
      pixel_format = ZBI_PIXEL_FORMAT_ARGB_2_10_10_10;
    } else if (*format == "x2b10g10r10" || *format == "a2b10g10r10") {
      pixel_format = ZBI_PIXEL_FORMAT_ABGR_2_10_10_10;
    } else {
      OnError("Ignoring unsupported simple framebuffer format");
      return devicetree::ScanState::kDone;
    }
    set_payload({.base = *base, .width = *width, .height = *height,
                 .stride = *stride / 4, .format = pixel_format});
    (*mmio_observer_)({.address = *base, .size = static_cast<size_t>(size)});
    return devicetree::ScanState::kDone;
  }

 private:
  const DevicetreeBootShimMmioObserver* mmio_observer_ = nullptr;
};

}  // namespace boot_shim
#endif  // ZIRCON_KERNEL_PHYS_LIB_BOOT_SHIM_INCLUDE_LIB_BOOT_SHIM_DEVICETREE_FRAMEBUFFER_H_
