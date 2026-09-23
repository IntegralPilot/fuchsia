// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_MMIO_PTR_MMIO_PTR_H_
#define LIB_MMIO_PTR_MMIO_PTR_H_

#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <zircon/compiler.h>

// Low level API for reading and writing to Memory-Mapped I/O buffers.
//
// The MMIO_PTR qualifier can be added to pointer type declarations to indicate that
// it can point to MMIO memory. The syntax is similar to `const` or `volatile`
// in that it can be placed before or after to the pointed-to type.
//
// Example: These all have the same type.
//
//   MMIO_PTR const uint8_t* buffer;
//   const MMIO_PTR uint8_t* buffer2;
//   const uint8_t MMIO_PTR* buffer3;
//
// MMIO_PTR pointers cannot be accessed directly through pointer indirection,
// array subscripting, or member access operators. They should only be accessed
// through this API.
//
// Example:
//
//   MMIO_PTR uint8_t* mmio_pointer;
//
//   // Incorrect way. The compiler complains with:
//   // warning: dereferencing y; was declared with a 'noderef' type [-Wnoderef]
//   //   value = *mmio_pointer;
//   //           ^~~~~~~~~~~~~
//   value = *mmio_pointer;
//
//   // Correct way
//   value = MmioRead8(mmio_pointer);

#ifdef __clang__
// The address space number needs to be unique, but the value is not necessarily
// meaningful except for a few reserved values for LLVM on specific machines.
// Address spaces 256, 257, and 258 are examples of reserved address spaces in
// LLVM for X86.
// TODO(https://fxbug.dev/42101561): It would be better if the compiler could accept string
// arguments to address_space since the number is arbitrary and just needs to be unique.
#define MMIO_PTR __attribute__((noderef, address_space(100)))
#else
// On other compilers, the qualifier has no effect and prohibited accesses are not
// detected.
#define MMIO_PTR
#endif

#ifdef __aarch64__

// The Linux/ARM64 KVM hypervisor does not support MMIO_PTR access via
// load/store instructions that use writeback, which the compiler might decide
// to generate.  (The ARM64 virtualization hardware requires software
// assistance for the writeback forms but not for the non-writeback forms, and
// KVM just doesn't bother to implement that software assistance.)  To minimize
// the demands on a hypervisor we might run under, we use inline assembly
// definitions here to ensure that only the non-writeback load/store
// instructions are used.

__NONNULL((2))
static inline void MmioWrite8(uint8_t data, MMIO_PTR volatile uint8_t* buffer) {
  __asm__ volatile("strb %w1, %0" : "=m"(*(volatile uint8_t*)buffer) : "r"(data) : "memory");
}

__NONNULL((2))
static inline void MmioWrite16(uint16_t data, MMIO_PTR volatile uint16_t* buffer) {
  __asm__ volatile("strh %w1, %0" : "=m"(*(volatile uint16_t*)buffer) : "r"(data) : "memory");
}

__NONNULL((2))
static inline void MmioWrite32(uint32_t data, MMIO_PTR volatile uint32_t* buffer) {
  __asm__ volatile("str %w1, %0" : "=m"(*(volatile uint32_t*)buffer) : "r"(data) : "memory");
}

__NONNULL((2))
static inline void MmioWrite64(uint64_t data, MMIO_PTR volatile uint64_t* buffer) {
  __asm__ volatile("str %1, %0" : "=m"(*(volatile uint64_t*)buffer) : "r"(data) : "memory");
}

__NONNULL((1))
static inline uint8_t MmioRead8(MMIO_PTR const volatile uint8_t* buffer) {
  uint8_t data;
  __asm__ volatile("ldrb %w0, %1" : "=r"(data) : "m"(*(volatile uint8_t*)buffer) : "memory");
  return data;
}

__NONNULL((1))
static inline uint16_t MmioRead16(MMIO_PTR const volatile uint16_t* buffer) {
  uint16_t data;
  __asm__ volatile("ldrh %w0, %1" : "=r"(data) : "m"(*(volatile uint16_t*)buffer) : "memory");
  return data;
}

__NONNULL((1))
static inline uint32_t MmioRead32(MMIO_PTR const volatile uint32_t* buffer) {
  // This is actually a 32-bit load by dint of using the w<n> register name in
  // the instruction via the %w modifier.  That instruction _should_ always
  // clear the high bits.  However, some buggy "hardware" (really, buggy
  // hypervisor or secure monitor implementations) has been observed to leave
  // the high bits in the register.  The compiler expects the norm that a
  // 32-bit register operand w<n> means the corresponding x<n> is then known to
  // have all-zero high bits (a uint32_t-typed lvalue means a 32-bit operand).
  // So with `uint32_t` here, it might then elide any widening to uint64_t in a
  // caller's use of the value.  By using `uint64_t` here the compiler thinks
  // the full 64-bit x<n> register might contain any unknown value, even though
  // the %w prefix means the 32-bit w<n> register is what's actually used in
  // the assembly instruction.  Then the explicit cast in the return requires
  // the compiler to do the narrowing that masks off the high bits; this is
  // likely free just by doing a later mov from w<n> instead of x<n>.
  uint64_t data;
  __asm__ volatile("ldr %w0, %1" : "=r"(data) : "m"(*(volatile uint32_t*)buffer) : "memory");
  return (uint32_t)data;
}

