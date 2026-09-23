// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_PHYS_INCLUDE_VM_PHYS_ARENA_H_
#define ZIRCON_KERNEL_VM_PHYS_INCLUDE_VM_PHYS_ARENA_H_

#include <lib/fit/result.h>
#include <lib/memalloc/range.h>
#include <stdint.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>

#include <bit>
#include <optional>
#include <span>
#include <string_view>
#include <type_traits>
#include <utility>

// The maximum number of wasted/unusable bytes the arena selection process
// should tolerate. These correspond to the bookkeeping slots used to account
// for page-sized holes in between actual RAM within an arena.
constexpr uint64_t kMaxWastedArenaBytes = 0x8'4000;  // 528 KiB

// The size and alignment of page bookkeeping entries within an arena (i.e.,
// `vm_page_t`). A static assertion in the kernel ensures that these are
// up-to-date.
constexpr uint64_t kArenaPageBookkeepingSize = 48;
constexpr uint64_t kArenaPageBookkeepingAlignment = 8;

// A selected arena and the location of its page bookkeeping within.
struct PmmArenaSelection {
  struct Range {
    constexpr uint64_t end() const { return base + size; }

    uint64_t base = 0;
    uint64_t size = 0;
  };

  Range arena;
  Range bookkeeping;

  // The aggregate number of wasted bytes (i.e., those corresponding to
  // bookkeeping slots accounting for page-sized holes in between actual RAM).
  uint64_t wasted_bytes;
};

// Represents an error that occurred within the arena selection process in trying
// to start an arena at the associated range.
struct PmmArenaSelectionError {
  enum class Type {
    // No run of free RAM was sufficiently large to account for the associated
    // would-be arena.
    kNoBookkeepingSpace,

    // The range itself was too small, resulting in the aligned arena either
    // being empty or only fitting its bookkeeping (which is not useful).
    kTooSmall,
  };

  static constexpr std::string_view ToString(Type type) {
    using namespace std::string_view_literals;

    switch (type) {
      case Type::kNoBookkeepingSpace:
        return "no bookkeeping space found"sv;
      case Type::kTooSmall:
        return "too small (less than two pages in size)"sv;
    }
    __UNREACHABLE;
  }

  memalloc::Range range;
  Type type;
};

// Selects PMM arenas from a provided set of memory ranges, identifying suitable
// bookkeeping space within them. Some of the provided ranges may specify
// allocations made before PMM initialization; bookkeeping will be placed so as
// to avoid these.
//
// Selected arenas may encompass 'holes' in RAM, as it may sometimes be
// preferable to include them to bridge disjoint RAM within the same arena and
// keep the total count of arenas low (versus avoiding the hole by creating two
// arenas). The tolerance for the inclusion of holes is parameterized by
// `kMaxWastedArenaBytes` above, which specifies the maximum amount of unusable
// bookkeeping bytes we wish to dedicate to pages within a hole.
//
// Selected arenas (in `PmmArenaSelection` form) are passed to the supplied
// `arenas` callback, while encountered errors (in `PmmArenaSelectionError`
// form) are passed to the supplied `errors` callback. Emitted arena and
// bookkeeping ranges will be `PageSize`-aligned.
//
// The algorithm is a relatively straightforward one. It is suitable for
// selecting the arenas we deal with today in a reasonable fashion, but it may
// well need to be evolved as we encounter more exotic configurations. It runs
// as follows:
//
// * We try to build up an arena starting at a given RAM range and an error
//   is encountered, then we report the error, throw away that range, and then
//   try to build an arena starting at the next range.
//
// * A prospective arena is built up, incorporating new RAM ranges, until either
//   the wastage limit is exceeded (an error) or suitable bookkeeping is found.
//
// * Once bookkeeping is found, we continue the process until we reach the last
//   range or find that incorporating the new RAM results in excess wastage or
//   insufficient bookkeeping. If we are unable to incorporate the new range, we
//   commit the arena as is and then restart the process at the new range.
//
// * During arena build-up, if we encounter a free RAM range that would admit
//   more bookkeeping capacity then we have currently, that becomes the arena's
//   new prospective bookkeeping range. The bookkeeping in a committed arena
//   will thus be put within the largest free RAM range encountered during this
//   process. More specifically, bookkeeping will be page-aligned and placed at
//   the end of this range.
//
// There are two reasons for choosing to place bookkeeping at the end of the
// largest free RAM range within an arena:
//
// (1) It is expedient to reserve 32-bit addressable RAM for certain device
//     drivers; the higher the bookkeeping, the more likely it is past the
//     32-bit addressable boundary.
//
// (2) Allocations are expected to be made in a first-fit fashion; the higher
//     the bookkeeping, the stabler the choice in the face of different early
//     allocation choices.
//
template <uint64_t PageSize, typename ArenaCallback, typename ErrorCallback>  //
constexpr void SelectPmmArenas(std::span<const memalloc::Range> ranges,       //
                               ArenaCallback&& arenas,                        //
                               ErrorCallback&& errors,                        //
                               uint64_t max_wasted_bytes = kMaxWastedArenaBytes);

