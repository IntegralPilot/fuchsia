// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The CUBIC congestion control algorithm as described in
//! [RFC 9438](https://tools.ietf.org/html/rfc9438).
//!
//! Note: This module uses floating point arithmetics, assuming the TCP stack is
//! in user space, as it is on Fuchsia. By not restricting ourselves, it is more
//! straightforward to implement and easier to understand. We don't need to care
//! about overflows and we get better precision. However, if this algorithm ever
//! needs to be run in kernel space, especially when fp arithmetics are not
//! allowed when the kernel deems saving fp registers too expensive, we should
//! use fixed point arithmetic. Casts from u32 to f32 are always fine as f32 can
//! represent a much bigger value range than u32; On the other hand, f32 to u32
//! casts are also fine because Rust guarantees rounding towards zero (+inf is
//! converted to u32::MAX), which aligns with our intention well.
//!
//! Reference: https://doc.rust-lang.org/reference/expressions/operator-expr.html#type-cast-expressions

use core::num::NonZeroU32;
use core::time::Duration;

use netstack3_base::{EffectiveMss, Instant};

use crate::internal::congestion::{CongestionControlParams, CongestionEvent};

/// Per RFC 9438 (https://tools.ietf.org/html/rfc9438#section-4.6):
///  Parameter beta_cubic SHOULD be set to 0.7.
const CUBIC_BETA: f32 = 0.7;
/// Per RFC 9438 (https://tools.ietf.org/html/rfc9438#section-5.1):
///  Therefore, C SHOULD be set to 0.4.
const CUBIC_C: f32 = 0.4;

/// The CUBIC algorithm state variables.
#[derive(Debug, Clone, Copy, PartialEq, derivative::Derivative)]
#[derivative(Default(bound = ""))]
pub(super) struct Cubic<I, const FAST_CONVERGENCE: bool> {
    /// The start of the current congestion avoidance epoch.
    epoch_start: Option<I>,
    /// Coefficient for the cubic term of time into the current congestion
    /// avoidance epoch.
    k: f32,
    /// The window size when the last congestion event occurred, in bytes.
    w_max: u32,
    /// The window size when `ssthresh` was most recently set (either upon
    /// exiting the first slow start or just before cwnd was reduced in the last
    /// congestion event), in bytes.
    cwnd_prior: u32,
    /// An estimate for the congestion window, in bytes, in the Reno friendly
    /// region.
    reno_w_est: u32,
    /// The running count of ACKed bytes during congestion avoidance that have
    /// not yet been accounted for by the Cubic congestion window. Effectively,
    /// it can be thought of as a remainder on the Cubic window, since our
    /// implementation uses integer arithmetic rather than floating point.
    remaining_cubic_bytes_acked: u32,
    /// The running count of ACKed bytes during congestion avoidance that have
    /// not yet been accounted for by the Reno friendly region. Effectively,
    /// it can be thought of as a remainder on the Reno window estimate, since
    /// our implementation uses integer arithmetic rather than floating point.
    remaining_reno_bytes_acked: u32,
}

impl<I: Instant, const FAST_CONVERGENCE: bool> Cubic<I, FAST_CONVERGENCE> {
    /// Returns the window size governed by the cubic growth function, in bytes.
    ///
    /// This function is responsible for the concave/convex regions described
    /// in the RFC.
    fn cubic_window(&self, t: Duration, mss: EffectiveMss) -> u32 {
        // Per RFC 9438 (https://www.rfc-editor.org/rfc/rfc9438#section-4.2):
        //       W_cubic(t) = C*(t-K)^3 + W_max (Fig. 1)
        let x = t.as_secs_f32() - self.k;
        let w_cubic = (self.cubic_c(mss) * f32::powi(x, 3)) + self.w_max as f32;
        w_cubic as u32
    }

