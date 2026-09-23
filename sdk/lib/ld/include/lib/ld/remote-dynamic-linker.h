// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_REMOTE_DYNAMIC_LINKER_H_
#define LIB_LD_REMOTE_DYNAMIC_LINKER_H_

#include <lib/zx/vmar.h>

#include <algorithm>
#include <optional>
#include <ranges>
#include <type_traits>

#include "abi.h"
#include "remote-abi-stub.h"
#include "remote-abi.h"
#include "remote-load-module.h"

namespace ld {

// ld::RemoteDynamicLinker represents a single remote dynamic linking session.
// It may or may not be the first or only dynamic linking session performed on
// the same process.  Each dynamic linking session defines its own symbolic
// dynamic linking domain and has its own passive ABI (stub dynamic linker).
//
// The second optional template parameter can select the "zygote mode"
// implementation.  This is used by the <lib/ld/remote-zygote.h> API, which
// provides the ld::RemoteZygote::Linker alias.  The zygote-mode linker is used
// in the same ways as the plain ld::RemoteDynamicLinker described here.
//
// Before creating an ld::RemoteDynamicLinker, the ld::RemoteAbiStub must be
// provided (see <lib/ld/remote-abi-stub.h>).  Only a single ld::RemoteAbiStub
// is needed to reuse the same stub dynamic linker binary across many dynamic
// linking sessions.  The ld::RemoteAbiStub can be provided in a constructor
// argument, or injected with the set_abi_stub method after default
// construction; it must be set before Init is called.
//
// The dynamic linking session proceeds in these phases, with one method each:
//
// * `Init()` starts the session by finding and decoding all the modules.  This
//   starts with root modules (such as a main executable), and acquires their
//   transitive DT_NEEDED dependencies via a callback function.  Additional
//   "implicit" modules may be specified to Init, such as a vDSO: these are
//   linked in even if they are not referenced by any DT_NEEDED dependency;
//   they always appear in the passive ABI, and if unreferenced will be last in
//   the list and have `.symbols_visible = false`.
//
//   Implicit modules serve multiple purposes.  One is just to satisfy any
//   DT_NEEDED dependencies for them--but that could just as well be handled by
//   giving the callback function special-case logic for certain names.  What's
//   unique about implicit modules is that they will be there even if they are
//   not referenced by any DT_NEEDED entry.  The use cases are things that are
//   going to be in the address space anyway for some reason other than use of
//   their symbolic ABI via dynamic linking.
//
//   One kind reason something is in the address space without its symbolic ABI
//   being used is that its code and/or data addresses are part of a direct ABI
//   used in some other way.  An example of this is the stub dynamic linker
//   (and likewise, the traditional in-process startup dynamic linker
//   implementing similar logic case): it contains runtime entry points used
//   implicitly by certain kinds of TLS accesses, so dynamic linking may
//   resolve certain relocations using addresses in this implicit shared
//   library, even though nothing refers to its DT_SONAME, nor to any symbol it
//   defines.  Another example is a vDSO: the process startup ABI includes
//   passing pointers into the vDSO image in various ways, so programs that
//   don't have a DT_NEEDED dependency on the vDSO anywhere might still have
//   code that follows those pointers and needs its code or data to be intact.
//
//   Another reason something is preemptively put into the address space is to
//   make sure its symbolic ABI will be available at runtime to things that use
//   it opportunistically or in special ways rather than via normal dynamic
//   linking dependencies.  When a module with `.symbols_visible = false` on
//   the list is also what it looks like when a module was added via `dlopen`
//   without using the `RTLD_GLOBAL` flag: a later attempt to `dlopen` that
//   name (or something else that transitively reaches a DT_NEEDED for that
//   same name) will find the module already loaded, and not try to load it
//   afresh.  Finally, it's important that every module that is present in the
//   address space for whatever reason be represented in the passive ABI of any
//   dynamic linking domain that might interact with it.  For example, unwinder
//   implemnentations will consult this module list (via the `dl_iterate_phdr`
//   API) to map any PC they come across to a module and find its unwinding
//   metadata.  Things go awry if a PC does not lie in a known module.
//
// * `Allocate()` sets the load address for each module using zx_vmar_allocate.
//   Each module gets a VMAR to reserve its part of the address space, and the
//   system call chooses a random available address for it (ASLR).  This is the
//   step that binds the dynamic linking session to a particular address layout
//   and set of relocation results, which depend on all the addresses.  The
//   session is now appropriate only for a single particular process, or a
//   single zygote that will spawn identical processes.  This is the first
//   point at which there is any need for a Zircon process to actually exist.
//   Creating the process and ultimately launching it are outside the scope of
//   this API.  The call to Allocate() must supply a zx::unowned_vmar where
//   zx::vmar::allocate() calls will be made to place each module.  That can be
//   the root VMAR of a process, or a smaller VMAR.  It must be large enough to
//   fit all the module images (including their .bss space beyond the size of
//   each ELF file image), and must permit the necessary mapping operations
//   (read, write, and execute, usually).  The absolute addresses for any
//   `Preplaced()` (`InitModule::WithLoadBias`) uses and their image sizes must
//   lie within this VMAR.
//
// * `Relocate()` fills in all the segment data that will need to be mapped in.
//   That is, it performs relocation on all modules and completes the passive
//   ABI data in the stub dynamic linker module.  When the Diagnostics object
//   passed to `Init()` used a policy of reporting multiple errors before
//   bailing out, then `Init()` returned "successfully" even if there were
//   errors reported such as an invalid ELF file or a `get_dep` callback that
//   couldn't find the file.  It may be appropriate to check the Diagnostics
//   object's error count and bail out before attempting relocation.  It's also
//   safe to have the policy of attempting relocation despite past errors in
//   the Init phase.  Any modules not decoded with sufficient success to safely
//   attempt relocation will be skipped.  Relocating the remaining modules may
//   produce many additional errors due to partially-decoded or corrupt
//   metadata or undefined symbols that would have come from missing or
//   corrupted files.  Such additional logging of e.g. undefined symbols may be
//   deemed useful, or not.  It is highly discouraged to proceed past the
//   Relocate phase on to additional calls if there were any errors reported
//   during or before the Relocate phase.  There is likely little benefit in
//   doing the address space layout or loading work to report additional error
//   details, and actually launching the process could be disastrous.
//
// * `Load()` loads all the segments finalized by `Relocate()` into the VMARs
//   created by `Allocate()`.  Since the VMARs have already been created and
//   the VMOs of segment contents already completed, nothing can usually go
//   wrong here except for resource exhaustion in creating mappings new VMOs
//   (either zero-fill or copy-on-write copies of relocated segments).  If
//   anything does go wrong, the process address space may be left in an
//   indeterminate state until the ld::RemoteDynamicLinker object is destroyed.
//
// * `Commit()` finally ensures that the VMARs created and mappings made can't
//   be changed or destroyed.  If Commit() is not called, then all the VMARs
//   will be destroyed when the ld::RuntimeDynamicLinker object is destroyed.
//
// After Commit() the object is only available for examining what was done.
// The VMAR handles are no longer available and the VmarLoader objects would
// need to be reinitialized to be used again.  The segment VMO handles are
// still available, but when not in zygote mode they are in use by process
// mappings and must not be touched.  In zygote mode, the relocated segment
// VMOs will be made read-only and then reused (directly for RELRO mappings or
// via copy-on-write copies) to load additional as-relocated process images.
//
// Various other methods are provided for interrogating the list of modules and
// accessing the dynamic linker stub module and the ld::RemoteAbi object.

template <class Elf = elfldltl::Elf<>, RemoteLoadZygote Zygote = RemoteLoadZygote::kNo,
          elfldltl::ElfMachine Machine = elfldltl::ElfMachine::kNative>
class RemoteDynamicLinker {
 public:
  using AbiStub = RemoteAbiStub<Elf, Machine>;
  using AbiStubPtr = AbiStub::Ptr;
  using Module = RemoteLoadModule<Elf, Zygote>;
  using DecodedModule = Module::Decoded;
  using DecodedModulePtr = DecodedModule::Ptr;
  using Soname = Module::Soname;
  using List = Module::List;
  using size_type = Elf::size_type;
  using TlsDescResolver = ld::StaticTlsDescResolver<Elf, Machine>;
  using TlsdescRuntimeHooks = TlsDescResolver::RuntimeHooks;

