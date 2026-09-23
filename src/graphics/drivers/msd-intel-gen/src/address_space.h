// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ADDRESS_SPACE_H
#define ADDRESS_SPACE_H

#include <lib/magma/util/status.h>
#include <lib/magma_service/util/address_space.h>

#include <map>
#include <mutex>
#include <unordered_map>

#include "gpu_mapping.h"

class AddressSpace : public magma::AddressSpace<GpuMapping> {
 public:
  AddressSpace(magma::AddressSpaceOwner* owner, AddressSpaceType type = ADDRESS_SPACE_PPGTT)
      : magma::AddressSpace<GpuMapping>(owner), type_(type) {}

  AddressSpaceType type() { return type_; }

  bool InsertWithBusMapping() override { return type_ == ADDRESS_SPACE_PPGTT; }

 private:
  AddressSpaceType type_;
};

#endif  // ADDRESS_SPACE_H
