// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_REMOTE_ABI_TRANSCRIBER_H_
#define LIB_LD_REMOTE_ABI_TRANSCRIBER_H_

#include <lib/elfldltl/abi-ptr.h>
#include <lib/elfldltl/abi-span.h>
#include <lib/elfldltl/layout.h>

#include <array>
#include <concepts>
#include <type_traits>
#include <utility>

namespace ld {

// RemoteAbiTranscriber<T> handles the destructuring aspect of ABI remoting for
// a type T.  The idea is that T is some remotable SomeType<SomeRemoteTraits>
// that can be copied from a corresponding SomeType<elfldltl::LocalAbiTraits>.
// Basically, it's a limited substitute for using some sort of generic language
// introspection features that C++ doesn't have.  It's just doing a deep copy
// where everything is copied verbatim except that elfldltl::AbiPtr<...> types
// get translated from the local address space to a remote address space.  The
// SomeRemoteTraits type is responsible for how that pointer translation works,
// but RemoteAbiTranscriber takes care of finding all the pointer members in T
// and its subobjects.

// This is the main point of specialization, which is the responsibility of the
// definer of T.  It gets explicit (often partial) specializations for the most
// interesting types.  Otherwise the default implementation handles three kinds
// of "uninteresting" types:
//
// * Single-byte integral types that are copied verbatim (e.g. bool, uint8_t).
//
// * Single-byte enum types that are copied verbatim.
//
// * All <lib/elfldltl/layout.h> and <lib/elfldltl/field.h> types (regardless
//   of ElfClass and ElfData, as per elfldltl::kIsLayout) are copied verbatim.
//
// * Class / struct types are copied member-by-member.  To make this work
//   they must provide the API of Abi* template aliases described below.
//
// Explicit partial specializations below cover the <lib/elfldltl/field.h>
// types, which are copied verbatim; and std::array, which is copied piecewise
// (separately transcribing each element, however that type is specialized).
//
// The partial specialization of elfldltl::AbiPtr is the only generic type
// that's considered "interesting".  elfldltl::AbiPtr or its derivatives (such
// as AbiSpan) must be used for all pointers in transcribable data structures.
// Those must take an AbiTraits template class parameter and propagate it down
// to all the ABI-sensitive types, especially AbiPtr.
template <typename T>
struct RemoteAbiTranscriber;

// Implementation declarations used below.
enum class RemoteAbiTranscriberImplType {
  kIntegral,     // Integer and enum types.
  kVerbatim,     // Types copied verbatim.
  kArray,        // std::array types copied element by element.
  kClass,        // Class and struct types copied member by member.
  kUnsupported,  // Type cannot be transcribed.
};

template <typename T, RemoteAbiTranscriberImplType>
struct RemoteAbiTranscriberImpl;

// Choose among the main implementation types.  Any type T not correctly
// categorized by this should have a RemoteAbiTranscriber<T> specialization.
template <typename T>
inline constexpr RemoteAbiTranscriberImplType kRemoteAbiTranscriberImplType =
    (std::is_integral_v<T> || std::is_enum_v<T>)  // Any integer-type type.
        ? RemoteAbiTranscriberImplType::kIntegral
        // elfldltl::Elf<...> types are already universal.
        : elfldltl::kIsLayout<T> ? RemoteAbiTranscriberImplType::kVerbatim
          // Other types without specializations transcribe member-by-member.
          : std::is_class_v<T> ? RemoteAbiTranscriberImplType::kClass
                               : RemoteAbiTranscriberImplType::kUnsupported;

template <typename T>
struct RemoteAbiTranscriber
    :  // This dispatches to the RemoteAbiTranscriberImpl partial
       // specializations below when there is no explicit specialization for T.
       public RemoteAbiTranscriberImpl<T, kRemoteAbiTranscriberImplType<T>> {};

// This is the implementation base class for types that are copied verbatim.
// It also serves as the API exemplar for what specializations must provide.
template <typename T>
struct RemoteAbiTranscriberImpl<T, RemoteAbiTranscriberImplType::kVerbatim> {
  static_assert(std::has_unique_object_representations_v<T>,
                "padding bytes not allowed in transcribable types");

