// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_MMC_SDMMC_SDMMC_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_MMC_SDMMC_SDMMC_VISITOR_H_

#include <lib/driver/devicetree/manager/visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

namespace sdmmc_dt {

class SdmmcVisitor : public fdf_devicetree::Visitor {
 public:
  static constexpr char kMaxFrequency[] = "max-frequency";
  static constexpr char kNoMmcHs400[] = "no-mmc-hs400";
  static constexpr char kNoMmcHs200[] = "no-mmc-hs200";
  static constexpr char kNoMmcHsDdr[] = "no-mmc-hsddr";
  static constexpr char kNonRemovable[] = "non-removable";
  static constexpr char kUseFidl[] = "use-fidl";

  SdmmcVisitor();

  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

 private:
  bool is_match(std::string_view name);

  std::unique_ptr<fdf_devicetree::PropertyParser> sdmmc_parser_;
};

}  // namespace sdmmc_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_MMC_SDMMC_SDMMC_VISITOR_H_