  // The Init method takes an InitModuleList as an argument.  Each element
  // describes an initial module, which is either a root module or an
  // implicitly-loaded module.  Init loads all these modules and all their
  // transitive dependencies as indicated by DT_NEEDED entries.
  //
  // Convenience functions are provided for creating InitModule objects for the
  // usual cases, or they can be default-constructed or aggregate-initialized
  // and their members set directly.  Each object requires a DecodedModulePtr,
  // acquired via <lib/ld/remote-decoded-module.h> RemoteDecodedModule::Create.
  //
  // The root modules are distinguished by having the `.visible_name` member
  // set.  The module list for this dynamic linking session starts with the
  // root modules in the order provided.  The list is then extended with all
  // their transitive dependencies, in breadth-first order, aka "load order".
  //
  // Each module with `.visible_name = std::nullopt` is an implicitly-loaded
  // module.  These are always loaded, but their place in the load order
  // depends on the DT_NEEDED dependency graph.  An implicitly-loaded module
  // must have a DT_SONAME; when a DT_NEEDED entry matches that name, the
  // module goes onto the list.  If no DT_NEEDED entry required the module,
  // then it is still loaded, but appears last in the list, with false for its
  // ld::abi::Abi<>::Module::symbols_visible flag.
  //
  // The `.load` member may optionally be initialized to direct how to load
  // that module.
  struct InitModule {
    // This is the default type for the `.load` member.  It says that the
    // module can go anywhere in the address space, leaving the choice up to
    // the kernel's ASLR within the VMAR passed to the Allocate method.
    struct LoadAnywhere {};

