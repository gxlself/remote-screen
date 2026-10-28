#pragma once

#include <algorithm>
#include <condition_variable>
#include <cstddef>
#include <cstdint>
#include <functional>
#include <memory>
#include <mutex>
#include <unordered_map>
#include <utility>
#include <vector>

namespace rsjni {

// The registry must outlive its callbacks. Callers must detach every consumer
// before destroying it; only the C ABI can establish the callback return barrier.
template <class Ctx>
class AttachRegistry {
 private:
  struct Entry {
    std::unique_ptr<Ctx> ctx;
    std::vector<Ctx*> attaching;
    uint64_t revision = 0;
    std::size_t users = 0;
    std::size_t callbacks = 0;
    bool busy = false;
  };

 public:
  class CallbackScope {
   public:
    CallbackScope(AttachRegistry& registry, Ctx* ctx)
        : registry_(registry), ctx_(ctx), previous_(currentCallback_) {
      // Include candidates: cached replay may arrive before doAttach returns.
      std::lock_guard<std::mutex> lock(registry_.mutex_);
      for (const auto& item : registry_.entries_) {
        const auto& entry = item.second;
        if (entry->ctx.get() == ctx ||
            std::find(entry->attaching.begin(), entry->attaching.end(), ctx) !=
                entry->attaching.end()) {
          id_ = item.first;
          entry_ = entry;
          ++entry_->callbacks;
          break;
        }
      }
      currentCallback_ = this;
    }

    ~CallbackScope() {
      currentCallback_ = previous_;
      if (entry_) {
        std::lock_guard<std::mutex> lock(registry_.mutex_);
        --entry_->callbacks;
        registry_.eraseUnused(id_, entry_);
      }
      // pendingDelete_ is destroyed after the lock has been released.
    }

    CallbackScope(const CallbackScope&) = delete;
    CallbackScope& operator=(const CallbackScope&) = delete;

   private:
    friend class AttachRegistry;
    AttachRegistry& registry_;
    Ctx* ctx_;
    CallbackScope* previous_;
    std::shared_ptr<Entry> entry_;
    uint64_t id_ = 0;
    std::unique_ptr<Ctx> pendingDelete_;
  };

  bool attach(uint64_t id, std::unique_ptr<Ctx> ctx,
              const std::function<bool(Ctx*)>& doAttach) {
    Operation operation(*this, id);
    const auto& entry = operation.entry;
    uint64_t revision;
    {
      std::lock_guard<std::mutex> lock(mutex_);
      entry->attaching.push_back(ctx.get());
      revision = entry->revision;
    }

    bool accepted;
    try {
      accepted = doAttach(ctx.get());
    } catch (...) {
      removeCandidate(entry, ctx.get());
      throw;
    }

    std::unique_ptr<Ctx> previous;
    {
      std::lock_guard<std::mutex> lock(mutex_);
      auto& candidates = entry->attaching;
      candidates.erase(std::find(candidates.begin(), candidates.end(), ctx.get()));
      // A reentrant callback may have detached/replaced this candidate during
      // cached replay. Do not resurrect it when the outer attach returns.
      if (accepted && revision == entry->revision) {
        previous = std::move(entry->ctx);
        entry->ctx = std::move(ctx);
        ++entry->revision;
      }
    }
    retire(std::move(previous));
    retire(std::move(ctx));
    return accepted;
  }

  void detach(uint64_t id, const std::function<void()>& doDetach) {
    Operation operation(*this, id);
    doDetach();
    std::unique_ptr<Ctx> previous;
    {
      std::lock_guard<std::mutex> lock(mutex_);
      previous = std::move(operation.entry->ctx);
      ++operation.entry->revision;
    }
    retire(std::move(previous));
  }

  bool contains(uint64_t id) const {
    std::lock_guard<std::mutex> lock(mutex_);
    const auto found = entries_.find(id);
    return found != entries_.end() && found->second->ctx != nullptr;
  }

 private:
  // Serialize competing calls for an id without holding any mutex across the
  // C ABI. Otherwise a completed call could free a newer C ABI consumer whose
  // registry bookkeeping has already completed on another thread.
  class Operation {
   public:
    Operation(AttachRegistry& registry, uint64_t id) : registry_(registry), id_(id) {
      std::unique_lock<std::mutex> lock(registry_.mutex_);
      auto& slot = registry_.entries_[id_];
      if (!slot) {
        slot = std::make_shared<Entry>();
      }
      entry = slot;
      ++entry->users;
      registry_.changed_.wait(lock, [&] {
        return !entry->busy || registry_.inCallback(entry);
      });
      ownsBusy_ = !entry->busy;
      entry->busy = true;
    }

    ~Operation() {
      {
        std::lock_guard<std::mutex> lock(registry_.mutex_);
        if (ownsBusy_) {
          entry->busy = false;
        }
        --entry->users;
        registry_.eraseUnused(id_, entry);
      }
      registry_.changed_.notify_all();
    }

    Operation(const Operation&) = delete;
    Operation& operator=(const Operation&) = delete;

    std::shared_ptr<Entry> entry;

   private:
    AttachRegistry& registry_;
    uint64_t id_;
    bool ownsBusy_ = false;
  };

  bool inCallback(const std::shared_ptr<Entry>& entry) const {
    for (auto* scope = currentCallback_; scope != nullptr; scope = scope->previous_) {
      if (&scope->registry_ == this && scope->entry_ == entry) {
        return true;
      }
    }
    return false;
  }

  void retire(std::unique_ptr<Ctx> ctx) {
    if (!ctx) {
      return;
    }
    CallbackScope* outermost = nullptr;
    for (auto* scope = currentCallback_; scope != nullptr; scope = scope->previous_) {
      if (&scope->registry_ == this && scope->ctx_ == ctx.get()) {
        outermost = scope;
      }
    }
    if (outermost != nullptr) {
      outermost->pendingDelete_ = std::move(ctx);
    }
  }

  void removeCandidate(const std::shared_ptr<Entry>& entry, Ctx* ctx) {
    std::lock_guard<std::mutex> lock(mutex_);
    auto& candidates = entry->attaching;
    candidates.erase(std::find(candidates.begin(), candidates.end(), ctx));
  }

  // Called only with mutex_ held. Waiters and callbacks keep the same Entry
  // discoverable until they finish, including after a self-detach.
  void eraseUnused(uint64_t id, const std::shared_ptr<Entry>& entry) {
    if (entry->users == 0 && entry->callbacks == 0 && !entry->ctx) {
      entries_.erase(id);
    }
  }

  inline static thread_local CallbackScope* currentCallback_ = nullptr;
  mutable std::mutex mutex_;
  std::condition_variable changed_;
  std::unordered_map<uint64_t, std::shared_ptr<Entry>> entries_;
};

}  // namespace rsjni
