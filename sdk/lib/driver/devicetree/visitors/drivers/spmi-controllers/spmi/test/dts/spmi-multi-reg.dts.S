// Copyright 2026 The Fuchsia Authors. All rights reserved.
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
			reg = <0 SPMI_USID>, <1 SPMI_USID>;
			reg-names = "pmic-a", "pmic-b";
		};

		target-b@3 {
			compatible = "vendor3,spmi-vreg";
			reg = <3 SPMI_USID>, <4 SPMI_USID>;
			reg-names = "vreg-1", "vreg-2";
		};
	};
};