// Within a given set of ranges, identifies each aligned allocation or hole in
// RAM, providing it to the callback. If part of an allocation or hole falls
// below a page boundary and was previously provided to the callback, it will
// not be again. The callback returns true iff it wished to proceed with the
// iteration.
//
// This routine is of interest to the PMM, as we wish to wire all such ranges
// that fall within an arena.
template <uint64_t PageSize, typename Callback>
void ForEachAlignedAllocationOrHole(std::span<const memalloc::Range> ranges, Callback&& cb) {
  std::optional<memalloc::Range> prev;
  for (memalloc::Range range : ranges) {
    if (range.type != memalloc::Type::kFreeRam) {
      range = range.AlignTo(PageSize);
    }

    if (prev) {
      // If `prev` is an allocation or hole, its aligning may have eaten into
      // `range`; trim accordingly.
      if (prev->type != memalloc::Type::kFreeRam) {
        if (prev->end() >= range.end()) {
          continue;
        }
        if (prev->end() > range.addr) {
          range.size -= prev->end() - range.addr;
          range.addr = prev->end();
        }
      }

      // Check for a hole between `prev` and `range`.
      if (prev->end() < range.addr) {
        memalloc::Range hole{
            .addr = prev->end(),
            .size = range.addr - prev->end(),
            .type = memalloc::Type::kReserved,
        };
        hole = hole.AlignTo(PageSize);
        if (!cb(hole)) {
          return;
        }

        // If `range` is free RAM, aligning the hole below it may have eaten
        // into it; trim accordingly.
        if (range.type == memalloc::Type::kFreeRam) {
          if (hole.end() >= range.end()) {
            continue;
          }
          if (hole.end() > range.addr) {
            range.size -= hole.end() - range.addr;
            range.addr = hole.end();
          }
        }
      }
    }

    if (range.type != memalloc::Type::kFreeRam) {
      // Non-RAM ranges (i.e., peripheral or unknown) should be treated as
      // holes.
      if (!memalloc::IsRamType(range.type)) {
        range.type = memalloc::Type::kReserved;
      }
      if (!cb(range)) {
        return;
      };
    }
    prev = range;
  }
}

namespace internal {

// PmmArenaSelector gives the implementation details of SelectPmmArenas(),
// declared above and defined below. While the the logic itself isn't actually
// object-oriented, the abstraction gives a convenient means of defining
// some shared, private helper routines.
template <uint64_t PageSize>
class PmmArenaSelector {
  static_assert(std::has_single_bit(PageSize));
  static_assert(std::has_single_bit(kArenaPageBookkeepingAlignment));

  // Page boundaries should be appropriate places to put bookkeeping.
  static_assert(kArenaPageBookkeepingAlignment <= PageSize);

