#![cfg(feature = "inventory")]

//! An LRU-evicted fixpoint memo that is re-verified in a later revision must
//! re-execute, not be mistaken for a cycle head that panicked earlier in the
//! revision (a bogus `Cancelled::PropagatedPanic`). A genuinely poisoned cycle
//! head must still propagate the panic on a subsequent fetch.

use salsa::{Database, Setter};

#[salsa::input]
struct MyInput {
    value: u32,
}

fn initial(_db: &dyn Database, _id: salsa::Id, _input: MyInput) -> u32 {
    0
}

fn recover(_db: &dyn Database, _cycle: &salsa::Cycle, _last: &u32, value: u32, _input: MyInput) -> u32 {
    value
}

#[salsa::tracked(lru = 1, cycle_fn = recover, cycle_initial = initial)]
fn head(db: &dyn Database, input: MyInput) -> u32 {
    follower(db, input).min(10)
}

#[salsa::tracked]
fn follower(db: &dyn Database, input: MyInput) -> u32 {
    head(db, input) + input.value(db)
}

#[salsa::tracked]
fn driver(db: &dyn Database, input: MyInput) -> u32 {
    head(db, input)
}

#[test]
fn evicted_cycle_head_reexecutes_after_reverification() {
    let mut db = salsa::DatabaseImpl::new();
    let k1 = MyInput::new(&db, 1);
    let k2 = MyInput::new(&db, 2);

    // R1: fixpoint-compute the cycle head for k1 (via a driver that records an
    // edge to it), then make k2 the most recent LRU entry so k1 is the victim.
    assert_eq!(driver(&db, k1), 10);
    assert_eq!(head(&db, k2), 10);

    // New revision: the LRU pass evicts head(k1)'s value, keeping its header.
    k2.set_value(&mut db).to(3);

    // Deep verification of the driver's edge stamps head(k1)'s `verified_at`
    // with the current revision without restoring the evicted value.
    assert_eq!(driver(&db, k1), 10);

    // A direct fetch then re-executes the evicted memo. This used to throw
    // `Cancelled::PropagatedPanic` for a panic that never happened.
    assert_eq!(head(&db, k1), 10);
}

fn poison_recover(
    _db: &dyn Database,
    _cycle: &salsa::Cycle,
    _last: &u32,
    _value: u32,
    _input: MyInput,
) -> u32 {
    panic!("genuine cycle recovery panic")
}

#[salsa::tracked(cycle_fn = poison_recover, cycle_initial = initial)]
fn poisoned_head(db: &dyn Database, input: MyInput) -> u32 {
    poisoned_follower(db, input)
}

#[salsa::tracked]
fn poisoned_follower(db: &dyn Database, input: MyInput) -> u32 {
    poisoned_head(db, input) + input.value(db)
}

#[test]
fn poisoned_cycle_head_still_propagates_panic() {
    let db = salsa::DatabaseImpl::new();
    let input = MyInput::new(&db, 1);

    let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| poisoned_head(&db, input)));
    let payload = first.expect_err("cycle recovery must panic");
    assert!(!payload.is::<salsa::Cancelled>(), "first panic is the user panic");

    let second = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| poisoned_head(&db, input)));
    let payload = second.expect_err("poisoned cycle head must keep panicking in the same revision");
    assert!(
        matches!(payload.downcast_ref::<salsa::Cancelled>(), Some(salsa::Cancelled::PropagatedPanic)),
        "second panic must be the propagated poison"
    );
}
