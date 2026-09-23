// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/utils/object_linker.h"

#include <lib/async/cpp/task.h>
#include <lib/async/cpp/wait.h>
#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>

#include <algorithm>
#include <functional>

#include "src/lib/fsl/handles/object_info.h"

namespace utils {

void ObjectLinkerBase::Link::LinkInvalidatedLocked(bool on_destruction) {
  if (link_invalidated_) {
    // link_invalidated_ will be reset to nullptr; this way we can avoid
    // assignment operators to |link_validated_|.
    auto link_invalidated = std::move(link_invalidated_);
    link_invalidated_ = [](auto) {};
    link_invalidated(on_destruction);
  }
}

void ObjectLinkerBase::Link::LinkUnresolvedLocked() {
  if (link_invalidated_) {
    // It's janky that we need to copy the closure before invoking it, but if we don't then there is
    // a crash when the mutable closure from `flatland::LinkSystem::CreateLinkToChild/Parent()` is
    // run a second time.  For unknown reasons, this causes the `ChildView/ParentViewportWatcher` to
    // be destroyed on the wrong dispatcher (as evidenced by a CHECK in the watcher's destructor).
    auto link_invalidated = link_invalidated_;
    link_invalidated(false);
  }
}

size_t ObjectLinkerBase::UnresolvedExportCount() {
  auto access = GetScopedAccess();
  return std::count_if(exports_.begin(), exports_.end(),
                       [](const auto& iter) { return iter.second.IsUnresolved(); });
}

size_t ObjectLinkerBase::UnresolvedImportCount() {
  auto access = GetScopedAccess();
  return std::count_if(imports_.begin(), imports_.end(),
                       [](const auto& iter) { return iter.second.IsUnresolved(); });
}

zx_koid_t ObjectLinkerBase::CreateEndpoint(zx::handle token, ErrorReporter* error_reporter,
                                           bool is_import) {
  // Select imports or exports to operate on based on the flag.
  auto& endpoints = is_import ? imports_ : exports_;
  auto access = GetScopedAccess();

  if (!token) {
    error_reporter->ERROR() << "Token is invalid";
    return ZX_KOID_INVALID;
  }

  zx_koid_t endpoint_id;
  zx_koid_t peer_endpoint_id;
  std::tie(endpoint_id, peer_endpoint_id) = fsl::GetKoids(token.get());
  if (endpoint_id == ZX_KOID_INVALID || peer_endpoint_id == ZX_KOID_INVALID) {
    error_reporter->ERROR() << "Token with ID " << token.get() << " refers to invalid objects";
    return ZX_KOID_INVALID;
  }

  auto endpoint_iter = endpoints.find(endpoint_id);
  if (endpoint_iter != endpoints.end()) {
    error_reporter->ERROR() << "Endpoint with id " << endpoint_id
                            << " is already in use by this ObjectLinker";
    return ZX_KOID_INVALID;
  }

  // Create a new endpoint in an unresolved state.  Full linking cannot occur
  // until Initialize() is called on the endpoint to provide a link object and
  // handler callbacks.
  auto raw_token = token.get();  // Read handle before move()-ing into Endpoint.
  auto dispatcher = async_get_default_dispatcher();
  auto emplaced_endpoint = endpoints.emplace(
      endpoint_id, Endpoint(peer_endpoint_id, std::move(token), dispatcher,
                            WaitForPeerDeath(dispatcher, raw_token, endpoint_id, is_import)));
  FX_DCHECK(emplaced_endpoint.second);

  return endpoint_id;
}

void ObjectLinkerBase::DestroyEndpoint(zx_koid_t endpoint_id, bool is_import, bool destroy_peer) {
  auto& endpoints = is_import ? imports_ : exports_;
  auto& peer_endpoints = is_import ? exports_ : imports_;
  auto access = GetScopedAccess();

  auto endpoint_iter = endpoints.find(endpoint_id);
  if (endpoint_iter == endpoints.end()) {
    FX_LOGS(ERROR) << "Attempted to remove an unknown endpoint " << endpoint_id
                   << " from ObjectLinker";
    return;
  }

  // If the object has a peer linked tell it about the object being removed,
  // which will immediately invalidate the peer.
  if (destroy_peer) {
    zx_koid_t peer_endpoint_id = endpoint_iter->second.peer_endpoint_id;
    auto peer_endpoint_iter = peer_endpoints.find(peer_endpoint_id);
    if (peer_endpoint_iter != peer_endpoints.end()) {
      Endpoint& peer_endpoint = peer_endpoint_iter->second;

      // Invalidate the peer endpoint.  If Initialize() has already been called on
      // the peer endpoint, then close its connection which will destroy it.
      // Otherwise, any future connection attempts will fail immediately with a
      // link_failed callback, due to peer_endpoint_id being marked as
      // invalid.
      //
      // This handles the case where the peer exists but Initialize() has not been
      // called on it yet (so no callbacks exist).
      peer_endpoint.peer_endpoint_id = ZX_KOID_INVALID;

      if (peer_endpoint.link) {
        if (peer_endpoint.peer_death_waiter) {
          // `peer_endpoint` is still waiting for a link to `endpoint`. This is a problem,
          // because, when `endpoint` is destroyed:
          // 1. `endpoint.token` will be destroyed
          // 2. `endpoint.token` is a `zx::handle` to the channel that
          // `peer_endpoint.peer_death_waiter`
          //    uses to detect the disappearance of `endpoint`.
          // 3. The kernel will then send a `PEER_CLOSED` signal to the peer's end of the channel,
          //    causing `peer_endpoint.peer_death_waiter` to execute.
          // 4. There is a race between the execution of `peer_endpoint.peer_death_waiter`, and the
          //    call to `Invalidate()` below. (The latter will free the memory that the former
          //    references.)
          FX_LOGS(ERROR)
              << "DestroyEndpoint() with |peer_endpoint.link && peer_endpoint.peer_death_waiter|";
        }
        peer_endpoint.link->Invalidate(/*on_destruction=*/false, /*invalidate_peer=*/true);
      } else {
        // It is safe to skip Invalidate. The peer_endpoint is in the map but its .link field is
        // nullptr, which means the peer link had called CreateEndpoint but did not reach
        // Initialize. Also, the .peer_endpoint_id field is set to ZX_KOID_INVALID just above, which
        // lets a belated call to Initialize to avoid a CHECK fail.
      }
    }
  }

  // At this point it is safe to completely erase the endpoint for the object.
  endpoints.erase(endpoint_iter);
}

void ObjectLinkerBase::InitializeEndpoint(ObjectLinkerBase::Link* link, zx_koid_t endpoint_id,
                                          bool is_import) {
  FX_DCHECK(link);

  auto& endpoints = is_import ? imports_ : exports_;
  auto access = GetScopedAccess();

  // Update the endpoint with the connection information.
  auto endpoint_iter = endpoints.find(endpoint_id);
  FX_DCHECK(endpoint_iter != endpoints.end());
  Endpoint& endpoint = endpoint_iter->second;

  // If the endpoint is no longer valid (i.e. its peer no longer exists), then
  // immediately signal a disconnection (which will destroy the endpoint)
  // instead of linking.
  //
  // This edge-case happens if the endpoint's peer is destroyed after the
  // endpoint is created, but before Initialize() is called on it.
  zx_koid_t peer_endpoint_id = endpoint.peer_endpoint_id;
  if (peer_endpoint_id == ZX_KOID_INVALID) {
    link->Invalidate(/*on_destruction=*/false, /*invalidate_peer=*/true);
    return;
  }

  if (!endpoint.link) {
    endpoint.link = link;

    // Attempt to locate and link with the endpoint's peer.
    AttemptLinking(endpoint_id, peer_endpoint_id, is_import);
  } else {
    endpoint.link = link;
  }
}

void ObjectLinkerBase::AttemptLinking(zx_koid_t endpoint_id, zx_koid_t peer_endpoint_id,
                                      bool is_import) {
  auto& endpoints = is_import ? imports_ : exports_;
  auto& peer_endpoints = is_import ? exports_ : imports_;
  auto access = GetScopedAccess();

  auto endpoint_iter = endpoints.find(endpoint_id);
  FX_DCHECK(endpoint_iter != endpoints.end());

  auto peer_endpoint_iter = peer_endpoints.find(peer_endpoint_id);
  if (peer_endpoint_iter == peer_endpoints.end()) {
    return;  // Peer endpoint hasn't even been created yet, bail.
  }

  Endpoint& endpoint = endpoint_iter->second;
  Endpoint& peer_endpoint = peer_endpoint_iter->second;
  if (!peer_endpoint.link) {
    return;  // Peer endpoint isn't connected yet, bail.
  }

  // Delete the peer waiters now that the endpoints are resolved.
  endpoint.peer_death_waiter = nullptr;
  peer_endpoint.peer_death_waiter = nullptr;

  // Do linking last, so clients see a consistent view of the Linker.
  // Always fire the callback for the Export first, so clients can rely on
  // callbacks firing in a certain order.
  if (is_import) {
    peer_endpoint.link->LinkResolved(endpoint.link);
    endpoint.link->LinkResolved(peer_endpoint.link);
  } else {
    endpoint.link->LinkResolved(peer_endpoint.link);
    peer_endpoint.link->LinkResolved(endpoint.link);
  }
}

std::unique_ptr<async::Wait> ObjectLinkerBase::WaitForPeerDeath(async_dispatcher_t* dispatcher,
                                                                zx_handle_t endpoint_handle,
                                                                zx_koid_t endpoint_id,
                                                                bool is_import) {
  FX_DCHECK(dispatcher);
  auto access = GetScopedAccess();
  // Each endpoint must be removed from being considered for linking if its
  // peer's handle is closed before the two entries are successfully linked.
  // This communication happens via the link_failed callback.
  //
  // Once linking has occurred, this communication happens via UnregisterExport
  // or UnregisterImport and the peer_destroyed callback.
  // TODO(https://fxbug.dev/42098366): Follow up on __ZX_OBJECT_PEER_CLOSED with Zircon.
  static_assert(ZX_CHANNEL_PEER_CLOSED == __ZX_OBJECT_PEER_CLOSED, "enum mismatch");
  static_assert(ZX_EVENTPAIR_PEER_CLOSED == __ZX_OBJECT_PEER_CLOSED, "enum mismatch");
  static_assert(ZX_FIFO_PEER_CLOSED == __ZX_OBJECT_PEER_CLOSED, "enum mismatch");
  static_assert(ZX_SOCKET_PEER_CLOSED == __ZX_OBJECT_PEER_CLOSED, "enum mismatch");
  auto waiter = std::make_unique<async::Wait>(
      endpoint_handle, __ZX_OBJECT_PEER_CLOSED, 0,
      [weak_this = weak_ptr_factory_.GetWeakPtr(), endpoint_id, is_import](
          async_dispatcher_t*, async::Wait*, zx_status_t status, const zx_packet_signal_t*) {
        if (status == ZX_ERR_CANCELED) {
          // (1) Can happen if this callback's dispatcher is shutting down.
          //     GFX runs these callbacks on the main (render) thread, and Flatland runs it on the
          //     instance thread, so it is okay to exit early.
          // (2) Can happen if the async::Wait object was itself destroyed as part of
          //     DestroyEndpoint. That path invalidates the peer eagerly, so it is okay
          //     to exit early.
          return;
        }

        // If `this` has been destroyed, there isn't really anything we can do here, so skip
        // entirely.
        if (!weak_this.get()) {
          FX_LOGS(ERROR) << "ObjectLinkerBase destroyed; skipping Endpoint cleanup";
          return;
        }

        auto access = weak_this->GetScopedAccess();
        auto& endpoints = is_import ? weak_this->imports_ : weak_this->exports_;
        auto endpoint_iter = endpoints.find(endpoint_id);
        if (endpoint_iter == endpoints.end()) {
          // Can happen if this callback executes after the async::Wait object
          // gets destroyed on another (Flatland) thread, and the cancel was lost.
          return;
        }
        Endpoint& endpoint = endpoint_iter->second;

        // Invalidate the endpoint.  If Initialize() has
        // already been called on the endpoint, then close
        // its connection (which will cause it to be
        // destroyed).  Any future connection attempts will
        // fail immediately with a link_failed call, due to
        // peer_endpoint_id being marked as invalid.
        endpoint.peer_endpoint_id = ZX_KOID_INVALID;
        if (endpoint.link) {
          endpoint.link->Invalidate(/*on_destruction=*/false, /*invalidate_peer=*/true);
        }
      });

  zx_status_t status = waiter->Begin(dispatcher);
  FX_DCHECK(status == ZX_OK);

  return waiter;
}

zx::handle ObjectLinkerBase::ReleaseToken(zx_koid_t endpoint_id, bool is_import) {
  auto& endpoints = is_import ? imports_ : exports_;
  auto& peer_endpoints = is_import ? exports_ : imports_;
  auto access = GetScopedAccess();

  // If the endpoint was resolved, it will still be invalidated, but the peer endpoint must be
  // unresolved first if it exists.
  auto endpoint_iter = endpoints.find(endpoint_id);
  FX_DCHECK(endpoint_iter != endpoints.end());

  zx_koid_t peer_endpoint_id = endpoint_iter->second.peer_endpoint_id;
  auto peer_endpoint_iter = peer_endpoints.find(peer_endpoint_id);
  if (peer_endpoint_iter == peer_endpoints.end()) {
    return std::move(endpoint_iter->second.token);
  }

  // Signal that the link is now unresolved, then re-create the peer death waiter to flag the
  // endpoint as unresolved.
  auto& peer_endpoint = peer_endpoint_iter->second;
  if (peer_endpoint.link) {
    peer_endpoint.link->LinkUnresolvedLocked();
  }

  peer_endpoint.peer_death_waiter = WaitForPeerDeath(
      peer_endpoint.dispatcher, peer_endpoint.token.get(), peer_endpoint_id, !is_import);

  return std::move(endpoint_iter->second.token);
}

}  // namespace utils
