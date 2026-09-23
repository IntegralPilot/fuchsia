// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_INTERRUPT_INCLUDE_DEV_INTERRUPT_H_
#define ZIRCON_KERNEL_DEV_INTERRUPT_INCLUDE_DEV_INTERRUPT_H_

#include <sys/types.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <kernel/cpu.h>
#include <kernel/mp.h>

__BEGIN_CDECLS

constexpr uint32_t MAX_MSI_IRQS = 32;
constexpr uint32_t MAX_INTERRUPTS = 1024;

using interrupt_vector_t = uint32_t;

struct interrupt_handler_t {
  void* cookie = nullptr;
  void (*fn)(void*) = nullptr;

  interrupt_handler_t() = default;
  interrupt_handler_t(decltype(nullptr)) : cookie(nullptr), fn(nullptr) {}
  interrupt_handler_t(void (*f)(void*)) : cookie(nullptr), fn(f) {}
  interrupt_handler_t(void* c, void (*f)(void*)) : cookie(c), fn(f) {}

  explicit operator bool() const { return fn != nullptr; }
};
static_assert(sizeof(interrupt_handler_t) == 16);

enum class interrupt_trigger_mode : uint32_t {
  EDGE,
  LEVEL,
};

constexpr const char* interrupt_trigger_mode_string(interrupt_trigger_mode mode) {
  switch (mode) {
    case interrupt_trigger_mode::EDGE:
      return "edge";
    case interrupt_trigger_mode::LEVEL:
      return "level";
  }
  return "unknown";
}

enum class interrupt_polarity : uint32_t {
  HIGH,
  LOW,
};

constexpr const char* interrupt_polarity_string(interrupt_polarity pol) {
  switch (pol) {
    case interrupt_polarity::HIGH:
      return "high";
    case interrupt_polarity::LOW:
      return "low";
  }
  return "unknown";
}

// (mask|unmask)_interrupt will disable (mask) or enable (unmask) the interrupt
// specified by |vector|.  In the case of interrupts which target a specific CPU
// but share an interrupt vector number (for example, SGIs and PPIs in ARM's GIC
// interrupt controller designs), these functions will affect the
// masked/unmasked state of the interrupt vector for the caller's CPU only.
// They will not mask/unmask the specified vector for any of the other CPUs.
zx_status_t mask_interrupt(interrupt_vector_t vector);
zx_status_t unmask_interrupt(interrupt_vector_t vector);
zx_status_t deactivate_interrupt(interrupt_vector_t vector);

void shutdown_interrupts();

// Shutdown interrupts for the calling CPU.
//
// Should be called before powering off the calling CPU.
extern "C" void shutdown_interrupts_curr_cpu();

// Suspend interrupts for the calling CPU.
//
// Should be called before entering a hardware suspend state.
zx_status_t suspend_interrupts_curr_cpu();

// Resume interrupts for the calling CPU.
//
// Should be called after coming out of a hardware suspend state.
zx_status_t resume_interrupts_curr_cpu();

// Configure the specified interrupt vector.  If it is invoked, it muust be
// invoked prior to interrupt registration
zx_status_t configure_interrupt(interrupt_vector_t vector, interrupt_trigger_mode tm,
                                interrupt_polarity pol);

zx_status_t get_interrupt_config(interrupt_vector_t vector, interrupt_trigger_mode* tm,
                                 interrupt_polarity* pol);

// Set the affinity for an interrupt. Intrinsically set to cpu 0 by default.
zx_status_t set_interrupt_affinity(interrupt_vector_t vector, cpu_mask_t mask);

// Registers a handler to be called for the given interrupt vector. The handler may be called
// with internal spinlocks held and should not itself call register_int_handler. This handler may
// be serialized with other handlers.
// This can be called repeatedly to change the handler/arg for a given vector.
//
// To unregister an interrupt handler (destroying any captured lambda in the process), users should
// call `register_int_handler(irq_num, nullptr);`.
//
// Notes about thread safety:
//
// When non-permanent interrupts (interrupts registered using |register_int_handler| instead of
// |register_permanent_int_handler|) are dispatched, an internal lock is held for the duration of
// the dispatch. The same lock is held when registering/unregistering an interrupt.  This leads to
// the following important side effects:
//
// 1) A call to |register_int_handler| automatically synchronizes with any IRQ in flight.  After the
//    call returns, any previously registered handler is guaranteed to have no in-flight callers,
//    and it is safe to destroy any resources associated with previous handler.
// 2) It is *never* safe to call |register_int_handler| from a user's interrupt handler
//    implementation.
// 3) If any locks might be obtained in a user's interrupt handler, none of those locks may be held
//    when making a call to |register_int_handler| from outside of a handler implementation.  Doing
//    so sets up potential deadlock via the classic A/B deadlock pattern.
//
zx_status_t register_int_handler(interrupt_vector_t vector, interrupt_handler_t handler);