  using Local = T;

  // The Context parameter can be forwarded (if only used once), or passed by
  // reference, down whatever layers there may be to reach an AbiPtr or other
  // interesting type whose FromLocal needs to consult that data structure.
  // What type Context should be is ultimately up to the specific AbiTraits
  // implementation class (derived from elfldltl::RemoteAbiTraits).
  //
  // When the member-by-member transcriber for class / struct types is being
  // used, the Context object must provide an additional template method.
  // Comments below about the Context::MemberFromLocal method explain further.
  template <typename Context>
  static constexpr bool FromLocal(Context&& ctx, T& out, const Local& in) {
    out = in;
    return true;
  }
};

// Integral and enum types can be copied verbatim, but only if single-byte.
// For multi-byte integer types, <lib/elfldltl/field.h> types should be used in
// case byte-swapping is required.
template <typename T>
struct RemoteAbiTranscriberImpl<T, RemoteAbiTranscriberImplType::kIntegral>
    : public RemoteAbiTranscriberImpl<T, RemoteAbiTranscriberImplType::kVerbatim> {
  static_assert(sizeof(T) == 1,
                "multi-byte integers and enums must use <lib/elfldltl/field.h> types");
};

// This partial specialization handles class / struct types by applying
// separate RemoteAbiTranscriber instantiations to each of their bases and
// members.  For this to work, each class type without its own specialization
// must instead provide some helpers in the form of inner type and template
// aliases.  No explicit specialization is required for a class or struct type
// if it meets the following API:
//
//  * `using AbiLocal = ...;` gives the LocalAbiTraits equivalent
//    instantiation of the specialized type: the type FromLocal will accept
//    as input.
//
//  * ```
//    template <template <class...> class Template>
//    using AbiBases = Template<MyBase, ...>;
//    ```
//    This alias instantiates Template with each direct base class of the
//    specialized class, if any.  The transcription will just instantiate
//    RemoteAbiTranscriber on each base class in turn and use its FromLocal on
//    the upcasted reference for that portion of the object.  **NOTE:** This
//    list must be a 1:1 parallel with `AbiLocal::AbiBases<Template>`!
//
//  * ```
//    template <template <auto...> class Template>
//    using AbiMembers = Template<&MyClass::member_, ...>;
//    ```
//    This alias instantiates Template with the pointer-to-member for each
//    member of the specialized class, if it has any.  The transcription will
//    just instantiate RemoteAbiTranscriber on each member's type in turn and
//    use its FromLocal.  **NOTE:** This list must be a 1:1 parallel with
//    `AbiLocal::AbiMembers<Template>`!
//
// This ultimately just calls `RemoteAbiTranscriber<MemberType>::FromLocal` on
// each `out.Member` and the corresponding `in.Member`.  The Context object
// passed to FromLocal on this transcriber for the class object must provide
// this additional template method:
//
//  ```
//  template <auto Member, typename MemberType, auto LocalMember, typename Local>
//  bool MemberFromLocal(MemberType& out, const Local& in) {
//    return ld::RemoteAbiTranscriber<MemberType>::FromLocal(
//        *this, out, in.*LocalMember);
//  }
//  ```
//
//  This is always called with four explicit template arguments, though the
//  argument values are always a `MemberType&` and a `const Local&`, and could
//  have been deduced.  The first template argument is some pointer-to-member
//  `MemberType Remote::*`, that is, a pointer-to-member for the remote version
//  of the class type.  The `out` argument is a reference to the specific
//  member in the remote version of the class object being filled in.  However,
//  the `in` argument is instead a reference to the whole class object whose
//  corresponding member is to be transcribed.  The generic implementation
//  shown above is the default logic: Just call the member type's FromLocal
//  with the same context object.  Explicit specializations for specific
//  members can use bespoke logic instead.
//
//  Context::MemberFromLocal must provide a const overload if the Context
//  object is passed to FromLocal as `const Context&`.  If there is only a
//  non-const overload of MemberFromLocal, then a mutable (or rvalue) `Context`
//  object must be passed to FromLocal, since that reference (or value) is what
//  MemberFromLocal will be called on.
//
template <typename T>
struct RemoteAbiTranscriberImpl<T, RemoteAbiTranscriberImplType::kClass> {
  static_assert(std::is_class_v<T>);
  static_assert(!std::is_const_v<T>,
                "const qualifiers should have been removed before instantiation");
  static_assert(!std::is_volatile_v<T>,
                "volatile qualifiers should have been removed before instantiation");
  static_assert(std::is_default_constructible_v<T>,
                "transcribable types must be default constructible");
  static_assert(std::is_trivially_destructible_v<T>,
                "transcribable types must be trivially destructible");
  static_assert(std::has_unique_object_representations_v<T>,
                "padding bytes not allowed in transcribable types");

