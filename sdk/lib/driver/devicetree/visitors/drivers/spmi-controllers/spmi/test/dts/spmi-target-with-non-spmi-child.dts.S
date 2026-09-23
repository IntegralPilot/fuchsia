// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "sdk/lib/driver/devicetree/visitors/drivers/spmi-controllers/spmi/spmi.h"

/dts-v1/;

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <1>;
	#size-cells = <1>;

	spmi@abcd0000 {
		compatible = "vendor1,spmi-controller";
		reg = <0xabcd0000 0x1000>;
		#address-cells = <2>;
		#size-cells = <0>;

		i2c-controller@2 {
			compatible = "vendor6,spmi-i2c-controller";
			reg = <2 SPMI_USID>;
			#address-cells = <1>;
			#size-cells = <0>;

			eeprom@50 {
				compatible = "vendor7,eeprom";
				reg = <0x50>;
			};
		};
	};
};
