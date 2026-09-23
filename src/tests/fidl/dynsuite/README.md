# FIDL dynsuite

The FIDL dynsuite tests clients and servers in FIDL bindings, ensuring they
behave according to the [bindings spec]. It is implemented as two independent
test suites: the [client_suite] and the [server_suite].

Tests are run by the **harness** component against a **runner** component.

Each suite has a single harness, and many runners: one for each binding flavor.
For example, "C++ natural" and "C++ wire" are separate runners.

Tests use two FIDL protocols: `Runner` and `Target`.
See [clientsuite.test.fidl] and [serversuite.test.fidl] for more details.

The tests are written in C++ using GoogleTest in
[client_suite/harness/tests] and [server_suite/harness/tests].

## Running the tests

    fx set core.x64 --with //bundles/fidl:tests
    fx test fidl_{client,server}_suite   # run all tests
    fx test fidl_server_suite_rust_test  # run the Rust server suite test

[bindings spec]: /docs/reference/fidl/language/bindings-spec.md
[client_suite]: client_suite/
[server_suite]: server_suite/
[client_suite/harness/tests]: client_suite/harness/tests/
[server_suite/harness/tests]: server_suite/harness/tests/
[clientsuite.test.fidl]: client_suite/fidl/clientsuite.test.fidl
[serversuite.test.fidl]: server_suite/fidl/serversuite.test.fidl