  using Local = T::AbiLocal;
  static_assert(!std::is_same_v<Local, T>,
                "should specialize to kVerbatim for types with no AbiTraits"
                " unless layout types, which kIsLayout should match");
  static_assert(std::is_trivially_destructible_v<Local>);

  template <class... B>
  struct BasesImpl {
    static_assert((!std::is_same_v<T, B> && ...));
    static_assert((std::is_base_of_v<B, T> && ...));

    // Inside this extra template layer, both B... and LocalB... parameter
    // packs are available to be expanded in parallel.
    template <class... LocalB>
    struct LocalBasesImpl {
      static_assert(sizeof...(LocalB) == sizeof...(B),
                    "AbiLocal::AbiBases mismatch: AbiLocal alias not to same template?");

      template <typename Context>
      static constexpr bool FromLocal(Context&& ctx, T& out, const Local& in) {
        return (RemoteAbiTranscriber<B>::FromLocal(ctx, static_cast<B&>(out),
                                                   static_cast<const LocalB&>(in)) &&
                ...);
      }
    };

    template <typename Context>
    static constexpr bool FromLocal(Context& ctx, T& out, const Local& in) {
      using Impl = Local::template AbiBases<LocalBasesImpl>;
      return Impl::FromLocal(ctx, out, in);
    }
  };
  using Bases = T::template AbiBases<BasesImpl>;

  template <auto Member, auto LocalMember, typename Context>
  static constexpr bool MemberFromLocal(Context& ctx, T& out, const Local& in) {
    using MemberType = std::decay_t<decltype(out.*Member)>;
    using LocalMemberType = std::decay_t<decltype(in.*LocalMember)>;
    using MemberTranscriber = RemoteAbiTranscriber<MemberType>;
    static_assert(std::is_same_v<typename MemberTranscriber::Local, LocalMemberType>);
    // Delegate to the context to transcribe the member.  Usually it just
    // delegates back to RemoteAbiTranscriber<MemberType> on in.*LocalMember.
    return ctx.template MemberFromLocal<Member, MemberType, LocalMember, Local>(out.*Member, in);
  }

  static constexpr bool Same(auto a, decltype(a) b) { return a == b; }
  template <typename A, typename B>
    requires(!std::same_as<A, B>)
  static constexpr bool Same(A a, B b) {
    return false;
  }

  template <auto... M>
  struct MembersImpl {
    // Ensure that all the template arguments are pointers to members of T.
    static_assert((std::is_member_object_pointer_v<decltype(M)> && ...));
    static_assert((!std::is_void_v<decltype(std::declval<T>().*M)> && ...));

    static constexpr size_t kSizeBytes = (sizeof(std::declval<T>().*M) + ...);

    template <auto O>
    static constexpr bool kHas = (Same(O, M) || ...);

    template <auto... O>
    struct HasAllOf {
      static constexpr bool value = (kHas<O> && ...);
    };

    template <typename F>
    static constexpr bool OnAllMembers(F&& f, T& x) {
      return (std::invoke(f, x.*M) && ...);
    }

