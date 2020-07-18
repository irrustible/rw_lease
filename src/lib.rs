use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicUsize, Ordering};
use atomic_prim_traits::AtomicInt;
use primitive_traits::*;

// #[cfg(feature="event-listener")]
// mod future;
// #[cfg(feature="event-listener")]
// pub use future::*;

/// Can happen when we try to take a read lease.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Blocked {
    /// There are too many readers, try again in a moment.
    Readers,
    /// There is a writer. Maybe it won't be just a moment, who knows?
    Writer,
}

/// Like an RWLock, except a writer will deny new read leases when it
/// wishes to write and will then wait until there are no more
/// readers. Leases are counted in and out on an atomic integer whose
/// type may be provided as the optional `A` parameter.
pub struct RWLease<T, A=AtomicUsize>
where A: AtomicInt, A::Prim: AddSign {
    pub(crate) atomic: A,
    pub(crate) value: UnsafeCell<T>,
}

impl<T, A> RWLease<T, A>
where A: AtomicInt, A::Prim: AddSign {

    pub fn new(value: T) -> RWLease<T, A> {
        RWLease { atomic: A::default(), value: UnsafeCell::new(value) }
    }

    #[cfg(test)]
    pub(crate) fn new_with_state(state: usize, value: T) -> RWLease<T, A> {
        RWLease { atomic: AtomicInt::new(state), value: UnsafeCell::new(value) }
    }

    /// Attempt to take a read lease, which will fail if there are too
    /// many read leases or a writer lease.
    pub fn try_read(&self) -> Result<ReadGuard<T, A>, Blocked> {
        let mask = <<A::Prim as AddSign>::Signed as Integer>::MIN.drop_sign();
        loop {
            let current = self.atomic.load(Ordering::SeqCst);
            let new = current + <A::Prim as Integer>::ONE;
            if new < mask {
                if self.atomic.compare_exchange_weak(
                    current, new, Ordering::SeqCst, Ordering::SeqCst
                ).is_ok() {
                    return Ok(ReadGuard::new(&self));
                }
            } else if (current & mask) != mask {
                return Err(Blocked::Readers)
            } else {
                return Err(Blocked::Writer)
            }
        }
    }

    pub fn try_write<'a>(&'a self) -> Result<DrainGuard<'a, T, A>, Blocked> {
        let mask_on = <<A::Prim as AddSign>::Signed as Integer>::MIN.drop_sign();
        let ret = self.atomic.fetch_or(mask_on, Ordering::SeqCst);
        if ret == <A::Prim as Integer>::ZERO {
            Ok(DrainGuard::new(&self, true))
        } else if (ret & mask_on) != mask_on {
            Ok(DrainGuard::new(&self, false))
        } else {
            Err(Blocked::Writer)
        }
    }

    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }
}

/// The DrainGuard represents waiting for the readers to release their
/// leases so we can take a write lease.
pub struct DrainGuard<'a, T, A>
where A: AtomicInt, A::Prim: AddSign {
    pub(crate) inner: Option<&'a RWLease<T, A>>,
    pub(crate) ready: bool,
}

impl<'a, T, A> DrainGuard<'a, T, A>
where A: AtomicInt, A::Prim: AddSign {

    pub(crate) fn new(inner: &'a RWLease<T, A>, ready: bool) -> DrainGuard<'a, T, A> {
        DrainGuard { inner: Some(inner), ready }
    }

    /// Attempts to upgrade to a WriteGuard. If readers are still
    /// locking it, returns self so you can try again
    pub fn try_upgrade(mut self) -> Result<WriteGuard<'a, T, A>, DrainGuard<'a, T, A>> {
        if self.ready {
            self.inner.take().map(|inner| WriteGuard::new(inner)).ok_or(self)
        } else {
            if let Some(inner) = &self.inner {
                let drained = <<A::Prim as AddSign>::Signed as Integer>::MIN.drop_sign();
                if inner.atomic.load(Ordering::SeqCst) == drained {
                    return Ok(WriteGuard::new(&inner));
                }
            }
            Err(self)
        }
    }
}

impl<'a, T, A> Drop for DrainGuard<'a, T, A>
where A: AtomicInt, A::Prim: AddSign {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            let mask = !<<A::Prim as AddSign>::Signed as Integer>::MIN.drop_sign();
            inner.atomic.fetch_and(mask, Ordering::SeqCst);
        }
    }
}

/// This guard signifies read access. When it drops, it will release the read lock.
pub struct ReadGuard<'a, T, A>
where A: AtomicInt, A::Prim: AddSign {
    pub(crate) inner: Option<&'a RWLease<T, A>>, 
}

impl<'a, T, A: AtomicInt> ReadGuard<'a, T, A>
where A: AtomicInt, A::Prim: AddSign {
    pub(crate) fn new(inner: &'a RWLease<T, A>) -> ReadGuard<'a, T, A> {
        ReadGuard { inner: Some(inner) }
    }
}

impl<'a, T, A> Deref for ReadGuard<'a, T, A>
where A: AtomicInt, A::Prim: AddSign {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.inner.unwrap().value.get() }
    }
}

impl<'a, T, A> Drop for ReadGuard<'a, T, A>
where A: AtomicInt, A::Prim: AddSign {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            let mask_on = <<A::Prim as AddSign>::Signed as Integer>::MIN.drop_sign();
            let mask_off = !mask_on;
            loop {
                let atomic = &inner.atomic;
                let current: A::Prim = atomic.load(Ordering::SeqCst);
                let masked = current & mask_off;
                if masked > <A::Prim as Integer>::ZERO {
                    let subbed = masked - <A::Prim as Integer>::ONE;
                    let new = subbed | (current & mask_on);
                    if atomic.compare_exchange_weak(
                        current, new, Ordering::SeqCst, Ordering::SeqCst
                    ).is_ok() { return }
                } else {
                    panic!("How did you get a zero reader count?");
                }
            }
        }
    }
}

/// This guard signifies write access. When it drops, it will release the write lock.
pub struct WriteGuard<'a, T, A>
where A: AtomicInt, A::Prim: AddSign {
    pub(crate) inner: &'a RWLease<T, A>,
}

impl<'a, T, A> WriteGuard<'a, T, A>
where A: AtomicInt, A::Prim: AddSign {
    fn new(inner: &'a RWLease<T, A>) -> WriteGuard<'a, T, A> {
        WriteGuard { inner }
    }
}

impl<'a, T, A> Deref for WriteGuard<'a, T, A>
where A: AtomicInt, A::Prim: AddSign {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.inner.value.get() }
    }
}

impl<'a, T, A> DerefMut for WriteGuard<'a, T, A>
where A: AtomicInt, A::Prim: AddSign {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.inner.value.get() }
    }
}

impl<'a, T, A> Drop for WriteGuard<'a, T, A>
where A: AtomicInt, A::Prim: AddSign {
    fn drop(&mut self) {
        let mask = !<<A::Prim as AddSign>::Signed as Integer>::MIN.drop_sign();
        self.inner.atomic.fetch_and(mask, Ordering::SeqCst);
    }
}
