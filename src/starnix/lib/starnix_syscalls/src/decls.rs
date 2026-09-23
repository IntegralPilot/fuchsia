// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(non_upper_case_globals)]

use crate::SyscallArg;
use starnix_types::arch::ArchWidth;
#[allow(unused_imports)]
use starnix_uapi::user_address::ArchSpecific;

/// Helper for for_each_syscall! that adds any architecture-specific syscalls.
///
/// X86_64 has many unique syscalls for legacy reasons. Newer architectures funnel some of these
/// through some newer and more general variants. The variants used by other platforms are listed in
/// the comments below.
#[cfg(target_arch = "x86_64")]
#[macro_export]
macro_rules! for_each_arch_syscall {
    {$callback:ident; $($context:ident;)* ; $($common_name:ident,)*} => {
        $callback!{
            $($context;)*
            $($common_name,)*

            // go/keep-sorted start

            _sysctl,  // (deprecated)
            access,  // faccessat
            afs_syscall, // (deprecated)
            alarm,  // setitimer
            arch_prctl,  // (unused)
            chmod,  // fchmodat
            chown,  // fchownat
            creat,  // openat
            create_module, // (deprecated)
            dup2,  // dup3
            epoll_create,  // epoll_create1
            epoll_ctl_old,  // (unused)
            epoll_wait,  // epoll_pwait
            epoll_wait_old,  // (unused)
            eventfd,  // eventfd2
            fork,  // clone
            futimesat,  // (deprecated)
            get_kernel_syms, // (deprecated)
            get_thread_area,  // (unused)
            getdents,  // getdents64
            getpgrp,  // getpgid
            getpmsg, // (unused)
            inotify_init,  // inotify_init1
            ioperm,  // (unused)
            iopl,  // (deprevated)
            lchown,  // fchownat
            link,  // linkat
            lstat,  // fstatat
            mkdir,  // mkdirat
            mknod,  // mknodat
            modify_ldt,  // (unused)
            open,  // openat
            pause,  // sigsuspend
            pipe,  // pipe2
            poll,  // ppoll
            putpmsg, // (unused)
            query_module, // (deprecated)
            readlink,  // readlinkat
            rename,  // renameat2
            renameat,  // renameat2
            rmdir,  // unlinkat
            security,  // (unused)
            select,  // pselect
            set_thread_area, // (unused)
            signalfd,  // signalfd4
            stat,  // fstatat
            symlink,  // symlinkat
            sysfs,  // (deprecated)
            time,  // gettimeofday
            tuxcall,  // (unused)
            unlink,  // unlinkat
            uselib,  // (deprecated)
            ustat,  // (deprecated)
            utime,  // utimesat
            utimes,  // utimesat
            vfork,  // clone
            vserver,  // (unused)

            // go/keep-sorted end
        }
    }
}

