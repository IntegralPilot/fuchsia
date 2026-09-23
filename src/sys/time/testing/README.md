# Timekeeper Test Realm Factory (TTRF)

The TTRF is an implementation of the [Test Realm Factory (TRF)][trf]
pattern, specifically for the [Timekeeper][tk] subsystem.  It is used
for making hermetic integration tests that interact with various Fuchsia
subsystems.

While there are many ways to implement hermetic integration testing support, we
settled on the TRF as a pattern, and are converting existing integration and
other tests to conform to the TRF patterns.**  This testing approach
standardizes the test fixture expectations, but also allow us to mix and match
test fixture versions to verify ABI guarantees.

## Before we begin: make sure that you use the right tool for the job

While TTRF ensures high fidelity of the Timekeeper subsystem in tests, it may be
a too heavyweight test fixture in case your testing needs are more focused than
"use everything that Timekeeper needs at testing time".

Specifically:

* If your testing needs only require access to a fake *monotonic* clock,
  not a fake UTC clock or real-time clock, you may be better served by the
  `fake-clock` library.

  NOTE: if a library you depend on needs a UTC clock, then so does your test.

* If you have a single-process unit test that should run in fake time, you may
  be better served by [`TestExecutor`][te]. Study the documentation carefully
  if you decide to use it, since timer wake behavior in fake time is subtle.

[te]: https://fuchsia-docs.firebaseapp.com/rust/fuchsia_async/struct.TestExecutor.html#method.new_with_fake_time

## Overview

TTRF spins up a hermetic instance of Timekeeper, optionally connected
to a fake [boot clock][mclk], a fake [UTC clock][utc] handle, a fake [Real
Time Clock (RTC)][rtc] service, and a real Timekeeper machinery. This setup
is housed in a hermetic realm, which is constructed by a Test Realm Factory
component.  The realm factory component for the Timekeeper test realm is
[here][ttrf].

The user of the TTRF component should ideally not concern themselves with TTRF
implementation details, but should rather start the test realm factory
component and use the APIs it offers to retrieve the FIDL APIs that it offers
for testing.

The TTRF has configuration options which can be passed in at realm creation
time. For most part, the author of a TTRF client test can ignore the options
that do not apply to their test.

## Example use

An example use of the TTRF can be found in the [examples directory][ex]. Below
is the run-down of the interesting points in that setup.

Start by establishing connection with the realm factory. Connecting to this
proxy gets us to all services that are started by the Timekeeper Test Realm
Factory (TTRF).

```
let ttr_proxy = client::connect_to_protocol::<fttr::RealmFactoryMarker>()
    .with_context(|| {
        format!(
            "while connecting to: {}",
            <fttr::RealmFactoryMarker as fidl::endpoints::ProtocolMarker>::DEBUG_NAME
        )
    })
    .expect("should be able to connect to the realm factory");
```

Next, we create UTC clock object that timekeeper will manage.

We give it an arbitrary backstop time. On real systems the backstop
time is generated based on the timestamp of the last change that
made it into the release.

```
let utc_clock = zx::Clock::create(zx::ClockOpts::empty(), Some(*BACKSTOP_TIME))
    .expect("zx calls should not fail");
```


RealmProxy endpoint is useful for connecting to any protocols in the test
realm, by name.

```
let (_rp_keepalive, rp_server_end) = endpoints::create_endpoints::<ftth::RealmProxy_Marker>();
```

`_rp_keepalive needs` to be held to ensure that the realm continues is kept
alive during the test runtime. We can also use this client end to request any
protocol that is available in TTRF by its name. In this test case, however, we
don't do any of that.

We duplicate the UTC clock object to pass it into the test realm while also
keeping a reference for this test.

```
let (push_source_puppet_client_end, _ignore_opts, _ignore_cobalt) = ttr_proxy
    .create_realm(
        fttr::RealmOptions { use_real_reference_clock: Some(true), ..Default::default() },
        utc_clock.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("duplicated"),
        rp_server_end,
    )
    .await
    .expect("FIDL protocol error")
    .expect("Error value returned from the call");
```

Sampling the boot clock directly is OK since we configured the timekeeper
test realm to use the real boot clock. Convert to a proxy so we can send
RPCs.

Now we tell Timekeeper to set a UTC time sample. We do this by establishing a
correspondence between a reading of the boot clock, and the reading of the
UTC clock. We then also provide a standard deviation of the estimation error.
The "push source puppet" is an endpoint that allows us to inject "fake"
readings of the time source.

```
let sample_reference = zx::BootInstant::get();
let push_source_puppet = push_source_puppet_client_end.into_proxy().expect("infallible");
```

When we inject the time sample as shown below, Timekeeper will see that as if
the time source provided a time sample, and will adjust all clocks accordingly.

```
const STD_DEV: zx::Duration = zx::Duration::from_millis(50);
push_source_puppet
    .set_sample(&TimeSample {
        utc: Some(VALID_TIME.into_nanos()),
        reference: Some(sample_reference.into_nanos()),
        standard_deviation: Some(STD_DEV.into_nanos()),
        ..Default::default()
    })
    .await
    .expect("FIDL call succeeds");
```

We can now wait until the UTC clock is started. The snippet below shows the
canonical way to do that.

The signal below is a direct consequence of the `set_sample` call above. The
above call is synchronized because it goes to a separate process. We can
therefore write this test in a sequential manner.

```
fasync::OnSignals::new(
    &utc_clock,
    zx::Signals::from_bits(fft::SIGNAL_UTC_CLOCK_LOGGING_QUALITY).unwrap(),
)
.await
.expect("wait on signal is a success");
}
```

Here is the manifest file from the example.

```
{
    include: [
        "//sdk/lib/sys/component/realm_builder.shard.cml",
        "//src/lib/fake-clock/lib/client.shard.cml",
        "//src/sys/test_runners/rust/default.shard.cml",
        "inspect/client.shard.cml",
        "syslog/client.shard.cml",
    ],
    program: {
        binary: "bin/src_sys_time_example",
    },
    use: [
        {
            protocol: [ "test.time.realm.RealmFactory" ],
            from: "parent",
        },
        {
            protocol: [ "fuchsia.testing.FakeClockControl" ],
            from: "parent",
        },
    ],
    offer: [
        {
            protocol: [
                "fuchsia.metrics.MetricEventLoggerFactory",
                "fuchsia.time.external.PushSource",
            ],
            from: "parent",
            to: "#realm_builder",
        },
    ],
}
```

[ex]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/time/testing/
[fcl]: /src/lib/fake-clock
[mclk]: https://fuchsia.dev/fuchsia-src/concepts/kernel/time/monotonic
[rtc]: https://fuchsia.dev/reference/fidl/fuchsia.hardware.rtc
[tk]: /src/sys/time
[trf]: https://fuchsia.dev/fuchsia-src/development/testing/components/test_realm_factory
[trfc]: /src/sys/time/testing/realm-proxy
[utc]: https://fuchsia.dev/fuchsia-src/concepts/kernel/time/utc/overview
