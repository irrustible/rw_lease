#![no_std]

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicUsize, Ordering};

use atomic_prim_traits::AtomicInt;
use primitive_traits::*;

// These are currently broken. We'll just not incude them for now so
// we can get the 0.1.0 release out
//
// #[cfg(feature="async")]
// mod future;
// #[cfg(feature="async")]
// pub use future::*;

/// Can happen when we try to take a lease.
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
pub struct RWLease<T, A = AtomicUsize> {
    atomic: A,
    value: UnsafeCell<T>,
}

impl<T, A> RWLease<T, A>
where
    A: AtomicInt,
    A::Prim: AddSign,
{
    pub fn new(value: T) -> RWLease<T, A> {
        RWLease {
            atomic: A::default(),
            value: UnsafeCell::new(value),
        }
    }

    #[cfg(test)]
    fn new_with_state(state: A::Prim, value: T) -> RWLease<T, A> {
        RWLease {
            atomic: AtomicInt::new(state),
            value: UnsafeCell::new(value),
        }
    }

    /// Attempt to take a read lease by CAS or explain why we couldn't.
    pub fn read(&self) -> Result<ReadGuard<T, A>, Blocked> {
        self.poll_read()?;
        Ok(ReadGuard::new(&self))
    }

    pub fn write(&self) -> Result<DrainGuard<T, A>, Blocked> {
        self.poll_write_mark()
            .map(|ready| DrainGuard::new(&self, ready))
    }

    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }

    fn poll_read(&self) -> Result<(), Blocked> {
        let mask = <<A::Prim as AddSign>::Signed as Integer>::MIN.drop_sign();
        let current = self.atomic.load(Ordering::SeqCst);
        if current < <A::Prim as Integer>::MAX {
            // avoid overflow on the next line
            let new = current + <A::Prim as Integer>::ONE;
            if new < mask {
                // Hot path, if we assume writes and read saturation are
                // rare. I would like to remove the CAS from here, but
                // until we have saturating addition or more complex
                // atomic ops, that doesn't seem possible.
                self.atomic
                    .compare_exchange_weak(current, new, Ordering::SeqCst, Ordering::SeqCst)
                    .map(drop)
                    .map_err(|_| Blocked::LostRace)
            } else if (current & mask) != mask {
                Err(Blocked::Readers)
            } else {
                Err(Blocked::Writer)
            }
        } else {
            Err(Blocked::Writer)
        }
    }

    fn poll_write_mark(&self) -> Result<bool, Blocked> {
        let mask = <<A::Prim as AddSign>::Signed as Integer>::MIN.drop_sign();
        let ret = self.atomic.fetch_or(mask, Ordering::SeqCst);

        if ret == <A::Prim as Integer>::ZERO {
            Ok(true)
        } else if (ret & mask) != mask {
            // No readers
            Ok(false)
        } else {
            // We'll have to wait for some readers
            Err(Blocked::Writer)
        }
    }

    fn poll_write_upgrade(&self) -> bool {
        let drained = <<A::Prim as AddSign>::Signed as Integer>::MIN.drop_sign();
        drained == self.atomic.load(Ordering::SeqCst)
    }

    fn done_reading(&self) -> <A as AtomicInt>::Prim {
        let one = <<A as AtomicInt>::Prim as Integer>::ONE;
        self.atomic.fetch_sub(one, Ordering::SeqCst)
    }

    fn done_writing(&self) {
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
where
    A: 'a + AtomicInt,
    A::Prim: AddSign,
    T: 'a,
{
    lease: &'a RWLease<T, A>,
    ready: bool,
}

impl<'a, T, A> DrainGuard<'a, T, A>
where
    A: 'a + AtomicInt,
    A::Prim: AddSign,
    T: 'a,
{
    fn new(lease: &'a RWLease<T, A>, ready: bool) -> DrainGuard<'a, T, A> {
        DrainGuard { lease, ready }
    }

    /// Attempts to upgrade to a WriteGuard. If readers are still
    /// locking it, returns self so you can try again
    pub fn upgrade(self) -> Result<WriteGuard<'a, T, A>, Self> {
        if self.ready || self.lease.poll_write_upgrade() {
            let lease = self.lease;
            // skip the drop handler
            core::mem::forget(self);
            Ok(WriteGuard::new(lease))
        } else {
            Err(self)
        }
    }
}

impl<'a, T, A> Drop for DrainGuard<'a, T, A>
where
    A: 'a + AtomicInt,
    A::Prim: AddSign,
    T: 'a,
{
    fn drop(&mut self) {
        let mask = !<<A::Prim as AddSign>::Signed as Integer>::MIN.drop_sign();
        self.lease.atomic.fetch_and(mask, Ordering::SeqCst);
    }
}

/// This guard signifies read access. When it drops, it will release the read lock.
#[derive(Debug)]
pub struct ReadGuard<'a, T, A>
where
    A: 'a + AtomicInt,
    A::Prim: AddSign,
    T: 'a,
{
    lease: &'a RWLease<T, A>,
}

impl<'a, T, A: AtomicInt> ReadGuard<'a, T, A>
where
    A: 'a + AtomicInt,
    A::Prim: AddSign,
    T: 'a,
{
    fn new(lease: &'a RWLease<T, A>) -> ReadGuard<'a, T, A> {
        ReadGuard { lease }
    }
}

impl<'a, T, A> Deref for ReadGuard<'a, T, A>
where
    A: 'a + AtomicInt,
    A::Prim: AddSign,
    T: 'a,
{
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lease.value.get() }
    }
}

