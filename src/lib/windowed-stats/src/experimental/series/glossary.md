# Glossary

This document describes the terminology and concepts of the [round-robin time
series](https://en.wikipedia.org/wiki/RRDtool) APIs in the `windowed-stats`
library. The primary mechanism provided by this API is the [time
matrix](#time-matrix). This is **not** an API reference, though some of these
concepts are represented very directly in APIs.

### Aggregation

An aggregation is the output of a [statistic](#statistic) computed from some
number of [samples](#sample). **This is not to be confused with a sample**: an
aggregation is the accumulation of one or more samples. Aggregations are pushed
into circular in-memory [buffers](#buffers) based on timing and the [sampling
intervals](#sampling-interval) of their associated [time matrix](#time-matrix).

Though an _aggregation_ is specifically the output of a _statistic_, these terms
are sometimes used interchangeably.

### Buffer

A buffer is a bounded [ring
buffer](https://en.wikipedia.org/wiki/Circular_buffer) that stores and evicts
the [aggregations](#aggregation) of a [time series](#time-series) as
[samples](#sample) are folded or [interpolated](#interpolation). Buffers provide
a [FIFO
cache](https://en.wikipedia.org/wiki/Cache_replacement_policies#First_in_first_out(FIFO))
of aggregations: as capacity is exhausted, the oldest aggregations are
overwritten by the newest.

Whenever possible, buffers compress aggregations to reduce memory usage. Buffers
have an **approximate** [capacity](#capacity) based on thresholds on either
sample count or memory usage, because compression cannot guarantee precise
counts nor usage. Compression is deployed based on the [statistic](#statistic)
and aggregation of a time series. Uncompressed buffers are used when no suitable
compression technique is available for aggregation data, such as IEEE
floating-point data.

The buffers of a [time matrix](#time-matrix) can be queried and serialized for
later analysis. A serialized buffer contains both time series data and metadata
for downstream tooling, such as labels for bitsets and the [data
semantic](#data-semantic).

### Capacity

A capacity defines the bounds on **either** the number of
[aggregations](#aggregation) **or** amount of memory supported by a
[buffer](#buffer). Capacity affects the [durability](#durability) and memory
usage of its buffered [time series](#time-series). Capacity is specified as part
of a [sampling interval](#sampling-interval).

When expressed in terms of aggregations, capacity establishes a **lower bound**.
This is the minimum number of sampling intervals that a buffer must persist in
memory before evicting data (and so is effectively the minimum number of
aggregations, because a buffer stores an aggregation per interval).

When expressed in terms of memory, capacity establishes an **upper bound**. This
is the maximum amount of memory that a buffer may allocate before evicting data.

Note that buffers cannot generally guarantee an exact number of aggregations nor
an exact amount of memory, because compression and lower memory usage are
preferred over precise thresholds. The number of aggregations or bytes retained
may vary depending on the compression applied to the data, which is not
configurable.

### Data Semantic

TBD.

### Durability

Durability describes the bounds on how far in the **past** that
[samples](#sample) are represented in some data. High durability data represents
samples from the far past while low durability data only represents more recent
samples.

High durability [sampling intervals](#sampling-interval) must sacrifice memory
and/or [granularity](#granularity).

### Granularity

Granularity describes the bounds on the duration of [sampling
intervals](#sampling-intervals) and [periods](#sampling-periods). High
granularity data is collected over shorter intervals and represents more precise
state while low granularity data is collected over longer intervals and is less
precise.

High granularity sampling intervals must sacrifice memory and/or
[durability](#durability).

Granularity is also known as _resolution_.

### Interpolation

TBD.

### Sample

A sample is atomic numeric data associated with an event, state, system, etc. of
interest. Samples are the unit data ingested by [statistics](#statistics) to
ultimately produce [time matrices](#time-matrix). Note that samples are never
persisted in memory nor storage, but are instead folded into
[aggregations](#aggregation).

### Sampler

A sampler is an arbitrary function over [samples](#sample) with some arbitrary
state. A sampler is said to _fold_ samples into its state or simply _to sample_.
The state of a sampler typically changes as it folds. For example, [the
`ArithmeticMean` type](#arithmeticmean) is a sampler that folds samples into a
sum and a count to compute an arithmetic mean.

### Sampling Interval

A sampling interval is a duration in which [samples](#sample) are
[folded](#statistic) into an [aggregation](#aggregation). Sampling intervals
determine the timing of aggregations and [interpolation](#interpolation) in
[buffered](#buffer) [time series](#time-series) and are defined by the following
quantities:

1. A maximum [sampling period](#sampling-period): the maximum duration in which
   a sample must be observed. This subdivides the sampling interval and
   determines how frequently [interpolation](#interpolation) may occur within an
   interval.
1. A [sampling period](#sampling-period) count: the (non-zero) number of
   sampling periods that form the sampling interval. This determines the minimum
   number of samples (or interpolations) folded into the aggregation for a
   sampling interval.
1. A [capacity](#capacity): the bound on either the number of sampling intervals
   or amount of memory in an associated buffer.

These quantities are notated in the **reverse order** in which they appear
above, delimited by "x". For example, 10x2x5s notates capacity for at least 10
intervals, each formed from two 5s sampling periods (each interval spans 10s).
When capacity is an upper bound on memory, it is notated with units of data,
such as 1MBx2x5s for no more than one Megabyte.

Capacity is an extrinsic quantity: it does not describe the sampling interval
itself, but instead how much data to retain in buffers. A sampling interval is
illustrated below in relation to a buffer and its capacity, count, and period:

```
  | Buffer                                                      |
  |---------------------------+-----+---------------------------|
  | Interval 1                | ... | Interval N                | x Capacity
  |----------+-----+----------|-----|----------+-----+----------|
  | Period 1 | ... | Period M | ... | Period 1 | ... | Period M | x Count x Capacity

  }- Period -{

t -------------------------------------------------------------->
  |                                                             |
  0                                            modulo Period x Count x Capacity
```

### Sampling Period

A sampling period is the basic unit of time that defines a [sampling
interval](#sampling-interval) and represents the maximum duration in which a
sample **must** be observed. For any such duration in which no sample is
observed, an [interpolated sample](#interpolation) is used instead.

This can also be conceptualized as its inverse: the _minimum sampling
frequency_.

### Sampling Profile

A sampling profile is a collection of one or more [sampling
intervals](#sampling-interval) used to define a [time matrix](#time-matrix) over
those intervals. The library provides named sampling profiles that align well
with each other to ease analysis.

### Statistic

A statistic is a [sampler](#sampler) that folds [samples](#sample) into a
statistical [aggregation](#aggregation). This is essentially a statistical
function over samples, such as a sum or arithmetic mean.

Though an _aggregation_ is specifically the output of a _statistic_, these terms
are sometimes used interchangeably.

### Timestamp

TBD.

### Time Matrix

A time matrix [computes](#statistic) and [buffers](#buffer) statistical data
over [samples](#sample) with varying temporal [granularity](#granularity) and
[durability](#durability) over time per a [sampling profile](#sampling-profile).
Put another way, a time matrix is **one or more** buffered [time
series](#time-series). **The time matrix is the primary mechanism provided by
these APIs.**

A time matrix is a [sampler](#sampler) over **timed data**: each of its samples
is composed with a [timestamp](#timestamp) that determines how its data is
[aggregated](#statistic), how samples are [interpolated](#interpolation), and
when [aggregations](#aggregation) are pushed into [buffers](#buffer).
Additionally, a time matrix can be queried for its buffers at a given time via a
timestamp, which may interpolate samples before producing serialized buffers
that can be logged for later analysis.

The one or more time series in a time matrix (defined by the [sampling
intervals](#sampling-interval) in the given sampling profile) differ only in
their associated sampling intervals; the [statistic](#statistic),
[interpolation](#interpolation), etc. are the same for all series in a matrix.
The buffered aggregations always describe the same data.

A time matrix is also know as a _multi-resolution time series_ or _round-robin
time series_ and is a type of [_data
cube_](https://en.wikipedia.org/wiki/Data_cube) (hence the term _matrix_).

### Time Series

A time series is the composition of a [statistic](#statistic),
[interpolation](#interpolation), and [sampling interval](#sampling-interval)
that describes how and when to fold and aggregate samples over time.

**There is no explicit API for time series**: instead, a [time
matrix](#time-matrix) is constructed from the components above and one or more
sampling intervals that describe each time series in the matrix.
