//! most-recent tab list and the Ctrl hold walk (Task 8).
//!
//! Source: Prism Terminal's `tabMru.ts`. The walk is a SNAPSHOT of the list taken at the first
//! Tab of a Ctrl hold; the list itself is reordered only when the hold ends (Ctrl released), so
//! a long hold can reach an older tab instead of ping-ponging between the two front ones.

/// One Ctrl hold: the list as it was at its first press, and where the walk is in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Walk {
    pub list: Vec<String>,
    pub at: usize,
}

/// The tab came to the front: `[id, ...the rest]`. The same list back when it already was.
pub fn touch_mru(mru: &[String], id: &str) -> Vec<String> {
    if mru.first().map(String::as_str) == Some(id) {
        return mru.to_vec();
    }
    let mut out = Vec::with_capacity(mru.len() + 1);
    out.push(id.to_string());
    out.extend(mru.iter().filter(|t| t.as_str() != id).cloned());
    out
}

/// Follow the strip: closed tabs leave, tabs the list has not seen join after the known ones in
/// strip order. An empty list (a launch, or a restore that settled every placeholder under a new
/// id) starts from the tab in front, then the strip.
pub fn sync_mru(mru: &[String], tab_ids: &[String], active_id: Option<&str>) -> Vec<String> {
    let kept: Vec<String> = mru
        .iter()
        .filter(|id| tab_ids.contains(id))
        .cloned()
        .collect();
    if kept.is_empty() {
        return match active_id.filter(|a| tab_ids.iter().any(|t| t == a)) {
            Some(active) => {
                let mut out = vec![active.to_string()];
                out.extend(tab_ids.iter().filter(|t| t.as_str() != active).cloned());
                out
            }
            None => tab_ids.to_vec(),
        };
    }
    let added: Vec<String> = tab_ids
        .iter()
        .filter(|id| !kept.contains(id))
        .cloned()
        .collect();
    let mut out = kept;
    out.extend(added);
    out
}

/// A walk needs somewhere to go: with fewer than two tabs there is none.
pub fn start_walk(mru: &[String]) -> Option<Walk> {
    (mru.len() >= 2).then(|| Walk {
        list: mru.to_vec(),
        at: 0,
    })
}

/// Tab goes one further back (`dir` 1), Shift+Tab the other way (`dir` -1). Wraps both ways, so
/// a fresh Shift+Tab lands on the oldest tab.
pub fn step_walk(walk: &Walk, dir: i32) -> Walk {
    let n = walk.list.len() as i64;
    let at = (walk.at as i64 + i64::from(dir)).rem_euclid(n) as usize;
    Walk {
        list: walk.list.clone(),
        at,
    }
}

/// The tab id the walk currently lands on.
pub fn walk_target(walk: &Walk) -> &str {
    &walk.list[walk.at]
}

