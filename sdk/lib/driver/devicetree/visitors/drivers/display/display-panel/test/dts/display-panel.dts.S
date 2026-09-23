// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "display-panel-test.h"

/dts-v1/;

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	display {
		compatible = "test,display";
		panel-type = <TEST_PANEL_TYPE>;
	};
};
