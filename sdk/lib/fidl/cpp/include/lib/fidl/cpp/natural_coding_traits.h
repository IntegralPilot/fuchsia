// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FIDL_CPP_NATURAL_CODING_TRAITS_H_
#define LIB_FIDL_CPP_NATURAL_CODING_TRAITS_H_

#include <lib/fidl/cpp/box.h>
#include <lib/fidl/cpp/features.h>
#include <lib/fidl/cpp/natural_decoder.h>
#include <lib/fidl/cpp/natural_encoder.h>
#include <lib/fidl/cpp/time.h>
#include <lib/fidl/cpp/wire/traits.h>
#include <lib/stdcompat/optional.h>

#if __FIDL_SUPPORT_HANDLES
#include <lib/zx/channel.h>
#endif  // __FIDL_SUPPORT_HANDLES

#include <algorithm>
#include <array>
#include <cstddef>
#include <limits>
#include <optional>
#include <string>
#include <type_traits>
#include <utility>
#include <vector>

namespace fidl::internal {

//
// Default construction helpers
//
// All natural domain objects are default constructible with sensible default
// states, with the exception of strict unions. There is no good default for a
// strict union. During decoding, we use this collection of traits and markers
// to help the FIDL runtime construct temporarily an invalid strict union (or
// aggregates thereof), and never give the object to the user if decoding errors
// prevent us from properly initializing it with a member.
//

// |DefaultConstructPossiblyInvalidObjectTag| selects a constructor that is only
// usable by the FIDL runtime, and may construct the object in an invalid state.
// This is useful in decoding where we must first construct the object and then
// populate it with valid contents.
struct DefaultConstructPossiblyInvalidObjectTag {};

// |DefaultConstructPossiblyInvalidObject| has a |Make| that makes an instance
// of |T| without any external inputs. For objects containing strict unions, the
// strict unions will be constructed in an invalid state.
//
// It is a way to expose the dangerous powers of invalid default construction
// only to the FIDL runtime, and forcing end users to start their objects with
// valid state.
template <typename T>
struct DefaultConstructPossiblyInvalidObject {
  static constexpr T Make() {
    if constexpr (std::is_default_constructible_v<T>) {
      return T{};
    } else {
      return T{DefaultConstructPossiblyInvalidObjectTag{}};
    }
  }
};

template <typename E, size_t N>
struct DefaultConstructPossiblyInvalidObject<std::array<E, N>> {
  static constexpr std::array<E, N> Make() {
    if constexpr (std::is_trivially_default_constructible_v<E>) {
      // Shortcut for arrays of primitives etc. This does not guarantee copy
      // elision, but eliminates zero initialization. Experimentation shows that
      // usually the compiler can eliminate the copy of `t`.
      std::array<E, N> t;
      return t;
    } else {
      if constexpr (std::is_default_constructible_v<E>) {
        // Shortcut for default constructible arrays.
        return std::array<E, N>{};
      } else {
        return ArrayMaker<N>::MakeArray(
            [] { return E{DefaultConstructPossiblyInvalidObject<E>::Make()}; });
      }
    }
  }

 private:
  template <std::size_t Idx = N>
  struct ArrayMaker {
    template <typename ElementMaker, typename... Ts>
    static std::array<E, N> MakeArray(ElementMaker maker, Ts... tail) {
      return ArrayMaker<Idx - 1>::MakeArray(maker, maker(), std::move(tail)...);
    }
  };

