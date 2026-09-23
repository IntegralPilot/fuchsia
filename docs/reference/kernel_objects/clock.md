# Clock

## NAME

clock - Kernel object used to track the progress of time.

## SYNOPSIS

A clock is a one dimensional affine transformation of the
[clock monotonic](/reference/syscalls/clock_get_monotonic.md) reference
timeline, which may be atomically adjusted by a clock maintainer, and observed by
clients.

## DESCRIPTION

### Properties

The properties of a clock are established when the clock is created and cannot
be changed afterwards. Currently, three clock properties are defined.

#### **ZX_CLOCK_OPT_MONOTONIC**

When set, the clock is guaranteed to have monotonic behavior. This is to say
that any sequence of observations of the clock is guaranteed to produce a
sequence of times that are always greater than or equal to the previous
observations. A monotonic clock can never go backwards, although it _can_ jump
forwards. Formally:

Given a clock _C_, Let C(x) be the function that maps from the reference
timeline _C's_ timeline. C(x) is a piecewise linear function made up of all the
affine transformation segments over all time as determined  by _C's_ maintainer.
_C_ is monotonic if and only if:

for all _R<sub>1</sub>_, _R<sub>2</sub>_ : _R<sub>2</sub> >= R<sub>1</sub>_

_C(R<sub>2</sub>) >= C(R<sub>1</sub>)_

#### **ZX_CLOCK_OPT_CONTINUOUS**

When set, the clock is guaranteed to have continuous behavior. This is to say
that any update to the clock transformation is guaranteed to be first order
continuous with the previous transformation segment. Formally:

Let _C<sub>i</sub>(x)_ be the _i<sub>th</sub>_ affine transformation segment of
_C(x)_. Let _R<sub>i</sub>_ be the first point in time on the reference timeline
for which _C<sub>i</sub>(x)_ is defined. A clock _C_ is continuous if and only
if: for all _i_

_C<sub>i</sub>(R<sub>i + 1</sub>) = C<sub>i + 1</sub>(R<sub>i + 1</sub>)_

#### **Backstop Time**

The backstop time of a clock represents the minimum value that a clock may ever
be set to. Since clocks can only tick forwards, and never backwards, it is
impossible for an observer of a clock to ever receive a value that is less than
the backstop time configured by a clock's creator.

A backstop time may be provided via the `zx_create_args_v1_t` structure at
creation time. Otherwise, it will default to 0.

During clock update operations, any attempt to set the clock's value to
something less than the backstop time will fail with **ZX_ERR_INVALID_ARGS**. A
clock that has not been initially set will always report the backstop time
configured for the clock. Backstop times may never be less than the default
value of zero.

### Implied properties

+ The reference clock for all clock objects in the system is clock monotonic.
+ The nominal units of all clock objects are specified to be nanoseconds. This
  property is not configurable.
+ The units of frequency adjustment for all clock objects are specified to be
  parts per million, or PPM.
+ The maximum permissible range of frequency adjustment of a clock object is
  specified to be \[-1000, +1000\] PPM. This property is not configurable.

### Additional creation options

