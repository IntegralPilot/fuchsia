// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::energy::*;

/// Arbitrarily chosen value.
/// N can be parameterized once const generics found its way into stable (RFC 2000).
const N: usize = 20;

/// Tracks the moving average for signal strength given in dBm.
#[derive(Debug)]
pub struct SignalStrengthAverage {
    sum: FemtoWatt,
    samples: [DecibelMilliWatt; N],
    n: usize,
    i: usize,
}

impl SignalStrengthAverage {
    pub fn new() -> Self {
        Self { sum: FemtoWatt(0), n: 0, i: 0, samples: [DecibelMilliWatt(i8::MIN); N] }
    }

    pub fn avg_dbm(&self) -> DecibelMilliWatt {
        self.avg_femto_watt().into()
    }

    pub fn avg_femto_watt(&self) -> FemtoWatt {
        FemtoWatt(match self.n {
            0 => 0,
            _ => self.sum.0 / (self.n as u64),
        })
    }

    pub fn add(&mut self, dbm: DecibelMilliWatt) {
        if self.n < N {
            self.n += 1;
        } else {
            self.sum -= self.samples[self.i].into();
        }
        self.sum += dbm.into();
        self.samples[self.i] = dbm;
        self.i = (self.i + 1) % N;
    }

    pub fn reset(&mut self) {
        self.n = 0;
        self.sum = FemtoWatt(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Bounds in these tests have been rewritten to step around a bug found in the Rust toolchain.
    // The Rust toolchain bug is being investigated at https://fxbug.dev/381238482.

    #[test]
    fn avg() {
        let mut signal_avg = SignalStrengthAverage::new();
        // Test 10 samples:
        // Range should be: -30..-20
        for dbm_abs in (21..=30).rev() {
            let dbm = -dbm_abs;
            print!("{} + ", FemtoWatt::from(DecibelMilliWatt(dbm)).0);
            signal_avg.add(DecibelMilliWatt(dbm));
        }
        // Avg. actual: -14.58dBm
        // Avg. due to femotWatt approximations: -14.65dBm
        assert_eq!(signal_avg.avg_femto_watt(), FemtoWatt(3_421_503_488));
        assert_eq!(signal_avg.avg_dbm(), FemtoWatt(3_421_503_488).into());

        // Fill up sample count to N.
        // Range should be -20..-10
        for dbm_abs in (11..=20).rev() {
            let dbm = -dbm_abs;
            print!("{} + ", FemtoWatt::from(DecibelMilliWatt(dbm)).0);
            signal_avg.add(DecibelMilliWatt(dbm));
        }
        // Avg. actual: -4.17dBm
        // Avg. due to femotWatt approximations: -4.24dBm
        assert_eq!(signal_avg.avg_femto_watt(), FemtoWatt(18_811_768_012));
        assert_eq!(signal_avg.avg_dbm(), FemtoWatt(18_811_768_012).into());

        // Overflow sample count. Effectively, only [-20, 0) will be summed up due to N = 20.
        // Range should be -10..0
        for dbm_abs in (1..=10).rev() {
            let dbm = -dbm_abs;
            signal_avg.add(DecibelMilliWatt(dbm));
        }
        // Avg. actual: -7.18dBm
        // Avg. due to femotWatt approximations: -7.26dBm
        assert_eq!(signal_avg.avg_femto_watt(), FemtoWatt(187_852_809_830));
        assert_eq!(signal_avg.avg_dbm(), FemtoWatt(187_852_809_830).into());
    }

    #[fuchsia::test]
    fn reset() {
        let mut signal_avg = SignalStrengthAverage::new();
        // Range should be -30..0
        for dbm_abs in 1..=30 {
            let dbm = -dbm_abs;
            signal_avg.add(DecibelMilliWatt(dbm));
        }
        signal_avg.reset();

        assert_eq!(signal_avg.avg_dbm(), DecibelMilliWatt(-128));
        assert_eq!(signal_avg.avg_femto_watt(), FemtoWatt(0));

        signal_avg.add(DecibelMilliWatt(-30));
        assert_eq!(signal_avg.avg_dbm(), DecibelMilliWatt(-30));
        assert_eq!(signal_avg.avg_femto_watt(), FemtoWatt(983_564_288));
    }
}
