// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[macro_use]
mod _macros;
mod log;
mod mesh;
mod minecraft;

use core::mem::MaybeUninit;

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use fidl_next::EncoderExt as _;
use rand::rngs::StdRng;
use rand::{Rng, RngExt as _, SeedableRng as _};

pub trait Generate {
    fn generate(rng: &mut impl Rng) -> Self;
}

impl Generate for bool {
    fn generate(rng: &mut impl Rng) -> Self {
        rng.random_bool(0.5)
    }
}

impl Generate for u32 {
    fn generate(rng: &mut impl Rng) -> Self {
        rng.random()
    }
}

impl<T: Generate> Generate for Box<T> {
    fn generate(rng: &mut impl Rng) -> Self {
        Box::new(T::generate(rng))
    }
}

impl<T: Generate> Generate for Option<T> {
    fn generate(rng: &mut impl Rng) -> Self {
        if rng.random_bool(0.5) { Some(T::generate(rng)) } else { None }
    }
}

impl<T: Generate, const N: usize> Generate for [T; N] {
    fn generate(rng: &mut impl Rng) -> Self {
        let mut result = MaybeUninit::<[T; N]>::uninit();
        for i in 0..N {
            unsafe {
                result.as_mut_ptr().cast::<T>().add(i).write(T::generate(rng));
            }
        }
        unsafe { result.assume_init() }
    }
}

fn generate_vec<T: Generate>(rng: &mut impl Rng, size: usize) -> Vec<T> {
    let mut result = Vec::with_capacity(size);
    for _ in 0..size {
        result.push(T::generate(rng));
    }
    result
}

fn make_rng() -> StdRng {
    // Nothing up my sleeve: seed is first 32 bytes of pi
    StdRng::from_seed([
        0x32, 0x43, 0xF6, 0xA8, 0x88, 0x5A, 0x30, 0x8D, 0x31, 0x31, 0x98, 0xA2, 0xE0, 0x37, 0x07,
        0x34, 0x4A, 0x40, 0x93, 0x82, 0x22, 0x99, 0xF3, 0x1D, 0x00, 0x82, 0xEF, 0xA9, 0x8E, 0xC4,
        0xE6, 0xC8,
    ])
}

fn bench_rust<T, D>(c: &mut Criterion, name: &str, input: T, input_size: usize, default: D)
where
    T: fidl::encoding::TypeMarker,
    for<'a> &'a T: fidl::encoding::Encode<T, fidl::encoding::NoHandleResourceDialect>,
    T::Owned: fidl::encoding::Decode<T, fidl::encoding::NoHandleResourceDialect>,
    D: 'static + Fn() -> T::Owned,
{
    let mut decode_buf = Vec::new();
    let mut decode_handle_buf = Vec::new();
    fidl::encoding::Encoder::<'_, fidl::encoding::NoHandleResourceDialect>::encode::<T>(
        &mut decode_buf,
        &mut decode_handle_buf,
        &input,
    )
    .unwrap();
    let mut handle_infos = fidl::convert_handle_dispositions_to_infos(decode_handle_buf).unwrap();

    c.bench_function(&format!("rust/{name}/encode/{input_size}x"), move |b| {
        let mut buf = Vec::new();
        let mut handle_buf = Vec::new();
        b.iter(|| {
            buf.clear();
            handle_buf.clear();
            black_box(
                fidl::encoding::Encoder::<'_, fidl::encoding::NoHandleResourceDialect>::encode::<T>(
                    &mut buf,
                    &mut handle_buf,
                    black_box(&input),
                ),
            )
            .unwrap();
        });
    });

    c.bench_function(
        &format!("rust/{name}/decode_natural/{input_size}x"),
        move |b| {
            b.iter(|| {
                let mut out = default();

                black_box(fidl::encoding::Decoder::<'_, fidl::encoding::NoHandleResourceDialect>::decode_with_context::<T>(
                    fidl::encoding::Context {
                        wire_format_version: fidl::encoding::WireFormatVersion::V2,
                    },
                    black_box(&decode_buf),
                    &mut handle_infos,
                    &mut out,
                )).unwrap();
            });
        }
    );
}

macro_rules! bench_rust_next {
    ($c:expr, $name:expr, $input:expr, $input_size:expr, $T:ty, $W:ty) => {{
        use fidl_next::AsDecoderExt as _;

        let input = $input;
        let mut decode_chunks = Vec::new();
        decode_chunks.encode_next::<$W, _>(&input).unwrap();

        $c.bench_function(&format!("rust_next/{}/encode/{}x", $name, $input_size), move |b| {
            let mut chunks = Vec::new();
            b.iter(|| {
                chunks.clear();
                black_box(chunks.encode_next::<$W, _>(black_box(&input))).unwrap();
            });
        });

        let decode_wire_chunks = decode_chunks.clone();
        $c.bench_function(&format!("rust_next/{}/decode_wire/{}x", $name, $input_size), move |b| {
            b.iter_batched(
                || decode_wire_chunks.clone(),
                |decode_chunks| {
                    black_box(decode_chunks.into_decoded::<$W>()).unwrap();
                },
                criterion::BatchSize::SmallInput,
            );
        });

        $c.bench_function(
            &format!("rust_next/{}/decode_natural/{}x", $name, $input_size),
            move |b| {
                b.iter_batched(
                    || decode_chunks.clone(),
                    |decode_chunks| {
                        let decoded = decode_chunks.into_decoded::<$W>().unwrap();
                        black_box(decoded.take_as::<$T>());
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }};
}

fn bench_log(c: &mut Criterion) {
    const INPUT_SIZES: [usize; 4] = [1, 10, 100, 1000];

    for input_size in INPUT_SIZES {
        bench_rust(c, "log", log::generate_input_rust(input_size), input_size, || {
            fidl_test_benchmark::Logs { logs: Vec::new() }
        });
        bench_rust(c, "mesh", mesh::generate_input_rust(input_size), input_size, || {
            fidl_test_benchmark::Mesh { triangles: Vec::new() }
        });
        bench_rust(c, "minecraft", minecraft::generate_input_rust(input_size), input_size, || {
            fidl_test_benchmark::Players { players: Vec::new() }
        });

        bench_rust_next!(
            c,
            "log",
            log::generate_input_rust_next(input_size),
            input_size,
            fidl_next_test_benchmark::Logs,
            fidl_next_test_benchmark::wire::Logs<'_>
        );
        bench_rust_next!(
            c,
            "mesh",
            mesh::generate_input_rust_next(input_size),
            input_size,
            fidl_next_test_benchmark::Mesh,
            fidl_next_test_benchmark::wire::Mesh<'_>
        );
        bench_rust_next!(
            c,
            "minecraft",
            minecraft::generate_input_rust_next(input_size),
            input_size,
            fidl_next_test_benchmark::Players,
            fidl_next_test_benchmark::wire::Players<'_>
        );
    }
}

criterion_group!(benchmark, bench_log);
criterion_main!(benchmark);
