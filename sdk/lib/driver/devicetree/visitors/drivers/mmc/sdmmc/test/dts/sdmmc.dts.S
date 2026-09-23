// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file
#include "sdmmc.h"
#include "sdk/lib/driver/devicetree/visitors/drivers/interrupt-controllers/arm-gic/arm-gicv2.h"

/dts-v1/;

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	mmc@ff000000 {
		compatible = "test-sdmmc";
		reg = <0x0 0xff000000 0x0 0x2000>;
		max-frequency = <MAX_FREQUENCY>;
		no-mmc-hs400;
		no-mmc-hsddr;
	};

	sdhci@fe000000 {
		compatible = "test-sdmmc";
		reg = <0x0 0xfe000000 0x0 0x2000>;
		non-removable;
	};
};