// This macro ensures there is a unique way to identify syscalls that have a
// distinct implementation from the primary architecture.
#[cfg(all(target_arch = "aarch64"))]
#[macro_export]
macro_rules! for_each_arch_arch32_syscall {
    {$callback:ident; $($context:ident;)* ; $($common_name:ident,)*} => {
        $callback!{
            $($context;)*
            $($common_name,)*

            // go/keep-sorted start

            ARM_breakpoint,
            ARM_cacheflush,
            ARM_set_tls,
            ARM_usr26,
            ARM_usr32,
            _llseek,
            _newselect,
            arm_fadvise64_64,
            chown,
            chown32,
            clock_gettime64,
            dup2,
            epoll_create,
            epoll_pwait,
            epoll_pwait2,
            epoll_wait,
            execve,
            execveat,
            fallocate,
            fanotify_mark,
            fchown32,
            fcntl,
            fcntl64,
            fstat,
            fstat64,
            fstatat64,
            fstatfs,
            fstatfs64,
            ftruncate,
            ftruncate64,
            get_robust_list,
            getdents,
            getegid32,
            geteuid32,
            getgid32,
            getgroups32,
            getitimer,
            getresgid32,
            getresuid32,
            getrusage,
            gettimeofday,
            io_pgetevents,
            io_pgetevents_time64,
            io_setup,
            io_submit,
            ioctl,
            kexec_load,
            keyctl,
            lseek,
            lstat,
            mkdir,
            mmap2,
            mq_getsetattr,
            mq_notify,
            mq_open,
            msgctl,
            msgrcv,
            msgsnd,
            open,
            open_by_handle_at,
            openat,
            poll,
            ppoll,
            ppoll_time64,
            pread64,
            preadv,
            preadv2,
            pselect6,
            pselect6_time64,
            ptrace,
            pwrite64,
            pwritev,
            pwritev2,
            readahead,
            recv,
            recvfrom,
            recvmmsg,
            recvmmsg_time64,
            recvmsg,
            renameat,  // renameat2
            rt_sigaction,
            rt_sigpending,
            rt_sigprocmask,
            rt_sigqueueinfo,
            rt_sigreturn,
            rt_sigsuspend,
            rt_sigtimedwait,
            rt_sigtimedwait_time64,
            rt_tgsigqueueinfo,
            sched_getaffinity,
            sched_setaffinity,
            semctl,
            send,
            sendfile,
            sendfile64,
            sendmmsg,
            sendmsg,
            set_robust_list,
            setfsgid32,
            setfsuid32,
            setgid32,
            setgroups32,
            setitimer,
            setregid32,
            setresgid32,
            setresuid32,
            setreuid32,
            setrlimit,
            settimeofday,
            setuid32,
            shmat,
            shmctl,
            sigaction,
            sigaltstack,
            signalfd,
            signalfd4,
            sigpending,
            sigprocmask,
            sigreturn,
            stat,
            stat64,
            statfs,
            statfs64,
            sync_file_range2,
            sysinfo,
            timer_create,
            timer_gettime64,
            times,
            truncate,
            truncate64,
            ugetrlimit,
            unlink,
            ustat,
            vfork,
            wait4,
            waitid,

            // go/keep-sorted end
        }
    }
}

#[cfg(target_arch = "aarch64")]
#[macro_export]
macro_rules! for_each_arch_syscall {
    {$callback:ident; $($context:ident;)* ; $($common_name:ident,)*} => {
        $callback!{
            $($context;)*
            $($common_name,)*
            renameat,  // renameat2
        }
    }
}

#[cfg(target_arch = "riscv64")]
#[macro_export]
macro_rules! for_each_arch_syscall {
    {$callback:ident; $($context:ident;)* ; $($common_name:ident,)*} => {
        $callback!{
            $($context;)*
            $($common_name,)*
        }
    }
}

