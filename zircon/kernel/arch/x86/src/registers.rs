// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// This header is intended to be included in both C and ASM
pub const X86_CR0_PE: u64 = 0x00000001; /* protected mode enable */
pub const X86_CR0_MP: u64 = 0x00000002; /* monitor coprocessor */
pub const X86_CR0_EM: u64 = 0x00000004; /* emulation */
pub const X86_CR0_TS: u64 = 0x00000008; /* task switched */
pub const X86_CR0_ET: u64 = 0x00000010; /* extension type */
pub const X86_CR0_NE: u64 = 0x00000020; /* enable x87 exception */
pub const X86_CR0_WP: u64 = 0x00010000; /* supervisor write protect */
pub const X86_CR0_NW: u64 = 0x20000000; /* not write-through */
pub const X86_CR0_CD: u64 = 0x40000000; /* cache disable */
pub const X86_CR0_PG: u64 = 0x80000000; /* enable paging */
pub const X86_CR4_PAE: u64 = 0x00000020; /* PAE paging */
pub const X86_CR3_BASE_MASK: u64 = ((1u64 << 39) - 1) << 12;
pub const X86_CR4_PGE: u64 = 0x00000080; /* page global enable */
pub const X86_CR4_OSFXSR: u64 = 0x00000200; /* os supports fxsave */
pub const X86_CR4_OSXMMEXPT: u64 = 0x00000400; /* os supports xmm exception */
pub const X86_CR4_UMIP: u64 = 0x00000800; /* User-mode instruction prevention */
pub const X86_CR4_VMXE: u64 = 0x00002000; /* enable vmx */
pub const X86_CR4_FSGSBASE: u64 = 0x00010000; /* enable {rd,wr}{fs,gs}base */
pub const X86_CR4_PCIDE: u64 = 0x00020000; /* Process-context ID enable  */
pub const X86_CR4_OSXSAVE: u64 = 0x00040000; /* os supports xsave */
pub const X86_CR4_SMEP: u64 = 0x00100000; /* SMEP protection enabling */
pub const X86_CR4_SMAP: u64 = 0x00200000; /* SMAP protection enabling */
pub const X86_CR4_PKE: u64 = 0x00400000; /* Enable protection keys */
pub const X86_EFER_SCE: u64 = 0x00000001; /* enable SYSCALL */
pub const X86_EFER_LME: u64 = 0x00000100; /* long mode enable */
pub const X86_EFER_LMA: u64 = 0x00000400; /* long mode active */
pub const X86_EFER_NXE: u64 = 0x00000800; /* to enable execute disable bit */
pub const X86_MSR_IA32_PLATFORM_ID: u32 = 0x00000017; /* platform id */
pub const X86_MSR_IA32_APIC_BASE: u32 = 0x0000001b; /* APIC base physical address */
pub const X86_MSR_IA32_TSC_ADJUST: u32 = 0x0000003b; /* TSC adjust */
pub const X86_MSR_IA32_SPEC_CTRL: u32 = 0x00000048; /* Speculative Execution Controls */
pub const X86_SPEC_CTRL_IBRS: u64 = 1 << 0;
// Partitions indirect branch predictors across hyperthreads
pub const X86_SPEC_CTRL_STIBP: u64 = 1 << 1; /* Single Thread Indirect Branch Predictors */
pub const X86_SPEC_CTRL_SSBD: u64 = 1 << 2;
pub const X86_MSR_SMI_COUNT: u32 = 0x00000034; /* Number of SMI interrupts since boot */
pub const X86_MSR_IA32_PRED_CMD: u32 = 0x00000049; /* Indirect Branch Prediction Command */
pub const X86_MSR_IA32_BIOS_UPDT_TRIG: u32 = 0x00000079; /* Microcode Patch Loader */
pub const X86_MSR_IA32_BIOS_SIGN_ID: u32 = 0x0000008b; /* BIOS update signature */
pub const X86_MSR_IA32_MTRRCAP: u32 = 0x000000fe; /* MTRR capability */
pub const X86_MSR_IA32_ARCH_CAPABILITIES: u32 = 0x0000010a;
pub const X86_ARCH_CAPABILITIES_RDCL_NO: u64 = 1 << 0;
pub const X86_ARCH_CAPABILITIES_IBRS_ALL: u64 = 1 << 1;
pub const X86_ARCH_CAPABILITIES_RSBA: u64 = 1 << 2;
pub const X86_ARCH_CAPABILITIES_SSB_NO: u64 = 1 << 4;
pub const X86_ARCH_CAPABILITIES_MDS_NO: u64 = 1 << 5;
pub const X86_ARCH_CAPABILITIES_TSX_CTRL: u64 = 1 << 7;
pub const X86_ARCH_CAPABILITIES_TAA_NO: u64 = 1 << 8;
pub const X86_MSR_IA32_FLUSH_CMD: u32 = 0x0000010b; /* L1D$ Flush control */
pub const X86_MSR_IA32_TSX_CTRL: u32 = 0x00000122; /* Control to enable/disable TSX instructions */
pub const X86_TSX_CTRL_RTM_DISABLE: u64 = 1 << 0; /* Force all RTM instructions to abort */
pub const X86_TSX_CTRL_CPUID_DISABLE: u64 = 1 << 1; /* Mask RTM and HLE in CPUID */
pub const X86_MSR_IA32_SYSENTER_CS: u32 = 0x00000174; /* SYSENTER CS */
pub const X86_MSR_IA32_SYSENTER_ESP: u32 = 0x00000175; /* SYSENTER ESP */
pub const X86_MSR_IA32_SYSENTER_EIP: u32 = 0x00000176; /* SYSENTER EIP */
pub const X86_MSR_IA32_MCG_CAP: u32 = 0x00000179; /* global machine check capability */
pub const X86_MSR_IA32_MCG_STATUS: u32 = 0x0000017a; /* global machine check status */
pub const X86_MSR_IA32_MISC_ENABLE: u32 = 0x000001a0; /* enable/disable misc processor features */
pub const X86_MSR_IA32_MISC_ENABLE_TURBO_DISABLE: u64 = 1 << 38;
pub const X86_MSR_IA32_TEMPERATURE_TARGET: u32 = 0x000001a2; /* Temperature target */
pub const X86_MSR_IA32_ENERGY_PERF_BIAS: u32 = 0x000001b0; /* Energy / Performance Bias */
pub const X86_MSR_IA32_MTRR_PHYSBASE0: u32 = 0x00000200; /* MTRR PhysBase0 */
pub const X86_MSR_IA32_MTRR_PHYSMASK0: u32 = 0x00000201; /* MTRR PhysMask0 */
pub const X86_MSR_IA32_MTRR_PHYSMASK9: u32 = 0x00000213; /* MTRR PhysMask9 */
pub const X86_MSR_IA32_MTRR_DEF_TYPE: u32 = 0x000002ff; /* MTRR default type */
pub const X86_MSR_IA32_MTRR_FIX64K_00000: u32 = 0x00000250; /* MTRR FIX64K_00000 */
pub const X86_MSR_IA32_MTRR_FIX16K_80000: u32 = 0x00000258; /* MTRR FIX16K_80000 */
pub const X86_MSR_IA32_MTRR_FIX16K_A0000: u32 = 0x00000259; /* MTRR FIX16K_A0000 */
pub const X86_MSR_IA32_MTRR_FIX4K_C0000: u32 = 0x00000268; /* MTRR FIX4K_C0000 */
pub const X86_MSR_IA32_MTRR_FIX4K_F8000: u32 = 0x0000026f; /* MTRR FIX4K_F8000 */
pub const X86_MSR_IA32_PAT: u32 = 0x00000277; /* PAT */
pub const X86_MSR_IA32_TSC_DEADLINE: u32 = 0x000006e0; /* TSC deadline */