// A simple alias for `register_int_handler(irq_num, nullptr)` for those who prefer it.
static inline zx_status_t unregister_int_handler(interrupt_vector_t vector) {
  return register_int_handler(vector, nullptr);
}

// Registers a handler to be called for the given interrupt vector. Once this is used to set a
// handler it is an error to modify the vector again through this or register_int_handler.
// Registration via this method allows the interrupt manager to avoid needing to synchronize
// re-registrations with invocations, which can be much more efficient and avoid unneeded
// serialization of handlers.
zx_status_t register_permanent_int_handler(interrupt_vector_t vector, interrupt_handler_t handler);

// These return the [base, max] range of vectors that can be used with zx_interrupt syscalls
// This api will need to evolve if valid vector ranges later are not contiguous
uint32_t interrupt_get_base_vector();
uint32_t interrupt_get_max_vector();

bool is_valid_interrupt(interrupt_vector_t vector, uint32_t flags);

interrupt_vector_t remap_interrupt(interrupt_vector_t vector);

// sends an inter-processor interrupt
zx_status_t interrupt_send_ipi(cpu_mask_t target, mp_ipi ipi);

// performs per-cpu initialization for the interrupt controller
void interrupt_init_percpu();

// A structure which holds the state of a block of IRQs allocated by the
// platform to be used for delivering MSI or MSI-X interrupts.
struct msi_block_t {
  uint64_t tgt_addr;  // The target write transaction physical address
  // The data which the device should write when triggering an IRQ.  Note,
  // only the lower 16 bits are used when the block has been allocated for MSI
  // instead of MSI-X
  uint32_t tgt_data;
  uint32_t base_irq_id;  // The first IRQ id in the allocated block
  uint32_t num_irq;      // The number of irqs in the allocated block
  bool allocated;        // Whether or not this block has been allocated
  bool is_32bit;         // 32 bit if true, 64 bit otherwise
};

static_assert(sizeof(msi_block_t) == 24, "msi_block_t size mismatch");
static_assert(alignof(msi_block_t) == 8, "msi_block_t alignment mismatch");

// Methods used to determine if a platform supports MSI or not, and if so,
// whether or not the platform can mask individual MSI vectors at the
// platform level.
//
// If the platform supports MSI, it must supply valid implementations of
// msi_alloc_block, msi_free_block, and msi_register_handler.
//
// If the platform supports MSI masking, it must supply a valid
// implementation of MaskUnmaskMsi.
bool msi_is_supported();
bool msi_supports_masking();
void msi_mask_unmask(const msi_block_t* block, uint msi_id, bool mask);

// Method used for platform allocation of blocks of MSI and MSI-X compatible
// IRQ targets.
//
// @param requested_irqs The total number of irqs being requested.
// @param can_target_64bit True if the target address of the MSI block can
//        be located past the 4GB boundary.  False if the target address must be
//        in low memory.
// @param is_msix True if this request is for an MSI-X compatible block.  False
//        for plain old MSI.
// @param out_block A pointer to the allocation bookkeeping to be filled out
//        upon successful allocation of the requested block of IRQs.
//
// @return A status code indicating the success or failure of the operation.
zx_status_t msi_alloc_block(uint requested_irqs, bool can_target_64bit, bool is_msix,
                            msi_block_t* out_block);

// Method used to free a block of MSI IRQs previously allocated by msi_alloc_block().
// This does not unregister IRQ handlers.
//
// @param block A pointer to the block to be returned
void msi_free_block(msi_block_t* block);

// Register a handler function for a given msi_id within an msi_block. Passing a
// NULL handler will effectively unregister a handler for a given msi_id within the
// block.
extern "C" void msi_register_handler(const msi_block_t* block, uint msi_id,
                                     interrupt_handler_t handler);

__END_CDECLS

#endif  // ZIRCON_KERNEL_DEV_INTERRUPT_INCLUDE_DEV_INTERRUPT_H_
