//! Input C1: replies and clipboard scenarios (Task 31): termReplies, termClipboard, bell, hiddenResize. Each is a stub until its port task fills it.

use crate::ctx::Ctx;

/// Stub until Task 31 ports "termReplies".
pub fn term_replies(ctx: &mut Ctx) -> Result<(), String> {
    ctx.not_ported(31)
}

/// Stub until Task 31 ports "termClipboard".
pub fn term_clipboard(ctx: &mut Ctx) -> Result<(), String> {
    ctx.not_ported(31)
}

/// Stub until Task 31 ports "bell".
pub fn bell(ctx: &mut Ctx) -> Result<(), String> {
    ctx.not_ported(31)
}

/// Stub until Task 31 ports "hiddenResize".
pub fn hidden_resize(ctx: &mut Ctx) -> Result<(), String> {
    ctx.not_ported(31)
}