    // This type requests loading at a specific load address, which is
    // represented as the bias added to `.decoded_module->vaddr_start()`.
    struct WithLoadBias {
      WithLoadBias() = delete;
      constexpr explicit WithLoadBias(size_type bias) : load_bias{bias} {}

      size_type load_bias;
    };
    static_assert(!std::is_default_constructible_v<WithLoadBias>);
    static_assert(std::is_trivially_copyable_v<WithLoadBias>);

    // This type indicates that the module is already present in the process
    // address space and does not need to be loaded at all.  This module is
    // treated specially in that its DT_NEEDED dependencies won't be examined,
    // and the module itself won't be loaded.  Instead, it will just go into
    // the module list and provide symbol definitions as if it had been loaded.
    // As in WithLoadBias, its load address is specified in terms of the bias
    // added to its `.decoded_module->vaddr_start()`, as returned by the
    // load_bias() method on An existing module from a previous session.
    struct AlreadyLoaded {
      AlreadyLoaded() = delete;
      constexpr explicit AlreadyLoaded(size_type bias) : load_bias{bias} {}

      size_type load_bias;
    };
    static_assert(!std::is_default_constructible_v<AlreadyLoaded>);
    static_assert(std::is_trivially_copyable_v<AlreadyLoaded>);

    // This is the type of the `.load` member: one of the above, with the
    // default-constructed state being LoadAnywhere.
    using Load = std::variant<LoadAnywhere, WithLoadBias, AlreadyLoaded>;

    // This is the module to load.  It must be a valid pointer whose
    // `->HasModule()` returns true, indicating it was decoded sufficiently
    // successfully to attempt relocation safely.
    DecodedModulePtr decoded_module;

    // This indicates whether this is a visible initial module, i.e. a root
    // module.  The root modules go first in the load order and their symbols
    // are always marked as visible in ld::abi::Abi<>::Module::symbols_visible.
    // Each root module has a name.  A standard main executable is the only
    // root module and usually called ld::abi::Abi<>::kExecutableName (the
    // empty string, which is not the same as having no name!).  If left as
    // std::nullopt, this is instead an implicitly-loaded module.
    std::optional<Soname> visible_name;

    // This can be set to one of the types defined above to direct the loading.
    // When left to the default construction, this gets LoadAnywhere.
    Load load;
  };

  // The Init method takes a vector of InitModule objects, whose
  // `.decoded_module` references are consumed by the call.
  using InitModuleList = std::vector<InitModule>;

  // On success, the Init method returns a vector whose size matches the
  // InitModuleList::size() of the argument list.  Each InitResult element is
  // an iterator into the `modules()` list that corresponds to the InitModule
  // with the same index in the InitModuleList argument.
  using InitResult = std::vector<typename List::iterator>;

  // The `get_dep` callback passed to the Init method gets a DT_NEEDED string
  // and must return a value convertible to this.  The std::nullopt value
  // indicates the module could not be found or could not decoded, and
  // diagnostics have already been logged about those details.  Otherwise, it
  // must be a non-null DecodedModulePtr; often that module will have a
  // DT_SONAME matching the requested name, but it need not have a DT_SONAME at
  // all and if it does it need not match the requested name.  (Later DT_NEEDED
  // dependencies on either the name in the original request or the DT_SONAME
  // will all reuse the same module and not repeat the callback.)
  using GetDepResult = std::optional<DecodedModulePtr>;

  // If default-constructed, set_abi_stub() must be used before Init().
  RemoteDynamicLinker() = default;

  // The object is movable and move-assignable.
  RemoteDynamicLinker(RemoteDynamicLinker&&) = default;

  // The AbiStubPtr can be set in the constructor or with set_abi_stub().
  explicit RemoteDynamicLinker(AbiStubPtr abi_stub) : abi_stub_{std::move(abi_stub)} {}

  RemoteDynamicLinker& operator=(RemoteDynamicLinker&&) = default;

  const AbiStubPtr& abi_stub() const { return abi_stub_; }

  void set_abi_stub(AbiStubPtr abi_stub) { abi_stub_ = std::move(abi_stub); }

  // Shorthand to create an InitialModuleList element for a root module, which
  // always has an explicit name.
  static InitModule RootModule(DecodedModulePtr decoded_module, Soname visible_name) {
    return InitModule{
        .decoded_module = std::move(decoded_module),
        .visible_name = visible_name,
    };
  }