#### **ZX_CLOCK_OPT_AUTO_START**
When you use this option during clock creation, the clock begins in a started
state instead of the default non-started state. See [Starting a
clock](#starting-a-clock) for details.

### Reading the clock

Given a clock handle, users may query the current time given by that clock using
the `zx_clock_read()` syscall. Clock reads **ZX_RIGHT_READ** permissions. Clock
reads are guaranteed to be coherent for all observers. This is to say that, if
two observers query the clock at exactly the same reference time _R_, that they
will always see the same value _C(R)_.

### Reference timelines, `zx_ticks_get()`, and `zx_clock_get_monotonic()`

As noted earlier, zx_clock_get_monotonic() is the reference timeline for all
user-created zircon clocks. This means that if a user knows a clock instance's
current transformation, then given a value on the clock instance's timeline, the
corresponding point on the clock monotonic timeline may be computed (and
vice-versa). It also means that in the absence of a rate adjustment made to the
kernel clock, clock monotonic and the kernel clock will tick at exactly the same
rate.

In addition to the clock monotonic timeline, the zircon kernel also exposes the
"ticks" timeline via `zx_ticks_get()` and `zx_ticks_per_second()`. Internally,
ticks is actually the reference timeline for clock monotonic and is read
directly from an architecture appropriate timer unit accessible to the kernel.
Clock monotonic is actually a linear transformation of the ticks timeline
normalized to nanosecond units. Both timelines start ticking from zero as the
kernel starts.

Because clock monotonic is a static transformation based on ticks, and all kernel
clocks are transformations based on clock monotonic, ticks may also serve as a
reference clock for kernel clocks in addition to clock monotonic.

### Fetching the clock's details

In addition to simply reading the current value of the clock, advanced users who
possess **ZX_RIGHT_READ** permissions may also read the clock and get extended
details in the process using `zx_clock_get_details()`. Upon a successful call,
the details structure returned to callers will include:

+ The current clock monotonic to clock transformation.
+ The current ticks to clock transformation.
+ The current symmetric [error bound estimate](#error-bound) (if any) for the
  clock.
+ The last time the clock was updated as defined by the clock monotonic
  reference timeline.
+ An observation of the system tick counter, which was taken during the
  observation of the clock.
+ All of the static properties of the clock defined at creation time.
+ A generation nonce whose value changes every time the clock's underlying
  transformation is updated.

Advanced users may use these details to not only compute a recent `now` value
for the clock (by transforming the reported ticks-now observation using the
ticks-to-clock transformation, both reported by the get details operation), but
to also:

+ Know whether the clock transformation has been changed since the last
  `zx_clock_get_details()` operation (using the generation nonce).  Note that a
  clock's generation nonce is not guaranteed to start at any given value, nor to
  change in any specific way (such as incrementing by a fixed amount) with each
  update.  Instead, with every update the generation nonce will be changed to a
  value which is guaranteed to be different from the value it had immediately
  before the update took place.
+ Compose the clock transformation with other clocks' transformations to reason
  about the relationship between two clocks.
+ Know the clock maintainer's best estimate of [error bound](#error-bound).
+ Reason about the range of possible future values of the clock relative to the
  reference clock based on the last correction time, the current transformation,
  and the maximum permissible correction factor for the clock (see the maximum
  permissive range of frequency adjustment described in the |Implied properties|
  section above.

### Starting a clock and clock signals {#starting-a-clock}

Immediately after creation, a clock has not yet been started. All attempts to
read the clock will return the clock's configured backstop time, which defaults
to 0 if unspecified during creation.

A clock begins running after the very first update operation performed by a
clock's maintainer, which **must** include a set-value operation. The clock
will begin running at that point with a rate equal to the reference clock plus
the deviation from nominal specified by the maintainer.

Clocks also have a **ZX_CLOCK_STARTED** signal, which may be used by users to
know when the clock has actually been started. Initially, this signal is not
set, but it becomes set after the first successful update operation. Once
started, a clock will never stop and the **ZX_CLOCK_STARTED** signal will always
be asserted.

Initially, the clock is a clone of clock monotonic, which makes the
transformation between the clock monotonic timeline and synthetic timeline the
identity function. This clock may still be [maintained](#maintaining-a-clock)
after creation, subject to the limitations imposed by rights, the
**ZX_CLOCK_OPT_MONOTONIC** and **ZX_CLOCK_OPT_CONTINUOUS** properties, and the
configured backstop time.

If a clock is created with the **ZX_CLOCK_OPT_AUTO_START** options, it cannot be
configured with a backstop time that is greater than the current clock
monotonic time. If this was allowed, this would result in a state where a
clock's current time is set to a time before its backstop time.

### Maintaining a clock {#maintaining-a-clock}

Users who possess **ZX_RIGHT_WRITE** permissions for a clock object may act as a
maintainer of the clock using the `zx_clock_update()` syscall. Three parameters
of the clock may be adjusted during each call to `zx_clock_update()`, but not
all three need to be adjusted each time. These values are:

+ The clock's absolute value.
+ The frequency adjustment of the clock (deviation from nominal expressed in
  ppm)
+ The absolute [error bound estimate](#error-bound) of the clock (expressed in
  nanoseconds)

Changes to a clocks transformation occur during the syscall itself. The
specific reference time of the adjustment may not be specified by the user.

Any change to the absolute value of a clock with the **ZX_CLOCK_OPT_MONOTONIC**
property set on it which would result in non-monotonic behavior will fail with a
return code of **ZX_ERR_INVALID_ARGS**.

The first update operation is what starts a clock ticking and **must** include a
set-value operation.

Aside from the very first set-value  operation, all attempts to set the absolute
value of a clock with the **ZX_CLOCK_OPT_CONTINUOUS** property set on it will
fail with a return code of **ZX_ERR_INVALID_ARGS**

### Notes on the clock error bound estimate {#error-bound}

The `zx_clock_get_details()` syscall provides the user with a number of fine
grained details about a clock, including the "error bound estimate". This
value, expressed in nanoseconds, represents the clock maintainer's best current
estimate of how wrong the clock currently might be relative to the reference(s)
being used by the maintainer. For example, if a user fetched a time `X` with an
error bound estimate of `E`, then the clock maintainer is attempting to say that
it believes that the clock's actual value is somewhere in the range `[ X-E, X+E ]`.

The level of confidence in this estimate is _not_ specified by the kernel APIs.
It is possible that some clock maintainers are using a strict bound, while
others are using a bound that is not provable but provides "high confidence",
while others still might have little to no confidence in their estimates.

In the case that a user needs to understand the objective quality of the error
estimates they are accessing (for example, to enforce certificate validity
dates, or DRM license expiration), they should understand which component in the
system is maintaining their clock, and what guarantees that maintainer provides
when it comes to the confidence levels of its published error bound estimates.

### Mappable clocks

In addition to reading a clock or getting its details using the standard
syscalls (`zx_clock_read()` and `zx_clock_get_details()`), Zircon kernel clocks
offer another approach which can decrease observation overhead by (almost
always) skipping the need to enter the Zircon kernel in order to observe the
clock.  The main idea here is that clocks are, at their core, nothing more than
an affine transformation applied to the current value of the selected reference
clock.  If the transformation state can be accessed by user-mode via shared
memory, and the underlying reference clock does not require entering the Zircon
kernel to read, then the synthetic clock can be also observed without ever
needing to enter the kernel. Enter "mappable" clocks.

#### Creation

A mappable kernel clock can be created by calling `zx_clock_create()` and
passing `ZX_CLOCK_OPT_MAPPABLE` at the time of creation. The newly created clock
handle will contain the `ZX_RIGHT_MAP` permission as well as all of the other
default permissions.  Additionally, the options field reported via a get-details
operation will contain the `ZX_CLOCK_OPT_MAPPABLE` flag, reflecting the fact
that this is a mappable clock.

#### Mapping

In order to observe the clock's state using shared memory, two things must
happen first.

1) The mapped size of the clock state must be determined.
2) The clock's shared state must be mapped into the user's address space.

To fetch the clock's mapped size, users must call `zx_object_get_info()` with
the `ZX_INFO_CLOCK_MAPPED_SIZE` topic, passing a `uint64_t` sized buffer to
receive the mapped size of the clock.

Once the size is known, users may call `zx_vmar_map_clock()` in order to
actually map the clock's state into their address space.  This call operates
very similarly to `zx_vmap_map_vmo()`, only with some extra built in
restrictions.  Notably:

1) Only the `ZX_VM_PERM_READ` permission is allowed, all of the other
   `ZX_VM_PERM` flags are explicitly disallowed, regardless of the handle
   rights the user possesses.  Mapped clock data must always be read only.
2) The `len` parameter passed to the syscall *must* match the value retrieved
   from the get-info call.
