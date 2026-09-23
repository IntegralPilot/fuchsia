// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use mmio::{register, register_block};

register! {
    #[register(offset = 0x00 << 2, mode = RW)]
    pub struct Reg0(u32) {
        pub val, set_val: 31, 0;
        pub bool, sampling_stop, set_sampling_stop: 14;
        pub fifo_cnt_irq, set_fifo_cnt_irq: 8, 4;
        pub bool, fifo_irq_en, set_fifo_irq_en: 3;
        pub bool, sampling_start, set_sampling_start: 2;
        pub bool, sampling_enable, set_sampling_enable: 0;
    }

    #[register(offset = 0x01 << 2, mode = RW)]
    pub struct ChanList(u32);

    #[register(offset = 0x02 << 2, mode = RW)]
    pub struct AvgCntl(u32);

    #[register(offset = 0x03 << 2, mode = RW)]
    pub struct Reg3(u32) {
        pub val, set_val: 31, 0;
        pub bool, adc_en, set_adc_en: 21;
    }

    #[register(offset = 0x04 << 2, mode = RW)]
    pub struct Delay(u32);

    #[register(offset = 0x06 << 2, mode = RO)]
    pub struct FifoRd(u32);

    #[register(offset = 0x07 << 2, mode = RW)]
    pub struct AuxSw(u32);

    #[register(offset = 0x08 << 2, mode = RW)]
    pub struct Chan10Sw(u32);

    #[register(offset = 0x09 << 2, mode = RW)]
    pub struct DetectIdleSw(u32);

    #[register(offset = 0x0b << 2, mode = RW)]
    pub struct Reg11(u32) {
        pub val, set_val: 31, 0;
        pub bool, ts_vbg_en, set_ts_vbg_en: 13;
        pub bool, rsv6, set_rsv6: 6;
        pub bool, rsv5, set_rsv5: 5;
        pub bool, rsv1, set_rsv1: 1;
    }

    #[register(offset = 0x0d << 2, mode = RW)]
    pub struct Reg13(u32);

    #[register(offset = 0x24 << 2, mode = RW)]
    pub struct AoSarClk(u32) {
        pub val, set_val: 31, 0;
        pub bool, clk_ena, set_clk_ena: 8;
        pub clk_src, set_clk_src: 10, 9;
        pub clk_div, set_clk_div: 7, 0;
    }
}

register_block! {
    pub struct AdcRegsBlock<M> {
        pub reg0: Reg0,
        pub chan_list: ChanList,
        pub avg_cntl: AvgCntl,
        pub reg3: Reg3,
        pub delay: Delay,
        pub fifo_rd: FifoRd,
        pub aux_sw: AuxSw,
        pub chan_10_sw: Chan10Sw,
        pub detect_idle_sw: DetectIdleSw,
        pub reg11: Reg11,
        pub reg13: Reg13,
    }
}

register_block! {
    pub struct AoRegsBlock<M> {
        pub ao_sar_clk: AoSarClk,
    }
}
