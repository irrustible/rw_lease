#![feature(test)]

extern crate test;

use std::sync::Arc;
use rw_lease::{Blocked, RWLease};
use std::thread;

use test::Bencher;

#[bench]
fn create(b: &mut Bencher) {
    b.iter(|| {
        let lease: RWLease<()> = RWLease::new(());
        test::black_box(lease);
    })
}

#[bench]
fn contention_reads(b: &mut Bencher) {
    b.iter(|| run(10, 1000));
}

#[bench]
fn no_contention_reads(b: &mut Bencher) {
    b.iter(|| run(1, 10000));
}

fn run(num_threads: usize, iter: usize) {
    let m = Arc::new(RWLease::new(0i32));
    let mut threads = Vec::new();

    for _ in 0..num_threads {
        let m = m.clone();
        threads.push(thread::spawn(move || {
            for _ in 0..iter {
                loop {
                    match m.read() {
                        Ok(r) => { test::black_box(*r); break; },
                        Err(Blocked::LostRace) => (),
                        Err(_) => unreachable!(),
                    }
                }
            }
        }));
    }

    for t in threads {
        t.join().unwrap();
    }
}