3) There is no ability to specify a `vmo_offset` when mapping the clock's state.
   The entire clock state region must always be mapped.

Mapping a clock requires both the `ZX_RIGHT_READ` and `ZX_RIGHT_MAP` permissions
on the clock handle.

A successful map operation will return the virtual address of where the clock
object's state has been mapped in the specified VMAR.

#### Unmapping

The process of unmapping a clock is identical to the process of unmapping a VMO
or an IO Buffer region.  Users simply call `zx_vmar_unmap()` on the VMAR they
used to map the clock, passing the length received from the original get-info
call, and virtual address received from the `zx_clock_map()` call.

Users may close their clock's handle after mapping it and still read the
clock's state.  Just like closing a VMO handle will not destroy any mappings of
that VMO, closing the clock handle will also not destroy any of its mappings.
See `zx_vmar_unmap()` and `zx_vmar_destroy()`.

#### Observing a mapped clock

To observe a mapped clock, users call either `zx_clock_read_mapped()` or
`zx_clock_get_details_mapped()`, passing the virtual address for the mapped
clock state as the first parameter.  Everything else remains the same, please
refer to the documentation for `zx_clock_read()` and `zx_clock_get_details()`
for more information.

Any of the following actions will result in undefined behavior:

 - Attempting to call a mapped clock observation syscall using any virtual
   address aside from the one returned by the original call to `zx_clock_map()`.
 - Attempting to call a mapped clock observation syscall using clock state which
   has been either partially, or completely, unmapped.

Additionally, users *must not* make any attempt to use the mapped clock state to
infer details about the clock itself.  The format of the clock's state is
unspecified and subject to change at any time.  The *only* legitimate way to use
a clock's mapped state is in conjunction with calls to `zx_clock_read_mapped()
or `zx_clock_get_details_mapped()`.

