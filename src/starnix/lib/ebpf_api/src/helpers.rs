// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::MapKey;
use crate::maps::{Map, MapValueRef, RingBuffer, RingBufferWakeupPolicy};
use ebpf::{BpfValue, EbpfBufferPtr, EbpfHelperImpl, EbpfProgramContext, FromBpfValue, HelperSet};
use inspect_stubs::track_stub;
use linux_uapi::{
    BPF_SK_STORAGE_GET_F_CREATE, bpf_func_id_BPF_FUNC_get_current_pid_tgid,
    bpf_func_id_BPF_FUNC_get_current_uid_gid, bpf_func_id_BPF_FUNC_get_netns_cookie,
    bpf_func_id_BPF_FUNC_get_retval, bpf_func_id_BPF_FUNC_get_smp_processor_id,
    bpf_func_id_BPF_FUNC_get_socket_cookie, bpf_func_id_BPF_FUNC_get_socket_uid,
    bpf_func_id_BPF_FUNC_ktime_get_boot_ns, bpf_func_id_BPF_FUNC_ktime_get_coarse_ns,
    bpf_func_id_BPF_FUNC_ktime_get_ns, bpf_func_id_BPF_FUNC_map_delete_elem,
    bpf_func_id_BPF_FUNC_map_lookup_elem, bpf_func_id_BPF_FUNC_map_update_elem,
    bpf_func_id_BPF_FUNC_probe_read_str, bpf_func_id_BPF_FUNC_probe_read_user,
    bpf_func_id_BPF_FUNC_probe_read_user_str, bpf_func_id_BPF_FUNC_ringbuf_discard,
    bpf_func_id_BPF_FUNC_ringbuf_reserve, bpf_func_id_BPF_FUNC_ringbuf_submit,
    bpf_func_id_BPF_FUNC_set_retval, bpf_func_id_BPF_FUNC_sk_fullsock,
    bpf_func_id_BPF_FUNC_sk_lookup_tcp, bpf_func_id_BPF_FUNC_sk_lookup_udp,
    bpf_func_id_BPF_FUNC_sk_release, bpf_func_id_BPF_FUNC_sk_storage_get,
    bpf_func_id_BPF_FUNC_skb_load_bytes, bpf_func_id_BPF_FUNC_skb_load_bytes_relative,
    bpf_func_id_BPF_FUNC_trace_printk, bpf_map_type_BPF_MAP_TYPE_RINGBUF,
    bpf_map_type_BPF_MAP_TYPE_SK_STORAGE, gid_t, pid_t, uid_t,
};
use smallvec::SmallVec;
use zerocopy::IntoBytes as _;

pub trait MapsContext<'a> {
    fn on_map_access(&mut self, map: &Map);
    fn add_value_ref(&mut self, map_ref: MapValueRef<'a>);
}

pub trait MapsProgramContext: EbpfProgramContext {
    fn on_map_access(context: &mut Self::RunContext<'_>, map: &Map);
    fn add_value_ref<'a>(context: &mut Self::RunContext<'a>, map_ref: MapValueRef<'a>);
}

impl<C: EbpfProgramContext> MapsProgramContext for C
where
    for<'a> C::RunContext<'a>: MapsContext<'a>,
{
    fn on_map_access(context: &mut Self::RunContext<'_>, map: &Map) {
        context.on_map_access(map);
    }

    fn add_value_ref<'a>(context: &mut Self::RunContext<'a>, map_ref: MapValueRef<'a>) {
        context.add_value_ref(map_ref);
    }
}

fn bpf_map_lookup_elem<'a, C: MapsProgramContext>(
    context: &mut C::RunContext<'a>,
    map: BpfValue,
    key: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
) -> BpfValue {
    // SAFETY: The `map` must be a reference to a `Map` object kept alive by the program itself.
    let map: &Map = unsafe { &*map.as_ptr::<Map>() };

    // SAFETY: safety is ensured by the verifier.
    let key = unsafe { EbpfBufferPtr::new(key.as_ptr::<u8>(), map.schema.key_size as usize) };
    let key: MapKey = key.load();

    C::on_map_access(context, map);

    let Some(value_ref) = map.lookup(&key) else {
        return BpfValue::default();
    };

    let result: BpfValue = value_ref.ptr().raw_ptr().into();

    // If this is a map with ref-counted elements then save the reference for
    // the lifetime of the program.
    if value_ref.is_ref_counted() {
        C::add_value_ref(context, value_ref);
    }

    result
}

fn bpf_map_update_elem<C: MapsProgramContext>(
    context: &mut C::RunContext<'_>,
    map: BpfValue,
    key: BpfValue,
    value: BpfValue,
    flags: BpfValue,
    _: BpfValue,
) -> BpfValue {
    // SAFETY: The `map` must be a reference to a `Map` object kept alive by the program itself.
    let map: &Map = unsafe { &*map.as_ptr::<Map>() };

    // TODO(https://fxbug.dev/496639039): This should be checked by the verifier.
    if map.schema.map_type == bpf_map_type_BPF_MAP_TYPE_SK_STORAGE {
        return BpfValue::default();
    }

    // SAFETY: safety is ensured by the verifier.
    let key = unsafe { EbpfBufferPtr::new(key.as_ptr::<u8>(), map.schema.key_size as usize) };
    let key: MapKey = key.load();

    // SAFETY: safety is ensured by the verifier.
    let value = unsafe { EbpfBufferPtr::new(value.as_ptr::<u8>(), map.schema.value_size as usize) };
    let flags = flags.as_u64();

    C::on_map_access(context, map);

    map.update(&key, value, flags).map(|_| 0).unwrap_or(u64::MAX).into()
}

