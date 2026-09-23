// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "sdio.h"

/dts-v1/;

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	mmc@ffffb000 {
		compatible = "fuchsia,sdmmc";
		reg = <0x0 0xffffb000 0x0 0x1000>;
		#address-cells = <1>;
		#size-cells = <0>;

		child@0 {
			compatible = "fuchsia,test";
			reg = <SDIO_FUNCTION_0>;
		};

		child@1 {
			compatible = "fuchsia,test";
			reg = <SDIO_FUNCTION_1 SDIO_FUNCTION_2>;
		};
	};
};
