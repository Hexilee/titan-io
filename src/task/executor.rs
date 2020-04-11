use crossbeam_deque::{Injector, Stealer, Worker};
use futures::task::{Context, Poll};
use once_cell::sync::Lazy;
use std::future::Future;
use std::iter;
use std::pin::Pin;
use std::sync::Arc;
use std::thread;

type Task = async_task::Task<()>;

pub struct JoinHandler<T>(async_task::JoinHandle<T, ()>);

impl<T> Future for JoinHandler<T> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.0).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(opt) => Poll::Ready(opt.unwrap()),
        }
    }
}

fn find_task<T>(local: &Worker<T>, global: &Injector<T>, stealers: &[Stealer<T>]) -> Option<T> {
    // Pop a task from the local queue, if not empty.
    local.pop().or_else(|| {
        // Otherwise, we need to look for a task elsewhere.
        iter::repeat_with(|| {
            // Try stealing a batch of tasks from the global queue.
            global
                .steal_batch_and_pop(local)
                // Or try stealing a task from one of the other threads.
                .or_else(|| stealers.iter().map(|s| s.steal()).collect())
        })
        // Loop while no task was stolen and any steal operation needs to be retried.
        .find(|s| !s.is_retry())
        // Extract the stolen task, if there is one.
        .and_then(|s| s.success())
    })
}

static SCHEDULE: Lazy<Arc<Injector<Task>>> = Lazy::new(|| {
    let injector = Arc::new(Injector::new());
    let nums = num_cpus::get();
    let workers = (0..nums).map(|_| Worker::new_fifo()).collect::<Vec<_>>();
    let stealers = workers
        .iter()
        .map(|worker| worker.stealer())
        .collect::<Vec<Stealer<Task>>>();
    for worker in workers {
        let injector = injector.clone();
        let stealers = stealers.clone();
        thread::spawn(move || loop {
            if let Some(task) = find_task(&worker, &injector, &stealers) {
                task.run()
            }
        });
    }
    injector
});

pub fn spawn<F, R>(fut: F) -> JoinHandler<R>
where
    R: 'static + Send,
    F: 'static + Send + Future<Output = R>,
{
    let (task, handler) = async_task::spawn(fut, |f| SCHEDULE.push(f), ());
    task.schedule();
    JoinHandler(handler)
}