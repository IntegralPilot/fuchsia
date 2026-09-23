// TODO(https://fxbug.dev/42074781): Remove these types since no libc interfaces use them.
#if defined(_POSIX_SOURCE) || defined(_POSIX_C_SOURCE) || defined(_XOPEN_SOURCE) || \
    defined(_GNU_SOURCE) || defined(_BSD_SOURCE)

struct sigaltstack {
  void* ss_sp;
  int ss_flags;
  size_t ss_size;
};

#define SA_NOCLDSTOP 1
#define SA_NOCLDWAIT 2
#define SA_SIGINFO 4
#define SA_ONSTACK 0x08000000
#define SA_RESTART 0x10000000
#define SA_NODEFER 0x40000000
#define SA_RESETHAND 0x80000000
#define SA_RESTORER 0x04000000

#define SIGSTKSZ 12288
#define MINSIGSTKSZ 6144

#if defined(__x86_64__)

#ifdef _GNU_SOURCE
#define REG_R8 0
#define REG_R9 1
#define REG_R10 2
#define REG_R11 3
#define REG_R12 4
#define REG_R13 5
#define REG_R14 6
#define REG_R15 7
#define REG_RDI 8
#define REG_RSI 9
#define REG_RBP 10
#define REG_RBX 11
#define REG_RDX 12
#define REG_RAX 13
#define REG_RCX 14
#define REG_RSP 15
#define REG_RIP 16
#define REG_EFL 17
#define REG_CSGSFS 18
#define REG_ERR 19
#define REG_TRAPNO 20
#define REG_OLDMASK 21
#define REG_CR2 22
#endif
typedef long long greg_t, gregset_t[23];
typedef struct _fpstate {
  unsigned short cwd, swd, ftw, fop;
  unsigned long long rip, rdp;
  unsigned mxcsr, mxcr_mask;
  struct {
    unsigned short significand[4], exponent, padding[3];
  } _st[8];
  struct {
    unsigned element[4];
  } _xmm[16];
  unsigned padding[24];
} * fpregset_t;
typedef struct {
  gregset_t gregs;
  fpregset_t fpregs;
  unsigned long long __reserved1[8];
} mcontext_t;
typedef struct __ucontext {
  unsigned long uc_flags;
  struct __ucontext* uc_link;
  stack_t uc_stack;
  mcontext_t uc_mcontext;
  sigset_t uc_sigmask;
  unsigned long __fpregs_mem[64];
} ucontext_t;

#elif defined(__aarch64__)

typedef struct sigcontext {
  unsigned long fault_address;
  unsigned long regs[31];
  unsigned long sp, pc, pstate;
  long double __reserved[256];
} mcontext_t;

typedef struct __ucontext {
  unsigned long uc_flags;
  struct ucontext* uc_link;
  stack_t uc_stack;
  sigset_t uc_sigmask;
  mcontext_t uc_mcontext;
} ucontext_t;

#else

typedef struct sigcontext {
  unsigned long regs[32];  // pc is in place of x0 (zero)
} mcontext_t;

typedef struct __ucontext {
  unsigned long uc_flags;
  struct ucontext* uc_link;
  stack_t uc_stack;
  sigset_t uc_sigmask;
  mcontext_t uc_mcontext;
} ucontext_t;

#endif

#endif
