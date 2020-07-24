#![feature(test)]

extern crate test;

use std::sync::{Arc, RwLock};
use std::thread;

use test::Bencher;

#[bench]
fn create(b: &mut Bencher) {
    b.iter(|| test::black_box(RwLock::new(())));
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
    let m = Arc::new(RwLock::new(0i32));
    let mut threads = Vec::new();

    for _ in 0..num_threads {
        let m = m.clone();
        threads.push(thread::spawn(move || {
            for _ in 0..iter {
                test::black_box(*m.read().unwrap());
            }
        }));
    }

    for t in threads {
        t.join().unwrap();
    }
}
