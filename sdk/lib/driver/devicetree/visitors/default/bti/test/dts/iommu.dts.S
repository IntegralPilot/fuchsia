// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file
/dts-v1/;

#include "iommu.h"

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	sample-bti-device1 {
		compatible = "fuchsia,sample-bti-device";
		iommus = <&stub_iommu TEST_BTI_ID1>;
	};

	stub_iommu: iommu {
		#iommu-cells = <1>;
		phandle = <TEST_IOMMU_PHANDLE>;
	};

	sample-bti-device2 {
		compatible = "fuchsia,sample-bti-device";
		iommus = <&stub_iommu TEST_BTI_ID2>;
		iommu-names = TEST_BTI_ID2_NAME;
	};
};
