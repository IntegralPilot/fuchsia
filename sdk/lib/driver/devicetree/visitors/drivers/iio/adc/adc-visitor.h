// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_IIO_ADC_ADC_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_IIO_ADC_ADC_VISITOR_H_

#include <fidl/fuchsia.hardware.adcimpl/cpp/fidl.h>
#include <lib/driver/devicetree/visitors/driver-visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

#include <cstdint>
#include <map>

#include "sdk/lib/driver/devicetree/visitors/drivers/iio/lib/iio-visitor.h"

namespace adc_dt {

class AdcVisitor : public iio_dt::IioVisitor {
 private:
  struct AdcController {
    void AddChannel(uint32_t chan_idx, const std::string& name, const std::string& parent_name);

    std::vector<fuchsia_hardware_adcimpl::AdcChannel> channels;
  };

  zx::result<> FinalizeNode(fdf_devicetree::Node& node) override;
  zx::result<> ParseReferenceChild(fdf_devicetree::Node& child,
                                   fdf_devicetree::ReferenceNode& parent,
                                   fdf_devicetree::PropertyCells specifiers,
                                   std::optional<std::string_view> name) override;
  bool is_match(fdf_devicetree::Node& node) override;

  // Return an existing or a new instance of AdcController.
  AdcController& GetController(uint32_t node_id);
  zx::result<> AddChildNodeSpec(fdf_devicetree::Node& child, uint32_t chan_id);

  // Mapping of controller node ID to its info.
  std::map<uint32_t, AdcController> controllers_;
};

}  // namespace adc_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_IIO_ADC_ADC_VISITOR_H_