  // Shorthand to create an InitialModuleList element for the common case: the
  // main executable as root module.
  static InitModule Executable(DecodedModulePtr decoded_module) {
    return RootModule(std::move(decoded_module), abi::Abi<Elf>::kExecutableName);
  }

  // Shorthand to create an InitialModuleList element for an implicit module,
  // such as the vDSO.
  static InitModule Implicit(DecodedModulePtr decoded_module) {
    return InitModule{.decoded_module = std::move(decoded_module)};
  }

  // Shorthand to create an InitialModuleList element for a module whose load
  // bias is chosen rather than left to ASLR.  If the optional visible_name is
  // given, this is a root module; otherwise it's an implicit module.
  static InitModule Preplaced(  //
      DecodedModulePtr decoded_module, size_type load_bias,
      std::optional<Soname> visible_name = std::nullopt) {
    return InitModule{.decoded_module = std::move(decoded_module),
                      .visible_name = visible_name,
                      .load = WithLoadBias{load_bias}};
  }

  // Shorthand to create an InitialModuleList element for a module already
  // loaded in place.
  static InitModule Preloaded(  //
      DecodedModulePtr decoded_module, size_type load_bias,
      std::optional<Soname> visible_name = std::nullopt) {
    return InitModule{.decoded_module = std::move(decoded_module),
                      .visible_name = visible_name,
                      .load = AlreadyLoaded{load_bias}};
  }

  // Shorthand for turning a previous set of initial modules into a new one.
  // This produces the list for a secondary dynamic linking session that takes
  // the initial modules from this session as preloaded implicit modules.  This
  // takes the (successful) return value from Init, but it should be used only
  // after the Allocate phase when all the load addresses are known.
  InitModuleList PreloadedImplicit(const InitResult& list) {
    InitModuleList result;
    result.reserve(list.size());
    for (const auto& mod : list) {
      result.emplace_back(Preloaded(mod->decoded_module(), mod->load_bias()));
    }
    return result;
  }

  // Other accessors should be used only after a successful Init call (below).

  RemoteAbi<Module, Machine>& remote_abi() { return remote_abi_; }
  const RemoteAbi<Module, Machine>& remote_abi() const { return remote_abi_; }

  List& modules() { return modules_; }
  const List& modules() const { return modules_; }

  Module& abi_stub_module() { return modules_[stub_modid_]; }
  const Module& abi_stub_module() const { return modules_[stub_modid_]; }

  // When loading a main executable in the normal fashion, it's always the
  // first of the root modules given to Init() and so the first in the
  // modules() list.
  Module& main_module() { return modules_.front(); }
  const Module& main_module() const { return modules_.front(); }

  // Return the runtime address for the main module's Ehdr::e_entry PC address.
  // This should be used after Allocate(), below.
  size_type main_entry() const {
    const Module& main = main_module();
    return main.decoded().exec_info().relative_entry + main.load_bias();
  }

  // Return any PT_GNU_STACK size request from the main module.
  std::optional<size_type> main_stack_size() const {
    return main_module().decoded().exec_info().stack_size;
  }

  // Return the runtime address of the _ld_abi symbol in the stub dynamic
  // linker, which is the root of the passive ABI for this dynamic linking
  // namespace.  This should be used after Allocate(), below.  The data
  // structure to be mapped at that address will be completed by Relocate().
  size_type abi_vaddr() const { return abi_stub_->abi_vaddr() + abi_stub_module().load_bias(); }

  // Return the runtime address of the traditional r_debug struct for this
  // dynamic linking namespace, which might be understood by a debugger.
  size_type rdebug_vaddr() const {
    return abi_stub_->rdebug_vaddr() + abi_stub_module().load_bias();
  }

  // Find an existing Module in the modules() list by name or SONAME.  Returns
  // nullptr if none matches.  The returned pointer is invalidated by adding
  // modules to the list.
  Module* FindModule(Soname soname) {
    auto it = std::find(modules_.begin(), modules_.end(), soname);
    if (it != modules_.end()) {
      return &*it;
    }
    return nullptr;
  }

  // This returns false if any module was not successfully decoded enough to
  // attempt relocation on it.  If this returns true, some modules may still
  // have errors like missing or incomplete symbol or relocation information,
  // but it's at least valid to call Relocate on them to generate whatever
  // specific errors might result.
  bool AllModulesValid() const { return std::ranges::all_of(modules_, &Module::HasModule); }

  // This is a shorthand for a view filtered down to modules that have been
  // decoded successfully enough to attempt relocation on them, i.e. where
  // .HasModule() returns true.
  auto ValidModules() { return std::views::filter(modules_, &Module::HasModule); }
  auto ValidModules() const { return std::views::filter(modules_, &Module::HasModule); }

