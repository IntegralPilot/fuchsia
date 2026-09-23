// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/ddi-physical-layer-manager.h"

#include <lib/driver/logging/cpp/logger.h>
#include <zircon/assert.h>

#include "src/graphics/display/drivers/intel-display/ddi-physical-layer.h"
#include "src/graphics/display/drivers/intel-display/hardware-common.h"
#include "src/graphics/display/drivers/intel-display/intel-display.h"

namespace intel_display {

DdiReference::DdiReference() = default;

DdiReference::DdiReference(DdiPhysicalLayer* phy) : phy_(phy) {
  ZX_DEBUG_ASSERT(phy_);
  phy_->AddRef();
}

DdiReference::~DdiReference() {
  if (phy_) {
    phy_->Release();
  }
}

DdiReference::DdiReference(DdiReference&& rhs) noexcept {
  phy_ = rhs.phy_;
  rhs.phy_ = nullptr;
}

DdiReference& DdiReference::operator=(DdiReference&& rhs) noexcept {
  if (phy_) {
    phy_->Release();
  }

  phy_ = rhs.phy_;
  rhs.phy_ = nullptr;
  return *this;
}

DdiPhysicalLayer::PhysicalLayerInfo DdiReference::GetPhysicalLayerInfo() const {
  ZX_DEBUG_ASSERT(phy_);
  return phy_->GetPhysicalLayerInfo();
}

DdiReference DdiManager::GetDdiReference(DdiId ddi_id) {
  auto ddi_state_iter = ddi_map_.find(ddi_id);
  ZX_DEBUG_ASSERT_MSG(ddi_state_iter != ddi_map_.end(),
                      "DdiManager: DDI %d is not available. Cannot request reference for DDI.",
                      ddi_id);
  DdiPhysicalLayer* phy = ddi_state_iter->second.get();
  // If there's no existing reference to the PHY, we'll need to enable the PHY
  // first.
  ZX_DEBUG_ASSERT(phy);
  if (phy->Enable()) {
    return DdiReference(phy);
  }
  return DdiReference();
}

DdiManagerSkylake::DdiManagerSkylake() {
  for (const DdiId ddi_id : DdiIds<registers::Platform::kSkylake>()) {
    ddi_map()[ddi_id] = std::make_unique<DdiSkylake>(ddi_id);
  }
}

DdiManagerTigerLake::DdiManagerTigerLake(Controller* controller)
    : DdiManagerTigerLake(controller->power(), controller->mmio_space(),
                          controller->igd_opregion()) {}

DdiManagerTigerLake::DdiManagerTigerLake(Power* power, fdf::MmioBuffer* mmio_space,
                                         const IgdOpRegion& igd_opregion) {
  for (const DdiId ddi_id : DdiIds<registers::Platform::kTigerLake>()) {
    if (!igd_opregion.HasDdi(ddi_id)) {
      fdf::trace("DDI {} not initialized because it's omitted in VBT.", ddi_id);
      continue;
    }

    if (ddi_id >= DdiId::DDI_A && ddi_id <= DdiId::DDI_C) {
      auto ddi = std::make_unique<ComboDdiTigerLake>(ddi_id, mmio_space);
      // TODO(https://fxbug.dev/42066037): Create an initialization API in the base class.
      if (!ddi->Initialize()) {
        fdf::error("Failed to initialize DDI {}. It will remain unused.", ddi_id);
        continue;
      }
      auto [it, emplace_success] = ddi_map().try_emplace(ddi_id, std::move(ddi));
      ZX_DEBUG_ASSERT_MSG(emplace_success, "DDI %d already in map", ddi_id);
      continue;
    }

    if (ddi_id >= DdiId::DDI_TC_1 && ddi_id <= DdiId::DDI_TC_6) {
      // Type-C DDI
      const bool is_static_port = !igd_opregion.IsTypeC(ddi_id);
      ddi_map()[ddi_id] =
          std::make_unique<TypeCDdiTigerLake>(ddi_id, power, mmio_space, is_static_port);
      continue;
    }

    ZX_DEBUG_ASSERT_MSG(false, "Unhandled DDI list entry - DDI %d", ddi_id);
  }
}

}  // namespace intel_display
