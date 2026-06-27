/// Scores how well texts match a search query, tolerating typos of omission.
///
/// The query is split into words. Every word has to match one of the searched fields, either as a
/// substring or as a subsequence of characters. Substring matches at word starts rank highest.
#[derive(Debug, Clone)]
pub struct Matcher {
    words: Vec<Vec<char>>,
}

const SUBSTRING: i64 = 100;
const WORD_START: i64 = 60;
const FIELD_START: i64 = 40;
const SUBSEQUENCE: i64 = 20;
const GAP_PENALTY: i64 = 2;

impl Matcher {
    pub fn new(query: &str) -> Self {
        let words = query
            .split_whitespace()
            .map(|word| word.chars().flat_map(char::to_lowercase).collect())
            .collect();
        Self { words }
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// The combined score of all query words over `fields`, or `None` if any word is missing.
    /// Earlier fields weigh more than later ones.
    pub fn score(&self, fields: &[&str]) -> Option<i64> {
        let fields: Vec<Vec<char>> = fields
            .iter()
            .map(|field| field.chars().flat_map(char::to_lowercase).collect())
            .collect();
        let mut total = 0;
        for word in &self.words {
            let best = fields
                .iter()
                .enumerate()
                .filter_map(|(index, field)| {
                    word_score(word, field).map(|score| score - index as i64 * 5)
                })
                .max()?;
            total += best;
        }
        Some(total)
    }
}

fn word_score(word: &[char], field: &[char]) -> Option<i64> {
    if word.is_empty() {
        return Some(0);
    }
    if let Some(position) = find(field, word) {
        let mut score = SUBSTRING + word.len() as i64;
        if position == 0 {
            score += FIELD_START;
        }
        if position == 0 || !field[position - 1].is_alphanumeric() {
            score += WORD_START;
        }
        return Some(score);
    }
    subsequence_score(word, field)
}

fn find(haystack: &[char], needle: &[char]) -> Option<usize> {
    if needle.len() > haystack.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|start| haystack[*start..].starts_with(needle))
}

fn subsequence_score(word: &[char], field: &[char]) -> Option<i64> {
    let mut gaps = 0;
    let mut previous: Option<usize> = None;
    let mut position = 0;
    for character in word {
        let found = field[position..]
            .iter()
            .position(|candidate| candidate == character)?;
        let index = position + found;
        if let Some(previous) = previous {
            gaps += (index - previous - 1) as i64;
        }
        previous = Some(index);
        position = index + 1;
    }
    Some((SUBSEQUENCE + word.len() as i64 - gaps * GAP_PENALTY).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_word_has_to_match() {
        let matcher = Matcher::new("rust docs");
        assert!(
            matcher
                .score(&["The Rust Book", "https://doc.rust-lang.org/book"])
                .is_some()
        );
        assert!(
            matcher
                .score(&["The Rust Book", "https://example.com"])
                .is_none()
        );
    }

    #[test]
    fn prefixes_beat_inner_substrings_and_subsequences() {
        let matcher = Matcher::new("git");
        let prefix = matcher.score(&["GitHub"]).unwrap();
        let inner = matcher.score(&["Legit"]).unwrap();
        let spread = matcher.score(&["Great idea today"]).unwrap();
        assert!(prefix > inner);
        assert!(inner > spread);
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert!(
            Matcher::new("ATOLL")
                .score(&["atoll - wikipedia"])
                .is_some()
        );
    }

    #[test]
    fn earlier_fields_weigh_more() {
        let matcher = Matcher::new("news");
        let title = matcher.score(&["News", "https://example.com"]).unwrap();
        let url = matcher
            .score(&["Example", "https://news.example.com"])
            .unwrap();
        assert!(title > url);
    }

    #[test]
    fn empty_query_matches_everything() {
        let matcher = Matcher::new("   ");
        assert!(matcher.is_empty());
        assert_eq!(matcher.score(&["anything"]), Some(0));
    }
}