/// Intended to be used with other macros to produce code that needs to handle
/// each syscall.
///
/// This list contains all cross-architecture syscalls, and delegates through for_each_arch_syscall!
/// to add in any architecture-specific ones.
#[macro_export]
macro_rules! for_each_syscall {
    {$callback:ident $(,$context:ident)*} => {
        $crate::for_each_arch_syscall!{
            $callback;
            $($context;)*
            ;

            // go/keep-sorted start

            accept,
            accept4,
            acct,
            add_key,
            adjtimex,
            bind,
            bpf,
            brk,
            cachestat,
            capget,
            capset,
            chdir,
            chroot,
            clock_adjtime,
            clock_getres,
            clock_gettime,
            clock_nanosleep,
            clock_settime,
            clone,
            clone3,
            close,
            close_range,
            connect,
            copy_file_range,
            delete_module,
            dup,
            dup3,
            epoll_create1,
            epoll_ctl,
            epoll_pwait,
            epoll_pwait2,
            eventfd2,
            execve,
            execveat,
            exit,
            exit_group,
            faccessat,
            faccessat2,
            fadvise64,
            fallocate,
            fanotify_init,
            fanotify_mark,
            fchdir,
            fchmod,
            fchmodat,
            fchown,
            fchownat,
            fcntl,
            fdatasync,
            fgetxattr,
            finit_module,
            flistxattr,
            flock,
            fremovexattr,
            fsconfig,
            fsetxattr,
            fsmount,
            fsopen,
            fspick,
            fstat,
            fstatfs,
            fsync,
            ftruncate,
            futex,
            futex_waitv,
            get_mempolicy,
            get_robust_list,
            getcpu,
            getcwd,
            getdents64,
            getegid,
            geteuid,
            getgid,
            getgroups,
            getitimer,
            getpeername,
            getpgid,
            getpid,
            getppid,
            getpriority,
            getrandom,
            getresgid,
            getresuid,
            getrlimit,
            getrusage,
            getsid,
            getsockname,
            getsockopt,
            gettid,
            gettimeofday,
            getuid,
            getxattr,
            init_module,
            inotify_add_watch,
            inotify_init1,
            inotify_rm_watch,
            io_cancel,
            io_destroy,
            io_getevents,
            io_pgetevents,
            io_setup,
            io_submit,
            io_uring_enter,
            io_uring_register,
            io_uring_setup,
            ioctl,
            ioprio_get,
            ioprio_set,
            kcmp,
            kexec_file_load,
            kexec_load,
            keyctl,
            kill,
            landlock_add_rule,
            landlock_create_ruleset,
            landlock_restrict_self,
            lgetxattr,
            linkat,
            listen,
            listxattr,
            llistxattr,
            lookup_dcookie,
            lremovexattr,
            lseek,
            lsetxattr,
            madvise,
            mbind,
            membarrier,
            memfd_create,
            memfd_secret,
            migrate_pages,
            mincore,
            mkdirat,
            mknodat,
            mlock,
            mlock2,
            mlockall,
            mmap,
            mount,
            mount_setattr,
            move_mount,
            move_pages,
            mprotect,
            mq_getsetattr,
            mq_notify,
            mq_open,
            mq_timedreceive,
            mq_timedsend,
            mq_unlink,
            mremap,
            msgctl,
            msgget,
            msgrcv,
            msgsnd,
            msync,
            munlock,
            munlockall,
            munmap,
            name_to_handle_at,
            nanosleep,
            newfstatat,
            nfsservctl,
            open_by_handle_at,
            open_tree,
            openat,
            openat2,
            perf_event_open,
            personality,
            pidfd_getfd,
            pidfd_open,
            pidfd_send_signal,
            pipe2,
            pivot_root,
            pkey_alloc,
            pkey_free,
            pkey_mprotect,
            ppoll,
            prctl,
            pread64,
            preadv,
            preadv2,
            prlimit64,
            process_madvise,
            process_mrelease,
            process_vm_readv,
            process_vm_writev,
            pselect6,
            ptrace,
            pwrite64,
            pwritev,
            pwritev2,
            quotactl,
            quotactl_fd,
            read,
            readahead,
            readlinkat,
            readv,
            reboot,
            recvfrom,
            recvmmsg,
            recvmsg,
            remap_file_pages,
            removexattr,
            renameat2,
            request_key,
            restart_syscall,
            rseq,
            rt_sigaction,
            rt_sigpending,
            rt_sigprocmask,
            rt_sigqueueinfo,
            rt_sigreturn,
            rt_sigsuspend,
            rt_sigtimedwait,
            rt_tgsigqueueinfo,
            sched_get_priority_max,
            sched_get_priority_min,
            sched_getaffinity,
            sched_getattr,
            sched_getparam,
            sched_getscheduler,
            sched_rr_get_interval,
            sched_setaffinity,
            sched_setattr,
            sched_setparam,
            sched_setscheduler,
            sched_yield,
            seccomp,
            semctl,
            semget,
            semop,
            semtimedop,
            sendfile,
            sendmmsg,
            sendmsg,
            sendto,
            set_mempolicy,
            set_mempolicy_home_node,
            set_robust_list,
            set_tid_address,
            setdomainname,
            setfsgid,
            setfsuid,
            setgid,
            setgroups,
            sethostname,
            setitimer,
            setns,
            setpgid,
            setpriority,
            setregid,
            setresgid,
            setresuid,
            setreuid,
            setrlimit,
            setsid,
            setsockopt,
            settimeofday,
            setuid,
            setxattr,
            shmat,
            shmctl,
            shmdt,
            shmget,
            shutdown,
            sigaltstack,
            signalfd4,
            socket,
            socketpair,
            splice,
            statfs,
            statx,
            swapoff,
            swapon,
            symlinkat,
            sync,
            sync_file_range,
            syncfs,
            sysinfo,
            syslog,
            tee,
            tgkill,
            timer_create,
            timer_delete,
            timer_getoverrun,
            timer_gettime,
            timer_settime,
            timerfd_create,
            timerfd_gettime,
            timerfd_settime,
            times,
            tkill,
            truncate,
            umask,
            umount2,
            uname,
            unlinkat,
            unshare,
            userfaultfd,
            utimensat,
            vhangup,
            vmsplice,
            wait4,
            waitid,
            write,
            writev,

            // go/keep-sorted end
        }
    }
}

