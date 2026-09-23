// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file
#include "interconnect.h"

/dts-v1/;

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	interconnectA: interconnect@ffffa000 {
		compatible = "fuchsia,interconnect";
		reg = <0x0 0xffffa000 0x0 0x1000>;
		#interconnect-cells = <1>;
	};

	// Too many members in interconnects
	video@ffffc000 {
		compatible = "test-video";
		reg = <0x0 0xffffc000 0x0 0x1000>;
		interconnects = <&interconnectA ICC_ID1 &interconnectA ICC_ID2 &interconnectA ICC_ID3>;
		interconnect-names = PATH1_NAME;
	};

	// Too few members in interconnects
	audio@ffffd000 {
		compatible = "test-audio";
		reg = <0x0 0xffffd000 0x0 0x1000>;
		interconnects = <&interconnectA ICC_ID6>;
		interconnect-names = PATH3_NAME;
	};
};
