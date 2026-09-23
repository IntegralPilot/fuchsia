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

			gpio: gpio@2000 {
				reg = <0x2000 0x800>;
			};

			i2c: i2c@3000 {
				reg = <0x3000 0x400>, <0xffff 1>;
				reg-names = "i2c-core", "i2c-config";
			};
		};

		target-b@3 {
			compatible = "vendor3,spmi-vreg";
			reg = <3 SPMI_USID>;
			reg-names = "vreg";
		};
	};

	not-spmi@abce0000 {
		compatible = "vendor1,not-spmi-controller";
		reg = <0xabce0000 0x1000>;

		/* The duplicate gpio entry should be ignored. */
		spmis = <&gpio>, <&i2c>, <&gpio>;
	};

	spmi@abcf0000 {
		compatible = "vendor4,spmi-controller";
		reg = <0xabcf0000 0x1000>;
		#address-cells = <2>;
		#size-cells = <0>;

		target-c@0 {
			compatible = "vendor5,spmi-pmic";
			reg = <0 SPMI_USID>;
		};
	};
};
