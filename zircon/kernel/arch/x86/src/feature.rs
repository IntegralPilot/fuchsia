// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

pub const MAX_SUPPORTED_CPUID: u32 = 0x17;
pub const MAX_SUPPORTED_CPUID_HYP: u32 = 0x40000001;
pub const MAX_SUPPORTED_CPUID_EXT: u32 = 0x80000021;

pub const X86_MAX_CSTATES: usize = 12;

unsafe extern "C" {
    #[link_name = "_cpuid"]
    static mut CPUID: [CpuidLeaf; (MAX_SUPPORTED_CPUID + 1) as usize];

    #[link_name = "_cpuid_hyp"]
    static mut CPUID_HYP:
        [CpuidLeaf; (MAX_SUPPORTED_CPUID_HYP - X86CpuidLeafNum::HypVendor as u32 + 1) as usize];

    #[link_name = "_cpuid_ext"]
    static mut CPUID_EXT:
        [CpuidLeaf; (MAX_SUPPORTED_CPUID_EXT - X86CpuidLeafNum::ExtBase as u32 + 1) as usize];

    #[link_name = "max_cpuid"]
    static mut MAX_CPUID: u32;

    #[link_name = "max_hyp_cpuid"]
    static mut MAX_HYP_CPUID: u32;

    #[link_name = "max_ext_cpuid"]
    static mut MAX_EXT_CPUID: u32;

    #[link_name = "x86_vendor"]
    static mut X86_VENDOR: X86VendorList;

    #[link_name = "g_x86_feature_fsgsbase"]
    static mut G_X86_FEATURE_FSGSBASE: bool;

    #[link_name = "g_x86_feature_invpcid"]
    static mut G_X86_FEATURE_INVPCID: bool;

    #[link_name = "g_x86_feature_pcid_enabled"]
    static mut G_X86_FEATURE_PCID_ENABLED: bool;

    #[link_name = "g_x86_feature_has_smap"]
    static mut G_X86_FEATURE_HAS_SMAP: bool;

    #[link_name = "x86_hypervisor"]
    static mut X86_HYPERVISOR: X86HypervisorList;

    #[link_name = "g_hypervisor_has_pv_clock"]
    static mut HYPERVISOR_HAS_PV_CLOCK: bool;

    #[link_name = "g_hypervisor_has_pv_eoi"]
    static mut HYPERVISOR_HAS_PV_EOI: bool;

    #[link_name = "g_hypervisor_has_pv_ipi"]
    static mut HYPERVISOR_HAS_PV_IPI: bool;

    #[link_name = "x86_microarch_config"]
    static mut X86_MICROARCH_CONFIG: *const X86MicroarchConfig;

    #[link_name = "g_has_ibpb"]
    static mut G_HAS_IBPB: bool;

    #[link_name = "g_ras_fill_on_ctxt_switch"]
    static mut G_RAS_FILL_ON_CTXT_SWITCH: bool;

    #[link_name = "g_cpu_vulnerable_to_rsb_underflow"]
    static mut G_CPU_VULNERABLE_TO_RSB_UNDERFLOW: bool;

    #[link_name = "g_cpu_vulnerable_to_rsb_cross_thread"]
    static mut G_CPU_VULNERABLE_TO_RSB_CROSS_THREAD: bool;

    #[link_name = "g_should_ibpb_on_ctxt_switch"]
    static mut G_SHOULD_IBPB_ON_CTXT_SWITCH: bool;

    #[link_name = "g_ssb_mitigated"]
    static mut G_SSB_MITIGATED: bool;

    #[link_name = "g_l1d_flush_on_vmentry"]
    static mut G_L1D_FLUSH_ON_VMENTRY: bool;

    #[link_name = "g_md_clear_on_user_return"]
    static mut G_MD_CLEAR_ON_USER_RETURN: bool;

    #[link_name = "g_has_enhanced_ibrs"]
    static mut G_HAS_ENHANCED_IBRS: bool;

    #[link_name = "g_has_meltdown"]
    static mut G_HAS_MELTDOWN: bool;
}