/// As above, but this calls through for the arch32 for arch support.
/// It's worth noting that common syscalls above are not common across
/// arches like x86 or arm.  The common list below reflects only
/// those common across arch32 arches.
///
/// Additionally, common syscalls are excluded if they have a different
/// implementation when compared to the main arch.
#[cfg(target_arch = "aarch64")]
#[macro_export]
macro_rules! for_each_arch32_syscall {
    {$callback:ident $(,$context:ident)*} => {
        $crate::for_each_arch_arch32_syscall!{
            $callback;
            $($context;)*
            ;

            // go/keep-sorted start

            // memfd_secret,  Not yet in our arm uapi
            _sysctl,
            accept,
            accept4,
            access,  // faccessat
            acct,
            add_key,
            adjtimex,
            bind,
            bpf,
            brk,
            cachestat,
            capget,
            capset,
            chdir,
            chroot,
            clock_adjtime,
            clock_getres,
            clock_gettime,
            clock_nanosleep,
            clock_settime,
            clone,
            clone3,
            close,
            close_range,
            connect,
            copy_file_range,
            delete_module,
            dup,
            dup3,
            epoll_create1,
            epoll_ctl,
            eventfd2,
            exit,
            exit_group,
            faccessat,
            faccessat2,
            fanotify_init,
            fchdir,
            fchmod,
            fchmodat,
            fchown,
            fchownat,
            fdatasync,
            fgetxattr,
            finit_module,
            flistxattr,
            flock,
            fremovexattr,
            fsconfig,
            fsetxattr,
            fsmount,
            fsopen,
            fspick,
            fsync,
            futex,
            futex_waitv,
            get_mempolicy,
            getcpu,
            getcwd,
            getdents64,
            getegid,
            geteuid,
            getgid,
            getgroups,
            getpeername,
            getpgid,
            getpid,
            getppid,
            getpriority,
            getrandom,
            getresgid,
            getresuid,
            getsid,
            getsockname,
            getsockopt,
            gettid,
            getuid,
            getuid32,
            getxattr,
            init_module,
            inotify_add_watch,
            inotify_init1,
            inotify_rm_watch,
            io_cancel,
            io_destroy,
            io_getevents,
            io_uring_enter,
            io_uring_register,
            io_uring_setup,
            ioprio_get,
            ioprio_set,
            kcmp,
            kexec_file_load,
            kill,
            landlock_add_rule,
            landlock_create_ruleset,
            landlock_restrict_self,
            lgetxattr,
            linkat,
            listen,
            listxattr,
            llistxattr,
            lookup_dcookie,
            lremovexattr,
            lsetxattr,
            madvise,
            mbind,
            membarrier,
            memfd_create,
            migrate_pages,
            mincore,
            mkdirat,
            mknodat,
            mlock,
            mlock2,
            mlockall,
            mount,
            mount_setattr,
            move_mount,
            move_pages,
            mprotect,
            mq_timedreceive,
            mq_timedsend,
            mq_unlink,
            mremap,
            msgget,
            msync,
            munlock,
            munlockall,
            munmap,
            name_to_handle_at,
            nanosleep,
            nfsservctl,
            open_tree,
            openat2,
            perf_event_open,
            personality,
            pidfd_getfd,
            pidfd_open,
            pidfd_send_signal,
            pipe2,
            pivot_root,
            pkey_alloc,
            pkey_free,
            pkey_mprotect,
            prctl,
            prlimit64,
            process_madvise,
            process_mrelease,
            process_vm_readv,
            process_vm_writev,
            quotactl,
            quotactl_fd,
            read,
            readlinkat,
            readv,
            reboot,
            remap_file_pages,
            removexattr,
            renameat2,
            request_key,
            restart_syscall,
            rseq,
            sched_get_priority_max,
            sched_get_priority_min,
            sched_getattr,
            sched_getparam,
            sched_getscheduler,
            sched_rr_get_interval,
            sched_setattr,
            sched_setparam,
            sched_setscheduler,
            sched_yield,
            seccomp,
            semget,
            semop,
            semtimedop,
            sendto,
            set_mempolicy,
            set_mempolicy_home_node,
            set_tid_address,
            setdomainname,
            setfsgid,
            setfsuid,
            setgid,
            setgroups,
            sethostname,
            setns,
            setpgid,
            setpriority,
            setregid,
            setresgid,
            setresuid,
            setreuid,
            setsid,
            setsockopt,
            setuid,
            setxattr,
            shmdt,
            shmget,
            shutdown,
            socket,
            socketpair,
            splice,
            statx,
            swapoff,
            swapon,
            symlinkat,
            sync,
            syncfs,
            syslog,
            tee,
            tgkill,
            timer_delete,
            timer_getoverrun,
            timer_gettime,
            timer_settime,
            timerfd_create,
            timerfd_gettime,
            timerfd_settime,
            tkill,
            umask,
            umount2,
            uname,
            unlinkat,
            unshare,
            userfaultfd,
            utimensat,
            vhangup,
            vmsplice,
            write,
            writev,

            // go/keep-sorted end
        }
    }
}

