//! Shared LIGHTSYNC view helpers: the effect-selector model both frontends
//! render (built-in sweeps, the five CUSTOM slots, the unlabeled effects
//! 6-9 as one flat list) and the mirrored-pair rule for the inside-out /
//! outside-in directions.
//!
//! The sysfs surface stays raw (`wheel_led_effect` 1-9 plus the slot
//! attrs); everything here is presentation-side mapping so the GUI and the
//! TUI cannot drift apart on how a selection translates to device writes.

/// Labels for `wheel_led_effect` values 1..=4, in value order: value `n`
/// is `EFFECT_LABELS[n - 1]`.
pub const EFFECT_LABELS: [&str; 4] = ["Inside out", "Outside in", "Right to left", "Left to right"];

/// How many CUSTOM slots the wheel stores (`wheel_led_slot` 0..=4).
pub const CUSTOM_SLOTS: usize = 5;

/// How many entries the effect selector shows: the 4 labeled sweeps, the
/// 5 CUSTOM slots, and the 4 unlabeled effects 6-9.
pub const SELECTION_COUNT: usize = 13;

/// One effect-selector choice, resolved back to what the device needs
/// written: a plain `wheel_led_effect` value, or a CUSTOM slot (which
/// writes `wheel_led_slot` first and then `wheel_led_effect = 5`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    /// A non-custom `wheel_led_effect` value (1-4 or 6-9).
    Effect(u8),
    /// A CUSTOM slot (`wheel_led_slot` value, 0-based 0..=4).
    Custom(u8),
}

/// The selector index showing the device state `effect` (+ `slot` when
/// `effect` is 5, the custom mode). Selector order: effects 1..=4, then
/// CUSTOM slots 0..=4, then effects 6..=9. Out-of-range input clamps to
/// the nearest valid entry rather than panicking (a device could report
/// anything).
pub fn selection_index(effect: u8, slot: u8) -> usize {
    match effect {
        0..=4 => usize::from(effect.max(1)) - 1,
        5 => 4 + usize::from(slot.min(CUSTOM_SLOTS as u8 - 1)),
        6..=9 => usize::from(effect) + 3,
        _ => SELECTION_COUNT - 1,
    }
}

/// The inverse of `selection_index`: what picking selector entry `idx`
/// means. Past-the-end input clamps to the last entry.
pub fn index_selection(idx: usize) -> Selection {
    match idx {
        0..=3 => Selection::Effect(idx as u8 + 1),
        4..=8 => Selection::Custom(idx as u8 - 4),
        9..=12 => Selection::Effect(idx as u8 - 3),
        _ => Selection::Effect(9),
    }
}

/// The 13 selector labels, in `selection_index` order. CUSTOM entries show
/// "CUSTOM N: <name>" using `slot_names[N - 1]` (trimmed), or a plain
/// "CUSTOM N" when that entry is empty or missing; a short (or empty)
/// names list still yields all 13 labels.
pub fn dropdown_labels(slot_names: &[String]) -> Vec<String> {
    let mut labels: Vec<String> = EFFECT_LABELS.iter().map(|s| s.to_string()).collect();
    for n in 1..=CUSTOM_SLOTS {
        let name = slot_names.get(n - 1).map(|s| s.trim()).filter(|s| !s.is_empty());
        labels.push(match name {
            Some(name) => format!("CUSTOM {n}: {name}"),
            None => format!("CUSTOM {n}"),
        });
    }
    for e in 6..=9 {
        labels.push(format!("Effect {e}"));
    }
    labels
}

/// Whether `direction` (a `wheel_led_direction` enum value: 0 = left to
/// right, 1 = right to left, 2 = inside-out, 3 = outside-in) collapses the
/// 10 LEDs into 5 mirrored pairs (1-10, 2-9, 3-8, 4-7, 5-6).
pub fn mirrored(direction: u8) -> bool {
    direction == 2 || direction == 3
}

/// The swatch paired with index `i` on the 10-LED strip when a mirrored
/// direction is active.
pub fn mirror_index(i: usize) -> usize {
    9 - i
}

