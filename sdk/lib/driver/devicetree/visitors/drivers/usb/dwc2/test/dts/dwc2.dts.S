// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "dwc2-test.h"
#include "sdk/lib/driver/devicetree/visitors/drivers/interrupt-controllers/arm-gic/arm-gicv2.h"

/dts-v1/;

/ {
	compatible = "fuchsia,test-dt";
	model = "test";
	#address-cells = <2>;
	#size-cells = <2>;
	interrupt-parent = <&gic>;

	gic: interrupt-controller@ffffd0000 {
		#interrupt-cells = <3>;
		interrupt-controller;
		reg = <0x0 0xFFFFD000 0x0 0x1000>;
	};

	stub_iommu: stub_iommu {
		#iommu-cells = <0>;
	};

	usb_phy: usb_test_phy {
		#phy-cells = <0>;
	};

	dwc2: usb@ff400000 {
		compatible = "snps,dwc2";
		reg = <0x0 0xff400000 0x0 0x2000>;
		phys = <&usb_phy>;
		phy-names = "dwc2-phy";
		iommus = <&stub_iommu>;
		interrupts = <GIC_SPI 31 GIC_IRQ_MODE_EDGE_RISING>;
		g-rx-fifo-size = <TEST_G_RX_FIFO_SIZE>;
		g-np-tx-fifo-size = <TEST_G_NP_TX_FIFO_SIZE>;
		g-tx-fifo-size = <TEST_G_TX_FIFO_SIZE_0 TEST_G_TX_FIFO_SIZE_1>;
		g-turnaround-time = <TEST_G_TURNAROUND_TIME>;
		dma-burst-len = <TEST_DMA_BURST_LENGTH>;
	};
};
