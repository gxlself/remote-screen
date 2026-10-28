#include "rs_attach_registry.h"

#include <atomic>
#include <cassert>
#include <future>
#include <iostream>
#include <stdexcept>
#include <thread>

namespace {

std::atomic<int> constructed{0};
std::atomic<int> destroyed{0};

struct FakeCtx {
  explicit FakeCtx(int value = 0) : value(value) {
    ++constructed;
  }
  ~FakeCtx() {
    ++destroyed;
  }
  const int value;
};

using Registry = rsjni::AttachRegistry<FakeCtx>;

void replacementAndFailure() {
  Registry registry;
  const int before = destroyed;
  assert(registry.attach(42, std::make_unique<FakeCtx>(1), [&](FakeCtx* ctx) {
    assert(ctx->value == 1);
    assert(!registry.contains(42));  // doAttach must run without the mutex.
    return true;
  }));
  assert(registry.contains(42));
  assert(destroyed == before);
  assert(registry.attach(42, std::make_unique<FakeCtx>(2), [](FakeCtx*) { return true; }));
  assert(destroyed == before + 1);
  assert(!registry.attach(42, std::make_unique<FakeCtx>(3), [](FakeCtx*) { return false; }));
  assert(destroyed == before + 2);
  assert(registry.contains(42));

  int detachCalls = 0;
  registry.detach(42, [&] {
    ++detachCalls;
    assert(registry.contains(42));  // doDetach also runs without the mutex.
  });
  assert(!registry.contains(42));
  assert(destroyed == before + 3);
  registry.detach(42, [&] { ++detachCalls; });
  assert(detachCalls == 2);
  assert(destroyed == before + 3);
}

void selfDetach() {
  Registry registry;
  auto ctx = std::make_unique<FakeCtx>();
  auto* raw = ctx.get();
  const int before = destroyed;
  assert(registry.attach(1, std::move(ctx), [](FakeCtx*) { return true; }));
  {
    Registry::CallbackScope outer(registry, raw);
    {
      Registry::CallbackScope inner(registry, raw);
      registry.detach(1, [] {});
      assert(!registry.contains(1));
      assert(destroyed == before);
    }
    assert(destroyed == before);  // Outermost scope owns deferred deletion.
  }
  assert(destroyed == before + 1);
}

void detachDuringReplay() {
  Registry registry;
  const int before = destroyed;
  assert(registry.attach(1, std::make_unique<FakeCtx>(), [&](FakeCtx* ctx) {
    // Match Rust's synchronous replay on a worker while Java waits in attach.
    std::thread callback([&] {
      Registry::CallbackScope scope(registry, ctx);
      registry.detach(1, [] {});
      assert(destroyed == before);
    });
    callback.join();
    return true;
  }));
  assert(!registry.contains(1));
  assert(destroyed == before + 1);
}

void replacementDuringCallback() {
  Registry registry;
  auto ctx = std::make_unique<FakeCtx>();
  auto* raw = ctx.get();
  const int before = destroyed;
  assert(registry.attach(1, std::move(ctx), [](FakeCtx*) { return true; }));
  {
    Registry::CallbackScope scope(registry, raw);
    assert(registry.attach(1, std::make_unique<FakeCtx>(), [](FakeCtx*) { return true; }));
    assert(destroyed == before);
    assert(registry.contains(1));
  }
  assert(destroyed == before + 1);
  registry.detach(1, [] {});
  assert(destroyed == before + 2);
}

void externalDetachWaitingForCallback() {
  Registry registry;
  auto ctx = std::make_unique<FakeCtx>();
  auto* raw = ctx.get();
  assert(registry.attach(1, std::move(ctx), [](FakeCtx*) { return true; }));
  const int before = destroyed;
  std::promise<void> callbackStarted;
  std::promise<void> detachEntered;
  auto entered = detachEntered.get_future();
  std::promise<void> callbackDone;
  auto done = callbackDone.get_future();
  std::thread callback([&] {
    {
      Registry::CallbackScope scope(registry, raw);
      callbackStarted.set_value();
      entered.wait();
      assert(registry.contains(1));
      // Bypass the pending external call, which is waiting for this callback.
      registry.detach(1, [] {});
      assert(destroyed == before);
    }
    callbackDone.set_value();
  });
  callbackStarted.get_future().wait();
  std::thread detacher([&] {
    registry.detach(1, [&] {
      detachEntered.set_value();
      done.wait();
    });
  });
  callback.join();
  detacher.join();
  assert(!registry.contains(1));
  assert(destroyed == before + 1);
}

void independentIds() {
  Registry registry;
  std::promise<void> attachEntered;
  std::promise<void> finishAttach;
  auto finish = finishAttach.get_future();
  std::thread first([&] {
    assert(registry.attach(1, std::make_unique<FakeCtx>(), [&](FakeCtx*) {
      attachEntered.set_value();
      finish.wait();
      return true;
    }));
  });
  attachEntered.get_future().wait();
  assert(registry.attach(2, std::make_unique<FakeCtx>(), [](FakeCtx*) { return true; }));
  registry.detach(2, [] {});
  finishAttach.set_value();
  first.join();
  registry.detach(1, [] {});
}

void throwingAttachReleasesOperation() {
  Registry registry;
  bool caught = false;
  try {
    registry.attach(1, std::make_unique<FakeCtx>(), [](FakeCtx*) -> bool {
      throw std::runtime_error("fake attach failure");
    });
  } catch (const std::runtime_error&) {
    caught = true;
  }
  assert(caught);
  assert(!registry.contains(1));
  assert(registry.attach(1, std::make_unique<FakeCtx>(), [](FakeCtx*) { return true; }));
  registry.detach(1, [] {});
}

void concurrentAttachDetach() {
  Registry registry;
  const int createdBefore = constructed;
  const int destroyedBefore = destroyed;
  std::promise<void> start;
  auto ready = start.get_future().share();
  std::atomic<int> activeCalls{0};
  auto worker = [&] {
    ready.wait();
    for (int i = 0; i < 1000; ++i) {
      assert(registry.attach(7, std::make_unique<FakeCtx>(i), [&](FakeCtx* ctx) {
        assert(++activeCalls == 1);
        assert(ctx->value == i);
        std::this_thread::yield();
        --activeCalls;
        return true;
      }));
      registry.detach(7, [&] {
        assert(++activeCalls == 1);
        std::this_thread::yield();
        --activeCalls;
      });
    }
  };
  std::thread first(worker);
  std::thread second(worker);
  start.set_value();
  first.join();
  second.join();
  registry.detach(7, [] {});
  assert(!registry.contains(7));
  assert(constructed - createdBefore == 2000);
  assert(destroyed - destroyedBefore == constructed - createdBefore);
}

}  // namespace

int main() {
  replacementAndFailure();
  selfDetach();
  detachDuringReplay();
  replacementDuringCallback();
  externalDetachWaitingForCallback();
  independentIds();
  throwingAttachReleasesOperation();
  concurrentAttachDetach();
  assert(constructed == destroyed);
  std::cout << "attach_registry_test: 8 tests passed; constructed=" << constructed
            << ", destroyed=" << destroyed << '\n';
}
