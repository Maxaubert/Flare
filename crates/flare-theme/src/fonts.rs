//! Terminal font faces, sizes and session zoom (Task 8).

/// One terminal face as Prism Terminal lists it (`stack` is the CSS font-family list).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TermFont {
    pub id: &'static str,
    pub name: &'static str,
    pub stack: &'static str,
}