pub const X86_MSR_IA32_X2APIC_APICID: u32 = 0x00000802; /* x2APIC ID Register (R/O) */
pub const X86_MSR_IA32_X2APIC_VERSION: u32 = 0x00000803; /* x2APIC Version Register (R/O) */
pub const X86_MSR_IA32_X2APIC_TPR: u32 = 0x00000808; /* x2APIC Task Priority Register (R/W) */
pub const X86_MSR_IA32_X2APIC_PPR: u32 = 0x0000080A; /* x2APIC Processor Priority Register (R/O) */
pub const X86_MSR_IA32_X2APIC_EOI: u32 = 0x0000080B; /* x2APIC EOI Register (W/O) */
pub const X86_MSR_IA32_X2APIC_LDR: u32 = 0x0000080D; /* x2APIC Logical Destination Register (R/O) */
pub const X86_MSR_IA32_X2APIC_SIVR: u32 = 0x0000080F; /* x2APIC Spurious Interrupt Vector Register (R/W) */
pub const X86_MSR_IA32_X2APIC_ISR0: u32 = 0x00000810; /* x2APIC In-Service Register Bits 31:0 (R/O) */
pub const X86_MSR_IA32_X2APIC_ISR1: u32 = 0x00000811; /* x2APIC In-Service Register Bits 63:32 (R/O) */
pub const X86_MSR_IA32_X2APIC_ISR2: u32 = 0x00000812; /* x2APIC In-Service Register Bits 95:64 (R/O) */
pub const X86_MSR_IA32_X2APIC_ISR3: u32 = 0x00000813; /* x2APIC In-Service Register Bits 127:96 (R/O) */
pub const X86_MSR_IA32_X2APIC_ISR4: u32 = 0x00000814; /* x2APIC In-Service Register Bits 159:128 (R/O) */
pub const X86_MSR_IA32_X2APIC_ISR5: u32 = 0x00000815; /* x2APIC In-Service Register Bits 191:160 (R/O) */
pub const X86_MSR_IA32_X2APIC_ISR6: u32 = 0x00000816; /* x2APIC In-Service Register Bits 223:192 (R/O) */
pub const X86_MSR_IA32_X2APIC_ISR7: u32 = 0x00000817; /* x2APIC In-Service Register Bits 255:224 (R/O) */
pub const X86_MSR_IA32_X2APIC_TMR0: u32 = 0x00000818; /* x2APIC Trigger Mode Register Bits 31:0 (R/O) */
pub const X86_MSR_IA32_X2APIC_TMR1: u32 = 0x00000819; /* x2APIC Trigger Mode Register Bits 63:32 (R/O) */
pub const X86_MSR_IA32_X2APIC_TMR2: u32 = 0x0000081A; /* x2APIC Trigger Mode Register Bits 95:64 (R/O) */
pub const X86_MSR_IA32_X2APIC_TMR3: u32 = 0x0000081B; /* x2APIC Trigger Mode Register Bits 127:96 (R/O) */
pub const X86_MSR_IA32_X2APIC_TMR4: u32 = 0x0000081C; /* x2APIC Trigger Mode Register Bits 159:128 (R/O) */
pub const X86_MSR_IA32_X2APIC_TMR5: u32 = 0x0000081D; /* x2APIC Trigger Mode Register Bits 191:160 (R/O) */
pub const X86_MSR_IA32_X2APIC_TMR6: u32 = 0x0000081E; /* x2APIC Trigger Mode Register Bits 223:192 (R/O) */
pub const X86_MSR_IA32_X2APIC_TMR7: u32 = 0x0000081F; /* x2APIC Trigger Mode Register Bits 255:224 (R/O) */
pub const X86_MSR_IA32_X2APIC_IRR0: u32 = 0x00000820; /* x2APIC Interrupt Request Register Bits 31:0 (R/O) */
pub const X86_MSR_IA32_X2APIC_IRR1: u32 = 0x00000821; /* x2APIC Interrupt Request Register Bits 63:32 (R/O) */
pub const X86_MSR_IA32_X2APIC_IRR2: u32 = 0x00000822; /* x2APIC Interrupt Request Register Bits 95:64 (R/O) */
pub const X86_MSR_IA32_X2APIC_IRR3: u32 = 0x00000823; /* x2APIC Interrupt Request Register Bits 127:96 (R/O) */
pub const X86_MSR_IA32_X2APIC_IRR4: u32 = 0x00000824; /* x2APIC Interrupt Request Register Bits 159:128 (R/O) */
pub const X86_MSR_IA32_X2APIC_IRR5: u32 = 0x00000825; /* x2APIC Interrupt Request Register Bits 191:160 (R/O) */
pub const X86_MSR_IA32_X2APIC_IRR6: u32 = 0x00000826; /* x2APIC Interrupt Request Register Bits 223:192 (R/O) */
pub const X86_MSR_IA32_X2APIC_IRR7: u32 = 0x00000827; /* x2APIC Interrupt Request Register Bits 255:224 (R/O) */
pub const X86_MSR_IA32_X2APIC_ESR: u32 = 0x00000828; /* x2APIC Error Status Register (R/W) */
pub const X86_MSR_IA32_X2APIC_LVT_CMCI: u32 = 0x0000082F; /* x2APIC LVT Corrected Machine Check Interrupt Register (R/W) */
pub const X86_MSR_IA32_X2APIC_ICR: u32 = 0x00000830; /* x2APIC Interrupt Command Register (R/W) */
pub const X86_MSR_IA32_X2APIC_LVT_TIMER: u32 = 0x00000832; /* x2APIC LVT Timer Interrupt Register (R/W) */
pub const X86_MSR_IA32_X2APIC_LVT_THERMAL: u32 = 0x00000833; /* x2APIC LVT Thermal Sensor Interrupt Register (R/W) */
pub const X86_MSR_IA32_X2APIC_LVT_PMI: u32 = 0x00000834; /* x2APIC LVT Performance Monitor Interrupt Register (R/W) */
pub const X86_MSR_IA32_X2APIC_LVT_LINT0: u32 = 0x00000835; /* x2APIC LVT LINT0 Register (R/W) */
pub const X86_MSR_IA32_X2APIC_LVT_LINT1: u32 = 0x00000836; /* x2APIC LVT LINT1 Register (R/W) */
pub const X86_MSR_IA32_X2APIC_LVT_ERROR: u32 = 0x00000837; /* x2APIC LVT Error Register (R/W) */
pub const X86_MSR_IA32_X2APIC_INIT_COUNT: u32 = 0x00000838; /* x2APIC Initial Count Register (R/W) */
pub const X86_MSR_IA32_X2APIC_CUR_COUNT: u32 = 0x00000839; /* x2APIC Current Count Register (R/O) */
pub const X86_MSR_IA32_X2APIC_DIV_CONF: u32 = 0x0000083E; /* x2APIC Divide Configuration Register (R/W) */
pub const X86_MSR_IA32_X2APIC_SELF_IPI: u32 = 0x0000083F; /* x2APIC Self IPI Register (W/O) */

