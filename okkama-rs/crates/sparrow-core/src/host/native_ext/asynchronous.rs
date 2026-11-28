//! Only the owned job and atomic cancellation state cross the pool boundary.
use super::*;
use std::{cell::RefCell, future::Future, rc::Rc, sync::{Arc, atomic::AtomicBool}};

struct Job {
    pointer: *mut c_void,
    callbacks: &'static okk_async,
}
// Registration promises that jobs can move between the VM and pool threads.
unsafe impl Send for Job {}
impl Drop for Job {
    fn drop(&mut self) { unsafe { (self.callbacks.drop)(self.pointer); } }
}
impl Job {
    fn execute(self, cancellation: &Cancellation) -> Self {
        unsafe { (self.callbacks.execute)(self.pointer, (cancellation as *const Cancellation).cast()); }
        self
    }
}

#[repr(C)]
struct Cancellation {
    public: okk_cancel,
    requested: AtomicBool,
    interrupted: Option<Arc<AtomicBool>>,
}
unsafe extern "C" fn cancelled(public: *const okk_cancel) -> u32 {
    let state = unsafe { &*public.cast::<Cancellation>() };
    if state.interrupted.as_ref().is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        state.requested.store(true, Ordering::Relaxed);
    }
    u32::from(state.requested.load(Ordering::Relaxed))
}

struct Pending {
    task: RefCell<Option<tokio::task::JoinHandle<Job>>>,
    cancellation: Arc<Cancellation>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        // Detaching the join handle leaves the job owned by execute, even at teardown.
        self.cancellation.requested.store(true, Ordering::Relaxed);
    }
}
impl Pending {
    async fn finish(&self) -> Result<Option<Job>, NativeFailure> {
        std::future::poll_fn(|cx| {
            let mut task = self.task.borrow_mut();
            let Some(handle) = task.as_mut() else { return std::task::Poll::Ready(Ok(None)); };
            let result = std::pin::Pin::new(handle).poll(cx);
            if result.is_ready() { task.take(); }
            result.map(|result| result.map(Some))
        }).await
            .map_err(|_| NativeFailure { domain: "core.runtime".into(), operation: "extension execute".into(),
                code: "unexpected".into(), message: "Native extension worker failed".into() })
    }
}

struct Completed {
    job: Job,
    binding: Rc<Binding>,
}
impl super::super::native_io::Output for Completed {
    fn publish(self: Box<Self>, host: &mut Heap) -> Result<Slot, Refusal> {
        let (word, _) = invoke(host, &self.binding, &[], true, |c, _, out| unsafe {
            (self.job.callbacks.complete)(c, self.job.pointer, out)
        })?;
        Ok(host.slot(word, &self.binding.result))
    }
}

pub(super) fn start(host: &mut Heap, binding: Rc<Binding>, arguments: &[Word]) -> Result<Slot, Refusal> {
    let Callback::Async(callbacks) = binding.callback else { return Err(fault(&binding, "sync callback used asynchronously")); };
    let mut job = None;
    invoke(host, &binding, arguments, false, |c, args, _| {
        let mut pointer = std::ptr::null_mut();
        let status = unsafe { (callbacks.prepare)(c, args, &mut pointer) };
        // OK transfers ownership even if an ignored host fault later rejects the call.
        if status == OKK_OK { job = Some(Job { pointer, callbacks }); }
        status
    })?;
    let job = job.expect("successful prepare");
    let cancellation = Arc::new(Cancellation {
        public: okk_cancel { cancelled }, requested: AtomicBool::new(false),
        interrupted: unsafe { (*host.rt).limits.interrupt.clone() },
    });
    let worker_cancel = cancellation.clone();
    let pending = Rc::new(Pending {
        task: RefCell::new(Some(tokio::task::spawn_blocking(move || job.execute(&worker_cancel)))), cancellation,
    });
    let cleanup = pending.clone();
    host.native_io_threaded(async move {
        let job = pending.finish().await?.expect("unpublished extension job");
        Ok(Completed { job, binding })
    }, Some(Box::new(move || {
        cleanup.cancellation.requested.store(true, Ordering::Relaxed);
        Box::pin(async move { drop(cleanup.finish().await?); Ok(()) })
    }))).map_err(Into::into)
}
