use atomic_prim_traits::AtomicInt;
use event_listener::{Event, EventListener};
use primitive_traits::*;
use simple_mutex::Mutex;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicUsize, spin_loop_hint};
use super::{Blocked, RWLease};

const READ_SPINS: usize = 100;
const WRITE_SPINS: usize = 100;

/// Requires the `event-listener` feature.
pub struct AsyncRWLease<T, A=AtomicUsize>
where A: AtomicInt, A::Prim: AddSign {
    pub(crate) lease: RWLease<T,A>,
    pub(crate) read: Event,
    pub(crate) write: Mutex<Option<Waker>>, // *cry*
}

impl<T, A> AsyncRWLease<T, A>
where A: AtomicInt, A::Prim: AddSign + Into<usize> {

    pub fn new(value: T) -> AsyncRWLease<T, A> {
        AsyncRWLease {
            lease: RWLease::new(value),
            read: Event::new(),
            write: Mutex::new(None),
        }
    }

    pub fn poll_read<'a>(&'a self, wait_on_write: bool) -> PollReadGuard<'a, T, A> {
        PollReadGuard::new(self, wait_on_write)
    }

    pub fn poll_write<'a>(&'a self, wait_on_write: bool) -> PollWriteGuard<'a, T, A> {
        PollWriteGuard::new(self, wait_on_write)
    }

    pub fn into_inner(self) -> T {
        self.lease.into_inner()
    }

    fn done_reading(&self) {
        let mask = <<A::Prim as AddSign>::Signed as Integer>::MIN.drop_sign();
        let one = <A::Prim as Integer>::ONE;
        let old = self.lease.done_reading();
        if old == mask + one { // writing waiting, we're the last reader
            let mut lock = self.write.lock();
            if let Some(waker) = lock.take() {
                waker.wake();
            }
        } else if old < mask { // there may be a reader waiting
            self.read.notify_additional(1);
        }
    }

    fn done_writing(&self) {
        let max_readers = !<<A::Prim as AddSign>::Signed as Integer>::MIN.drop_sign();
        self.lease.done_writing();
        self.read.notify(max_readers.into());
    }

}

pub struct PollReadGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign + Into<usize>, T: 'a {
    pub(crate) lease: Option<&'a AsyncRWLease<T, A>>,
    /// If it's write locked, we may want to fail because it could take a while.
    pub(crate) wait_on_write: bool,
    /// How we will get an event it's ready to read
    pub(crate) listener: Option<EventListener>,
}

impl<'a, T, A> PollReadGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign + Into<usize>, T: 'a {
    fn new(lease: &'a AsyncRWLease<T,A>, wait_on_write: bool) -> Self {
        PollReadGuard { lease: Some(lease), wait_on_write, listener: None }
    }
}

impl<'a, T, A> Future for PollReadGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign + Into<usize>, T: 'a {
    type Output = Result<AsyncReadGuard<'a, T, A>, Blocked>;
    fn poll(self: Pin<&mut Self>, _ctx: &mut Context) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        if let Some(lease) = this.lease {
            let mut last_failure: Option<Blocked> = None;
            for _ in 1..READ_SPINS {
                if let Err(e) = lease.lease.poll_read() {
                    last_failure = Some(e);
                    spin_loop_hint();
                } else {
                    let guard = Ok(AsyncReadGuard::new(this.lease.take().unwrap()));
                    return Poll::Ready(guard);
                }
            }
            if (Some(Blocked::Writer) == last_failure) && !this.wait_on_write {
                Poll::Ready(Err(Blocked::Writer))
            } else {
                this.listener = Some(lease.read.listen());
                Poll::Pending
            }
        } else {
            panic!("PollReadGuard already resolved!")
        }
    }
}

impl<'a, T, A> Drop for PollReadGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign + Into<usize>, T: 'a {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            lease.done_writing();
        }
    }
}

pub struct PollWriteGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign + Into<usize>, T: 'a {
    pub(crate) lease: Option<&'a AsyncRWLease<T, A>>,
    /// If it's write locked, we may want to fail because it could take a while.
    pub(crate) wait_on_write: bool,
    /// Did we set the mark?
    marked: bool,
}

impl<'a, T, A> PollWriteGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign + Into<usize>, T: 'a {
    fn new(lease: &'a AsyncRWLease<T,A>, wait_on_write: bool) -> Self {
        PollWriteGuard { lease: Some(lease), wait_on_write, marked: false }
    }
}

impl<'a, T, A> Drop for PollWriteGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign + Into<usize>, T: 'a {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            lease.done_writing();
        }
    }
}

impl<'a, T, A> Future for PollWriteGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign + Into<usize>, T: 'a {
    type Output = Result<AsyncWriteGuard<'a, T, A>, Blocked>;
    fn poll(self: Pin<&mut Self>, ctx: &mut Context) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        if let Some(lease) = this.lease {
            if !this.marked {
                match lease.lease.poll_write_mark() {
                    Ok(false) => { this.marked = true; } // fall through
                    Ok(true) => {
                        return Poll::Ready(Ok(AsyncWriteGuard::new(this.lease.take().unwrap())));
                    }
                    Err(err) => { // only blocks on other writers
                        if this.wait_on_write {
                            *lease.write.lock() = Some(ctx.waker().clone());
                            match lease.lease.poll_write_mark() { // race - maybe it just finished?
                                Ok(false) => { this.marked = true; }
                                Ok(true) => {
                                    let lease = this.lease.take().unwrap();
                                    return Poll::Ready(Ok(AsyncWriteGuard::new(lease)));
                                }
                                _ => { return Poll::Pending; }
                            }
                        } else {
                            return Poll::Ready(Err(err))
                        }
                    }
                }
            }
            for _ in 1..WRITE_SPINS {
                if lease.lease.poll_write_upgrade() {
                    return Poll::Ready(Ok(AsyncWriteGuard::new(this.lease.take().unwrap())));
                } else {
                    spin_loop_hint();
                }
            }
            *lease.write.lock() = Some(ctx.waker().clone());
        }
        Poll::Pending // Either we already completed 
    }
}

/// Requires the `event-listener` feature.
pub struct AsyncReadGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign + Into<usize>, T: 'a {
    pub(crate) lease: &'a AsyncRWLease<T, A>,
}

impl<'a, T, A> AsyncReadGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign + Into<usize>, T: 'a {
    fn new(lease: &'a AsyncRWLease<T, A>) -> Self {
        AsyncReadGuard { lease }
    }
}

impl<'a, T, A> Deref for AsyncReadGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign + Into<usize>, T: 'a {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lease.lease.value.get() }
    }
}

impl<'a, T, A> Drop for AsyncReadGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign + Into<usize>, T: 'a {
    fn drop(&mut self) {
        self.lease.done_reading();
    }
}

/// Requires the `event-listener` feature.
pub struct AsyncWriteGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign + Into<usize>, T: 'a {
    pub(crate) lease: &'a AsyncRWLease<T, A>,
}

impl<'a, T, A> AsyncWriteGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign + Into<usize>, T: 'a {
    fn new(lease: &'a AsyncRWLease<T, A>) -> Self {
        AsyncWriteGuard { lease }
    }
}

impl<'a, T, A> Deref for AsyncWriteGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign + Into<usize>, T: 'a {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lease.lease.value.get() }
    }
}

impl<'a, T, A> DerefMut for AsyncWriteGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign + Into<usize>, T: 'a {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lease.lease.value.get() }
    }
}

impl<'a, T, A> Drop for AsyncWriteGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign + Into<usize>, T: 'a {
    fn drop(&mut self) {
        self.lease.done_writing();
    }
}
