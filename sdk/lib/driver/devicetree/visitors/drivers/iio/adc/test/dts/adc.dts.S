// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file
#include "adc.h"

/dts-v1/;

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	adcA: vadc@ffffa000 {
		compatible = "fuchsia,adc";
		reg = <0x0 0xffffa000 0x0 0x1000>;
		#io-channel-cells = <1>;
	};

	adcB: adc@ffffb000 {
		compatible = "fuchsia,gpio";
		reg = <0x0 0xffffb000 0x0 0x1000>;
		#io-channel-cells = <1>;
	};

	audio@ffffc000 {
		compatible = "test,audio";
		reg = <0x0 0xffffc000 0x0 0x1000>;
		io-channels = <&adcA ADC_CHAN1 &adcB ADC_CHAN2>;
		io-channel-names = ADC_CHAN1_NAME, ADC_CHAN2_NAME;
	};

	video@ffffd000 {
		compatible = "test,video";
		reg = <0x0 0xffffd000 0x0 0x1000>;
		io-channels = <&adcB ADC_CHAN3>;
		io-channel-names = ADC_CHAN3_NAME;
	};
};
