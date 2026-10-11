//! Candidate list management
//!
//! Handles the list of conversion candidates with pagination support.

/// Source of a conversion candidate — which subsystem produced it.
/// Presentation ([`label`](Self::label), [`is_deletable`](Self::is_deletable))
/// is derived from this on read, never stored, so it can't fall out of sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateSource {
    /// User dictionary lookup
    UserDictionary,
    /// Learning cache (user history)
    Learning,
    /// Model inference result
    Model,
    /// System dictionary lookup (also covers reading→symbol lookups via
    /// mozc's symbol.tsv — they're treated as just another dictionary).
    Dictionary,
    /// Rewriter-generated variant (half-width katakana, symbol)
    Rewriter,
    /// Date/time candidate rendered from the clock (`[date]` phrases)
    Date,
    /// Hiragana/katakana fallback
    Fallback,
}

impl CandidateSource {
    /// Aux-text label telling the user which subsystem produced the
    /// candidate. Empty for sources that aren't worth calling out (Fallback).
    pub fn label(&self) -> &'static str {
        match self {
            CandidateSource::UserDictionary => "\u{1F464} \u{30E6}\u{30FC}\u{30B6}\u{30FC}", // 👤 ユーザー
            CandidateSource::Learning => "\u{1F4DD} \u{5B66}\u{7FD2}", // 📝 学習
            CandidateSource::Model => "\u{1F916} AI",                  // 🤖 AI
            CandidateSource::Dictionary => "\u{1F4DA} \u{8F9E}\u{66F8}", // 📚 辞書
            CandidateSource::Rewriter => "\u{1F504} \u{5909}\u{63DB}", // 🔄 変換
            CandidateSource::Date => "\u{1F4C5} \u{65E5}\u{4ED8}",     // 📅 日付
            CandidateSource::Fallback => "",
        }
    }

    /// Short glyph for tight spots like the Ctrl+R filter header in the
    /// conversion aux text. Empty for Fallback, which has no view of its
    /// own — its kana ride at the tail of the rewriter view.
    pub fn emoji(&self) -> &'static str {
        match self {
            CandidateSource::UserDictionary => "\u{1F464}", // 👤
            CandidateSource::Learning => "\u{1F4DD}",       // 📝
            CandidateSource::Model => "\u{1F916}",          // 🤖
            CandidateSource::Dictionary => "\u{1F4DA}",     // 📚
            CandidateSource::Rewriter => "\u{1F504}",       // 🔄
            CandidateSource::Date => "\u{1F4C5}",           // 📅
            CandidateSource::Fallback => "",
        }
    }

    /// Whether candidates from this source can be removed from the learning
    /// history with Ctrl+Backspace / Ctrl+Delete.
    pub fn is_deletable(&self) -> bool {
        matches!(self, CandidateSource::Learning)
    }

    /// Whether committing a candidate from this source is recorded in the
    /// learning cache. Date/time candidates are rendered from the clock, so
    /// a recorded one would resurface later as a stale date.
    pub fn is_learnable(&self) -> bool {
        !matches!(self, CandidateSource::Date)
    }
}

/// A single conversion candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The converted text
    pub text: String,
    /// The original reading (hiragana)
    pub reading: Option<String>,
    /// Which subsystem produced this candidate, if known.
    pub source: Option<CandidateSource>,
    /// Per-candidate description shown as the right-side comment on the
    /// candidate (mozc-style). Only set when the candidate itself has a
    /// meaningful description — symbol descriptions like `三点リーダ`,
    /// rewriter descriptions like `[全]英大文字`. Source labels are
    /// intentionally excluded so they don't duplicate the aux text.
    pub description: Option<String>,
}

impl Candidate {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            reading: None,
            source: None,
            description: None,
        }
    }

    pub fn with_reading(text: impl Into<String>, reading: impl Into<String>) -> Self {
        Self {
            reading: Some(reading.into()),
            ..Self::new(text)
        }
    }

    /// Aux-text source label (`🤖 AI`, `📚 辞書`, ...) derived from `source`;
    /// `None` when the source is unknown or has no label.
    pub fn source_label(&self) -> Option<&'static str> {
        self.source.map(|s| s.label()).filter(|l| !l.is_empty())
    }

    /// Whether this candidate can be removed from the learning history with
    /// Ctrl+Backspace / Ctrl+Delete. See [`CandidateSource::is_deletable`].
    pub fn is_deletable(&self) -> bool {
        self.source.is_some_and(|s| s.is_deletable())
    }
}

/// A list of candidates with pagination and selection support
#[derive(Debug, Clone)]
pub struct CandidateList {
    /// All candidates
    candidates: Vec<Candidate>,
    /// Currently selected candidate index
    cursor: usize,
    /// Number of candidates per page
    page_size: usize,
    /// Index of the first row of the list's second section, when the window
    /// is drawn in two (predictions above, conversions below). `Some(0)` is
    /// a list whose first section is empty.
    divider: Option<usize>,
    /// The window is shown with no row highlighted: nothing is selected yet,
    /// so Enter still commits the composition. Any cursor move selects.
    unselected: bool,
    /// The row a conversion starts on, kept with the list so that leaving
    /// the predictions for the conversion lands where Space would have.
    answer: usize,
}

