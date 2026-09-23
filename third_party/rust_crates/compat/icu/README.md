# rust_icu_* crate aliases

We keep these aliases so that future crate version updates do not need to touch
code outside of `//third_party/rust_crates`.

The aliases here point to specific versioned targets in
`//third_party/rust_crates/BUILD.gn`. When updating the `rust_icu_*` crates
to a new version, update the references in this directory's `BUILD.gn` file
instead of searching and replacing throughout the entire codebase.
