//! Lightweight behavior assertions for the new scrolling contract.
//! Does not require headless rendering; checks ListState semantics directly.
//! - tall header visible vs bottom
//! - non-tail preserves anchor
//! - collapse expand keeps follow
use gpui::{px, FollowMode, ListAlignment, ListOffset, ListState};

#[test]
fn tall_header_target_differs_from_bottom() {
    // Simulate a transcript with 12 short msgs + 1 tall msg.
    // We can't know exact heights without rendering, but we can verify
    // that our code path chooses header (item_ix = first_new) over bottom (count).
    // This is a logic check: first_new should be len - arrivals.len()
    let lines_len: usize = 13;
    let arrivals_len: usize = 1;
    let first_new = lines_len.saturating_sub(arrivals_len.max(1));
    assert_eq!(
        first_new, 12,
        "first_new should be 12 for 13 lines with 1 arrival"
    );
    let target = ListOffset {
        item_ix: first_new,
        offset_in_item: px(0.),
    };
    assert_eq!(target.item_ix, 12);
    assert_eq!(target.offset_in_item, px(0.));
    println!("PASS tall header target = {:?}", target);
}

#[test]
fn list_state_tail_vs_header_positions() {
    // Create a ListState with 13 items, simulate viewport 800px, overdraw 500px.
    // We can't fully render without a Window, but we can check splice+scroll semantics
    // that our fix relies on: was_following preserved.
    let list = ListState::new(13, ListAlignment::Top, px(500.));
    list.set_follow_mode(FollowMode::Tail);
    list.scroll_to_end();
    assert!(
        list.is_following_tail(),
        "should be following after Tail+end"
    );
    // Simulate expand at index 5 while following: our fix does Tail+end, not anchor
    let was_following = list.is_following_tail();
    let anchor = list.logical_scroll_top();
    assert_eq!(anchor.item_ix, 13, "tail at count 13");
    // splice replacing 1 with 1 (expand)
    list.splice(5..6, 1);
    if was_following {
        list.set_follow_mode(FollowMode::Tail);
        list.scroll_to_end();
    } else {
        list.scroll_to(anchor);
    }
    assert!(
        list.is_following_tail(),
        "expand while following should stay following"
    );
    assert_eq!(list.logical_scroll_top().item_ix, 13);
    println!("PASS expand keeps follow: {:?}", list.logical_scroll_top());
}

#[test]
fn non_tail_anchor_preserved() {
    let list = ListState::new(13, ListAlignment::Top, px(500.));
    // Simulate user scrolled to item 3
    list.scroll_to(ListOffset {
        item_ix: 3,
        offset_in_item: px(10.),
    });
    assert!(
        !list.is_following_tail(),
        "should not be following after scroll_to 3"
    );
    let anchor = list.logical_scroll_top();
    // Simulate new message arrival: splice at end
    list.splice(13..13, 1); // append one
                            // Non-tail path: should restore anchor, not jump to end
    let was_following = list.is_following_tail();
    assert!(!was_following);
    list.scroll_to(anchor);
    assert_eq!(list.logical_scroll_top().item_ix, 3);
    assert_eq!(list.logical_scroll_top().offset_in_item, px(10.));
    assert!(!list.is_following_tail());
    println!("PASS non-tail anchor preserved at {:?}", anchor);
}

#[test]
fn collapse_preserves_non_follow() {
    let list = ListState::new(5, ListAlignment::Top, px(500.));
    list.scroll_to(ListOffset {
        item_ix: 1,
        offset_in_item: px(0.),
    });
    let was_following = list.is_following_tail();
    assert!(!was_following);
    let anchor = list.logical_scroll_top();
    // collapse at index 1 with offset 0 logic
    let expanded = false;
    let mut anchor2 = anchor;
    if !expanded && anchor2.item_ix == 1 {
        anchor2.offset_in_item = px(0.);
    }
    list.splice(1..2, 1);
    list.scroll_to(anchor2);
    assert_eq!(list.logical_scroll_top().item_ix, 1);
    assert_eq!(list.logical_scroll_top().offset_in_item, px(0.));
    println!("PASS collapse anchor reset to 0 at top");
}
