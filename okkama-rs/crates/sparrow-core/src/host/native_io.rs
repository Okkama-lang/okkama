//! Heap-free Rust futures polled by Tokio. Publishing a completed result is a
//! separate synchronous step; no VM heap borrow can survive an I/O suspension.
use crate::host::*;
use okkama_runtime_abi::NativeFailure;
use std::{
    cell::{Cell, RefCell},
    future::Future,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll, Waker},
};

/// Implementations may own Rust data/resources, never untraced VM Slots.
pub(crate) trait Output {
    fn publish(self: Box<Self>, heap: &mut Heap) -> Result<Slot, Error>;
}
pub(crate) type ResultValue = Result<Box<dyn Output>, NativeFailure>;
pub(crate) type Body = Pin<Box<dyn Future<Output = ResultValue>>>;
pub(crate) type Cleanup = Pin<Box<dyn Future<Output = Result<(), NativeFailure>>>>;
pub(crate) type CancelHook = Box<dyn FnOnce() -> Cleanup>;
enum Completion {
    Value(ResultValue),
    Cancelled(Result<(), NativeFailure>),
}

pub(crate) struct Request {
    body: RefCell<Option<Body>>,
    result: RefCell<Option<Completion>>,
    cancel_hook: RefCell<Option<CancelHook>>,
    cleanup: RefCell<Option<Cleanup>>,
    cancelling: Cell<bool>,
    finished: Cell<bool>,
    /// The body is woken only from a provider thread (a SQLite worker), so
    /// the scheduler need not turn the reactor for it while it spins (#943).
    threaded: Cell<bool>,
    /// The waker of the last poll. Only the request's own driver frame polls
    /// it, and the driver makes a new waker for every poll, so keeping one per
    /// poll grew without bound while a request waited (#1010); the `Future`
    /// contract asks only that the latest be woken.
    waiter: RefCell<Option<Waker>>,
}
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeIoRequest")
            .field("finished", &self.finished.get())
            .finish()
    }
}
impl Request {
    fn new<F, T>(future: F) -> Rc<Self>
    where
        F: Future<Output = Result<T, NativeFailure>> + 'static,
        T: Output + 'static,
    {
        Rc::new(Self {
            body: RefCell::new(Some(Box::pin(async move {
                future.await.map(|v| Box::new(v) as Box<dyn Output>)
            }))),
            result: RefCell::new(None),
            cancel_hook: RefCell::new(None),
            cleanup: RefCell::new(None),
            cancelling: Cell::new(false),
            finished: Cell::new(false),
            threaded: Cell::new(false),
            waiter: RefCell::new(None),
        })
    }
    pub fn poll(&self, cx: &mut Context<'_>) -> Poll<()> {
        if self.finished.get() {
            return Poll::Ready(());
        }
        let result = if self.cancelling.get() {
            self.cleanup
                .borrow_mut()
                .as_mut()
                .expect("native cancellation cleanup")
                .as_mut()
                .poll(cx)
                .map(Completion::Cancelled)
        } else {
            self.body
                .borrow_mut()
                .as_mut()
                .expect("pending native I/O")
                .as_mut()
                .poll(cx)
                .map(Completion::Value)
        };
        match result {
            Poll::Pending => {
                let mut waiter = self.waiter.borrow_mut();
                if !waiter.as_ref().is_some_and(|w| w.will_wake(cx.waker())) {
                    *waiter = Some(cx.waker().clone());
                }
                Poll::Pending
            }
            Poll::Ready(value) => {
                self.body.borrow_mut().take();
                self.cleanup.borrow_mut().take();
                *self.result.borrow_mut() = Some(value);
                self.finished.set(true);
                self.wake();
                Poll::Ready(())
            }
        }
    }
    /// Wake the last poller, if any.
    fn wake(&self) {
        let waker = self.waiter.borrow_mut().take();
        if let Some(waker) = waker {
            waker.wake();
        }
    }
    /// True while only a provider thread can wake the request: its body is
    /// threaded and no cancellation cleanup (which may need the reactor) runs.
    pub fn off_reactor(&self) -> bool {
        self.threaded.get() && !self.cancelling.get() && !self.finished.get()
    }
    /// Returns true when the language Future must remain pending until cleanup.
    pub fn request_cancel(&self) -> bool {
        if self.cancelling.get() {
            return true;
        }
        let hook = self.cancel_hook.borrow_mut().take();
        if let Some(hook) = hook {
            self.body.borrow_mut().take();
            self.result.borrow_mut().take();
            self.cancelling.set(true);
            self.finished.set(false);
            *self.cleanup.borrow_mut() = Some(hook());
            self.wake();
            true
        } else {
            self.cancel();
            false
        }
    }
    pub fn cancel(&self) {
        self.cancel_hook.borrow_mut().take();
        self.cleanup.borrow_mut().take();
        self.body.borrow_mut().take();
        self.result.borrow_mut().take();
        self.finished.set(true);
        self.wake();
    }
}
impl Heap {
    pub(crate) fn native_io<F, T>(&mut self, body: F) -> Result<Slot, Error>
    where
        F: Future<Output = Result<T, NativeFailure>> + 'static,
        T: Output + 'static,
    {
        self.native_io_with_cleanup(body, None)
    }
    pub(crate) fn native_io_with_cleanup<F, T>(
        &mut self,
        body: F,
        cleanup: Option<CancelHook>,
    ) -> Result<Slot, Error>
    where
        F: Future<Output = Result<T, NativeFailure>> + 'static,
        T: Output + 'static,
    {
        let request = Request::new(body);
        *request.cancel_hook.borrow_mut() = cleanup;
        let slot = self.future_with_state(State::Io)?;
        self.futures.get_mut(slot.bits)?.io = Some(request);
        Ok(slot)
    }
    /// A request whose body is woken only from a provider thread (it waits on
    /// a channel a worker thread answers, never on the reactor or a timer);
    /// a cancellation cleanup is not bound by that.
    pub(crate) fn native_io_threaded<F, T>(
        &mut self,
        body: F,
        cleanup: Option<CancelHook>,
    ) -> Result<Slot, Error>
    where
        F: Future<Output = Result<T, NativeFailure>> + 'static,
        T: Output + 'static,
    {
        let slot = self.native_io_with_cleanup(body, cleanup)?;
        if let Some(request) = &self.futures.get(slot.bits)?.io {
            request.threaded.set(true);
        }
        Ok(slot)
    }
    /// Publish a completed request into its Future's state.
    pub(crate) fn native_io_settle(&mut self, future: Slot) -> Result<(), Error> {
        if !matches!(self.future(future)?.state, State::Io) {
            return Ok(());
        }
        let request = self
            .future(future)?
            .io
            .as_ref()
            .ok_or_else(|| Error("missing native I/O request".into()))?
            .clone();
        let result = request
            .result
            .borrow_mut()
            .take()
            .ok_or_else(|| Error("native I/O has not completed".into()))?;
        let value = match result {
            Completion::Value(Ok(output)) => match output.publish(self) {
                Ok(value) => Ok(value),
                Err(error) => Err(self.failure_new(FailureKind::Program(error.0))?),
            },
            Completion::Value(Err(error)) => {
                let payload = self.native_failure_new(error)?;
                Err(self.failure_new(FailureKind::Native(payload))?)
            }
            Completion::Cancelled(cleanup) => {
                let cancelled = self.failure_new(FailureKind::Cancelled)?;
                if let Err(error) = cleanup {
                    let payload = self.native_failure_new(error)?;
                    let secondary = self.failure_new(FailureKind::Native(payload))?;
                    self.failure_link(cancelled, secondary);
                }
                Err(cancelled)
            }
        };
        let cell = self.futures.get_mut(future.bits)?;
        cell.state = State::Ready(value);
        cell.io = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Nothing;
    impl Output for Nothing {
        fn publish(self: Box<Self>, _heap: &mut Heap) -> Result<Slot, Error> {
            Ok(Slot::default())
        }
    }

    fn pending() -> Rc<Request> {
        Request::new(std::future::pending::<Result<Nothing, NativeFailure>>())
    }

    fn poll(request: &Request) -> Poll<()> {
        request.poll(&mut Context::from_waker(Waker::noop()))
    }

    struct Count(std::sync::atomic::AtomicUsize);
    impl std::task::Wake for Count {
        fn wake(self: std::sync::Arc<Self>) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// #1010: the driver polls with a new waker every time; a waiting request
    /// keeps only the latest, and wakes it when it is cancelled.
    #[test]
    fn a_waiting_request_keeps_only_the_latest_waker() {
        let request = pending();
        let counts: Vec<std::sync::Arc<Count>> =
            (0..100).map(|_| std::sync::Arc::new(Count(std::sync::atomic::AtomicUsize::new(0)))).collect();
        for count in &counts {
            let waker = Waker::from(count.clone());
            assert!(request.poll(&mut Context::from_waker(&waker)).is_pending());
        }
        for count in &counts[..99] {
            assert_eq!(std::sync::Arc::strong_count(count), 1, "an earlier waker is not kept");
        }
        assert_eq!(std::sync::Arc::strong_count(&counts[99]), 2);
        *request.cancel_hook.borrow_mut() = Some(Box::new(|| Box::pin(std::future::pending())));
        assert!(request.request_cancel());
        assert_eq!(counts[99].0.load(std::sync::atomic::Ordering::Relaxed), 1, "the latest is woken");
    }

    /// A threaded request stays off the reactor only while its body runs: a
    /// cancellation cleanup may wait on anything (#943).
    #[test]
    fn threaded_request_is_off_reactor_until_cancelled() {
        let plain = pending();
        assert!(!plain.off_reactor());
        let threaded = pending();
        threaded.threaded.set(true);
        assert!(poll(&threaded).is_pending());
        assert!(threaded.off_reactor());
        *threaded.cancel_hook.borrow_mut() = Some(Box::new(|| Box::pin(std::future::pending())));
        assert!(threaded.request_cancel());
        assert!(!threaded.off_reactor(), "the cleanup may need the reactor");
        let done = Request::new(async { Ok(Nothing) });
        done.threaded.set(true);
        assert!(poll(&done).is_ready());
        assert!(!done.off_reactor());
    }
}
