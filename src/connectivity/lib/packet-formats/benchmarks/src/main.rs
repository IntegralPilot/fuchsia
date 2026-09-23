// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_criterion::FuchsiaCriterion;
use fuchsia_criterion::criterion::Criterion;
use packet::{Buf, BufferAlloc};

pub(crate) mod ip;
mod tcp;
mod udp;

/// A trait abstracting [`criterion::Bencher`] for use in tests.
trait Bencher {
    fn iter<O, F: FnMut() -> O>(&mut self, f: F);
}

trait BenchmarkGroup {
    type Bencher<'a>: Bencher;
    fn register(
        &mut self,
        name: impl Into<String>,
        f: impl for<'a> FnMut(&mut Self::Bencher<'a>) + 'static,
    );
}

impl Bencher for criterion::Bencher<'_> {
    fn iter<O, F: FnMut() -> O>(&mut self, f: F) {
        self.iter(f);
    }
}

impl BenchmarkGroup for criterion::BenchmarkGroup<'_, criterion::measurement::WallTime> {
    type Bencher<'a> = criterion::Bencher<'a>;

    fn register(
        &mut self,
        name: impl Into<String>,
        f: impl for<'a> FnMut(&mut Self::Bencher<'a>) + 'static,
    ) {
        let _ = self.bench_function(name, f);
    }
}

fn gather_benchmarks<G: BenchmarkGroup>(group: &mut G) {
    tcp::get_benches(group);
    udp::get_benches(group);
}

fn main() {
    let mut c = FuchsiaCriterion::default();
    let internal_c: &mut Criterion = &mut c;
    *internal_c = std::mem::take(internal_c)
        .warm_up_time(std::time::Duration::from_millis(1))
        .measurement_time(std::time::Duration::from_millis(100))
        .sample_size(100);
    let name = "fuchsia.netstack.packet-formats";

    let mut group = c.benchmark_group(name);
    gather_benchmarks(&mut group);
    group.finish();
}

pub(crate) struct BufSliceAlloc<'a>(&'a mut [u8]);

impl<'a> BufferAlloc<Buf<&'a mut [u8]>> for BufSliceAlloc<'a> {
    type Error = String;
    fn alloc(self, len: usize) -> Result<Buf<&'a mut [u8]>, Self::Error> {
        let Self(space) = self;
        if len > space.len() {
            Err(format!("invalid requested length {len}, has {}", space.len()))
        } else {
            let (head, _) = space.split_at_mut(len);
            Ok(Buf::new(head, ..))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(crate) struct TestBencher;

    impl Bencher for TestBencher {
        fn iter<O, F: FnMut() -> O>(&mut self, mut f: F) {
            for _ in 0..2 {
                let _ = f();
            }
        }
    }

    impl BenchmarkGroup for TestBencher {
        type Bencher<'a> = Self;

        fn register(
            &mut self,
            name: impl Into<String>,
            mut f: impl for<'a> FnMut(&mut Self::Bencher<'a>) + 'static,
        ) {
            // Add something to stdout so we can tell benchmarks apart, given we
            // can't dynamically generate test cases.
            println!("=== running {}", name.into());
            f(self);
        }
    }

    /// Runs all the benchmarks with a TestBencher with few iterations as a
    /// smoke test that all benchmarks are correct.
    #[test]
    fn smoke_test_benches() {
        gather_benchmarks(&mut TestBencher);
    }
}
