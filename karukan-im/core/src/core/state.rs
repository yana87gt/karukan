//! Input state machine
//!
//! Defines the states of the IME and transitions between them.

use super::candidate::{CandidateList, CandidateSource};
use super::preedit::Preedit;

/// How a Conversion state came about, which decides what Space does in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConversionStage {
    /// The usual case: Space steps to the next candidate.
    #[default]
    Open,
    /// Tab stepped into the prediction rows of the window that was showing
    /// while typing. Space leaves them for the conversion of exactly what
    /// was typed.
    Predicted,
}

/// The current state of the IME
#[derive(Debug, Clone, Default)]
pub enum InputState {
    /// No input, waiting for user to type
    #[default]
    Empty,

    /// Composing mode - building preedit text (hiragana, katakana, or alphabet)
    Composing {
        /// The preedit string being composed
        preedit: Preedit,
    },

    /// Conversion mode - selecting from candidates
    Conversion {
        /// The preedit string showing conversion result
        preedit: Preedit,
        /// List of conversion candidates (possibly source-filtered)
        candidates: CandidateList,
        /// The (settled) reading the conversion was built from
        reading: String,
        /// Active Ctrl+R source filter; `None` shows the full list
        filter: Option<CandidateSource>,
        /// Where the conversion came from; see [`ConversionStage`]
        stage: ConversionStage,
    },
}

impl InputState {
    /// Check if the engine is in the Empty (idle) state
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    /// Get the current preedit if any
    pub fn preedit(&self) -> Option<&Preedit> {
        match self {
            Self::Empty => None,
            Self::Composing { preedit, .. } => Some(preedit),
            Self::Conversion { preedit, .. } => Some(preedit),
        }
    }

    /// Get mutable reference to preedit
    pub fn preedit_mut(&mut self) -> Option<&mut Preedit> {
        match self {
            Self::Empty => None,
            Self::Composing { preedit, .. } => Some(preedit),
            Self::Conversion { preedit, .. } => Some(preedit),
        }
    }

    /// Get the active source filter in conversion state
    pub fn filter(&self) -> Option<CandidateSource> {
        match self {
            Self::Conversion { filter, .. } => *filter,
            _ => None,
        }
    }

    /// The presentation stage in the Conversion state (`Open` elsewhere).
    pub fn stage(&self) -> ConversionStage {
        match self {
            Self::Conversion { stage, .. } => *stage,
            _ => ConversionStage::Open,
        }
    }

    /// The reading a conversion was built from, if in the Conversion state.
    pub fn reading(&self) -> Option<&str> {
        match self {
            Self::Conversion { reading, .. } => Some(reading),
            _ => None,
        }
    }

    /// Get candidates in conversion state
    pub fn candidates(&self) -> Option<&CandidateList> {
        match self {
            Self::Conversion { candidates, .. } => Some(candidates),
            _ => None,
        }
    }

    /// Get mutable reference to candidates
    pub fn candidates_mut(&mut self) -> Option<&mut CandidateList> {
        match self {
            Self::Conversion { candidates, .. } => Some(candidates),
            _ => None,
        }
    }
}
