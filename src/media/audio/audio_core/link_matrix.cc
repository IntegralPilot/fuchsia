// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/link_matrix.h"

#include <algorithm>
#include <ostream>
#include <unordered_map>
#include <utility>

namespace media::audio {
namespace {

using LinkType = std::pair<AudioObject::Type, AudioObject::Type>;
constexpr std::array<LinkType, 3> kValidLinks{
    LinkType{AudioObject::Type::AudioRenderer, AudioObject::Type::Output},
    LinkType{AudioObject::Type::Input, AudioObject::Type::AudioCapturer},
    LinkType{AudioObject::Type::Output, AudioObject::Type::AudioCapturer},
};

void CheckLinkIsValid(AudioObject* source, AudioObject* dest) {
  FX_CHECK(source != nullptr);
  FX_CHECK(dest != nullptr);

  FX_CHECK(std::ranges::any_of(
      kValidLinks, [source_type = source->type(), dest_type = dest->type()](auto pair) {
        auto [valid_source_type, valid_dest_type] = pair;
        return source_type == valid_source_type && dest_type == valid_dest_type;
      }));
}

std::ostream& operator<<(std::ostream& out, const AudioObject* object) {
  out << static_cast<const void*>(object) << " ";
  if (object) {
    if (object->is_audio_capturer() || object->is_audio_renderer()) {
      out << "(" << object->usage()->ToString() << ")";
    } else {
      out << "(" << object->type() << ")";
    }
  }
  return out;
}

}  // namespace

zx_status_t LinkMatrix::LinkObjects(std::shared_ptr<AudioObject> source,
                                    std::shared_ptr<AudioObject> dest,
                                    std::shared_ptr<const LoudnessTransform> loudness_transform)
    FXL_LOCKS_EXCLUDED(lock_) {
  TRACE_DURATION("audio", "LinkMatrix::LinkObjects");
  CheckLinkIsValid(source.get(), dest.get());

  auto dest_link_init_result = source->InitializeDestLink(*dest);
  if (dest_link_init_result.is_error()) {
    return dest_link_init_result.error();
  }
  auto stream = dest_link_init_result.take_value();

  auto source_link_init_result = dest->InitializeSourceLink(*source, stream);
  if (source_link_init_result.is_error()) {
    return source_link_init_result.error();
  }
  auto [mixer, mix_domain] = source_link_init_result.take_value();

  {
    std::scoped_lock lock(lock_);
    DestLinkSet(source.get()).insert(Link(dest, loudness_transform, stream, mixer, mix_domain));
    SourceLinkSet(dest.get()).insert(Link(source, loudness_transform, stream, mixer, mix_domain));
  }

  source->OnLinkAdded();
  dest->OnLinkAdded();

  return ZX_OK;
}

void LinkMatrix::Unlink(AudioObject& key) FXL_LOCKS_EXCLUDED(lock_) {
  TRACE_DURATION("audio", "LinkMatrix::Unlink");
  std::scoped_lock lock(lock_);

  auto dest_list = DestLinkSet(&key);
  std::ranges::for_each(dest_list, [this, &key](auto& dest) {
    auto& sources = SourceLinkSet(dest.key);
    auto source = sources.find(Link(&key));
    if (source == sources.end()) {
      std::ostringstream oss;
      if constexpr (kLogRoutingChanges) {
        oss << &key;
      } else {
        oss << "AudioObject";
      }
      FX_LOGS(WARNING) << "Trying to unlink " << oss.str() << " -- no source found";
      return;
    }
    auto dest_object = dest.object.lock();
    if (dest_object) {
      dest_object->CleanupSourceLink(key, source->stream);
      key.CleanupDestLink(*dest_object);
    }

    sources.erase(Link(&key));
  });

  auto source_list = SourceLinkSet(&key);
  std::ranges::for_each(source_list, [this, &key](auto& source) {
    auto& dests = DestLinkSet(source.key);
    auto dest = dests.find(Link(&key));
    if (dest == dests.end()) {
      std::ostringstream oss;
      if constexpr (kLogRoutingChanges) {
        oss << &key;
      } else {
        oss << "AudioObject";
      }
      FX_LOGS(WARNING) << "Trying to unlink " << oss.str() << " -- no dest found";
      return;
    }

    auto source_object = source.object.lock();
    if (source_object) {
      source_object->CleanupDestLink(key);
      key.CleanupSourceLink(*source_object, dest->stream);
    }

    dests.erase(Link(&key));
  });

  sources_.erase(&key);
  dests_.erase(&key);
}

void LinkMatrix::ForEachDestLink(const AudioObject& object, fit::function<void(LinkHandle)> f)
    FXL_LOCKS_EXCLUDED(lock_) {
  TRACE_DURATION("audio", "LinkMatrix::ForEachDestLink");
  std::scoped_lock lock(lock_);

  for (auto& link : DestLinkSet(&object)) {
    TRACE_DURATION("audio", "LinkMatrix::ForEachDestLink.link");
    if (auto ptr = link.object.lock()) {
      f(LinkHandle{
          .object = ptr,
          .loudness_transform = link.loudness_transform,
          .stream = link.stream,
          .mixer = link.mixer,
          .mix_domain = link.mix_domain,
      });
    }
  }
}

void LinkMatrix::ForEachSourceLink(const AudioObject& object, fit::function<void(LinkHandle)> f)
    FXL_LOCKS_EXCLUDED(lock_) {
  TRACE_DURATION("audio", "LinkMatrix::ForEachSourceLink");
  std::scoped_lock lock(lock_);

  for (auto& link : SourceLinkSet(&object)) {
    TRACE_DURATION("audio", "LinkMatrix::ForEachSourceLink.link");
    if (auto ptr = link.object.lock()) {
      f(LinkHandle{
          .object = ptr,
          .loudness_transform = link.loudness_transform,
          .stream = link.stream,
          .mixer = link.mixer,
          .mix_domain = link.mix_domain,
      });
    }
  }
}

size_t LinkMatrix::DestLinkCount(const AudioObject& object) {
  std::scoped_lock lock(lock_);

  return DestLinkSet(&object).size();
}

size_t LinkMatrix::SourceLinkCount(const AudioObject& object) {
  std::scoped_lock lock(lock_);

  return SourceLinkSet(&object).size();
}

void LinkMatrix::DestLinks(const AudioObject& object, std::vector<LinkHandle>* store)
    FXL_LOCKS_EXCLUDED(lock_) {
  TRACE_DURATION("audio", "LinkMatrix::DestLinks");
  std::scoped_lock lock(lock_);

  OnlyStrongLinks(DestLinkSet(&object), store);
}

void LinkMatrix::SourceLinks(const AudioObject& object, std::vector<LinkHandle>* store)
    FXL_LOCKS_EXCLUDED(lock_) {
  TRACE_DURATION("audio", "LinkMatrix::SourceLinks");
  std::scoped_lock lock(lock_);

  OnlyStrongLinks(SourceLinkSet(&object), store);
}

bool LinkMatrix::AreLinked(const AudioObject& source, AudioObject& dest) FXL_LOCKS_EXCLUDED(lock_) {
  TRACE_DURATION("audio", "LinkMatrix::AreLinked");
  std::scoped_lock lock(lock_);

  auto dests = DestLinkSet(&source);
  bool forward_linked =
      std::ranges::any_of(dests, [&dest](const auto& candidate) { return candidate.key == &dest; });

  auto sources = SourceLinkSet(&dest);
  bool backward_linked = std::ranges::any_of(
      sources, [&source](const auto& candidate) { return candidate.key == &source; });
  FX_CHECK(forward_linked == backward_linked)
      << "Routing inconsistency " << (forward_linked ? "forward" : "backward") << "-linked but not "
      << (forward_linked ? "backward" : "forward") << "-linked";

  return forward_linked;
}

std::string LinkMatrix::UsageStrFromPair(const AudioObject* source, const AudioObject* dest) {
  if (!source) {
    return "No source";
  }
  if (!dest) {
    return "No dest";
  }
  if (source->is_audio_capturer() || source->is_audio_renderer()) {
    return source->usage() ? source->usage()->ToString() : "Unknown source usage";
  }
  return dest->usage() ? dest->usage()->ToString() : "Unknown dest usage";
}

void LinkMatrix::DisplayCurrentRouting() {
  if constexpr (!kLogRoutingChanges) {
    return;
  }

  std::scoped_lock lock(lock_);

  FX_LOGS(INFO) << "******************************************************************************";
  FX_LOGS(INFO) << "Per-source routing:";
  std::ostringstream out;
  for (const auto& [source, dests_for_source] : dests_) {
    out << "    " << source << " -> {";
    for (const auto& dest : dests_for_source) {
      FX_LOGS(INFO) << out.str();
      std::ostringstream().swap(out);
      out << "                                               " << dest.key << ",";
    }
    FX_LOGS(INFO) << out.str() << " }";
    std::ostringstream().swap(out);
  }
  FX_LOGS(INFO) << "Per-dest routing:";
  for (const auto& [dest, sources_for_dest] : sources_) {
    out << "  { ";
    for (const auto& source : sources_for_dest) {
      out << source.key << ",";
      FX_LOGS(INFO) << out.str();
      std::ostringstream().swap(out);
      out << "    ";
    }
    FX_LOGS(INFO) << out.str() << "                                      } -> " << dest;
    std::ostringstream().swap(out);
  }
  FX_LOGS(INFO) << "******************************************************************************";
}

void LinkMatrix::OnlyStrongLinks(LinkSet& link_set, std::vector<LinkHandle>* store) {
  TRACE_DURATION("audio", "LinkMatrix::OnlyStrongLinks");
  store->clear();
  for (const auto& link : link_set) {
    if (auto ptr = link.object.lock()) {
      store->push_back(LinkHandle{
          .object = ptr,
          .loudness_transform = link.loudness_transform,
          .stream = link.stream,
          .mixer = link.mixer,
          .mix_domain = link.mix_domain,
      });
    }
  }
}

LinkMatrix::LinkSet& LinkMatrix::SourceLinkSet(const AudioObject* object) FXL_REQUIRE(lock_) {
  if (!sources_.contains(object)) {
    sources_.insert({object, {}});
  }
  return sources_[object];
}

LinkMatrix::LinkSet& LinkMatrix::DestLinkSet(const AudioObject* object) FXL_REQUIRE(lock_) {
  if (!dests_.contains(object)) {
    dests_.insert({object, {}});
  }

  return dests_[object];
}

}  // namespace media::audio
