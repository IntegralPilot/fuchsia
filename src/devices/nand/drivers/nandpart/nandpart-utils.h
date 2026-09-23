// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_NAND_DRIVERS_NANDPART_NANDPART_UTILS_H_
#define SRC_DEVICES_NAND_DRIVERS_NANDPART_NANDPART_UTILS_H_

#include <fidl/fuchsia.boot.metadata/cpp/fidl.h>
#include <fuchsia/hardware/nand/c/banjo.h>
#include <lib/zbi-format/partition.h>
#include <zircon/types.h>

zx_status_t SanitizePartitionMap(fuchsia_boot_metadata::PartitionMap& pmap,
                                 const nand_info_t& nand_info);

#endif  // SRC_DEVICES_NAND_DRIVERS_NANDPART_NANDPART_UTILS_H_