__NONNULL((1))
static inline uint64_t MmioRead64(MMIO_PTR const volatile uint64_t* buffer) {
  uint64_t data;
  __asm__ volatile("ldr %0, %1" : "=r"(data) : "m"(*(volatile uint64_t*)buffer) : "memory");
  return data;
}

#elif defined(__x86_64__) || defined(__i386__)

// Some versions the Linux/x86 KVM Hypervisor do not support writing to MMIOs
// via `mov` instructions from vector registers, which the compiler might
// generate.  To minimize the demands on a hypervisor we might run under, we
// use inline assembly definitions here to ensure that only the simple integer
// `mov` instructions are used.

__NONNULL((2))
static inline void MmioWrite8(uint8_t data, MMIO_PTR volatile uint8_t* buffer) {
  __asm__ volatile("movb %1, %0" : "=m"(*(volatile uint8_t*)buffer) : "ir"(data));
}

__NONNULL((2))
static inline void MmioWrite16(uint16_t data, MMIO_PTR volatile uint16_t* buffer) {
  __asm__ volatile("movw %1, %0" : "=m"(*(volatile uint16_t*)buffer) : "ir"(data));
}

__NONNULL((2))
static inline void MmioWrite32(uint32_t data, MMIO_PTR volatile uint32_t* buffer) {
  __asm__ volatile("movl %1, %0" : "=m"(*(volatile uint32_t*)buffer) : "ir"(data));
}

__NONNULL((1))
static inline uint8_t MmioRead8(MMIO_PTR const volatile uint8_t* buffer) {
  uint8_t data;
  __asm__ volatile("movb %1, %0" : "=r"(data) : "m"(*(volatile uint8_t*)buffer));
  return data;
}

__NONNULL((1))
static inline uint16_t MmioRead16(MMIO_PTR const volatile uint16_t* buffer) {
  uint16_t data;
  __asm__ volatile("movw %1, %0" : "=r"(data) : "m"(*(volatile uint16_t*)buffer));
  return data;
}

__NONNULL((1))
static inline uint32_t MmioRead32(MMIO_PTR const volatile uint32_t* buffer) {
  uint32_t data;
  __asm__ volatile("movl %1, %0" : "=r"(data) : "m"(*(volatile uint32_t*)buffer));
  return data;
}

#ifdef __x86_64__

__NONNULL((2))
static inline void MmioWrite64(uint64_t data, MMIO_PTR volatile uint64_t* buffer) {
  __asm__ volatile("movq %1, %0" : "=m"(*(volatile uint64_t*)buffer) : "er"(data));
}

__NONNULL((1))
static inline uint64_t MmioRead64(MMIO_PTR const volatile uint64_t* buffer) {
  uint64_t data;
  __asm__ volatile("movq %1, %0" : "=r"(data) : "m"(*(volatile uint64_t*)buffer));
  return data;
}

#endif  // __x86_64__

#elif defined(__riscv)

// RISC-V doesn't have any fancier load/store flavors anyway.

__NONNULL((2))
static inline void MmioWrite8(uint8_t data, MMIO_PTR volatile uint8_t* buffer) {
  *(volatile uint8_t*)buffer = data;
}

__NONNULL((2))
static inline void MmioWrite16(uint16_t data, MMIO_PTR volatile uint16_t* buffer) {
  *(volatile uint16_t*)buffer = data;
}

__NONNULL((2))
static inline void MmioWrite32(uint32_t data, MMIO_PTR volatile uint32_t* buffer) {
  *(volatile uint32_t*)buffer = data;
}

__NONNULL((2))
static inline void MmioWrite64(uint64_t data, MMIO_PTR volatile uint64_t* buffer) {
  *(volatile uint64_t*)buffer = data;
}

__NONNULL((1))
static inline uint8_t MmioRead8(MMIO_PTR const volatile uint8_t* buffer) {
  return *(volatile uint8_t*)buffer;
}

__NONNULL((1))
static inline uint16_t MmioRead16(MMIO_PTR const volatile uint16_t* buffer) {
  return *(volatile uint16_t*)buffer;
}

__NONNULL((1))
static inline uint32_t MmioRead32(MMIO_PTR const volatile uint32_t* buffer) {
  return *(volatile uint32_t*)buffer;
}

__NONNULL((1))
static inline uint64_t MmioRead64(MMIO_PTR const volatile uint64_t* buffer) {
  return *(volatile uint64_t*)buffer;
}

#else

// We may need other machine-specific implementations here in the future.
#error "No MMIO access implementation for this arch."

#endif