impl<'a, T, A> Drop for ReadGuard<'a, T, A>
where
    A: 'a + AtomicInt,
    A::Prim: AddSign,
    T: 'a,
{
    fn drop(&mut self) {
        self.lease.done_reading();
    }
}

/// This guard signifies write access. When it drops, it will release the write lock.
#[derive(Debug)]
pub struct WriteGuard<'a, T, A>
where
    A: 'a + AtomicInt,
    A::Prim: AddSign,
    T: 'a,
{
    lease: &'a RWLease<T, A>,
}

impl<'a, T, A> WriteGuard<'a, T, A>
where
    A: 'a + AtomicInt,
    A::Prim: AddSign,
    T: 'a,
{
    fn new(lease: &'a RWLease<T, A>) -> WriteGuard<'a, T, A> {
        WriteGuard { lease }
    }
}

impl<'a, T, A> Deref for WriteGuard<'a, T, A>
where
    A: 'a + AtomicInt,
    A::Prim: AddSign,
    T: 'a,
{
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lease.value.get() }
    }
}

impl<'a, T, A> DerefMut for WriteGuard<'a, T, A>
where
    A: 'a + AtomicInt,
    A::Prim: AddSign,
    T: 'a,
{
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lease.value.get() }
    }
}

impl<'a, T, A> Drop for WriteGuard<'a, T, A>
where
    A: 'a + AtomicInt,
    A::Prim: AddSign,
    T: 'a,
{
    fn drop(&mut self) {
        self.lease.done_writing();
    }
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::AtomicU8;

    use crate::*;

    #[test]
    fn solo_reading() {
        let rw: RWLease<usize, AtomicUsize> = RWLease::new(123);
        let r = rw.read().expect("read guard");
        assert_eq!(*r, 123);
    }

    #[test]
    fn read_with_writer() {
        // maximum readers, writer bit
        let rw: RWLease<u8, AtomicU8> = RWLease::new_with_state(128, 123);
        assert_eq!(rw.read().unwrap_err(), Blocked::Writer);
    }

    #[test]
    fn read_all_ones() {
        // maximum readers, writer bit
        let rw: RWLease<u8, AtomicU8> = RWLease::new_with_state(255, 123);
        assert_eq!(rw.read().unwrap_err(), Blocked::Writer);
    }

    #[test]
    fn read_with_max_readers() {
        let rw: RWLease<u8, AtomicU8> = RWLease::new_with_state(127, 123);
        assert_eq!(rw.read().unwrap_err(), Blocked::Readers);
    }

    #[test]
    fn solo_writing() {
        let rw: RWLease<usize> = RWLease::new(123);
        {
            let d = rw.write().expect("drain guard");
            let mut w = d.upgrade().expect("write guard");
            assert_eq!(*w, 123);
            *w = 124;
            assert_eq!(*w, 124);
            assert_eq!(rw.read().unwrap_err(), Blocked::Writer);
        }
        let r = rw.read().expect("read guard");
        assert_eq!(*r, 124);
    }
}
