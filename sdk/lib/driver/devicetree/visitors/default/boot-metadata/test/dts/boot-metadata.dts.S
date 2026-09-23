// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file
/dts-v1/;

#include "boot-metadata-test.h"

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	sample-device {
		compatible = "fuchsia,sample-device";
		boot-metadata = <TEST_ZBI_TYPE1 TEST_ZBI_EXTRA1 TEST_ZBI_TYPE2 TEST_ZBI_EXTRA2>;
	};
};