fn bpf_map_delete_elem<C: MapsProgramContext>(
    context: &mut C::RunContext<'_>,
    map: BpfValue,
    key: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
) -> BpfValue {
    // SAFETY: The `map` must be a reference to a `Map` object kept alive by the program itself.
    let map: &Map = unsafe { &*map.as_ptr::<Map>() };

    // TODO(https://fxbug.dev/496639039): This should be checked by the verifier.
    if map.schema.map_type == bpf_map_type_BPF_MAP_TYPE_SK_STORAGE {
        return BpfValue::default();
    }

    // SAFETY: safety is ensured by the verifier.
    let key = unsafe { EbpfBufferPtr::new(key.as_ptr::<u8>(), map.schema.key_size as usize) };
    let key: MapKey = key.load();

    C::on_map_access(context, map);

    map.delete(&key).map(|_| 0).unwrap_or(u64::MAX).into()
}

fn bpf_trace_printk<C: EbpfProgramContext>(
    _context: &mut C::RunContext<'_>,
    _fmt: BpfValue,
    _fmt_size: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
) -> BpfValue {
    track_stub!(TODO("https://fxbug.dev/534355500"), "bpf_trace_printk");
    0.into()
}

fn bpf_ktime_get_ns<C: EbpfProgramContext>(
    _context: &mut C::RunContext<'_>,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
) -> BpfValue {
    zx::MonotonicInstant::get().into_nanos().into()
}

fn bpf_ringbuf_reserve<C: EbpfProgramContext>(
    _context: &mut C::RunContext<'_>,
    map: BpfValue,
    size: BpfValue,
    flags: BpfValue,
    _: BpfValue,
    _: BpfValue,
) -> BpfValue {
    // SAFETY: The safety of the operation is ensured by the bpf verifier. The `map` must be a
    // reference to a `Map` object kept alive by the program itself.
    let map: &Map = unsafe { &*map.as_ptr::<Map>() };

    // Map type is checked by the verifier.
    assert!(map.schema.map_type == bpf_map_type_BPF_MAP_TYPE_RINGBUF);

    let Ok(size) = u32::try_from(size) else {
        return BpfValue::default();
    };
    let flags = u64::from(flags);
    map.ringbuf_reserve(size, flags).map(BpfValue::from).unwrap_or_else(|_| BpfValue::default())
}

fn bpf_ringbuf_submit<C: EbpfProgramContext>(
    _context: &mut C::RunContext<'_>,
    data: BpfValue,
    flags: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
) -> BpfValue {
    let flags = RingBufferWakeupPolicy::from(flags);

    // SAFETY: The safety of the operation is ensured by the bpf verifier. The data has to come from
    // the result of a reserve call.
    unsafe {
        RingBuffer::submit(u64::from(data), flags);
    }
    0.into()
}

fn bpf_ringbuf_discard<C: EbpfProgramContext>(
    _context: &mut C::RunContext<'_>,
    data: BpfValue,
    flags: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
) -> BpfValue {
    let flags = RingBufferWakeupPolicy::from(flags);

    // SAFETY: The safety of the operation is ensured by the bpf verifier. The data has to come from
    // the result of a reserve call.
    unsafe {
        RingBuffer::discard(u64::from(data), flags);
    }
    0.into()
}

fn bpf_ktime_get_boot_ns<C: EbpfProgramContext>(
    _context: &mut C::RunContext<'_>,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
) -> BpfValue {
    track_stub!(TODO("https://fxbug.dev/534355721"), "bpf_ktime_get_boot_ns");
    0.into()
}

fn bpf_probe_read_user<C: EbpfProgramContext>(
    _context: &mut C::RunContext<'_>,
    dst: BpfValue,
    size: BpfValue,
    _src: BpfValue,
    _: BpfValue,
    _: BpfValue,
) -> BpfValue {
    track_stub!(TODO("https://fxbug.dev/534354547"), "bpf_probe_read_user");
    // The real helper copies `size` bytes from user memory into `dst`. Until
    // that is implemented, zero `dst`: the verifier models it as written
    // (`output: true`), so leaving it untouched would let the program read back
    // uninitialized executor memory.
    let size = size.as_usize();
    if size > 0 {
        // SAFETY: the verifier guarantees `dst` points to `size` writable bytes
        // (MemoryParameter { output: true, size: Reference { index: 1 } }).
        unsafe { std::ptr::write_bytes(dst.as_ptr::<u8>(), 0, size) };
    }
    0.into()
}

fn bpf_probe_read_user_str<C: EbpfProgramContext>(
    _context: &mut C::RunContext<'_>,
    dst: BpfValue,
    size: BpfValue,
    _src: BpfValue,
    _: BpfValue,
    _: BpfValue,
) -> BpfValue {
    track_stub!(TODO("https://fxbug.dev/534355539"), "bpf_probe_read_user_str");
    // The real helper copies a NUL-terminated string from user memory into
    // `dst`. Until that is implemented, zero `dst`: the verifier models it as
    // written (`output: true`), so leaving it untouched would let the program
    // read back uninitialized executor memory.
    let size = size.as_usize();
    if size > 0 {
        // SAFETY: the verifier guarantees `dst` points to `size` writable bytes
        // (MemoryParameter { output: true, size: Reference { index: 1 } }).
        unsafe { std::ptr::write_bytes(dst.as_ptr::<u8>(), 0, size) };
    }
    0.into()
}

