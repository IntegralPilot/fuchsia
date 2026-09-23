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

	sample-test-ranges@RANGE_UNIT_NAME {
		compatible = "fuchsia,test-ranges";
		reg = <0x0 RANGE_BASE 0x0 RANGE_SIZE>;
		#address-cells = <2>;
		#size-cells = <2>;
		ranges = <0x0 0x0 0x0 RANGE_BASE 0x0 RANGE_SIZE>;

		sample-test-ranges-child@RANGE_OFFSET_UNIT {
			compatible = "fuchsia,test-ranges-child";
			reg = <0x0 RANGE_OFFSET 0x0 RANGE_OFFSET_SIZE>;
		};
	};
};
