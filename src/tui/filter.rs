//! `/` filtering: which rows survive what you have typed.
//!
//! Deliberately a filter and not a ranker. The Connections tab is already
//! ordered by when you last opened each one, and re-sorting on every keystroke
//! would take that away for a guess about what you meant. Typing narrows; it
//! never reshuffles.

/// Does any of `haystacks` match `query`? Case-insensitive, and a substring
/// match wins outright; failing that we accept an in-order subsequence, so
/// `prd` finds `prod-replica` without needing the exact spelling.
pub(super) fn matches(query: &str, haystacks: &[&str]) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    haystacks.iter().any(|h| {
        let h = h.to_lowercase();
        h.contains(&q) || subsequence(&q, &h)
    })
}

/// Every char of `needle`, in order, somewhere in `haystack`.
fn subsequence(needle: &str, haystack: &str) -> bool {
    let mut chars = haystack.chars();
    needle
        .chars()
        .all(|c| chars.any(|h| h.eq_ignore_ascii_case(&c)))
}
