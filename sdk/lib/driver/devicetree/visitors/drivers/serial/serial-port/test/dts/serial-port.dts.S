// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "serial-port-test.h"

/dts-v1/;

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	bt_uart: bt-uart {
		compatible = "test,bt-uart";
		serial-port = <TEST_CLASS TEST_VID TEST_PID>;
		#uart-cells = <0>;
	};

	bt {
		compatible = "bt-controller";
		uarts = <&bt_uart>;
		uart-names = TEST_NAME;
	};
};
