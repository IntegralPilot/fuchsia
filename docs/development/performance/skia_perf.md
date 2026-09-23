# Using the Skia Perf performance dashboard for Fuchsia

Fuchsia uses [Skia
Perf](https://skia.googlesource.com/buildbot/+/main/perf/README.md) as
a dashboarding system for monitoring performance results. Skia Perf's
functionality includes:

*   A database for storing results from performance tests run in
    Continuous Integration (CI/postsubmit) builds.
*   An alerting system for detecting regressions and improvements in
    performance metrics (anomaly/changepoint detection) and filing
    Buganizer issues for regressions.
*   A web interface for viewing graphs of performance metrics.

You can view results for Fuchsia's public performance tests at
[fuchsia-perf.luci.app](https://fuchsia-perf.luci.app), Fuchsia's
public instance of Skia Perf.

Googlers can refer to the [Google-internal docs on using Skia Perf for
Fuchsia](https://goto.google.com/skia-perf-for-fuchsia).

Skia Perf is an open source project and has some [public
documentation](https://skia.googlesource.com/buildbot/+/main/perf/README.md)
in its [Git repository](https://skia.googlesource.com/buildbot/+/main/perf/).

Googlers can also refer to the [Google-internal docs for Skia
Perf](https://goto.google.com/skia-perf).

Note that the name Skia Perf comes from the fact that the system was
originally developed by the [Skia project](https://skia.org/) for
monitoring Skia performance. Skia Perf is now used by many other
projects besides Skia.

## Chromeperf migration

Fuchsia is in the process of switching from using the
[Chromeperf](https://chromeperf.appspot.com/) web UI as a performance
dashboard to using Skia Perf's web UI.

The resulting performance dashboard is effectively a hybrid of Skia
Perf and Chromeperf. Results data is still uploaded to Chromeperf's
database using Chromeperf's upload API, and we use Chromeperf's
regression detection and bug filing implementation, while Skia Perf's
web UI fetches results data from Chromeperf's database.

This means that parts of Fuchsia's codebase and docs still refer to
Chromeperf rather than Skia Perf.

Furthermore, Chromeperf is sometimes referred to as the "Catapult
performance dashboard" in the Fuchsia codebase, or just "Catapult" for
short, because its code lives in the [Catapult project Git
repo](https://chromium.googlesource.com/catapult/).

## See also

*   [Chromeperf/Skia Perf uploading and configuration](chromeperf_uploading_config.md)
