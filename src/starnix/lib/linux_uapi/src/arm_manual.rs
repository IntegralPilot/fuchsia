// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{arch_translate_data, check_arch_independent_layout, translate_data};

fn saturating_u64_to_u32(v: u64) -> u32 {
    if v > u32::MAX.into() { u32::MAX } else { v as u32 }
}

fn saturating_i64_to_i32(v: i64) -> i32 {
    if v > i32::MAX.into() {
        i32::MAX
    } else if v < i32::MIN.into() {
        i32::MIN
    } else {
        v as i32
    }
}

arch_translate_data! {
    BidiFrom<flock> {
        l_type,
        l_whence,
        l_start,
        l_len,
        l_pid
    }

    BidiFrom<flock64> {
        l_type,
        l_whence,
        l_start,
        l_len,
        l_pid
    }

    BidiFrom<itimerspec> {
        it_interval,
        it_value
    }

    BidiFrom<itimerval> {
        it_interval,
        it_value
    }

    BidiFrom<__kernel_fsid_t> {
        val
    }

    BidiFrom<sigaltstack> {
        ss_sp,
        ss_flags,
        ss_size
    }

    BidiFrom<cmsghdr> {
        cmsg_len,
        cmsg_level,
        cmsg_type,
    }

    BidiFrom<sock_filter> {
        code,
        jt,
        jf,
        k,
    }

    BidiFrom<ucred> {
        pid,
        uid,
        gid,
    }

    BidiFrom<rusage> {
        ru_utime,
        ru_stime,
        ru_maxrss,
        ru_ixrss,
        ru_idrss,
        ru_isrss,
        ru_minflt,
        ru_majflt,
        ru_nswap,
        ru_inblock,
        ru_oublock,
        ru_msgsnd,
        ru_msgrcv,
        ru_nsignals,
        ru_nvcsw,
        ru_nivcsw,
    }

    BidiFrom<iovec> {
        iov_base,
        iov_len,
    }

    BidiFrom<input_event> {
        time,
        type_,
        code,
        value,
    }
}

translate_data! {
    TryFrom<crate::stat> for crate::arch32::stat64 {
        st_dev = st_dev;
        __st_ino = st_ino(0);
        st_mode = st_mode;
        st_nlink = st_nlink;
        st_uid = st_uid;
        st_gid = st_gid;
        st_rdev = st_rdev;
        st_size = st_size;
        st_blksize = st_blksize;
        st_blocks = st_blocks;
        st_atime = st_atime;
        st_atime_nsec = st_atime_nsec;
        st_mtime = st_mtime;
        st_mtime_nsec = st_mtime_nsec;
        st_ctime = st_ctime;
        st_ctime_nsec = st_ctime_nsec;
        st_ino = st_ino;
        ..Default::default()
    }

    TryFrom<crate::statfs> for crate::arch32::statfs {
        f_type = f_type;
        f_bsize = f_bsize;
        f_blocks = f_blocks;
        f_bfree = f_bfree;
        f_bavail = f_bavail;
        f_files = f_files;
        f_ffree = f_ffree;
        f_fsid = f_fsid;
        f_namelen = f_namelen;
        f_frsize = f_frsize;
        f_flags = f_flags;
        ..Default::default()
    }

    TryFrom<crate::statfs> for crate::arch32::statfs64 {
        f_type = f_type;
        f_bsize = f_bsize;
        f_blocks = f_blocks;
        f_bfree = f_bfree;
        f_bavail = f_bavail;
        f_files = f_files;
        f_ffree = f_ffree;
        f_fsid = f_fsid;
        f_namelen = f_namelen;
        f_frsize = f_frsize;
        f_flags = f_flags;
        ..Default::default()
    }
}

arch_translate_data! {
    From32<timespec> {
        tv_sec,
        tv_nsec,
    }
}
impl From<crate::timespec> for crate::arch32::timespec {
    fn from(tv: crate::timespec) -> Self {
        Self {
            tv_sec: saturating_i64_to_i32(tv.tv_sec),
            tv_nsec: saturating_i64_to_i32(tv.tv_nsec),
        }
    }
}

arch_translate_data! {
    From32<timeval> {
        tv_sec,
        tv_usec,
    }
}
impl From<crate::timeval> for crate::arch32::timeval {
    fn from(tv: crate::timeval) -> Self {
        Self {
            tv_sec: saturating_i64_to_i32(tv.tv_sec),
            tv_usec: saturating_i64_to_i32(tv.tv_usec),
        }
    }
}

arch_translate_data! {
    From32<rlimit> {
        rlim_cur,
        rlim_max,
    }
}
impl From<crate::rlimit> for crate::arch32::rlimit {
    fn from(rlimit: crate::rlimit) -> Self {
        Self {
            rlim_cur: saturating_u64_to_u32(rlimit.rlim_cur),
            rlim_max: saturating_u64_to_u32(rlimit.rlim_max),
        }
    }
}

