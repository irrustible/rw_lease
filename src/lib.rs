use std::sync::atomic::{AtomicUsize, Ordering};

pub(crate) const BIT_ON: usize = isize::MIN as usize;
pub(crate) const BIT_OFF: usize = isize::MAX as usize;

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum TryReadError {
    ReadLocked,
    WriteLocked,
}

pub struct WriteLocked();

/// A Reader-Writer lock based on an AtomicUsize
pub struct RWLease {
    pub(crate) inner: AtomicUsize,
}

impl RWLease {
    pub fn new() -> RWLease {
        RWLease { inner: AtomicUsize::new(0) }
    }
    #[cfg(test)]
    pub(crate) fn new_at(what: usize) -> RWLease {
        RWLease { inner: AtomicUsize::new(what) }
    }
    /// Attempt to take a read lock, which will fail if there are too many read locks or a writer lock.
    pub fn try_read(&self) -> Result<ReadGuard, TryReadError> {
        loop {
            let current = self.inner.load(Ordering::SeqCst);
            if current < BIT_OFF {
                if self.inner.compare_exchange_weak(current, current+1, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                    return Ok(ReadGuard::new(&self.inner));
                }
            } else if current & BIT_ON != BIT_ON {
                return Err(TryReadError::ReadLocked)
            } else {
                return Err(TryReadError::WriteLocked)
            }
        }
    }
    pub fn try_write<'a>(&'a self) -> Result<Drain<'a>, WriteLocked> {
        let ret = self.inner.fetch_or(BIT_ON, Ordering::SeqCst);
        if ret == 0 {
            Ok(Drain::new(&self.inner, true))
        } else if ret & BIT_ON != BIT_ON {
            Ok(Drain::new(&self.inner, false))
        } else {
            Err(WriteLocked())
        }
    }
}

/// The Drain represents waiting for the readers to release their
/// locks so we can take a write lock.
pub struct Drain<'a> {
    pub(crate) inner: Option<&'a AtomicUsize>,
    pub(crate) ready: bool,
}

impl<'a> Drain<'a> {
    pub(crate) fn new(inner: &'a AtomicUsize, ready: bool) -> Drain<'a> {
        Drain { inner: Some(inner), ready }
    }
    /// Attempts to upgrade to a WriteGuard. If readers are still
    /// locking it, returns self so you can try again
    pub fn try_upgrade(mut self) -> Result<WriteGuard<'a>, Drain<'a>> {
        if self.ready {
            match self.inner.take() {
                Some(inner) => Ok(WriteGuard { inner }),
                _ => Err(self),
            }
        } else {
            if let Some(atomic) = &self.inner {
                if atomic.load(Ordering::SeqCst) == BIT_ON {
                    return Ok(WriteGuard::new(atomic));
                }
            }
            Err(self)
        }
    }
}

impl<'a> Drop for Drain<'a> {
    fn drop(&mut self) {
        if let Some(atomic) = self.inner.take() {
            atomic.fetch_and(BIT_ON, Ordering::SeqCst);
        }
    }
}

/// This guard signifies read access. When it drops, it will release the read lock.
pub struct ReadGuard<'a> {
    pub(crate) inner: Option<&'a AtomicUsize>,
}

impl<'a> ReadGuard<'a> {
    pub(crate) fn new(inner: &'a AtomicUsize) -> ReadGuard<'a> {
        ReadGuard { inner: Some(inner) }
    }
}

impl<'a> Drop for ReadGuard<'a> {
    fn drop(&mut self) {
        if let Some(atomic) = self.inner.take() {
            loop {
                let current = atomic.load(Ordering::SeqCst);
                let masked = current & BIT_OFF;
                if masked > 0 {
                    let new = (masked - 1) | (current & BIT_ON);
                    if atomic.compare_exchange_weak(current, new, Ordering::SeqCst, Ordering::SeqCst).is_ok() { return }
                } else {
                    panic!("How did you get a zero reader count?");
                }
            }
        }
    }
}

/// This guard signifies write access. When it drops, it will release the write lock.
pub struct WriteGuard<'a> {
    pub(crate) inner: &'a AtomicUsize,
}

impl<'a> WriteGuard<'a> {
    fn new(inner: &'a AtomicUsize) -> WriteGuard<'a> {
        WriteGuard { inner }
    }
}

impl<'a> Drop for WriteGuard<'a> {
    fn drop(&mut self) {
        self.inner.fetch_and(BIT_ON, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod test {
    #[test]
    // In which we verify we know how to steal a bit by abusing our
    // knowledge of how integers are represented.
    fn bit_hax() {
        assert_eq!(i8::MIN as u8, 1 << 7);
        assert_eq!(i8::MAX as u8, !(1 << 7));
        assert_eq!(i16::MIN as u16, 1 << 15);
        assert_eq!(i16::MAX as u16, !(1 << 15));
        assert_eq!(i32::MIN as u32, 1 << 31);
        assert_eq!(i32::MAX as u32, !(1 << 31));
        assert_eq!(i64::MIN as u64, 1 << 63);
        assert_eq!(i64::MAX as u64, !(1 << 63));
    }

    #[test]
    #[cfg(target_pointer_size="32")]
    fn constants() {
        assert_eq!(BIT_ON.leading_ones(), 1);
        assert_eq!(BIT_ON.trailing_zeroes(), 31);
        assert_eq!(BIT_OFF.leading_zeroes(), 1);
        assert_eq!(BIT_OFF.trailing_ones(), 31);
    }

    #[test]
    #[cfg(target_pointer_size="64")]
    fn constants() {
        assert_eq!(BIT_ON.leading_ones(), 1);
        assert_eq!(BIT_ON.trailing_zeroes(), 63);
        assert_eq!(BIT_OFF.leading_zeroes(), 1);
        assert_eq!(BIT_OFF.trailing_ones(), 63);
    }

}
