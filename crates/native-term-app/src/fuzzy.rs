//! Fuzzy matching for the session search: the query's characters must
//! appear in order (case-insensitive); consecutive runs, word starts and
//! an early first match score higher. Several space-separated words must
//! all match, each in any of the host's fields.

/// Score of `query` (one word) in `text`, or `None` if it doesn't match.
pub fn score_word(query: &str, text: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let text: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let query: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    // exact substring: best, earlier is better
    if let Some(pos) = find(&text, &query) {
        let start_bonus = if pos == 0 || !text[pos - 1].is_alphanumeric() { 50 } else { 0 };
        return Some(1000 + start_bonus - pos as i64 - text.len() as i64 / 8);
    }
    let mut score = 0i64;
    let mut ti = 0;
    let mut previous: Option<usize> = None;
    for &qc in &query {
        let found = (ti..text.len()).find(|&i| text[i] == qc)?;
        score += 10;
        if previous == Some(found.wrapping_sub(1)) {
            score += 15;
        }
        if found == 0 || !text[found - 1].is_alphanumeric() {
            score += 10;
        }
        if previous.is_none() {
            score -= found as i64;
        }
        previous = Some(found);
        ti = found + 1;
    }
    Some(score)
}

fn find(text: &[char], query: &[char]) -> Option<usize> {
    if query.len() > text.len() {
        return None;
    }
    (0..=text.len() - query.len()).find(|&i| text[i..i + query.len()] == *query)
}

/// Score of a whole query against several fields (label first: it counts
/// double), or `None` if any word matches none of them.
pub fn score(query: &str, fields: &[&str]) -> Option<i64> {
    let mut total = 0;
    for word in query.split_whitespace() {
        let best = fields
            .iter()
            .enumerate()
            .filter_map(|(i, field)| score_word(word, field).map(|s| if i == 0 { s * 2 } else { s }))
            .max()?;
        total += best;
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_and_ranking() {
        assert!(score_word("osd", "ceph-osd1").is_some());
        assert!(score_word("cph1", "ceph-osd1").is_some());
        assert!(score_word("xyz", "ceph-osd1").is_none());
        assert!(score_word("OSD", "ceph-osd1").is_some(), "case-insensitive");
        assert!(score_word("生产", "osd 1 (生产)").is_some());
        // substring beats scattered; word start beats middle
        assert!(score_word("web", "web01").unwrap() > score_word("web", "w-e-b").unwrap());
        assert!(score_word("osd", "osd1").unwrap() > score_word("osd", "ceph-osd1").unwrap());
        assert!(score_word("osd", "ceph-osd1").unwrap() > score_word("osd", "cephosd1").unwrap());
    }

    #[test]
    fn all_words_must_match() {
        let fields = ["osd 1", "ceph.osd-1", "10.32.16.70", "ops"];
        assert!(score("osd 16.70", &fields).is_some());
        assert!(score("osd root", &fields).is_none());
        assert!(score("", &fields) == Some(0));
        // a label hit counts more than the same hit in another field
        assert!(score("osd", &["osd", "x"]).unwrap() > score("osd", &["x", "osd"]).unwrap());
    }
}