  template <>
  struct ArrayMaker<0> {
    template <typename ElementMaker, typename... Ts>
    static std::array<E, N> MakeArray(ElementMaker maker, Ts... tail) {
      return std::array<E, N>{std::move(tail)...};
    }
  };
};

//
// Constraints
//

struct NaturalCodingConstraintEmpty {};

template <zx_obj_type_t ObjType, zx_rights_t Rights, bool IsOptional>
struct NaturalCodingConstraintHandle {
  static constexpr zx_obj_type_t obj_type = ObjType;
  static constexpr zx_rights_t rights = Rights;
  static constexpr bool is_optional = IsOptional;
};

template <size_t Limit = std::numeric_limits<size_t>::max()>
struct NaturalCodingConstraintString {
  static constexpr size_t limit = Limit;
};

template <typename Inner_, size_t Limit = std::numeric_limits<size_t>::max()>
struct NaturalCodingConstraintVector {
  using Inner = Inner_;
  static constexpr size_t limit = Limit;
};

static constexpr size_t kRecursionDepthInitial = 0;
static constexpr size_t kRecursionDepthMax = FIDL_RECURSION_DEPTH;

template <typename T, typename Constraint, class Enable = void>
struct NaturalCodingTraits;

template <typename T, typename Constraint>
size_t NaturalEncodingInlineSize(NaturalEncoder* encoder) {
  ZX_DEBUG_ASSERT(encoder->wire_format() == ::fidl::internal::WireFormatVersion::kV2);
  return NaturalCodingTraits<T, Constraint>::kInlineSize;
}

template <typename T, typename Constraint>
size_t NaturalDecodingInlineSize(NaturalDecoder* decoder) {
  ZX_DEBUG_ASSERT(decoder->wire_format() == ::fidl::internal::WireFormatVersion::kV2);
  return NaturalCodingTraits<T, Constraint>::kInlineSize;
}

template <typename T, typename Constraint>
constexpr bool NaturalIsMemcpyCompatible() {
  return NaturalCodingTraits<T, Constraint>::kIsMemcpyCompatible;
}

template <typename T>
struct NaturalCodingTraits<T, NaturalCodingConstraintEmpty,
                           typename std::enable_if<IsPrimitive<T>::value>::type> {
  static constexpr size_t kInlineSize = sizeof(T);
  static constexpr bool kIsMemcpyCompatible = true;

  static void Encode(NaturalEncoder* encoder, const T* value, size_t offset,
                     size_t recursion_depth) {
    *encoder->template GetPtr<T>(offset) = *value;
  }
  static void Decode(NaturalDecoder* decoder, T* value, size_t offset, size_t recursion_depth) {
    *value = *decoder->template GetPtr<T>(offset);
  }
};

template <>
struct NaturalCodingTraits<bool, NaturalCodingConstraintEmpty> {
  static constexpr size_t kInlineSize = sizeof(bool);
  static constexpr bool kIsMemcpyCompatible = false;

