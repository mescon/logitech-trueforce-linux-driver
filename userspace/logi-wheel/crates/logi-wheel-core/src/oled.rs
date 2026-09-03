//! The base's Dynamic OLED as the apps see it: the ten layouts, what each
//! takes, how to compose a `wheel_oled` frame from typed fields, how to read
//! one back into fields, and how to draw an approximation of it on screen.
//!
//! The driver owns the panel's rules and validates every write; this is the
//! presentation-side mirror of its schema (`hidpp_dd_oled_schema` in the
//! driver, `docs/PROTOCOL_SPECIFICATION.md` 12.3), so both apps compose the
//! same string from the same fields and cannot drift apart on it.

/// One field of a layout's payload, in write order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// A number 0..=255: a gauge fill or its indicator mark.
    Number { label: &'static str },
    /// Text up to `width` characters, space-padded by the driver.
    Text { label: &'static str, width: usize },
}

impl Field {
    pub fn label(&self) -> &'static str {
        match self {
            Field::Number { label } | Field::Text { label, .. } => label,
        }
    }
}

/// How a text field is drawn, for the preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Small,
    Medium,
    Large,
    VeryLarge,
}

/// Where a preview row sits and how its text is placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Centre,
    Right,
    /// First character pinned left, the rest right-aligned: the renderer's
    /// two-zone rule on the wide large rows.
    TwoZone,
}

/// One layout: letter, what it is, the fields it takes in write order.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub index: u8,
    pub letter: char,
    pub name: &'static str,
    pub fields: &'static [Field],
}

pub const LAYOUTS: [Layout; 10] = [
    Layout { index: 0, letter: 'A', name: "Black screen", fields: &[] },
    Layout { index: 1, letter: 'B', name: "Firmware test pattern", fields: &[] },
    Layout { index: 2, letter: 'C', name: "Gauge", fields: &[Field::Number { label: "Fill" }] },
    Layout {
        index: 3,
        letter: 'D',
        name: "Gauge with a label",
        fields: &[
            Field::Number { label: "Fill" },
            Field::Number { label: "Mark" },
            Field::Text { label: "Label", width: 11 },
        ],
    },
    Layout {
        index: 4,
        letter: 'E',
        name: "Gauge with two values",
        fields: &[
            Field::Number { label: "Fill" },
            Field::Number { label: "Mark" },
            Field::Text { label: "Right (3)", width: 3 },
            Field::Text { label: "Left (7)", width: 7 },
        ],
    },
    Layout {
        index: 5,
        letter: 'F',
        name: "Small gear, big value",
        fields: &[Field::Text { label: "Gear", width: 1 }, Field::Text { label: "Value", width: 3 }],
    },
    Layout {
        index: 6,
        letter: 'G',
        name: "Big gear, value beside it",
        fields: &[Field::Text { label: "Gear", width: 1 }, Field::Text { label: "Value", width: 3 }],
    },
    Layout {
        index: 7,
        letter: 'H',
        name: "Small row over a large row",
        fields: &[Field::Text { label: "Top", width: 21 }, Field::Text { label: "Bottom", width: 10 }],
    },
    Layout {
        index: 8,
        letter: 'I',
        name: "Four rows, right-aligned",
        fields: &[
            Field::Text { label: "Row 1", width: 19 },
            Field::Text { label: "Row 2 (large)", width: 10 },
            Field::Text { label: "Row 3", width: 19 },
            Field::Text { label: "Row 4 (large)", width: 10 },
        ],
    },
    Layout {
        index: 9,
        letter: 'J',
        name: "Four rows, centred",
        fields: &[
            Field::Text { label: "Row 1", width: 19 },
            Field::Text { label: "Row 2 (large)", width: 10 },
            Field::Text { label: "Row 3", width: 19 },
            Field::Text { label: "Row 4 (large)", width: 10 },
        ],
    },
];

/// The layout for a letter (either case) or a digit.
pub fn layout(key: char) -> Option<&'static Layout> {
    let k = key.to_ascii_uppercase();
    LAYOUTS.iter().find(|l| l.letter == k || char::from(b'0' + l.index) == k)
}

/// Labels for a layout picker: "G  Big gear, value beside it".
pub fn picker_labels() -> Vec<String> {
    LAYOUTS.iter().map(|l| format!("{}  {}", l.letter, l.name)).collect()
}

/// What went wrong composing a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposeError {
    /// Field `index` is longer than the layout allows.
    TooLong { index: usize, width: usize },
    /// Field `index` should be a number 0..=255.
    NotANumber { index: usize },
    /// A field contains the separator, which the frame cannot carry.
    Separator { index: usize },
}