fn bpf_ktime_get_coarse_ns<C: EbpfProgramContext>(
    _context: &mut C::RunContext<'_>,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
) -> BpfValue {
    track_stub!(TODO("https://fxbug.dev/534355976"), "bpf_ktime_get_coarse_ns");
    0.into()
}

fn bpf_probe_read_str<C: EbpfProgramContext>(
    _context: &mut C::RunContext<'_>,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
) -> BpfValue {
    track_stub!(TODO("https://fxbug.dev/534355560"), "bpf_probe_read_str");
    0.into()
}

fn bpf_get_smp_processor_id<C: EbpfProgramContext>(
    _context: &mut C::RunContext<'_>,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
) -> BpfValue {
    track_stub!(TODO("https://fxbug.dev/534354909"), "bpf_get_smp_processor_id");
    0.into()
}

pub trait CurrentTaskContext {
    fn get_uid_gid(&self) -> (uid_t, gid_t);
    fn get_tid_tgid(&self) -> (pid_t, pid_t);
}

pub trait CurrentTaskProgramContext: EbpfProgramContext {
    fn get_uid_gid<'a>(context: &mut Self::RunContext<'a>) -> (uid_t, gid_t);
    fn get_tid_tgid<'a>(context: &mut Self::RunContext<'a>) -> (pid_t, pid_t);
}

impl<C: EbpfProgramContext> CurrentTaskProgramContext for C
where
    for<'a> C::RunContext<'a>: CurrentTaskContext,
{
    fn get_uid_gid<'a>(context: &mut Self::RunContext<'a>) -> (uid_t, gid_t) {
        context.get_uid_gid()
    }
    fn get_tid_tgid<'a>(context: &mut Self::RunContext<'a>) -> (pid_t, pid_t) {
        context.get_tid_tgid()
    }
}

fn bpf_get_current_uid_gid<C: CurrentTaskProgramContext>(
    context: &mut C::RunContext<'_>,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
) -> BpfValue {
    let (uid, gid) = C::get_uid_gid(context);
    (uid as u64 | (gid as u64) << 32).into()
}

fn bpf_get_current_pid_tgid<C: CurrentTaskProgramContext>(
    context: &mut C::RunContext<'_>,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
) -> BpfValue {
    let (pid, tgid) = C::get_tid_tgid(context);
    (pid as u64 | (tgid as u64) << 32).into()
}

// Trait for `EbpfProgramContext` where the first argument is a `SocketRef`,
// i.e. it references a socket.
pub trait Arg1IsSocketProgramContext: EbpfProgramContext {
    type Arg1AsSocket<'a>: FromBpfValue<Self::RunContext<'a>> + SocketRef;
}

impl<C> Arg1IsSocketProgramContext for C
where
    C: EbpfProgramContext,
    for<'a> Self::Arg1<'a>: FromBpfValue<Self::RunContext<'a>> + SocketRef,
{
    type Arg1AsSocket<'a> = Self::Arg1<'a>;
}

// Marker trait for `EbpfProgramContext` that supports `bpf_get_socket_uid`.
pub trait SocketCookieProgramContext: Arg1IsSocketProgramContext {}
impl<C> SocketCookieProgramContext for C where C: Arg1IsSocketProgramContext {}

fn bpf_get_socket_cookie<'a, C: SocketCookieProgramContext>(
    context: &mut C::RunContext<'a>,
    arg1: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
) -> BpfValue {
    // SAFETY: Verifier checks that the argument points at the value that
    // that's passed as the first argument.
    let arg1_as_socket = unsafe { C::Arg1AsSocket::from_bpf_value(context, arg1) };
    arg1_as_socket.get_socket_cookie().unwrap_or(0).into()
}

pub trait SocketRef {
    fn get_socket_cookie(&self) -> Option<u64>;
    fn get_socket_uid(&self) -> Option<uid_t>;
}

// A trait for eBPF run context with `bpf_sock` pointers.
pub trait BpfSockContext: Sized {
    type BpfSockRef: SocketRef + FromBpfValue<Self>;
}

pub trait SkStorageProgramContext: EbpfProgramContext {
    type BpfSockRef<'a>: SocketRef + FromBpfValue<Self::RunContext<'a>>;
}

impl<C> SkStorageProgramContext for C
where
    C: EbpfProgramContext,
    for<'a> C::RunContext<'a>: BpfSockContext,
{
    type BpfSockRef<'a> = <C::RunContext<'a> as BpfSockContext>::BpfSockRef;
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LoadBytesBase {
    MacHeader,
    NetworkHeader,
}

// Marker trait for `EbpfProgramContext` that supports `bpf_get_socket_uid`.
pub trait SocketUidProgramContext: Arg1IsSocketProgramContext {}
impl<C> SocketUidProgramContext for C where C: Arg1IsSocketProgramContext {}

fn bpf_get_socket_uid<'a, C: SocketUidProgramContext>(
    context: &mut C::RunContext<'a>,
    sk_buf: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
) -> BpfValue {
    const OVERFLOW_UID: uid_t = 65534;
    // SAFETY: Verifier checks that the first argument points at a `__sk_buff`.
    let sk_buf = unsafe { C::Arg1AsSocket::from_bpf_value(context, sk_buf) };
    sk_buf.get_socket_uid().unwrap_or(OVERFLOW_UID).into()
}

