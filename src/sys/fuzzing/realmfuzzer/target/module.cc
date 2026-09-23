// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/realmfuzzer/target/module.h"

#include <lib/syslog/cpp/macros.h>
#include <simdutf.h>
#include <zircon/status.h>

#include "src/sys/fuzzing/common/module.h"
#include "src/sys/fuzzing/common/sancov.h"

namespace fuzzing {

NO_SANITIZE_ALL
zx_status_t Module::Import(uint8_t* counters, const uintptr_t* pcs, size_t num_pcs) {
  FX_CHECK(counters && pcs && num_pcs);
  if (auto status = counters_.Mirror(counters, num_pcs); status != ZX_OK) {
    FX_LOGS(WARNING) << "Failed to mirror module counters: " << zx_status_get_string(status);
    return status;
  }
  // Make a position independent table from the PCs.
  auto pc_table = std::make_unique<ModulePC[]>(num_pcs);
  for (size_t i = 0; i < num_pcs; ++i) {
    pc_table[i].pc = pcs[i * 2] - pcs[0];
    pc_table[i].flags = pcs[i * 2 + 1];
  }
  // Double hash using both FNV1a and DJB2a to reduce the likelihood of collisions. We could use a
  // cryptographic hash here, but that introduces unwanted dependencies, and this is good enough.
  // The algorithms are taken from http://www.isthe.com/chongo/tech/comp/fnv/index.html and
  // http://www.cse.yorku.ca/~oz/hash.html.
  uint64_t fnv1a = 14695981039346656037ULL;
  uint64_t djb2a = 5381;
  auto* u8 = reinterpret_cast<uint8_t*>(pc_table.get());
  size_t size = num_pcs * sizeof(ModulePC);
  while (size-- > 0) {
    fnv1a = (fnv1a ^ *u8) * 1099511628211ULL;
    djb2a = ((djb2a << 5) + djb2a) ^ *u8;
    u8++;
  }

  // Encode using base-64.
  uint64_t hex[2] = {fnv1a, djb2a};
  char id[ZX_MAX_NAME_LEN];
  FX_DCHECK(simdutf::base64_length_from_binary(sizeof(hex)) < sizeof(id));
  auto len = simdutf::binary_to_base64(reinterpret_cast<const char*>(hex), sizeof(hex), id);
  id_ = std::string(id, len);
  return ZX_OK;
}

NO_SANITIZE_ALL
zx_status_t Module::Share(zx::vmo* out) const {
  if (auto status = counters_.Share(out); status != ZX_OK) {
    FX_LOGS(WARNING) << "Failed to share module: " << zx_status_get_string(status);
    return status;
  }
  if (auto status = out->set_property(ZX_PROP_NAME, id_.data(), id_.size()); status != ZX_OK) {
    FX_LOGS(WARNING) << "Failed to set module ID: " << zx_status_get_string(status);
    return status;
  }
  return ZX_OK;
}

}  // namespace fuzzing