/// Mirror the left half of `items` onto the right half (left half wins),
/// making the list match what a mirrored direction plays. Works for any
/// even or odd length; the middle element of an odd-length list stays.
pub fn mirror_left_half<T: Clone>(items: &mut [T]) {
    let n = items.len();
    for i in 0..n / 2 {
        items[n - 1 - i] = items[i].clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_labels_cover_values_1_to_4_in_order() {
        assert_eq!(EFFECT_LABELS[0], "Inside out"); // wheel_led_effect 1
        assert_eq!(EFFECT_LABELS[1], "Outside in"); // 2
        assert_eq!(EFFECT_LABELS[2], "Right to left"); // 3
        assert_eq!(EFFECT_LABELS[3], "Left to right"); // 4
    }

    #[test]
    fn selection_index_orders_sweeps_customs_then_numbered() {
        assert_eq!(selection_index(1, 0), 0);
        assert_eq!(selection_index(4, 0), 3);
        assert_eq!(selection_index(5, 0), 4);
        assert_eq!(selection_index(5, 4), 8);
        assert_eq!(selection_index(6, 0), 9);
        assert_eq!(selection_index(9, 0), 12);
    }

    #[test]
    fn selection_index_clamps_out_of_range_device_values() {
        assert_eq!(selection_index(0, 0), 0, "effect 0 clamps to the first entry");
        assert_eq!(selection_index(200, 0), 12, "past-the-end effect clamps to the last");
        assert_eq!(selection_index(5, 200), 8, "past-the-end slot clamps to CUSTOM 5");
    }

    #[test]
    fn every_index_round_trips_through_selection() {
        for idx in 0..SELECTION_COUNT {
            let back = match index_selection(idx) {
                Selection::Effect(e) => selection_index(e, 0),
                Selection::Custom(s) => selection_index(5, s),
            };
            assert_eq!(back, idx);
        }
    }

    #[test]
    fn every_selection_round_trips_through_index() {
        for e in [1u8, 2, 3, 4, 6, 7, 8, 9] {
            assert_eq!(index_selection(selection_index(e, 0)), Selection::Effect(e));
        }
        for s in 0..CUSTOM_SLOTS as u8 {
            assert_eq!(index_selection(selection_index(5, s)), Selection::Custom(s));
        }
    }

    #[test]
    fn index_selection_clamps_past_the_end() {
        assert_eq!(index_selection(999), Selection::Effect(9));
    }

    #[test]
    fn dropdown_labels_has_13_entries_in_selector_order() {
        let labels = dropdown_labels(&[]);
        assert_eq!(labels.len(), SELECTION_COUNT);
        assert_eq!(labels[0], "Inside out");
        assert_eq!(labels[3], "Left to right");
        assert_eq!(labels[4], "CUSTOM 1");
        assert_eq!(labels[8], "CUSTOM 5");
        assert_eq!(labels[9], "Effect 6");
        assert_eq!(labels[12], "Effect 9");
    }

    #[test]
    fn dropdown_labels_show_trimmed_slot_names() {
        let names = vec![
            "RACE".to_string(),
            "  GT7  ".to_string(),
            String::new(),
            "   ".to_string(),
        ];
        let labels = dropdown_labels(&names);
        assert_eq!(labels[4], "CUSTOM 1: RACE");
        assert_eq!(labels[5], "CUSTOM 2: GT7", "names are trimmed");
        assert_eq!(labels[6], "CUSTOM 3", "empty name falls back to the plain label");
        assert_eq!(labels[7], "CUSTOM 4", "whitespace-only name counts as empty");
        assert_eq!(labels[8], "CUSTOM 5", "missing entry counts as empty");
    }

    #[test]
    fn mirrored_only_for_inside_out_and_outside_in() {
        assert!(!mirrored(0), "left to right is not mirrored");
        assert!(!mirrored(1), "right to left is not mirrored");
        assert!(mirrored(2), "inside-out mirrors");
        assert!(mirrored(3), "outside-in mirrors");
        assert!(!mirrored(4), "unknown directions do not mirror");
    }

    #[test]
    fn mirror_index_pairs_the_strip_ends() {
        assert_eq!(mirror_index(0), 9);
        assert_eq!(mirror_index(9), 0);
        assert_eq!(mirror_index(4), 5);
        assert_eq!(mirror_index(5), 4);
        for i in 0..10 {
            assert_eq!(mirror_index(mirror_index(i)), i);
        }
    }

    #[test]
    fn mirror_left_half_copies_left_onto_right() {
        let mut v = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        mirror_left_half(&mut v);
        assert_eq!(v, vec![1, 2, 3, 4, 5, 5, 4, 3, 2, 1]);
    }

    #[test]
    fn mirror_left_half_keeps_the_middle_of_an_odd_list() {
        let mut v = vec![1, 2, 3, 4, 5];
        mirror_left_half(&mut v);
        assert_eq!(v, vec![1, 2, 3, 2, 1]);
    }
}