// Trait for packets that support `bpf_load_bytes_relative`.
pub trait PacketWithLoadBytes {
    fn load_bytes_relative(
        &self,
        base: LoadBytesBase,
        offset: usize,
        buf: EbpfBufferPtr<'_>,
    ) -> i64;
}

// Trait for `EbpfProgramContext` that supports `bpf_load_bytes_relative`.
pub trait SkbLoadBytesProgramContext: EbpfProgramContext {
    fn skb_load_bytes_relative<'a>(
        context: &mut Self::RunContext<'a>,
        sk_buf: BpfValue,
        base: LoadBytesBase,
        offset: usize,
        buf: EbpfBufferPtr<'_>,
    ) -> i64;
}

impl<C: EbpfProgramContext> SkbLoadBytesProgramContext for C
where
    for<'b> C::Arg1<'b>: FromBpfValue<C::RunContext<'b>>,
    for<'b> C::Arg1<'b>: PacketWithLoadBytes,
{
    fn skb_load_bytes_relative<'a>(
        context: &mut Self::RunContext<'a>,
        sk_buf: BpfValue,
        base: LoadBytesBase,
        offset: usize,
        buf: EbpfBufferPtr<'_>,
    ) -> i64 {
        // SAFETY: Verifier checks that the argument points at the same value
        // that was passed to the program as the first argument.
        let sk_buf = unsafe { C::Arg1::from_bpf_value(context, sk_buf) };
        sk_buf.load_bytes_relative(base, offset, buf)
    }
}

fn bpf_skb_load_bytes<'a, C: SkbLoadBytesProgramContext>(
    context: &mut C::RunContext<'a>,
    sk_buf: BpfValue,
    offset: BpfValue,
    to: BpfValue,
    len: BpfValue,
    _: BpfValue,
) -> BpfValue {
    let base = LoadBytesBase::NetworkHeader;

    let Ok(offset) = offset.as_u64().try_into() else {
        return u64::MAX.into();
    };

    // SAFETY: The verifier ensures that `to` points to a valid buffer of at
    // least `len` bytes that the eBPF program has permission to access.
    let buf = unsafe { EbpfBufferPtr::new(to.as_ptr::<u8>(), len.as_u64() as usize) };

    C::skb_load_bytes_relative(context, sk_buf, base, offset, buf).into()
}

fn bpf_skb_load_bytes_relative<'a, C: SkbLoadBytesProgramContext>(
    context: &mut C::RunContext<'a>,
    sk_buf: BpfValue,
    offset: BpfValue,
    to: BpfValue,
    len: BpfValue,
    start_header: BpfValue,
) -> BpfValue {
    let base = match start_header.as_u64() {
        0 => LoadBytesBase::MacHeader,
        1 => LoadBytesBase::NetworkHeader,
        _ => return u64::MAX.into(),
    };

    let Ok(offset) = offset.as_u64().try_into() else {
        return u64::MAX.into();
    };

    // SAFETY: The verifier ensures that `to` points to a valid buffer of at
    // least `len` bytes that the eBPF program has permission to access.
    let buf = unsafe { EbpfBufferPtr::new(to.as_ptr::<u8>(), len.as_u64() as usize) };

    C::skb_load_bytes_relative(context, sk_buf, base, offset, buf).into()
}

fn bpf_sk_storage_get<'a, C: SkStorageProgramContext + MapsProgramContext>(
    context: &mut C::RunContext<'a>,
    map: BpfValue,
    sk: BpfValue,
    value: BpfValue,
    flags: BpfValue,
    _: BpfValue,
) -> BpfValue {
    if sk.is_zero() {
        return BpfValue::default();
    }

    // SAFETY: Verifier ensures that `sk` is either null or a pointer to
    // `bpf_sock`. The null case is checked above.
    let bpf_sock = unsafe { C::BpfSockRef::from_bpf_value(context, sk) };

    // Use socket cookie to identify the socket in the map.
    let Some(socket_id) = bpf_sock.get_socket_cookie() else {
        return BpfValue::default();
    };

    let key = socket_id.as_bytes();

    // SAFETY: The `map` must be a reference to a `Map` object kept alive by the program itself.
    let map: &Map = unsafe { &*map.as_ptr::<Map>() };

    // Checked by the verifier.
    assert!(map.schema.map_type == bpf_map_type_BPF_MAP_TYPE_SK_STORAGE);

    C::on_map_access(context, map);

    if let Some(value_ref) = map.lookup(key) {
        let result: BpfValue = value_ref.ptr().raw_ptr().into();
        C::add_value_ref(context, value_ref);
        return result;
    }

    if flags.as_u32() & BPF_SK_STORAGE_GET_F_CREATE != 0 {
        let mut vec;
        let init_val = if value.as_u64() == 0 {
            vec = SmallVec::<[u8; 128]>::new();
            vec.resize(map.schema.value_size as usize, 0);
            (&mut vec[..]).into()
        } else {
            // SAFETY: The verifier ensures that `value` points to a valid buffer.
            unsafe { EbpfBufferPtr::new(value.as_ptr::<u8>(), map.schema.value_size as usize) }
        };

        let r = map.update(key, init_val, 0);
        if r.is_ok() {
            if let Some(value_ref) = map.lookup(key) {
                let result: BpfValue = value_ref.ptr().raw_ptr().into();
                C::add_value_ref(context, value_ref);
                return result;
            }
        }
    }

    BpfValue::default()
}

