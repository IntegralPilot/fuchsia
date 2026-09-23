// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file
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

		target-a@0 {
			compatible = "vendor2,spmi-pmic";
			reg = <0 SPMI_USID>;
			reg-names = "target-a";
			#address-cells = <1>;
			#size-cells = <1>;

			vreg@1000 {
				compatible = "vendor2,pmic-vreg";
				reg = <0x1000 0x1000>;
			};

			gpio@2000 {
				compatible = "vendor2,pmic-gpio";
				reg = <0x2000 0x800>;
			};

			i2c@3000 {
				compatible = "vendor2,pmic-i2c";
				reg = <0x3000 0x400>, <0x10000 1>;
				reg-names = "i2c-core", "i2c-config";
			};
		};

		target-b@3 {
			compatible = "vendor3,spmi-vreg";
			reg = <3 SPMI_USID>;
			reg-names = "vreg";
		};
	};
};
