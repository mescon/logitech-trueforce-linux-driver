//! A G HUB-style point-list curve editor for the pedal/steering response
//! curves. The user edits control points (input/output percent) plus lower
//! and upper deadzones; the composed curve is what the driver's 0x80A4
//! uploader takes, and `render` draws a live ASCII preview.
//!
//! The point-list model and its composition into a driver-valid curve live in
//! `logi_dd_core::curve::Curve`, shared with any other frontend; this module
//! only adds the field-navigation state (which field the arrow keys act on,
//! and which point is selected) that is specific to the TUI.

use logi_dd_core::curve::{interp, to_pct, Curve, FULL};
use logi_dd_core::Value;

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
    curve: Curve,
    pub sel: usize,
    pub field: Field,
}

impl CurveEditor {
    /// Seed from a value; see `Curve::from_value`.
    pub fn from_value(attr: &'static str, v: &Value) -> CurveEditor {
        CurveEditor { attr, curve: Curve::from_value(attr, v), sel: 0, field: Field::Point }
    }

    pub fn point_count(&self) -> usize {
        self.curve.points().len()
    }

    /// Compose the final `in:out` curve for upload; see `Curve::compose`.
    pub fn compose(&self) -> Vec<(u16, u16)> {
        self.curve.compose()
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
                let n = self.curve.points().len() as i32;
                self.sel = (self.sel as i32 + d).clamp(0, n - 1) as usize;
            }
            Field::Input => self.move_input(d * PCT),
            Field::Output => self.move_output(d * PCT),
            Field::Lower => {
                let cur = self.curve.lower_deadzone() as i32;
                let v = (cur + d).clamp(0, 99) as u8;
                self.curve.set_lower_deadzone(v);
            }
            Field::Upper => {
                let cur = self.curve.upper_deadzone() as i32;
                let v = (cur + d).clamp(0, 99) as u8;
                self.curve.set_upper_deadzone(v);
            }
        }
    }

    /// Move the selected point's input, clamped strictly between its
    /// neighbours. The two endpoints' inputs are pinned (0 and FULL); the
    /// model (`Curve::move_point`) enforces that.
    fn move_input(&mut self, delta: i32) {
        let (cur_in, cur_out) = self.curve.points()[self.sel];
        let candidate = (cur_in as i32 + delta).clamp(0, FULL as i32) as u16;
        self.curve.move_point(self.sel, candidate, cur_out);
    }

    /// Move the selected point's output, clamped so outputs stay
    /// non-decreasing. The endpoint outputs are pinned too, like their
    /// inputs: the driver's uploader requires the curve to start 0:0 and end
    /// FULL:FULL, and the deadzones handle any dead/saturated ends.
    fn move_output(&mut self, delta: i32) {
        let (cur_in, cur_out) = self.curve.points()[self.sel];
        let candidate = (cur_out as i32 + delta).clamp(0, FULL as i32) as u16;
        self.curve.move_point(self.sel, cur_in, candidate);
    }

    /// Insert a point midway between the selected point and the next one, and
    /// select it. No-op on the last point (nothing to bisect toward), or when
    /// there is no room for a distinct input between them.
    pub fn add_point(&mut self) {
        let last = self.curve.points().len() - 1;
        if self.sel >= last {
            return;
        }
        let (ai, _) = self.curve.points()[self.sel];
        let (bi, _) = self.curve.points()[self.sel + 1];
        if bi - ai < 2 {
            return; // no room for a distinct input between them
        }
        let mid = ((ai as u32 + bi as u32) / 2) as u16;
        self.curve.add_point(mid);
        self.sel += 1;
        self.field = Field::Input;
    }

    /// Delete the selected point. Endpoints cannot be deleted.
    pub fn delete_point(&mut self) {
        self.curve.remove_point(self.sel);
        let last = self.curve.points().len() - 1;
        if self.sel > last {
            self.sel = last;
        }
    }

    /// Value shown for any field (for rendering the whole panel).
    pub fn value_of(&self, f: Field) -> String {
        match f {
            Field::Point => format!("{} / {}", self.sel + 1, self.curve.points().len()),
            Field::Input => format!("{}%", to_pct(self.curve.points()[self.sel].0)),
            Field::Output => format!("{}%", to_pct(self.curve.points()[self.sel].1)),
            Field::Lower => format!("{}%", self.curve.lower_deadzone()),
            Field::Upper => format!("{}%", self.curve.upper_deadzone()),
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
        let curve = self.curve.compose();

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
        let (si, so) = self.curve.points()[self.sel];
        let lo = (self.curve.lower_deadzone() as u32 * FULL as u32 / 100) as u16;
        let hi = ((100 - self.curve.upper_deadzone() as u32) * FULL as u32 / 100) as u16;
        let span = hi.saturating_sub(lo) as u32;
        let mapped = lo as u32 + (si as u32 * span / FULL as u32);
        grid[row(so)][col(mapped as u16)] = b'@';

        grid.into_iter().map(|r| String::from_utf8(r).unwrap()).collect()
    }
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
    fn add_point_bisects_and_selects() {
        let mut e = linear();
        e.add_point();
        assert_eq!(e.curve.points().len(), 3);
        assert_eq!(e.sel, 1);
        assert_eq!(e.curve.points()[1], (FULL / 2, FULL / 2));
        is_valid(&e.compose());
    }

    #[test]
    fn endpoints_inputs_are_pinned() {
        let mut e = linear();
        e.sel = 0;
        e.field = Field::Input;
        e.adjust(10);
        assert_eq!(e.curve.points()[0].0, 0, "first input stays 0");
        e.sel = 1;
        e.adjust(-10);
        assert_eq!(e.curve.points()[1].0, FULL, "last input stays FULL");
    }

    #[test]
    fn endpoint_outputs_are_pinned_so_compose_stays_valid() {
        // Regression: raising the first point's output or lowering the last
        // point's output must not produce a curve the driver rejects.
        let mut e = linear();
        e.field = Field::Output;
        e.sel = 0;
        e.adjust(5); // try to lift (0,0) -> (0, >0)
        assert_eq!(e.curve.points()[0].1, 0, "first output stays 0");
        e.sel = 1;
        e.adjust(-5); // try to drop (FULL,FULL) -> (FULL, <FULL)
        assert_eq!(e.curve.points()[1].1, FULL, "last output stays FULL");
        is_valid(&e.compose());
    }

    #[test]
    fn middle_input_clamps_between_neighbours() {
        let mut e = linear();
        e.add_point(); // point at (32767, 32767), sel=1
        e.field = Field::Input;
        // shove far right: cannot pass the last point's input
        e.adjust(1000);
        assert!(e.curve.points()[1].0 < FULL);
        assert!(e.curve.points()[1].0 > e.curve.points()[0].0);
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
        assert_eq!(e.curve.points().len(), 3);
        e.sel = 1;
        e.delete_point();
        assert_eq!(e.curve.points().len(), 2);
        // endpoints can't be deleted
        e.sel = 0;
        e.delete_point();
        e.sel = 1;
        e.delete_point();
        assert_eq!(e.curve.points().len(), 2);
    }

    #[test]
    fn deadzones_cannot_sum_over_99() {
        let mut e = linear();
        e.field = Field::Lower;
        for _ in 0..200 {
            e.adjust(1);
        }
        assert_eq!(e.curve.lower_deadzone(), 99);
        e.field = Field::Upper;
        e.adjust(1);
        assert_eq!(e.curve.upper_deadzone(), 0, "upper cannot grow while lower is 99");
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
