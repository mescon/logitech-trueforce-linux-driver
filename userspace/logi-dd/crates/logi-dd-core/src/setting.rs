use crate::kind::Kind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Ffb,
    Rotation,
    Sensitivity,
    TrueForce,
    Pedals,
    Leds,
    Profiles,
    Calibration,
    Info,
}

impl Category {
    pub const ALL: &'static [Category] = &[
        Category::Ffb,
        Category::Rotation,
        Category::Sensitivity,
        Category::TrueForce,
        Category::Pedals,
        Category::Leds,
        Category::Profiles,
        Category::Calibration,
        Category::Info,
    ];
    pub fn label(&self) -> &'static str {
        match self {
            Category::Ffb => "Force feedback",
            Category::Rotation => "Rotation",
            Category::Sensitivity => "Sensitivity",
            Category::TrueForce => "TrueForce",
            Category::Pedals => "Pedals",
            Category::Leds => "LEDs",
            Category::Profiles => "Profiles / mode",
            Category::Calibration => "Calibration",
            Category::Info => "Info",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    ReadWrite,
    ReadOnly,
    Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeReq {
    Any,
    DesktopOnly,
    OnboardOnly,
}

#[derive(Debug, Clone, Copy)]
pub struct SettingSpec {
    pub attr: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub category: Category,
    pub kind: Kind,
    pub access: Access,
    pub mode_req: ModeReq,
}