impl TryFrom<crate::sigset_t> for u32 {
    type Error = ();
    fn try_from(sigset: crate::sigset_t) -> Result<Self, ()> {
        sigset.sig[0].try_into().map_err(|_| ())
    }
}

impl From<u32> for crate::sigset_t {
    fn from(sigset: u32) -> Self {
        crate::sigset_t { sig: [sigset.into()] }
    }
}

impl From<crate::arch32::sigset64_t> for crate::sigset_t {
    fn from(sigset: crate::arch32::sigset64_t) -> Self {
        Self { sig: [sigset.sig[0].into()] }
    }
}

impl From<crate::sigset_t> for crate::arch32::sigset64_t {
    fn from(sigset: crate::sigset_t) -> Self {
        Self { sig: [sigset.sig[0] as u32] }
    }
}

arch_translate_data! {
    BidiFrom<__kernel_sigaction> {
        sa_handler,
        sa_mask,
        sa_flags,
        sa_restorer,
    }
}

translate_data! {
    BidiTryFrom<crate::arch32::sigaction64, crate::__kernel_sigaction> {
        sa_handler = sa_handler;
        sa_mask = sa_mask;
        sa_flags = sa_flags;
        sa_restorer = sa_restorer;
    }
}
impl From<crate::arch32::sigval> for crate::sigval {
    fn from(sigval: crate::arch32::sigval) -> Self {
        // SAFETY: This is safe because the union has a single field.
        let bindgen_opaque_blob = unsafe { sigval._bindgen_opaque_blob };
        let bindgen_opaque_blob_as_u64: u64 = bindgen_opaque_blob.into();
        Self { _bindgen_opaque_blob: zerocopy::transmute!(bindgen_opaque_blob_as_u64) }
    }
}

impl TryFrom<crate::sigval> for crate::arch32::sigval {
    type Error = ();
    fn try_from(sigval: crate::sigval) -> Result<Self, ()> {
        // SAFETY: This is safe because the union has a single field.
        let bindgen_opaque_blob = unsafe { sigval._bindgen_opaque_blob };
        let bindgen_opaque_blob_as_u64: u64 = zerocopy::transmute!(bindgen_opaque_blob);
        Ok(Self { _bindgen_opaque_blob: bindgen_opaque_blob_as_u64.try_into().map_err(|_| ())? })
    }
}

impl From<crate::arch32::sigevent> for crate::sigevent {
    fn from(sigevent: crate::arch32::sigevent) -> Self {
        let mut _sigev_un = crate::sigevent__bindgen_ty_1::default();
        // SAFETY: This is safe because the union has all bytes defined.
        match sigevent.sigev_notify as u32 {
            crate::SIGEV_THREAD_ID => unsafe {
                _sigev_un._tid = sigevent._sigev_un._tid.into();
            },
            crate::SIGEV_THREAD => unsafe {
                _sigev_un._sigev_thread = crate::sigevent__bindgen_ty_1__bindgen_ty_1 {
                    _function: sigevent._sigev_un._sigev_thread._function.into(),
                    _attribute: sigevent._sigev_un._sigev_thread._attribute.into(),
                };
            },
            _ => {}
        }
        Self {
            sigev_value: sigevent.sigev_value.into(),
            sigev_signo: sigevent.sigev_signo.into(),
            sigev_notify: sigevent.sigev_notify.into(),
            _sigev_un,
        }
    }
}

impl TryFrom<crate::sigevent> for crate::arch32::sigevent {
    type Error = ();
    fn try_from(sigevent: crate::sigevent) -> Result<Self, ()> {
        let mut _sigev_un = crate::arch32::sigevent__bindgen_ty_1::default();
        // SAFETY: This is safe because the union has all bytes defined.
        match sigevent.sigev_notify as u32 {
            crate::SIGEV_THREAD_ID => unsafe {
                _sigev_un._tid = sigevent._sigev_un._tid.try_into().map_err(|_| ())?;
            },
            crate::SIGEV_THREAD => unsafe {
                _sigev_un._sigev_thread = crate::arch32::sigevent__bindgen_ty_1__bindgen_ty_1 {
                    _function: sigevent
                        ._sigev_un
                        ._sigev_thread
                        ._function
                        .try_into()
                        .map_err(|_| ())?,
                    _attribute: sigevent
                        ._sigev_un
                        ._sigev_thread
                        ._attribute
                        .try_into()
                        .map_err(|_| ())?,
                };
            },
            _ => {}
        }
        Ok(Self {
            sigev_value: sigevent.sigev_value.try_into().map_err(|_| ())?,
            sigev_signo: sigevent.sigev_signo.try_into().map_err(|_| ())?,
            sigev_notify: sigevent.sigev_notify.try_into().map_err(|_| ())?,
            _sigev_un,
        })
    }
}