  static void Encode(NaturalEncoder* encoder, const bool* value, size_t offset,
                     size_t recursion_depth) {
    *encoder->template GetPtr<bool>(offset) = *value;
  }
  static void Encode(NaturalEncoder* encoder, const std::vector<bool>::iterator& value,
                     size_t offset, size_t recursion_depth) {
    bool b = *value;
    Encode(encoder, &b, offset, recursion_depth);
  }
  static void Decode(NaturalDecoder* decoder, bool* value, size_t offset, size_t recursion_depth) {
    uint8_t uintval = *decoder->template GetPtr<uint8_t>(offset);
    if (!(uintval == 0 || uintval == 1)) {
      decoder->SetError(kCodingErrorInvalidBoolean);
      return;
    }

    *value = *decoder->template GetPtr<bool>(offset);
  }
  static void Decode(NaturalDecoder* decoder, const std::vector<bool>::iterator& value,
                     size_t offset, size_t recursion_depth) {
    bool b;
    Decode(decoder, &b, offset, recursion_depth);
    *value = b;
  }
};

template <bool Value>
class NaturalUseStdCopy {};

template <typename T, typename Constraint>
void NaturalEncodeVectorBody(NaturalUseStdCopy<true>, NaturalEncoder* encoder,
                             typename std::vector<T>::iterator in_begin,
                             typename std::vector<T>::iterator in_end, size_t out_offset,
                             size_t recursion_depth) {
  static_assert(NaturalCodingTraits<T, Constraint>::kInlineSize == sizeof(T),
                "stride doesn't match object size");
  std::copy(in_begin, in_end, encoder->template GetPtr<T>(out_offset));
}

template <typename T, typename Constraint>
void NaturalEncodeVectorBody(NaturalUseStdCopy<false>, NaturalEncoder* encoder,
                             typename std::vector<T>::iterator in_begin,
                             typename std::vector<T>::iterator in_end, size_t out_offset,
                             size_t recursion_depth) {
  size_t stride = NaturalEncodingInlineSize<T, Constraint>(encoder);
  for (typename std::vector<T>::iterator in_it = in_begin; in_it != in_end;
       in_it++, out_offset += stride) {
    if constexpr (std::is_same_v<T, bool>) {
      NaturalCodingTraits<bool, Constraint>::Encode(encoder, in_it, out_offset, recursion_depth);
    } else {
      NaturalCodingTraits<T, Constraint>::Encode(encoder, &*in_it, out_offset, recursion_depth);
    }
  }
}

template <typename T, typename Constraint>
void NaturalDecodeVectorBody(NaturalUseStdCopy<true>, NaturalDecoder* decoder,
                             size_t in_begin_offset, size_t in_end_offset, std::vector<T>* out,
                             size_t count, size_t recursion_depth) {
  static_assert(NaturalCodingTraits<T, Constraint>::kInlineSize == sizeof(T),
                "stride doesn't match object size");
  *out = std::vector<T>(decoder->template GetPtr<T>(in_begin_offset),
                        decoder->template GetPtr<T>(in_end_offset));
}

template <typename T, typename Constraint>
void NaturalDecodeVectorBody(NaturalUseStdCopy<false>, NaturalDecoder* decoder,
                             size_t in_begin_offset, size_t in_end_offset, std::vector<T>* out,
                             size_t count, size_t recursion_depth) {
  out->reserve(count);
  size_t stride = NaturalDecodingInlineSize<T, Constraint>(decoder);
  size_t in_offset = in_begin_offset;
  size_t index = 0;
  for (; in_offset < in_end_offset; in_offset += stride, index++) {
    // Avoid materializing a |T| if it's already default constructible.
    // Large aggregates (e.g. arrays of primitives) can be constructed with little stack usage,
    // in debug builds or ASAN builds.
    // Note that in practice the two code paths should be equivalent under optimization.
    if constexpr (std::is_default_constructible_v<T>) {
      out->emplace_back();
    } else {
      out->emplace_back(DefaultConstructPossiblyInvalidObject<T>::Make());
    }
    if constexpr (std::is_same_v<T, bool>) {
      bool b;
      NaturalCodingTraits<bool, Constraint>::Decode(decoder, &b, in_offset, recursion_depth);
      (*out)[index] = b;
    } else {
      NaturalCodingTraits<T, Constraint>::Decode(decoder, &(*out)[index], in_offset,
                                                 recursion_depth);
    }
  }
}

template <typename T, typename Constraint>
struct NaturalCodingTraits<::std::vector<T>, Constraint> {
  using InnerConstraint = typename Constraint::Inner;
  static constexpr size_t kInlineSize = sizeof(fidl_vector_t);
  static constexpr bool kIsMemcpyCompatible = false;

