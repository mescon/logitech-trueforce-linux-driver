//! The point-list curve model shared by every frontend that shapes a pedal or
//! steering response curve (TUI, GUI, ...). `compose` turns the user's control
//! points plus lower/upper deadzones into the `in:out` point list the
//! driver's 0x80A4 uploader takes.
//!
//! The wheel does not report a loaded curve's points back (only the count),
//! so this authors curves rather than round-tripping them: a `Curve` seeded
//! from an already-shaped axis starts from the value it was handed,
//! defaulting to linear.

use crate::Value;

pub const FULL: u16 = 65535;

/// A point-list response curve: control points (input/output, 0..=FULL) plus
/// lower and upper deadzones (percent). `pts` is always sorted by input with
/// `pts[0].0 == 0` and `pts[last].0 == FULL`; outputs are non-decreasing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Curve {
    pts: Vec<(u16, u16)>,
    lower_dz: u8,
    upper_dz: u8,
}

/// Round a 0..=65535 value to a whole percent for display.
pub fn to_pct(v: u16) -> u32 {
    (v as u32 * 100 + (FULL as u32 / 2)) / FULL as u32
}

impl Curve {
    /// Seed from a value. A non-empty curve loads its points; anything else
    /// (built-in / empty) starts from linear. Deadzones start at zero since
    /// the device does not report them back. `attr` is accepted for parity
    /// with the driver's per-attribute reads; every curve attribute seeds the
    /// same way today.
    pub fn from_value(_attr: &str, v: &Value) -> Curve {
        let pts = match v {
            Value::Curve(p) if p.len() >= 2 => p.clone(),
            _ => vec![(0, 0), (FULL, FULL)],
        };
        Curve { pts, lower_dz: 0, upper_dz: 0 }
    }

    pub fn points(&self) -> &[(u16, u16)] {
        &self.pts
    }

    pub fn lower_deadzone(&self) -> u8 {
        self.lower_dz
    }

    pub fn upper_deadzone(&self) -> u8 {
        self.upper_dz
    }

    /// Set the lower deadzone, clamped so it cannot push the pair's sum over
    /// 99%.
    pub fn set_lower_deadzone(&mut self, v: u8) {
        self.lower_dz = v.min(99u8.saturating_sub(self.upper_dz));
    }

    /// Set the upper deadzone, clamped so it cannot push the pair's sum over
    /// 99%.
    pub fn set_upper_deadzone(&mut self, v: u8) {
        self.upper_dz = v.min(99u8.saturating_sub(self.lower_dz));
    }

    /// Compose the final `in:out` curve for upload: the control points
    /// remapped into the live input band `[lower, 100-upper]`, with flat
    /// dead/saturated segments outside it. Always returns a driver-valid
    /// curve (strictly increasing inputs, non-decreasing outputs, pinned
    /// 0:0 and FULL:FULL).
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