    // Inside this extra template layer, both M... and LocalM... parameter
    // packs are available to be expanded in parallel.
    template <auto... LocalM>
    struct LocalMembersImpl {
      static_assert(sizeof...(LocalM) == sizeof...(M),
                    "AbiLocal::AbiMembers mismatch: AbiLocal alias not to same template?");

      template <typename Context>
      static constexpr bool FromLocal(Context& ctx, T& out, const Local& in) {
        return (MemberFromLocal<M, LocalM, Context>(ctx, out, in) && ...);
      }
    };

    template <typename Context>
    static constexpr bool FromLocal(Context& ctx, T& out, const Local& in) {
      using Impl = Local::template AbiMembers<LocalMembersImpl>;
      return Impl::FromLocal(ctx, out, in);
    }
  };
  using Members = T::template AbiMembers<MembersImpl>;

  template <typename Context>
  static constexpr bool FromLocal(Context&& ctx, T& out, const Local& in) {
    return Bases::FromLocal(ctx, out, in) && Members::FromLocal(ctx, out, in);
  }

  static_assert(sizeof(T) == Members::kSizeBytes,
                "AbiMembers list incomplete or struct has implicit padding");

  // This will check that the template parameter lists all the members in
  // T::AbiMembers.
  template <auto... M>
  static constexpr void AssertAbiMembers() {
    using CheckMembers = MembersImpl<M...>;
    using HasAll = T::template AbiMembers<CheckMembers::template HasAllOf>;
    static_assert(HasAll::value);
  }
};

// This will quickly prevent compilation for any type not handled by the
// dispatch above or by an explicit (perhaps partial) specialization
// (implemented below or elsewhere).
template <typename T>
struct RemoteAbiTranscriberImpl<T, RemoteAbiTranscriberImplType::kUnsupported> {
  static_assert(std::is_void_v<T>, "need RemoteAbiTranscriber specialization");
};

// std::array types are copied piecewise.  There is no specialization for
// direct T[N] because transcribable types should use std::array instead.
template <typename T, size_t N>
struct RemoteAbiTranscriber<std::array<T, N>> {
  using Remote = std::array<T, N>;
  using LocalT = RemoteAbiTranscriber<T>::Local;
  using Local = std::array<LocalT, N>;

  template <typename Context>
  static constexpr bool FromLocal(Context&& ctx, Remote& out, const Local& in) {
    for (size_t i = 0; i < N; ++i) {
      if (!RemoteAbiTranscriber<T>::FromLocal(ctx, out[i], in[i])) {
        return false;
      }
    }
    return true;
  }
};

// Ultimately the only truly interesting kind of specialization is for
// pointers, via AbiPtr.  The remote pointer value's representation is always
// Elf::Addr, but how it's computed is delegated back to the Context object
// that's passed into FromLocal via the `FromLocalPtr` method:
//
// ```
// std::optional<Addr> FromLocalPtr(const LocalT& ptr);
// ```
//
// Here LocalT is the local equivalent of T, `RemoteAbiTranscriber<T>::Local`:
// the remote type `AbiPtr<T, Elf, RemoteTraits>` corresponds to the local type
// `AbiPtr<LocalT, Elf>`.  This method can be templated and/or have specific
// overloads for specific types: it must be callable with a `const LocalT&`
// when `AbiPtr<LocalT>` is used.
//
// This method must have a non-const overload if the Context object is passed
// to FromLocal as `const Context&`.  If there is only a non-const overload of
// FromLocalPtr, then a mutable (or rvalue) `Context` object must be passed to
// FromLocal, since that reference (or value) is what MemberFromLocal will be
// called on.
//
// 'FromLocalPtr' can return `std::nullopt` for failure.  The details of the
// failure should be diagnosed in some fashion before return, using the Context
// object as appropriate to direct that.  On success, it should yield the
// address value that this pointer should have in the remote address space.
//
template <typename T, class Elf, class RemoteTraits>
struct RemoteAbiTranscriber<elfldltl::AbiPtr<T, Elf, RemoteTraits>> {
  using Addr = Elf::Addr;

