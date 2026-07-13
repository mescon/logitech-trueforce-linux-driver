use logi_dd_core::{Error, Kind, Value};

pub struct EditState {
    pub attr: &'static str,
    pub kind: Kind,
    pub draft: Value,
    pub buffer: String,
}

impl EditState {
    pub fn start(attr: &'static str, kind: Kind, current: &Value) -> EditState {
        let buffer = match (kind, current) {
            (Kind::TextField { .. }, Value::Text(s)) => s.clone(),
            _ => String::new(),
        };
        EditState { attr, kind, draft: current.clone(), buffer }
    }

    pub fn bump(&mut self, d: i32) {
        self.draft = match (self.kind, &self.draft) {
            (Kind::Percent, Value::Percent(n)) => {
                Value::Percent((*n as i32 + d).clamp(0, 100) as u8)
            }
            (Kind::IntRange { min, max, step, .. }, Value::Int(n)) => {
                Value::Int((*n + d * step).clamp(min, max))
            }
            (Kind::Enum(vs), Value::Enum(n)) => {
                let len = vs.len() as i32;
                Value::Enum((*n as i32 + d).rem_euclid(len) as u8)
            }
            (Kind::Toggle { .. }, Value::Bool(b)) => Value::Bool(!*b),
            (_, v) => v.clone(),
        };
    }

    pub fn push_char(&mut self, c: char) {
        if let Kind::TextField { max_len } = self.kind {
            if self.buffer.chars().count() < max_len {
                self.buffer.push(c);
                self.draft = Value::Text(self.buffer.clone());
            }
        }
    }

    pub fn backspace(&mut self) {
        if matches!(self.kind, Kind::TextField { .. }) {
            self.buffer.pop();
            self.draft = Value::Text(self.buffer.clone());
        }
    }

    pub fn commit_value(&self) -> Result<Value, Error> {
        self.kind.validate(&self.draft)?;
        Ok(self.draft.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use logi_dd_core::{Kind, Value};

    #[test]
    fn percent_bump_clamps() {
        let mut e = EditState::start("wheel_strength", Kind::Percent, &Value::Percent(99));
        e.bump(5);
        assert_eq!(e.commit_value().unwrap(), Value::Percent(100));
        e.bump(-200);
        assert_eq!(e.commit_value().unwrap(), Value::Percent(0));
    }

    #[test]
    fn intrange_bump_respects_step_and_bounds() {
        let k = Kind::IntRange { min: 90, max: 2700, step: 10, unit: "deg" };
        let mut e = EditState::start("wheel_range", k, &Value::Int(900));
        e.bump(1);
        assert_eq!(e.commit_value().unwrap(), Value::Int(910));
    }

    #[test]
    fn enum_bump_wraps() {
        let k = Kind::Enum(&["kf", "tf"]);
        let mut e = EditState::start("wheel_texture_route", k, &Value::Enum(1));
        e.bump(1);
        assert_eq!(e.commit_value().unwrap(), Value::Enum(0));
    }

    #[test]
    fn text_edit_buffer() {
        let mut e = EditState::start("wheel_led_slot_name", Kind::TextField { max_len: 8 }, &Value::Text("RACE".into()));
        e.push_char('R');
        assert_eq!(e.commit_value().unwrap(), Value::Text("RACER".into()));
        e.backspace();
        assert_eq!(e.commit_value().unwrap(), Value::Text("RACE".into()));
    }
}
