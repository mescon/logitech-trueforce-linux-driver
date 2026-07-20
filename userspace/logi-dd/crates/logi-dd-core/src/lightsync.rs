//! Shared LIGHTSYNC view helpers: the effect-selector model both frontends
//! render (the built-in sweeps and the five custom slots, matching G HUB's
//! 4 + 5 model) and the mirrored-pair rule for the inside-out / outside-in
//! directions.
//!
//! The sysfs surface stays raw (`wheel_led_effect` 1-9 plus the slot
//! attrs); everything here is presentation-side mapping so the GUI and the
//! TUI cannot drift apart on how a selection translates to device writes.
//! Effect values 5-9 ARE the five custom slots (5 = CUSTOM 1 .. 9 =
//! CUSTOM 5, hardware-confirmed 2026-07-20): a device readback of any of
//! them maps onto that slot's CUSTOM entry. Only a value outside the
//! decoded 1-9 range (nothing the driver writes, but a device could
//! report anything) gets a trailing raw "Effect N" entry, appended only
//! while the device currently reports it, so the selector never lies
//! about the current state.

/// Labels for `wheel_led_effect` values 1..=4, in value order: value `n`
/// is `EFFECT_LABELS[n - 1]`.
pub const EFFECT_LABELS: [&str; 4] = ["Inside out", "Outside in", "Right to left", "Left to right"];

/// How many custom slots the wheel stores (`wheel_led_slot` 0..=4). A slot
/// is a saved lighting preset: colors, direction, name and brightness.
pub const CUSTOM_SLOTS: usize = 5;

/// How many standing entries the effect selector shows: the 4 labeled
/// sweeps and the 5 custom slots. A raw out-of-range effect adds one
/// trailing entry at index `SELECTION_COUNT` while the device reports it
/// (see [`dropdown_labels`]).
pub const SELECTION_COUNT: usize = 9;

/// One effect-selector choice, resolved back to what the device needs
/// written: a plain `wheel_led_effect` value, or a CUSTOM slot (which
/// writes `wheel_led_slot` first and then `wheel_led_effect = 5`; the
/// driver renders effect 5 as `5 + slot` on the wire, and writing the
/// slot first keeps the slot-scoped attrs targeting the rendered slot).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    /// A non-custom `wheel_led_effect` value (a sweep 1-4, or the raw
    /// current value when the trailing out-of-range entry is re-picked).
    Effect(u8),
    /// A CUSTOM slot (`wheel_led_slot` value, 0-based 0..=4).
    Custom(u8),
}

/// The selector index showing the device state `effect` (+ `slot` when
/// `effect` is 5). Selector order: effects 1..=4, then CUSTOM slots
/// 0..=4. Effect values 5-9 are the custom slots themselves (slot
/// `effect - 5`); effect 5 additionally consults `slot`, because through
/// the driver's sysfs surface a stored 5 means "custom mode with the
/// active slot" (`wheel_led_slot`), while a raw 6-9 readback carries its
/// slot in the value itself. Any other value selects the trailing raw
/// entry (index `SELECTION_COUNT`, which [`dropdown_labels`] appends for
/// exactly that state). An out-of-range slot clamps rather than panicking
/// (a device could report anything).
pub fn selection_index(effect: u8, slot: u8) -> usize {
    match effect {
        1..=4 => usize::from(effect) - 1,
        5 => 4 + usize::from(slot.min(CUSTOM_SLOTS as u8 - 1)),
        6..=9 => 4 + usize::from(effect - 5),
        _ => SELECTION_COUNT,
    }
}

/// The inverse of `selection_index`: what picking selector entry `idx`
/// means. `current_effect` is the device's current `wheel_led_effect`
/// value: picking the trailing raw entry (or anything past the standing
/// list) re-selects that current value, since the trailing entry only
/// exists to display it.
pub fn index_selection(idx: usize, current_effect: u8) -> Selection {
    match idx {
        0..=3 => Selection::Effect(idx as u8 + 1),
        4..=8 => Selection::Custom(idx as u8 - 4),
        _ => Selection::Effect(current_effect),
    }
}

