//! A one-line text field model for editing box titles in place: caret, selection, word-wise
//! movement and deletion (Ctrl+Backspace / Ctrl+Delete / Ctrl+arrows), as Windows text boxes do.
//! Positions are char indices; `utf16_pos` converts for DirectWrite.

#[derive(Clone, Debug)]
pub struct TextEdit {
    pub text: Vec<char>,
    pub caret: usize,
    pub anchor: usize,
}

#[derive(PartialEq, Clone, Copy)]
enum Class {
    Space,
    Word,
    Punct,
}

fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

impl TextEdit {
    /// The field with all of `s` selected.
    pub fn new(s: &str) -> Self {
        let text: Vec<char> = s.chars().collect();
        TextEdit { caret: text.len(), anchor: 0, text }
    }

    pub fn string(&self) -> String {
        self.text.iter().collect()
    }

    pub fn sel(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }

    pub fn has_sel(&self) -> bool {
        self.caret != self.anchor
    }

    pub fn selected(&self) -> String {
        let (a, b) = self.sel();
        self.text[a..b].iter().collect()
    }

    /// Start of the word before `i` (skips spaces first), as Ctrl+Left / Ctrl+Backspace do.
    pub fn prev_word(&self, i: usize) -> usize {
        let mut i = i;
        while i > 0 && class(self.text[i - 1]) == Class::Space {
            i -= 1;
        }
        if i == 0 {
            return 0;
        }
        let c = class(self.text[i - 1]);
        while i > 0 && class(self.text[i - 1]) == c {
            i -= 1;
        }
        i
    }

    /// Start of the next word after `i` (the word, then the spaces after it), as Ctrl+Right does.
    pub fn next_word(&self, i: usize) -> usize {
        let n = self.text.len();
        let mut i = i;
        if i < n {
            let c = class(self.text[i]);
            if c != Class::Space {
                while i < n && class(self.text[i]) == c {
                    i += 1;
                }
            }
        }
        while i < n && class(self.text[i]) == Class::Space {
            i += 1;
        }
        i
    }

    /// The word (or run of spaces / punctuation) around `i`, for double-click.
    pub fn word_at(&self, i: usize) -> (usize, usize) {
        let n = self.text.len();
        if n == 0 {
            return (0, 0);
        }
        let i = i.min(n - 1);
        let c = class(self.text[i]);
        let (mut a, mut b) = (i, i);
        while a > 0 && class(self.text[a - 1]) == c {
            a -= 1;
        }
        while b < n && class(self.text[b]) == c {
            b += 1;
        }
        (a, b)
    }

    fn delete_sel(&mut self) -> bool {
        if !self.has_sel() {
            return false;
        }
        let (a, b) = self.sel();
        self.text.drain(a..b);
        self.caret = a;
        self.anchor = a;
        true
    }

    pub fn insert(&mut self, s: &str) {
        self.delete_sel();
        let add: Vec<char> = s.chars().filter(|c| !c.is_control()).collect();
        let n = add.len();
        self.text.splice(self.caret..self.caret, add);
        self.caret += n;
        self.anchor = self.caret;
    }

    pub fn backspace(&mut self, word: bool) {
        if self.delete_sel() || self.caret == 0 {
            return;
        }
        let from = if word { self.prev_word(self.caret) } else { self.caret - 1 };
        self.text.drain(from..self.caret);
        self.caret = from;
        self.anchor = from;
    }

    pub fn delete(&mut self, word: bool) {
        if self.delete_sel() || self.caret >= self.text.len() {
            return;
        }
        let to = if word { self.next_word(self.caret) } else { self.caret + 1 };
        self.text.drain(self.caret..to);
        self.anchor = self.caret;
    }

    pub fn move_to(&mut self, i: usize, extend: bool) {
        self.caret = i.min(self.text.len());
        if !extend {
            self.anchor = self.caret;
        }
    }

    pub fn left(&mut self, word: bool, extend: bool) {
        let to = if !extend && !word && self.has_sel() {
            self.sel().0
        } else if word {
            self.prev_word(self.caret)
        } else {
            self.caret.saturating_sub(1)
        };
        self.move_to(to, extend);
    }

    pub fn right(&mut self, word: bool, extend: bool) {
        let to = if !extend && !word && self.has_sel() {
            self.sel().1
        } else if word {
            self.next_word(self.caret)
        } else {
            self.caret + 1
        };
        self.move_to(to, extend);
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
    }

    /// UTF-16 offset of char index `i` (DirectWrite positions are UTF-16).
    pub fn utf16_pos(&self, i: usize) -> u32 {
        self.text[..i.min(self.text.len())].iter().map(|c| c.len_utf16() as u32).sum()
    }

    /// Char index of a UTF-16 offset.
    pub fn char_pos(&self, u: u32) -> usize {
        let mut acc = 0u32;
        for (i, c) in self.text.iter().enumerate() {
            if acc >= u {
                return i;
            }
            acc += c.len_utf16() as u32;
        }
        self.text.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str, caret: usize) -> TextEdit {
        let mut e = TextEdit::new(s);
        e.move_to(caret, false);
        e
    }

    #[test]
    fn ctrl_backspace_deletes_previous_word() {
        let mut e = at("New box", 7);
        e.backspace(true);
        assert_eq!(e.string(), "New ");
        e.backspace(true);
        assert_eq!(e.string(), "");
        let mut e = at("Games  ", 7);
        e.backspace(true);
        assert_eq!(e.string(), "");
        let mut e = at("a.b", 3);
        e.backspace(true);
        assert_eq!(e.string(), "a.");
    }

    #[test]
    fn typing_replaces_selection_and_skips_controls() {
        let mut e = TextEdit::new("New box");
        e.insert("Games\u{7f}");
        assert_eq!(e.string(), "Games");
        assert_eq!(e.caret, 5);
    }

    #[test]
    fn word_moves_and_delete() {
        let mut e = at("one two three", 0);
        e.right(true, false);
        assert_eq!(e.caret, 4);
        e.right(true, true);
        assert_eq!(e.selected(), "two ");
        e.delete(false);
        assert_eq!(e.string(), "one three");
        e.delete(true);
        assert_eq!(e.string(), "one ");
        e.left(true, false);
        assert_eq!(e.caret, 0);
    }

    #[test]
    fn utf16_positions() {
        let e = TextEdit::new("a😀b");
        assert_eq!(e.utf16_pos(2), 3);
        assert_eq!(e.char_pos(3), 2);
    }
}
