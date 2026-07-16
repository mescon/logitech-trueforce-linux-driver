//! A G HUB-style point-list curve editor for the pedal/steering response
//! curves. The user edits control points (input/output percent) plus lower and
//! upper deadzones; `compose` turns those into the `in:out` point list the
//! driver's 0x80A4 uploader takes, and `render` draws a live ASCII preview.
//!
//! The wheel does not report a loaded curve's points back (only the count), so
//! this authors curves rather than round-tripping them: an editor opened on an
//! already-shaped axis starts from the value it was handed, defaulting to
//! linear.

use logi_dd_core::Value;

const FULL: u16 = 65535;
/// One percent of full scale, the step for input/output nudges.
const PCT: i32 = 655;

/// Which field the arrow keys act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Point,
    Input,
    Output,
    Lower,
    Upper,
}

impl Field {
    const ORDER: [Field; 5] =
        [Field::Point, Field::Input, Field::Output, Field::Lower, Field::Upper];
    pub fn label(&self) -> &'static str {
        match self {
            Field::Point => "Point",
            Field::Input => "Input",
            Field::Output => "Output",
            Field::Lower => "Lower deadzone",
            Field::Upper => "Upper deadzone",
        }
    }
}

pub struct CurveEditor {
    pub attr: &'static str,
    /// Control points, always sorted by input with `pts[0].0 == 0` and
    /// `pts[last].0 == FULL`; outputs non-decreasing.
    pub pts: Vec<(u16, u16)>,
    pub lower_dz: u8,
    pub upper_dz: u8,
    pub sel: usize,
    pub field: Field,
}

/// Round a 0..=65535 value to a whole percent for display.
pub fn to_pct(v: u16) -> u32 {
    (v as u32 * 100 + (FULL as u32 / 2)) / FULL as u32
}

impl CurveEditor {
    /// Seed from a value. A non-empty curve loads its points; anything else
    /// (built-in / empty) starts from linear. Deadzones start at zero since the
    /// device does not report them back.
    pub fn from_value(attr: &'static str, v: &Value) -> CurveEditor {
        let pts = match v {
            Value::Curve(p) if p.len() >= 2 => p.clone(),
            _ => vec![(0, 0), (FULL, FULL)],
        };
        CurveEditor { attr, pts, lower_dz: 0, upper_dz: 0, sel: 0, field: Field::Point }
    }

    pub fn point_count(&self) -> usize {
        self.pts.len()
    }

    /// Compose the final `in:out` curve for upload: the control points remapped
    /// into the live input band `[lower, 100-upper]`, with flat dead/saturated
    /// segments outside it. Always returns a driver-valid curve (strictly
    /// increasing inputs, non-decreasing outputs, pinned 0:0 and FULL:FULL).
    pub fn compose(&self) -> Vec<(u16, u16)> {
        let lo = (self.lower_dz as u32 * FULL as u32 / 100) as u16;
        let hi = ((100 - self.upper_dz as u32) * FULL as u32 / 100) as u16;
        let span = hi.saturating_sub(lo) as u32;

        let mut out: Vec<(u16, u16)> = Vec::with_capacity(self.pts.len() + 2);
        if self.lower_dz > 0 {
            push_pt(&mut out, 0, 0);
        }
        for &(inp, outp) in &self.pts {
            let mapped = lo as u32 + (inp as u32 * span / FULL as u32);
            push_pt(&mut out, mapped as u16, outp);
        }
        if self.upper_dz > 0 {
            push_pt(&mut out, FULL, FULL);
        }

        // Guarantee the pinned endpoints the uploader demands.
        if out.first().map(|p| p.0) != Some(0) {
            out.insert(0, (0, 0));
        }
        if out.last().map(|p| p.0) != Some(FULL) {
            push_pt(&mut out, FULL, FULL);
        }
        out
    }

    pub fn to_value(&self) -> Value {
        Value::Curve(self.compose())
    }