fn bpf_sk_fullsock<C: EbpfProgramContext>(
    _context: &mut C::RunContext<'_>,
    sk: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
) -> BpfValue {
    sk
}

pub trait ReturnValueContext {
    fn set_retval(&mut self, value: i32) -> i32;
    fn get_retval(&self) -> i32;
}

pub trait ReturnValueProgramContext: EbpfProgramContext {
    fn set_retval<'a>(context: &mut Self::RunContext<'a>, value: i32) -> i32;
    fn get_retval<'a>(context: &mut Self::RunContext<'a>) -> i32;
}

impl<C: EbpfProgramContext> ReturnValueProgramContext for C
where
    for<'a> C::RunContext<'a>: ReturnValueContext,
{
    fn set_retval<'a>(context: &mut Self::RunContext<'a>, value: i32) -> i32 {
        context.set_retval(value)
    }
    fn get_retval<'a>(context: &mut Self::RunContext<'a>) -> i32 {
        context.get_retval()
    }
}

fn bpf_set_retval<C: ReturnValueProgramContext>(
    context: &mut C::RunContext<'_>,
    value: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
) -> BpfValue {
    C::set_retval(context, value.as_i32()).into()
}

fn bpf_get_retval<C: ReturnValueProgramContext>(
    context: &mut C::RunContext<'_>,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
) -> BpfValue {
    C::get_retval(context).into()
}

fn bpf_sk_lookup_tcp<C: EbpfProgramContext>(
    _context: &mut C::RunContext<'_>,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
) -> BpfValue {
    track_stub!(TODO("https://fxbug.dev/534355706"), "bpf_sk_lookup_tcp");
    0.into()
}

fn bpf_sk_lookup_udp<C: EbpfProgramContext>(
    _context: &mut C::RunContext<'_>,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
) -> BpfValue {
    track_stub!(TODO("https://fxbug.dev/534355185"), "bpf_sk_lookup_udp");
    0.into()
}

fn bpf_sk_release<C: EbpfProgramContext>(
    _context: &mut C::RunContext<'_>,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
) -> BpfValue {
    track_stub!(TODO("https://fxbug.dev/534355132"), "bpf_sk_release");
    0.into()
}

fn bpf_get_netns_cookie<C: EbpfProgramContext>(
    _context: &mut C::RunContext<'_>,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
    _: BpfValue,
) -> BpfValue {
    track_stub!(TODO("https://fxbug.dev/534354509"), "bpf_get_netns_cookie");
    const DEFAULT_NETWORK_NAMESPACE_COOKIE: u64 = 1;
    DEFAULT_NETWORK_NAMESPACE_COOKIE.into()
}

fn get_common_helpers<C: MapsProgramContext>() -> impl Iterator<Item = (u32, EbpfHelperImpl<C>)> {
    [
        (bpf_func_id_BPF_FUNC_ktime_get_boot_ns, EbpfHelperImpl(bpf_ktime_get_boot_ns::<C>)),
        (bpf_func_id_BPF_FUNC_ktime_get_coarse_ns, EbpfHelperImpl(bpf_ktime_get_coarse_ns::<C>)),
        (bpf_func_id_BPF_FUNC_ktime_get_ns, EbpfHelperImpl(bpf_ktime_get_ns::<C>)),
        (bpf_func_id_BPF_FUNC_map_delete_elem, EbpfHelperImpl(bpf_map_delete_elem::<C>)),
        (bpf_func_id_BPF_FUNC_map_lookup_elem, EbpfHelperImpl(bpf_map_lookup_elem::<C>)),
        (bpf_func_id_BPF_FUNC_map_update_elem, EbpfHelperImpl(bpf_map_update_elem::<C>)),
        (bpf_func_id_BPF_FUNC_probe_read_str, EbpfHelperImpl(bpf_probe_read_str::<C>)),
        (bpf_func_id_BPF_FUNC_probe_read_user, EbpfHelperImpl(bpf_probe_read_user::<C>)),
        (bpf_func_id_BPF_FUNC_probe_read_user_str, EbpfHelperImpl(bpf_probe_read_user_str::<C>)),
        (bpf_func_id_BPF_FUNC_ringbuf_discard, EbpfHelperImpl(bpf_ringbuf_discard::<C>)),
        (bpf_func_id_BPF_FUNC_ringbuf_reserve, EbpfHelperImpl(bpf_ringbuf_reserve::<C>)),
        (bpf_func_id_BPF_FUNC_ringbuf_submit, EbpfHelperImpl(bpf_ringbuf_submit::<C>)),
        (bpf_func_id_BPF_FUNC_trace_printk, EbpfHelperImpl(bpf_trace_printk::<C>)),
        (bpf_func_id_BPF_FUNC_get_smp_processor_id, EbpfHelperImpl(bpf_get_smp_processor_id::<C>)),
    ]
    .into_iter()
}

/// Returns helper implementations that depend on `CurrentTask`.
fn get_current_task_helpers<C: CurrentTaskProgramContext>()
-> impl Iterator<Item = (u32, EbpfHelperImpl<C>)> {
    [
        (bpf_func_id_BPF_FUNC_get_current_uid_gid, EbpfHelperImpl(bpf_get_current_uid_gid::<C>)),
        (bpf_func_id_BPF_FUNC_get_current_pid_tgid, EbpfHelperImpl(bpf_get_current_pid_tgid::<C>)),
    ]
    .into_iter()
}