    /// Updates the estimated Reno window upon the reception of an ACK.
    fn reno_friendly_window(&mut self, bytes_acked: u32, cwnd: u32, mss: EffectiveMss) {
        // Per RFC 9438 (https://tools.ietf.org/html/rfc9438#section-4.3):
        //   alpha_cubic must be equal to 3 * ((1-beta_cubic)/(1+beta_cubic))
        const CUBIC_ALPHA: f32 = 3.0 * ((1.0 - CUBIC_BETA) / (1.0 + CUBIC_BETA));
        // Per RFC 9438 (https://tools.ietf.org/html/rfc9438#section-4.3):
        //   Once [...] W_est >= cwnd_prior, the sender SHOULD set alpha_cubic
        //   to 1 to ensure that it can achieve the same congestion window
        //   increment rate as Reno.
        let cubic_alpha = if self.reno_w_est >= self.cwnd_prior { 1.0 } else { CUBIC_ALPHA };

        // Per RFC 9438 (https://tools.ietf.org/html/rfc9438#section-4.3):
        //   W_est = W_est + alpha_cubic * (segments_acked / cwnd)
        //
        // Note: Here we use a similar approach as in appropriate byte counting
        // (RFC 3465) - We count how many bytes are now acked, then we use
        // Figure 4 to calculate how many acked bytes are needed to increase our
        // cwnd by an even multiple of MSS.
        self.remaining_reno_bytes_acked =
            self.remaining_reno_bytes_acked.saturating_add(bytes_acked);
        let required_bytes = (cwnd as f32 / cubic_alpha) as u32;
        let required_bytes = required_bytes.max(1);
        let num_increments = self.remaining_reno_bytes_acked / required_bytes;
        self.reno_w_est =
            self.reno_w_est.saturating_add(num_increments.saturating_mul(u32::from(mss)));
        self.remaining_reno_bytes_acked %= required_bytes;
    }

