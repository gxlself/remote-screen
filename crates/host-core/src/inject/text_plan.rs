//! Turn already-committed text into macOS injection steps.
//!
//! Text is split with [`utf16_chunks`], then overlong grapheme clusters are
//! split at code point boundaries so each `Unicode` step carries at most
//! [`MAX_UNITS_PER_EVENT`] UTF-16 code units without splitting surrogate pairs.
//! Normal grapheme clusters stay intact. Line breaks and tabs are
//! represented as real key presses instead. Other C0 controls, DEL and C1
//! controls are discarded: this text has already been committed by the phone's
//! input method, so it must not be able to trigger editing or escape sequences.
//!
//! The macOS layer must post every `Unicode` step with virtual key 0 and clear
//! flags, even while a modifier is physically held. Otherwise Command+text can
//! become a keyboard shortcut instead of literal text. `Key` steps also use
//! empty flags, so Return and Tab are unmodified key presses.

use crate::inject::text::utf16_chunks;

/// Maximum UTF-16 code units carried by one text event (06 · 输入注入).
pub const MAX_UNITS_PER_EVENT: usize = 20;
/// macOS virtual keycode for Return (`kVK_Return`).
pub const VK_RETURN: u16 = 0x24;
/// macOS virtual keycode for Tab (`kVK_Tab`).
pub const VK_TAB: u16 = 0x30;

/// One keyDown/keyUp pair for the macOS injector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextStep {
    /// One keyDown/keyUp pair; both events carry this UTF-16 string.
    Unicode(Vec<u16>),
    /// One unmodified keyDown/keyUp pair for this virtual keycode.
    Key(u16),
}

/// Ordered injection steps and counters for one text message.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextPlan {
    pub steps: Vec<TextStep>,
    pub dropped_controls: usize,
    pub utf16_units: usize,
}

/// Plan a text message using the standard 20-unit event limit.
pub fn plan_text(text: &str) -> TextPlan {
    plan_text_with_limit(text, MAX_UNITS_PER_EVENT)
}

/// Plan a text message, splitting text chunks at `max_units` UTF-16 units.
///
/// Extended grapheme clusters stay intact when they fit. Overlong clusters
/// are split greedily at code point boundaries into separate `Unicode` steps,
/// without merging with adjacent chunks or splitting surrogate pairs. For
/// this fallback only, the limit is `max(max_units, 2)`, so limits zero and
/// one can still hold a supplementary-plane character.
///
/// CRLF is one Return press. A lone CR, LF, U+2028 or U+2029 is also one
/// Return; Tab is one Tab. Other C0 controls (including Backspace and Escape),
/// DEL and C1 controls are dropped without breaking the surrounding text run.
pub fn plan_text_with_limit(text: &str, max_units: usize) -> TextPlan {
    let mut plan = TextPlan::default();
    let mut run = String::new();
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                flush_run(&mut plan, &mut run, max_units);
                plan.steps.push(TextStep::Key(VK_RETURN));
            }
            '\n' | '\u{2028}' | '\u{2029}' => {
                flush_run(&mut plan, &mut run, max_units);
                plan.steps.push(TextStep::Key(VK_RETURN));
            }
            '\t' => {
                flush_run(&mut plan, &mut run, max_units);
                plan.steps.push(TextStep::Key(VK_TAB));
            }
            '\u{0000}'..='\u{001f}' | '\u{007f}' | '\u{0080}'..='\u{009f}' => {
                plan.dropped_controls += 1;
            }
            _ => run.push(ch),
        }
    }

    flush_run(&mut plan, &mut run, max_units);
    plan
}

fn flush_run(plan: &mut TextPlan, run: &mut String, max_units: usize) {
    let limit = max_units.max(2);
    for chunk in utf16_chunks(run, max_units) {
        plan.utf16_units += chunk.len();
        if chunk.len() <= limit {
            plan.steps.push(TextStep::Unicode(chunk));
            continue;
        }

        let mut start = 0;
        while start < chunk.len() {
            let mut end = start.saturating_add(limit).min(chunk.len());
            // A high surrogate must remain with its following low surrogate.
            if end < chunk.len() && (0xd800..=0xdbff).contains(&chunk[end - 1]) {
                end -= 1;
            }
            plan.steps
                .push(TextStep::Unicode(chunk[start..end].to_vec()));
            start = end;
        }
    }
    run.clear();
}