    pub fn next_field(&mut self) {
        let i = Field::ORDER.iter().position(|f| *f == self.field).unwrap_or(0);
        self.field = Field::ORDER[(i + 1) % Field::ORDER.len()];
    }

    pub fn prev_field(&mut self) {
        let i = Field::ORDER.iter().position(|f| *f == self.field).unwrap_or(0);
        self.field = Field::ORDER[(i + Field::ORDER.len() - 1) % Field::ORDER.len()];
    }

    /// Left/right on the current field.
    pub fn adjust(&mut self, d: i32) {
        match self.field {
            Field::Point => {
                let n = self.pts.len() as i32;
                self.sel = (self.sel as i32 + d).clamp(0, n - 1) as usize;
            }
            Field::Input => self.move_input(d * PCT),
            Field::Output => self.move_output(d * PCT),
            Field::Lower => {
                self.lower_dz = (self.lower_dz as i32 + d)
                    .clamp(0, 99 - self.upper_dz as i32) as u8;
            }
            Field::Upper => {
                self.upper_dz = (self.upper_dz as i32 + d)
                    .clamp(0, 99 - self.lower_dz as i32) as u8;
            }
        }
    }

    /// Move the selected point's input, clamped strictly between its
    /// neighbours. The two endpoints' inputs are pinned (0 and FULL).
    fn move_input(&mut self, delta: i32) {
        let last = self.pts.len() - 1;
        if self.sel == 0 || self.sel == last {
            return;
        }
        let lo = self.pts[self.sel - 1].0 as i32 + 1;
        let hi = self.pts[self.sel + 1].0 as i32 - 1;
        if lo > hi {
            return;
        }
        let cur = self.pts[self.sel].0 as i32;
        self.pts[self.sel].0 = (cur + delta).clamp(lo, hi) as u16;
    }

    /// Move the selected point's output, clamped so outputs stay non-decreasing.
    fn move_output(&mut self, delta: i32) {
        let last = self.pts.len() - 1;
        let lo = if self.sel == 0 { 0 } else { self.pts[self.sel - 1].1 as i32 };
        let hi = if self.sel == last { FULL as i32 } else { self.pts[self.sel + 1].1 as i32 };
        let cur = self.pts[self.sel].1 as i32;
        self.pts[self.sel].1 = (cur + delta).clamp(lo, hi) as u16;
    }

    /// Insert a point midway between the selected point and the next one, and
    /// select it. No-op on the last point (nothing to bisect toward).
    pub fn add_point(&mut self) {
        let last = self.pts.len() - 1;
        if self.sel >= last {
            return;
        }
        let (ai, ao) = self.pts[self.sel];
        let (bi, bo) = self.pts[self.sel + 1];
        if bi - ai < 2 {
            return; // no room for a distinct input between them
        }
        let mid = ((ai as u32 + bi as u32) / 2) as u16;
        let mout = ((ao as u32 + bo as u32) / 2) as u16;
        self.pts.insert(self.sel + 1, (mid, mout));
        self.sel += 1;
        self.field = Field::Input;
    }

    /// Delete the selected point. Endpoints cannot be deleted.
    pub fn delete_point(&mut self) {
        let last = self.pts.len() - 1;
        if self.sel == 0 || self.sel == last || self.pts.len() <= 2 {
            return;
        }
        self.pts.remove(self.sel);
        if self.sel > self.pts.len() - 1 {
            self.sel = self.pts.len() - 1;
        }
    }

    /// Value shown for any field (for rendering the whole panel).
    pub fn value_of(&self, f: Field) -> String {
        match f {
            Field::Point => format!("{} / {}", self.sel + 1, self.pts.len()),
            Field::Input => format!("{}%", to_pct(self.pts[self.sel].0)),
            Field::Output => format!("{}%", to_pct(self.pts[self.sel].1)),
            Field::Lower => format!("{}%", self.lower_dz),
            Field::Upper => format!("{}%", self.upper_dz),
        }
    }

    pub const FIELDS: [Field; 5] = Field::ORDER;

