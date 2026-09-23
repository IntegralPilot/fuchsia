# TPM2 Software Stack (tss) Module
This module is intended to wrap all the unsafe calls to the lower level Rust
bindgen located in //third_party/tpm2-tss/rust/tpm2-tss-sys library. This is
a private module intended to be used by the parent //src/security/lib/tpm. The
library utilizes this safe wrapper to implement general purpose TPM commands.

## Safety
Safety must be derived from the underlying C code specified in
`//third_party/tpm2-tss:tss2-esys`. Memory allocation has been structured in
such a way that most allocations should occur directly in C which means they
also must be freed in C. Pointers should never be directly exposed from this
module.