macro_rules! x86_cpuid_bit {
    ($name:ident,$leaf:ident,$word:expr,$bit:expr) => {
        pub const $name: X86CpuidBit =
            X86CpuidBit { leaf_num: X86CpuidLeafNum::$leaf, word: $word, bit: $bit };
    };
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CpuidLeaf {
    pub a: u32,
    pub b: u32,
    pub c: u32,
    pub d: u32,
}

zr::static_assert!(core::mem::size_of::<CpuidLeaf>() == 16);
zr::static_assert!(core::mem::align_of::<CpuidLeaf>() == 4);

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X86CpuidLeafNum {
    Base = 0,
    ModelFeatures = 0x1,
    CacheV1 = 0x2,
    CacheV2 = 0x4,
    Mon = 0x5,
    ThermalAndPower = 0x6,
    ExtendedFeatureFlags = 0x7,
    PerformanceMonitoring = 0xa,
    Topology = 0xb,
    XSave = 0xd,
    Pt = 0x14,
    Tsc = 0x15,

    HypVendor = 0x40000000,
    KvmFeatures = 0x40000001,

    /// HypBase
    ExtBase = 0x80000000,
    ExtModelFeatures = 0x80000001,
    CpuidBrand = 0x80000002,
    ExtPowerManagement = 0x80000007,
    ExtAddressSizes = 0x80000008,
    AmdTopology = 0x8000001e,
}

zr::static_assert!(core::mem::size_of::<X86CpuidLeafNum>() == 4);
zr::static_assert!(core::mem::align_of::<X86CpuidLeafNum>() == 4);

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct X86CpuidBit {
    pub leaf_num: X86CpuidLeafNum,
    pub word: u8,
    pub bit: u8,
}

zr::static_assert!(core::mem::size_of::<X86CpuidBit>() == 8);
zr::static_assert!(core::mem::align_of::<X86CpuidBit>() == 4);

#[repr(u32)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum X86VendorList {
    #[default]
    Unknown = 0,
    Intel = 1,
    Amd = 2,
}

zr::static_assert!(core::mem::size_of::<X86VendorList>() == 4);
zr::static_assert!(core::mem::align_of::<X86VendorList>() == 4);

#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct X86ModelInfo {
    pub processor_type: u8,
    pub family: u8,
    pub model: u8,
    pub stepping: u8,

    pub display_family: u32,
    pub display_model: u32,

    pub patch_level: u32,
}

zr::static_assert!(core::mem::size_of::<X86ModelInfo>() == 16);
zr::static_assert!(core::mem::align_of::<X86ModelInfo>() == 4);

#[repr(u32)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum X86MicroarchList {
    #[default]
    Unknown = 0,
    IntelNehalem = 1,
    IntelWestmere = 2,
    IntelSandyBridge = 3,
    IntelIvyBridge = 4,
    IntelBroadwell = 5,
    IntelHaswell = 6,
    IntelSkylake = 7,
    IntelCannonlake = 8,
    IntelIcelake = 9,
    IntelTigerlake = 10,
    IntelAlderlake = 11,
    IntelSilvermont = 12,
    IntelGoldmont = 13,
    IntelGoldmontPlus = 14,
    AmdBulldozer = 15,
    AmdJaguar = 16,
    AmdZen = 17,
}

zr::static_assert!(core::mem::size_of::<X86MicroarchList>() == 4);
zr::static_assert!(core::mem::align_of::<X86MicroarchList>() == 4);

#[repr(u32)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum X86HypervisorList {
    #[default]
    Unknown = 0,
    None = 1,
    Kvm = 2,
}

zr::static_assert!(core::mem::size_of::<X86HypervisorList>() == 4);
zr::static_assert!(core::mem::align_of::<X86HypervisorList>() == 4);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct X86IdleStateT {
    pub name: *const core::ffi::c_char,
    pub mwait_hint: u32,
    pub exit_latency: u32,
    pub flushes_tlb: bool,
}

zr::static_assert!(core::mem::size_of::<X86IdleStateT>() == 24);
zr::static_assert!(core::mem::align_of::<X86IdleStateT>() == 8);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct X86IdleStatesT {
    pub states: [X86IdleStateT; X86_MAX_CSTATES],
    pub default_state_mask: u32,
}

zr::static_assert!(core::mem::size_of::<X86IdleStatesT>() == 296);
zr::static_assert!(core::mem::align_of::<X86IdleStatesT>() == 8);

