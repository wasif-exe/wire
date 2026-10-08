use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

pub trait Runnable {
    fn poll_run(&mut self, cx: &mut Context<'_>) -> Poll<()>;
}

pub struct Task<F> {
    future: Pin<Box<F>>,
}

impl<F> Task<F>
where
    F: Future<Output = ()> + 'static,
{
    pub fn new(future: F) -> Self {
        Self {
            future: Box::pin(future),
        }
    }
}

impl<F> Runnable for Task<F>
where
    F: Future<Output = ()> + 'static,
{
    fn poll_run(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.future.as_mut().poll(cx)
    }
}

pub struct TaskSlot {
    pub runnable: Option<Box<dyn Runnable>>,
    pub is_queued: bool,
    pub generation: u32,
}

impl TaskSlot {
    pub const fn empty() -> Self {
        Self {
            runnable: None,
            is_queued: false,
            generation: 0,
        }
    }
}
