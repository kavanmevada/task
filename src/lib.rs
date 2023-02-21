extern crate alloc;

use core::{
    any::Any,
    future::Future,
    marker::PhantomData,
    panic::{AssertUnwindSafe, UnwindSafe},
    pin::Pin,
    task::{Context, Poll, Waker},
};

use alloc::sync::Arc;
use std::sync::{atomic::AtomicBool, Mutex};

enum State<T: Send> {
    Ready(Result<T, Error>),
    Awaiting(Waker),
    NotYetPolled,
}

type BoxedAny = Box<dyn Any + Send>;
type ExecFn = dyn (Fn(Arc<Runnable>, &mut Context) -> Poll<()>) + Send + Sync;

pub struct Runnable {
    future: Mutex<Pin<BoxedAny>>,
    output: Mutex<BoxedAny>,
    schedule_fn: Arc<dyn Fn(Arc<Self>, &mut Context) -> Poll<()> + Send + Sync>,
    exec_fn: &'static ExecFn,
    is_cancelled: AtomicBool,
}

impl Unpin for Runnable {}

impl Runnable {
    pub fn cancel(&mut self) {
        self.is_cancelled
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn schedule<'r>(self: &'r Arc<Self>) -> impl Future<Output = ()> + 'r {
        core::future::poll_fn(move |cx| (self.schedule_fn)(self.clone(), cx))
    }

    pub fn run<'r>(self: Arc<Self>) -> impl Future<Output = ()> + 'r {
        std::future::poll_fn(move |cx| {
            let result = (self.exec_fn)(Arc::clone(&self), cx);

            if result.is_pending() {
                println!("re-scheduling");
            }

            match result {
                Poll::Ready(_) => Poll::Ready(()),
                Poll::Pending => match (self.schedule_fn)(Arc::clone(&self), cx) {
                    Poll::Ready(_) => Poll::Ready(()),
                    Poll::Pending => Poll::Pending,
                },
            }
        })
    }
}

pub struct Task<T> {
    runnable: Arc<Runnable>,
    _owned: PhantomData<T>,
}

impl<T: Send + 'static, Fut: Future<Output = T>> Future for Task<T> {
    type Output = Result<T, Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut res = self.runnable.output.lock().ok();

        let res = res
            .as_mut()
            .and_then(|o| o.downcast_mut::<State<T>>())
            .expect("failed to downcast or posioned");

        if self
            .runnable
            .is_cancelled
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Poll::Ready(Err(Error::FutCancelled));
        }

        match core::mem::replace(res, State::Awaiting(cx.waker().clone())) {
            State::Ready(o) => Poll::Ready(o),
            State::Awaiting(_) => Poll::Pending,
            State::NotYetPolled => Poll::Pending,
        }
    }
}

impl<T: Send + 'static, Fut: Future<Output = T> + UnwindSafe + 'static> Task<T> {
    const EXEC_FN: &'static ExecFn = &Task::<T>::exec;

    fn exec(runnable: Arc<Runnable>, cx: &mut Context) -> Poll<()> {
        if runnable
            .is_cancelled
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            if let Some(State::Awaiting(waker)) =
                runnable.output.lock().as_deref_mut().ok().and_then(|lock| {
                    core::mem::replace(lock, Box::new(State::<T>::NotYetPolled))
                        .downcast::<State<T>>()
                        .map(|s| *s)
                        .ok()
                })
            {
                waker.wake()
            }

            return Poll::Ready(());
        }

        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            unsafe {
                runnable
                    .future
                    .lock()
                    .unwrap()
                    .as_mut()
                    .map_unchecked_mut(|r| r.downcast_mut::<Fut>().unwrap())
            }
            .poll(cx)
        }));

        match match result {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(o)) => Poll::Ready(Ok(o)),
            Err(_) => Poll::Ready(Err(Error::ExecError)),
        } {
            Poll::Ready(o) => {
                if let Some(State::Awaiting(waker)) =
                    runnable.output.lock().as_deref_mut().ok().and_then(|lock| {
                        core::mem::replace(lock, Box::new(State::Ready(o)))
                            .downcast::<State<T>>()
                            .map(|s| *s)
                            .ok()
                    })
                {
                    waker.wake()
                }

                Poll::Ready(())
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

pub fn spawn<T: Send + 'static, Fut: Future<Output = T> + Send + UnwindSafe + 'static>(
    fut: Fut,
    schedule_fn: impl Fn(Arc<Runnable>, &mut Context) -> Poll<()> + Send + Sync + 'static,
) -> (Arc<Runnable>, Task<T>) {
    let output = Mutex::new(Box::new(State::<T>::NotYetPolled) as BoxedAny);

    let runnable = Arc::new(Runnable {
        future: Mutex::new(Box::pin(fut)),
        output,
        schedule_fn: Arc::new(schedule_fn),
        exec_fn: Task::<T>::EXEC_FN,
        is_cancelled: AtomicBool::new(false),
    });

    let task = Task {
        runnable: runnable.clone(),
        _owned: PhantomData,
    };

    (runnable, task)
}

#[derive(Debug, PartialEq)]
pub enum Error {
    ExecError,
    FutCancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn async_fibo(n: u64) -> Pin<Box<dyn Future<Output = u64> + Send + UnwindSafe>> {
        Box::pin(async move {
            // core::future::pending::<()>().await;

            match n {
                0 => 0,
                1 => 1,
                _ => async_fibo(n - 1).await + async_fibo(n - 2).await,
            }
        })
    }

    #[test]
    fn polled_before_run() {
        futures_lite::future::block_on(async {
            let (runnable, task) = spawn(async_fibo(10), |_, _cx| Poll::Ready(()));

            std::thread::spawn(move || {
                futures_lite::future::block_on({
                    std::thread::sleep(core::time::Duration::from_millis(250));

                    println!("runnable is polled!");
                    runnable.run()
                })
            });

            println!("task is polled!");
            assert_eq!(task.await, Ok(55));
        })
    }

    #[test]
    fn polled_after_run() {
        futures_lite::future::block_on(async {
            let (runnable, task) = spawn(async_fibo(10), |_, _cx| Poll::Ready(()));

            futures_lite::future::block_on(runnable.run());

            assert_eq!(task.await, Ok(55));
        })
    }
}
