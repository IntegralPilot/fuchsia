# OWNERS

Given the vast breadth of the code under //src/lib/ and the project's desire to
encourage more local ownership, there is intentionally no //src/lib/OWNERS.
Rather, the addition of new subdirectories is expected to fall back toward
requiring an [OWNERS override][owners-override] in the following way.

The initial commit of //src/lib/foo should be limited to an OWNERS file, as well
as possibly a README.md. The override should be requested on this change, in
turn limiting the sign-off to the high-level intent of the new codebase and its
desired set of maintainers. Once those maintainers are in place, contributions
of actual code can proceed as usual under their direction alone.

[owners-override]: https://fuchsia.dev/fuchsia-src/development/source_code/owners?hl=en#owners_override