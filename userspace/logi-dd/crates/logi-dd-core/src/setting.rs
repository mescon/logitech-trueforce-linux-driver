use crate::kind::Kind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Ffb,
    Steering,
    Pedals,
    Leds,
    Profiles,
    Info,
}

impl Category {
    pub const ALL: &'static [Category] = &[
        Category::Ffb,
        Category::Steering,
        Category::Pedals,
        Category::Leds,
        Category::Profiles,
        Category::Info,
    ];
    pub fn label(&self) -> &'static str {
        match self {
            // Ffb folds in TrueForce (a haptic layer of the same force path).
            Category::Ffb => "Force feedback",
            // Steering folds in the old Rotation, Sensitivity and Calibration:
            // range, response curve, sensitivity and centre calibration are all
            // the one steering axis.
            Category::Steering => "Steering",
            Category::Pedals => "Pedals",
            Category::Leds => "LIGHTSYNC",
            Category::Profiles => "Profiles / mode",
            // The page carries the live input monitor and the force
            // simulations alongside the identity rows, so say so.
            Category::Info => "Info / Testing",
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