 public:
  template <typename ArenaCallback, typename ErrorCallback>
  static constexpr void Select(std::span<const memalloc::Range> ranges, ArenaCallback&& arenas,
                               ErrorCallback&& errors, uint64_t max_wasted_bytes) {
    auto it = ranges.begin();
    while (it != ranges.end()) {
      // Peripheral and unknown ranges should be treated as holes, so no point
      // starting an arena there.
      if (!memalloc::IsRamType(it->type)) {
        ++it;
        continue;
      }

      fit::result result = SelectNext(it, ranges.end(), arenas, max_wasted_bytes);
      if (result.is_error()) {
        // We encountered an error in trying to start an arena: record the error
        // and try to start the arena at the next range.
        errors(PmmArenaSelectionError{.range = *it, .type = result.error_value()});
        ++it;
      } else {
        it = result.value();
      }
    }
  }

 private:
  using ErrorType = PmmArenaSelectionError::Type;
  using RangeIterator = std::span<const memalloc::Range>::iterator;

  static constexpr uint64_t RoundDown(uint64_t addr) { return addr & -PageSize; }
  static constexpr uint64_t RoundUp(uint64_t addr) { return (addr + PageSize - 1) & -PageSize; }

  // The required bookkeeping size in bytes for an arena of the given size.
  static constexpr uint64_t BookkeepingSize(uint64_t arena_size) {
    return RoundUp((arena_size / PageSize) * kArenaPageBookkeepingSize);
  }