pub const X86_MSR_IA32_EFER: u32 = 0xc0000080; /* EFER */
pub const X86_MSR_IA32_STAR: u32 = 0xc0000081; /* system call address */
pub const X86_MSR_IA32_LSTAR: u32 = 0xc0000082; /* long mode call address */
pub const X86_MSR_IA32_CSTAR: u32 = 0xc0000083; /* ia32-e compat call address */
pub const X86_MSR_IA32_FMASK: u32 = 0xc0000084; /* system call flag mask */
pub const X86_MSR_IA32_FS_BASE: u32 = 0xc0000100; /* fs base address */
pub const X86_MSR_IA32_GS_BASE: u32 = 0xc0000101; /* gs base address */
pub const X86_MSR_IA32_KERNEL_GS_BASE: u32 = 0xc0000102; /* kernel gs base */
pub const X86_MSR_IA32_TSC_AUX: u32 = 0xc0000103; /* TSC aux */
pub const X86_MSR_IA32_PM_ENABLE: u32 = 0x00000770; /* enable/disable HWP */
pub const X86_MSR_IA32_HWP_CAPABILITIES: u32 = 0x00000771; /* HWP performance range enumeration */
pub const X86_MSR_IA32_HWP_REQUEST: u32 = 0x00000774; /* power manage control hints */
pub const X86_MSR_AMD_VIRT_SPEC_CTRL: u32 = 0xc001011f; /* AMD speculative execution controls */
/* See IA32_SPEC_CTRL */
pub const X86_CR4_PSE: u64 = 0xffffffef; /* Disabling PSE bit in the CR4 */