impl std::fmt::Display for ComposeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComposeError::TooLong { index, width } => {
                write!(f, "field {} is wider than {width} characters", index + 1)
            }
            ComposeError::NotANumber { index } => write!(f, "field {} must be 0 to 255", index + 1),
            ComposeError::Separator { index } => write!(f, "field {} cannot contain |", index + 1),
        }
    }
}

/// Compose the `wheel_oled` string for `layout` from `values`, one per
/// field in write order. Missing trailing values are left blank; text is
/// trimmed on the right only, so leading spaces keep their meaning on the
/// two-zone rows. This validates what the driver validates, so a frame
/// that composes is a frame the wheel accepts.
pub fn compose(layout: &Layout, values: &[String]) -> Result<String, ComposeError> {
    let mut out = String::from(layout.letter);
    for (i, field) in layout.fields.iter().enumerate() {
        let raw = values.get(i).map(String::as_str).unwrap_or("");
        out.push('|');
        match field {
            Field::Number { .. } => {
                let t = raw.trim();
                if t.is_empty() {
                    out.push('0');
                } else {
                    let n: u32 = t.parse().map_err(|_| ComposeError::NotANumber { index: i })?;
                    if n > 255 {
                        return Err(ComposeError::NotANumber { index: i });
                    }
                    out.push_str(&n.to_string());
                }
            }
            Field::Text { width, .. } => {
                if raw.contains('|') {
                    return Err(ComposeError::Separator { index: i });
                }
                let t = raw.trim_end();
                if t.chars().count() > *width {
                    return Err(ComposeError::TooLong { index: i, width: *width });
                }
                out.push_str(t);
            }
        }
    }
    // Trailing empty fields add nothing the driver needs.
    while out.ends_with('|') {
        out.pop();
    }
    Ok(out)
}

/// Read a `wheel_oled` string back into (layout, values), for seeding an
/// editor from what the wheel is showing. `off`, empty, or an unknown
/// layout give `None`.
pub fn parse(frame: &str) -> Option<(&'static Layout, Vec<String>)> {
    let frame = frame.trim();
    let mut parts = frame.split('|');
    let head = parts.next()?.trim();
    let mut chars = head.chars();
    let key = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    let layout = self::layout(key)?;
    let mut values: Vec<String> = parts.map(str::to_string).collect();
    values.resize(layout.fields.len(), String::new());
    Some((layout, values))
}

/// One drawn row of the preview: the text as the panel would place it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewRow {
    pub text: String,
    pub size: Size,
    pub align: Align,
}

