use super::{POLYGON_DEFAULT_SIDES, STAR_DEFAULT_POINTS, step_sides};
use crate::geometry::{MAX_SIDES, MIN_SIDES};

/// One press is one side, in the direction the key points.
#[test]
fn each_press_is_one_side() {
    assert_eq!(step_sides(5, true, false), 6);
    assert_eq!(step_sides(5, false, true), 4);
    assert_eq!(step_sides(5, false, false), 5);
    // Both at once cancel.
    assert_eq!(step_sides(5, true, true), 5);
}

/// `↓` at the floor stays at the floor; a naive `u32` `sides - 1` would wrap.
#[test]
fn the_floor_holds_instead_of_wrapping() {
    assert_eq!(step_sides(MIN_SIDES, false, true), MIN_SIDES);
    assert_eq!(step_sides(POLYGON_DEFAULT_SIDES, false, true), MIN_SIDES);
    assert_eq!(step_sides(MIN_SIDES + 1, false, true), MIN_SIDES);
    // Zero is not reachable by the gesture but is what an uninitialised field holds.
    assert_eq!(step_sides(0, false, true), MIN_SIDES);
}

/// `↑` stops at the ceiling.
#[test]
fn the_ceiling_holds_too() {
    assert_eq!(step_sides(MAX_SIDES, true, false), MAX_SIDES);
    assert_eq!(step_sides(MAX_SIDES - 1, true, false), MAX_SIDES);
    assert_eq!(step_sides(MAX_SIDES + 100, true, false), MAX_SIDES);
}

/// Held down, both directions converge on their bound and stay there.
#[test]
fn holding_a_key_settles_at_the_bound() {
    let mut up = STAR_DEFAULT_POINTS;
    let mut down = STAR_DEFAULT_POINTS;
    for _ in 0..(MAX_SIDES * 2) {
        up = step_sides(up, true, false);
        down = step_sides(down, false, true);
    }
    assert_eq!(up, MAX_SIDES);
    assert_eq!(down, MIN_SIDES);
}