pub type X86GetTimerFreqFuncT = Option<unsafe extern "C" fn() -> u64>;
pub type X86RebootSystemFuncT = Option<unsafe extern "C" fn()>;
pub type X86RebootReasonFuncT = Option<unsafe extern "C" fn(reason: u64)>;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct X86MicroarchConfig {
    pub x86_microarch: X86MicroarchList,
    pub get_apic_freq: X86GetTimerFreqFuncT,
    pub get_tsc_freq: X86GetTimerFreqFuncT,
    pub reboot_system: X86RebootSystemFuncT,
    pub reboot_reason: X86RebootReasonFuncT,
    pub disable_c1e: bool,
    pub idle_prefer_hlt: bool,
    pub idle_states: X86IdleStatesT,
}

zr::static_assert!(core::mem::size_of::<X86MicroarchConfig>() == 344);
zr::static_assert!(core::mem::align_of::<X86MicroarchConfig>() == 8);

x86_cpuid_bit! {X86_FEATURE_SSE3, ModelFeatures, 2, 0}
x86_cpuid_bit! {X86_FEATURE_MON, ModelFeatures, 2, 3}
x86_cpuid_bit! {X86_FEATURE_VMX, ModelFeatures, 2, 5}
x86_cpuid_bit! {X86_FEATURE_TM2, ModelFeatures, 2, 8}
x86_cpuid_bit! {X86_FEATURE_SSSE3, ModelFeatures, 2, 9}
x86_cpuid_bit! {X86_FEATURE_PDCM, ModelFeatures, 2, 15}
x86_cpuid_bit! {X86_FEATURE_PCID, ModelFeatures, 2, 17}
x86_cpuid_bit! {X86_FEATURE_SSE4_1, ModelFeatures, 2, 19}
x86_cpuid_bit! {X86_FEATURE_SSE4_2, ModelFeatures, 2, 20}
x86_cpuid_bit! {X86_FEATURE_X2APIC, ModelFeatures, 2, 21}
x86_cpuid_bit! {X86_FEATURE_TSC_DEADLINE, ModelFeatures, 2, 24}
x86_cpuid_bit! {X86_FEATURE_AESNI, ModelFeatures, 2, 25}
x86_cpuid_bit! {X86_FEATURE_XSAVE, ModelFeatures, 2, 26}
x86_cpuid_bit! {X86_FEATURE_AVX, ModelFeatures, 2, 28}
x86_cpuid_bit! {X86_FEATURE_RDRAND, ModelFeatures, 2, 30}
x86_cpuid_bit! {X86_FEATURE_HYPERVISOR, ModelFeatures, 2, 31}
x86_cpuid_bit! {X86_FEATURE_FPU, ModelFeatures, 3, 0}
x86_cpuid_bit! {X86_FEATURE_SEP, ModelFeatures, 3, 11}
x86_cpuid_bit! {X86_FEATURE_CLFLUSH, ModelFeatures, 3, 19}
x86_cpuid_bit! {X86_FEATURE_ACPI, ModelFeatures, 3, 22}
x86_cpuid_bit! {X86_FEATURE_MMX, ModelFeatures, 3, 23}
x86_cpuid_bit! {X86_FEATURE_FXSR, ModelFeatures, 3, 24}
x86_cpuid_bit! {X86_FEATURE_SSE, ModelFeatures, 3, 25}
x86_cpuid_bit! {X86_FEATURE_SSE2, ModelFeatures, 3, 26}
x86_cpuid_bit! {X86_FEATURE_TM, ModelFeatures, 3, 29}
x86_cpuid_bit! {X86_FEATURE_DTS, ThermalAndPower, 0, 0}
x86_cpuid_bit! {X86_FEATURE_TURBO, ThermalAndPower, 0, 1}
x86_cpuid_bit! {X86_FEATURE_PLN, ThermalAndPower, 0, 4}
x86_cpuid_bit! {X86_FEATURE_PTM, ThermalAndPower, 0, 6}
x86_cpuid_bit! {X86_FEATURE_HWP, ThermalAndPower, 0, 7}
x86_cpuid_bit! {X86_FEATURE_HWP_NOT, ThermalAndPower, 0, 8}
x86_cpuid_bit! {X86_FEATURE_HWP_ACT, ThermalAndPower, 0, 9}
x86_cpuid_bit! {X86_FEATURE_HWP_PREF, ThermalAndPower, 0, 10}
x86_cpuid_bit! {X86_FEATURE_TURBO_MAX, ThermalAndPower, 0, 14}
x86_cpuid_bit! {X86_FEATURE_HW_FEEDBACK, ThermalAndPower, 2, 0}
x86_cpuid_bit! {X86_FEATURE_PERF_BIAS, ThermalAndPower, 2, 3}
x86_cpuid_bit! {X86_FEATURE_FSGSBASE, ExtendedFeatureFlags, 1, 0}
x86_cpuid_bit! {X86_FEATURE_TSC_ADJUST, ExtendedFeatureFlags, 1, 1}
x86_cpuid_bit! {X86_FEATURE_AVX2, ExtendedFeatureFlags, 1, 5}
x86_cpuid_bit! {X86_FEATURE_SMEP, ExtendedFeatureFlags, 1, 7}
x86_cpuid_bit! {X86_FEATURE_ERMS, ExtendedFeatureFlags, 1, 9}
x86_cpuid_bit! {X86_FEATURE_INVPCID, ExtendedFeatureFlags, 1, 10}
x86_cpuid_bit! {X86_FEATURE_AVX512F, ExtendedFeatureFlags, 1, 16}
x86_cpuid_bit! {X86_FEATURE_AVX512DQ, ExtendedFeatureFlags, 1, 17}
x86_cpuid_bit! {X86_FEATURE_RDSEED, ExtendedFeatureFlags, 1, 18}
x86_cpuid_bit! {X86_FEATURE_SMAP, ExtendedFeatureFlags, 1, 20}
x86_cpuid_bit! {X86_FEATURE_AVX512IFMA, ExtendedFeatureFlags, 1, 21}
x86_cpuid_bit! {X86_FEATURE_CLFLUSHOPT, ExtendedFeatureFlags, 1, 23}
x86_cpuid_bit! {X86_FEATURE_CLWB, ExtendedFeatureFlags, 1, 24}
x86_cpuid_bit! {X86_FEATURE_PT, ExtendedFeatureFlags, 1, 25}
x86_cpuid_bit! {X86_FEATURE_AVX512PF, ExtendedFeatureFlags, 1, 26}
x86_cpuid_bit! {X86_FEATURE_AVX512ER, ExtendedFeatureFlags, 1, 27}
x86_cpuid_bit! {X86_FEATURE_AVX512CD, ExtendedFeatureFlags, 1, 28}
x86_cpuid_bit! {X86_FEATURE_AVX512BW, ExtendedFeatureFlags, 1, 30}
x86_cpuid_bit! {X86_FEATURE_AVX512VL, ExtendedFeatureFlags, 1, 31}
x86_cpuid_bit! {X86_FEATURE_AVX512VBMI, ExtendedFeatureFlags, 2, 1}
x86_cpuid_bit! {X86_FEATURE_UMIP, ExtendedFeatureFlags, 2, 2}
x86_cpuid_bit! {X86_FEATURE_PKU, ExtendedFeatureFlags, 2, 3}
x86_cpuid_bit! {X86_FEATURE_AVX512VBMI2, ExtendedFeatureFlags, 2, 6}
x86_cpuid_bit! {X86_FEATURE_AVX512VNNI, ExtendedFeatureFlags, 2, 11}
x86_cpuid_bit! {X86_FEATURE_AVX512BITALG, ExtendedFeatureFlags, 2, 12}
x86_cpuid_bit! {X86_FEATURE_AVX512VPDQ, ExtendedFeatureFlags, 2, 14}
x86_cpuid_bit! {X86_FEATURE_AVX512QVNNIW, ExtendedFeatureFlags, 3, 2}
x86_cpuid_bit! {X86_FEATURE_AVX512QFMA, ExtendedFeatureFlags, 3, 3}
x86_cpuid_bit! {X86_FEATURE_MD_CLEAR, ExtendedFeatureFlags, 3, 10}
x86_cpuid_bit! {X86_FEATURE_IBRS_IBPB, ExtendedFeatureFlags, 3, 26}
x86_cpuid_bit! {X86_FEATURE_STIBP, ExtendedFeatureFlags, 3, 27}
x86_cpuid_bit! {X86_FEATURE_L1D_FLUSH, ExtendedFeatureFlags, 3, 28}
x86_cpuid_bit! {X86_FEATURE_ARCH_CAPABILITIES, ExtendedFeatureFlags, 3, 29}
x86_cpuid_bit! {X86_FEATURE_SSBD, ExtendedFeatureFlags, 3, 31}
x86_cpuid_bit! {X86_FEATURE_TOPOLOGY_SHIFT, Topology, 0, 0}
x86_cpuid_bit! {X86_FEATURE_TOPOLOGY_SINGLE_PROCESSOR, Topology, 0, 1}
x86_cpuid_bit! {X86_FEATURE_TOPOLOGY_LOGICAL_PROCESSOR, Topology, 0, 8}
x86_cpuid_bit! {X86_FEATURE_TOPOLOGY_CORE, Topology, 0, 9}
x86_cpuid_bit! {X86_FEATURE_KVM_PV_CLOCK, KvmFeatures, 0, 3}
x86_cpuid_bit! {X86_FEATURE_KVM_PV_EOI, KvmFeatures, 0, 6}
x86_cpuid_bit! {X86_FEATURE_KVM_PV_IPI, KvmFeatures, 0, 11}
x86_cpuid_bit! {X86_FEATURE_KVM_PV_CLOCK_STABLE, KvmFeatures, 0, 24}
x86_cpuid_bit! {X86_FEATURE_AMD_TOPO, ExtModelFeatures, 2, 22}
x86_cpuid_bit! {X86_FEATURE_SYSCALL, ExtModelFeatures, 3, 11}
x86_cpuid_bit! {X86_FEATURE_NX, ExtModelFeatures, 3, 20}
x86_cpuid_bit! {X86_FEATURE_HUGE_PAGE, ExtModelFeatures, 3, 26}
x86_cpuid_bit! {X86_FEATURE_RDTSCP, ExtModelFeatures, 3, 27}
x86_cpuid_bit! {X86_FEATURE_INVAR_TSC, ExtPowerManagement, 3, 8}
x86_cpuid_bit! {X86_FEATURE_INVLPGB, ExtAddressSizes, 1, 3}

