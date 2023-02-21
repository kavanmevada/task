## About crate

[![Build](https://github.com/kavanmevada/task/actions/workflows/ci.yml/badge.svg)](
https://github.com/kavanmevada/task/actions)
[![License](https://img.shields.io/badge/license-Apache--2.0_OR_MIT-blue.svg)](
https://github.com/kavanmevada/task)

All executors have a queue that holds scheduled tasks:

```rust
let (sender, receiver) = flume::unbounded();
```

A task is created using `spawn()` which return a `Runnable` and a `Task`:

```rust
// A future that will be spawned.
let future = async { 1 + 2 };

// Construct a task.
let (runnable, task) = task::spawn(future, move |runnable, cx| {
    let fut = sender.send_async(runnable);
    pin_utils::pin_mut!(fut);

    fut.poll(cx)
});

runnable.schedule();
```

The `Runnable` is used to poll the task's future, and the `Task` is used to await its
output.

Finally, we need a loop that takes scheduled tasks from the queue and runs them:

```rust
for runnable in receiver {

// A function that schedules the task when it gets woken up.
    runnable.run().await;
}
```

Method `run()` polls the task's future once. Then, the `Runnable`
vanishes and only reappears when its `Waker` wakes the task, thus
scheduling it to be run again.

## License

Licensed under either of

 * Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

#### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.