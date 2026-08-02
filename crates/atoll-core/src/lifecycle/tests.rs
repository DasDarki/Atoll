use super::*;
use LifecycleCommand::{Hide, Show, Unload, Wake};

const A: TabId = TabId(1);
const B: TabId = TabId(2);
const C: TabId = TabId(3);
const D: TabId = TabId(4);
const W: WindowId = WindowId(1);
const V: WindowId = WindowId(2);

#[test]
fn every_tab_starts_sleeping_or_archived() {
    let lifecycle = TabLifecycle::new(4);
    assert_eq!(lifecycle.state_of(A, false), TabState::Sleeping);
    assert_eq!(lifecycle.state_of(A, true), TabState::Archived);
}

#[test]
fn activating_a_sleeping_tab_wakes_and_shows_it() {
    let mut lifecycle = TabLifecycle::new(4);
    assert_eq!(lifecycle.activate(W, A), vec![Wake(A), Show(A)]);
    assert_eq!(lifecycle.state_of(A, false), TabState::Live);
}

#[test]
fn switching_tabs_freezes_the_previous_one() {
    let mut lifecycle = TabLifecycle::new(4);
    lifecycle.activate(W, A);
    assert_eq!(lifecycle.activate(W, B), vec![Hide(A), Wake(B), Show(B)]);
    assert_eq!(lifecycle.state_of(A, false), TabState::Frozen);
    assert_eq!(lifecycle.state_of(B, false), TabState::Live);
}

#[test]
fn returning_to_a_frozen_tab_does_not_reload_it() {
    let mut lifecycle = TabLifecycle::new(4);
    lifecycle.activate(W, A);
    lifecycle.activate(W, B);
    assert_eq!(lifecycle.activate(W, A), vec![Hide(B), Show(A)]);
    assert_eq!(lifecycle.frozen(), &[B]);
}

#[test]
fn activating_the_live_tab_is_a_no_op() {
    let mut lifecycle = TabLifecycle::new(4);
    lifecycle.activate(W, A);
    assert!(lifecycle.activate(W, A).is_empty());
}

#[test]
fn frozen_limit_unloads_the_least_recently_used_tab() {
    let mut lifecycle = TabLifecycle::new(2);
    lifecycle.activate(W, A);
    lifecycle.activate(W, B);
    lifecycle.activate(W, C);
    assert_eq!(
        lifecycle.activate(W, D),
        vec![Hide(C), Wake(D), Show(D), Unload(A)]
    );
    assert_eq!(lifecycle.frozen(), &[C, B]);
    assert_eq!(lifecycle.state_of(A, false), TabState::Sleeping);
}

#[test]
fn protected_tabs_survive_the_frozen_limit() {
    let mut lifecycle = TabLifecycle::new(1);
    lifecycle.set_protected(A, true);
    lifecycle.activate(W, A);
    lifecycle.activate(W, B);
    assert_eq!(lifecycle.activate(W, C), vec![Hide(B), Wake(C), Show(C)]);
    assert_eq!(lifecycle.frozen(), &[B, A]);
    assert_eq!(
        lifecycle.activate(W, D),
        vec![Hide(C), Wake(D), Show(D), Unload(B)]
    );
    assert_eq!(lifecycle.frozen(), &[C, A]);
}

#[test]
fn unprotecting_a_tab_enforces_the_limit_again() {
    let mut lifecycle = TabLifecycle::new(1);
    lifecycle.set_protected(A, true);
    lifecycle.activate(W, A);
    lifecycle.activate(W, B);
    lifecycle.activate(W, C);
    assert_eq!(lifecycle.set_protected(A, false), vec![Unload(A)]);
}

#[test]
fn sleeping_the_live_tab_leaves_no_live_tab() {
    let mut lifecycle = TabLifecycle::new(4);
    lifecycle.activate(W, A);
    assert_eq!(lifecycle.sleep(A), vec![Unload(A)]);
    assert_eq!(lifecycle.live_in(W), None);
    assert!(lifecycle.sleep(A).is_empty());
}