// Trait for `EbpfProgramContext` implementations that are used for
// `BPF_PROG_TYPE_CGROUP_SOCK` programs.
pub trait CgroupSockProgramContext:
    MapsProgramContext
    + SocketCookieProgramContext
    + CurrentTaskProgramContext
    + SkStorageProgramContext
{
    fn get_helpers() -> HelperSet<Self> {
        [
            (bpf_func_id_BPF_FUNC_get_netns_cookie, EbpfHelperImpl(bpf_get_netns_cookie::<Self>)),
            (bpf_func_id_BPF_FUNC_get_socket_cookie, EbpfHelperImpl(bpf_get_socket_cookie::<Self>)),
            (bpf_func_id_BPF_FUNC_sk_storage_get, EbpfHelperImpl(bpf_sk_storage_get::<Self>)),
            (bpf_func_id_BPF_FUNC_sk_lookup_tcp, EbpfHelperImpl(bpf_sk_lookup_tcp::<Self>)),
            (bpf_func_id_BPF_FUNC_sk_lookup_udp, EbpfHelperImpl(bpf_sk_lookup_udp::<Self>)),
            (bpf_func_id_BPF_FUNC_sk_release, EbpfHelperImpl(bpf_sk_release::<Self>)),
        ]
        .into_iter()
        .chain(get_common_helpers())
        .chain(get_current_task_helpers())
        .collect()
    }
}

// Trait for `EbpfProgramContext` implementations that are used for
// `BPF_PROG_TYPE_CGROUP_SOCKADDR` programs.
pub trait CgroupSockAddrProgramContext:
    MapsProgramContext
    + SocketCookieProgramContext
    + CurrentTaskProgramContext
    + SkStorageProgramContext
{
    fn get_helpers() -> HelperSet<Self> {
        [
            (bpf_func_id_BPF_FUNC_get_netns_cookie, EbpfHelperImpl(bpf_get_netns_cookie::<Self>)),
            (bpf_func_id_BPF_FUNC_get_socket_cookie, EbpfHelperImpl(bpf_get_socket_cookie::<Self>)),
            (bpf_func_id_BPF_FUNC_sk_storage_get, EbpfHelperImpl(bpf_sk_storage_get::<Self>)),
            (bpf_func_id_BPF_FUNC_sk_lookup_tcp, EbpfHelperImpl(bpf_sk_lookup_tcp::<Self>)),
            (bpf_func_id_BPF_FUNC_sk_lookup_udp, EbpfHelperImpl(bpf_sk_lookup_udp::<Self>)),
            (bpf_func_id_BPF_FUNC_sk_release, EbpfHelperImpl(bpf_sk_release::<Self>)),
        ]
        .into_iter()
        .chain(get_common_helpers())
        .chain(get_current_task_helpers())
        .collect()
    }
}

// Trait for `EbpfProgramContext` implementations that are used for
// `BPF_PROG_TYPE_CGROUP_SOCKOPT` programs.
pub trait CgroupSockOptProgramContext:
    MapsProgramContext + CurrentTaskProgramContext + ReturnValueProgramContext + SkStorageProgramContext
{
    fn get_helpers() -> HelperSet<Self> {
        [
            (bpf_func_id_BPF_FUNC_get_netns_cookie, EbpfHelperImpl(bpf_get_netns_cookie::<Self>)),
            (bpf_func_id_BPF_FUNC_set_retval, EbpfHelperImpl(bpf_set_retval::<Self>)),
            (bpf_func_id_BPF_FUNC_get_retval, EbpfHelperImpl(bpf_get_retval::<Self>)),
            (bpf_func_id_BPF_FUNC_sk_storage_get, EbpfHelperImpl(bpf_sk_storage_get::<Self>)),
            (bpf_func_id_BPF_FUNC_sk_lookup_tcp, EbpfHelperImpl(bpf_sk_lookup_tcp::<Self>)),
            (bpf_func_id_BPF_FUNC_sk_lookup_udp, EbpfHelperImpl(bpf_sk_lookup_udp::<Self>)),
            (bpf_func_id_BPF_FUNC_sk_release, EbpfHelperImpl(bpf_sk_release::<Self>)),
        ]
        .into_iter()
        .chain(get_common_helpers())
        .chain(get_current_task_helpers())
        .collect()
    }
}

// Trait for `EbpfProgramContext` implementations that are used for
// `BPF_PROG_TYPE_SOCKET_FILTER` programs.
pub trait SocketFilterProgramContext:
    MapsProgramContext
    + SocketUidProgramContext
    + SocketCookieProgramContext
    + SkbLoadBytesProgramContext
{
    fn get_helpers() -> HelperSet<Self> {
        vec![
            (bpf_func_id_BPF_FUNC_get_netns_cookie, EbpfHelperImpl(bpf_get_netns_cookie::<Self>)),
            (bpf_func_id_BPF_FUNC_get_socket_uid, EbpfHelperImpl(bpf_get_socket_uid::<Self>)),
            (bpf_func_id_BPF_FUNC_get_socket_cookie, EbpfHelperImpl(bpf_get_socket_cookie::<Self>)),
            (bpf_func_id_BPF_FUNC_skb_load_bytes, EbpfHelperImpl(bpf_skb_load_bytes::<Self>)),
            (
                bpf_func_id_BPF_FUNC_skb_load_bytes_relative,
                EbpfHelperImpl(bpf_skb_load_bytes_relative::<Self>),
            ),
        ]
        .into_iter()
        .chain(get_common_helpers())
        .collect()
    }
}

