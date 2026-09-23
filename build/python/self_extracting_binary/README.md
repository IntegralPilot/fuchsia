# Self-extracting Python Lacewing test

Compile a host Rust binary that includes a Lacewing artifacts archive as its
data resource. When the Rust binary is run, it extracts the archive, then
executes the unpacked Python runtime with the appropriate args to run a
Lacewing test.
