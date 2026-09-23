# Ring Buffer Behavior

Ring buffers are used to convey audio between parties (usually in different
processes), allowing concurrent, asynchronous data access without requiring
locks. This pattern works because both parties share an understanding of which
buffer areas are safe to access, and how those areas change over time.

The descriptions below explain how audio data frames are moved from one party to
another. As two examples, we detail (1) the movement of audio from a *client* to
a *driver* (playback to that driver's hardware), as well as (2) the movement of
audio from a *driver* to a *client* (recording from that driver's hardware).
However, ring buffers can be used to convey audio between two app-level clients
or between two drivers, as well.

Some audio hardware can transfer data to/from system memory *without*
involvement from software (e.g. the host-based driver); other audio hardware
accomplishes this with software running on the hardware itself (e.g. DSP
firmware). Nonetheless, henceforth for consistency we will refer to this
movement of audio data as being done by the 'driver'. For details about this
distinction, see the 'Hardware versus Software' section toward the end of this
document.

Ring buffer users avoid the need for mutexes or other active synchronization
because they share three important pieces of information. The first is the
*memory bounds* of the ring buffer itself. The second is the rate at which the
audio must be produced and consumed; this rate is defined by the *format*
specified in the `CreateRingBuffer` command. Third, the two parties must share
an understanding of the ring buffer's *start time*.

While the ring buffer is not started, time has no effect on the ring buffer's
state. While the ring buffer is started, there is a *ring buffer position* that
continually moves across the ring buffer at a constant speed set by the
predefined format. By definition, at 'start time', this ring buffer position 'R'
begins at the beginning of the ring buffer, at frame 0.

While the ring buffer is started, the driver is constrained in the size of data
transfers that it can make in a single I/O operation. For playback (where the
driver is Consumer, and the client is Producer), audio frames are consumed by
the driver in transfers that can be as large as `driver_transfer_bytes`. For
capture (where the driver is Producer, and the client is Consumer), audio frames
are produced by the driver in transfers that can be as large as
`driver_transfer_bytes`.

These driver data transfers mean that there is always a section of the ring
buffer that is unsafe for the client to be writing (or reading, if the ring
buffer is being used for capture). This unsafe buffer region is defined on one
side by the current ring buffer position 'R', and on the other side by a 'safe
pointer' location. Depending on whether the ring buffer is being used for
playback or capture, this is either "the safe frame location for a Producer to
write" ('P') or "the safe frame location for a Consumer to read" ('C'),
respectively. The diagrams below label these pointers as 'R', 'P', 'C'.

For playback, the region between 'R' and 'P' must not be written by the client
at that time. For capture, the region between 'C' and 'R' must not be read
during that time.

Once a ring buffer starts, these pointers begin moving at a fixed rate. 'R'
begins moving at the `start_time` returned from `RingBuffer::Start`, from lower
addresses to higher addresses, and instantaneously restarting at the beginning
of the ring buffer when reaching its end. This movement of 'R', 'P' and 'C'
enables a Consumer to safely read ring buffer contents that were previously
written by a Producer, after an appropriate time duration has passed.

To pass audio through the ring buffer, the Producer must write data BEFORE the
Consumer transfers occur. Conversely the Consumer must read data AFTER the
Producer transfers occur. For this reason, 'P' (which we define during playback)
is always ahead of 'R', whereas 'C' (which we define during capture) is always
behind 'R'. Restated, 'P' refers to a frame that is earlier than the frame
referred to by 'R', and 'C' refers to a frame that is later than the frame
referred to by 'R'. A given frame would be referred to by 'P' before it is
referred to by 'R', before it is referred to by 'C'. In the diagrams below, ring
buffer frame 0 is at the left, and ring buffer positions move from left to
right. Therefore when looking from left to right, we expect the diagrams to show
'C' then 'R' then 'P' (modulo the effects of wraparound).

## Playback

Before starting the ring buffer, playback clients may safely write any ring
buffer location. For this reason, 'P' is not yet defined.

```
                                 Ring Buffer
+-----------------------------------------------------------------------------+
[<--                             safe to write                             -->)
[             (to pre-populate the ring buffer before starting it)            )
+-----------------------------------------------------------------------------+
0=R                                                                           0
```

For audio to be played as soon as possible, the client should write their first
frames of audio where they will be the first thing the driver reads when the
ring buffer starts: the beginning of the ring buffer. If a client has sufficient
audio available (perhaps the entire audio file), it may choose to pre-populate
the whole ring buffer before starting it. Other clients receive audio as a
real-time stream; those clients can still pre-populate audio to the beginning of
the ring buffer but *must* write more than `driver_transfer_bytes` of audio,
since upon `Start` the driver may immediately consume that much data from the
ring buffer. This means that the client must continue writing audio from that
frame (labelled 's' below) *before* the driver reads it (before 'P' reaches it).

```
                               Ring Buffer
+-------------------------+---------+-----------------------------------------+
[<-- Pre-populated by the client -->)      not yet written by the client      )
[< driver_transfer_bytes >)                                                   )
+-------------------------+---------+-----------------------------------------+
0=R                                 s                                         0
```

If the client *cannot* pre-populate enough audio, then they should start their
audio at an offset, rather than the beginning of the ring buffer. This relies on
the zeroed-out contents of the VMO to be the first audio read by the driver. As
above, this offset (again called 's') must be sufficient for the client to
provide *subsequent* audio frames before the driver consumes them. For example:

```
                               Ring Buffer
+-------------------------+---------+-----------------------------------------+
[    Offset   [<-- Pre-populated -->)      not yet written by the client      )
[< driver_transfer_bytes >)                                                   )
+-------------------------+---------+-----------------------------------------+
0=R                                 s                                         0
```

Once the ring buffer is started, it is not safe for the client to write data to
the ring buffer between 'R' and 'P', because this represents data already in use
(potentially already consumed by the driver). The client may safely write the
rest of the ring buffer (between 'P' and '0/R').

As always, the client should never write *too* close to 'P', as it is an
instantaneous hypothetical pointer which could advance during the delay of even
a single CPU instruction. The *effective* 'safe to write' region for a client is
always changing, as 'P' is constantly moving. For this reason, a client should
write _ahead_ (at a higher memory address) such that it always has enough time
to write more data ahead of 'P'.

This is the state of the playback ring buffer, at the moment it is started:

```
                               Ring Buffer
+-------------------------+---------------------------------------------------+
[<--  unsafe to write  -->[<--           safe to write (not yet            -->)
[< driver_transfer_bytes >[              consumed by the driver)              )
+-------------------------+---------------------------------------------------+
0=R                       P                                                   0
```

As time passes, the driver reads data in chunks of `driver_transfer_bytes` or
less, at the rate specified in `CreateRingBuffer`. Many drivers use a 'ping
pong' pattern where they read half of their allocated ring buffer region at a
time, to allow time for these reads to occur safely. Regardless of the size of
the driver transfers, the Position and Safe pointers ('R' and 'P') move to the
right at the same rate, but do so smoothly. As a result, the "unsafe for client
writes" area moves gradually through the ring buffer, while maintaining a
constant size equal to `driver_transfer_bytes`. Thus, after some period we now
have:

```
                               Ring Buffer
+------------+-------------------------+--------------------------------------+
[<-- safe -->[<--  unsafe to write  -->[<--     safe to write (not yet     -->)
[  to write  [< driver_transfer_bytes >[        consumed by the driver)       )
+------------+-------------------------+--------------------------------------+
0            R                         P                                      0
```

Later, 'P' wraps around the ring buffer before 'R' does. Note that the region
from 0 to 'P', plus the region from 'R' to the end of the ring buffer, adds up
to `driver_transfer_bytes`:

```
                               Ring Buffer
+---------------+--------------------------------------------------+----------+
[<--  unsafe -->[<--        safe to write (to overwrite         -->[<-unsafe->)
[ransfer_bytes >[              already-consumed data               [< driver_t)
+---------------+--------------------------------------------------+----------+
0               P                                                  R          0
```

In steady state, i.e. once the process has wrapped around the ring buffer, any
frame at or greater than 'P' (up to a limit of 'R + ring_buffer_size') is safe
for the client to write. Restated, and factoring in ring buffer wraparound, the
Producer can safely write either the ranges [0, R) + [P, ring_buffer_size), or
alternately range [P, R), depending on where 'R' lies relative to the ring
buffer wraparound point -- either the above diagram, or (more frequently) this
one:

```
                               Ring Buffer
+--------------------------+-------------------------+------------------------+
[<--   safe to write    -->[<--  unsafe to write  -->[<--   safe to write  -->)
[                          [< driver_transfer_bytes >[                        )
+--------------------------+-------------------------+------------------------+
0                          R                         P                        0
```

Note the boundary requirements: the "unsafe for Producer to write" region is [R,
P), so a Producer *cannot* safely write location 'R' (which is equivalent to 'R
+ ring_buffer_size', the producer high-water location). Similarly, the "safe for
Producer to write" region is [P, R) (with wraparound), so a Consumer *cannot*
safely read location 'P'.

But in practice, that precise frame is not safe for *either party* to access.
Frame pointer locations 'P' and 'R' are theoretical and instantaneous. By the
time the driver reads from 'R', that pointer will have slightly moved, rendering
that location unsafe for reads; by the time the client writes to 'P', that
pointer will have slightly moved, rendering that location unsafe for writes. The
Producer and Consumer must always maintain a level of safety padding ahead of
their "safe" pointer locations.

The `driver_transfer_bytes` value specified by a driver is critical for ensuring
that clients do not write into memory that the driver is still actively reading.
With the 'ping pong' pattern mentioned above, a driver would specify a value for
`driver_transfer_bytes` that is twice the size of the actual transfers
themselves. Indeed it would reflect the size of the internal double-buffer that
provides the extra duration of safety padding.

## Recording

While recording, it is only safe for the client to read the part of the ring
buffer that is not simultaneously being written by the driver. Before capture
begins, the driver has not yet written anything for the client to read.

At the instant that capture starts (reported by `RingBuffer::Start`), the driver
cannot immediately transfer frames to the ring buffer, because these frames have
not yet been acquired. The driver must first accumulate enough frames to make a
transfer, and only thereafter would move that amount to the ring buffer starting
at frame '0'. Many drivers use a double-buffer (or 'ping pong') pattern where
they transfer half of their buffering amount in each transfer. Since at the
moment of 'Start' no audio frames are yet available for the client to read, 'C'
is effectively undefined. However it will be helpful to think of a position 'b'
(which will become 'C'). This 'b' lags frame 'R' by a fixed offset and has not
yet reached frame location 0. Here is the ring buffer state when it is started:

```
                               Ring Buffer
+---------------------------------------------------+-------------------------+
[<--         safe to read (but empty, not yet written by driver)           -->)
[                                                   [< driver_transfer_bytes >)
+---------------------------------------------------+-------------------------+
0=R                                                 b                         0
```

After the ring buffer is started but before 'R' has advanced by
`driver_transfer_bytes`, the client cannot yet safely read ANY newly captured
frames, because they may not have yet been transferred into the ring buffer.
Although 'R' is advancing, the driver may or may not have made any transfers
into the buffer yet. With the 'ping pong' pattern, the driver waits until the
first half of its internal buffer is full before transferring its contents into
the ring buffer -- and while this transfer occurs, the other half of the
internal buffer remains available to safely receive subsequent frames.

The amount of audio that has actually been captured into the ring buffer will
change with each driver transfer, so it moves across the ring buffer in a
"chunky" way. By contrast, 'R' and 'C' will by definition move in a perfectly
smooth manner; they are guaranteed to *always* bound where the actual
most-recently-captured frame lies.

At this time, because 'R' has not yet advanced by `driver_transfer_bytes`, 'C'
is still effectively undefined. Our marker 'b' continues to advance, lagging 'R'
by a fixed offset and still not yet reaching 0:

```
                               Ring Buffer
+--------------+--------------------------------------------------+-----------+
[<-- unsafe -->[<--           empty, not yet written by driver             -->)
[ansfer_bytes >)                                                  [< driver_tr)
+--------------+--------------------------------------------------+-----------+
0              R                                                  b           0
```

Once the ring buffer position 'R' has advanced by *exactly*
`driver_transfer_bytes`, the driver is guaranteed to have made at least the
initial transfer of audio frames into the ring buffer. With the 'ping pong'
pattern, the driver will have already transferred its first-half ('ping') buffer
into the ring buffer some time ago, and its second-half ('pong') buffer will
have just been filled and can now be written to the ring buffer. Location 'b'
has reached the beginning of the ring buffer, so 'C' is now defined and begins
to smoothly advance at the same rate as 'R' (as determined by the ring buffer's
frame rate and sample format). So at this instant we have:

```
                               Ring Buffer
+-------------------------+---------------------------------------------------+
[<--       unsafe      -->[<--       empty, not yet written by driver      -->)
[< driver_transfer_bytes >[                                                   )
+-------------------------+---------------------------------------------------+
0=C                       R                                                 b=0
```

As the ring buffer position 'R' advances further, the client can safely read
frames in the region between '0' and 'C'. It is unsafe for the client to read
data from 'C' up to 'R', because this is where the driver is simultaneously
writing. This region progresses across the ring buffer, maintaining a constant
size of `driver_transfer_bytes`. Conceptually the ring buffer is now in this
state:

```
                               Ring Buffer
+--------------------+-------------------------+------------------------------+
[<   safe to read   >[<--  unsafe to read   -->[<--     empty, not yet     -->)
[newly-captured audio[< driver_transfer_bytes >[       written by driver      )
+--------------------+-------------------------+------------------------------+
0                    C                         R                              0
```

Later, 'R' wraps around the ring buffer before 'C' does. Note that the region
from 0 to 'R', plus the region from 'C' to the end of the ring buffer, adds up
to `driver_transfer_bytes`.

As always, the client should never read *too* close to 'R', as it is an
instantaneous hypothetical pointer which could advance during the delay of even
a single CPU instruction. The *effective* 'safe to read' region for a client is
always changing, as 'R' is constantly moving. For this reason, a client should
read _ahead_ (at a higher memory address) such that it always has enough time to
read more data ahead of 'R'.

This is the state of the ring buffer, at some time after its first wraparound:

```
                               Ring Buffer
+-----------+--------------------------------------------------+--------------+
[<--unsafe->[<--                safe to read                -->[<-- unsafe -->)
[fer_bytes >[                 (captured audio)                 [< driver_trans)
+-----------+--------------------------------------------------+--------------+
0           R                                                  C              0
```

In steady state, i.e. once the process has wrapped around the ring buffer, any
frame less 'C' (up to the limit of 'R - ring_buffer_size') is safe for the
client to read. Restated, and factoring in ring wraparound, the Consumer can
safely read either ranges [0, C) + [R, ring_buffer_size), or alternately range
[R, C) -- depending on where 'R' lies relative to the ring wraparound point --
either the diagram above, or (more frequently) this one:

```
                               Ring Buffer
+--------------------------+-------------------------+------------------------+
[<--    safe to read    -->[<--      unsafe       -->[<--   safe to read   -->)
[                          [< driver_transfer_bytes >[                        )
+--------------------------+-------------------------+------------------------+
0                          C                         R                        0
```

Note the boundary requirements: the "unsafe for Consumer to read" region is [C,
R), so a Consumer *cannot* safely read location 'C' (the Consumer low-water
frame location). Similarly, the "safe for Consumer to read" region is [R, C)
(with wraparound), so a Producer *cannot* safely write location 'R'.

But in practice, that precise frame is not safe for *either party* to access.
Frame pointer locations 'R' and 'C' are theoretical and instantaneous. By the
time the driver writes to 'C', that pointer will have slightly moved, rendering
that location unsafe for writes; by the time the client reads from 'R', that
pointer will have slightly moved, rendering that location unsafe for reads. The
Producer and Consumer must always maintain a level of safety padding ahead of
their "safe" pointer locations.

The `driver_transfer_bytes` value specified by a driver is critical for ensuring
that clients do not read into memory that the driver is still actively updating.
With the 'ping pong' pattern mentioned above, a driver would specify a value for
`driver_transfer_bytes` that is twice the size of the actual transfers
themselves. Indeed it would reflect the size of its internal double-buffer that
provides the extra duration of safety padding.

## Hardware versus Software (or hardware transfers, versus driver process-and-copy)

Ring buffer data frames can be directly consumed/generated by audio hardware:
i.e. `driver_transfer_bytes` might map directly to the size of a hardware FIFO
block, since that FIFO block would determine the upper limit amount of data read
ahead or held back. Note that if the FIFO buffer is used in the traditional
"high water" way (such as 'ping pong' design where only half of the FIFO is used
at any time -- after first filling the entire FIFO at `Start` time), then
`driver_transfer_bytes` would be set to the size of the internal FIFO buffer,
which would be double the size of the internal transfers if using the 'ping
pong' pattern. Even if smaller transfers are used, if the full size of the FIFO
is used (for instance, upon `Start` when filling an initially empty hardware
FIFO), then `driver_transfer_bytes` must be set to the entire size of this FIFO
buffer.

Ring buffer data may instead be consumed/generated by audio driver *software*
that is conceptually situated between the ring buffer and the audio hardware. In
this case, for playback as an example, the `driver_transfer_bytes` read ahead
amount must be large enough such that the driver guarantees no undetected
underruns, based on the client requirement to generate data at the rate
specified by `CreateRingBuffer` and at locations derived from `start_time` of
`Start`. Conversely, for capture `driver_transfer_bytes` must be large enough
for the driver to guarantee no underruns when generating data as determined by
`CreateRingBuffer` and `Start`. Also, it is expected that the
`driver_transfer_bytes` in these cases would be larger than merely the size of
the transfer itself, since it must also include any safety padding to account
for delays from scheduling and executing this driver processing.
