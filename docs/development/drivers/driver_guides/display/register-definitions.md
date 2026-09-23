# Principles for modern register definitions in Fuchsia display drivers

This document aims to apply some modern software engineering practices to the
context of driver development. The overall intent is to capitalize on the
benefits of Fuchsia's modern environment for driver development, which are

- a fully featured C++ toolchain with access to modern language features
- (almost) full access to libraries
- support for unit tests

## Motivation

Registers constitute the primary interface between our drivers and the hardware,
making them, effectively, a critical API.

While we have limited control over the underlying hardware and firmware
implementation, we have significant control over the software representation of
the register API. We can remove common pain points in driver development by
applying modern software development principles: expressive naming, typeful
programming with scoped enums, rigorous precondition checks.

Like any software API, register definitions are a force multiplier throughout
the entire driver lifecycle. Improving the readability of register definitions
will accelerate initial development, streamline code review, and reduce
long-term maintenance costs.

## The principles

### Expressive register and field names

The register and field names in the datasheets are not cast in stone. We must
not hesitate to use different names when that results in more readable code.

Follow the practices below for renaming registers and/or fields.

1. Use the readable name in the definition.
2. Use the readable name consistently throughout the driver, in both comments
   and variable names.
3. Mention the datasheet name exactly once, in the register / field definition's
   documentation comment.

The practices above maximize the benefits of the readable name, while
acknowledging the rename exactly once. This is sufficient for checking our
driver code against the datasheets, and for surfacing the rename to readers who
search for the datasheet name in our code.

The example below shows renaming a register and one of its fields. The comments
include the names used by the vendor's datasheet, as well as a reference into
the datasheet (more on that later).

```c++
// PP_CONTROL (Panel Power Control)
//
// Tiger Lake: IHD-OS-TGL-Vol 2c-1.22-Rev2.0 Part 2 pages 961-962
class PchPanelPowerControl : public hwreg::RegisterBase<PchPanelPowerControl, uint32_t> {
public:
  // If true, the eDP port's VDD is on even if the panel power sequence hasn't
  // been completed. Intended for panels that need VDD for DP AUX transactions.
  DEF_BIT(3, vdd_always_on);  // PRM name: VDD Override
};
```

