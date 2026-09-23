# dupefinder

Produces a report on duplicated heap allocations and likely source locations.

To start, collect a heap snapshot (see http://go/fuchsia-heapdump for details):

```
ffx profile heapdump snapshot \
    --output-file /path/to/symbolized.pb.gz \
    --output-contents-dir /path/to/contents \
    --by-name instrumented-process.cm
    --symbolize
```

Note that the snapshot must be symbolized to get meaningful results from this
tool.

Pass the collected snapshot to dupefinder:

```
fx dupefinder \
    --profile /path/to/symbolized.pb.gz \
    --contents-dir /path/to/contents \
    --html-output /path/to/dupes_report.html
```

The report has some basic configuration options, see `fx dupefinder --help`.
