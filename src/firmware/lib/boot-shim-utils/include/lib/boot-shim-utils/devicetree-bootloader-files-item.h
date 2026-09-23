// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef SRC_FIRMWARE_LIB_BOOT_SHIM_UTILS_INCLUDE_LIB_BOOT_SHIM_UTILS_DEVICETREE_BOOTLOADER_FILES_ITEM_H_
#define SRC_FIRMWARE_LIB_BOOT_SHIM_UTILS_INCLUDE_LIB_BOOT_SHIM_UTILS_DEVICETREE_BOOTLOADER_FILES_ITEM_H_

#include <lib/boot-shim/devicetree.h>

// `DevicetreeBootloaderFilesItem` looks for the device tree node "/chosen/google/bootloader-files/"
// and adds all subnodes under it as bootloader file ZBI items. i.e.:
//
//   / {
//       chosen {
//           google {
//               bootloader-files {
//                   gbl-file-0 {
//                     id = foo
//                     data = <bytes content of `file1`>
//                   }
//                   gbl-file-1 {
//                     id = bar
//                     data = <bytes content of `file2`>
//                   }
//               };
//           };
//       };
//   };
class DevicetreeBootloaderFilesItem
    : public boot_shim::DevicetreeItemBase<DevicetreeBootloaderFilesItem, 1>,
      public boot_shim::ItemBase {
 public:
  // Sets a scratch buffer for storing the bootloader files extracted from the device tree. The
  // buffer must be aligned to `ZBI_ALIGNMENT`.
  void SetScratchBuffer(std::span<std::byte> buffer);

  // Following are all required methods from base classes.

  devicetree::ScanState OnNode(const devicetree::NodePath& path,
                               const devicetree::PropertyDecoder& decoder);
  devicetree::ScanState OnScan() { return devicetree::ScanState::kDone; }
  size_t size_bytes() const { return zbitl::View(files_.storage()).container_header()->length; }
  fit::result<DataZbi::Error> AppendItems(DataZbi& zbi) const;

 private:
  zbitl::Image<std::span<std::byte>> files_;
};

#endif  // SRC_FIRMWARE_LIB_BOOT_SHIM_UTILS_INCLUDE_LIB_BOOT_SHIM_UTILS_DEVICETREE_BOOTLOADER_FILES_ITEM_H_