  static void Encode(NaturalEncoder* encoder, ::std::vector<T>* value, size_t offset,
                     size_t recursion_depth) {
    size_t count = value->size();
    if (count > Constraint::limit) {
      encoder->SetError(kCodingErrorVectorLimitExceeded);
      // Proceed to visit vector elements and collect handles to close.
    }
    if (recursion_depth + 1 > kRecursionDepthMax) {
      encoder->SetError(kCodingErrorRecursionDepthExceeded);
      return;
    }

    fidl_vector_t* vector = encoder->template GetPtr<fidl_vector_t>(offset);
    vector->count = count;
    vector->data = reinterpret_cast<void*>(FIDL_ALLOC_PRESENT);
    size_t stride = NaturalEncodingInlineSize<T, InnerConstraint>(encoder);
    size_t base = encoder->Alloc(count * stride);
    internal::NaturalEncodeVectorBody<T, InnerConstraint>(
        internal::NaturalUseStdCopy<NaturalIsMemcpyCompatible<T, InnerConstraint>()>(), encoder,
        value->begin(), value->end(), base, recursion_depth + 1);
  }
  static void Decode(NaturalDecoder* decoder, ::std::vector<T>* value, size_t offset,
                     size_t recursion_depth) {
    fidl_vector_t* encoded = decoder->template GetPtr<fidl_vector_t>(offset);
    size_t count = encoded->count;
    if (count > Constraint::limit) {
      decoder->SetError(kCodingErrorVectorLimitExceeded);
      return;
    }
    switch (reinterpret_cast<uintptr_t>(encoded->data)) {
      case FIDL_ALLOC_PRESENT:
        break;
      case FIDL_ALLOC_ABSENT: {
        decoder->SetError(kCodingErrorNullDataReceivedForNonNullableVector);
        return;
      }
      default: {
        decoder->SetError(kCodingErrorInvalidPresenceIndicator);
        return;
      }
    }
    if (recursion_depth + 1 > kRecursionDepthMax) {
      decoder->SetError(kCodingErrorRecursionDepthExceeded);
      return;
    }

    size_t stride = NaturalDecodingInlineSize<T, InnerConstraint>(decoder);
    size_t base;
    size_t allocation_size;
    if (__builtin_mul_overflow(count, stride, &allocation_size)) {
      decoder->SetError(kCodingErrorAllocationSizeExceeds32Bits);
      return;
    }
    if (!decoder->Alloc(allocation_size, &base)) {
      return;
    }
    internal::NaturalDecodeVectorBody<T, InnerConstraint>(
        internal::NaturalUseStdCopy<NaturalIsMemcpyCompatible<T, InnerConstraint>()>(), decoder,
        base, base + stride * count, value, count, recursion_depth + 1);
  }
};

template <typename T, size_t N, typename Constraint>
struct NaturalCodingTraits<::std::array<T, N>, Constraint> {
  static constexpr size_t kInlineSize = NaturalCodingTraits<T, Constraint>::kInlineSize * N;
  static constexpr bool kIsMemcpyCompatible =
      NaturalCodingTraits<T, Constraint>::kIsMemcpyCompatible;

  static void Encode(NaturalEncoder* encoder, std::array<T, N>* value, size_t offset,
                     size_t recursion_depth) {
    size_t stride = NaturalEncodingInlineSize<T, Constraint>(encoder);
    if constexpr (kIsMemcpyCompatible) {
      memcpy(encoder->template GetPtr<void>(offset), value->data(), N * stride);
    } else {
      for (size_t i = 0; i < N; ++i) {
        NaturalCodingTraits<T, Constraint>::Encode(encoder, &value->at(i), offset + i * stride,
                                                   recursion_depth);
      }
    }
  }
  static void Decode(NaturalDecoder* decoder, std::array<T, N>* value, size_t offset,
                     size_t recursion_depth) {
    size_t stride = NaturalDecodingInlineSize<T, Constraint>(decoder);
    if constexpr (kIsMemcpyCompatible) {
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wnontrivial-memaccess"
      memcpy(value->data(), decoder->template GetPtr<void>(offset), N * stride);
#pragma GCC diagnostic pop
    } else {
      for (size_t i = 0; i < N; ++i) {
        NaturalCodingTraits<T, Constraint>::Decode(decoder, &value->at(i), offset + i * stride,
                                                   recursion_depth);
      }
    }
  }
};

#if __FIDL_SUPPORT_HANDLES
template <typename T, typename Constraint>
struct NaturalCodingTraits<
    T, Constraint, typename std::enable_if<std::is_base_of<zx::object_base, T>::value>::type> {
  static constexpr size_t kInlineSize = sizeof(zx_handle_t);
  static constexpr bool kIsMemcpyCompatible = false;

  static void Encode(NaturalEncoder* encoder, zx::object_base* value, size_t offset,
                     size_t recursion_depth) {
    encoder->EncodeHandle(value->release(),
                          {
                              .obj_type = Constraint::obj_type,
                              .rights = Constraint::rights,
                          },
                          offset, Constraint::is_optional);
  }
  static void Decode(NaturalDecoder* decoder, zx::object_base* value, size_t offset,
                     size_t recursion_depth) {
    zx_handle_t handle = ZX_HANDLE_INVALID;
    decoder->DecodeHandle(&handle,
                          {
                              .obj_type = Constraint::obj_type,
                              .rights = Constraint::rights,
                          },
                          offset, Constraint::is_optional);
    value->reset(handle);
  }
};

#endif  // __FIDL_SUPPORT_HANDLES

template <zx_clock_t ClockId>
struct NaturalCodingTraits<fidl::basic_time<ClockId>, NaturalCodingConstraintEmpty> {
  static constexpr size_t kInlineSize = sizeof(zx_time_t);
  static constexpr bool kIsMemcpyCompatible = true;

