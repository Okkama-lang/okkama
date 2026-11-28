//! Bounded log draining on the host pool; the VM keeps scheduling other tasks.
use super::{Error, Heap, Outcome, Output, Slot};

struct Drained;
impl Output for Drained {
    fn publish(self: Box<Self>, _heap: &mut Heap) -> Result<Slot, crate::natives::Refusal> {
        Ok(Slot::default())
    }
}

#[cfg(feature = "host")]
async fn wait_on_pool(wait: impl FnOnce() -> bool + Send + 'static) {
    let mut task = tokio::task::spawn_blocking(wait);
    // Pool admission is part of the bound too; cancellation must not leave
    // queued drain jobs behind an unrelated slow job.
    struct Abort(tokio::task::AbortHandle);
    impl Drop for Abort {
        fn drop(&mut self) { self.0.abort(); }
    }
    let _abort = Abort(task.abort_handle());
    let _ = tokio::time::timeout(okkama_runtime_core::log::DRAIN_TIMEOUT, &mut task).await;
}

impl Heap {
    pub(super) fn native_log_drain(&mut self) -> Result<Outcome, Error> {
        let drain = okkama_runtime_core::log::snapshot();
        let future = self.native_io(async move {
            #[cfg(feature = "host")]
            wait_on_pool(move || drain.wait()).await;
            #[cfg(not(feature = "host"))]
            let _ = drain.wait();
            Ok(Drained)
        })?;
        Ok(Outcome::Value(future))
    }
}

#[cfg(all(test, feature = "host"))]
mod tests {
    use super::*;
    use std::{
        future::Future,
        sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc},
        task::{Context, Waker},
        time::{Duration, Instant},
    };

    #[test]
    fn log_drain_yields_and_bounds_pool_admission() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all().max_blocking_threads(1).build().unwrap();
        runtime.block_on(async {
            let (started, begin) = tokio::sync::oneshot::channel();
            let (release, wait) = mpsc::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                let _ = started.send(());
                let _ = wait.recv_timeout(Duration::from_secs(5));
            });
            begin.await.unwrap();
            let ran = Arc::new(AtomicBool::new(false));
            let into = ran.clone();
            let mut drain = Box::pin(wait_on_pool(move || {
                into.store(true, Ordering::SeqCst);
                true
            }));
            let start = Instant::now();
            assert!(drain.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
            assert!(start.elapsed() < Duration::from_millis(250));
            // The reactor still runs while the blocking pool has no free worker.
            tokio::time::sleep(Duration::from_millis(10)).await;
            drain.await;
            assert!(start.elapsed() >= okkama_runtime_core::log::DRAIN_TIMEOUT);
            assert!(start.elapsed() < Duration::from_millis(2500));
            release.send(()).unwrap();
            blocker.await.unwrap();
            // A barrier on the single pool worker observes all earlier jobs.
            tokio::task::spawn_blocking(|| {}).await.unwrap();
            assert!(!ran.load(Ordering::SeqCst));
        });
    }
}