  // Initialize the session by finding and decoding all the modules.  The root
  // modules from initial_modules (those with `.visible_name` set) go onto the
  // list first in the same order in which they appear there, and then all the
  // transitive dependencies are added in breadth-first order.
  //
  // The implicit modules are used by DT_SONAME as needed.  Any module that is
  // never referenced via DT_NEEDED goes onto the list at the end, with its
  // `.symbols_visible = false`.  Note that implicit modules never have their
  // own DT_NEEDED lists examined: they are expected either to have no
  // dependencies or to have had their dependencies preloaded in some fashion.
  //
  // For any other dependency, call get_dep as `GepDepResult(Soname)`.  The
  // return value is an alias for `std::optional<DecodedModulePtr>`.  This
  // callback is responsible for doing its own diagnostics logging as needed.
  // If it returns `std::nullopt`, then Init will return `std::nullopt`
  // immediately, as when the Diagnostics object returns false.  If it instead
  // returns a null DecodedModulePtr, that will be treated like the Diagnostics
  // object returning true after a failure: that dependency will be omitted,
  // but processing continues.
  //
  // The return value is `std::nullopt` if the Diagnostics object returned
  // false for an error or the get_dep function returned `std::nullopt`.
  //
  // The InitResult on success has the same number of elements as the argument
  // initial_modules list, each giving the iterator into `.modules()` for that
  // initial module's place in the load order.  The modules() list is complete,
  // remote_abi() has been initialized, and abi_stub_module() can be used.
  template <class Diagnostics, typename GetDep>
  std::optional<InitResult> Init(Diagnostics& diag, InitModuleList initial_modules,
                                 GetDep&& get_dep,
                                 std::optional<elfldltl::ElfMachine> machine = Machine) {
    static_assert(std::is_invocable_r_v<GetDepResult, GetDep, Soname>);

    assert(abi_stub_);

    assert(!initial_modules.empty());

    assert(std::ranges::all_of(initial_modules, [](const auto& init) {
      return init.decoded_module && init.decoded_module->HasModule();
    }));

    auto next_modid = [this]() -> uint32_t { return static_cast<uint32_t>(modules_.size()); };

    auto check_machine = [machine, &diag](const DecodedModule& decoded) {
      return !machine || decoded.machine() == *machine ||
             // TODO(https://fxbug.dev/479505969): module-prefixed diagnostics here?
             diag.FormatError("wrong e_machine for architecture: ", decoded.machine());
    };

    // Start the list with the root modules.  The first one is the main
    // executable if there is such a thing.  It gets symbolizer module ID 0.
    std::vector<uint32_t> initial_modules_modid(initial_modules.size(), static_cast<uint32_t>(-1));
    size_t implicit_module_count = 1;  // The stub counts specially.
    for (size_t i = 0; i < initial_modules.size(); ++i) {
      InitModule& init_module = initial_modules[i];
      if (!check_machine(*init_module.decoded_module)) [[unlikely]] {
        return std::nullopt;
      }
      if (init_module.visible_name) {
        initial_modules_modid[i] = next_modid();
        EmplaceModule(*init_module.visible_name, std::nullopt,
                      std::move(init_module.decoded_module));
        PlaceInitModule(modules_.back(), init_module.load);
      } else {
        ++implicit_module_count;
      }
    }

    // If it's in the initial_modules list with no visible_name, then return
    // that decoded module and update initial_modules_modid accordingly.
    auto find_implicit = [this, &initial_modules, &initial_modules_modid, &implicit_module_count](
                             Module& module, uint32_t modid) -> bool {
      // Short-circuit if all implicit modules have already been consumed.
      if (implicit_module_count == 0) {
        return false;
      }

      auto use_decoded = [&module, modid, this](DecodedModulePtr decoded) {
        module.set_decoded(std::move(decoded), modid, true, max_tls_modid_);
      };

      // The stub is an always-injected implicit module.  Its location in the
      // list needs to be recorded specially so abi_stub_module() can find it.
      if (module.name() == kStubSoname) {
        assert(implicit_module_count > 0);
        --implicit_module_count;
        stub_modid_ = modid;
        use_decoded(abi_stub_->decoded_module());
        return true;
      }

      for (size_t i = 0; i < initial_modules.size(); ++i) {
        InitModule& init_module = initial_modules[i];
        if (!init_module.visible_name && init_module.decoded_module->soname() == module.name()) {
          assert(implicit_module_count > 0);
          --implicit_module_count;
          initial_modules_modid[i] = modid;
          // Don't check this element again.
          init_module.visible_name = module.name();
          use_decoded(std::move(init_module.decoded_module));
          PlaceInitModule(module, init_module.load);
          return true;
        }
      }

      return {};
    };

    // The root modules now form a queue of modules to be loaded.  Iterate over
    // that queue, adding additional entries onto the queue for each DT_NEEDED
    // list.  Once past the root modules, each RemoteDecodedModule must be
    // acquired.  The total number of iterations is not known until the loop
    // terminates, every transitive dependency having been decoded.
    for (size_t idx = 0; idx < modules_.size(); ++idx) {
      Module& mod = modules_[idx];

      // List index becomes symbolizer module ID.
      const uint32_t modid = static_cast<uint32_t>(idx);

      if (!mod.HasDecoded()) {
        // This isn't one of the root modules, so it's only a needed SONAME.

        if (find_implicit(mod, modid)) {
          // The SONAME matches one of the implicit modules.  Importantly,
          // these modules' DT_NEEDED lists are not examined to enqueue more
          // dependencies.  This module is in this dynamic linking namespace,
          // but its dependencies are not necessarily in the same namespace.
          continue;
        }

        // Use the callback to get a DecodedModulePtr for the SONAME.
        if (GetDepResult result = get_dep(mod.name())) [[likely]] {
          if (!*result) [[unlikely]] {
            // The get_dep function failed, but said to keep going anyway.
            continue;
          }
          if (!check_machine(**result)) [[unlikely]] {
            return std::nullopt;
          }
          mod.set_decoded(std::move(*result), modid, true, max_tls_modid_);
        } else {
          return std::nullopt;
        }
      }

      // This extends modules_ with new DT_NEEDED modules.
      EnqueueDeps(mod);
    }

    // Any remaining implicit modules that weren't reached go on the end of the
    // list, with .symbols_visible=false.
    if (implicit_module_count > 0) {
      for (size_t i = 0; i < initial_modules.size(); ++i) {
        InitModule& init_module = initial_modules[i];
        if (!init_module.visible_name) {
          initial_modules_modid[i] = next_modid();
          EmplaceUnreferenced(std::move(init_module.decoded_module));
          PlaceInitModule(modules_.back(), init_module.load);
          if (--implicit_module_count == (stub_modid_ == 0 ? 1 : 0)) {
            break;
          }
        }
      }
      assert(implicit_module_count == (stub_modid_ == 0 ? 1 : 0));

      // And finally the same for the stub dynamic linker.
      if (stub_modid_ == 0) {
        stub_modid_ = next_modid();
        EmplaceUnreferenced(abi_stub_->decoded_module());
      }
    }

    // Now that the full module list is set, the RemoteAbi can be initialized.
    // To do the passive ABI layout, that needs to know both the total number
    // of modules and the number of modules that have PT_TLS segments.  That
    // layout might change the vaddr_size() of the abi_stub_module(), so this
    // must happen before Allocate.
    zx::result abi_result =
        remote_abi_.Init(diag, abi_stub_, abi_stub_module(), modules_, max_tls_modid_);
    if (abi_result.is_error()) {
      diag.SystemError("cannot initialize remote ABI heap",
                       elfldltl::ZirconError{abi_result.error_value()});
      return {};
    }

    // Now that the modules_ list won't change and invalidate its iterators,
    // reify the initial_modules indices into iterators into it.
    std::optional<InitResult> result{std::in_place};
    result->reserve(initial_modules_modid.size());
    for (uint32_t modid : initial_modules_modid) {
      assert(modid != static_cast<uint32_t>(-1));
      result->push_back(modules_.begin() + modid);
    }
    assert(result->size() == initial_modules.size());
    return result;
  }