/// A preview of `values` on `layout` as rows, top to bottom, following the
/// renderer's rules in 12.3. Gauges are shown as a bar description rather
/// than drawn, since the fill byte is the whole picture there.
pub fn preview(layout: &Layout, values: &[String]) -> Vec<PreviewRow> {
    let v = |i: usize| values.get(i).map(|s| s.trim_end().to_string()).unwrap_or_default();
    let n = |i: usize| values.get(i).and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0).min(255);
    let bar = |fill: u32| {
        let filled = (fill as usize * 16 + 127) / 255;
        format!("{}{}", "#".repeat(filled), "-".repeat(16 - filled))
    };
    match layout.letter {
        'A' => vec![],
        'B' => vec![PreviewRow { text: "(firmware test pattern)".into(), size: Size::Small, align: Align::Centre }],
        'C' => vec![PreviewRow { text: bar(n(0)), size: Size::Medium, align: Align::Centre }],
        'D' => vec![
            PreviewRow { text: bar(n(0)), size: Size::Medium, align: Align::Centre },
            PreviewRow { text: v(2), size: Size::Small, align: Align::Centre },
        ],
        'E' => vec![
            PreviewRow { text: bar(n(0)), size: Size::Medium, align: Align::Centre },
            PreviewRow { text: format!("{:<7} {:>3}", v(3), v(2)), size: Size::Small, align: Align::Centre },
        ],
        'F' => vec![PreviewRow { text: format!("{} {}", v(0), v(1)), size: Size::VeryLarge, align: Align::Centre }],
        'G' => vec![PreviewRow { text: format!("{} {}", v(0), v(1)), size: Size::VeryLarge, align: Align::Centre }],
        'H' => vec![
            PreviewRow { text: v(0), size: Size::Small, align: Align::Left },
            PreviewRow { text: v(1), size: Size::Large, align: Align::TwoZone },
        ],
        'I' => vec![
            PreviewRow { text: v(0), size: Size::Small, align: Align::Right },
            PreviewRow { text: v(1), size: Size::Large, align: Align::TwoZone },
            PreviewRow { text: v(2), size: Size::Small, align: Align::Right },
            PreviewRow { text: v(3), size: Size::Large, align: Align::TwoZone },
        ],
        'J' => vec![
            PreviewRow { text: v(0), size: Size::Small, align: Align::Centre },
            PreviewRow { text: v(1), size: Size::Large, align: Align::Centre },
            PreviewRow { text: v(2), size: Size::Small, align: Align::Centre },
            PreviewRow { text: v(3), size: Size::Large, align: Align::Centre },
        ],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn every_layout_matches_the_drivers_schema() {
        // Field counts and text widths as the driver's table and the RS50's
        // own descriptors report them (12.3).
        let widths: Vec<Vec<usize>> = LAYOUTS
            .iter()
            .map(|l| l.fields.iter().filter_map(|f| match f { Field::Text { width, .. } => Some(*width), _ => None }).collect())
            .collect();
        assert_eq!(widths, vec![vec![], vec![], vec![], vec![11], vec![3, 7], vec![1, 3], vec![1, 3], vec![21, 10], vec![19, 10, 19, 10], vec![19, 10, 19, 10]]);
        assert_eq!(LAYOUTS[3].fields.len(), 3, "D takes two gauge bytes then its label");
        assert_eq!(LAYOUTS[4].fields.len(), 4);
    }

    #[test]
    fn compose_matches_what_the_driver_takes() {
        assert_eq!(compose(layout('G').unwrap(), &s(&["3", "142"])).unwrap(), "G|3|142");
        assert_eq!(compose(layout('j').unwrap(), &s(&["Lap 12", "1:42.7", "Best", "1:41.9"])).unwrap(), "J|Lap 12|1:42.7|Best|1:41.9");
        assert_eq!(compose(layout('D').unwrap(), &s(&["200", "", "FUEL"])).unwrap(), "D|200|0|FUEL");
        assert_eq!(compose(layout('A').unwrap(), &[]).unwrap(), "A");
        // Trailing blanks are dropped; a leading space survives for the two-zone rule.
        assert_eq!(compose(layout('J').unwrap(), &s(&["only", "", "", ""])).unwrap(), "J|only");
        assert_eq!(compose(layout('H').unwrap(), &s(&["top", " 112%"])).unwrap(), "H|top| 112%");
    }

    #[test]
    fn compose_refuses_what_the_driver_refuses() {
        assert_eq!(compose(layout('G').unwrap(), &s(&["12", "1"])), Err(ComposeError::TooLong { index: 0, width: 1 }));
        assert_eq!(compose(layout('C').unwrap(), &s(&["300"])), Err(ComposeError::NotANumber { index: 0 }));
        assert_eq!(compose(layout('C').unwrap(), &s(&["full"])), Err(ComposeError::NotANumber { index: 0 }));
        assert_eq!(compose(layout('J').unwrap(), &s(&["a|b"])), Err(ComposeError::Separator { index: 0 }));
    }

    #[test]
    fn parse_reads_a_frame_back_for_the_editor() {
        let (l, v) = parse("G|3|142\n").unwrap();
        assert_eq!((l.letter, v), ('G', s(&["3", "142"])));
        let (l, v) = parse("J|Lap 12|1:42.7").unwrap();
        assert_eq!(l.letter, 'J');
        assert_eq!(v, s(&["Lap 12", "1:42.7", "", ""]), "missing trailing fields come back blank");
        assert!(parse("off").is_none());
        assert!(parse("").is_none());
        assert!(parse("Z|x").is_none());
        assert_eq!(parse("9").unwrap().0.letter, 'J', "a digit names the layout too");
    }

    #[test]
    fn preview_follows_the_renderers_rules() {
        let rows = preview(layout('H').unwrap(), &s(&["SMALL ROW", "112%"]));
        assert_eq!(rows[1].align, Align::TwoZone, "wide large rows split");
        let rows = preview(layout('J').unwrap(), &s(&["a", "b", "c", "d"]));
        assert!(rows.iter().all(|r| r.align == Align::Centre), "J centres every row");
        assert_eq!(rows[1].size, Size::Large);
        let rows = preview(layout('C').unwrap(), &s(&["255"]));
        assert_eq!(rows[0].text, "################");
        assert!(preview(layout('A').unwrap(), &[]).is_empty());
    }
}
