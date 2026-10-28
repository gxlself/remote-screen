import type {
  CoreEvent,
  CoreEventOf,
  CoreEventType,
} from '../../native/rscore/types';

export type Unsubscribe = () => void;

export type EventBus = {
  emit(event: CoreEvent): void;
  on(type: '*', cb: (event: CoreEvent) => void): Unsubscribe;
  on<T extends CoreEventType>(
    type: T,
    cb: (event: CoreEventOf<T>) => void,
  ): Unsubscribe;
};

type Listener = {
  type: CoreEventType | '*';
  cb: (event: CoreEvent) => void;
  active: boolean;
};

export function createEventBus(): EventBus {
  const listeners: Listener[] = [];

  const bus = {
    emit(event: CoreEvent) {
      for (const listener of [...listeners]) {
        if (!listener.active) {
          continue;
        }
        if (listener.type !== '*' && listener.type !== event.type) {
          continue;
        }
        try {
          listener.cb(event);
        } catch (error) {
          console.error(
            `RsCore event listener for ${event.type} failed`,
            error,
          );
        }
      }
    },
    on<T extends CoreEventType | '*'>(type: T, cb: (event: CoreEvent) => void) {
      const listener: Listener = {
        type,
        cb,
        active: true,
      };
      listeners.push(listener);
      return () => {
        if (!listener.active) {
          return;
        }
        listener.active = false;
        const index = listeners.indexOf(listener);
        if (index !== -1) {
          listeners.splice(index, 1);
        }
      };
    },
  };

  return bus as EventBus;
}
