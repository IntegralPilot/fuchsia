# testparser

Reviewed on: 2020-04-22

Parses stdout from various test frameworks into a common structured format.

This library is useful for instance for providing structured test results
on commit queue dashboards, or for performing more sophisticated data
on test results such as when identifying flakes or root-causing test
infrastructure failures.

Support for several common test frameworks and runtimes is provided:

*   C++ tests (via GoogleTest)
*   Rust tests (via rust-lang/libtest)
*   Go tests (via golang.org/pkg/testing)
*   Dart tests (via package:test)
*   Generic Test Runner Framework tests (agnostic of language & runtime)
*   Specialized Zircon unit testing framework
*   Specialized Vulkan CTS testing framework
*   Lacewing Python E2E testing framework (via Mobly)

This library is designed to be extensible and testable.
Adding support for new test frameworks is easy, simple, and fun!

## Building

`fx set core.x64 --with //tools/testing/testparser`

## Testing

```
fx set core.x64 --with-host //tools/testing/testparser:tests
fx test //tools/testing/testparser
```