impl From<crate::arch32::sysinfo> for crate::sysinfo {
    fn from(sysinfo: crate::arch32::sysinfo) -> Self {
        Self {
            uptime: sysinfo.uptime.into(),
            loads: sysinfo.loads.map(u64::from),
            totalram: sysinfo.totalram.into(),
            freeram: sysinfo.freeram.into(),
            sharedram: sysinfo.sharedram.into(),
            bufferram: sysinfo.bufferram.into(),
            totalswap: sysinfo.totalswap.into(),
            freeswap: sysinfo.freeswap.into(),
            procs: sysinfo.procs.into(),
            pad: sysinfo.pad.into(),
            totalhigh: sysinfo.totalhigh.into(),
            freehigh: sysinfo.freehigh.into(),
            mem_unit: sysinfo.mem_unit.into(),
            ..Default::default()
        }
    }
}

impl TryFrom<crate::sysinfo> for crate::arch32::sysinfo {
    type Error = ();
    fn try_from(mut sysinfo: crate::sysinfo) -> Result<Self, ()> {
        let [v1, v2, v3] = sysinfo.loads;
        let loads = [
            u32::try_from(v1).map_err(|_| ())?,
            u32::try_from(v2).map_err(|_| ())?,
            u32::try_from(v3).map_err(|_| ())?,
        ];

        while sysinfo.totalram > u32::MAX as u64 {
            sysinfo.totalram >>= 1;
            sysinfo.freeram >>= 1;
            sysinfo.sharedram >>= 1;
            sysinfo.bufferram >>= 1;
            sysinfo.totalswap >>= 1;
            sysinfo.freeswap >>= 1;
            sysinfo.totalhigh >>= 1;
            sysinfo.freehigh >>= 1;
            sysinfo.mem_unit <<= 1;
        }

        Ok(Self {
            uptime: sysinfo.uptime.try_into().map_err(|_| ())?,
            loads,
            totalram: sysinfo.totalram.try_into().map_err(|_| ())?,
            freeram: sysinfo.freeram.try_into().map_err(|_| ())?,
            sharedram: sysinfo.sharedram.try_into().map_err(|_| ())?,
            bufferram: sysinfo.bufferram.try_into().map_err(|_| ())?,
            totalswap: sysinfo.totalswap.try_into().map_err(|_| ())?,
            freeswap: sysinfo.freeswap.try_into().map_err(|_| ())?,
            procs: sysinfo.procs.try_into().map_err(|_| ())?,
            pad: sysinfo.pad.try_into().map_err(|_| ())?,
            totalhigh: sysinfo.totalhigh.try_into().map_err(|_| ())?,
            freehigh: sysinfo.freehigh.try_into().map_err(|_| ())?,
            mem_unit: sysinfo.mem_unit.try_into().map_err(|_| ())?,
            ..Default::default()
        })
    }
}

check_arch_independent_layout! {
    dma_heap_allocation_data {
        len,
        fd,
        fd_flags,
        heap_flags,
    }
}

arch_translate_data! {
    From32<fastrpc_ioctl_invoke> {
        handle,
        sc,
        pra,
    }

    From32<fastrpc_ioctl_invoke_fd> {
        inv,
        fds,
    }

    From32<fastrpc_ioctl_invoke2> {
        req,
        invparam,
        size,
        err,
    }

    From32<remote_buf> {
        pv,
        len,
    }

    From32<fastrpc_ioctl_init> {
        flags,
        file,
        filelen,
        filefd,
        mem,
        memlen,
        memfd,
    }
}

check_arch_independent_layout! {
    fastrpc_ioctl_capability {
        domain,
        attribute_ID,
        capability,
    }
}

arch_translate_data! {
    TryFrom64<fb_fix_screeninfo> {
        id,
        smem_start,
        smem_len,
        type_,
        type_aux,
        visual,
        xpanstep,
        ypanstep,
        ywrapstep,
        line_length,
        mmio_start,
        mmio_len,
        accel,
        capabilities,
        reserved,
    }

    BidiFrom<fb_bitfield> {
        offset,
        length,
        msb_right,
    }

    BidiFrom<fb_var_screeninfo> {
        xres,
        yres,
        xres_virtual,
        yres_virtual,
        xoffset,
        yoffset,
        bits_per_pixel,
        grayscale,
        red,
        green,
        blue,
        transp,
        nonstd,
        activate,
        height,
        width,
        accel_flags,
        pixclock,
        left_margin,
        right_margin,
        upper_margin,
        lower_margin,
        hsync_len,
        vsync_len,
        sync,
        vmode,
        rotate,
        colorspace,
        reserved,
    }
}