// Non-architectural MSRs
pub const X86_MSR_POWER_CTL: u32 = 0x000001fc; /* Power Control Register */
pub const X86_MSR_RAPL_POWER_UNIT: u32 = 0x00000606; /* RAPL unit multipliers */
pub const X86_MSR_PKG_POWER_LIMIT: u32 = 0x00000610; /* Package power limits */
pub const X86_MSR_PKG_ENERGY_STATUS: u32 = 0x00000611; /* Package energy status */
pub const X86_MSR_PKG_POWER_INFO: u32 = 0x00000614; /* Package power range info */
pub const X86_MSR_DRAM_POWER_LIMIT: u32 = 0x00000618; /* DRAM RAPL power limit control */
pub const X86_MSR_DRAM_ENERGY_STATUS: u32 = 0x00000619; /* DRAM energy status */
pub const X86_MSR_PP0_POWER_LIMIT: u32 = 0x00000638; /* PP0 RAPL power limit control */
pub const X86_MSR_PP0_ENERGY_STATUS: u32 = 0x00000639; /* PP0 energy status */
pub const X86_MSR_PP1_POWER_LIMIT: u32 = 0x00000640; /* PP1 RAPL power limit control */
pub const X86_MSR_PP1_ENERGY_STATUS: u32 = 0x00000641; /* PP1 energy status */
pub const X86_MSR_PLATFORM_ENERGY_COUNTER: u32 = 0x0000064d; /* Platform energy counter */
pub const X86_MSR_PPERF: u32 = 0x0000064e; /* Productive performance count */
pub const X86_MSR_PERF_LIMIT_REASONS: u32 = 0x0000064f; /* Clipping cause register */
pub const X86_MSR_GFX_PERF_LIMIT_REASONS: u32 = 0x000006b0; /* Clipping cause register for graphics */
pub const X86_MSR_PLATFORM_POWER_LIMIT: u32 = 0x0000065c; /* Platform power limit control */
pub const X86_MSR_AMD_F10_DE_CFG: u32 = 0xc0011029; /* AMD Family 10h+ decode config */
pub const X86_MSR_AMD_F10_DE_CFG_LFENCE_SERIALIZE: u64 = 1 << 1;