    pub(super) fn on_ack(
        &mut self,
        CongestionControlParams { cwnd, ssthresh, mss }: &mut CongestionControlParams,
        mut bytes_acked: NonZeroU32,
        now: I,
        rtt: Duration,
    ) {
        if *cwnd < *ssthresh {
            // TODO(https://fxbug.dev/513208004): Implement the HyStart++ slow
            // start algorithm.

            // Slow start, Per RFC 5681 (https://www.rfc-editor.org/rfc/rfc5681#page-6):
            // we RECOMMEND that TCP implementations increase cwnd, per:
            //   cwnd += min (N, SMSS)                      (2)
            *cwnd = cwnd.saturating_add(u32::min(bytes_acked.get(), u32::from(*mss)));
            if *cwnd <= *ssthresh {
                return;
            }
            // Now that we are moving out of slow start, we need to treat the
            // extra bytes differently, set the cwnd back to ssthresh and then
            // backtrack the portion of bytes that should be processed in
            // congestion avoidance.
            match cwnd.checked_sub(*ssthresh).and_then(NonZeroU32::new) {
                None => return,
                Some(diff) => bytes_acked = diff,
            }
            *cwnd = *ssthresh;
        }

        // Congestion avoidance.
        let epoch_start = match self.epoch_start {
            Some(epoch_start) => epoch_start,
            None => {
                // Setup the parameters for the current congestion avoidance epoch.
                if let Some(w_max_diff_cwnd) = self.w_max.checked_sub(*cwnd) {
                    // Per RFC 9438 (https://www.rfc-editor.org/rfc/rfc9438#section-4.2):
                    //   K is calculated using the following equation:
                    //       K = cube_root((w_max - cwnd_epoch) / C) (Fig. 2)
                    self.k = (w_max_diff_cwnd as f32 / self.cubic_c(*mss)).cbrt();
                } else {
                    // Per RFC 9438 (https://www.rfc-editor.org/rfc/rfc9438#section-4.10):
                    //   When CUBIC uses HyStart++ [RFC9406], it may exit the
                    //   the first slow start without incurring any packet loss
                    //   and thus w_max is undefined. In this special case,
                    //   CUBIC sets cwnd_prior = cwnd and switches to congestion
                    //   avoidance. It then increases its congestion window
                    //   size using Figure 1, where t is the elapsed time since
                    //   the beginning of the current congestion avoidance
                    //   stage, K is set to 0, and w_max is set to the
                    //   congestion window size at the beginning of the current
                    //   congestion avoidance stage.
                    self.k = 0.0;
                    self.w_max = *cwnd;
                    self.cwnd_prior = *cwnd;
                }
                self.epoch_start = Some(now);
                // Per RFC 9438 (https://www.rfc-editor.org/rfc/rfc9438#section-4.3):
                //   W_est is set equal to cwnd_epoch at the start of the
                //   congestion avoidance stage.
                self.reno_w_est = *cwnd;
                now
            }
        };

        // Per RFC 9438 (https://www.rfc-editor.org/rfc/rfc9438#section-4.2):
        //   Upon receiving a new ACK during congestion avoidance, CUBIC
        //   computes the target congestion window size after the next RTT
        //   using Figure 1 as follows [...]
        //       target = cwnd if W_cubic(t + RTT) < cwnd
        //       target = 1.5 * cwnd if W_cubic(t+ RTT) > 1.5 * cwnd
        //       target = W_cubic(t + RTT) otherwise
        // where earlier, t was defined as:
        //   t is the elapsed time in seconds from the beginning of the current
        //   congestion avoidance stage -- that is,
        //       t = t_current - t_epoch
        let t = now.saturating_duration_since(epoch_start);
        let target = self.cubic_window(t + rtt, *mss);
        let target = target.clamp(*cwnd, (1.5 * (*cwnd as f32)) as u32);

        // In a *very* rare case, we might overflow the counter if the acks
        // keep coming in and we can't increase our congestion window. Use
        // saturating add here as a defense so that we don't lost ack counts
        // by accident.
        self.remaining_cubic_bytes_acked =
            self.remaining_cubic_bytes_acked.saturating_add(bytes_acked.get());

        // Per RFC 9438 (https://www.rfc-editor.org/rfc/rfc9438#section-4.4):
        //   cwnd MUST be incremented by (target - cwnd)/cwnd for each
        //   received ACK.
        // Note: Here we use a similar approach as in appropriate byte counting
        // (RFC 3465) - We count how many bytes are now acked, then we use Eq. 1
        // to calculate how many acked bytes are needed to increase our cwnd
        // by an even multiple of MSS. The increase rate is (target - cwnd)/cwnd
        // segments per ACK. We can use the reciprocal of this rate to compute
        // the number of ACKs per segment. Because our cubic function is a
        // monotonically increasing function, this method is slightly more
        // aggressive - if we need N acks to increase our window by 1 MSS, then
        // it would take the RFC method at least N acks to increase the same
        // amount. This method is used in the original CUBIC paper[1], and it
        // eliminates the need to use f32 for cwnd, which is a bit awkward
        // especially because our unit is in bytes and it doesn't make much
        // sense to have byte number not to be a whole number.
        // [1]: (https://www.cs.princeton.edu/courses/archive/fall16/cos561/papers/Cubic08.pdf)
        let mut cubic_cwnd = *cwnd;
        if target >= *cwnd {
            let increase_rate = (target - *cwnd) as f32 / *cwnd as f32;
            // The number of bytes to increase cwnd by.
            let increase = (increase_rate * self.remaining_cubic_bytes_acked as f32) as u32;
            // Limit the increase to ensure we don't exceed the cubic target.
            let increase = increase.min(target - *cwnd);
            // Round the increase down to the nearest whole SMSS.
            let mss = u32::from(*mss);
            let increase = (increase / mss) * mss;
            if increase > 0 {
                // `saturating_add` avoids overflow in `cwnd`. See https://fxbug.dev/327628809.
                cubic_cwnd = cwnd.saturating_add(increase);
                let to_subtract_from_bytes_acked = (increase as f32 / increase_rate) as u32;
                self.remaining_cubic_bytes_acked =
                    self.remaining_cubic_bytes_acked.saturating_sub(to_subtract_from_bytes_acked);
            }
        }

        self.reno_friendly_window(bytes_acked.get(), *cwnd, *mss);

        // Per RFC 9438 (https://www.rfc-editor.org/rfc/rfc9438#section-4.3):
        //   CUBIC checks whether W_cubic(t) is less than W_est(t). If so,
        //   CUBIC is in the Reno-friendly region and cwnd SHOULD be set to
        //   W_est(t) at each reception of a new ACK.
        *cwnd = u32::max(cubic_cwnd, self.reno_w_est);
    }

