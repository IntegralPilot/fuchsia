# Power Framework Testing Client

## About

This directory contains a C++ client library that can be used by hermetic tests
(eg. driver unit tests) to access protocols provided from a hermetic instance of
the power framework. This includes the fake system-activity-governor, a power broker,
and a fake suspend HAL with test controls exposed.

## How to use

From the test target, depend on the gn target at `//src/power/testing/client/cpp`. This provides
a header that can be include with `#include "src/power/testing/client/cpp/client.h"`. This header
provides namespace functions inside the `test_client` namespace that can be used to connect
to the various protocols provided by the various power framework pieces.

The library also enforces that the test's component manifest contain the corresponding client
shard. This can be done like:

```cml
    include: [
        "//src/power/testing/client/meta/client.shard.cml",
    ],
```

## Tests

There is a unit test for the library at `//src/power/testing/client:tests`. This can be run with
`fx test power_testing_client_test` after including the test target in the build.
