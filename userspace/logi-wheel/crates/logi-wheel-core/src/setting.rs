use crate::kind::Kind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Info,
    Ffb,
    Steering,
    Pedals,
    Leds,
    Profiles,
}

impl Category {
    // Info first and default on startup: the app should open by showing
    // what was detected (or that nothing was) before the user moves on to
    // settings. Every consumer (sidebar order, digit-jump numbering, the
    // TUI's `cat_idx: 0` default) derives from this order rather than a
    // literal index, so putting Info first here is the one change that
    // moves it everywhere at once.
    pub const ALL: &'static [Category] = &[
        Category::Info,
        Category::Ffb,
        Category::Steering,
        Category::Pedals,
        Category::Leds,
        Category::Profiles,
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

/// What writing an attribute does to the others.
///
/// Most attributes are independent, and a snapshot can replay them in any
/// order. Four are not, and both of the profile bugs in issue #73 were the
/// same mistake about them: an attribute that selects a store written
/// after the values it overwrites. The light-strip selector had to move
/// last; the onboard-slot selector had to stop being replayed at all. Each
/// was fixed by hand with its own list. This is that knowledge as data, so
/// the replay order follows from the registry and a third case is a
/// one-line classification rather than a bug report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Independent of every other attribute. Replayed in file order.
    Setting,
    /// Chooses which store the wheel runs from (`wheel_mode`,
    /// `wheel_profile`). Writing it makes the wheel reload that store's
    /// values over everything live, so a snapshot neither saves nor
    /// replays it: these snapshots are desktop-mode state, and moving the
    /// wheel onto an onboard slot is the wrong side effect in any order.
    StoreSelector,
    /// Content of a custom light slot. Writing it activates that slot on
    /// the strip, so it must be replayed before the display selector or
    /// it steals the selection.
    SlotContent,
    /// Chooses what the strip displays (`wheel_led_effect`). Replayed
    /// after every [`Role::SlotContent`] write.
    DisplaySelector,
    /// Live content rather than a setting: what the base's screen is
    /// showing this moment (`wheel_oled`). Editable, but a snapshot has no
    /// business capturing or replaying it, any more than it would the
    /// rev-light level.
    Transient,
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

impl SettingSpec {
    /// This attribute's [`Role`]. The classification lives in the registry
    /// (`registry::role_of`), next to the table it describes.
    pub fn role(&self) -> Role {
        crate::registry::role_of(self.attr)
    }
}
