//! Unified game-discovery data model. Each launcher backend (Steam, Lutris
//! here; Heroic follows) scans its own install for games and reports them
//! as [`DiscoveredGame`]s; an aggregator merges the backends' results for
//! the Setup pages, which offer a shim install for any Wine game.

pub mod lutris;
pub mod steam;

use std::path::{Path, PathBuf};

/// Which launcher (or manual entry) reported a [`DiscoveredGame`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Steam,
    Lutris,
    Heroic,
    Manual,
}

impl Source {
    /// The label the front-ends show next to a discovered game.
    pub fn label(self) -> &'static str {
        match self {
            Source::Steam => "Steam",
            Source::Lutris => "Lutris",
            Source::Heroic => "Heroic",
            Source::Manual => "Manual",
        }
    }
}

/// How a game runs: a native Linux build, or a Windows build under Wine in
/// the given prefix (the shim installer's target).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameKind {
    Native,
    Wine { prefix: PathBuf },
}

/// One game found by a launcher backend (or entered manually), with enough
/// information for the Setup pages to offer a shim install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredGame {
    pub name: String,
    pub source: Source,
    pub kind: GameKind,
    /// Whether the TrueForce SDK shim's marker DLL is present in the game's
    /// wine prefix. Always `false` for [`GameKind::Native`]: the shim is
    /// Wine-only, there is no prefix to install it into.
    pub shim_installed: bool,
}

impl DiscoveredGame {
    /// The wine prefix backing this game, or `None` for a native game.
    pub fn prefix(&self) -> Option<&Path> {
        match &self.kind {
            GameKind::Wine { prefix } => Some(prefix),
            GameKind::Native => None,
        }
    }
}
