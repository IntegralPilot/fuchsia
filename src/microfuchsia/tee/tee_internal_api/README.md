# TEE Internal Core API

include/ contains the header files for the TEE Internal Core API v1.2.1.

* src/ contains the implementation of the API. It is currently a shared library,
although it might not stay that way.

* src/binding.rs is the generated Rust definitions based on the header files. It
is manually updated by running bindgen.sh and checking in the results.

* Richer, more idiomatic, more ergonomic, layout-compatible bindings are
  exported top-level within the tee_internal_api crate. Groups of constants that
  are morally enums and bitflags are defined as Rust enums and bitflags,
  respectively; TEE_ERROR_* constants are made to comprise a proper error type
  implementing std::error:Error; further, each
  tee_internal_api::binding::TEE_Foo type is made to correspond to a
  tee_internal_api::Foo enriched with those enum/bitflag abstraction when
  possible and stripped of FFI awkwardness. APIs that wrap this crate should
  opt to to use the tee_internal_api::Foo definitions over the
  tee_internal_api::TEE_Foo ones (except of course the implementations of the
  extern C API).