  // Tries to selects the next arena starting at the range pointed to by
  // `first`. If successful, the selection is provided to the `arenas` callback
  // and the iterator pointing to the next range after the arena is returned.
  template <typename ArenaCallback>
  static constexpr fit::result<ErrorType, RangeIterator> SelectNext(RangeIterator first,
                                                                    RangeIterator end,
                                                                    ArenaCallback&& arenas,
                                                                    uint64_t max_wasted_bytes) {
    using Selected = PmmArenaSelection::Range;

    ZX_DEBUG_ASSERT(first != end);
    ZX_DEBUG_ASSERT(memalloc::IsRamType(first->type));

    auto commit_arena = [&arenas](Selected arena, Selected bookkeeping,
                                  uint64_t wasted_bytes) -> fit::result<ErrorType> {
      // No point of committing the arena if it's empty or would be dedicated
      // entirely to its own bookkeeping.
      arena.size = RoundDown(arena.size);
      if (arena.size <= PageSize) {
        return fit::error(ErrorType::kTooSmall);
      }

      uint64_t bookkeeping_size = BookkeepingSize(arena.size);
      uint64_t bookkeeping_end = RoundDown(bookkeeping.end());
      ZX_DEBUG_ASSERT(bookkeeping.base + bookkeeping_size <= bookkeeping_end);
      bookkeeping.base = bookkeeping_end - bookkeeping_size;
      bookkeeping.size = bookkeeping_size;

      arenas(PmmArenaSelection{
          .arena = arena,
          .bookkeeping = bookkeeping,
          .wasted_bytes = wasted_bytes,
      });
      return fit::ok();
    };

    std::optional<Selected> arena, bookkeeping;
    uint64_t wasted_bytes = 0;
    for (auto it = first; it != end; ++it) {
      const memalloc::Range& range = *it;

      // Explicitly reserved ranges should not be present.
      ZX_DEBUG_ASSERT(range.type != memalloc::Type::kReserved);

      // Peripheral and unknown ranges should be treated as holes.
      if (!memalloc::IsRamType(range.type)) {
        continue;
      }

      // Attempt to initialize the arena starting at the provided range, and
      // possibly bookkeeping within it).
      if (!arena) {
        ZX_DEBUG_ASSERT(!bookkeeping);
        uint64_t start = RoundUp(range.addr);
        arena = Selected{.base = start, .size = range.end() - start};

        if (range.type == memalloc::Type::kFreeRam && BookkeepingSize(arena->size) <= arena->size) {
          // Since `PageBookkeepingAlignment <= PageSize`, the start of the arena
          // should already be appropriately aligned for bookkeeping.
          bookkeeping = arena;
        }
        continue;
      }

      // If incorporating the hole into the arena would result in excess
      // wastage, commit the currently arena if possible.
      uint64_t pages_in_hole = (range.addr - arena->end() + PageSize - 1) / PageSize;
      uint64_t waste_from_hole = kArenaPageBookkeepingSize * pages_in_hole;
      if (wasted_bytes + waste_from_hole > max_wasted_bytes) {
        if (!bookkeeping) {
          return fit::error(ErrorType::kNoBookkeepingSpace);
        }
        fit::result result = commit_arena(*arena, *bookkeeping, wasted_bytes);
        if (result.is_error()) {
          return result.take_error();
        }
        return fit::ok(it);
      }

      // The prospective arena if we were to extend to the end of the new range.
      uint64_t new_size = range.end() - arena->base;

      // Prospective bookkeeping space large enough to encompass `range` in the
      // arena. Left as std::nullopt if no such space is found.
      std::optional<Selected> new_bookkeeping;
      {
        uint64_t new_bookkeeping_size = BookkeepingSize(new_size);

        // If the old bookkeeping range still works, tentatively use that...
        bool old_still_works = bookkeeping && bookkeeping->size >= new_bookkeeping_size;
        if (old_still_works) {
          new_bookkeeping = bookkeeping;
        }

        // ...but if the new range is free RAM range and would allow for more
        // bookkeeping capacity use that instead.
        if (range.type == memalloc::Type::kFreeRam) {
          // Begin bookkeeping at a page-aligned boundary, since we might need
          // to reserve the page just before it.
          uint64_t start = RoundUp(range.addr);
          if (start < range.end()) {
            uint64_t size = range.end() - start;

            bool new_works_better =
                size >= new_bookkeeping_size && (!old_still_works || size > bookkeeping->size);
            if (new_works_better) {
              new_bookkeeping = Selected{start, size};
            }
          }
        }
      }

      // If we have already found bookkeeping for our arena but incorporating
      // the new RAM range would either result in insufficient bookkeeping
      // space, commit the current arena.
      if (bookkeeping && !new_bookkeeping) {
        fit::result result = commit_arena(*arena, *bookkeeping, wasted_bytes);
        if (result.is_error()) {
          return result.take_error();
        }
        return fit::ok(it);
      }

      arena->size = new_size;
      bookkeeping = new_bookkeeping;
      wasted_bytes += waste_from_hole;
    }

    // Commit any remaining arena.
    if (arena) {
      if (!bookkeeping) {
        return fit::error(ErrorType::kNoBookkeepingSpace);
      }
      fit::result result = commit_arena(*arena, *bookkeeping, wasted_bytes);
      if (result.is_error()) {
        return result.take_error();
      }
    }

    return fit::ok(end);
  }
};

}  // namespace internal

template <uint64_t PageSize, typename ArenaCallback, typename ErrorCallback>  //
constexpr void SelectPmmArenas(std::span<const memalloc::Range> ranges,       //
                               ArenaCallback&& arenas,                        //
                               ErrorCallback&& errors,                        //
                               uint64_t max_wasted_bytes) {
  static_assert(std::is_invocable_v<ArenaCallback, const PmmArenaSelection&>);
  static_assert(std::is_invocable_v<ErrorCallback, const PmmArenaSelectionError&>);

  internal::PmmArenaSelector<PageSize>::Select(ranges, std::forward<ArenaCallback>(arenas),
                                               std::forward<ErrorCallback>(errors),
                                               max_wasted_bytes);
}

#endif  // ZIRCON_KERNEL_VM_PHYS_INCLUDE_VM_PHYS_ARENA_H_
