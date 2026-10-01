//! Which rows of a list, a grid or a tree are chosen, as every desktop's
//! file manager has it: a click chooses one, Ctrl (⌘ on macOS) adds or
//! takes one away, Shift chooses from the last plain click to here; the
//! arrows, Home, End and the page keys move (Shift: and choose on the
//! way), Ctrl+A chooses all, typing a name's first letters goes to it,
//! Enter opens. The views ([`crate::ListView`], [`crate::GridView`]) feed
//! it their rows' keys in the order shown; the program keeps it.

use std::collections::HashSet;
use std::hash::Hash;

/// How long typed letters add up to one name (Explorer's is about this).
const TYPING: f64 = 1.0;

#[derive(Clone, Debug)]
pub struct Selection<K> {
    chosen: HashSet<K>,
    /// Where Shift chooses from.
    anchor: Option<usize>,
    /// Where the keys move from (the row last clicked or moved to).
    cursor: Option<usize>,
    typed: String,
    typed_at: f64,
}

impl<K> Default for Selection<K> {
    fn default() -> Selection<K> {
        Selection { chosen: HashSet::new(), anchor: None, cursor: None, typed: String::new(), typed_at: f64::MIN }
    }
}

/// What the keys did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keyed {
    Nothing,
    /// Moved (or chose) to this row: to be scrolled into view.
    Moved(usize),
    /// Enter: open what is chosen.
    Open,
}

impl<K: Clone + Eq + Hash> Selection<K> {
    pub fn contains(&self, key: &K) -> bool {
        self.chosen.contains(key)
    }

    pub fn is_empty(&self) -> bool {
        self.chosen.is_empty()
    }

