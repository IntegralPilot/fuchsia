# Changelog

## 1.4.0 (20250219)

- Updated `rand` and `rand-core` to 0.9. [#18](https://github.com/hoxxep/rapidhash/pull/18)
- Fixed issue where using feature `unsafe` and without `std` would fail to compile. [#15](https://github.com/hoxxep/rapidhash/issues/15) and [#17](https://github.com/hoxxep/rapidhash/pull/17)

## 1.3.0 (20241208)

- Added `rapidhash_file` for streaming file hashing. [#10](https://github.com/hoxxep/rapidhash/pull/10)
- Added file streaming and `--help` to the CLI. [#10](https://github.com/hoxxep/rapidhash/pull/10)

## 1.2.0 (20241204)

- Added rapidhash CLI via `cargo install rapidhash`.
- Docs typo fix.

## 1.1.0 (20241003)

- Deprecated `RapidHashBuilder`.
- Added `RapidBuildHasher` to replace `RapidHashBuilder`.

## 1.0.0 (20241002)

Ownership kindly transferred by Justin Bradford to [Liam Gray](https://github.com/hoxxep) and this repository.

- **Breaking:** Removed the `hash` function that only hashes on `u128` types.
- Added `rapidhash` and `rapidhash_seeded` functions to hash byte streams.
- Added `RapidHasher` and `RapidHasherInline` for hashing via a `std::hash::Hasher` compatible interface.
- Added `RapidHashMap`, `RapidInlineHashMap`, `RapidHashSet`, and `RapidInlineHashSet` helper types.
- Added `RapidHashBuilder` and `RapidInlineHashBuilder` for `std::hash::BuildHasher` implementing types compatible with `HashMap` and `HashSet`.
- Added `RapidRandomState` for random seed initialisation.
- Added `RapidRng`, `rapidrng_fast`, and `rapidrng_time` for random number generation inspired by the [wyhash crate](https://docs.rs/wyhash/latest/wyhash/) but based on `rapid_mix`.
- Added `std`, `rand`, `rng`, and `unsafe` features.
- Extensive benchmarking and optimisation.

## 0.1.0

Initial release by [Justin Bradford](https://github.com/jabr) supporting rapidhash on `u128` inputs.

- Added `hash` for rapidhashing `u128` types.
