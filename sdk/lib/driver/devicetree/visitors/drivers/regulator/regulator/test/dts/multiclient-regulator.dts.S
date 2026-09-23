// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "regulator-test.h"

/dts-v1/;

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	vreg: voltage-regulator {
		compatible = "voltage-regulator";
		regulator-name = REGULATOR_NAME;
		regulator-min-microvolt = <MIN_VOLTAGE>;
		regulator-max-microvolt = <MAX_VOLTAGE>;
		regulator-step-microvolt = <STEP_VOLTAGE>;
		#regulator-cells = <0>;
	};

	cpu-ctrl {
		compatible = "cpu-ctrl";
		regulators = <&vreg>;
	};

	gpu-ctrl {
		compatible = "gpu";
		regulators = <&vreg>;
	};
};