    /// Render the composed curve as an ASCII plot `h` rows by `w` cols. Input on
    /// X (0 left, full right), output on Y (0 bottom, full top). The selected
    /// control point is marked `@`, other plotted cells `*`.
    pub fn render(&self, w: usize, h: usize) -> Vec<String> {
        let w = w.max(8);
        let h = h.max(4);
        let mut grid = vec![vec![b' '; w]; h];
        let curve = self.compose();

        let col = |inp: u16| ((inp as usize * (w - 1)) / FULL as usize).min(w - 1);
        let row = |outp: u16| (h - 1) - ((outp as usize * (h - 1)) / FULL as usize).min(h - 1);

        // Plot the composed curve by walking each column's input to its output.
        // Indexing grid[y][x] with a computed y rules out an iterator loop.
        #[allow(clippy::needless_range_loop)]
        for x in 0..w {
            let inp = ((x * FULL as usize) / (w - 1)) as u32;
            let outp = interp(&curve, inp.min(FULL as u32) as u16);
            let y = row(outp);
            grid[y][x] = b'*';
        }
        // Mark the selected control point (mapped through the deadzones).
        let (si, so) = self.pts[self.sel];
        let lo = (self.lower_dz as u32 * FULL as u32 / 100) as u16;
        let hi = ((100 - self.upper_dz as u32) * FULL as u32 / 100) as u16;
        let span = hi.saturating_sub(lo) as u32;
        let mapped = lo as u32 + (si as u32 * span / FULL as u32);
        grid[row(so)][col(mapped as u16)] = b'@';

        grid.into_iter().map(|r| String::from_utf8(r).unwrap()).collect()
    }
}

/// Append `(inp, outp)`, keeping inputs strictly increasing. A collision means
/// the deadzone remap folded two control points onto one input, so keep the
/// higher output rather than emitting a duplicate the uploader would reject.
fn push_pt(out: &mut Vec<(u16, u16)>, inp: u16, outp: u16) {
    match out.last() {
        Some(&(pi, po)) if pi >= inp => {
            if outp > po {
                let n = out.len() - 1;
                out[n] = (pi, outp);
            }
        }
        _ => out.push((inp, outp)),
    }
}