        // Guarantee the pinned endpoints the uploader demands: it rejects any
        // curve not starting exactly 0:0 and ending FULL:FULL. Pin both the
        // inputs and the outputs, defensively, so a stray control point can
        // never produce a curve the driver refuses.
        if out.first().map(|p| p.0) != Some(0) {
            out.insert(0, (0, 0));
        }
        if out.last().map(|p| p.0) != Some(FULL) {
            push_pt(&mut out, FULL, FULL);
        }
        out[0].1 = 0;
        let n = out.len() - 1;
        out[n].1 = FULL;
        out
    }

    pub fn to_value(&self) -> Value {
        Value::Curve(self.compose())
    }

    /// Insert a control point at `input`, its output linearly interpolated
    /// from the existing points. A no-op if a point already sits at that
    /// exact input (nothing distinct to insert).
    pub fn add_point(&mut self, input: u16) {
        if let Err(idx) = self.pts.binary_search_by_key(&input, |p| p.0) {
            let output = interp(&self.pts, input);
            self.pts.insert(idx, (input, output));
        }
    }

    /// Remove the point at index `i`. Endpoints cannot be removed, and the
    /// curve cannot be shrunk below two points.
    pub fn remove_point(&mut self, i: usize) {
        let last = self.pts.len() - 1;
        if i == 0 || i == last || self.pts.len() <= 2 {
            return;
        }
        self.pts.remove(i);
    }

    /// Move the point at index `i` to `(input, output)`, clamped so the
    /// curve stays driver-valid: the input strictly between its neighbours,
    /// the output non-decreasing against them. Endpoints are pinned (0:0 and
    /// FULL:FULL) and cannot be moved.
    pub fn move_point(&mut self, i: usize, input: u16, output: u16) {
        let last = self.pts.len() - 1;
        if i == 0 || i == last {
            return;
        }
        let in_lo = self.pts[i - 1].0 as i32 + 1;
        let in_hi = self.pts[i + 1].0 as i32 - 1;
        let new_in = if in_lo <= in_hi {
            (input as i32).clamp(in_lo, in_hi) as u16
        } else {
            self.pts[i].0
        };
        let out_lo = self.pts[i - 1].1;
        let out_hi = self.pts[i + 1].1;
        let new_out = output.clamp(out_lo, out_hi);
        self.pts[i] = (new_in, new_out);
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
pub fn interp(curve: &[(u16, u16)], x: u16) -> u16 {
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

    fn linear() -> Curve {
        Curve::from_value("wheel_throttle_curve", &Value::Curve(vec![]))
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
        let c = linear();
        assert_eq!(c.points(), &[(0, 0), (FULL, FULL)]);
        is_valid(&c.compose());
    }

    #[test]
    fn add_point_bisects() {
        let mut c = linear();
        c.add_point(FULL / 2);
        assert_eq!(c.points().len(), 3);
        assert_eq!(c.points()[1], (FULL / 2, FULL / 2));
        is_valid(&c.compose());
    }

    #[test]
    fn add_point_at_existing_input_is_a_no_op() {
        let mut c = linear();
        c.add_point(0); // already the first point's input
        assert_eq!(c.points().len(), 2, "no duplicate inserted");
    }

    #[test]
    fn endpoints_inputs_are_pinned() {
        let mut c = linear();
        c.move_point(0, 10000, c.points()[0].1);
        assert_eq!(c.points()[0].0, 0, "first input stays 0");
        let last = c.points().len() - 1;
        c.move_point(last, 100, c.points()[last].1);
        assert_eq!(c.points()[last].0, FULL, "last input stays FULL");
    }

    #[test]
    fn endpoint_outputs_are_pinned_so_compose_stays_valid() {
        // Regression: raising the first point's output or lowering the last
        // point's output must not produce a curve the driver rejects.
        let mut c = linear();
        c.move_point(0, c.points()[0].0, 5000); // try to lift (0,0) -> (0, >0)
        assert_eq!(c.points()[0].1, 0, "first output stays 0");
        let last = c.points().len() - 1;
        c.move_point(last, c.points()[last].0, FULL - 5000); // try to drop (FULL,FULL)
        assert_eq!(c.points()[last].1, FULL, "last output stays FULL");
        is_valid(&c.compose());
    }

    #[test]
    fn middle_input_clamps_between_neighbours() {
        let mut c = linear();
        c.add_point(FULL / 2); // point at (32767, 32767), index 1
        // shove far right: cannot pass the last point's input
        c.move_point(1, FULL, c.points()[1].1);
        assert!(c.points()[1].0 < FULL);
        assert!(c.points()[1].0 > c.points()[0].0);
        is_valid(&c.compose());
    }

    #[test]
    fn output_stays_monotonic() {
        let mut c = linear();
        c.add_point(FULL / 2); // index 1, mid
        c.move_point(1, c.points()[1].0, FULL); // raise well above the neighbours
        c.move_point(1, c.points()[1].0, 0); // and back down; must never cross a neighbour
        is_valid(&c.compose());
    }

    #[test]
    fn remove_point_removes_middle_only() {
        let mut c = linear();
        c.add_point(FULL / 2);
        assert_eq!(c.points().len(), 3);
        c.remove_point(1);
        assert_eq!(c.points().len(), 2);
        // endpoints can't be removed
        c.remove_point(0);
        c.remove_point(1);
        assert_eq!(c.points().len(), 2);
    }

    #[test]
    fn lower_deadzone_holds_output_zero() {
        let mut c = linear();
        c.set_lower_deadzone(20);
        let out = c.compose();
        is_valid(&out);
        // at 10% input (below the 20% deadzone) output must still be 0
        assert_eq!(interp(&out, (FULL as u32 / 10) as u16), 0);
        // at 60% input output should be well above 0
        assert!(interp(&out, (FULL as u32 * 6 / 10) as u16) > 0);
    }

    #[test]
    fn upper_deadzone_saturates_early() {
        let mut c = linear();
        c.set_upper_deadzone(20);
        let out = c.compose();
        is_valid(&out);
        // by 85% input (past the 80% saturation point) output is full
        assert_eq!(interp(&out, (FULL as u32 * 85 / 100) as u16), FULL);
    }

    #[test]
    fn deadzones_cannot_sum_over_99() {
        let mut c = linear();
        c.set_lower_deadzone(200); // clamped to 99
        assert_eq!(c.lower_deadzone(), 99);
        c.set_upper_deadzone(1);
        assert_eq!(c.upper_deadzone(), 0, "upper cannot grow while lower is 99");
    }

    #[test]
    fn composed_curve_always_driver_valid_under_edits() {
        let mut c = linear();
        c.add_point(FULL / 3);
        c.add_point((2 * FULL as u32 / 3) as u16);
        c.move_point(2, c.points()[2].0, FULL); // bend hard
        c.set_lower_deadzone(10);
        c.set_upper_deadzone(15);
        is_valid(&c.compose());
    }
}