impl CandidateList {
    /// Default page size for candidate display
    pub const DEFAULT_PAGE_SIZE: usize = 9;

    /// Create a new candidate list
    pub fn new(candidates: Vec<Candidate>) -> Self {
        Self {
            candidates,
            cursor: 0,
            page_size: Self::DEFAULT_PAGE_SIZE,
            divider: None,
            unselected: false,
            answer: 0,
        }
    }

    /// Record the row a conversion starts on and select it.
    pub fn with_answer(mut self, answer: usize) -> Self {
        self.set_cursor(answer);
        self.answer = self.cursor;
        self
    }

    /// The row a conversion starts on (see [`Self::with_answer`]).
    pub fn answer(&self) -> usize {
        self.answer
    }

    /// Mark where the second section starts (see [`Self::divider`]).
    pub fn with_divider(mut self, divider: usize) -> Self {
        self.divider = Some(divider.min(self.candidates.len()));
        self
    }

    /// Show the list with no row highlighted until the cursor moves.
    pub fn with_no_selection(mut self) -> Self {
        self.unselected = true;
        self.cursor = 0;
        self
    }

    /// The row a forward wrap lands on: the head of the second section when
    /// the list has two. Stepping on past the end then returns to the
    /// conversion rows, and only stepping back up enters the predictions.
    fn wrap_start(&self) -> usize {
        self.divider
            .filter(|d| *d < self.candidates.len() && *d < self.page_size)
            .unwrap_or(0)
    }

    /// Whether a row is selected (false for a list shown without highlight).
    pub fn has_selection(&self) -> bool {
        !self.unselected
    }

    /// Index of the first row of the second section, if the list has two.
    pub fn divider(&self) -> Option<usize> {
        self.divider
    }

    /// The divider as a row index within the current page, only when it
    /// separates two non-empty parts of that page.
    pub fn page_divider(&self) -> Option<usize> {
        let start = self.page_start();
        let end = start + self.page_candidates().len();
        self.divider
            .filter(|d| (start + 1..end).contains(d))
            .map(|d| d - start)
    }

    /// The row on the current page (1-based, as [`Self::select_on_page`]
    /// takes it) that the window labels `number`. A two-part page numbers
    /// each part from 1, so the number is read in the part holding the
    /// highlight, and in the second part while nothing is highlighted: its
    /// rows are the ones Space opens on, so a number means the same row
    /// before and after Space.
    pub fn page_row_numbered(&self, number: usize) -> Option<usize> {
        let Some(divider) = self.page_divider() else {
            return Some(number);
        };
        if number == 0 {
            return None;
        }
        if self.has_selection() && self.page_cursor() < divider {
            (number <= divider).then_some(number)
        } else {
            Some(divider + number)
        }
    }

    /// Create a candidate list from strings (test fixture).
    #[cfg(test)]
    pub fn from_strings(strings: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::new(strings.into_iter().map(Candidate::new).collect())
    }

    /// Get all candidates
    pub fn candidates(&self) -> &[Candidate] {
        &self.candidates
    }

