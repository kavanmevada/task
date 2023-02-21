#![feature(test)]
extern crate test;

use core::future::Future;
use core::pin::Pin;
use std::panic::UnwindSafe;

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

mod spawn_recursive {
    use super::*;

    #[bench]
    fn task(b: &mut crate::test::bench::Bencher) -> impl std::process::Termination {
        b.iter(move || {
            futures_lite::future::block_on(async {
                let (runnable, task) =
                    task::spawn(async_fibo(10), |_, _cx| std::task::Poll::Ready(()));

                runnable.schedule().await;
                runnable.run().await;

                assert_eq!(task.await, Ok(55));
            })
        });
    }

    #[bench]
    fn async_task(b: &mut crate::test::bench::Bencher) -> impl std::process::Termination {
        b.iter(move || {
            futures_lite::future::block_on(async {
                let (runnable, task) = async_task::spawn(async_fibo(10), drop);
                runnable.run();

                assert_eq!(task.await, 55);
            })
        });
    }
}
