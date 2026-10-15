pub fn normalize(value: &str, insensitive: bool) -> String {
    let stripped: String = value
        .chars()
        .filter(|c| {
            let u = *c as u32;
            !(0x0300..=0x036F).contains(&u)
        })
        .collect();
    if insensitive {
        stripped.to_lowercase()
    } else {
        stripped
    }
}

fn tokens_from_normalized(normalized: &str) -> Vec<&str> {
    normalized
        .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .map(str::trim)
        .filter(|t| t.len() > 1)
        .collect()
}

/// Two-row Levenshtein with early length bail-out.
fn levenshtein(a: &str, b: &str) -> usize {
    if a == b {
        return 0;
    }
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (a, b) = if a.len() < b.len() { (b, a) } else { (a, b) };
    let m = a.len();
    let n = b.len();
    if n == 0 {
        return m;
    }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut cur = vec![0usize; n + 1];
    for i in 1..=m {
        cur[0] = i;
        let mut row_min = cur[0];
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            row_min = row_min.min(cur[j]);
        }
        std::mem::swap(&mut prev, &mut cur);
        let _ = row_min; // kept for future banded early-exit
    }
    prev[n]
}

fn matches_with_text(
    algorithm: &str,
    pattern: &str,
    text: &str,
    haystack_raw: &str,
    insensitive: bool,
) -> bool {
    if pattern.trim().is_empty() || algorithm == "none" {
        return false;
    }
    let pat = normalize(pattern, insensitive);
    match algorithm {
        "any" => tokens_from_normalized(&pat)
            .into_iter()
            .any(|t| text.contains(t)),
        "all" => tokens_from_normalized(&pat)
            .into_iter()
            .all(|t| text.contains(t)),
        "exact" => text.contains(&pat),
        "regex" => regex::RegexBuilder::new(pattern)
            .case_insensitive(insensitive)
            .build()
            .map(|re| re.is_match(haystack_raw))
            .unwrap_or(false),
        "fuzzy" => {
            let words: Vec<&str> = text
                .split(|c: char| c.is_whitespace() || ".,;:!?".contains(c))
                .filter(|w| w.len() > 3)
                .collect();
            tokens_from_normalized(&pat).into_iter().any(|t| {
                let max_d = 1.max(t.len() / 4);
                words.iter().any(|w| {
                    let (la, lb) = (t.len(), w.len());
                    if la.abs_diff(lb) > max_d {
                        return false;
                    }
                    levenshtein(t, w) <= max_d
                })
            })
        }
        "auto" => tokens_from_normalized(&pat)
            .into_iter()
            .any(|t| t.len() > 2 && text.contains(t)),
        _ => false,
    }
}

pub fn matches_algorithm(algorithm: &str, pattern: &str, haystack: &str, insensitive: bool) -> bool {
    let text = normalize(haystack, insensitive);
    matches_with_text(algorithm, pattern, &text, haystack, insensitive)
}

/// Like [`matches_algorithm`] but reuses a pre-normalized haystack (per-document classify).
pub fn matches_algorithm_pre(
    algorithm: &str,
    pattern: &str,
    text_normalized: &str,
    haystack_raw: &str,
    insensitive: bool,
) -> bool {
    matches_with_text(algorithm, pattern, text_normalized, haystack_raw, insensitive)
}

pub fn match_entity(name: &str, match_text: &str, algorithm: &str, insensitive: bool, content: &str) -> bool {
    let pattern = if !match_text.trim().is_empty() {
        match_text
    } else if algorithm == "auto" {
        name
    } else {
        ""
    };
    matches_algorithm(algorithm, pattern, content, insensitive)
}

pub fn match_entity_pre(
    name: &str,
    match_text: &str,
    algorithm: &str,
    insensitive: bool,
    text_ci: &str,
    text_cs: &str,
    content_raw: &str,
) -> bool {
    let pattern = if !match_text.trim().is_empty() {
        match_text
    } else if algorithm == "auto" {
        name
    } else {
        ""
    };
    let text = if insensitive { text_ci } else { text_cs };
    matches_algorithm_pre(algorithm, pattern, text, content_raw, insensitive)
}
