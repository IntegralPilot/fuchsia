# Fuchsia TPM Library
This library provides a general purpose high level interface to Trusted
Platform Module 2.0 devices for the Fuchsia operating system. The intention
of this library is to abstract low level details of TPM communication away
from the user and provide a difficult to misuse interface that they can
interact with. The library comes in two variants:

## Library Targets
* **tpm-agent**: This target is intended to be general purpose and used by any
  component with access to the protocol `fuchsia.tpm.Command`. Users should
  be able to build hardware backed key storage systems, attestation system
  etc. from this library.
* **tpm-device**: This target is intended for implementors of the
  `fuchsia.tpm` library itself; see `//src/security/lib/fuchsia-tpm-protocol`
   for a usage of this library. This isn't intended for general use and is only
   used in the internal implementation of the TPM stack itself. It targets
   the `fuchsia.tpm.TpmDevice` protocol.

## Library Structure
Internally this library is built on-top of the  Trusted Computing Group's
(TCG) TPM2 Software Stack. This library is written in C and is located at:
`//third_party/tpm2-tss`. We have built a rust layer using bindgen located at:
`//third_party/tpm2-tss/rust/tpm2-tss-sys`. Since this library is just a raw
binding wrapper on top of the C code the `tss` submodule in this crate is
responsible for providing a safe wrapper around this library and exposing
it to the library for use in building the higher level API.