    pub(super) fn on_congestion_event(
        &mut self,
        CongestionControlParams { cwnd, ssthresh, mss }: &mut CongestionControlParams,
        event: CongestionEvent,
        flight_size: u32,
    ) {
        // End the current congestion avoidance epoch.
        self.epoch_start = None;
        // Per RFC 9438 (https://www.rfc-editor.org/rfc/rfc9438#section-4.7):
        //   With fast convergence, when a congestion event occurs, W_max is
        //   updated as follows, before the window reduction described in
        //   Section 4.6:
        //       W_max = cwnd * ((1 + beta_cubic) / 2) if cwnd < W_max and fast
        //               convergence is enabled, further reduce W_max.
        //       W_max = cwnd otherwise.
        if FAST_CONVERGENCE && *cwnd < self.w_max {
            self.w_max = (*cwnd as f32 * (1.0 + CUBIC_BETA) / 2.0) as u32;
        } else {
            self.w_max = *cwnd;
        }

        // Per RFC 9438 (https://www.rfc-editor.org/rfc/rfc9438#section-4.6):
        //   When a congestion event is detected by the mechanisms described in
        //   Section 3.1, CUBIC updates W_max and reduces cwnd and ssthresh
        //   immediately, as described below.
        //       ssthresh = flight_size * beta_cubic
        //       cwnd_prior = cwnd
        //       cwnd = max(ssthresh, 2), if reduction on loss
        //       cwnd = max(ssthresh, 1), if reduction on ECE
        //       ssthresh = max(ssthresh, 2)
        self.cwnd_prior = *cwnd;
        let mss_u32 = u32::from(*mss);
        let ssthresh_segs = (flight_size as f32 * CUBIC_BETA) as u32 / mss_u32;
        *ssthresh = u32::max(ssthresh_segs, 2) * mss_u32;
        match event {
            CongestionEvent::PacketLoss => {
                *cwnd = *ssthresh;
            }
            CongestionEvent::Timeout => {
                // Per RFC 9438 (https://www.rfc-editor.org/rfc/rfc9438#section-4.8):
                //   In case of timeout, CUBIC follows Reno to reduce cwnd [RFC5681].
                // The Reno cwnd reduction strategy is described in RFC 5681
                // (https://www.rfc-editor.org/rfc/rfc5681#page-8):
                //   Furthermore, upon a timeout (as specified in [RFC2988]) cwnd MUST be
                //   set to no more than the loss window, LW, which equals 1 full-sized
                //   segment (regardless of the value of IW).
                *cwnd = mss_u32
            }
        }

        // Reset our running count of the acked bytes.
        self.remaining_cubic_bytes_acked = 0;
        self.remaining_reno_bytes_acked = 0;
    }

    fn cubic_c(&self, mss: EffectiveMss) -> f32 {
        // Note: cwnd and w_max are in unit of bytes as opposed to segments in
        // RFC, so C should be CUBIC_C * mss for our implementation.
        CUBIC_C * u32::from(mss) as f32
    }
}

#[cfg(test)]
mod tests {
    use netstack3_base::testutil::FakeInstantCtx;
    use netstack3_base::{EffectiveMss, InstantContext as _, Mss, MssSizeLimiters};
    use test_case::test_case;

    use super::*;

    const DEFAULT_MSS: EffectiveMss =
        EffectiveMss::from_mss(Mss::DEFAULT_IPV4, MssSizeLimiters { timestamp_enabled: false });
    impl<I: Instant, const FAST_CONVERGENCE: bool> Cubic<I, FAST_CONVERGENCE> {
        // Helper function in test that takes a u32 instead of a NonZeroU32
        // as we know we never pass 0 in the test and it's a bit clumsy to
        // convert a u32 into a NonZeroU32 every time.
        fn on_ack_u32(
            &mut self,
            params: &mut CongestionControlParams,
            bytes_acked: u32,
            now: I,
            rtt: Duration,
        ) {
            self.on_ack(params, NonZeroU32::new(bytes_acked).unwrap(), now, rtt)
        }
    }