pub fn x86_get_cpuid_leaf(leaf: X86CpuidLeafNum) -> Option<&'static CpuidLeaf> {
    let leaf_val = leaf as u32;
    // SAFETY: Values are only written to during single-threaded startup.
    unsafe {
        if leaf_val < X86CpuidLeafNum::HypVendor as u32 {
            let max = core::ptr::addr_of!(MAX_CPUID).read();
            if leaf_val > max {
                return None;
            }
            let ptr = core::ptr::addr_of!(CPUID).cast::<CpuidLeaf>();
            Some(&*ptr.add(leaf_val as usize))
        } else if leaf_val < X86CpuidLeafNum::ExtBase as u32 {
            let max_hyp = core::ptr::addr_of!(MAX_HYP_CPUID).read();
            if leaf_val > max_hyp {
                return None;
            }
            let offset = (leaf_val - X86CpuidLeafNum::HypVendor as u32) as usize;
            let ptr = core::ptr::addr_of!(CPUID_HYP).cast::<CpuidLeaf>();
            Some(&*ptr.add(offset))
        } else {
            let max_ext = core::ptr::addr_of!(MAX_EXT_CPUID).read();
            if leaf_val > max_ext {
                return None;
            }
            let offset = (leaf_val - X86CpuidLeafNum::ExtBase as u32) as usize;
            let ptr = core::ptr::addr_of!(CPUID_EXT).cast::<CpuidLeaf>();
            Some(&*ptr.add(offset))
        }
    }
}