  static void Encode(NaturalEncoder* encoder, const fidl::basic_time<ClockId>* value, size_t offset,
                     size_t recursion_depth) {
    *encoder->template GetPtr<zx_time_t>(offset) = value->get();
  }
  static void Decode(NaturalDecoder* decoder, fidl::basic_time<ClockId>* value, size_t offset,
                     size_t recursion_depth) {
    *value = fidl::basic_time<ClockId>(*decoder->template GetPtr<zx_time_t>(offset));
  }
};

template <zx_clock_t ClockId>
struct NaturalCodingTraits<fidl::basic_ticks<ClockId>, NaturalCodingConstraintEmpty> {
  static constexpr size_t kInlineSize = sizeof(zx_ticks_t);
  static constexpr bool kIsMemcpyCompatible = true;

  static void Encode(NaturalEncoder* encoder, const fidl::basic_ticks<ClockId>* value,
                     size_t offset, size_t recursion_depth) {
    *encoder->template GetPtr<zx_ticks_t>(offset) = value->get();
  }
  static void Decode(NaturalDecoder* decoder, fidl::basic_ticks<ClockId>* value, size_t offset,
                     size_t recursion_depth) {
    *value = fidl::basic_ticks<ClockId>(*decoder->template GetPtr<zx_ticks_t>(offset));
  }
};

template <typename T, typename Constraint>
struct NaturalCodingTraits<std::optional<std::vector<T>>, Constraint> {
  static constexpr size_t kInlineSize = sizeof(fidl_vector_t);
  static constexpr bool kIsMemcpyCompatible = false;

