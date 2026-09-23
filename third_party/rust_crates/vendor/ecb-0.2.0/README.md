# RustCrypto: ECB

[![crate][crate-image]][crate-link]
[![Docs][docs-image]][docs-link]
![MIT licensed][license-image]
![Rust Version][rustc-image]
[![Build Status][build-image]][build-link]

Generic implementation of the [Electronic Codebook][ECB] (ECB) block cipher
mode of operation.

<img src="https://github.com/magic-akari/ecb/assets/7829098/2d92507b-a014-4769-8022-71802e6f8bd9" width="50%"><img src="https://github.com/magic-akari/ecb/assets/7829098/8596750d-0034-4a7f-bf28-4482c62a9f97" width="50%">

See [documentation][cipher-doc] of the `cipher` crate for additional information.

## Minimum Supported Rust Version

Rust **1.85** or higher.

Minimum supported Rust version can be changed in the future, but it will be
done with a minor version bump.

## SemVer Policy

- All on-by-default features of this library are covered by SemVer
- MSRV is considered exempt from SemVer as noted above

## License
 * [MIT license](http://opensource.org/licenses/MIT)

[//]: # (badges)

[crate-image]: https://img.shields.io/crates/v/ecb.svg
[crate-link]: https://crates.io/crates/ecb
[docs-image]: https://docs.rs/ecb/badge.svg
[docs-link]: https://docs.rs/ecb/
[license-image]: https://img.shields.io/badge/license-MIT-blue.svg
[rustc-image]: https://img.shields.io/badge/rustc-1.85+-blue.svg
[build-image]: https://github.com/magic-akari/ecb/actions/workflows/test.yml/badge.svg?branch=main
[build-link]: https://github.com/magic-akari/ecb/actions/workflows/test.yml

[//]: # (general links)

[ECB]: https://en.wikipedia.org/wiki/Block_cipher_mode_of_operation#ECB
[cipher-doc]: https://docs.rs/cipher/
