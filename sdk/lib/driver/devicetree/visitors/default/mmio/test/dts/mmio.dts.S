// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file
/dts-v1/;

#include "reg.h"

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	reserved-memory {
		#address-cells = <2>;
		#size-cells = <2>;
		ranges;

		test_region_1: test-region-1@7000000 {
			reg = <0x00 TEST_BASE_1 0x00 TEST_LENGTH_1>;
			no-map;
		};

		test_region_2: test-region-2@8000000 {
			reg = <0x00 TEST_BASE_2 0x00 TEST_LENGTH_2>;
			no-map;
		};
	};

	reg-device@REG_A_UNIT_NAME {
		compatible = "fuchsia,test-device";
		reg = <0 REG_A_BASE 0 REG_A_LENGTH REG_B_BASE_WORD0 REG_B_BASE_WORD1 REG_B_LENGTH_WORD0 REG_B_LENGTH_WORD1 REG_C_BASE_WORD0 REG_C_BASE_WORD1 REG_C_LENGTH_WORD0 REG_C_LENGTH_WORD1>;
		reg-names = "reg-a", "reg-b", "reg-c";
	};

	memory-region-device {
		compatible = "fuchsia,test-device";
		memory-region = <&test_region_1>;
		memory-region-names = "test-region-1";
	};

	combination-device@REG_1_UNIT_NAME {
		compatible = "fuchsia,test-device";
		reg = <0 REG_1_BASE 0 REG_1_LENGTH>;
		reg-names = "reg-1";
		memory-region = <&test_region_2>;
		memory-region-names = "test-region-2";
	};
};
