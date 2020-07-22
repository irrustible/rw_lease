use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicUsize, Ordering};
use atomic_prim_traits::AtomicInt;
use primitive_traits::*;

#[cfg(feature="async")]
mod future;
#[cfg(feature="async")]
pub use future::*;

/// Can happen when we try to take a read lease.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Blocked {
    /// There are too many readers, try again in a moment.
    Readers,
    /// There is a writer. Maybe it won't be just a moment, who knows?
    Writer,
    /// We were beaten by another thread in the CAS
    LostRace,
}

/// An RWLock, but:
/// * Choose your atomic unsigned integer for storage:
///   * We will steal the high bit for the writer.
///   * We will count readers on the remaining bits.
/// * Bring your own synchronisation primitive:
///   * No looping
/// * Writers wait for a lack of readers before assuming Write access.
#[derive(Debug)]
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

    /// Attempt to take a read lease by CAS or explain why we couldn't.
    pub fn try_read(&self) -> Result<ReadGuard<T, A>, Blocked> {
        self.poll_read()?;
        Ok(ReadGuard::new(&self))
    }

    pub fn try_write<'a>(&'a self) -> Result<DrainGuard<'a, T, A>, Blocked> {
        self.poll_write_mark().map(|ready| DrainGuard::new(&self, ready))
    }

    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }

    // pub crate

    pub(crate) fn poll_read(&self) -> Result<(), Blocked> {
        let mask = <<A::Prim as AddSign>::Signed as Integer>::MIN.drop_sign();
        let current = self.atomic.load(Ordering::SeqCst);
        let new = current + <A::Prim as Integer>::ONE;
        if new < mask {
            // hot path, if we assume writes and read saturation are rare
            if self.atomic.compare_exchange_weak(
                current, new, Ordering::SeqCst, Ordering::SeqCst
            ).is_ok() {
                Ok(())
            } else {
                Err(Blocked::LostRace)
            }
        } else if (current & mask) != mask {
            Err(Blocked::Readers)
        } else {
            Err(Blocked::Writer)
        }
        
    }

    pub(crate) fn poll_write_mark(&self) -> Result<bool, Blocked> {
        let mask = <<A::Prim as AddSign>::Signed as Integer>::MIN.drop_sign();
        let ret = self.atomic.fetch_or(mask, Ordering::SeqCst);
        if ret == <A::Prim as Integer>::ZERO {
            Ok(true) // We can take write access straight away
        } else if (ret & mask) != mask {
            Ok(false) // We can
        } else {
            Err(Blocked::Writer)
        }
    }

    pub(crate) fn poll_write_upgrade(&self) -> bool {
        let drained = <<A::Prim as AddSign>::Signed as Integer>::MIN.drop_sign();
        drained == self.atomic.load(Ordering::SeqCst)
    }

    pub(crate) fn done_reading(&self) -> <A as AtomicInt>::Prim {
        let one = <<A as AtomicInt>::Prim as Integer>::ONE;
        self.atomic.fetch_sub(one, Ordering::SeqCst)
    }

    pub(crate) fn done_writing(&self) {
        let mask = !<<A::Prim as AddSign>::Signed as Integer>::MIN.drop_sign();
        self.atomic.fetch_and(mask, Ordering::SeqCst);
    }

}

unsafe impl<T: Send> Send for RWLease<T> {}
unsafe impl<T: Sync> Sync for RWLease<T> {}

/// The DrainGuard represents waiting for the readers to release their
/// leases so we can take a write lease.
#[derive(Debug)]
pub struct DrainGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign, T: 'a {
    pub(crate) lease: Option<&'a RWLease<T, A>>,
    pub(crate) ready: bool,
}

impl<'a, T, A> DrainGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign, T: 'a {

    pub(crate) fn new(lease: &'a RWLease<T, A>, ready: bool) -> DrainGuard<'a, T, A> {
        DrainGuard { lease: Some(lease), ready }
    }

    /// Attempts to upgrade to a WriteGuard. If readers are still
    /// locking it, returns self so you can try again
    pub fn try_upgrade(mut self) -> Result<WriteGuard<'a, T, A>, DrainGuard<'a, T, A>> {
        if self.ready {
            return self.lease.take().map(|lease| WriteGuard::new(lease)).ok_or(self);
        }
        if let Some(lease) = self.lease.take() {
            if lease.poll_write_upgrade() {
                return Ok(WriteGuard::new(lease));
            }
        }
        Err(self)
    }
}

impl<'a, T, A> Drop for DrainGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign, T: 'a {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            let mask = !<<A::Prim as AddSign>::Signed as Integer>::MIN.drop_sign();
            lease.atomic.fetch_and(mask, Ordering::SeqCst);
        }
    }
}

/// This guard signifies read access. When it drops, it will release the read lock.
#[derive(Debug)]
pub struct ReadGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign, T: 'a {
    pub(crate) lease: Option<&'a RWLease<T, A>>, 
}

impl<'a, T, A: AtomicInt> ReadGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign, T: 'a {
    pub(crate) fn new(lease: &'a RWLease<T, A>) -> ReadGuard<'a, T, A> {
        ReadGuard { lease: Some(lease) }
    }
}

impl<'a, T, A> Deref for ReadGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign, T: 'a {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lease.unwrap().value.get() }
    }
}

impl<'a, T, A> Drop for ReadGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign, T: 'a {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            lease.done_reading();
        }
    }
}

/// This guard signifies write access. When it drops, it will release the write lock.
#[derive(Debug)]
pub struct WriteGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign, T: 'a {
    pub(crate) lease: &'a RWLease<T, A>,
}

impl<'a, T, A> WriteGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign, T: 'a {
    fn new(lease: &'a RWLease<T, A>) -> WriteGuard<'a, T, A> {
        WriteGuard { lease }
    }
}

impl<'a, T, A> Deref for WriteGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign, T: 'a {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lease.value.get() }
    }
}

impl<'a, T, A> DerefMut for WriteGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign, T: 'a {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lease.value.get() }
    }
}

impl<'a, T, A> Drop for WriteGuard<'a, T, A>
where A: 'a + AtomicInt, A::Prim: AddSign, T: 'a {
    fn drop(&mut self) {
        self.lease.done_writing();
    }
}
