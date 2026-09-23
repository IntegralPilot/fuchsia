// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "vmo-manager.h"

#include <fbl/auto_lock.h>

namespace radar {

using StatusCode = fuchsia_hardware_radar::StatusCode;

fit::function<bool(const VmoManager::VmoMeta&)> VmoManager::VmoIdMatches(const uint32_t vmo_id) {
  return [vmo_id](const VmoMeta& vmo) { return vmo.vmo_id == vmo_id; };
}

void VmoManager::RemoveVmoId(const uint32_t vmo_id, std::vector<VmoMeta>* const list) {
  const auto it = std::remove_if(list->begin(), list->end(), VmoIdMatches(vmo_id));
  list->erase(it, list->end());
}

fit::result<StatusCode, uint32_t> VmoManager::WriteUnlockedVmoAndGetId(
    const cpp20::span<const uint8_t> data) {
  if (data.size_bytes() > burst_size_) {
    return fit::error(StatusCode::kVmoTooSmall);
  }

  fbl::AutoLock lock(&lock_);

  if (unlocked_vmos_.empty()) {
    return fit::error(StatusCode::kOutOfVmos);
  }

  // Take a VMO from the back of the unlocked list and move it to the back of the locked list.
  locked_vmos_.push_back(unlocked_vmos_.back());
  unlocked_vmos_.pop_back();

  memcpy(locked_vmos_.back().vmo_data, data.data(), data.size_bytes());
  return fit::success(locked_vmos_.back().vmo_id);
}

void VmoManager::UnlockVmo(const uint32_t vmo_id) {
  fbl::AutoLock lock(&lock_);

  // Find the VMO and move it to the back of the unlocked list. If the client is unlocking VMOs in
  // the order that they were received then this VMO should be the last in the locked list.
  const auto it = std::find_if(locked_vmos_.crbegin(), locked_vmos_.crend(), VmoIdMatches(vmo_id));
  if (it != locked_vmos_.crend()) {
    unlocked_vmos_.push_back(*it);
    // Convert the reverse iterator to a forward iterator to erase.
    locked_vmos_.erase((it + 1).base());
  }
}

StatusCode VmoManager::RegisterVmos(fidl::VectorView<const uint32_t> vmo_ids,
                                    fidl::VectorView<zx::vmo> vmos) {
  if (vmo_ids.size() != vmos.size() ||
      vmo_ids.size() > fuchsia_hardware_radar::wire::kVmoVectorMaxCount) {
    return StatusCode::kInvalidArgs;
  }

  fbl::AutoLock lock(&lock_);

  zx_status_t status = ZX_OK;
  size_t last_vmo_index = 0;
  for (; last_vmo_index < vmo_ids.size(); last_vmo_index++) {
    const uint32_t vmo_id = vmo_ids[last_vmo_index];

    status = registered_vmos_.RegisterWithKey(
        vmo_id, vmo_store::StoredVmo<void>(std::move(vmos[last_vmo_index])));
    if (status != ZX_OK) {
      break;
    }

    vmo_store::StoredVmo<void>* vmo = registered_vmos_.GetVmo(vmo_id);
    ZX_ASSERT(vmo != nullptr);
    if (vmo->data().size_bytes() < burst_size_) {
      status = ZX_ERR_BUFFER_TOO_SMALL;
      break;
    }

    unlocked_vmos_.push_back({.vmo_id = vmo_id, .vmo_data = vmo->data().data()});
  }

  // Registration for one of the VMOs failed, so undo any previous registrations that may have
  // succeeded.
  if (status != ZX_OK) {
    for (size_t i = 0; i <= last_vmo_index; i++) {
      const uint32_t vmo_id = vmo_ids[i];
      // Only unregister the final VMO if the status code isn't ZX_ERR_ALREADY_EXISTS. Otherwise
      // this would unregister a VMO that had been registered in a previous call call.
      if (i != last_vmo_index || status != ZX_ERR_ALREADY_EXISTS) {
        RemoveVmoId(vmo_id, &unlocked_vmos_);

        // We can't return handles to the caller, so just close them.
        zx::result<zx::vmo> vmo = registered_vmos_.Unregister(vmo_id);
      }
    }
  }

  switch (status) {
    case ZX_OK:
      // Reserve space for VMOs up front. Existing VMOs may be on the locked list, so the total
      // number of VMOs is the sum of the sizes of the two lists.
      locked_vmos_.reserve(locked_vmos_.size() + unlocked_vmos_.size());
      unlocked_vmos_.reserve(locked_vmos_.size() + unlocked_vmos_.size());
      return StatusCode::kSuccess;
    case ZX_ERR_BAD_HANDLE:
    case ZX_ERR_WRONG_TYPE:
    case ZX_ERR_BAD_STATE:
      return StatusCode::kVmoBadHandle;
    case ZX_ERR_ALREADY_EXISTS:
      return StatusCode::kVmoAlreadyRegistered;
    case ZX_ERR_ACCESS_DENIED:
      return StatusCode::kVmoAccessDenied;
    case ZX_ERR_OUT_OF_RANGE:
    case ZX_ERR_BUFFER_TOO_SMALL:
      return StatusCode::kVmoTooSmall;
    default:
      return StatusCode::kUnspecified;
  }
}

StatusCode VmoManager::RegisterVmos(fidl::VectorView<uint32_t> vmo_ids,
                                    fidl::VectorView<zx::vmo> vmos) {
  return RegisterVmos(
      fidl::VectorView<const uint32_t>::FromExternal(vmo_ids.data(), vmo_ids.size()), vmos);
}

fit::result<StatusCode> VmoManager::RegisterVmos(const std::vector<uint32_t>& vmo_ids,
                                                 std::vector<zx::vmo> vmos) {
  const StatusCode status =
      RegisterVmos(fidl::VectorView<const uint32_t>::FromExternal(vmo_ids.data(), vmo_ids.size()),
                   fidl::VectorView<zx::vmo>::FromExternal(vmos.data(), vmos.size()));
  if (status == StatusCode::kSuccess) {
    return fit::success();
  }
  return fit::error(status);
}

StatusCode VmoManager::UnregisterVmos(fidl::VectorView<const uint32_t> vmo_ids,
                                      fidl::VectorView<zx::vmo> out_vmos) {
  if (vmo_ids.size() != out_vmos.size() ||
      vmo_ids.size() > fuchsia_hardware_radar::wire::kVmoVectorMaxCount) {
    return StatusCode::kInvalidArgs;
  }

  fbl::AutoLock lock(&lock_);

  for (const uint32_t vmo_id : vmo_ids) {
    if (registered_vmos_.GetVmo(vmo_id) == nullptr) {
      return StatusCode::kVmoNotFound;
    }
  }

  for (size_t i = 0; i < vmo_ids.size(); i++) {
    const uint32_t vmo_id = vmo_ids[i];

    RemoveVmoId(vmo_id, &unlocked_vmos_);
    RemoveVmoId(vmo_id, &locked_vmos_);

    auto status = registered_vmos_.Unregister(vmo_id);
    ZX_ASSERT(status.is_ok());
    out_vmos[i] = std::move(status.value());
  }

  return StatusCode::kSuccess;
}

StatusCode VmoManager::UnregisterVmos(fidl::VectorView<uint32_t> vmo_ids,
                                      fidl::VectorView<zx::vmo> out_vmos) {
  return UnregisterVmos(
      fidl::VectorView<const uint32_t>::FromExternal(vmo_ids.data(), vmo_ids.size()), out_vmos);
}

fit::result<StatusCode, std::vector<zx::vmo>> VmoManager::UnregisterVmos(
    const std::vector<uint32_t>& vmo_ids) {
  std::vector<zx::vmo> vmos;
  vmos.resize(vmo_ids.size());

  const StatusCode status =
      UnregisterVmos(fidl::VectorView<const uint32_t>::FromExternal(vmo_ids.data(), vmo_ids.size()),
                     fidl::VectorView<zx::vmo>::FromExternal(vmos.data(), vmos.size()));
  if (status == StatusCode::kSuccess) {
    return fit::success(std::move(vmos));
  }
  return fit::error(status);
}

}  // namespace radar
