// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_LIB_AMLOGIC_INCLUDE_SOC_AML_A1_A1_HW_H_
#define SRC_DEVICES_LIB_AMLOGIC_INCLUDE_SOC_AML_A1_A1_HW_H_

// clock control registers
#define A1_CLK_BASE 0xfe000800
#define A1_CLK_LENGTH 0x400

#define A1_MSR_CLK_BASE 0xfe003400
#define A1_MSR_CLK_LENGTH 0x400

// Analog Control for PLL clock
#define A1_ANACTRL_BASE 0xfe007c00
#define A1_ANACTRL_LENGTH 0x400

// CPUCTRL for cpu clock
#define A1_CPUCTRL_CLK_CTRL0_BASE ((0x0020 << 2) + 0xfd000000)
#define A1_CPUCTRL_CLK_CTRL0_LENGTH 0x4

#define A1_ANACTRL_SYSPLL_CTRL0 (0x0040 << 2)  // 0x100
#define A1_ANACTRL_SYSPLL_CTRL1 (0x0041 << 2)  // 0x104
#define A1_ANACTRL_SYSPLL_CTRL2 (0x0042 << 2)  // 0x108
#define A1_ANACTRL_SYSPLL_CTRL3 (0x0043 << 2)  // 0x10c
#define A1_ANACTRL_SYSPLL_CTRL4 (0x0044 << 2)  // 0x110
#define A1_ANACTRL_SYSPLL_STS (0x0045 << 2)    // 0x114

#define A1_ANACTRL_HIFIPLL_CTRL0 (0x0050 << 2)  // 0x140
#define A1_ANACTRL_HIFIPLL_CTRL1 (0x0051 << 2)  // 0x144
#define A1_ANACTRL_HIFIPLL_CTRL2 (0x0052 << 2)  // 0x148
#define A1_ANACTRL_HIFIPLL_CTRL3 (0x0053 << 2)  // 0x14c
#define A1_ANACTRL_HIFIPLL_CTRL4 (0x0054 << 2)  // 0x150
#define A1_ANACTRL_HIFIPLL_STS (0x0055 << 2)    // 0x154

// gpio
#define A1_GPIO_BASE 0xfe000400
#define A1_GPIO_LENGTH 0x400
#define A1_GPIO_INTERRUPT_BASE ((0x10 << 2) + A1_GPIO_BASE)
#define A1_GPIO_INTERRUPT_LENGTH 0x14

// i2c
#define A1_I2C_LENGTH 0x200
#define A1_I2C_A_BASE 0xfe001400
#define A1_I2C_B_BASE 0xfe005c00
#define A1_I2C_C_BASE 0xfe006800
#define A1_I2C_D_BASE 0xfe006c00

// spicc
#define A1_SPICC0_BASE 0xfe003800
#define A1_SPICC0_LENGTH 0x400

// spifc
#define A1_SPIFC_BASE 0xfd000400
#define A1_SPIFC_LENGTH 0x400
#define A1_SPIFC_CLOCK_BASE 0xfe0008d8
#define A1_SPIFC_CLOCK_LENGTH 0x4

// rtc

// mailbox
#define A1_DSPA_BASE 0xfe030000
#define A1_DSPA_BASE_LENGTH 0x10000
#define A1_DSPB_BASE 0xfe040000
#define A1_DSPB_BASE_LENGTH 0x10000
#define A1_DSPA_PAYLOAD_BASE 0xffffd400
#define A1_DSPA_PAYLOAD_BASE_LENGTH 0x400
#define A1_DSPB_PAYLOAD_BASE 0xffffd800
#define A1_DSPB_PAYLOAD_BASE_LENGTH 0x400

// dsp

// Peripherals - datasheet is nondescript about this section, but it contains
//  top level ethernet control and temp sensor registers

// SDIO
#define A1_EMMC_A_BASE 0xfe010000
#define A1_EMMC_A_LENGTH 0x1000

// DMC
#define A1_DMC_BASE 0xfd020000
#define A1_DMC_LENGTH 0x400

// NNA

// Power domain

// Memory Power Domain

// Reset
#define A1_RESET_BASE 0xfe000000
#define A1_RESET_LENGTH 0x400