// Trait for `EbpfProgramContext` implementations that are used for
// `BPF_PROG_TYPE_CGROUP_SKB` programs.
pub trait CgroupSkbProgramContext:
    MapsProgramContext
    + SocketUidProgramContext
    + SocketCookieProgramContext
    + SkbLoadBytesProgramContext
    + SkStorageProgramContext
{
    fn get_helpers() -> HelperSet<Self> {
        vec![
            (bpf_func_id_BPF_FUNC_get_netns_cookie, EbpfHelperImpl(bpf_get_netns_cookie::<Self>)),
            (bpf_func_id_BPF_FUNC_get_socket_uid, EbpfHelperImpl(bpf_get_socket_uid::<Self>)),
            (bpf_func_id_BPF_FUNC_get_socket_cookie, EbpfHelperImpl(bpf_get_socket_cookie::<Self>)),
            (bpf_func_id_BPF_FUNC_skb_load_bytes, EbpfHelperImpl(bpf_skb_load_bytes::<Self>)),
            (
                bpf_func_id_BPF_FUNC_skb_load_bytes_relative,
                EbpfHelperImpl(bpf_skb_load_bytes_relative::<Self>),
            ),
            (bpf_func_id_BPF_FUNC_sk_storage_get, EbpfHelperImpl(bpf_sk_storage_get::<Self>)),
            (bpf_func_id_BPF_FUNC_sk_lookup_tcp, EbpfHelperImpl(bpf_sk_lookup_tcp::<Self>)),
            (bpf_func_id_BPF_FUNC_sk_lookup_udp, EbpfHelperImpl(bpf_sk_lookup_udp::<Self>)),
            (bpf_func_id_BPF_FUNC_sk_release, EbpfHelperImpl(bpf_sk_release::<Self>)),
            (bpf_func_id_BPF_FUNC_sk_fullsock, EbpfHelperImpl(bpf_sk_fullsock::<Self>)),
        ]
        .into_iter()
        .chain(get_common_helpers())
        .collect()
    }
}