/// Linear interpolation of a sorted `in:out` curve at input `x`.
fn interp(curve: &[(u16, u16)], x: u16) -> u16 {
    if curve.is_empty() {
        return x;
    }
    if x <= curve[0].0 {
        return curve[0].1;
    }
    for w in curve.windows(2) {
        let (x0, y0) = w[0];
        let (x1, y1) = w[1];
        if x <= x1 {
            if x1 == x0 {
                return y1;
            }
            let t = (x as u32 - x0 as u32) * (y1 as u32 - y0 as u32) / (x1 as u32 - x0 as u32);
            return (y0 as u32 + t) as u16;
        }
    }
    curve.last().unwrap().1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linear() -> CurveEditor {
        CurveEditor::from_value("wheel_throttle_curve", &Value::Curve(vec![]))
    }

    fn is_valid(curve: &[(u16, u16)]) {
        assert!(curve.len() >= 2, "need >=2 points");
        assert_eq!(curve[0], (0, 0), "must start 0:0");
        assert_eq!(*curve.last().unwrap(), (FULL, FULL), "must end FULL:FULL");
        for w in curve.windows(2) {
            assert!(w[1].0 > w[0].0, "inputs strictly increasing: {:?}", curve);
            assert!(w[1].1 >= w[0].1, "outputs non-decreasing: {:?}", curve);
        }
    }

    #[test]
    fn default_is_linear_two_points() {
        let e = linear();
        assert_eq!(e.pts, vec![(0, 0), (FULL, FULL)]);
        is_valid(&e.compose());
    }

    #[test]
    fn add_point_bisects_and_selects() {
        let mut e = linear();
        e.add_point();
        assert_eq!(e.pts.len(), 3);
        assert_eq!(e.sel, 1);
        assert_eq!(e.pts[1], (FULL / 2, FULL / 2));
        is_valid(&e.compose());
    }

    #[test]
    fn endpoints_inputs_are_pinned() {
        let mut e = linear();
        e.sel = 0;
        e.field = Field::Input;
        e.adjust(10);
        assert_eq!(e.pts[0].0, 0, "first input stays 0");
        e.sel = 1;
        e.adjust(-10);
        assert_eq!(e.pts[1].0, FULL, "last input stays FULL");
    }

    #[test]
    fn middle_input_clamps_between_neighbours() {
        let mut e = linear();
        e.add_point(); // point at (32767, 32767), sel=1
        e.field = Field::Input;
        // shove far right: cannot pass the last point's input
        e.adjust(1000);
        assert!(e.pts[1].0 < FULL);
        assert!(e.pts[1].0 > e.pts[0].0);
        is_valid(&e.compose());
    }

    #[test]
    fn output_stays_monotonic() {
        let mut e = linear();
        e.add_point(); // sel=1, mid
        e.field = Field::Output;
        e.adjust(1000); // raise well above the neighbours
        e.adjust(-1000); // and back down; must never cross a neighbour
        is_valid(&e.compose());
    }

    #[test]
    fn delete_removes_middle_only() {
        let mut e = linear();
        e.add_point();
        assert_eq!(e.pts.len(), 3);
        e.sel = 1;
        e.delete_point();
        assert_eq!(e.pts.len(), 2);
        // endpoints can't be deleted
        e.sel = 0;
        e.delete_point();
        e.sel = 1;
        e.delete_point();
        assert_eq!(e.pts.len(), 2);
    }

    #[test]
    fn lower_deadzone_holds_output_zero() {
        let mut e = linear();
        e.lower_dz = 20;
        let c = e.compose();
        is_valid(&c);
        // at 10% input (below the 20% deadzone) output must still be 0
        assert_eq!(interp(&c, (FULL as u32 / 10) as u16), 0);
        // at 60% input output should be well above 0
        assert!(interp(&c, (FULL as u32 * 6 / 10) as u16) > 0);
    }

    #[test]
    fn upper_deadzone_saturates_early() {
        let mut e = linear();
        e.upper_dz = 20;
        let c = e.compose();
        is_valid(&c);
        // by 85% input (past the 80% saturation point) output is full
        assert_eq!(interp(&c, (FULL as u32 * 85 / 100) as u16), FULL);
    }

    #[test]
    fn deadzones_cannot_sum_over_99() {
        let mut e = linear();
        e.field = Field::Lower;
        for _ in 0..200 {
            e.adjust(1);
        }
        assert_eq!(e.lower_dz, 99);
        e.field = Field::Upper;
        e.adjust(1);
        assert_eq!(e.upper_dz, 0, "upper cannot grow while lower is 99");
    }

    #[test]
    fn field_cycle_wraps() {
        let mut e = linear();
        assert_eq!(e.field, Field::Point);
        e.prev_field();
        assert_eq!(e.field, Field::Upper);
        e.next_field();
        assert_eq!(e.field, Field::Point);
    }

    #[test]
    fn render_has_requested_dimensions() {
        let mut e = linear();
        e.add_point();
        e.sel = 1;
        e.field = Field::Output;
        e.adjust(20); // bend it
        let rows = e.render(40, 10);
        assert_eq!(rows.len(), 10);
        assert!(rows.iter().all(|r| r.chars().count() == 40));
        // the selected point marker is present
        assert!(rows.iter().any(|r| r.contains('@')));
    }

    #[test]
    fn composed_curve_always_driver_valid_under_edits() {
        let mut e = linear();
        e.add_point();
        e.add_point();
        e.field = Field::Output;
        e.adjust(30);
        e.field = Field::Lower;
        e.adjust(10);
        e.field = Field::Upper;
        e.adjust(15);
        is_valid(&e.compose());
    }
}