  // Initialize the loader and allocate the address region for each module,
  // updating their runtime addr fields on success.  This must be called before
  // Relocate or Load.  If Init was told to keep going after decoding errors,
  // then this will just skip any modules that weren't substantially decoded.
  template <class Diagnostics>
  bool Allocate(Diagnostics& diag, zx::unowned_vmar vmar) {
    auto allocate = [&vmar = *vmar, vmar_base = std::optional<uint64_t>{},
                     &diag](Module& module) mutable -> bool {
      if (module.preloaded()) {
        // This was an InitModule::AlreadyLoaded case where PlaceInitialModule
        // called Module::Preloaded.  There's nothing to do here: this module
        // is already in the address space.
        return true;
      }

      std::optional<size_t> vmar_offset;
      if (module.module().vaddr_end != 0) {
        // Init did SetModuleVaddrBounds for an InitModule::WithLoadBias case.
        // Turn the vaddr_start into an offset within this VMAR.
        if (!vmar_base) {
          zx_info_vmar_t info;
          zx_status_t status = vmar.get_info(ZX_INFO_VMAR, &info, sizeof(info), nullptr, nullptr);
          if (status != ZX_OK) [[unlikely]] {
            return diag.SystemError("ZX_INFO_VMAR: ", elfldltl::ZirconError{status});
          }
          vmar_base = info.base;
        }
        if (module.module().vaddr_start < *vmar_base) [[unlikely]] {
          return diag.SystemError("chosen load address below VMAR base");
        }
        vmar_offset = module.module().vaddr_start - *vmar_base;
      }
      return module.Allocate(diag, vmar, vmar_offset);
    };
    return std::ranges::all_of(ValidModules(), allocate);
  }