pub const X86_MSR_AMD_LS_CFG: u32 = 0xc0011020; /* Load/store unit configuration */
pub const X86_AMD_LS_CFG_F15H_SSBD: u64 = 1 << 54;
pub const X86_AMD_LS_CFG_F16H_SSBD: u64 = 1 << 33;
pub const X86_AMD_LS_CFG_F17H_SSBD: u64 = 1 << 10;
pub const X86_MSR_K7_HWCR: u32 = 0xc0010015; /* AMD Hardware Configuration */
pub const X86_MSR_K7_HWCR_CPB_DISABLE: u64 = 1 << 25; /* Set to disable turbo ('boost') */

// KVM MSRs
pub const X86_MSR_KVM_PV_EOI_EN: u32 = 0x4b564d04; /* Enable paravirtual fast APIC EOI */
pub const X86_MSR_KVM_PV_EOI_EN_ENABLE: u64 = 1 << 0;

/* EFLAGS/RFLAGS */
pub const X86_FLAGS_CF: u64 = 1 << 0;
pub const X86_FLAGS_PF: u64 = 1 << 2;
pub const X86_FLAGS_AF: u64 = 1 << 4;
pub const X86_FLAGS_ZF: u64 = 1 << 6;
pub const X86_FLAGS_SF: u64 = 1 << 7;
pub const X86_FLAGS_TF: u64 = 1 << 8;
pub const X86_FLAGS_IF: u64 = 1 << 9;
pub const X86_FLAGS_DF: u64 = 1 << 10;
pub const X86_FLAGS_OF: u64 = 1 << 11;
pub const X86_FLAGS_STATUS_MASK: u64 = 0xfff;
pub const X86_FLAGS_IOPL_MASK: u64 = 3 << 12;
pub const X86_FLAGS_IOPL_SHIFT: u32 = 12;
pub const X86_FLAGS_NT: u64 = 1 << 14;
pub const X86_FLAGS_RF: u64 = 1 << 16;
pub const X86_FLAGS_VM: u64 = 1 << 17;
pub const X86_FLAGS_AC: u64 = 1 << 18;
pub const X86_FLAGS_VIF: u64 = 1 << 19;
pub const X86_FLAGS_VIP: u64 = 1 << 20;
pub const X86_FLAGS_ID: u64 = 1 << 21;
pub const X86_FLAGS_RESERVED_ONES: u64 = 0x2;
pub const X86_FLAGS_RESERVED: u64 = 0xffc0802a;
pub const X86_FLAGS_USER: u64 = X86_FLAGS_CF
    | X86_FLAGS_PF
    | X86_FLAGS_AF
    | X86_FLAGS_ZF
    | X86_FLAGS_SF
    | X86_FLAGS_TF
    | X86_FLAGS_DF
    | X86_FLAGS_OF
    | X86_FLAGS_NT
    | X86_FLAGS_AC
    | X86_FLAGS_ID;