/// Whether a hold still walks the strip as it is: the tab in front is the one the walk landed on
/// and the strip holds exactly the snapshot's tabs. A click on another tab mid-hold is a use like
/// any other; a tab opened or closed without Ctrl ends the walk too.
pub fn walk_holds(walk: &Walk, tab_ids: &[String], active_id: Option<&str>) -> bool {
    if active_id != Some(walk_target(walk)) || tab_ids.len() != walk.list.len() {
        return false;
    }
    walk.list.iter().all(|id| tab_ids.contains(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// A hold of Ctrl: the walk started from `mru`, then each Tab (1) or Shift+Tab (-1) pressed
    /// while it is held. Returns the tab landed on.
    fn hold(mru: &[&str], presses: &[i32]) -> Option<String> {
        hold_list(&v(mru), presses)
    }

    fn hold_list(mru: &[String], presses: &[i32]) -> Option<String> {
        let mut walk = start_walk(mru)?;
        for &dir in presses {
            walk = step_walk(&walk, dir);
        }
        Some(walk_target(&walk).to_string())
    }

    #[test]
    fn touch_puts_the_tab_in_front_once() {
        assert_eq!(touch_mru(&v(&["a", "b", "c"]), "c"), v(&["c", "a", "b"]));
        assert_eq!(touch_mru(&v(&["a", "b"]), "a"), v(&["a", "b"]));
        assert_eq!(touch_mru(&[], "a"), v(&["a"]));
    }

    #[test]
    fn touching_the_front_tab_changes_nothing() {
        assert_eq!(touch_mru(&v(&["a", "b"]), "a"), v(&["a", "b"]));
    }

    #[test]
    fn sync_drops_closed_tabs() {
        assert_eq!(
            sync_mru(&v(&["c", "a", "b"]), &v(&["a", "b"]), Some("a")),
            v(&["a", "b"])
        );
    }

    #[test]
    fn sync_adds_new_tabs_after_the_known_ones() {
        assert_eq!(
            sync_mru(&v(&["b", "a"]), &v(&["a", "b", "x", "y"]), Some("b")),
            v(&["b", "a", "x", "y"])
        );
    }

    #[test]
    fn sync_starts_an_empty_list_from_the_front() {
        assert_eq!(
            sync_mru(&[], &v(&["a", "b", "c"]), Some("b")),
            v(&["b", "a", "c"])
        );
        assert_eq!(
            sync_mru(&v(&["gone"]), &v(&["a", "b", "c"]), Some("c")),
            v(&["c", "a", "b"])
        );
        assert_eq!(sync_mru(&[], &[], None), Vec::<String>::new());
    }

    #[test]
    fn one_press_goes_to_the_tab_used_before() {
        assert_eq!(hold(&["c", "b", "a"], &[1]).as_deref(), Some("b"));
    }

    #[test]
    fn further_presses_walk_back_and_wrap() {
        assert_eq!(hold(&["c", "b", "a"], &[1, 1]).as_deref(), Some("a"));
        assert_eq!(hold(&["c", "b", "a"], &[1, 1, 1]).as_deref(), Some("c"));
    }

    #[test]
    fn shift_walks_the_other_way_from_the_far_end() {
        assert_eq!(hold(&["c", "b", "a"], &[-1]).as_deref(), Some("a"));
        assert_eq!(hold(&["c", "b", "a"], &[-1, -1]).as_deref(), Some("b"));
        assert_eq!(hold(&["c", "b", "a"], &[1, 1, -1]).as_deref(), Some("b"));
    }

    #[test]
    fn no_ping_pong_the_order_changes_when_the_hold_ends() {
        // Visit a, b, c by hand; then one Ctrl+Tab per hold.
        let mut list: Vec<String> = Vec::new();
        for id in ["a", "b", "c"] {
            list = touch_mru(&list, id);
        }
        let first = hold_list(&list, &[1]).unwrap();
        assert_eq!(first, "b");
        list = touch_mru(&list, &first); // Ctrl released on b
        let second = hold_list(&list, &[1]).unwrap();
        assert_eq!(second, "c"); // a flip back, the two last used
        list = touch_mru(&list, &second);
        // A long hold reaches the oldest, and releasing there brings it to the front.
        let third = hold_list(&list, &[1, 1]).unwrap();
        assert_eq!(third, "a");
        list = touch_mru(&list, &third);
        assert_eq!(list, v(&["a", "c", "b"]));
    }

    #[test]
    fn the_walk_is_a_snapshot() {
        let list = v(&["c", "b", "a"]);
        let walk = step_walk(&start_walk(&list).unwrap(), 1);
        assert_eq!(list, v(&["c", "b", "a"]));
        assert_eq!(walk_target(&walk), "b");
    }

    #[test]
    fn the_walk_holds_only_for_its_own_front_and_strip() {
        let walk = step_walk(&start_walk(&v(&["d", "c", "b", "a"])).unwrap(), 1); // landed on c
        assert!(walk_holds(&walk, &v(&["a", "b", "c", "d"]), Some("c")));
        // A click on another tab mid-hold is a use, not a step: the hold ends.
        assert!(!walk_holds(&walk, &v(&["a", "b", "c", "d"]), Some("a")));
        // A tab opened without Ctrl (a folder from Explorer) is not in the snapshot.
        assert!(!walk_holds(
            &walk,
            &v(&["a", "b", "c", "d", "x"]),
            Some("x")
        ));
        assert!(!walk_holds(
            &walk,
            &v(&["a", "b", "c", "d", "x"]),
            Some("c")
        ));
        // A closed tab could be its next stop.
        assert!(!walk_holds(&walk, &v(&["b", "c", "d"]), Some("c")));
    }

    #[test]
    fn nothing_to_walk_over_one_tab_or_none() {
        assert_eq!(start_walk(&v(&["a"])), None);
        assert_eq!(start_walk(&[]), None);
    }
}
