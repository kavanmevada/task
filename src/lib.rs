extern crate alloc;

use core::{
    any::Any,
    future::{Future, IntoFuture},
    marker::PhantomData,
    panic::{AssertUnwindSafe, UnwindSafe},
    pin::Pin,
    task::{Context, Poll, Waker},
};

use alloc::sync::Arc;
use std::sync::Mutex;

enum State<T: Send> {
    Ready(Result<T, Error>),
    Awaiting(Waker),
    NotYetPolled,
}

type BoxedAny = Box<dyn Any + Send>;
type ExecFn = dyn (Fn(&mut Runnable, &mut Context) -> Poll<()>) + Send + Sync;

pub struct Runnable {
    future: Pin<BoxedAny>,
    output: Arc<Mutex<BoxedAny>>,
    exec_fn: &'static ExecFn,
}

impl Runnable {
    pub fn run_or<'r, F, B, O>(&'r mut self, mut f: F) -> impl Future<Output = ()> + 'r
    where
        F: (FnMut(&mut Self) -> B) + 'r,
        B: IntoFuture<Output = O>,
    {
        std::future::poll_fn(move |cx| {
            let result = (self.exec_fn)(self, cx);
            let fut = f(self).into_future();

            pin_utils::pin_mut!(fut);

            match result {
                Poll::Ready(_) => Poll::Ready(()),
                Poll::Pending => match fut.as_mut().poll(cx) {
                    Poll::Ready(_) => Poll::Ready(()),
                    Poll::Pending => Poll::Pending,
                },
            }
        })
    }
}

pub struct Task<T, F: Future<Output = T>> {
    output: Arc<Mutex<BoxedAny>>,
    _owned: PhantomData<F>,
}

impl<T: Send + 'static, F: Future<Output = T>> Future for Task<T, F> {
    type Output = Result<F::Output, Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut res = self.output.lock().ok();
        let res = res
            .as_mut()
            .and_then(|o| o.downcast_mut::<State<T>>())
            .expect("failed to downcast or posioned");

        match core::mem::replace(res, State::Awaiting(cx.waker().clone())) {
            State::Ready(o) => Poll::Ready(o),
            State::Awaiting(_) => Poll::Pending,
            State::NotYetPolled => Poll::Pending,
        }
    }
}

impl<T: Send + 'static, Fut: Future<Output = T> + UnwindSafe + 'static> Task<T, Fut> {
    const EXEC_FN: &'static ExecFn = &Task::<T, Fut>::exec;

    fn exec(runnable: &mut Runnable, cx: &mut Context) -> Poll<()> {
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            unsafe {
                runnable
                    .future
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
                            .downcast::<State<Fut::Output>>()
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
) -> (Runnable, Task<T, Fut>) {
    let output = Arc::new(Mutex::new(
        Box::new(State::<Fut::Output>::NotYetPolled) as BoxedAny
    ));

    let runnable = Runnable {
        future: Box::pin(fut),
        output: Arc::clone(&output),
        exec_fn: Task::<T, Fut>::EXEC_FN,
    };

    let task = Task {
        output,
        _owned: PhantomData,
    };

    (runnable, task)
}

#[derive(Debug, PartialEq)]
pub enum Error {
    ExecError,
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
            let (mut runnable, task) = spawn(async_fibo(10));

            std::thread::spawn(move || {
                futures_lite::future::block_on({
                    std::thread::sleep(core::time::Duration::from_millis(250));

                    println!("runnable is polled!");
                    runnable.run_or(|_| core::future::ready(println!("future re-scheduled!")))
                })
            });

            println!("task is polled!");
            assert_eq!(task.await, Ok(55));
        })
    }

    #[test]
    fn polled_after_run() {
        futures_lite::future::block_on(async {
            let (mut runnable, task) = spawn(async_fibo(10));

            futures_lite::future::block_on(
                runnable.run_or(|_| core::future::ready(println!("future re-scheduled!"))),
            );

            assert_eq!(task.await, Ok(55));
        })
    }
}