  static void Encode(NaturalEncoder* encoder, std::optional<std::vector<T>>* value, size_t offset,
                     size_t recursion_depth) {
    if (value->has_value()) {
      fidl::internal::NaturalCodingTraits<std::vector<T>, Constraint>::Encode(
          encoder, &value->value(), offset, recursion_depth);
      return;
    }
    fidl_vector_t* vec = encoder->template GetPtr<fidl_vector_t>(offset);
    vec->count = 0;
    vec->data = reinterpret_cast<char*>(FIDL_ALLOC_ABSENT);
  }
  static void Decode(NaturalDecoder* decoder, std::optional<std::vector<T>>* value, size_t offset,
                     size_t recursion_depth) {
    fidl_vector_t* vec = decoder->template GetPtr<fidl_vector_t>(offset);
    switch (reinterpret_cast<uintptr_t>(vec->data)) {
      case FIDL_ALLOC_PRESENT: {
        std::vector<T> unwrapped;
        fidl::internal::NaturalCodingTraits<std::vector<T>, Constraint>::Decode(
            decoder, &unwrapped, offset, recursion_depth);
        value->emplace(std::move(unwrapped));
        return;
      }
      case FIDL_ALLOC_ABSENT: {
        if (vec->count != 0) {
          decoder->SetError(kCodingErrorNullVectorMustHaveSizeZero);
          return;
        }
        value->reset();
        return;
      }
      default: {
        decoder->SetError(kCodingErrorInvalidPresenceIndicator);
        return;
      }
    }
  }
};

template <typename T, typename Constraint>
struct NaturalCodingTraits<fidl::Box<T>, Constraint,
                           typename std::enable_if<!IsUnion<T>::value>::type> {
  static constexpr size_t kInlineSize = sizeof(uintptr_t);
  static constexpr bool kIsMemcpyCompatible = false;

  static void Encode(NaturalEncoder* encoder, fidl::Box<T>* value, size_t offset,
                     size_t recursion_depth) {
    if (value->unique_ptr().get()) {
      if (recursion_depth + 1 > kRecursionDepthMax) {
        encoder->SetError(kCodingErrorRecursionDepthExceeded);
        return;
      }

      *encoder->template GetPtr<uintptr_t>(offset) = FIDL_ALLOC_PRESENT;

      size_t alloc_size = NaturalEncodingInlineSize<T, Constraint>(encoder);
      NaturalCodingTraits<T, Constraint>::Encode(encoder, value->unique_ptr().get(),
                                                 encoder->Alloc(alloc_size), recursion_depth + 1);
    } else {
      *encoder->template GetPtr<uintptr_t>(offset) = FIDL_ALLOC_ABSENT;
    }
  }
  static void Decode(NaturalDecoder* decoder, fidl::Box<T>* value, size_t offset,
                     size_t recursion_depth) {
    uintptr_t ptr = *decoder->template GetPtr<uintptr_t>(offset);
    if (!ptr) {
      return value->reset();
    }

    if (recursion_depth + 1 > kRecursionDepthMax) {
      decoder->SetError(kCodingErrorRecursionDepthExceeded);
      return;
    }
    *value = std::make_unique<T>(DefaultConstructPossiblyInvalidObjectTag{});
    size_t alloc_size = NaturalDecodingInlineSize<T, Constraint>(decoder);
    size_t body_offset;
    if (!decoder->Alloc(alloc_size, &body_offset)) {
      return;
    }
    NaturalCodingTraits<T, Constraint>::Decode(decoder, value->unique_ptr().get(), body_offset,
                                               recursion_depth + 1);
  }
};

template <typename T, typename Constraint>
struct NaturalCodingTraits<fidl::Box<T>, Constraint,
                           typename std::enable_if<IsUnion<T>::value>::type> {
  static constexpr size_t kInlineSize = sizeof(fidl_union_t);
  static constexpr bool kIsMemcpyCompatible = false;

  static void Encode(NaturalEncoder* encoder, fidl::Box<T>* value, size_t offset,
                     size_t recursion_depth) {
    if (*value) {
      NaturalCodingTraits<T, Constraint>::Encode(encoder, value->unique_ptr().get(), offset,
                                                 recursion_depth);
      return;
    }

    // Buffer is zero-initialized.
  }

  static void Decode(NaturalDecoder* decoder, fidl::Box<T>* value, size_t offset,
                     size_t recursion_depth) {
    fidl_union_t* u = decoder->template GetPtr<fidl_union_t>(offset);
    if (u->tag == 0) {
      if (likely(FidlIsZeroEnvelope(&u->envelope))) {
        *value = nullptr;
        return;
      }
      decoder->SetError(kCodingErrorZeroTagButNonZeroEnvelope);
    }
    *value = std::make_unique<T>(DefaultConstructPossiblyInvalidObject<T>::Make());
    NaturalCodingTraits<T, Constraint>::Decode(decoder, value->unique_ptr().get(), offset,
                                               recursion_depth);
  }
};

template <typename Constraint>
struct NaturalCodingTraits<::std::string, Constraint> final {
  static constexpr size_t kInlineSize = sizeof(fidl_string_t);
  static constexpr bool kIsMemcpyCompatible = false;

