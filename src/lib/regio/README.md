# regio

`regio` ("reh-jee-o") is a general, extensible framework for register I/O. It is
agnostic of the choice of layout type, and aims to support varied notions of
'register' across common CPU architectures and hardware.

## `Register`

`Register` represents an abstract register tied to a specific layout, access
permissions, and an 'I/O backend' defining how it is accessed. Its API admits
the usual access methods when its permissions and backend permit them.

Conventions:

- It is expected that hardware-specific notions of register define themselves as
  a type alias or wrapper of `Register` and implement their own specialized
  constructors.

- When register instances are `const`-constructible, they ought to be `const`s
  and carry the same name as they are documented in architectural manuals. They
  are usually documented in screaming snake case already, so this makes for a
  nice, readable coincidence.

## Traits and testing

The [`traits`] module abstracts register access as traits (e.g., [`ReadReg`] and
[`SafeWriteReg`]), permitting register-related logic to be written generically
for testability. The [`testing`] module - available with the `testing` feature -
provides implementations of these traits (e.g., [`ExpectationReg`]) that may be
supplied instead of [`Register`] at test time.

For simple read-only cases, a closure can serve as a stand-in register:

```rust
# #[cfg(feature = "testing")] {
use regio::traits::ReadReg;

fn is_busy(reg: impl ReadReg<u32>) -> bool {
    reg.read() & 1 != 0
}

assert!(is_busy(&|| 0x3u32));
assert!(!is_busy(&|| 0x2u32));
# }
```

One can also reach for [`ExpectationReg`] to validates a full sequence of
expected accesses:

```rust
# #[cfg(feature = "testing")] {
use regio::testing::ExpectationReg;
use regio::traits::RwSafeReg;

fn set_enable_bit(reg: impl RwSafeReg<u32>) {
    reg.modify(|value| *value |= 1);
}

let mut reg = ExpectationReg::<u32>::new();
reg.expect_read(0xf0).expect_write(0xf1);
set_enable_bit(&reg);
// On drop, `reg` asserts that no expected accesses remain.
# }
```

## MMIO

The core MMIO types are `Offset`, `Mmio`, `MmioPtr`, and `MmioBank`. `Mmio` is
an alias of `Register` representing an MMIO register. Often MMIO addresses are
not statically known; in this case, we port the above `const` `Register`
convention over to defining the offset descriptors - which are usually
statically known - to be `const` instances bearing the documented register
names.

The following is example logic for driving a SiFive UART.

```rust
use core::ptr;

use bitrs::layout;
use regio::{MmioBank, MmioPtr, Offset, RwSafe};

layout!({
    struct TransmitDataRegister(u32);
    {
        let full @ 31; // Whether the TX FIFO is full.
        let __ @ 30..8;
        let data @ 7..0;
    }
});

const TXDATA: Offset<TransmitDataRegister, RwSafe> = Offset::new(0);

layout!({
    struct ReceiveDataRegister(u32);
    {
        let empty @ 31; // Whether the RX FIFO is empty.
        let __ @ 30..8;
        let data @ 7..0;
    }
});

const RXDATA: Offset<ReceiveDataRegister, RwSafe> = Offset::new(4);

layout!({
    struct InterruptEnableRegister(u32);
    {
        let _ @ 31..2;
        let rxwm @ 1; // Receive watermark interrupt enable
        let txwm @ 0; // Transmit watermark interrupt enable
    }
});

const IE: Offset<InterruptEnableRegister, RwSafe> = Offset::new(0x10);

// ...

const BANK_SIZE = 0x1c; // The highest documented offset is 0x18.

// Could be made const in this case, but in practice it may not be.
let uart_base = unsafe {
    MmioPtr::<u32, RwSafe>::new(ptr::with_exposed_provenance_mut(0x1001_3000))
};
let uart = MmioBank::new(uart_base, BANK_SIZE);

// Can access the MmioBank to get an Mmio.
unsafe { uart.at(IE) }.write(0u32.into());  // Disable all interrupts.

let data = TransmitDataRegister::from(0).set_data(0xab);
unsafe { uart.at(TXDATA) }.write(data);

// Simple reads can fluidly chain from MmioPtr -> Mmio -> Layout.
let _: u8 = unsafe { uart.at(RXDATA) }.read().data();
```

## arm64 system registers

`regio::arm64::SysReg` defines an arm64 system register, aliasing `Register`. It
takes a system register 'spec' as a generic parameter, all of which have been
stamped out in the `regio::arm64::spec` submodule with names equal to their
official mnemonics.

```rust
use regio::arm64::{SysReg, spec};

const TPIDR_EL0: SysReg<spec::TPIDR_EL0, u64> = SysReg::new();

println!("TPIDR_EL0: {:#x}", TPIDR_EL0.read().get());
```

## riscv64 CSRs

`regio::riscv64::Csr` defines a riscv64 CSR, aliasing `Register`. It takes a CSR
'encoding' as a generic parameter, all of which have been stamped out in the
`regio::riscv64::encoding` submodule with names equal to their official
mnemonics.

```rust
use regio::riscv64::{Csr, encoding};

const TIME: Csr<encoding::time, u64> = Csr::new();

println!("time: {:#x}", TIME.read());
```

## x86 Control Registers

`Cr` and `Xcr` are aliases of `Register` used to model x86 control registers:

```rust
use regio::x86::{Cr, Xcr};

const CR0: Cr<0, u64> = Cr::new();
const CR3: Cr<3, u64> = Cr::new();
const XCR0: Xcr<0, u64> = Cr::new();
```

## x86 CPUID

`CpuidValue` defines a CPUID value marker associated with a particular leaf,
subleaf, output register (`EAX`, `EBX`, `ECX`, or `EDX`), and layout type:

```rust
use regio::x86::{CpuidValue, EAX};

const CPUID_MAX_LEAF: CpuidValue<0x0, 0x0, EAX, u32> = CpuidValue::new();
```

CPUID access is abstracted via the [`Cpuid`] trait, providing methods to read
values, check leaf support, and conditionally read supported leaves:

- `DirectCpuid` issues actual `cpuid` instructions on x86 hardware:

```rust
use regio::x86::{Cpuid, DirectCpuid};

# #[cfg(any(target_arch = "x86", target_arch = "x86_64"))] {
println!("Maximum CPUID leaf number: {:#x}", DirectCpuid {}.read(CPUID_MAX_LEAF));
# }
```

- [`FakeCpuid`] (under [`regio::testing::x86`], available with the `testing`
  feature) provides a mock CPUID reader for testing and non-x86 environments:

```rust
# #[cfg(feature = "testing")] {
use regio::testing::x86::FakeCpuid;
use regio::x86::Cpuid;

let mut cpuid = FakeCpuid::new();
cpuid.populate(CPUID_MAX_LEAF, 0x7);
assert_eq!(cpuid.read(CPUID_MAX_LEAF), 0x7);
# }
```

## x86 MSRs

`Msr` is an alias of `Register` used to model MSRs:

```rust
use regio::RwSafe;
use regio::x86::Msr;

const IA32_TIME_STAMP_COUNTER: Msr<0x10, u64, RwSafe> = Msr::new();

println!("Current timestamp: {:#x}", IA32_TIME_STAMP_COUNTER.read());
```

## x86 I/O Ports

`Port` is an alias of `Register` used to model x86 I/O ports:

```rust
use regio::RwSafe;
use regio::x86::Port;

const SERIAL_PORT: Port<u8, u8, RwSafe> = Port::new(0x3f8);

SERIAL_PORT.write(b'A');
```



