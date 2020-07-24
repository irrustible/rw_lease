# rw_lease

[![License](https://img.shields.io/crates/l/rw_lease.svg)](https://github.com/irrustible/rw_lease/blob/main/LICENSE)
[![Package](https://img.shields.io/crates/v/rw_lease.svg)](https://crates.io/crates/rw_lease)
[![Documentation](https://docs.rs/rw_lease/badge.svg)](https://docs.rs/rw_lease)

Fast Reader-Writer lock with reader draining support. Based on a
single (parameterisable) atomic usize.

Notes:

* Steals the high bit for the writer lock.
* Designed for low contention, mostly-read workloads.
* Write locking requires waiting for readers to drop their locks.

## Benchmarks

There are benchmarks, which you can and should run. Here are some
numbers from my 2015 macbook pro on an AtomicUsize (the default):

| Benchmark         | Mutex        | RwLock         | RWLease        |
|-------------------|--------------|----------------|----------------|
| Create            | 110          | 107            | 1              |
| Uncontended Reads | 330731 (1.4) | 417664 (1.77)  | 235656 (1)     |
| Contended Reads   | 1140321 (1)  | 2367186 (2.08) | 1488557 (1.31) |

Notes: measurements in nanoseconds, parens = normalised to shortest run.

We haven't spent terribly long optimising the code, there may be some
wins left to gain.

## Copyright and License

Copyright (c) 2020 James Laver, rw_lease contributors.

This Source Code Form is subject to the terms of the Mozilla Public
License, v. 2.0. If a copy of the MPL was not distributed with this
file, You can obtain one at http://mozilla.org/MPL/2.0/.