    // The following expectations are extracted from table. 1 and table. 2 in
    // RFC 9438 (https://www.rfc-editor.org/rfc/rfc9438#section-5.1). Note that
    // some numbers do not match as-is, but the error rate is acceptable (~2%),
    // this can be attributed to a few things, e.g., the way we simulate is
    // slightly different from the the ideal process, as we start the first
    // congestion avoidance with the convex region which grows pretty fast, also
    // the theoretical estimation is an approximation already. The theoretical
    // value is included in the name for each case.
    //
    // NB: Skip the tests with a loss_rate_reciprocal of 100_000_000 as they
    // take too long to run.
    #[test_case(Duration::from_millis(100), 100 => 11; "rtt=0.1 p=0.01 Wavg=12")]
    #[test_case(Duration::from_millis(100), 1_000 => 38; "rtt=0.1 p=0.001 Wavg=38")]
    #[test_case(Duration::from_millis(100), 10_000 => 187; "rtt=0.1 p=0.0001 Wavg=187")]
    #[test_case(Duration::from_millis(100), 100_000 => 1057; "rtt=0.1 p=0.00001 Wavg=1054")]
    #[test_case(Duration::from_millis(100), 1_000_000 => 5938; "rtt=0.1 p=0.000001 Wavg=5926")]
    #[test_case(Duration::from_millis(100), 10_000_000 => 33464; "rtt=0.1 p=0.0000001 Wavg=33325")]
    #[test_case(Duration::from_millis(10), 100 => 11; "rtt=0.01 p=0.01 Wavg=12")]
    #[test_case(Duration::from_millis(10), 1_000 => 37; "rtt=0.01 p=0.001 Wavg=38")]
    #[test_case(Duration::from_millis(10), 10_000 => 121; "rtt=0.01 p=0.0001 Wavg=120")]
    #[test_case(Duration::from_millis(10), 100_000 => 386; "rtt=0.01 p=0.00001 Wavg=379")]
    #[test_case(Duration::from_millis(10), 1_000_000 => 1261; "rtt=0.01 p=0.000001 Wavg=1200")]
    #[test_case(Duration::from_millis(10), 10_000_000 => 5952; "rtt=0.01 p=0.0000001 Wavg=5926")]
    fn average_window_size(rtt: Duration, loss_rate_reciprocal: u32) -> u32 {
        // Run the test long enough to experience 5 loss events.
        let round_trips = loss_rate_reciprocal * 5;

        // The theoretical predictions do not consider fast convergence,
        // disable it.
        let mut cubic = Cubic::<_, false /* FAST_CONVERGENCE */>::default();
        let mut params = CongestionControlParams::with_mss(DEFAULT_MSS);
        // The theoretical value is a prediction for the congestion avoidance
        // only, set ssthresh to 1 so that we skip slow start. Slow start can
        // grow the window size very quickly.
        params.ssthresh = 1;

        let mut clock = FakeInstantCtx::default();

        let mut avg_pkts = 0.0f64;
        let mut ack_cnt = 0;

        // We simulate a deterministic loss model, i.e., for loss_rate p, we
        // drop one packet for every 1/p packets.
        for _ in 0..round_trips {
            let cwnd = params.rounded_cwnd().cwnd();
            if ack_cnt >= loss_rate_reciprocal {
                ack_cnt -= loss_rate_reciprocal;
                let flight_size = params.cwnd;
                cubic.on_congestion_event(&mut params, CongestionEvent::PacketLoss, flight_size);
            } else {
                ack_cnt += cwnd / u32::from(params.mss);
                // On a true TCP connection, we'd get at least one ack for every
                // two segments. However, for the purpose of our simulation, we
                // pretend that a singular ACK arrives that ACKs all the sent
                // bytes (i.e. the whole `cwnd`). This allows us to speed up the
                // simulation, without changing the underlying math.
                cubic.on_ack_u32(&mut params, cwnd, clock.now(), rtt);
            }
            clock.sleep(rtt);
            // NB: Use f64, as f32 looses precision on the test cases with a
            // large number of round trips.
            avg_pkts += (cwnd as f64 / u32::from(params.mss) as f64) as f64 / round_trips as f64;
        }
        avg_pkts as u32
    }