// MmioReadBuffer/MmioWriteBuffer provide methods for doing bulk/memcpy style transfers to/from
// device memory. These methods to not provide any access width guarantees and as such should only
// be used in situations where this is acceptable.
//
// For example, do _not_ use these functions to dump a bank of 32-bit MMIO registers, but rather
// use it only for accessing the small banks of RAM or ROM which might exist inside of a device
__NONNULL((1, 2))
static inline void MmioWriteBuffer(MMIO_PTR volatile void* mmio, const void* source, size_t size) {
#ifdef _LP64
  const size_t kMaxSize = 8;
#else
  const size_t kMaxSize = 4;
#endif
  uintptr_t source_ptr = (uintptr_t)source;
  uintptr_t mmio_ptr = (uintptr_t)mmio;
  while ((mmio_ptr & (kMaxSize - 1)) && size > 0) {
    MmioWrite8(*((const uint8_t*)source_ptr), (MMIO_PTR volatile uint8_t*)mmio_ptr);
    --size;
    ++mmio_ptr;
    ++source_ptr;
  }
  while (size >= kMaxSize) {
#ifdef _LP64
    uint64_t tmp;
    memcpy(&tmp, (const void*)source_ptr, sizeof(uint64_t));
    MmioWrite64(tmp, (MMIO_PTR volatile uint64_t*)mmio_ptr);
#else
    uint32_t tmp;
    memcpy(&tmp, (const void*)source_ptr, sizeof(uint32_t));
    MmioWrite32(tmp, (MMIO_PTR volatile uint32_t*)mmio_ptr);
#endif
    size -= kMaxSize;
    mmio_ptr += kMaxSize;
    source_ptr += kMaxSize;
  }
  while (size > 0) {
    MmioWrite8(*((const uint8_t*)source_ptr), (MMIO_PTR volatile uint8_t*)mmio_ptr);
    --size;
    ++mmio_ptr;
    ++source_ptr;
  }
}

__NONNULL((1, 2))
static inline void MmioReadBuffer(void* dest, MMIO_PTR const volatile void* mmio, size_t size) {
#ifdef _LP64
  const size_t kMaxSize = 8;
#else
  const size_t kMaxSize = 4;
#endif
  uintptr_t dest_ptr = (uintptr_t)dest;
  uintptr_t mmio_ptr = (uintptr_t)mmio;
  while ((mmio_ptr & (kMaxSize - 1)) && size > 0) {
    *((uint8_t*)dest_ptr) = MmioRead8((MMIO_PTR const volatile uint8_t*)mmio_ptr);
    --size;
    ++mmio_ptr;
    ++dest_ptr;
  }
  while (size >= kMaxSize) {
#ifdef _LP64
    *((uint64_t*)dest_ptr) = MmioRead64((MMIO_PTR const volatile uint64_t*)mmio_ptr);
#else
    *((uint32_t*)dest_ptr) = MmioRead32((MMIO_PTR const volatile uint32_t*)mmio_ptr);
#endif
    size -= kMaxSize;
    mmio_ptr += kMaxSize;
    dest_ptr += kMaxSize;
  }
  while (size > 0) {
    *((uint8_t*)dest_ptr) = MmioRead8((MMIO_PTR const volatile uint8_t*)mmio_ptr);
    --size;
    ++mmio_ptr;
    ++dest_ptr;
  }
}

#ifdef __cplusplus

// In C++ overloads allow using one name for all types.  C++ would usually use
// references rather than pointers to indicate non-nullability, but `noderef`
// pointers don't allow creating references since that's the same as
// dereferencing the pointer.

__NONNULL((2)) inline void MmioWrite(uint8_t data, MMIO_PTR volatile uint8_t* buffer) {
  MmioWrite8(data, buffer);
}

__NONNULL((2)) inline void MmioWrite(uint16_t data, MMIO_PTR volatile uint16_t* buffer) {
  MmioWrite16(data, buffer);
}

__NONNULL((2)) inline void MmioWrite(uint32_t data, MMIO_PTR volatile uint32_t* buffer) {
  MmioWrite32(data, buffer);
}

__NONNULL((1)) inline uint8_t MmioRead(MMIO_PTR const volatile uint8_t* buffer) {
  return MmioRead8(buffer);
}

__NONNULL((1)) inline uint16_t MmioRead(MMIO_PTR const volatile uint16_t* buffer) {
  return MmioRead16(buffer);
}

__NONNULL((1)) inline uint32_t MmioRead(MMIO_PTR const volatile uint32_t* buffer) {
  return MmioRead32(buffer);
}

#ifdef _LP64

__NONNULL((2)) inline void MmioWrite(uint64_t data, MMIO_PTR volatile uint64_t* buffer) {
  MmioWrite64(data, buffer);
}

__NONNULL((1)) inline uint64_t MmioRead(MMIO_PTR const volatile uint64_t* buffer) {
  return MmioRead64(buffer);
}

#endif  // _LP64

#endif  // __cplusplus

#endif  // LIB_MMIO_PTR_MMIO_PTR_H_
