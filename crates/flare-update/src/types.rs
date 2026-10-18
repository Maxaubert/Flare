//! M9 Task 4: the offer and check types.

/// An update on offer. `notes` is the raw release body (the parser in `notes.rs` reads it).
/// A preview offer is a mock: it has token 0 and names no download.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub version: String,
    pub notes: String,
    pub mock: bool,
    pub token: u64,
}

/// What the update engine is doing with the offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Idle,
    Downloading,
    Installing,
}
