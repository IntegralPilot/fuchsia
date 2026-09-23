// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file
/dts-v1/;

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;

	spi@ffffa000 {
		compatible = "fuchsia,spi";
		reg = <0x0 0xffffa000 0x0 0x1000>;
		#address-cells = <1>;
		#size-cells = <0>;

		child@0 {
			compatible = "fuchsia,test-spi-1";
			reg = <0>;
			spi-max-frequency = <1000000>;
		};

		child@1 {
			compatible = "fuchsia,test-spi-2";
			reg = <1>;
		};

		/* child@2 uses two chip select lines, and should get two SPI composite parents. */
		/* spi@ffffa000 should end up exposing four chip select lines total. */
		child@2 {
			compatible = "fuchsia,test-spi-3";
			reg = <2>, <3>;
			spi-max-frequency = <5000000>;
		};

		/* This child should be ignored by the visitor. */
		fuchsia,config {
			spi_property = <0>;
		};
	};
};