  // Acquire a StaticTlsDescResolver for Relocate that uses the stub dynamic
  // linker's TLSDESC entry points.  This resolver handles undefined weak
  // resolutions and it handles symbols resolved to a definition in a module
  // using static_tls_bias().  This is what's used by default if Relocate is
  // called with one argument.
  //
  // This can only be used after Allocate(), as that determines the runtime
  // code addresses for the stub dynamic linker; these addresses are stored in
  // the returned object.  Note they can also be modified later with e.g.
  // `.SetHook(TlsdescRuntime::kStatic, custom_hook)`; see <lib/ld/tlsdesc.h>.
  TlsDescResolver tls_desc_resolver() const {
    return abi_stub_->tls_desc_resolver(abi_stub_module().load_bias());
  }

  // Shorthand for the two-argument Relocate method below.
  bool Relocate(auto& diag) { return Relocate(diag, tls_desc_resolver()); }

  // Perform relocations on all modules.  The modules() list gives the set and
  // order of modules used for symbol resolution.
  //
  // For dynamic TLS references, the tls_desc_resolver is a callable object
  // with the signatures of ld::StaticTlsDescResolver (see <lib/ld/tlsdesc.h>),
  // usually from the tls_desc_resolver() method above to use runtime callbacks
  // supplied in the stub dynamic linker.
  //
  // If any module was not successfully decoded sufficiently to call the
  // Relocate method on that ld::RemoteLoadModule, then that module is just
  // skipped.  This doesn't cause a "failure" here because the Diagnostics
  // object must have reported the failures in decoding and decided to keep
  // going anyway, so there is nothing new to report.  The caller may have
  // decided to attempt relocation so as to diagnose all its specific errors,
  // rather than bailing out immediately after decoding failed on some of the
  // modules.  Probably callers will more often decide to bail out, since
  // missing dependency modules is an obvious recipe for undefined symbol
  // errors that aren't going to be more enlightening to the user.  But this
  // class supports any policy.
  template <class Diagnostics, typename TlsDescResolverType>
  bool Relocate(Diagnostics& diag, TlsDescResolverType&& tls_desc_resolver) {
    // If any module wasn't decoded successfully, just skip it.
    auto valid_modules = ValidModules();
    auto relocate = [&](auto& module) -> bool {
      // Resolve against successfully decoded modules, ignoring the others.
      return module.template Relocate<Machine>(diag, valid_modules, tls_desc_resolver);
    };

    // After the segments are complete, make sure all the VMO handles are
    // read-only so they don't accidentally get mutated.  This isn't necessary
    // in non-zygote mode since the object won't usually be saved long anyway.
    auto protect_segments = [&diag](auto& module) -> bool {
      auto protect_segment = [&diag]<class Segment>(Segment& segment) -> bool {
        if constexpr (elfldltl::kSegmentHasFilesz<Segment>) {
          zx::result<> result = segment.MakeImmutable();
          if (result.is_error()) [[unlikely]] {
            return diag.SystemError(  //
                "cannot drop ZX_RIGHT_WRITE from finished zygote VMO",
                elfldltl::ZirconError{result.error_value()});
          }
        }
        return true;
      };
      return module.load_info().VisitSegments(protect_segment);
    };

    return std::ranges::all_of(valid_modules, relocate) && FinishAbi(diag) &&
           (Zygote == RemoteLoadZygote::kNo ||  // No need for non-zygote.
            std::ranges::all_of(valid_modules, protect_segments));
  }

  // Load each module into the VMARs created by Allocate.  This should only be
  // attempted after Relocate has succeeded with no errors reported to the
  // Diagnostics object.  Loading the object will likely work if relocation was
  // incomplete, but using the code and data thus loaded would almost certainly
  // be very unsafe.  There's no benefit to loading code and then not starting
  // the process, so loading of incomplete state should not be attempted.
  //
  // After this, all the mappings are in place with their proper and final
  // protections.  The VMAR handles still exist to allow mapping changes, but
  // those VMARs will be destroyed when this object is destroyed unless Commit
  // is called first.
  template <class Diagnostics>
  bool Load(Diagnostics& diag) {
    auto load = [&diag](Module& module) { return module.Load(diag); };
    return std::ranges::all_of(ValidModules(), load);
  }