    #[test]
    fn cubic_example() {
        let mut clock = FakeInstantCtx::default();
        let mut cubic = Cubic::<_, true /* FAST_CONVERGENCE */>::default();
        let mut params = CongestionControlParams::with_mss(DEFAULT_MSS);
        const RTT: Duration = Duration::from_millis(100);

        // Assert we have the correct initial window.
        assert_eq!(params.cwnd, 4 * u32::from(DEFAULT_MSS));

        // Slow start.
        clock.sleep(RTT);
        for _seg in 0..params.cwnd / u32::from(DEFAULT_MSS) {
            cubic.on_ack_u32(&mut params, u32::from(DEFAULT_MSS), clock.now(), RTT);
        }
        assert_eq!(params.cwnd, 8 * u32::from(DEFAULT_MSS));

        clock.sleep(RTT);
        let flight_size = params.cwnd;
        cubic.on_congestion_event(&mut params, CongestionEvent::Timeout, flight_size);
        assert_eq!(params.cwnd, u32::from(DEFAULT_MSS));

        // We are now back in slow start.
        clock.sleep(RTT);
        cubic.on_ack_u32(&mut params, u32::from(DEFAULT_MSS), clock.now(), RTT);
        assert_eq!(params.cwnd, 2 * u32::from(DEFAULT_MSS));

        clock.sleep(RTT);
        for _ in 0..2 {
            cubic.on_ack_u32(&mut params, u32::from(DEFAULT_MSS), clock.now(), RTT);
        }
        assert_eq!(params.cwnd, 4 * u32::from(DEFAULT_MSS));

        // In this roundtrip, we enter a new congestion epoch from slow start,
        // in this round trip, both cubic and the reno-friendly window equations
        // will be reset, so the cwnd in this round trip will be ssthresh, which
        // is 2680 bytes, or 5 full sized segments.
        clock.sleep(RTT);
        for _seg in 0..params.cwnd / u32::from(DEFAULT_MSS) {
            cubic.on_ack_u32(&mut params, u32::from(DEFAULT_MSS), clock.now(), RTT);
        }
        assert_eq!(params.cwnd, 5 * u32::from(DEFAULT_MSS));

        // In the Reno-Friendly region, the cwnd is increased by alpha_cubic MSS
        // per cwnd of acked data. Since alpha_cubic is approximately 0.53, in
        // practice it takes 2 full RTT to observe this increase.
        for _ in 0..2 {
            clock.sleep(RTT);
            for _seg in 0..params.cwnd / u32::from(DEFAULT_MSS) {
                cubic.on_ack_u32(&mut params, u32::from(DEFAULT_MSS), clock.now(), RTT);
            }
        }
        assert_eq!(params.cwnd, 6 * u32::from(DEFAULT_MSS));
    }

    // This is a regression test for https://fxbug.dev/327628809.
    #[test_case(u32::MAX ; "cwnd is u32::MAX")]
    #[test_case(u32::MAX - 1; "cwnd is u32::MAX - 1")]
    fn repro_overflow_b327628809(cwnd: u32) {
        let clock = FakeInstantCtx::default();
        let mut cubic = Cubic::<_, true /* FAST_CONVERGENCE */>::default();
        let mut params = CongestionControlParams { ssthresh: 0, cwnd, mss: DEFAULT_MSS };
        const RTT: Duration = Duration::from_millis(100);

        cubic.on_ack(&mut params, NonZeroU32::MIN, clock.now(), RTT);
    }

    // This is a regression test for https://fxbug.dev/412748465.
    #[test]
    fn repro_overflow_b412748465() {
        let clock = FakeInstantCtx::default();
        let mut cubic = Cubic::<_, true /* FAST_CONVERGENCE */>::default();
        // Setup the params in slow start with `cwnd` close to overflow.
        let mut params =
            CongestionControlParams { ssthresh: u32::MAX, cwnd: u32::MAX - 1, mss: DEFAULT_MSS };
        const RTT: Duration = Duration::from_millis(100);
        // Ack enough bytes to push cwnd over u32::MAX.
        cubic.on_ack(
            &mut params,
            NonZeroU32::new(2).unwrap(), /*bytes_acked*/
            clock.now(),
            RTT,
        );
    }

    // Verify that the `flight_size` is used when updating congestion parameters
    // after a congestion event, rather than the `cwnd`.
    #[test_case(20, 20, 14; "same_as_cwnd")]
    #[test_case(20, 10, 7; "half_of_cwnd")]
    #[test_case(20, 0, 2; "saturates_to_min")]
    fn congestion_events_account_for_flight_size(cwnd: u32, flight_size: u32, expected_cwnd: u32) {
        let mut cubic =
            Cubic::<netstack3_base::testutil::FakeInstant, true /* FAST_CONVERGENCE */>::default();
        let mss = u32::from(DEFAULT_MSS);

        let cwnd = cwnd * mss;
        let flight_size = flight_size * mss;
        let expected_cwnd = expected_cwnd * mss;

        let mut params = CongestionControlParams { ssthresh: 0, cwnd, mss: DEFAULT_MSS };
        cubic.on_congestion_event(&mut params, CongestionEvent::PacketLoss, flight_size);

        assert_eq!(params.ssthresh, expected_cwnd);
        assert_eq!(params.cwnd, expected_cwnd);
    }
}
