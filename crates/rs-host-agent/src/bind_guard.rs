use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use host_core::update::agent::{UpdateService, SELF_CHECK_FRAME_WAIT, SELF_CHECK_LISTEN_WAIT};
use host_core::update::recovery::StartupOutcome;
use tokio::sync::watch;

/// A startup recovery operation that can be awaited before reporting a bind
/// failure to the process supervisor.
pub trait OutcomeWaiter: Send + Sync {
    fn wait_outcome(&self) -> Pin<Box<dyn Future<Output = StartupOutcome> + Send + '_>>;
}

impl OutcomeWaiter for UpdateService {
    fn wait_outcome(&self) -> Pin<Box<dyn Future<Output = StartupOutcome> + Send + '_>> {
        Box::pin(UpdateService::wait_outcome(self))
    }
}

fn outcome_wait_timeout() -> Duration {
    SELF_CHECK_LISTEN_WAIT + SELF_CHECK_FRAME_WAIT + Duration::from_secs(10)
}

/// Run one bind attempt and publish readiness only after it succeeds.
///
/// When startup recovery is attached, a failed bind is held until recovery
/// has finished (or the self-check budget expires), so a pending update can
/// roll back before the original bind error terminates the process.
pub async fn bind_then_signal<F, Fut, T, E>(
    bind: F,
    listening: &watch::Sender<bool>,
    guard: Option<&dyn OutcomeWaiter>,
) -> Result<T, E>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    match bind().await {
        Ok(value) => {
            listening.send_replace(true);
            Ok(value)
        }
        Err(error) => {
            if let Some(guard) = guard {
                let _ = tokio::time::timeout(outcome_wait_timeout(), guard.wait_outcome()).await;
            }
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    struct TestWaiter {
        rx: watch::Receiver<Option<StartupOutcome>>,
        called: Arc<AtomicBool>,
    }

    impl OutcomeWaiter for TestWaiter {
        fn wait_outcome(&self) -> Pin<Box<dyn Future<Output = StartupOutcome> + Send + '_>> {
            self.called.store(true, Ordering::SeqCst);
            let mut rx = self.rx.clone();
            Box::pin(async move {
                loop {
                    if let Some(outcome) = rx.borrow().clone() {
                        return outcome;
                    }
                    if rx.changed().await.is_err() {
                        return StartupOutcome::Clean;
                    }
                }
            })
        }
    }

    fn waiter() -> (
        Arc<TestWaiter>,
        watch::Sender<Option<StartupOutcome>>,
        Arc<AtomicBool>,
    ) {
        let (tx, rx) = watch::channel(None);
        let called = Arc::new(AtomicBool::new(false));
        (
            Arc::new(TestWaiter {
                rx,
                called: Arc::clone(&called),
            }),
            tx,
            called,
        )
    }

    #[tokio::test]
    async fn bind_success_signals_readiness() {
        let (listening, _) = watch::channel(false);
        let result = bind_then_signal(|| async { Ok::<_, ()>(7_u8) }, &listening, None).await;

        assert_eq!(result, Ok(7));
        assert!(*listening.borrow());
    }

    #[tokio::test]
    async fn bind_failure_with_guard_waits_before_returning_error() {
        tokio::time::pause();
        let (listening, _) = watch::channel(false);
        let (waiter, outcome, called) = waiter();
        let task = tokio::spawn(async move {
            bind_then_signal(
                || async { Err::<(), _>("bind failed") },
                &listening,
                Some(waiter.as_ref()),
            )
            .await
        });
        tokio::task::yield_now().await;
        assert!(called.load(Ordering::SeqCst));
        assert!(!task.is_finished());
        outcome.send(Some(StartupOutcome::Clean)).unwrap();

        assert_eq!(task.await.unwrap(), Err("bind failed"));
    }

    #[tokio::test]
    async fn bind_failure_without_guard_returns_immediately() {
        let (listening, _) = watch::channel(false);
        let result =
            bind_then_signal(|| async { Err::<(), _>("bind failed") }, &listening, None).await;

        assert_eq!(result, Err("bind failed"));
        assert!(!*listening.borrow());
    }

    #[tokio::test]
    async fn bind_failure_with_stuck_guard_is_bounded() {
        tokio::time::pause();
        let (listening, _) = watch::channel(false);
        let (waiter, _outcome, called) = waiter();
        let task = tokio::spawn(async move {
            bind_then_signal(
                || async { Err::<(), _>("bind failed") },
                &listening,
                Some(waiter.as_ref()),
            )
            .await
        });
        tokio::task::yield_now().await;
        assert!(called.load(Ordering::SeqCst));
        assert!(!task.is_finished());

        tokio::time::advance(outcome_wait_timeout()).await;
        assert_eq!(task.await.unwrap(), Err("bind failed"));
    }
}
