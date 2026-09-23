// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "bad-block.h"

#include "aml-bad-block.h"

namespace nand {

zx::result<std::shared_ptr<BadBlock>> BadBlock::Create(Config config) {
  switch (config.bad_block_config.type()) {
    case fuchsia_hardware_nand::BadBlockConfigType::kAmlogicUboot:
      return AmlBadBlock::Create(config);
    default:
      return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
}

}  // namespace nand
