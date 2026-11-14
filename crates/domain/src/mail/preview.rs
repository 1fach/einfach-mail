use std::fmt;

/// The plain-text opening of a message body, normalized for a list row.
///
/// Every value holds 1 to [`Self::MAX_CHARS`] characters, has no control, bidirectional
/// control, or invisible padding characters, separates words with single spaces, and has no
/// leading or trailing space.
#[derive(Clone, Eq, PartialEq)]
pub struct MessagePreview(Box<str>);

impl MessagePreview {
    /// The most characters a preview holds.
    pub const MAX_CHARS: usize = 256;

    /// Normalizes untrusted provider or cached text into a preview.
    ///
    /// Control characters, line and paragraph separators, and Unicode whitespace become one
    /// space, and runs of them collapse. Bidirectional controls and invisible padding
    /// characters are dropped. The result is trimmed, and reading stops once
    /// [`Self::MAX_CHARS`] characters are produced. Returns `None` when no printable
    /// character remains.
    ///
    /// Normalizing a preview's own text returns an equal preview.
    #[must_use]
    pub fn from_untrusted(text: &str) -> Option<Self> {
        let mut normalized = String::new();
        let mut length = 0;
        let mut pending_space = false;
        for character in text.chars() {
            match classify(character) {
                CharacterClass::Dropped => {}
                CharacterClass::Space => pending_space = length > 0,
                CharacterClass::Kept => {
                    let needed = if pending_space { 2 } else { 1 };
                    if length + needed > Self::MAX_CHARS {
                        break;
                    }
                    if pending_space {
                        normalized.push(' ');
                        pending_space = false;
                    }
                    normalized.push(character);
                    length += needed;
                }
            }
        }
        (!normalized.is_empty()).then(|| Self(normalized.into_boxed_str()))
    }

    /// Returns the normalized text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for MessagePreview {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MessagePreview(<redacted>)")
    }
}

enum CharacterClass {
    Kept,
    Space,
    Dropped,
}

fn classify(character: char) -> CharacterClass {
    match character {
        // Bidirectional controls, which could reorder the visible text of a row.
        '\u{061C}' | '\u{200E}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
        // Invisible padding that marketing mail adds to a preheader.
        | '\u{00AD}' | '\u{034F}' | '\u{200B}' | '\u{200C}' | '\u{2060}' | '\u{FEFF}' => {
            CharacterClass::Dropped
        }
        '\u{2028}' | '\u{2029}' => CharacterClass::Space,
        character if character.is_control() || character.is_whitespace() => CharacterClass::Space,
        _ => CharacterClass::Kept,
    }
}

#[cfg(test)]
mod tests;