    /// Get the number of candidates
    pub fn len(&self) -> usize {
        self.candidates.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    /// Get the current cursor position
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Get the page size
    pub fn page_size(&self) -> usize {
        self.page_size
    }

    /// Get the current page number (0-indexed)
    pub fn current_page(&self) -> usize {
        self.cursor.checked_div(self.page_size).unwrap_or(0)
    }

    /// Get the total number of pages
    pub fn total_pages(&self) -> usize {
        if self.page_size == 0 || self.candidates.is_empty() {
            0
        } else {
            self.candidates.len().div_ceil(self.page_size)
        }
    }

    /// Get the start index of the current page
    pub fn page_start(&self) -> usize {
        self.current_page() * self.page_size
    }

    /// Get the candidates for the current page
    pub fn page_candidates(&self) -> &[Candidate] {
        let start = self.page_start();
        let end = (start + self.page_size).min(self.candidates.len());
        &self.candidates[start..end]
    }

    /// Get the cursor position within the current page (0-indexed)
    pub fn page_cursor(&self) -> usize {
        self.cursor - self.page_start()
    }

    /// Get the currently selected candidate
    pub fn selected(&self) -> Option<&Candidate> {
        self.candidates.get(self.cursor)
    }

    /// Get the currently selected text
    pub fn selected_text(&self) -> Option<&str> {
        self.selected().map(|c| c.text.as_str())
    }

    /// Move to the next candidate
    pub fn move_next(&mut self) -> bool {
        self.unselected = false;
        if self.cursor + 1 < self.candidates.len() {
            self.cursor += 1;
            true
        } else if !self.candidates.is_empty() {
            // Wrap to beginning
            self.cursor = self.wrap_start();
            true
        } else {
            false
        }
    }

    /// Move to the previous candidate
    pub fn move_prev(&mut self) -> bool {
        self.unselected = false;
        if self.cursor > 0 {
            self.cursor -= 1;
            true
        } else if !self.candidates.is_empty() {
            // Wrap to end
            self.cursor = self.candidates.len() - 1;
            true
        } else {
            false
        }
    }

    /// Move to the next page
    pub fn next_page(&mut self) -> bool {
        self.unselected = false;
        if self.candidates.is_empty() {
            return false;
        }

        let next_page_start = self.page_start() + self.page_size;
        if next_page_start < self.candidates.len() {
            self.cursor = next_page_start;
            true
        } else {
            // Wrap to first page
            self.cursor = self.wrap_start();
            true
        }
    }

    /// Move to the previous page
    pub fn prev_page(&mut self) -> bool {
        self.unselected = false;
        if self.candidates.is_empty() {
            return false;
        }

        let current_page = self.current_page();
        if current_page > 1 {
            self.cursor = (current_page - 1) * self.page_size;
            true
        } else if current_page == 1 {
            self.cursor = self.wrap_start();
            true
        } else {
            // Wrap to last page
            let last_page = self.total_pages().saturating_sub(1);
            self.cursor = last_page * self.page_size;
            true
        }
    }

    /// Select a candidate by index within the current page (1-9)
    pub fn select_on_page(&mut self, page_index: usize) -> Option<&Candidate> {
        self.unselected = false;
        if page_index == 0 || page_index > self.page_size {
            return None;
        }

        let absolute_index = self.page_start() + page_index - 1;
        if absolute_index < self.candidates.len() {
            self.cursor = absolute_index;
            self.selected()
        } else {
            None
        }
    }

    /// Move the cursor to `cursor`, clamped into the list (0 when empty).
    pub fn set_cursor(&mut self, cursor: usize) {
        self.unselected = false;
        self.cursor = cursor.min(self.candidates.len().saturating_sub(1));
    }
}

impl Default for CandidateList {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_candidate_list_basic() {
        let candidates = CandidateList::from_strings(["今日", "京", "恭"]);
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates.selected_text(), Some("今日"));
    }

    #[test]
    fn test_candidate_list_navigation() {
        let mut candidates = CandidateList::from_strings(["a", "b", "c"]);

        assert!(candidates.move_next());
        assert_eq!(candidates.selected_text(), Some("b"));

        assert!(candidates.move_next());
        assert_eq!(candidates.selected_text(), Some("c"));

        // Wrap around
        assert!(candidates.move_next());
        assert_eq!(candidates.selected_text(), Some("a"));

        // Wrap back
        assert!(candidates.move_prev());
        assert_eq!(candidates.selected_text(), Some("c"));
    }

    #[test]
    fn test_candidate_list_pagination() {
        // Default page_size is 9, so 20 items = 3 pages (9+9+2)
        let items: Vec<_> = (1..=20).map(|i| format!("item{}", i)).collect();
        let mut candidates = CandidateList::from_strings(items);

        assert_eq!(candidates.total_pages(), 3);
        assert_eq!(candidates.current_page(), 0);
        assert_eq!(candidates.page_candidates().len(), 9);

        candidates.next_page();
        assert_eq!(candidates.current_page(), 1);
        assert_eq!(candidates.page_start(), 9);

        candidates.next_page();
        assert_eq!(candidates.current_page(), 2);
        assert_eq!(candidates.page_candidates().len(), 2);

        // Wrap to first page
        candidates.next_page();
        assert_eq!(candidates.current_page(), 0);
    }

    #[test]
    fn test_two_part_list_wraps_to_its_second_section() {
        // 2 predictions, then 12 conversions: pages of 9 and 5.
        let items: Vec<_> = (1..=14).map(|i| format!("item{}", i)).collect();
        let mut candidates = CandidateList::from_strings(items).with_divider(2);

        // Past the end: back to the first conversion, not a prediction.
        candidates.set_cursor(13);
        candidates.move_next();
        assert_eq!(candidates.cursor(), 2);

        // Stepping back up is what enters the predictions.
        candidates.move_prev();
        assert_eq!(candidates.cursor(), 1);

        // Paging onto the first page lands below the divider too.
        candidates.set_cursor(13);
        candidates.next_page();
        assert_eq!(candidates.cursor(), 2);
        candidates.next_page();
        assert_eq!(candidates.cursor(), 9);
        candidates.prev_page();
        assert_eq!(candidates.cursor(), 2);

        // The divider is drawn on the page it falls on only.
        assert_eq!(candidates.page_divider(), Some(2));
        candidates.next_page();
        assert_eq!(candidates.page_divider(), None);
        assert_eq!(candidates.page_row_numbered(3), Some(3));
    }

    #[test]
    fn test_candidate_list_select_on_page() {
        let items: Vec<_> = (1..=20).map(|i| format!("item{}", i)).collect();
        let mut candidates = CandidateList::from_strings(items);

        // Select item 3 on first page
        candidates.select_on_page(3);
        assert_eq!(candidates.selected_text(), Some("item3"));

        // Move to second page and select item 2
        candidates.next_page();
        candidates.select_on_page(2);
        assert_eq!(candidates.selected_text(), Some("item11")); // 9 + 2 = 11
    }
}