  using Remote = elfldltl::AbiPtr<T, Elf, RemoteTraits>;
  using DecayedT = std::decay_t<T>;

  // Recay<DecayedT> -> T: restore the const/volatile lost from T.
  template <typename OtherT>
  using RecayV = std::conditional_t<std::is_volatile_v<T>, std::add_volatile_t<OtherT>, OtherT>;
  template <typename OtherT>
  using Recay = RecayV<std::conditional_t<std::is_const_v<T>, std::add_const_t<OtherT>, OtherT>>;

  // Propagate the const/volatile from T to the ...::Local type, but never
  // instantiate RemoteAbiTranscriber with a const/volatile type.
  using LocalT = Recay<typename RemoteAbiTranscriber<DecayedT>::Local>;
  using Local = elfldltl::AbiPtr<LocalT, Elf, elfldltl::LocalAbiTraits>;

  template <typename Context>
  static constexpr bool FromLocal(Context&& ctx, Remote& out, const Local& in) {
    if (!in) {
      // A default-constructed AbiPtr is a null pointer in every ABI.
      out = Remote{};
      return true;
    }

    // The Context::FromLocalPtr method is responsible for the translation of a
    // valid pointer.  It's passed as a (const) reference rather than a pointer
    // since it cannot be nullptr.
    std::optional<Addr> address = std::forward<Context>(ctx).FromLocalPtr(*in);
    if (!address) [[unlikely]] {
      return false;
    }
    out = Remote::FromAddress(*address);
    return true;
  }
};

// elfldltl::AbiSpan doesn't just provide the kClass template API because it
// doesn't know how to translate its value_type to its local counterpart so as
// to deliver the right AbiLocal.  Instead, this partial specialization for
// AbiSpan types just uses the AbiSpan constructor, ptr() and size() methods.
template <typename T, size_t N, class Elf, class RemoteTraits>
struct RemoteAbiTranscriber<elfldltl::AbiSpan<T, N, Elf, RemoteTraits>> {
  using Remote = elfldltl::AbiSpan<T, N, Elf, RemoteTraits>;
  using RemotePtr = Remote::Ptr;
  using PtrTranscriber = RemoteAbiTranscriber<RemotePtr>;
  using LocalPtr = PtrTranscriber::Local;
  using LocalValue = LocalPtr::value_type;
  using Local = elfldltl::AbiSpan<LocalValue, N, Elf>;

  template <typename Context>
  static constexpr bool FromLocal(Context&& ctx, Remote& out, const Local& in) {
    RemotePtr out_ptr;
    if (!RemoteAbiTranscriber<RemotePtr>::FromLocal(  //
            std::forward<Context>(ctx), out_ptr, in.ptr())) [[unlikely]] {
      return false;
    }
    out = Remote{out_ptr, in.size()};
    return true;
  }
};

// elfldltl::AbiStringView doesn't have that problem but it's handled by a
// similar specialization just for the uniformity that none of the classes in
// <lib/elfldltl/abi-*.h> provide AbiLocal et al.
template <class Elf, class RemoteTraits>
struct RemoteAbiTranscriber<elfldltl::AbiStringView<Elf, RemoteTraits>> {
  using Remote = elfldltl::AbiStringView<Elf, RemoteTraits>;
  using RemoteSpan = typename Remote::Span;
  using Local = elfldltl::AbiStringView<Elf>;

  template <typename Context>
  static constexpr bool FromLocal(Context&& ctx, Remote& out, const Local& in) {
    RemoteSpan out_span;
    if (!RemoteAbiTranscriber<RemoteSpan>::FromLocal(  //
            std::forward<Context>(ctx), out_span, in.as_span())) [[unlikely]] {
      return false;
    }
    out = Remote{out_span};
    return true;
  }
};

}  // namespace ld

#endif  // LIB_LD_REMOTE_ABI_TRANSCRIBER_H_
