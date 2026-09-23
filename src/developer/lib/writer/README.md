# Writer

This crate contains code to perform I/O operations from command line programs.
This includes programs running in Fuchsia.

As a result, this code should be friendly to build and include using either
the host toolchain or the fuchsia toolchain.

Ffx subtools should not use this crate directly, but rather use
//src/developer/ffx/lib/writer which are host toolchain only, and include the
code for dependency injection via TryFromEnv.
