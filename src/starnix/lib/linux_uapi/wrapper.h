// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_LIB_LINUX_UAPI_WRAPPER_H_
#define SRC_STARNIX_LIB_LINUX_UAPI_WRAPPER_H_

#include <stddef.h>
#include <sys/socket.h>
#include <typedefs.h>

#include <asm/ioctls.h>
#include <asm/poll.h>
#include <asm/sigcontext.h>
#include <asm/signal.h>
#include <asm/socket.h>
#include <asm/stat.h>
#include <asm/statfs.h>
#include <linux/aio_abi.h>
#include <linux/android/binder.h>
#include <linux/android/binderfs.h>
#include <linux/ashmem.h>
#include <linux/audit.h>
#include <linux/auxvec.h>
#include <linux/capability.h>
#include <linux/close_range.h>
#include <linux/dm-ioctl.h>
#include <linux/dma-buf.h>
#include <linux/dma-heap.h>
#include <linux/elf.h>
#include <linux/errno.h>
#include <linux/eventpoll.h>
#include <linux/fadvise.h>
#include <linux/falloc.h>
#include <linux/fb.h>
#include <linux/fcntl.h>
#include <linux/fib_rules.h>
#include <linux/filter.h>
#include <linux/fs.h>
#include <linux/fscrypt.h>
#include <linux/fsverity.h>
#include <linux/futex.h>
#include <linux/if_arp.h>
#include <linux/if_packet.h>
#include <linux/if_tun.h>
#include <linux/inotify.h>
#include <linux/input.h>
#include <linux/io_uring.h>
#include <linux/ip.h>
#include <linux/ipv6.h>
#include <linux/kcmp.h>
#include <linux/limits.h>
#include <linux/loop.h>
#include <linux/magic.h>
#include <linux/membarrier.h>
#include <linux/memfd.h>
#include <linux/mman.h>
#include <linux/mqueue.h>
#include <linux/netfilter/nf_nat.h>
#include <linux/netfilter/xt_TPROXY.h>
#include <linux/netfilter/xt_bpf.h>
#include <linux/netfilter/xt_mark.h>
#include <linux/netfilter_ipv4/ip_tables.h>
#include <linux/netfilter_ipv4/ipt_REJECT.h>
#include <linux/netfilter_ipv6/ip6_tables.h>
#include <linux/netfilter_ipv6/ip6t_REJECT.h>
#include <linux/netlink.h>
#include <linux/oom.h>
#include <linux/perf_event.h>
#include <linux/personality.h>
#include <linux/pfkeyv2.h>
#include <linux/pidfd.h>
#include <linux/poll.h>
#include <linux/prctl.h>
#include <linux/ptrace.h>
#include <linux/random.h>
#include <linux/reboot.h>
#include <linux/resource.h>
#include <linux/rtc.h>
#include <linux/rtnetlink.h>
#include <linux/sched.h>
#include <linux/sched/types.h>
#include <linux/seccomp.h>
#include <linux/securebits.h>
#include <linux/signal.h>
#include <linux/signalfd.h>
#include <linux/sock_diag.h>
#include <linux/socket.h>
#include <linux/sockios.h>
#include <linux/stat.h>
#include <linux/sync_file.h>
#include <linux/sysinfo.h>
#include <linux/taskstats.h>
#include <linux/termios.h>
#include <linux/thermal.h>
#include <linux/time.h>
#include <linux/timerfd.h>
#include <linux/times.h>
#include <linux/uinput.h>
#include <linux/uio.h>
#include <linux/un.h>
#include <linux/unistd.h>
#include <linux/usb/functionfs.h>
#include <linux/userfaultfd.h>
#include <linux/utsname.h>
#include <linux/vm_sockets.h>
#include <linux/wait.h>
#include <linux/xattr.h>
#include <linux/xfrm.h>

#ifndef __arm__
#include <asm/ucontext.h>
#include <linux/bpf.h>
#include <linux/bpf_perf_event.h>

#endif  // #ifndef __arm__

#ifndef __KERNEL__
#define __KERNEL__ 1
#define __HAS_KERNEL__ 0
#else
#define __HAS_KERNEL__ 1
#endif
// Specific fuse header to use android specific extensions
#include "src/starnix/lib/linux_uapi/third_party/libfuse/fuse_kernel.h"
#if __HAS_KERNEL__
#undef __KERNEL__
#endif
#undef __HAS_KERNEL__

// Android kgsl header
#include "src/starnix/lib/linux_uapi/stub/kgsl/msm_kgsl.h"

// Custom driver headers
#include "src/starnix/lib/linux_uapi/stub/drivers/char/adsprpc_shared.h"

#ifdef __x86_64__
#include <asm/prctl.h>
#endif

#ifdef __arm__
// Hand-crafted file in stub which provides
//   __NR_arch32_[name]
// when the implementation of [name] differs
// between 32-bit and 64-bit.
#include <arm_arch32_syscalls.h>

// Definition of ucontext
#include <arm_arch32_ucontext.h>
#endif

#include <fcntl.h>

#include "stub/missing_includes.h"

#endif  // SRC_STARNIX_LIB_LINUX_UAPI_WRAPPER_H_
