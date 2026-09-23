// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file
/dts-v1/;

#include "smc-test.h"

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	sample-device {
		compatible = "fuchsia,sample-device";
		smcs = <TEST_SMC_BASE TEST_SMC_COUNT TEST_SMC_EXCLUSIVE_FLAG>;
		smc-names = TEST_SMC_NAME;
	};
};
