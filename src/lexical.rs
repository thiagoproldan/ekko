//! Lexical search over item text: the words an agent gives `search`, matched
//! the way a person means them and ranked by how much each item is about them.
//!
//! It replaces a lowercase substring test, which answered a different
//! question. Measured on the ekko board on 2026-09-15 (note 166): "check
//! cycle" found nothing because no description holds those words in that
//! order, four of the eight hits for "cycle" only said "recycled", "decisao"
//! missed "Decisão", and every hit came back in id order clipped to its first
//! 160 characters, however far into a note the match sat -- so an agent read
//! each hit in full to learn why it was there. Three rules answer those:
//!
//! - Text is folded before it is compared: lowercase, with the Latin letters
//!   that carry diacritics read as their base letter, so "decisão" and
//!   "decisao" are one word. A table rather than Unicode normalisation: the
//!   boards are written in Portuguese and English, and the table covers them
//!   without a dependency.
//! - A query word matches a word that starts with it once it is three
//!   characters long -- "cycle" finds "cycles" and never "recycled" -- and a
//!   shorter one only the whole word. Words match in any order. When some item
//!   matches every word only those are hits; when none does, the items that
//!   match any word are, and the caller is told which it got.
//! - Hits rank by how many words they match, then by BM25 as Lucene computes
//!   it: idf = ln(1 + (N - n + 0.5) / (n + 0.5)), k1 = 1.2, b = 0.75.
//! - A query word loses its plural or verb ending before it is matched, so
//!   "handoffs" finds "handoff" and "decisões" finds "decisão": on this board
//!   13 of 18 inflected words found less than half of what their base form
//!   did. Only the query is stemmed -- a text's word already matches any
//!   query word it starts with -- so a stem can only widen what a word finds.

use std::cmp::Reverse;
use std::collections::HashSet;

const K1: f64 = 1.2;
const B: f64 = 0.75;
/// Below this many characters a query word must match a whole word: "id"
/// would otherwise match every "idea" and "identity".
const PREFIX_FROM: usize = 3;

/// The words of a search, folded, each once, in the order given.
#[derive(Debug, Clone, Default)]
pub struct Query {
    terms: Vec<String>,
}