/// Macro used to declare program type for a `EbpfProgramContext` implementation.
/// Implements `StaticHelperSet` trait for the context type.
///
/// # Example
///
/// The following example declares that `MyEbpfProgramContext` is used to run
/// socket filter programs:
///
/// ```
/// ebpf_program_context_type!(MyEbpfProgramContext, SocketFilterProgramContext);
/// ```
#[macro_export]
macro_rules! ebpf_program_context_type {
    ($context:ty, $subtrait:ty) => {
        impl $subtrait for $context {}
        ebpf::static_helper_set!($context, <$context as $subtrait>::get_helpers());
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::maps::{Map, PinnedMap};
    use ebpf::{BpfValue, EbpfProgramContext, FromBpfValue, MapFlags, MapSchema};
    use linux_uapi::{BPF_SK_STORAGE_GET_F_CREATE, bpf_map_type_BPF_MAP_TYPE_SK_STORAGE};

    struct MockSocket {
        cookie: u64,
    }
    impl SocketRef for MockSocket {
        fn get_socket_cookie(&self) -> Option<u64> {
            Some(self.cookie)
        }
        fn get_socket_uid(&self) -> Option<uid_t> {
            Some(0)
        }
    }
    impl<'a> FromBpfValue<TestRunContext<'a>> for MockSocket {
        unsafe fn from_bpf_value(_context: &mut TestRunContext<'a>, value: BpfValue) -> Self {
            Self { cookie: value.as_u64() }
        }
    }

    struct TestRunContext<'a> {
        map_refs: Vec<MapValueRef<'a>>,
    }
    impl<'a> BpfSockContext for TestRunContext<'a> {
        type BpfSockRef = MockSocket;
    }
    impl<'a> MapsContext<'a> for TestRunContext<'a> {
        fn on_map_access(&mut self, _map: &Map) {}
        fn add_value_ref(&mut self, map_ref: MapValueRef<'a>) {
            self.map_refs.push(map_ref);
        }
    }

    struct TestContext;
    impl EbpfProgramContext for TestContext {
        type RunContext<'a> = TestRunContext<'a>;
        type Packet<'a> = ();
        type Arg1<'a> = ();
        type Arg2<'a> = ();
        type Arg3<'a> = ();
        type Arg4<'a> = ();
        type Arg5<'a> = ();
        type Map = PinnedMap;
    }

    #[fuchsia::test]
    fn test_sk_storage_get_uaf() {
        let schema = MapSchema {
            map_type: bpf_map_type_BPF_MAP_TYPE_SK_STORAGE,
            key_size: 4,
            value_size: 8,
            max_entries: 0,
            flags: MapFlags::NoPrealloc,
        };
        let map = Map::new(schema, "test").unwrap();
        let map_value = BpfValue::from(&*map as *const Map);

        let mut context = TestRunContext { map_refs: vec![] };

        // 1. Create entry for socket 42
        let sk_value1 = BpfValue::from(42u64);
        let init_value1 = [0x11u8; 8];
        let init_value_ptr1 = BpfValue::from(init_value1.as_ptr());
        let flags = BpfValue::from(BPF_SK_STORAGE_GET_F_CREATE as u64);

        let ptr1 = bpf_sk_storage_get::<TestContext>(
            &mut context,
            map_value,
            sk_value1,
            init_value_ptr1,
            flags,
            BpfValue::default(),
        );
        assert!(!ptr1.is_zero());

        // Verify initial value
        // SAFETY: ptr1 is a valid pointer to the map value.
        unsafe {
            assert_eq!(*(ptr1.as_ptr::<u64>()), 0x1111111111111111);
        }

        // 2. Delete entry for socket 42 from map
        let key_bytes = 42u64.to_ne_bytes();
        map.delete(&key_bytes).unwrap();

        // 3. Create entry for socket 43
        // If UAF exists, this should reuse the same memory block because it was freed.
        let sk_value2 = BpfValue::from(43u64);
        let init_value2 = [0x22u8; 8];
        let init_value_ptr2 = BpfValue::from(init_value2.as_ptr());

        let ptr2 = bpf_sk_storage_get::<TestContext>(
            &mut context,
            map_value,
            sk_value2,
            init_value_ptr2,
            flags,
            BpfValue::default(),
        );
        assert!(!ptr2.is_zero());

        // We want to assert that the value at ptr1 has NOT changed, which means it was not reused.
        // This assertion will FAIL without the fix (UAF occurs,
        // ptr1's memory is overwritten with ptr2's init value),
        // and PASS with the fix (ptr1's memory is kept alive).
        // SAFETY: ptr1 points to memory that is kept alive by the reference in `context`.
        unsafe {
            assert_eq!(*(ptr1.as_ptr::<u64>()), 0x1111111111111111);
        }
    }

    #[fuchsia::test]
    fn test_sk_storage_get_uaf_query() {
        let schema = MapSchema {
            map_type: bpf_map_type_BPF_MAP_TYPE_SK_STORAGE,
            key_size: 4,
            value_size: 8,
            max_entries: 0,
            flags: MapFlags::NoPrealloc,
        };
        let map = Map::new(schema, "test").unwrap();
        let map_value = BpfValue::from(&*map as *const Map);

        let mut context = TestRunContext { map_refs: vec![] };

        // 1. Create entry for socket 42
        let sk_value1 = BpfValue::from(42u64);
        let init_value1 = [0x11u8; 8];
        let init_value_ptr1 = BpfValue::from(init_value1.as_ptr());
        let flags = BpfValue::from(BPF_SK_STORAGE_GET_F_CREATE as u64);

        let ptr1 = bpf_sk_storage_get::<TestContext>(
            &mut context,
            map_value,
            sk_value1,
            init_value_ptr1,
            flags,
            BpfValue::default(),
        );
        assert!(!ptr1.is_zero());

        // Clear context to simulate that we don't hold the creation
        // reference anymore. The map still holds the reference.
        context.map_refs.clear();

        // 2. Query entry for socket 42 (without CREATE flag)
        let ptr1_query = bpf_sk_storage_get::<TestContext>(
            &mut context,
            map_value,
            sk_value1,
            BpfValue::default(),
            BpfValue::default(),
            BpfValue::default(),
        );
        assert_eq!(ptr1.as_u64(), ptr1_query.as_u64());

        // 3. Delete entry for socket 42 from map
        let key_bytes = 42u64.to_ne_bytes();
        map.delete(&key_bytes).unwrap();

        // 4. Create entry for socket 43
        // If UAF exists, this should reuse the same memory block
        // because it was freed.
        let sk_value2 = BpfValue::from(43u64);
        let init_value2 = [0x22u8; 8];
        let init_value_ptr2 = BpfValue::from(init_value2.as_ptr());

        let ptr2 = bpf_sk_storage_get::<TestContext>(
            &mut context,
            map_value,
            sk_value2,
            init_value_ptr2,
            flags,
            BpfValue::default(),
        );
        assert!(!ptr2.is_zero());

        // We want to assert that the value at ptr1_query has NOT
        // changed.
        // SAFETY: ptr1_query points to memory that is kept alive by
        // the reference in `context` (from the query).
        unsafe {
            assert_eq!(*(ptr1_query.as_ptr::<u64>()), 0x1111111111111111);
        }
    }

    /// Regression test: the verifier models the probe_read_user{,_str}
    /// destination as written (`output: true`), but the runtime helpers are
    /// stubs. They must zero the destination so a program cannot read back
    /// uninitialized executor memory. Without the fix the sentinel bytes below
    /// survive the call.
    ///
    /// TODO("https://fxbug.dev/534354547","https://fxbug.dev/534355539"): Replace
    /// these tests when the real helpers are implemented.
    #[fuchsia::test]
    fn test_probe_read_user_zeroes_destination() {
        let mut context = TestRunContext { map_refs: vec![] };

        // Stand in for stale executor-stack memory with a non-zero sentinel.
        let mut dst = [0xAAu8; 8];
        let ret = bpf_probe_read_user::<TestContext>(
            &mut context,
            BpfValue::from(dst.as_mut_ptr()),
            BpfValue::from(dst.len() as u64),
            BpfValue::default(),
            BpfValue::default(),
            BpfValue::default(),
        );
        assert_eq!(ret.as_u64(), 0);
        assert_eq!(dst, [0u8; 8], "probe_read_user left the destination uninitialized");

        let mut dst_str = [0xBBu8; 8];
        let _ = bpf_probe_read_user_str::<TestContext>(
            &mut context,
            BpfValue::from(dst_str.as_mut_ptr()),
            BpfValue::from(dst_str.len() as u64),
            BpfValue::default(),
            BpfValue::default(),
            BpfValue::default(),
        );
        assert_eq!(dst_str, [0u8; 8], "probe_read_user_str left the destination uninitialized");
    }
}