/// The selector labels, in `selection_index` order: the 4 sweeps, then the
/// 5 custom slots, then, ONLY while `current_effect` is outside 1-9, one
/// trailing "Effect N" entry showing that raw device state. Custom entries
/// show "CUSTOM N: <name>" using `slot_names[N - 1]` (trimmed), or a plain
/// "CUSTOM N" when that entry is empty, missing, or just the slot's own
/// default name ("CUSTOM N", any case), so an unnamed slot never renders
/// as "CUSTOM 1: CUSTOM 1"; a short (or empty) names list still yields all
/// standing labels.
pub fn dropdown_labels(slot_names: &[String], current_effect: u8) -> Vec<String> {
    let mut labels: Vec<String> = EFFECT_LABELS.iter().map(|s| s.to_string()).collect();
    for n in 1..=CUSTOM_SLOTS {
        let default = format!("CUSTOM {n}");
        let name = slot_names
            .get(n - 1)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case(&default));
        labels.push(match name {
            Some(name) => format!("{default}: {name}"),
            None => default,
        });
    }
    if !(1..=9).contains(&current_effect) {
        labels.push(format!("Effect {current_effect}"));
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
    fn selection_index_orders_sweeps_then_customs() {
        assert_eq!(selection_index(1, 0), 0);
        assert_eq!(selection_index(4, 0), 3);
        assert_eq!(selection_index(5, 0), 4);
        assert_eq!(selection_index(5, 4), 8);
    }

    #[test]
    fn effects_5_to_9_map_to_their_custom_slot_entries() {
        // Effect values 5-9 ARE the custom slots (5 = CUSTOM 1 .. 9 =
        // CUSTOM 5); a readback of any of them selects that CUSTOM entry.
        for e in 5..=9u8 {
            assert_eq!(selection_index(e, 0), 4 + usize::from(e - 5), "effect {e}");
        }
        // 6-9 carry their slot in the value itself; the slot attr only
        // disambiguates effect 5 (custom mode with the active slot).
        assert_eq!(selection_index(7, 4), 6, "effect 7 is CUSTOM 3 regardless of the slot attr");
        assert_eq!(selection_index(5, 3), 7, "effect 5 follows the active slot");
    }

    #[test]
    fn selection_index_sends_out_of_range_effects_to_the_trailing_entry() {
        // Only values outside the decoded 1-9 range have no standing
        // entry; they select the raw trailing entry `dropdown_labels`
        // appends for that state.
        for e in [0u8, 10, 200] {
            assert_eq!(selection_index(e, 0), SELECTION_COUNT, "effect {e}");
        }
        assert_eq!(selection_index(5, 200), 8, "past-the-end slot clamps to CUSTOM 5");
    }

    #[test]
    fn every_index_round_trips_through_selection() {
        for idx in 0..SELECTION_COUNT {
            let back = match index_selection(idx, 1) {
                Selection::Effect(e) => selection_index(e, 0),
                Selection::Custom(s) => selection_index(5, s),
            };
            assert_eq!(back, idx);
        }
    }

    #[test]
    fn every_selection_round_trips_through_index() {
        for e in [1u8, 2, 3, 4] {
            assert_eq!(index_selection(selection_index(e, 0), e), Selection::Effect(e));
        }
        for s in 0..CUSTOM_SLOTS as u8 {
            assert_eq!(index_selection(selection_index(5, s), 5), Selection::Custom(s));
        }
    }

    #[test]
    fn the_trailing_entry_re_selects_the_current_raw_effect() {
        // Picking the appended "Effect 200" entry (or anything past the
        // standing list) just re-writes the current raw value.
        assert_eq!(index_selection(SELECTION_COUNT, 200), Selection::Effect(200));
        assert_eq!(index_selection(999, 0), Selection::Effect(0));
    }

    #[test]
    fn dropdown_labels_has_9_standing_entries_in_selector_order() {
        let labels = dropdown_labels(&[], 1);
        assert_eq!(labels.len(), SELECTION_COUNT, "no raw entry while a labeled effect is active");
        assert_eq!(labels[0], "Inside out");
        assert_eq!(labels[3], "Left to right");
        assert_eq!(labels[4], "CUSTOM 1");
        assert_eq!(labels[8], "CUSTOM 5");
        assert!(!labels.iter().any(|l| l.starts_with("Effect ")), "no unlabeled effects offered");
    }

    #[test]
    fn dropdown_labels_append_the_raw_entry_only_while_active() {
        for e in [0u8, 10, 200] {
            let labels = dropdown_labels(&[], e);
            assert_eq!(labels.len(), SELECTION_COUNT + 1, "effect {e}");
            assert_eq!(labels[SELECTION_COUNT], format!("Effect {e}"));
        }
        // The whole decoded 1-9 range has standing entries: 1-4 are the
        // sweeps, 5-9 the custom slots.
        for e in 1..=9u8 {
            assert_eq!(dropdown_labels(&[], e).len(), SELECTION_COUNT, "effect {e}");
        }
    }

    #[test]
    fn a_device_readback_of_7_shows_custom_3() {
        // The dropdown round-trips raw device effects 5-9 to the CUSTOM
        // entries: 7 = CUSTOM 3 (slot 2), and re-committing that entry
        // resolves back to slot 2.
        let labels = dropdown_labels(&[], 7);
        assert_eq!(labels[selection_index(7, 0)], "CUSTOM 3");
        assert_eq!(index_selection(selection_index(7, 0), 7), Selection::Custom(2));
    }

    #[test]
    fn dropdown_labels_show_trimmed_slot_names() {
        let names = vec![
            "RACE".to_string(),
            "  GT7  ".to_string(),
            String::new(),
            "   ".to_string(),
        ];
        let labels = dropdown_labels(&names, 1);
        assert_eq!(labels[4], "CUSTOM 1: RACE");
        assert_eq!(labels[5], "CUSTOM 2: GT7", "names are trimmed");
        assert_eq!(labels[6], "CUSTOM 3", "empty name falls back to the plain label");
        assert_eq!(labels[7], "CUSTOM 4", "whitespace-only name counts as empty");
        assert_eq!(labels[8], "CUSTOM 5", "missing entry counts as empty");
    }

    #[test]
    fn dropdown_labels_collapse_the_slots_own_default_name() {
        // The wheel ships each slot named "CUSTOM N" (uppercased); showing
        // "CUSTOM 1: CUSTOM 1" would just repeat the label.
        let names = vec![
            "CUSTOM 1".to_string(),
            "Custom 2".to_string(),
            "CUSTOM 1".to_string(),
        ];
        let labels = dropdown_labels(&names, 1);
        assert_eq!(labels[4], "CUSTOM 1", "the default name collapses");
        assert_eq!(labels[5], "CUSTOM 2", "case-insensitively");
        assert_eq!(labels[6], "CUSTOM 3: CUSTOM 1", "another slot's default is a real name");
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
