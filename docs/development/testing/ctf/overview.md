# Compatibility Tests for Fuchsia

Note: Compatibility testing documents are being overhauled in Q2
2024.  Some information in this document is out of date. See
[this ticket][bug-ticket-01]{:.external} for details.

Compatibility Testing for Fuchsia is a mechanism to freeze artifacts on release
branches and then thaw those artifacts for testing in CI/CQ.

This is commonly used to detect compatibility changes between the Fuchsia
platform surface represented by the release branch and that represented by
the current state of the Fuchsia source repository.

To learn how it works, and to get started on adding CTF tests for
your area, please see the links below.

## Getting started

* Read the [user guide] for the latest guidance on using CTF.

## Contributing to CTF

* To learn how to add a test to CTF, read the [contributing guide].
* For examples, see the [Rust echo test], the [C++ echo test] and the complete
  set of tests in [//sdk/ctf/tests][all tests].

## Documentation

* For the background, motivation and goals of CTF, see the [CTF overview].
* For frequently asked questions, see the [FAQ].

### RFCs

* [RFC-0141]: CTF Process
* [RFC-0015]: Compatibility Tests for Fuchsia (CTF).

## Contact

* Email: <fuchsia-ctf-team@google.com>
* Bugs: <https://issuetracker.google.com/components/1375729>

<!-- Links. Please link source code to https://cs.opensource.google -->

[all tests]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/ctf/tests
[contributing guide]: /docs/development/testing/ctf/contributing_tests.md
[CTF overview]: /docs/development/testing/ctf/compatibility_testing.md
[FAQ]: /docs/development/testing/ctf/faq.md
[RFC-0015]: /docs/contribute/governance/rfcs/0015_cts.md
[RFC-0141]: /docs/contribute/governance/rfcs/0141_cts_process.md
[user guide]: /docs/development/testing/ctf/user_guide.md
[C++ echo test]: /sdk/ctf/tests/examples/fidl/fuchsia.examples/cc/
[Rust echo test]: /sdk/ctf/tests/examples/fidl/fuchsia.examples/rust/
[bug-ticket-01]: https://fxbug.dev/339893257