pub fn x86_feature_test(bit: X86CpuidBit) -> bool {
    debug_assert!(bit.word <= 3 && bit.bit <= 31);
    if bit.word > 3 || bit.bit > 31 {
        return false;
    }
    let Some(leaf) = x86_get_cpuid_leaf(bit.leaf_num) else {
        return false;
    };
    let val = match bit.word {
        0 => leaf.a,
        1 => leaf.b,
        2 => leaf.c,
        3 => leaf.d,
        _ => return false,
    };
    ((1u32 << bit.bit) & val) != 0
}

#[inline]
pub fn x86_get_vendor() -> X86VendorList {
    // SAFETY: Value is only written to during single threaded startup
    unsafe { core::ptr::addr_of!(X86_VENDOR).read() }
}

#[inline]
pub fn x86_hypervisor_has_pv_clock() -> bool {
    // SAFETY: Value is only written to during single threaded startup
    unsafe { core::ptr::addr_of!(HYPERVISOR_HAS_PV_CLOCK).read() }
}

#[inline]
pub fn x86_hypervisor_has_pv_eoi() -> bool {
    // SAFETY: Value is only written to during single threaded startup
    unsafe { core::ptr::addr_of!(HYPERVISOR_HAS_PV_EOI).read() }
}

#[inline]
pub fn x86_hypervisor_has_pv_ipi() -> bool {
    // SAFETY: Value is only written to during single threaded startup
    unsafe { core::ptr::addr_of!(HYPERVISOR_HAS_PV_IPI).read() }
}

