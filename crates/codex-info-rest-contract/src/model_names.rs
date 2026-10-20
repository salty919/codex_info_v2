//! Session model normalization defined by DATA RECORDER-MODEL-01.
//! Wire validation remains separate: old recorded labels are not rewritten.

use crate::MAX_PUBLIC_MODEL_SCALARS;

fn forbidden(character: char) -> bool {
    matches!(character, '\u{0000}'..='\u{001f}' | '\u{007f}'..='\u{009f}')
        || crate::is_bidi_formatting(character)
}

fn white_space(character: char) -> bool {
    matches!(character,
        '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{0085}' | '\u{00a0}' |
        '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}'..='\u{2029}' |
        '\u{202f}' | '\u{205f}' | '\u{3000}')
}

pub fn normalize_model_name(original: &str) -> Option<&str> {
    if original.chars().any(forbidden) {
        return None;
    }
    let normalized = original.trim_matches(white_space);
    (!normalized.is_empty() && normalized.chars().count() <= MAX_PUBLIC_MODEL_SCALARS)
        .then_some(normalized)
}

/// Keeps only a bounded normalized name and a bounded possible interior
/// whitespace suffix. Arbitrarily long outer whitespace is never truncated
/// into a different name, and forbidden characters are checked before trim.
#[derive(Default)]
pub struct ModelNameBuilder {
    normalized: String,
    pending_white_space: String,
    scalars: usize,
    pending_scalars: usize,
    invalid: bool,
}

impl ModelNameBuilder {
    pub fn push(&mut self, character: char) {
        if forbidden(character) {
            self.invalid = true;
        }
        if self.invalid {
            return;
        }
        if white_space(character) {
            if self.scalars > 0 {
                self.pending_scalars = self
                    .pending_scalars
                    .saturating_add(1)
                    .min(MAX_PUBLIC_MODEL_SCALARS + 1);
                if self.scalars + self.pending_scalars <= MAX_PUBLIC_MODEL_SCALARS {
                    self.pending_white_space.push(character);
                }
            }
            return;
        }
        let next = self.scalars + self.pending_scalars + 1;
        if next > MAX_PUBLIC_MODEL_SCALARS {
            self.invalid = true;
            return;
        }
        self.normalized.push_str(&self.pending_white_space);
        self.normalized.push(character);
        self.pending_white_space.clear();
        self.pending_scalars = 0;
        self.scalars = next;
    }

    pub fn finish(self) -> Option<String> {
        (!self.invalid && self.scalars > 0).then_some(self.normalized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_uses_the_fixed_scalar_and_character_contract() {
        fn check(original: &str, expected: Option<&str>) {
            assert_eq!(
                normalize_model_name(original),
                expected,
                "direct: {original:?}"
            );
            let mut builder = ModelNameBuilder::default();
            for character in original.chars() {
                builder.push(character);
            }
            assert_eq!(
                builder.finish().as_deref(),
                expected,
                "stream: {original:?}"
            );
        }
        check("", None);
        check(" \u{3000}", None);
        check(&"a".repeat(128), Some(&"a".repeat(128)));
        check(&"a".repeat(129), None);
        check(&"\u{1f680}".repeat(128), Some(&"\u{1f680}".repeat(128)));
        check(&"\u{1f680}".repeat(129), None);
        check("\u{a0}e\u{301}\u{2003}X\u{3000}", Some("e\u{301}\u{2003}X"));
        check("\u{200b}X\u{feff}", Some("\u{200b}X\u{feff}"));
        check(
            &format!("{}gpt-6.1-sol{}", "\u{3000}".repeat(4096), " ".repeat(4096)),
            Some("gpt-6.1-sol"),
        );
        check(&format!("a{}b", " ".repeat(127)), None);
        check(
            &format!("a{}b", " ".repeat(126)),
            Some(&format!("a{}b", " ".repeat(126))),
        );
        for codepoint in (0..=0x1f).chain(0x7f..=0x9f).chain([
            0x61c, 0x200e, 0x200f, 0x202a, 0x202b, 0x202c, 0x202d, 0x202e, 0x2066, 0x2067, 0x2068,
            0x2069,
        ]) {
            let character = char::from_u32(codepoint).unwrap();
            check(&format!("{character}gpt-6.1-sol"), None);
            check(&format!("gpt-6.1-sol{character}"), None);
        }
    }
}