Expressive names make it easier to understand what the code using the registers
is doing. This shifts bug detection earlier in the development timeline ("shift
left"), by increasing the chances that bugs are found during code review or
casual code inspection. Example:

- Bad: `control.set_pu_en1(true).set_pdwn1(true)` is fairly inscrutable, and a
  reviewer will likely gloss over the code
- Good:
  `control.set_cc1_connected_to_pull_up(true).set_cc1_connected_to_pull_down(true)`
  is more likely to make a reviewer wonder if the driver will cause a
  short-circuit

[`hwreg`][hwreg-library], our library for describing registers, turns register
names into class names, and creates method names from field names. If the
resulting names go against
[the *Naming* section in the Google C++ style guide][google-cpp-style-guide-naming]
that's a very strong indication that we need better names. In particular, our
C++ style guide aggressively optimizes for the readers of the code, whereas
hardware signal names are traditionally optimized for taking up minimal space in
schematics.

#### Precedents

[The *Write around non-inclusive code terms* section in the Google developer documentation style guide][google-docs-guide-inclusive-naming]
effectively recommends creating a wrapper around a non-inclusive term. We must
not hesitate to deploy this same strategy to obtain more readable code.

[The *Default to C-like Formatting* section in the lowRISC Verilog Coding Style Guide][lowrisc-verilog-style-guide-formatting]
argues that the Google C++ style guide sets a good precedent for hardware.
[The *Use descriptive names* section in the lowRISC Verilog Coding Style Guide][lowrisc-verilog-style-guide-naming]
guide matches
[the *Naming* section in the Google C++ style guide][google-cpp-style-guide-naming]
in the conservative attitude towards abbreviations, and in the general principle
of optimizing for readers over writers.

### Predicate semantics for bits

Bits that gate (enable or disable) behaviors are effectively predicates. The
bool type is a great match for these bits' values.

To facilitate bool representation, the names of predicate bits should convey
states (via adjectives), rather than actions (via verbs). Example:

- Good: `power_savings_enabled`
    - `if (!power_savings_enabled())` correctly reads like a getter with no side
       effects
    - `set_power_savings_enabled(false)` flows nicely when read
- Bad: `enable_power_savings`
    - `if (!enable_power_savings())` reads like calling a method with side
      effects and checking for failure
    - `set_enable_power_savings(false)` does not flow well

Prefer positive names, such as names suffixed with `_enabled` rather than
`_disabled`. Example:

- Good: `if (power_savings_enabled())`
- Bad: `if (!power_savings_disabled())` results in a
  [double negative][wikipedia-double-negative]{.external}

Bits with trigger semantics can be described using the `_in_progress` suffix,
supplemented by comments that clearly specify the driver responsibilities.

```c++
  // True while the reset FSM (Finite State Machine) is active.
  //
  // The display driver sets this bit to true to initiate a software reset.
  // The reset FSM hardware sets this bit back to false when it completes
  // the reset operation.
  DEF_BIT(0, reset_in_progress);  // PRM name: RESET
```

### Scoped enums for field values

C++'s scoped enums (`enum class`) are a great example of typeful programming.
They eliminate the class of bugs caused by accidentally using values in
incorrect contexts, such as using a port ID instead of an operation mode ID. By
contrast, unscoped enum (`enum`) values implicitly convert to the underlying
integer type, which implicitly converts to any other unscoped enum type.

Any field whose datasheet description includes a table is a prime candidate to
be represented as a scoped enum.

Bits that don't have predicate semantics are also prime candidates for scoped
enum representation. For example, a bit that toggles between
[HDMI][wikipedia-hdmi] and [DisplayPort][wikipedia-display-port] output should
be considered a 1-bit field rather than a predicate, and represented using
scoped enums.

### Helpers for normalizing field encodings

Register value encodings were probably chosen to meet hardware design goals,
such as simplified implementation, improved interoperability with other IP
pieces. It is very rare that the encodings that make the most sense for hardware
are also the best fit for driver software.

The following common cases are prime candidates for normalizing.

- Encodings aimed at reducing the number of bits required to represent the
  useful values
    - biased encodings: field value \= conceptual value \+ delta
    - scaled encodings: field value \= conceptual value \* scale
    - logarithmic encodings: field value \= log2(conceptual value)
    - non-integral value encodings: fixed point or unusual (not IEEE 754\)
      floating point representations
- Concepts where we standardized on a hardware-agnostic representation

Usually, normalizing is best done right in the register class, by adding a
getter and setter. This minimizes the amount of code exposed to the suboptimal
encoding. The getter and the setter must have shorter names than the raw field,
to make correct usage easier than incorrect usage.

The example below shows a field definition with a helper.

```c++
  // The number of color channels expected from the unpacker.
  //
  // The field uses a non-trivial encoding (value minus 1).  Prefer the helpers
  // `channel_count()` and `set_channel_count()` to accessing this field
  // directly.
  DEF_FIELD(13, 12, channel_count_minus_one);  // PRM name: CHANNELS

  // The number of color channels expected from the unpacker.
  int32_t channel_count() const {
    // The arithmetic and casting operations do not overflow (causing UB) because
    // the field is 2-bits wide.
    return static_cast<int32_t>(channel_count_minus_one()) + 1;
  }

  // See `channel_count()`.
  //
  // `channel_count` must be in the range [1, 4].
  LayerImageSourceFormat& set_channel_count(int32_t channel_count) {
    // See below for checking preconditions on `channel_count`.

    // The arithmetic and casting operations do not overflow (causing UB)
    // because of the range enforced by the checks above.
    return set_channel_count_minus_one(static_cast<uint32_t>(channel_count - 1));
  }
```

Some common patterns are below.

- Raw field name suffixes can convey unusual scaling. Example:
    - raw field name (used by helpers): `clock_speed_19200hz` communicates that
      the value is expressed in multiples of a 19.2 kHz base clock
    - helper name (used by the rest of the code): `clock_speed_hz` appears to be
      easier to use (which is what we want), and conveys that the value is
      expressed in [Hertz (Hz)](https://en.wikipedia.org/wiki/Hertz){.external}
- Raw field name suffixes can convey biasing. Example:
    - raw field name: `cycle_count_minus_one` communicates that the value uses
      minus-one encoding (1 is encoded as 0, 2 is encoded as 1, so on)
    - helper name: `cycle_count` appears to be easier to use, which is what we
      want
- Generic suffixes `_select` or `_bits` make it clear that the raw has an
  unusual encoding. Example:
    - raw field name: `image_width_bits`
    - helper name: `image_width_px` (widths are generally expressed in
      [pixels][wikipedia-pixel]{.external})

Encoding-normalizing helpers are also an opportunity to optimize the type used
to represent the normalized values. For example,
[the *Integer Types* section in the Google C++ style guide][google-cpp-style-guide-integer-types]
recommends that values involved in arithmetic operations are represented using
signed integers, whereas values that represent bit patterns (such as register
fields) are represented using unsigned integers.

### Helpers for logging

Each scoped enum used by the higher-level driver code must include a
`std::formatter` specialization. This has proven easiest to accomplish by
defining a `ToString()` function that returns `std::string_view`, and by calling
that function in the format method of a formatter that inherits from
`std::formatter<std::string_view>`.

For example, the header that defines a DisplayPortType scoped enum would also
declare a function with the prototype below.

```c++
std::string_view DisplayPortTypeToString(DisplayPortType display_port_type);
```

The function would be used in the (fairly boilerplate) specialization below.

```c++
template <>
struct std::formatter<DisplayPortType> : std::formatter<std::string_view> {
  auto format(DisplayPortType display_port_type, auto& ctx) const {
    return std::formatter<std::string_view>::format(
      DisplayPortTypeToString(display_port_type), ctx);
};
```

Optionally, each scoped enum used by the higher-level driver code must be
accompanied by a function that maps enum values to `const char*`. This
facilitates logging the enum with functions that use printf format specifiers,
such as `ZX_ASSERT()` and `ZX_DEBUG_ASSERT()`.

For example, the header that defines a DisplayPortType scoped enum would also
declare a function with the prototype below.

```c++
const char* DisplayPortTypeToString(DisplayPortType display_port_type);
```

Scoped enums that are only intended to be used by encoding-normalizing helpers
are not covered by this guideline, because the higher-level driver code is best
served by logging normalized values.

### Precondition checks

Every method must check that its preconditions are met, when it is feasible to
do so. Precondition checks have two big benefits.

1. Bug detection time shifts left on the timeline, because some bugs will trip
   the precondition checks.
2. Expressing preconditions in code helps flag imprecise contracts.

#### Targets for precondition checks

Encoding-normalizing helpers usually have preconditions around the range and
precision of input values. These preconditions ideally ensure that the helpers
perform lossless conversion, and that our drivers don't write any undocumented
or invalid values to registers.

As a precedent, the setters generated by `DEF_FIELD()` macros have a
precondition that the value to be set fits into the field.

Preconditions only make sense on values produced by code that we control. In
particular, drivers can't place any precondition on values read from hardware
registers, because the driver code doesn't control the hardware implementation.
Instead, driver code must be prepared to handle (or intentionally ignore)
incorrect or undocumented values in register fields.

#### Actionable failures

Failed precondition checks
[should be actionable][google-testing-blog-actionable-failures].
Checks should print out the state involved in the failed precondition, unless
it's obvious from the predicate.

In the example below, the `channel_count` value is a useful piece of information
for someone investigating the failure.

```c++
ZX_DEBUG_ASSERT_MSG(channel_count >= 1, "Invalid channel count: %d", channel_count);
```

The message format above uses two techniques to reduce parsing ambiguity.

1. The variable is at the end of the message, so the reader doesn't have to
   guess where the variable's string representation ends.
2. The variable is separated from the rest of the message by a colon and a space
   (`:` ) so the reader doesn’t have to guess where the string representation
   begins.

The example below is a rare case where printing the state is not necessary,
because the only value that could lead to failure is zero.

```c++
ZX_DEBUG_ASSERT(denominator != 0);
```

Predicates with multiple terms joined by and operators (`&&`) should be broken
into separate checks. This results in slightly more information for debugging
failures. The failed predicate may appear redundant with the logged value, but
buggy hardware may not behave logically.

The example below shows how a range check for `channel_count` is best written as
two separate checks.

```c
ZX_DEBUG_ASSERT_MSG(channel_count >= 1, "Invalid channel count: %d", channel_count);
ZX_DEBUG_ASSERT_MSG(channel_count <= 4, "Invalid channel count: %d", channel_count);
```

#### Debug assertions

Precondition checking uses one of the following methods.

- `ZX_ASSERT()` crashes the driver in production when the precondition is not
  met. This brings down all drivers that share the driver host process.
- `ZX_DEBUG_ASSERT()` only crashes the driver in development builds when the
  precondition is not met.

`ZX_DEBUG_ASSERT()` is appropriate when continued execution will only result in
localized failures that do not threaten the entire system. For example,
incorrectly configured power gates usually cause localized failures such as
excessive power consumption or ignored commands.

`ZX_ASSERT()` is particularly appropriate when continued execution would
compromise the entire system's integrity. Performing I/O to incorrect addresses
jeopardizes system integrity. Invalid electrical circuit configuration
(short-circuit, bad voltage / current regulation) is also a threat to system
integrity. Keep in mind that we rarely know enough about the hardware we're
driving to assert that failures will be localized. So, `ZX_ASSERT()` is almost
always the right choice.

Precondition checks that require a disproportionately large amount of resources
(CPU cycles, memory bandwidth) may not be suitable for production code, but are
still feasible in engineering builds. Using `ZX_DEBUG_ASSERT()` for these checks
is preferable to leaving them out altogether. To help the optimizer,
`ZX_DEBUG_ASSERT_IMPLEMENTED` must be used to guard any code that exists
strictly to support the debug checks.

By contrast, precondition checking is infeasible in the following situations.

- The check would introduce side-effects.
    - We must assume that register access has side-effects, so checks that
      require register access are unsafe.
    - The CPU time consumed by the checks becomes an undesirable side-effect in
      time-sensitive environments, such as interrupt handlers.
- Implementing the check adds coupling (for getting information from multiple
  places), and the architectural cost of the coupling exceeds the estimated
  benefit of reducing bugs.

### Coping with incomplete documentation

Vendor documentation may use different terms for the same concept, and/or may
use the terms here to refer to different concepts. Please use the terms below as
described in this section.

**Reserved** registers and fields are explicitly documented as not being
currently used. The documentation sometimes mandates setting the fields to a
value, and most often all bits must be zero (MBZ). When the documentation does
not specify a safe value, reserved registers and fields must not be modified.

Use `DEF_RSVDZ_BIT` and `DEF_RSVDZ_FIELD` if and only if the underlying fields
are documented as MBZ.

Call out registers with reserved fields that don't have documented values using
a comment similar to the example below.

```c++
// This register has bits that are reserved but not MBZ (must be zero). So, it
// can only be safely updated via read-modify-write operations.
```

Registers and fields are **not documented** (undocumented) when the
documentation completely omits them, or when the documented description lacks
sufficient detail to support driver development.

In some cases, the documentation uses addresses (MMIO / I2C address, bit
positions) to reference undocumented registers and fields. In other cases,
documentation may reference register and field names that lack addresses or
descriptions.

Registers and fields are **not defined** when we acknowledge their existence,
usually via comments, but choose not to write definitions. This is usually the
case when we don't plan to use the registers, and can't justify the cost of
producing and maintaining their definitions.

### Documentation

Future driver maintainers (which may be you, a few years from now) will need to
look up how registers and fields work, just as much as other engineers look up
API references. Having good documentation on hand will reduce the time it takes
to get up to speed, so folks can spend more time on enjoyable activities, such
as coding.

#### References

As the lowest level of software above hardware, drivers have the unique property
that programmers can't dive into the code below them to see how things work.
This differs from most other software written at Google, where a suspicion that
a class or method comment is incorrect can be checked by looking at the
implementation. We can compensate by carefully documenting the provenance of the
information we use to write the drivers.

Drivers must contain a reference to the documents (datasheets, programmer
manuals, specifications) used to develop them. The references save future
readers from having to search for documentation, and can prevent confusion
caused by cross-referencing a driver against a different document (such as a
newer datasheet revision) from the one used by the driver's authors.

The example below shows maintaining a list of references in a `README.md`
section.

```markdown
## References

The code contains references to the following documents.

- [the FUSB302B datasheet][datasheet]{.external} - Revision 5, August 2021, publication
  order number FUSB302B/D - referenced as "Rev 5 datasheet"
- [the USB Power Delivery Specification][usb-pd-spec]{.external} - Revision 3.1,
  Version 1.7, January 2023 - referenced as `usbpd3.1`

[datasheet]: https://www.onsemi.com/pdf/datasheet/fusb302b-d.pdf
[usb-pd-spec]: https://usb.org/document-library/usb-power-delivery
```

Each register should have a reference to the datasheet description that was
used. Deep references (section and/or page numbers) save future readers from
having to guess which part of the datasheet served as the source of information.
Specific references are particularly useful when the datasheet doesn't have good
section headings, or when the same register is described in multiple places.

The example below shows a deep reference in a register definition's
documentation.

```c++
// DEVICE ID - Identifies the chip.
//
// Rev 5 datasheet: Table 17 on page 19
class DeviceId : public Fusb302Register<DeviceIdReg> {
  // ...
```

The above guidelines assume public hardware documentation. Building drivers
based on non-public documentation has some subtleties that are outside the scope
of this document.

#### Deviations from official documentation

Information learned by experimental results must be explicitly called out.
Example: *Experiments using the i5-1135G7 processor indicate that Tiger Lake
display engines use the same registers.*

Information that contradicts the datasheets must also be explicitly called out,
to avoid confusion. Example: *While Section 3.5 page 172 of the datasheet claims
that the interrupt register is Read/Clear, experiments using a XT351 show that
the register must be explicitly cleared by writing zero to it.*

#### API contract-style doc comments

Datasheets greatly differ in terms of writing style, clarity, and organization.
For this reason, consulting a datasheet has a non-trivial mental energy cost.
The cost may tempt developers to skip referencing the datasheet, especially when
making small changes during maintenance tasks. This means that drivers that
require referencing the datasheet for all development work have a higher risk of
accumulating errors during maintenance.

This issue can be countered by adding brief comments to registers and their
fields. The comments can briefly summarize the functionality of the register or
field, and don't need to cover all the information in the datasheet. The primary
audience should be folks who need to dive into the driver for quick debugging
modifications. The goal of the comments is to maximize the odds that these folks
catch errors that would be immediately obvious to someone familiar with the
hardware.

The guidance in
[the Comments section in the C++ style guide][google-cpp-style-guide-comments]
applies quite well to register and field comments. A good perspective is to
write the comments as an API contract between driver developers and the
hardware.

[google-docs-guide-inclusive-naming]: https://developers.google.com/style/inclusive-documentation#write-around
[google-cpp-style-guide-comments]: https://google.github.io/styleguide/cppguide.html#Comments
[google-cpp-style-guide-naming]: https://google.github.io/styleguide/cppguide.html#Naming
[google-cpp-style-guide-integer-types]: https://google.github.io/styleguide/cppguide.html#Integer_Types
[google-testing-blog-actionable-failures]: https://testing.googleblog.com/2024/05/test-failures-should-be-actionable.html
[hwreg-library]: /zircon/system/ulib/hwreg/
[lowrisc-verilog-style-guide-formatting]: https://github.com/lowRISC/style-guides/blob/master/VerilogCodingStyle.md#default-to-c-like-formatting
[lowrisc-verilog-style-guide-naming]: https://github.com/lowRISC/style-guides/blob/master/VerilogCodingStyle.md#use-descriptive-names
[wikipedia-double-negative]: https://en.wikipedia.org/wiki/Double_negative
[wikipedia-display-port]: https://en.wikipedia.org/wiki/DisplayPort
[wikipedia-hdmi]: https://en.wikipedia.org/wiki/HDMI
[wikipedia-pixel]: https://en.wikipedia.org/wiki/Pixel