  // This should only be called after Load (and everything before) has
  // succeeded.  This commits all the mappings to their VMARs permanently.  The
  // sole handle to each VMAR is dropped here, so no more changes to those
  // VMARs can be made--only unmapping a whole module's vaddr range en masse to
  // destroy the VMAR.
  //
  // This should be the last use of the object when not in Zygote mode.  The
  // list of modules and each module's segments can still be examined, but the
  // VMOs for relocated segments are now being read and written through process
  // mappings and must not be disturbed.  The VmarLoader object for each module
  // will be in moved-from state, and cannot be used without reinitialization.
  //
  // In zygote mode, this object can be moved into the ld::RemoteZygote
  // constructor after Commit().  If it's moved in without Commit(), then all
  // the mappings made in the original process VMAR will be destroyed and the
  // existing process should not be started, but the zygote will still work
  // just the same to start more processes.
  void Commit() {
    for (Module& module : ValidModules()) {
      // After this, destroying the module won't destroy its VMAR any more.  No
      // more changes can be made to mappings in that VMAR, except by unmapping
      // the whole thing.
      module.Commit();
    }
  }

 private:
  using Loader = Module::Loader;

  using InitModuleLoad = InitModule::Load;
  using LoadAnywhere = InitModule::LoadAnywhere;
  using WithLoadBias = InitModule::WithLoadBias;
  using AlreadyLoaded = InitModule::AlreadyLoaded;

  static constexpr Soname kStubSoname = abi::Abi<Elf>::kSoname;

  // Add a new module to the list.  If no decoded_module is supplied here,
  // it must be fetched later as a dependency.
  void EmplaceModule(Soname name, std::optional<uint32_t> loaded_by_modid,
                     DecodedModulePtr decoded_module = {}, bool symbols_visible = true) {
    modules_.emplace_back(name, loaded_by_modid);
    if (decoded_module) {
      const uint32_t modid = static_cast<uint32_t>(modules_.size() - 1);
      modules_.back().set_decoded(std::move(decoded_module), modid, symbols_visible,
                                  max_tls_modid_);
    }
  }

  // Call EmplaceModule for each DT_NEEDED that's not already on the list.
  void EnqueueDeps(const Module& module) {
    if (!module.HasModule()) [[unlikely]] {
      // The module wasn't decoded properly so its DT_NEEDED was never
      // extracted, but Diagnostics said to keep going.
      assert(module.decoded().needed().empty());
      return;
    }

    const uint32_t loaded_by_modid = module.module().symbolizer_modid;
    for (Soname soname : module.decoded().needed()) {
      if (!FindModule(soname)) {
        EmplaceModule(soname, loaded_by_modid);
      }
    }
  }

  // Call EmplaceModule for an implicit module that was never referenced by any
  // DT_NEEDED.  This module will use its DT_SONAME as its name, and it will be
  // recorded as not being loaded by any other module.  This tells the ABI
  // remoting that the name string is found in its own RODATA (where its
  // DT_STRTAB is) rather than in a referring module's RODATA (its DT_STRTAB).
  void EmplaceUnreferenced(DecodedModulePtr decoded) {
    assert(decoded);
    EmplaceModule(decoded->soname(), std::nullopt, std::move(decoded), false);
  }

  // Dispatch to another overload for the specific type.
  static void PlaceInitModule(Module& mod, const InitModuleLoad& load) {
    auto place = [&mod](const auto& load) { PlaceInitModule(mod, load); };
    std::visit(place, load);
  }

  // Nothing special for a LoadAnywhere initial module.
  static void PlaceInitModule(Module& mod, LoadAnywhere anywhere) {}

  // For a WithLoadBias case, store the address for Allocate() to use.
  static void PlaceInitModule(Module& mod, WithLoadBias preplaced) {
    mod.Preplaced(preplaced.load_bias);
  }

  // For an AlreadyLoaded case, set up the addresses and make sure Allocate()
  // skips the module.
  static void PlaceInitModule(Module& mod, AlreadyLoaded preloaded) {
    mod.Preloaded(preloaded.load_bias);
  }

  template <class Diagnostics>
  bool FinishAbi(Diagnostics& diag) {
    if (stub_modid_ == 0) [[unlikely]] {
      // The Diagnostics object must have said to keep going after a previous
      // failure, since the stub module should always have been placed by now.
      return true;
    }
    assert(!modules().empty());
    zx::result<> result = std::move(remote_abi_).Finish(diag, abi_stub_module(), modules());
    if (result.is_error()) [[unlikely]] {
      return diag.SystemError("cannot complete passive ABI setup: ",
                              elfldltl::ZirconError{result.error_value()});
    }
    return true;
  }

  AbiStubPtr abi_stub_;
  RemoteAbi<Module, Machine> remote_abi_;
  List modules_;
  size_type max_tls_modid_ = 0;
  uint32_t stub_modid_ = 0;
};

}  // namespace ld

#endif  // LIB_LD_REMOTE_DYNAMIC_LINKER_H_