  static void Encode(NaturalEncoder* encoder, std::string* value, size_t offset,
                     size_t recursion_depth) {
    const size_t size = value->size();
    if (value->size() > Constraint::limit) {
      encoder->SetError(kCodingErrorStringLimitExceeded);
      return;
    }
    bool valid = utfutils_is_valid_utf8(value->data(), value->size());
    if (!valid) {
      encoder->SetError(kCodingErrorStringNotValidUtf8);
      return;
    }
    if (recursion_depth + 1 > kRecursionDepthMax) {
      encoder->SetError(kCodingErrorRecursionDepthExceeded);
      return;
    }

    fidl_string_t* string = encoder->template GetPtr<fidl_string_t>(offset);
    string->size = size;
    string->data = reinterpret_cast<char*>(FIDL_ALLOC_PRESENT);
    size_t base = encoder->Alloc(size);
    char* payload = encoder->template GetPtr<char>(base);
    memcpy(payload, value->data(), size);
  }
  static void Decode(NaturalDecoder* decoder, std::string* value, size_t offset,
                     size_t recursion_depth) {
    if (recursion_depth + 1 > kRecursionDepthMax) {
      decoder->SetError(kCodingErrorRecursionDepthExceeded);
      return;
    }

    fidl_string_t* string = decoder->template GetPtr<fidl_string_t>(offset);
    if (string->size > Constraint::limit) {
      decoder->SetError(kCodingErrorStringLimitExceeded);
      return;
    }
    switch (reinterpret_cast<uintptr_t>(string->data)) {
      case FIDL_ALLOC_PRESENT:
        break;
      case FIDL_ALLOC_ABSENT: {
        decoder->SetError(kCodingErrorNullDataReceivedForNonNullableString);
        return;
      }
      default: {
        decoder->SetError(kCodingErrorInvalidPresenceIndicator);
        return;
      }
    }
    size_t base;
    if (!decoder->Alloc(string->size, &base)) {
      return;
    }
    char* payload = decoder->template GetPtr<char>(base);
    bool valid = utfutils_is_valid_utf8(payload, string->size);
    if (!valid) {
      decoder->SetError(kCodingErrorStringNotValidUtf8);
      return;
    }
    *value = std::string(payload, string->size);
  }
};

template <typename Constraint>
struct NaturalCodingTraits<std::optional<std::string>, Constraint> {
  static constexpr size_t kInlineSize = sizeof(fidl_string_t);
  static constexpr bool kIsMemcpyCompatible = false;

  static void Encode(NaturalEncoder* encoder, std::optional<std::string>* value, size_t offset,
                     size_t recursion_depth) {
    if (value->has_value()) {
      fidl::internal::NaturalCodingTraits<std::string, Constraint>::Encode(encoder, &value->value(),
                                                                           offset, recursion_depth);
      return;
    }
    fidl_string_t* string = encoder->template GetPtr<fidl_string_t>(offset);
    string->size = 0;
    string->data = reinterpret_cast<char*>(FIDL_ALLOC_ABSENT);
  }
  static void Decode(NaturalDecoder* decoder, std::optional<std::string>* value, size_t offset,
                     size_t recursion_depth) {
    fidl_string_t* string = decoder->template GetPtr<fidl_string_t>(offset);
    switch (reinterpret_cast<uintptr_t>(string->data)) {
      case FIDL_ALLOC_PRESENT: {
        std::string unwrapped;
        fidl::internal::NaturalCodingTraits<std::string, Constraint>::Decode(
            decoder, &unwrapped, offset, recursion_depth);
        value->emplace(unwrapped);
        return;
      }
      case FIDL_ALLOC_ABSENT: {
        if (string->size != 0) {
          decoder->SetError(kCodingErrorNullStringMustHaveSizeZero);
          return;
        }
        value->reset();
        return;
      }
      default: {
        decoder->SetError(kCodingErrorInvalidPresenceIndicator);
        return;
      }
    }
  }
};

#if __FIDL_SUPPORT_HANDLES
template <typename T, typename Constraint>
struct NaturalCodingTraits<ClientEnd<T>, Constraint> {
  static constexpr size_t kInlineSize = sizeof(zx_handle_t);
  static constexpr bool kIsMemcpyCompatible = false;