    pub fn len(&self) -> usize {
        self.chosen.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &K> {
        self.chosen.iter()
    }

    /// The row the keys move from.
    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    pub fn clear(&mut self) {
        self.chosen.clear();
        self.anchor = None;
        self.cursor = None;
    }

    /// Only `key` (the row at `index`), as a plain click.
    pub fn only(&mut self, key: K, index: usize) {
        self.chosen.clear();
        self.chosen.insert(key);
        self.anchor = Some(index);
        self.cursor = Some(index);
    }

    /// These, whatever was chosen before (a folder's new listing, a
    /// program's own choice); the keys start again from the top.
    pub fn set(&mut self, keys: impl IntoIterator<Item = K>) {
        self.chosen = keys.into_iter().collect();
        self.anchor = None;
        self.cursor = None;
    }

    /// Forgets what is no longer shown (`keys`: the rows now), after a
    /// folder was read again.
    pub fn keep(&mut self, keys: &[K]) {
        let shown: HashSet<&K> = keys.iter().collect();
        self.chosen.retain(|k| shown.contains(k));
        let last = keys.len().checked_sub(1);
        self.anchor = self.anchor.zip(last).map(|(a, l)| a.min(l));
        self.cursor = self.cursor.zip(last).map(|(c, l)| c.min(l));
    }

    /// A click on the row at `index` of `keys` (the rows in the order
    /// shown) with these modifiers.
    pub fn click(&mut self, keys: &[K], index: usize, modifiers: egui::Modifiers) {
        let Some(key) = keys.get(index) else { return };
        if modifiers.shift {
            let from = self.anchor.unwrap_or(index).min(keys.len() - 1);
            if !modifiers.command {
                self.chosen.clear();
            }
            let (a, b) = (from.min(index), from.max(index));
            self.chosen.extend(keys[a..=b].iter().cloned());
            self.cursor = Some(index);
        } else if modifiers.command {
            if !self.chosen.remove(key) {
                self.chosen.insert(key.clone());
            }
            self.anchor = Some(index);
            self.cursor = Some(index);
        } else {
            self.only(key.clone(), index);
        }
    }

    /// A right click on the row at `index`: it is chosen (alone) unless it
    /// already is, so the menu is about it.
    pub fn context_click(&mut self, keys: &[K], index: usize) {
        if let Some(key) = keys.get(index).filter(|k| !self.chosen.contains(k)) {
            self.only(key.clone(), index);
        }
    }

    /// The keys pressed this frame, for rows `keys` laid out `across` to a
    /// line (1 for a list), `page` rows to a screen; `name(i)` is what
    /// typing finds the row at `i` by.
    pub fn keys(
        &mut self,
        input: &egui::InputState,
        keys: &[K],
        across: usize,
        page: usize,
        name: impl Fn(usize) -> String,
    ) -> Keyed {
        if keys.is_empty() {
            return Keyed::Nothing;
        }
        let last = keys.len() - 1;
        let across = across.max(1);
        let mods = input.modifiers;
        if mods.command && input.key_pressed(egui::Key::A) {
            self.chosen = keys.iter().cloned().collect();
            return Keyed::Nothing;
        }
        if input.key_pressed(egui::Key::Enter) && !self.chosen.is_empty() {
            return Keyed::Open;
        }
        let at = self.cursor.map(|c| c.min(last));
        let step = |back: bool, by: usize| match at {
            None => 0,
            Some(c) if back => c.saturating_sub(by),
            Some(c) => (c + by).min(last),
        };
        let pressed = |key| input.key_pressed(key);
        let to = if pressed(egui::Key::ArrowDown) {
            Some(step(false, across))
        } else if pressed(egui::Key::ArrowUp) {
            Some(step(true, across))
        } else if across > 1 && pressed(egui::Key::ArrowRight) {
            Some(step(false, 1))
        } else if across > 1 && pressed(egui::Key::ArrowLeft) {
            Some(step(true, 1))
        } else if pressed(egui::Key::PageDown) {
            Some(step(false, page.max(1)))
        } else if pressed(egui::Key::PageUp) {
            Some(step(true, page.max(1)))
        } else if pressed(egui::Key::Home) {
            Some(0)
        } else if pressed(egui::Key::End) {
            Some(last)
        } else {
            self.typed(input, at, keys.len(), &name)
        };
        let Some(to) = to else { return Keyed::Nothing };
        if mods.shift {
            let from = self.anchor.unwrap_or(at.unwrap_or(to));
            self.chosen.clear();
            let (a, b) = (from.min(to), from.max(to));
            self.chosen.extend(keys[a..=b].iter().cloned());
            self.anchor = Some(from);
            self.cursor = Some(to);
        } else {
            self.only(keys[to].clone(), to);
        }
        Keyed::Moved(to)
    }

    /// Letters typed: the next row whose name starts with them.
    fn typed(
        &mut self,
        input: &egui::InputState,
        at: Option<usize>,
        count: usize,
        name: &dyn Fn(usize) -> String,
    ) -> Option<usize> {
        if input.modifiers.command || input.modifiers.alt {
            return None;
        }
        let text: String = input
            .events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        if text.trim().is_empty() {
            return None;
        }
        if input.time - self.typed_at > TYPING {
            self.typed.clear();
        }
        self.typed_at = input.time;
        self.typed.push_str(&text.to_lowercase());
        // one letter again and again: the next with it; more: from here
        let same = self.typed.chars().all(|c| self.typed.starts_with(c));
        let wanted =
            if same { &self.typed[..self.typed.chars().next().map_or(0, char::len_utf8)] } else { &self.typed };
        let start = match at {
            Some(c) if same => c + 1,
            Some(c) => c,
            None => 0,
        };
        (0..count).map(|i| (start + i) % count).find(|&i| name(i).to_lowercase().starts_with(wanted))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> Vec<u32> {
        (0..6).collect()
    }

    fn chosen(s: &Selection<u32>) -> Vec<u32> {
        let mut v: Vec<u32> = s.iter().copied().collect();
        v.sort();
        v
    }

    const PLAIN: egui::Modifiers = egui::Modifiers::NONE;
    const SHIFT: egui::Modifiers = egui::Modifiers::SHIFT;
    const COMMAND: egui::Modifiers = egui::Modifiers::COMMAND;

    #[test]
    fn clicks_choose_as_a_file_manager_does() {
        let (k, mut s) = (keys(), Selection::default());
        s.click(&k, 1, PLAIN);
        assert_eq!(chosen(&s), [1]);
        s.click(&k, 4, SHIFT);
        assert_eq!(chosen(&s), [1, 2, 3, 4], "from the plain click");
        s.click(&k, 2, COMMAND);
        assert_eq!(chosen(&s), [1, 3, 4], "taken away");
        s.click(&k, 0, SHIFT);
        assert_eq!(chosen(&s), [0, 1, 2], "from the Ctrl click, the rest let go");
        s.context_click(&k, 1);
        assert_eq!(chosen(&s), [0, 1, 2], "a chosen row keeps the rest");
        s.context_click(&k, 5);
        assert_eq!(chosen(&s), [5]);
    }

    fn input(key: Option<egui::Key>, modifiers: egui::Modifiers, text: &str, time: f64) -> egui::InputState {
        let mut events = vec![egui::Event::ModifiersChanged(modifiers)];
        if let Some(key) = key {
            events.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
        }
        if !text.is_empty() {
            events.push(egui::Event::Text(text.into()));
        }
        let raw = egui::RawInput { events, time: Some(time), ..Default::default() };
        let mut state = egui::InputState::default();
        state = state.begin_pass(raw, false, 1.0, Default::default());
        state
    }

    #[test]
    fn keys_move_and_choose() {
        let (k, mut s) = (keys(), Selection::default());
        let none = |_| String::new();
        let key = |key, m| input(Some(key), m, "", 0.0);
        assert_eq!(s.keys(&key(egui::Key::ArrowDown, PLAIN), &k, 1, 3, none), Keyed::Moved(0), "from nothing: the top");
        assert_eq!(s.keys(&key(egui::Key::ArrowDown, PLAIN), &k, 1, 3, none), Keyed::Moved(1));
        assert_eq!(s.keys(&key(egui::Key::PageDown, SHIFT), &k, 1, 3, none), Keyed::Moved(4));
        assert_eq!(chosen(&s), [1, 2, 3, 4]);
        assert_eq!(s.keys(&key(egui::Key::End, PLAIN), &k, 1, 3, none), Keyed::Moved(5));
        assert_eq!(chosen(&s), [5]);
        assert_eq!(s.keys(&key(egui::Key::ArrowUp, PLAIN), &k, 2, 3, none), Keyed::Moved(3), "a grid two across");
        assert_eq!(s.keys(&key(egui::Key::ArrowLeft, PLAIN), &k, 2, 3, none), Keyed::Moved(2));
        assert_eq!(s.keys(&key(egui::Key::A, COMMAND), &k, 1, 3, none), Keyed::Nothing);
        assert_eq!(chosen(&s), [0, 1, 2, 3, 4, 5]);
        assert_eq!(s.keys(&key(egui::Key::Enter, PLAIN), &k, 1, 3, none), Keyed::Open);
    }

    #[test]
    fn typing_goes_to_a_name() {
        let names = ["boot", "Bin", "etc", "efi", "var", "vmlinuz"];
        let (k, mut s) = (keys(), Selection::default());
        let name = |i: usize| names[i].to_string();
        let typed = |t, time| input(None, PLAIN, t, time);
        assert_eq!(s.keys(&typed("v", 0.0), &k, 1, 3, name), Keyed::Moved(4));
        assert_eq!(s.keys(&typed("m", 0.3), &k, 1, 3, name), Keyed::Moved(5), "vm: from here");
        assert_eq!(s.keys(&typed("b", 5.0), &k, 1, 3, name), Keyed::Moved(0), "later: a new name, round the end");
        assert_eq!(s.keys(&typed("b", 5.2), &k, 1, 3, name), Keyed::Moved(1), "the same letter: the next");
        assert_eq!(s.keys(&typed("x", 9.0), &k, 1, 3, name), Keyed::Nothing);
    }
}