/* DR6 */
pub const X86_DR6_B0: u64 = 1 << 0;
pub const X86_DR6_B1: u64 = 1 << 1;
pub const X86_DR6_B2: u64 = 1 << 2;
pub const X86_DR6_B3: u64 = 1 << 3;
pub const X86_DR6_BD: u64 = 1 << 13;
pub const X86_DR6_BS: u64 = 1 << 14;
pub const X86_DR6_BT: u64 = 1 << 15;

// NOTE: DR6 is used as a read-only status registers, and it is not writeable through userspace.
//       Any bits attempted to be written will be ignored.
pub const X86_DR6_USER_MASK: u64 =
    X86_DR6_B0 | X86_DR6_B1 | X86_DR6_B2 | X86_DR6_B3 | X86_DR6_BD | X86_DR6_BS | X86_DR6_BT;
/* Only bits in X86_DR6_USER_MASK are writeable.
 * Bits 12 and 32:63 must be written with 0, the rest as 1s */
pub const X86_DR6_MASK: u64 = 0xffff0ff0;

/* DR7 */
pub const X86_DR7_L0: u64 = 1 << 0;
pub const X86_DR7_G0: u64 = 1 << 1;
pub const X86_DR7_L1: u64 = 1 << 2;
pub const X86_DR7_G1: u64 = 1 << 3;
pub const X86_DR7_L2: u64 = 1 << 4;
pub const X86_DR7_G2: u64 = 1 << 5;
pub const X86_DR7_L3: u64 = 1 << 6;
pub const X86_DR7_G3: u64 = 1 << 7;
pub const X86_DR7_LE: u64 = 1 << 8;
pub const X86_DR7_GE: u64 = 1 << 9;
pub const X86_DR7_GD: u64 = 1 << 13;
pub const X86_DR7_RW0: u64 = 3 << 16;
pub const X86_DR7_LEN0: u64 = 3 << 18;
pub const X86_DR7_RW1: u64 = 3 << 20;
pub const X86_DR7_LEN1: u64 = 3 << 22;
pub const X86_DR7_RW2: u64 = 3 << 24;
pub const X86_DR7_LEN2: u64 = 3 << 26;
pub const X86_DR7_RW3: u64 = 3 << 28;
pub const X86_DR7_LEN3: u64 = 3 << 30;

