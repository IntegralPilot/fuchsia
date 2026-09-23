# Mock-Omaha-Server

Updated: 2024-08

This is an implementation of a subset of the Omaha
[Omaha server protocol](https://github.com/google/omaha/blob/HEAD/doc/ServerProtocolV3.md)
which can be used to develop applications based on the
[omaha-client lib](https://github.com/google/omaha-client).

# Mode of operation

The mock-omaha-server serves as counterpart when testing applications based on the omaha
client library. It acts as a standalone http server, and responds to client requests depending
on the app ID in the request. A JSON structure can be supplied on the command line with the
`--responses_by_appid` argument when starting the mock server. This structure contains the
map of app ID to responses, for example:

```
{
    "appid_01": {
        "response": "NoUpdate",
        "merkle": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        "check_assertion": "UpdatesEnabled",
        "version": "0.1.2.3",
        "codebase": "fuchsia-pkg://omaha.mock.fuchsia.com/",
        "package_path": "update"
    },
    "appid_02": {
        ...
    },
    ...
}
```

A default argument `EXAMPLE_RESPONSES_BY_APPID` is supplied in [main.rs](src/main.rs), which
is used in case no map is supplied on the command line.

# Example session

The code is designed to work out-of-the-box with the "hello-world" example of the omaha-client lib.
Thus, when working with the source code after a simple git checkout of the
[omaha-client lib repository](https://github.com/google/omaha-client), the following session
illustrates how to work with the mock server:

```
$ git clone https://github.com/google/omaha-client.git
[... git clone progress ...]
$ cd omaha-client
$ cargo run
   Compiling omaha_client v0.2.0 (/path/to/omaha-client/omaha-client)
   Compiling mock-omaha-server v0.1.0 (/path/to/omaha-client/mock-omaha-server)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 14.59s
     Running `target/debug/mock-omaha-server`
listening on http://[::]:39205/
```

Note that, unless the port is specified with the `--port` switch, it will be picked by whatever
free port the lib gets from the operating system. At this point the mock omaha server is waiting
for requests, so now the hello-world example can be started **in a second terminal**, specifying
the URL of the mock server as printed in its output above:

```
$ cargo run --example hello-world -- -u http://[::]:39205
[...]
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.09s
     Running `target/debug/examples/hello-world -u 'http://[::]:39205'`
Event: ScheduleChange(
    UpdateCheckSchedule {
        last_update_time: None,
        next_update_time: 2024-08-05 09:23:41.527 UTC (1722849821.527518708) and No Monotonic wait: 100ms,
    },
)
[... after a few seconds ...]
Event: StateChange(
    CheckingForUpdates(
        ScheduledTask,
    ),
)
Event: OmahaServerResponse(
    Response {
        protocol_version: "3.0",

```