#[test]
fn deactivate_freezes_without_new_live_tab() {
    let mut lifecycle = TabLifecycle::new(4);
    lifecycle.activate(W, A);
    assert_eq!(lifecycle.deactivate(W), vec![Hide(A)]);
    assert_eq!(lifecycle.live_in(W), None);
    assert_eq!(lifecycle.state_of(A, false), TabState::Frozen);
}

#[test]
fn demote_skips_protected_tabs_and_never_touches_the_live_tab() {
    let mut lifecycle = TabLifecycle::new(8);
    lifecycle.activate(W, A);
    lifecycle.activate(W, B);
    lifecycle.activate(W, C);
    lifecycle.set_protected(A, true);
    assert_eq!(lifecycle.demote_least_recent(), Some(Unload(B)));
    assert_eq!(lifecycle.demote_least_recent(), None);
    assert_eq!(lifecycle.live_in(W), Some(C));
}

#[test]
fn forget_drops_protection() {
    let mut lifecycle = TabLifecycle::new(4);
    lifecycle.set_protected(A, true);
    lifecycle.activate(W, A);
    assert_eq!(lifecycle.forget(A), vec![Unload(A)]);
    assert!(!lifecycle.is_protected(A));
}

#[test]
fn every_window_has_its_own_live_tab() {
    let mut lifecycle = TabLifecycle::new(4);
    lifecycle.activate(W, A);
    assert_eq!(lifecycle.activate(V, B), vec![Wake(B), Show(B)]);
    assert_eq!(lifecycle.state_of(A, false), TabState::Live);
    assert_eq!(lifecycle.state_of(B, false), TabState::Live);
    assert_eq!(lifecycle.activate(V, C), vec![Hide(B), Wake(C), Show(C)]);
    assert_eq!(lifecycle.live_in(W), Some(A));
    assert_eq!(lifecycle.live_in(V), Some(C));
}

#[test]
fn a_live_tab_moves_to_another_window_without_reloading() {
    let mut lifecycle = TabLifecycle::new(4);
    lifecycle.activate(W, A);
    assert_eq!(lifecycle.activate(V, A), vec![Show(A)]);
    assert_eq!(lifecycle.live_in(W), None);
    assert_eq!(lifecycle.live_in(V), Some(A));
}

#[test]
fn sleeping_a_live_tab_clears_its_window() {
    let mut lifecycle = TabLifecycle::new(4);
    lifecycle.activate(W, A);
    lifecycle.activate(V, B);
    assert_eq!(lifecycle.sleep(B), vec![Unload(B)]);
    assert_eq!(lifecycle.live_in(V), None);
    assert_eq!(lifecycle.live_in(W), Some(A));
}

#[test]
fn busy_tabs_survive_the_frozen_limit_until_they_are_idle() {
    let mut lifecycle = TabLifecycle::new(1);
    lifecycle.activate(W, A);
    lifecycle.set_busy(A, true);
    lifecycle.activate(W, B);
    assert_eq!(lifecycle.activate(W, C), vec![Hide(B), Wake(C), Show(C)]);
    assert_eq!(lifecycle.frozen(), &[B, A]);
    assert!(lifecycle.is_busy(A));
    assert_eq!(lifecycle.set_busy(A, false), vec![Unload(A)]);
    assert!(lifecycle.set_busy(A, false).is_empty());
}

#[test]
fn demote_skips_busy_tabs() {
    let mut lifecycle = TabLifecycle::new(8);
    lifecycle.activate(W, A);
    lifecycle.activate(W, B);
    lifecycle.activate(W, C);
    lifecycle.set_busy(A, true);
    assert_eq!(lifecycle.demote_least_recent(), Some(Unload(B)));
    assert_eq!(lifecycle.demote_least_recent(), None);
    assert_eq!(lifecycle.forget(A), vec![Unload(A)]);
    assert!(!lifecycle.is_busy(A));
}