impl Query {
    pub fn new(text: &str) -> Self {
        let mut terms: Vec<String> = Vec::new();
        for word in words(&text.chars().collect::<Vec<_>>()) {
            let term = stem(&word.folded);
            if !terms.contains(&term) {
                terms.push(term);
            }
        }
        Query { terms }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// The query word a folded word of the text matches, if any.
    fn term_of(&self, word: &str) -> Option<usize> {
        self.terms.iter().position(|term| matches(term, word))
    }
}

/// A folded query word without its plural or verb ending, by light rules for
/// English and Portuguese, the languages the boards are written in: a table
/// of endings rather than a Snowball stemmer, for no dependency. A rule
/// applies only when it leaves enough of the word -- four letters after
/// -ing and -ed, whose short words are seldom inflected ("string", "need"),
/// three after a plural -- and a word ending in -ss, -us or -is keeps its s.
fn stem(word: &str) -> String {
    let cut = |ending: &str, shortest: usize| {
        word.strip_suffix(ending).filter(|stem| stem.chars().count() >= shortest).map(str::to_string)
    };
    let plural_s = !["ss", "us", "is"].iter().any(|kept| word.ends_with(kept));
    // decisoes -> decis, dependencies -> dependenc, matches -> match: each a
    // start shared by the singular and the plural.
    cut("oes", 4)
        .or_else(|| cut("ies", 4))
        .or_else(|| cut("ing", 4))
        .or_else(|| cut("ed", 4))
        .or_else(|| ["ches", "shes", "sses", "xes", "zes"].iter().find_map(|ending| word.ends_with(ending).then(|| cut("es", 3)).flatten()))
        .or_else(|| cut("s", 3).filter(|_| plural_s))
        .unwrap_or_else(|| word.to_string())
}

fn matches(term: &str, word: &str) -> bool {
    if term.chars().count() >= PREFIX_FROM {
        word.starts_with(term)
    } else {
        word == term
    }
}

/// One matching text: its position among the texts ranked, how many query
/// words it matched, and its BM25 score.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub index: usize,
    pub matched: usize,
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ranked {
    /// Best first.
    pub hits: Vec<Hit>,
    /// Whether the hits match every word of the query, rather than some.
    pub every_word: bool,
}

/// Ranks `texts` against `query` by the rules the module describes.
pub fn rank(query: &Query, texts: &[&str]) -> Ranked {
    let terms = query.terms.len();
    let counted: Vec<(usize, Vec<usize>)> = texts
        .iter()
        .map(|text| {
            let found = words(&text.chars().collect::<Vec<_>>());
            let mut counts = vec![0; terms];
            for word in &found {
                for (at, term) in query.terms.iter().enumerate() {
                    if matches(term, &word.folded) {
                        counts[at] += 1;
                    }
                }
            }
            (found.len(), counts)
        })
        .collect();

    let documents = texts.len() as f64;
    let average = (counted.iter().map(|(length, _)| *length).sum::<usize>() as f64 / documents.max(1.0)).max(1.0);
    let containing: Vec<f64> =
        (0..terms).map(|at| counted.iter().filter(|(_, counts)| counts[at] > 0).count() as f64).collect();
    let every_word = counted.iter().any(|(_, counts)| counts.iter().all(|&count| count > 0));

    let mut hits: Vec<Hit> = counted
        .iter()
        .enumerate()
        .filter_map(|(index, (length, counts))| {
            let matched = counts.iter().filter(|&&count| count > 0).count();
            if matched == 0 || (every_word && matched < terms) {
                return None;
            }
            let score = counts
                .iter()
                .zip(&containing)
                .filter(|(&count, _)| count > 0)
                .map(|(&count, &n)| {
                    let idf = (1.0 + (documents - n + 0.5) / (n + 0.5)).ln();
                    let tf = count as f64;
                    idf * tf * (K1 + 1.0) / (tf + K1 * (1.0 - B + B * *length as f64 / average))
                })
                .sum();
            Some(Hit { index, matched, score })
        })
        .collect();
    hits.sort_by(|a, b| b.matched.cmp(&a.matched).then(b.score.total_cmp(&a.score)).then(a.index.cmp(&b.index)));
    Ranked { hits, every_word }
}

/// `text` on one line, cut to `window` characters around where it best
/// matches `query`: the stretch holding the most different query words, with
/// a short lead-in, cut at word boundaries and marked with an ellipsis on
/// each side something was cut from. A text that fits is returned whole.
pub fn snippet(text: &str, query: &Query, window: usize) -> String {
    let flat: Vec<char> = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
    if flat.len() <= window {
        return flat.into_iter().collect();
    }
    let found: Vec<(usize, usize)> =
        words(&flat).into_iter().filter_map(|word| query.term_of(&word.folded).map(|term| (word.start, term))).collect();
    let lead = window / 4;
    let latest = flat.len() - window;
    let start = found
        .iter()
        .map(|&(at, _)| at.saturating_sub(lead).min(latest))
        .max_by_key(|&start| {
            let covered: HashSet<usize> =
                found.iter().filter(|&&(at, _)| at >= start && at < start + window).map(|&(_, term)| term).collect();
            (covered.len(), Reverse(start))
        })
        .unwrap_or(0);

    let slack = window / 5;
    let mut from = start;
    while from > 0 && from < start + slack && flat[from - 1] != ' ' {
        from += 1;
    }
    let mut to = start + window;
    if to < flat.len() {
        let floor = to - slack;
        while to > floor && flat[to] != ' ' {
            to -= 1;
        }
    }
    let body: String = flat[from..to].iter().collect();
    format!(
        "{}{}{}",
        if from > 0 { "\u{2026}" } else { "" },
        body.trim(),
        if to < flat.len() { "\u{2026}" } else { "" }
    )
}

/// A word of a text: where it starts, in characters, and its folded form.
struct Word {
    start: usize,
    folded: String,
}

/// The runs of letters and digits in `chars`.
fn words(chars: &[char]) -> Vec<Word> {
    let mut out = Vec::new();
    let mut start = None;
    for (at, c) in chars.iter().enumerate() {
        match (c.is_alphanumeric(), start) {
            (true, None) => start = Some(at),
            (false, Some(from)) => {
                out.push(Word { start: from, folded: fold(&chars[from..at]) });
                start = None;
            }
            _ => {}
        }
    }
    if let Some(from) = start {
        out.push(Word { start: from, folded: fold(&chars[from..]) });
    }
    out
}

/// Lowercase, with the Latin letters that carry diacritics read as their base.
fn fold(chars: &[char]) -> String {
    let mut out = String::with_capacity(chars.len());
    for c in chars.iter().flat_map(|c| c.to_lowercase()) {
        match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => out.push('a'),
            'ç' | 'ć' | 'č' => out.push('c'),
            'ď' | 'đ' => out.push('d'),
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => out.push('e'),
            'ğ' => out.push('g'),
            'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' | 'ı' => out.push('i'),
            'ł' => out.push('l'),
            'ñ' | 'ń' | 'ň' => out.push('n'),
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => out.push('o'),
            'ř' => out.push('r'),
            'ś' | 'š' | 'ş' => out.push('s'),
            'ť' | 'ţ' => out.push('t'),
            'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => out.push('u'),
            'ý' | 'ÿ' => out.push('y'),
            'ź' | 'ż' | 'ž' => out.push('z'),
            'ß' => out.push_str("ss"),
            'æ' => out.push_str("ae"),
            'œ' => out.push_str("oe"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranked(query: &str, texts: &[&str]) -> (Vec<usize>, bool) {
        let ranked = rank(&Query::new(query), texts);
        (ranked.hits.iter().map(|hit| hit.index).collect(), ranked.every_word)
    }

    #[test]
    fn a_query_word_loses_its_plural_or_verb_ending() {
        for (word, stemmed) in [
            ("handoffs", "handoff"),
            ("decisions", "decision"),
            ("blocking", "block"),
            ("blocked", "block"),
            ("blocks", "block"),
            ("caching", "cach"),
            ("stashed", "stash"),
            ("releases", "release"),
            ("matches", "match"),
            ("fixes", "fix"),
            ("dependencies", "dependenc"),
            ("decisoes", "decis"),
            ("sessoes", "sess"),
            ("tarefas", "tarefa"),
            // Left whole: too little would be left, or the s is the word's.
            ("string", "string"),
            ("need", "need"),
            ("ids", "ids"),
            ("status", "status"),
            ("analysis", "analysis"),
            ("process", "process"),
            ("cycle", "cycle"),
        ] {
            assert_eq!(stem(word), stemmed, "{word}");
        }
    }

    /// The inflected query finds what the base form finds, and a stem still
    /// matches only the start of a word.
    #[test]
    fn an_inflected_query_finds_the_base_form() {
        let texts = ["the handoff of task 3", "a decisão foi tomada", "blocked by 4", "recycled cycle notes"];
        assert_eq!(ranked("handoffs", &texts).0, vec![0]);
        assert_eq!(ranked("decisões", &texts).0, vec![1]);
        assert_eq!(ranked("blocking", &texts).0, vec![2]);
        assert_eq!(ranked("cycles", &texts).0, vec![3], "cycle, and never recycled");
    }

    #[test]
    fn case_and_accents_fold_away() {
        assert_eq!(fold(&"Decisão Ação ÜBER Straße".chars().collect::<Vec<_>>()), "decisao acao uber strasse");
        assert_eq!(ranked("decisao", &["Decisão tomada", "outra coisa"]), (vec![0], true));
        assert_eq!(ranked("AÇÃO", &["uma acao", "nada"]), (vec![0], true));
    }

    #[test]
    fn a_word_matches_the_start_of_a_word_and_never_its_middle() {
        assert_eq!(ranked("cycle", &["recycled ids break caches", "the cycle check", "cycles of checks"]).0, vec![1, 2]);
        assert_eq!(ranked("id", &["an idea", "the id"]).0, vec![1], "a short word matched inside a longer one");
    }

    #[test]
    fn words_match_in_any_order_and_every_word_beats_some() {
        let texts = ["grafo de dependências", "um grafo", "dependências soltas"];
        assert_eq!(ranked("dependencias grafo", &texts), (vec![0], true));
        assert_eq!(ranked("grafo zebra", &texts), (vec![1, 0], false));
        assert_eq!(ranked("zebra", &texts), (vec![], false));
    }

    #[test]
    fn a_text_about_the_word_ranks_above_one_that_mentions_it() {
        let texts = ["a long note that mentions the cycle once among many other words written here", "cycle check"];
        assert_eq!(ranked("cycle", &texts).0, vec![1, 0]);
    }

    #[test]
    fn a_snippet_shows_where_the_text_matched() {
        let text = format!("{}the important keyword is here at the end", "padding ".repeat(40));
        let shown = snippet(&text, &Query::new("keyword"), 60);
        assert!(shown.starts_with('\u{2026}') && shown.contains("keyword is here at the end"), "{shown}");
        assert!(shown.chars().count() <= 61, "{shown}");
        assert_eq!(snippet("short text", &Query::new("text"), 60), "short text");
        let head = snippet(&format!("keyword first, then {}", "padding ".repeat(40)), &Query::new("keyword"), 60);
        assert!(head.starts_with("keyword first") && head.ends_with('\u{2026}'), "{head}");
    }
}
