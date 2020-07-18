// use event_listener::{Event, EventListener};
// use std::future::Future;
// use std::pin::Pin;
// use std::task::{Context, Poll};
// use super::{BIT_ON, BIT_OFF};

// /// Requires the `event-listener` feature.
// struct AsyncRWLease {
//     pub(crate) atomic: AtomicUsize,
//     pub(crate) read: Event,
//     pub(crate) write: Event,
// }

// impl AsyncRWLease {
//     pub fn read<'a>(&'a self, wait_on_write: bool) -> PollReadLease<'a> {
//         PollReadLease { inner: &self.atomic, read: Event::new(), write: Event::new() }
//     }
// }

// /// Requires the `event-listener` feature.
// pub struct PollReadLease<'a> {
//     pub(crate) inner: &'a AsyncRWLease,
//     /// If it's write locked, we may want to fail because it could take a while.
//     pub(crate) wait_on_write: bool,
// }

// /// Requires the `event-listener` feature.
// pub struct PollWriteLease<'a> {
//     pub(crate) inner: &'a AsyncRWLease,
// }

// /// Requires the `event-listener` feature.
// pub struct AsyncReadLease<'a> {
//     pub(crate) inner: &'a AsyncRWLease,
// }

// /// Requires the `event-listener` feature.
// pub struct AsyncWriteLease<'a> {
//     pub(crate) inner: &'a AsyncRWLease,
// }