When properly accessed using the provided syscalls, clock observations will be
"multi-observer" consistent, meaning the properties such as monotonicity and
continuity will be consistent across a set of observations made by multiple
observers _if those observations have an established order_. See
[here](/docs/contribute/governance/rfcs/0266_memory_mappable_kernel_clocks.md#Consistency)
for details.

#### Clock mapping representation in `zx_info_maps_t` and `zx_info_vmos_t` structures

Zircon currently provides two diagnostic `zx_object_get_info()` topics for
fetching information about about currently active mappings, `ZX_INFO_VMAR_MAPS`
and `ZX_INFO_PROCESS_MAPS`, both of which will return an array of
`zx_info_maps_t` strcutres describing the currently active mappings in the
specified VMAR, or process.

Clock mappings will be enumerated in the same way that VMO mappings are, with
the following differences.

1) Since clocks are not currently nameable objects, the string "kernel-clock"
   will be returned as the name of the mapping, instead of the current name of a
   VMO as would typically be done.
2) The KOID field of the mapping will be populated using the clock object's
   KOID.

Additionally, programs can get info on the current VMOs associated with a
process using the `ZX_INFO_PROCESS_VMOS` topic to fetch an array of
`zx_info_vmos_t` structures.  When a process has a clock
mapped into its address space, the clock will show up in this enumaration as
well.  Just like in the `zx_info_maps_t` case, the KOID reported will be the
KOID of the clock object, and the name reported will be "kernel-clock".
Additionally, the `flags` field of the entry will have the
`ZX_INFO_VMO_VIA_MAPPING` bit set to indicate that the object is being
enumerated on the list because of the active mapping.

#### Example

Here is a simplified example showing how you might map and read a clock.

```c++
class MappedClockReader {
 public:
  MappedClockReader(const zx::clock& clock) {
    zx_status_t status = clock.get_info(ZX_INFO_CLOCK_MAPPED_SIZE, &mapped_clock_size_,
                                        sizeof(mapped_clock_size_), nullptr, nullptr));
    ZX_ASSERT(status == ZX_OK);

    status = zx::vmar::root_self()->map_clock(ZX_VM_PERM_READ | ZX_VM_MAP_RANGE, 0, clock,
                                              mapped_clock_size_, &clock_addr_);
    ZX_ASSERT(status == ZX_OK);
  }

  ~MappedClockReader() {
    if (clock_addr_ != 0) {
      zx::vmar::root_self()->unmap(clock_addr_, mapped_clock_size_);
    }
  }

  zx_time_t Read() {
    zx_time_t result{0};
    const zx_status_t status = zx::clock::read_mapped(clock_addr_, &result);
    ZX_ASSERT(status == ZX_OK);
    return result;
  }

 private:
  zx_vaddr_t clock_addr_{0};
  uint64_t mapped_clock_size_{0};
}
```

## REFERENCES

 - [clock transformations](/docs/concepts/kernel/clock_transformations.md)
 - [`zx_clock_create()`](/reference/syscalls/clock_create.md) - create a clock
 - [`zx_clock_read()`](/reference/syscalls/clock_read.md) - read the time of the clock
 - [`zx_clock_get_details()`](/reference/syscalls/clock_get_details.md) - fetch the details of a clock's relationship to its reference
 - [`zx_clock_update()`](/reference/syscalls/clock_update.md) - adjust the current relationship of a clock to its reference
 - [mappable clocks RFC](/docs/contribute/governance/rfcs/0266_memory_mappable_kernel_clocks.md)
 - [`zx_clock_read_mapped()`](/reference/syscalls/clock_read_mapped.md) - read the time of a mapped clock
 - [`zx_clock_get_details_mapped()`](/reference/syscalls/clock_get_details_mapped.md) - fetch the details of a mapped clock's relationship to its reference
 - [`zx_clock_map()`](/reference/syscalls/vmar_map_clock.md) - map a clock's state into the specified VMAR
 - [`zx_vmar_destroy()`](/reference/syscalls/vmar_destroy.md) - destroy a VMAR and all of the mappings in it.
 - [`zx_vmar_map()`](/reference/syscalls/vmar_map.md) - map all or part of a VMO the specified VMAR
 - [`zx_vmar_unmap()`](/reference/syscalls/vmar_unmap.md) - unmap a currently active mapping in a VMAR
 - [`ZX_INFO_CLOCK_MAPPED_SIZE`]: (/reference/syscalls/object_get_info#zx_info_clock_mapped_size)
 - [`ZX_INFO_VMAR_MAPS`](/reference/syscalls/object_get_info#zx_info_vmar_maps) - Fetch info about currently active mappins in a VMAR.
 - [`ZX_INFO_PROCESS_MAPS`](/reference/syscalls/object_get_info#zx_info_process_maps) - Fetch info about currently active mappins in a process.
