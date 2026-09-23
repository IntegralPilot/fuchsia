# External Rust crates that are published from in-tree source

This directory contains crates which are published to `crates.io` from source
located within the Fuchsia tree. We always want to use the in-tree versions of
these crates instead of vendoring a copy of them. This directory should contain
symlinks to the directories within the Fuchsia tree that contain the sources for
these crates.
