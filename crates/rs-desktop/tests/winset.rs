use rs_desktop::winset::{OpenDecision, WindowSet};

#[test]
fn one_window_per_host() {
    let mut set = WindowSet::new();
    assert_eq!(set.insert("h1", 7, "one"), Ok(()));
    assert_eq!(set.decide("h1"), OpenDecision::FocusExisting(7));
    assert_eq!(set.decide("h2"), OpenDecision::OpenNew);
    assert_eq!(set.insert("h1", 8, "two"), Err(7));
    assert_eq!(set.get_by_session(7), Some("one"));
    assert_eq!(set.get_by_session(8), None);
    assert_eq!(set.len(), 1);
}

#[test]
fn remove() {
    let mut set = WindowSet::new();
    set.insert("h1", 7, "one").unwrap();
    assert_eq!(set.remove_session(7), Some("one"));
    assert_eq!(set.decide("h1"), OpenDecision::OpenNew);
    assert_eq!(set.remove_session(7), None);
    assert_eq!(set.remove_session(99), None);
    assert!(set.is_empty());
    assert!(set.sessions().is_empty());
    assert_eq!(set.get_by_session(7), None);
    assert_eq!(set.session_for_host("h1"), None);
    assert_eq!(set.insert("h1", 9, "new"), Ok(()));
    assert_eq!(set.session_for_host("h1"), Some(9));
}

#[test]
fn focus_tracking() {
    let mut set = WindowSet::new();
    set.insert("h1", 7, "one").unwrap();
    set.insert("h2", 8, "two").unwrap();
    set.set_focused(7, true);
    assert_eq!(set.focused(), Some((7, "one")));
    set.set_focused(8, false);
    assert_eq!(set.focused(), Some((7, "one")));
    set.set_focused(99, true);
    assert_eq!(set.focused(), Some((7, "one")));
    set.set_focused(7, false);
    assert_eq!(set.focused(), None);
    set.set_focused(7, true);
    assert_eq!(set.remove_session(7), Some("one"));
    assert_eq!(set.focused(), None);
    assert_eq!(set.decide("h1"), OpenDecision::OpenNew);
    assert_eq!(set.remove_session(99), None);
    assert_eq!(set.len(), 1);
    assert!(!set.is_empty());
    assert_eq!(set.sessions(), vec![8]);
    assert_eq!(set.get_by_session(8), Some("two"));
    assert_eq!(set.session_for_host("h2"), Some(8));
    set.set_focused(8, true);
    assert_eq!(set.focused(), Some((8, "two")));
    set.remove_session(8);
    assert_eq!(set.focused(), None);
    assert!(set.is_empty());
}
