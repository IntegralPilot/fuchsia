// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_DEVICETREE_TESTING_INCLUDE_LIB_DEVICETREE_TESTING_LOADED_DTB_H_
#define ZIRCON_KERNEL_LIB_DEVICETREE_TESTING_INCLUDE_LIB_DEVICETREE_TESTING_LOADED_DTB_H_

#include <lib/devicetree/devicetree.h>
#include <lib/fit/result.h>

#include <cstddef>
#include <cstdint>
#include <string>
#include <string_view>
#include <vector>

namespace devicetree::testing {

// Represents a 'dtb' file in memory.
struct LoadedDtb {
  // Returns an instance of a devicetree::Devicetree based on the loaded data.
  devicetree::Devicetree fdt() const {
    return devicetree::Devicetree(ByteView(dtb.data(), dtb.size()));
  }

  std::vector<uint8_t> dtb;
};

// Loads a 'dtb' file named |dtb_file| from the device tree repository.
fit::result<std::string, LoadedDtb> LoadDtb(std::string_view dtb_file);

}  // namespace devicetree::testing

#endif  // ZIRCON_KERNEL_LIB_DEVICETREE_TESTING_INCLUDE_LIB_DEVICETREE_TESTING_LOADED_DTB_H_