/// A particular invocation of a system call.
///
/// Contains the declaration of the invoked system call, as well as which arguments it was invoked
/// with.
pub struct Syscall {
    pub decl: SyscallDecl,
    pub arg0: SyscallArg,
    pub arg1: SyscallArg,
    pub arg2: SyscallArg,
    pub arg3: SyscallArg,
    pub arg4: SyscallArg,
    pub arg5: SyscallArg,
}

impl std::fmt::Debug for Syscall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?}({:#x}, {:#x}, {:#x}, {:#x}, {:#x}, {:#x})",
            self.decl, self.arg0, self.arg1, self.arg2, self.arg3, self.arg4, self.arg5
        )
    }
}

/// Evaluates to a string literal for the given syscall number when called back by for_each_syscall.
#[macro_export]
macro_rules! syscall_number_to_name_literal_callback {
    {$number:ident; $($name:ident,)*} => {
        $crate::__paste::paste! {
            match $number as u32 {
                $(starnix_uapi::[<__NR_ $name>] => stringify!($name),)*
                _ => "<unknown syscall>",
            }
        }
    }
}

// As above, but for the arch32 namespace.
#[cfg(target_arch = "aarch64")]
#[macro_export]
macro_rules! syscall_arch32_number_to_name_literal_callback {
    {$number:ident; $($name:ident,)*} => {
        $crate::__paste::paste! {
            match $number as u32 {
                $(starnix_uapi::arch32::[<__NR_ $name>] => stringify!($name),)*
                _ => "<unknown syscall>",
            }
        }
    }
}

/// Evaluates to a str literal for the given syscall number when called back by for_each_syscall.
#[macro_export]
macro_rules! syscall_number_to_trace_name_callback {
    {$number:ident; $($name:ident,)*} => {
        $crate::__paste::paste! {
            match $number as u32 {
                $(starnix_uapi::[<__NR_ $name>] => concat!("sys_", stringify!($name)),)*
                _ => "<unknown syscall>",
            }
        }
    }
}

// As above, but for the arch32 namespace.
#[cfg(target_arch = "aarch64")]
#[macro_export]
macro_rules! syscall_arch32_number_to_trace_name_callback {
    {$number:ident; $($name:ident,)*} => {
        $crate::__paste::paste! {
            match $number as u32 {
                $(starnix_uapi::arch32::[<__NR_ $name>] => concat!("sys_", stringify!($name)),)*
                _ => "<unknown syscall>",
            }
        }
    }
}

/// A system call declaration.
///
/// Describes the name of the syscall and its number.
#[derive(Copy, Clone)]
pub struct SyscallDecl {
    pub number: u64,

    #[cfg_attr(target_arch = "x86_64", allow(unused))]
    pub arch_width: ArchWidth,
}

impl SyscallDecl {
    /// The SyscallDecl for the given syscall number.
    ///
    /// Returns &DECL_UNKNOWN if the given syscall number is not known.
    pub fn from_number(number: u64, arch_width: ArchWidth) -> SyscallDecl {
        Self { arch_width, number }
    }

    pub fn name(&self) -> &'static str {
        // Make a binding because the macro makes it difficult to pass a member variable.
        let number = self.number;

        #[cfg(target_arch = "aarch64")]
        if self.arch_width.is_arch32() {
            return for_each_arch32_syscall! { syscall_arch32_number_to_name_literal_callback, number };
        }

        for_each_syscall! { syscall_number_to_name_literal_callback, number }
    }

    pub fn trace_name(&self) -> &'static str {
        // Make a binding because the macro makes it difficult to pass a member variable.
        let number = self.number;

        #[cfg(target_arch = "aarch64")]
        if self.arch_width.is_arch32() {
            return for_each_arch32_syscall! { syscall_arch32_number_to_trace_name_callback, number };
        }

        for_each_syscall! { syscall_number_to_trace_name_callback, number }
    }
}

impl std::fmt::Debug for SyscallDecl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.name(), self.number)
    }
}