  static void Encode(NaturalEncoder* encoder, ClientEnd<T>* value, size_t offset,
                     size_t recursion_depth) {
    encoder->EncodeHandle(value->TakeChannel().release(),
                          {
                              .obj_type = Constraint::obj_type,
                              .rights = Constraint::rights,
                          },
                          offset, Constraint::is_optional);
  }

  static void Decode(NaturalDecoder* decoder, ClientEnd<T>* value, size_t offset,
                     size_t recursion_depth) {
    zx_handle_t handle = ZX_HANDLE_INVALID;
    decoder->DecodeHandle(&handle,
                          {
                              .obj_type = Constraint::obj_type,
                              .rights = Constraint::rights,
                          },
                          offset, Constraint::is_optional);
    *value = ClientEnd<T>(zx::channel(handle));
  }
};

template <typename T, typename Constraint>
struct NaturalCodingTraits<ServerEnd<T>, Constraint> {
  static constexpr size_t kInlineSize = sizeof(zx_handle_t);
  static constexpr bool kIsMemcpyCompatible = false;

  static void Encode(NaturalEncoder* encoder, ServerEnd<T>* value, size_t offset,
                     size_t recursion_depth) {
    encoder->EncodeHandle(value->TakeChannel().release(),
                          {
                              .obj_type = Constraint::obj_type,
                              .rights = Constraint::rights,
                          },
                          offset, Constraint::is_optional);
  }

  static void Decode(NaturalDecoder* decoder, ServerEnd<T>* value, size_t offset,
                     size_t recursion_depth) {
    zx_handle_t handle = ZX_HANDLE_INVALID;
    decoder->DecodeHandle(&handle,
                          {
                              .obj_type = Constraint::obj_type,
                              .rights = Constraint::rights,
                          },
                          offset, Constraint::is_optional);
    *value = ServerEnd<T>(zx::channel(handle));
  }
};
#endif  // __FIDL_SUPPORT_HANDLES

template <typename Constraint, typename T>
void NaturalEncode(NaturalEncoder* encoder, T* value, size_t offset, size_t recursion_depth) {
  NaturalCodingTraits<T, Constraint>::Encode(encoder, value, offset, recursion_depth);
}

template <typename Constraint, typename T>
void NaturalDecode(NaturalDecoder* decoder, T* value, size_t offset, size_t recursion_depth) {
  NaturalCodingTraits<T, Constraint>::Decode(decoder, value, offset, recursion_depth);
}

using NaturalTopLevelDecodeFn = void (*)(NaturalDecoder*, void* value, size_t offset);

template <typename FidlType>
constexpr NaturalTopLevelDecodeFn MakeNaturalTopLevelDecodeFn() {
  return [](NaturalDecoder* decoder, void* value, size_t offset) {
    ::fidl::internal::NaturalCodingTraits<FidlType, NaturalCodingConstraintEmpty>::Decode(
        decoder, reinterpret_cast<FidlType*>(value), offset, kRecursionDepthInitial);
  };
}

// Create a |fidl::NaturalDecoder| and decode the inputted |message|.
// In case of error, handles in |message| are consumed.
// In case of success, handle values will be embedded in the natural type |value|; the caller
// must ensure that |value| points to an instance of default constructed natural type that
// matches the one decoded by |decode_fn|.
// This is the top-level function to call to perform decoding using coding traits.
fidl::Status NaturalDecode(::fidl::WireFormatMetadata metadata, size_t inline_size,
                           NaturalTopLevelDecodeFn decode_fn, ::fidl::EncodedMessage& message,
                           void* value);

}  // namespace fidl::internal

#endif  // LIB_FIDL_CPP_NATURAL_CODING_TRAITS_H_