// NOTE1: Even though the GD bit is writable, we disable it for the write_state syscall because it
//        complicates a lot the reasoning about how to access the registers. This is because
//        enabling this bit would make any other access to debug registers to issue an exception.
//        New syscalls should be define to lock/unlock debug registers.
// NOTE2: LE/GE bits are normally ignored, but the manual recommends always setting it to 1 in
//        order to be backwards compatible. Hence they are not writable from userspace.
pub const X86_DR7_USER_MASK: u64 = X86_DR7_L0
    | X86_DR7_G0
    | X86_DR7_L1
    | X86_DR7_G1
    | X86_DR7_L2
    | X86_DR7_G2
    | X86_DR7_L3
    | X86_DR7_G3
    | X86_DR7_RW0
    | X86_DR7_LEN0
    | X86_DR7_RW1
    | X86_DR7_LEN1
    | X86_DR7_RW2
    | X86_DR7_LEN2
    | X86_DR7_RW3
    | X86_DR7_LEN3;

/* Bits 11:12, 14:15 and 32:63 must be cleared to 0. Bit 10 must be set to 1. */
pub const X86_DR7_MASK: u64 = (1 << 10) | X86_DR7_LE | X86_DR7_GE;

pub const HW_DEBUG_REGISTERS_COUNT: usize = 4;

/* Indices of xsave feature states; state components are
 * enumerated in Intel Vol 1 section 13.1 */
pub const X86_XSAVE_STATE_INDEX_X87: u32 = 0;
pub const X86_XSAVE_STATE_INDEX_SSE: u32 = 1;
pub const X86_XSAVE_STATE_INDEX_AVX: u32 = 2;
pub const X86_XSAVE_STATE_INDEX_MPX_BNDREG: u32 = 3;
pub const X86_XSAVE_STATE_INDEX_MPX_BNDCSR: u32 = 4;
pub const X86_XSAVE_STATE_INDEX_AVX512_OPMASK: u32 = 5;
pub const X86_XSAVE_STATE_INDEX_AVX512_LOWERZMM_HIGH: u32 = 6;
pub const X86_XSAVE_STATE_INDEX_AVX512_HIGHERZMM: u32 = 7;
pub const X86_XSAVE_STATE_INDEX_PT: u32 = 8;
pub const X86_XSAVE_STATE_INDEX_PKRU: u32 = 9;

/* Bit masks for xsave feature states. */
pub const X86_XSAVE_STATE_BIT_X87: u64 = 1 << X86_XSAVE_STATE_INDEX_X87;
pub const X86_XSAVE_STATE_BIT_SSE: u64 = 1 << X86_XSAVE_STATE_INDEX_SSE;
pub const X86_XSAVE_STATE_BIT_AVX: u64 = 1 << X86_XSAVE_STATE_INDEX_AVX;
pub const X86_XSAVE_STATE_BIT_MPX_BNDREG: u64 = 1 << X86_XSAVE_STATE_INDEX_MPX_BNDREG;
pub const X86_XSAVE_STATE_BIT_MPX_BNDCSR: u64 = 1 << X86_XSAVE_STATE_INDEX_MPX_BNDCSR;
pub const X86_XSAVE_STATE_BIT_AVX512_OPMASK: u64 = 1 << X86_XSAVE_STATE_INDEX_AVX512_OPMASK;
pub const X86_XSAVE_STATE_BIT_AVX512_LOWERZMM_HIGH: u64 =
    1 << X86_XSAVE_STATE_INDEX_AVX512_LOWERZMM_HIGH;
pub const X86_XSAVE_STATE_BIT_AVX512_HIGHERZMM: u64 = 1 << X86_XSAVE_STATE_INDEX_AVX512_HIGHERZMM;
pub const X86_XSAVE_STATE_BIT_PT: u64 = 1 << X86_XSAVE_STATE_INDEX_PT;
pub const X86_XSAVE_STATE_BIT_PKRU: u64 = 1 << X86_XSAVE_STATE_INDEX_PKRU;

// Maximum buffer size needed for xsave and variants. To allocate, see ...BUFFER_SIZE below.
pub const X86_MAX_EXTENDED_REGISTER_SIZE: usize = 1024;