#[inline]
pub fn x86_has_hypervisor() -> bool {
    // SAFETY: Value is only written to during single threaded startup
    unsafe { core::ptr::addr_of!(X86_HYPERVISOR).read() != X86HypervisorList::None }
}

#[inline]
pub fn x86_get_hypervisor() -> X86HypervisorList {
    // SAFETY: Value is only written to during single threaded startup
    unsafe { core::ptr::addr_of!(X86_HYPERVISOR).read() }
}

#[inline]
pub fn x86_get_microarch_config() -> Option<&'static X86MicroarchConfig> {
    // SAFETY: Value is only written to during single threaded startup
    unsafe {
        let ptr = core::ptr::addr_of!(X86_MICROARCH_CONFIG).read();
        ptr.as_ref()
    }
}

#[inline]
pub fn x86_cpu_has_ibpb() -> bool {
    // SAFETY: Value is only written to during single threaded startup
    unsafe { core::ptr::addr_of!(G_HAS_IBPB).read() }
}

#[inline]
pub fn x86_cpu_should_ras_fill_on_ctxt_switch() -> bool {
    // SAFETY: Value is only written to during single threaded startup
    unsafe { core::ptr::addr_of!(G_RAS_FILL_ON_CTXT_SWITCH).read() }
}

#[inline]
pub fn x86_cpu_vulnerable_to_rsb_cross_thread() -> bool {
    // SAFETY: Value is only written to during single threaded startup
    unsafe { core::ptr::addr_of!(G_CPU_VULNERABLE_TO_RSB_CROSS_THREAD).read() }
}

#[inline]
pub fn x86_cpu_vulnerable_to_rsb_underflow() -> bool {
    // SAFETY: Value is only written to during single threaded startup
    unsafe { core::ptr::addr_of!(G_CPU_VULNERABLE_TO_RSB_UNDERFLOW).read() }
}

#[inline]
pub fn x86_cpu_should_ibpb_on_ctxt_switch() -> bool {
    // SAFETY: Value is only written to during single threaded startup
    unsafe { core::ptr::addr_of!(G_SHOULD_IBPB_ON_CTXT_SWITCH).read() }
}

#[inline]
pub fn x86_cpu_should_mitigate_ssb() -> bool {
    // SAFETY: Value is only written to during single threaded startup
    unsafe { core::ptr::addr_of!(G_SSB_MITIGATED).read() }
}

#[inline]
pub fn x86_cpu_should_l1d_flush_on_vmentry() -> bool {
    // SAFETY: Value is only written to during single threaded startup
    unsafe { core::ptr::addr_of!(G_L1D_FLUSH_ON_VMENTRY).read() }
}

#[inline]
pub fn x86_cpu_should_md_clear_on_user_return() -> bool {
    // SAFETY: Value is only written to during single threaded startup
    unsafe { core::ptr::addr_of!(G_MD_CLEAR_ON_USER_RETURN).read() }
}

#[inline]
pub fn x86_cpu_has_enhanced_ibrs() -> bool {
    // SAFETY: Value is only written to during single threaded startup
    unsafe { core::ptr::addr_of!(G_HAS_ENHANCED_IBRS).read() }
}

#[inline]
pub fn x86_cpu_has_meltdown() -> bool {
    // SAFETY: Value is only written to during single threaded startup
    unsafe { core::ptr::addr_of!(G_HAS_MELTDOWN).read() }
}