#define A1_RESET0_REGISTER 0x0
#define A1_RESET1_REGISTER 0x4
#define A1_RESET2_REGISTER 0x8
#define A1_SEC_RESET0_REGISTER 0x140
#define A1_RESET0_LEVEL 0x40
#define A1_RESET1_LEVEL 0x44
#define A1_RESET2_LEVEL 0x48
#define A1_SEC_RESET0_LEVEL 0x144
#define A1_RESET0_MASK 0x80
#define A1_RESET1_MASK 0x84
#define A1_RESET2_MASK 0x88
#define A1_SEC_RESET0__MASK 0x148

// IRQs
#define A1_DSPA_RECV_IRQ 32  // 0+32
#define A1_DSPB_RECV_IRQ 36  // 4+32
#define A1_I2C_A_IRQ 64      // 32+32
#define A1_SPICC0_IRQ 80     // 32+48
#define A1_GPIO_IRQ_0 81     // 32+49
#define A1_GPIO_IRQ_1 82     // 32+50
#define A1_GPIO_IRQ_2 83     // 32+51
#define A1_GPIO_IRQ_3 84     // 32+52
#define A1_GPIO_IRQ_4 85     // 32+53
#define A1_GPIO_IRQ_5 86     // 32+54
#define A1_GPIO_IRQ_6 87     // 32+55
#define A1_GPIO_IRQ_7 88     // 32+56
#define A1_TS_PLL_IRQ 89     // 57+32
#define A1_SD_EMMC_A_IRQ 90  // 58+32
#define A1_I2C_B_IRQ 100     // 32+68
#define A1_I2C_C_IRQ 108     // 32+76
#define A1_I2C_D_IRQ 110     // 32+78
#define A1_USB_IRQ 122       // 32+90
#define A1_DDR_BW_IRQ 141    // 109+32

// PWM
#define A1_PWM_LENGTH 0x400  // applies to each PWM bank
#define A1_PWM_AB_BASE 0xfe002400
#define A1_PWM_PWM_A 0x0
#define A1_PWM_PWM_B 0x4
#define A1_PWM_MISC_REG_AB 0x8
#define A1_DS_A_B 0xc
#define A1_PWM_TIME_AB 0x10
#define A1_PWM_A2 0x14
#define A1_PWM_B2 0x18
#define A1_PWM_BLINK_AB 0x1c
#define A1_PWM_LOCK_AB 0x20

#define A1_PWM_CD_BASE 0xfe002800
#define A1_PWM_PWM_C 0x0
#define A1_PWM_PWM_D 0x4
#define A1_PWM_MISC_REG_CD 0x8
#define A1_DS_C_D 0xc
#define A1_PWM_TIME_CD 0x10
#define A1_PWM_C2 0x14
#define A1_PWM_D2 0x18
#define A1_PWM_BLINK_CD 0x1c
#define A1_PWM_LOCK_CD 0x20

#define A1_PWM_EF_BASE 0xfe005400
#define A1_PWM_PWM_E 0x0
#define A1_PWM_PWM_F 0x4
#define A1_PWM_MISC_REG_EF 0x8
#define A1_DS_E_F 0xc
#define A1_PWM_TIME_EF 0x10
#define A1_PWM_E2 0x14
#define A1_PWM_F2 0x18
#define A1_PWM_BLINK_EF 0x1c
#define A1_PWM_LOCK_EF 0x20

// AUDIO

// For 'fdf::MmioBuffer::Create'
// |base| is guaranteed to be page aligned.

// USB
#define A1_USBCTRL_BASE 0xfe004400
#define A1_USBCTRL_LENGTH 0x400
#define A1_USBPHY_BASE 0xfe004000
#define A1_USBPHY_LENGTH 0x400
#define A1_USB_BASE 0xff400000
#define A1_USB_LENGTH 0x100000
// sys_ctrl
#define A1_SYS_CTRL_BASE 0xfe005800
#define A1_SYS_CTRL_LENGTH 0x400

// sticky register -  not reset by watchdog
#define A1_SYS_CTRL_SEC_STATUS_REG13 ((0x00cd << 2) + 0xfe005800)

// Temperature
#define A1_TEMP_SENSOR_PLL_BASE 0xfe004c00
#define A1_TEMP_SENSOR_PLL_LENGTH 0x50
#define A1_TEMP_SENSOR_PLL_TRIM A1_SYS_CTRL_SEC_STATUS_REG13
#define A1_TEMP_SENSOR_PLL_TRIM_LENGTH 0x4

#endif  // SRC_DEVICES_LIB_AMLOGIC_INCLUDE_SOC_AML_A1_A1_HW_H_
