// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file
/dts-v1/;

#include "config.h"
#include <lib/ddk/platform-defs.h>

/ {
	compatible = "fuchsia,sample-dt";
	model = "example";
	#address-cells = <2>;
	#size-cells = <2>;

	sample-device@0 {
		compatible = "fuchsia,sample-device";
		reg = <0 TEST_REG_A_BASE 0 TEST_REG_A_LENGTH TEST_REG_B_BASE_WORD0 TEST_REG_B_BASE_WORD1 TEST_REG_B_LENGTH_WORD0 TEST_REG_B_LENGTH_WORD1 TEST_REG_C_BASE_WORD0 TEST_REG_C_BASE_WORD1 TEST_REG_C_LENGTH_WORD0 TEST_REG_C_LENGTH_WORD1>;
		device_specific_prop = <DEVICE_SPECIFIC_PROP_VALUE>;
	};

	stub_iommu: iommu {
		#iommu-cells = <1>;

		// phandle is optional, but set here for testing purpose.
		phandle = <TEST_IOMMU_PHANDLE>;
	};

	sample-bti-device {
		compatible = "fuchsia,sample-bti-device";
		iommus = <&stub_iommu TEST_BTI_ID>;
	};
};
