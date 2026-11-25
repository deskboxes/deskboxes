//! Finding icons by name (the desktop search): what was typed, read into words and filters, and
//! how well a name matches it. Best matches first: the whole name, then the start of the name,
//! the start of a word, the words' initials ("vs" → Visual Studio), anywhere in the name, and
//! last a word with a typo in it ("chorme" → Chrome).

/// Lowercase without accents, so "zolc" finds "Żółć" and "resume" finds "Résumé".
pub fn fold(s: &str) -> String {
    s.chars()
        .flat_map(|c| c.to_lowercase())
        .map(|c| match c {
            'ą' | 'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' | 'ā' => 'a',
            'ć' | 'ç' | 'č' => 'c',
            'ę' | 'é' | 'è' | 'ê' | 'ë' | 'ě' | 'ē' => 'e',
            'í' | 'ì' | 'î' | 'ï' | 'ī' => 'i',
            'ł' => 'l',
            'ń' | 'ñ' | 'ň' => 'n',
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'ø' | 'ō' => 'o',
            'ś' | 'š' | 'ß' => 's',
            'ú' | 'ù' | 'û' | 'ü' | 'ů' | 'ū' => 'u',
            'ý' | 'ÿ' => 'y',
            'ź' | 'ż' | 'ž' => 'z',
            'ř' => 'r',
            'ď' => 'd',
            'ť' => 't',
            c => c,
        })
        .collect()
}

/// What was typed: the words to find, and extensions typed as `pdf:` (any of them).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub words: Vec<String>,
    pub exts: Vec<String>,
}

impl Query {
    pub fn parse(text: &str) -> Query {
        let mut q = Query::default();
        for w in fold(text).split_whitespace() {
            match w.strip_suffix(':').map(|e| e.trim_start_matches('.')) {
                Some(e) if !e.is_empty() && e.chars().all(|c| c.is_alphanumeric()) => q.exts.push(e.to_string()),
                _ => q.words.push(w.to_string()),
            }
        }
        q
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty() && self.exts.is_empty()
    }

    /// The extension filter lets `ext` ("PDF", ".pdf") through.
    pub fn ext_ok(&self, ext: &str) -> bool {
        self.exts.is_empty() || {
            let e = fold(ext.trim_start_matches('.'));
            self.exts.iter().any(|x| *x == e)
        }
    }

    /// How well `name` matches every word (None: some word is not in it). No words: everything
    /// matches equally.
    pub fn score(&self, name: &str) -> Option<u32> {
        if self.words.is_empty() {
            return Some(0);
        }
        self.score_folded(&Folded::new(name), &mut Scratch::default())
    }

    /// `score` for a name folded beforehand (the search keeps them between keystrokes).
    /// `scratch` holds the typo check's rows, reused from name to name.
    pub fn score_folded(&self, f: &Folded, scratch: &mut Scratch) -> Option<u32> {
        if self.words.is_empty() {
            return Some(0);
        }
        let mut total = 0;
        for w in &self.words {
            total += word_score(w, f, scratch)?;
        }
        // Shorter names first among equals ("Chrome" before "Chrome Remote Desktop").
        Some(total * 4 + 100u32.saturating_sub(f.len as u32).min(100) / 4)
    }
}

/// A name read for matching: folded, its words and their initials.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Folded {
    n: String,
    tokens: Vec<String>,
    token_chars: Vec<Vec<char>>,
    initials: String,
    len: usize,
}

impl Folded {
    pub fn new(name: &str) -> Folded {
        let n = fold(name);
        let tokens: Vec<String> = n.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(str::to_string).collect();
        let token_chars = tokens.iter().map(|t| t.chars().collect()).collect();
        let initials = tokens.iter().filter_map(|t| t.chars().next()).collect();
        let len = n.chars().count();
        Folded { n, tokens, token_chars, initials, len }
    }

    /// The same text was folded (two names that fold alike score alike).
    pub fn same(&self, other: &Folded) -> bool {
        self.n == other.n
    }
}

/// Rows reused by the typo check.
#[derive(Default)]
pub struct Scratch {
    rows: [Vec<usize>; 3],
    word: Vec<char>,
}

fn word_score(w: &str, f: &Folded, scratch: &mut Scratch) -> Option<u32> {
    let n = f.n.as_str();
    if n == w {
        return Some(1000);
    }
    if n.starts_with(w) {
        return Some(900);
    }
    if f.tokens.iter().any(|t| t.starts_with(w)) {
        return Some(800);
    }
    let wl = w.chars().count();
    if wl >= 2 && f.initials.starts_with(w) {
        return Some(700);
    }
    if n.contains(w) {
        return Some(600);
    }
    // A typo: a word (or its start) at most one edit away, two for long words.
    if wl >= 4 {
        let most = if wl >= 8 { 2 } else { 1 };
        let mut wc = std::mem::take(&mut scratch.word);
        wc.clear();
        wc.extend(w.chars());
        let mut best = usize::MAX;
        for tc in &f.token_chars {
            // The whole word, or its start (the rest not typed yet): each length once.
            let mut lens = [tc.len(), wl.saturating_sub(1), wl, wl + 1];
            lens.sort_unstable();
            let mut last = usize::MAX;
            for k in lens {
                if k == last || k < wl.saturating_sub(most) || k > tc.len() {
                    continue;
                }
                last = k;
                let limit = most.min(best.saturating_sub(1));
                if let Some(d) = edits_within(&wc, &tc[..k], limit, &mut scratch.rows) {
                    best = best.min(d);
                }
                if best == 0 {
                    break;
                }
            }
            if best == 0 {
                break;
            }
        }
        scratch.word = wc;
        if best <= most {
            return Some(400 - 100 * best as u32);
        }
    }
    None
}

/// Edits (insert, delete, change, swap two neighbours) from `a` to `b`, if at most `max`
/// (None: more). Three rows, reused; a row whose every cell is past `max` ends it early.
fn edits_within(a: &[char], b: &[char], max: usize, rows: &mut [Vec<usize>; 3]) -> Option<usize> {
    let (n, m) = (a.len(), b.len());
    if n.abs_diff(m) > max {
        return None;
    }
    for r in rows.iter_mut() {
        r.clear();
        r.resize(m + 1, 0);
    }
    // rows[0]: i - 2, rows[1]: i - 1, rows[2]: i.
    for (j, x) in rows[1].iter_mut().enumerate() {
        *x = j;
    }
    for i in 1..=n {
        rows[2][0] = i;
        let mut low = i;
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut d = (rows[1][j] + 1).min(rows[2][j - 1] + 1).min(rows[1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d = d.min(rows[0][j - 2] + 1);
            }
            rows[2][j] = d;
            low = low.min(d);
        }
        if low > max {
            return None;
        }
        rows.rotate_left(1);
    }
    // (After the last rotation the final row is rows[1].)
    let d = rows[1][m];
    (d <= max).then_some(d)
}

/// Edits from `a` to `b`, the full table (the reference the bounded check is tested against).
#[cfg(test)]
fn edits(a: &[char], b: &[char]) -> usize {
    let (n, m) = (a.len(), b.len());
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for j in 0..=m {
        d[0][j] = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[n][m]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(q: &str, name: &str) -> Option<u32> {
        Query::parse(q).score(name)
    }

    #[test]
    fn finds_and_ranks() {
        assert!(s("chrome", "Google Chrome").is_some());
        assert!(s("zolc", "Żółć").is_some());
        assert!(s("vs", "Visual Studio Code").is_some());
        assert!(s("vsc", "Visual Studio Code").is_some());
        assert!(s("chorme", "Google Chrome").is_some(), "a swap");
        assert!(s("chrom", "Google Chrome").is_some());
        assert!(s("photshop", "Adobe Photoshop 2025").is_some(), "a missing letter");
        assert!(s("xyz", "Google Chrome").is_none());
        assert!(s("cat", "Chrome").is_none(), "short words need to be exact");
        assert!(s("chrome word", "Google Chrome").is_none(), "every word");
        // Whole name > start > word start > initials > inside > typo.
        let order = [s("steam", "Steam"), s("steam", "Steam Deck"), s("steam", "My Steam"), s("ms", "My Steam"), s("tea", "My Steam"), s("staem", "My Steam")];
        for w in order.windows(2) {
            assert!(w[0].unwrap() > w[1].unwrap(), "{order:?}");
        }
        assert!(s("chrome", "Chrome").unwrap() > s("chrome", "Chrome Remote Desktop").unwrap());
    }

    #[test]
    fn extension_filters() {
        let q = Query::parse("report pdf: .TXT:");
        assert_eq!(q.words, vec!["report"]);
        assert_eq!(q.exts, vec!["pdf", "txt"]);
        assert!(q.ext_ok(".PDF") && q.ext_ok("txt") && !q.ext_ok("docx"));
        assert!(Query::parse("pdf:").words.is_empty());
        assert!(Query::parse("a: :").exts == vec!["a"]);
        assert!(Query::default().ext_ok("anything"));
    }

    #[test]
    fn bounded_edits_match_the_full_table() {
        let words = ["", "a", "ab", "ba", "chrome", "chorme", "chrom", "crhome", "photoshop", "photshop", "phtoshop", "zolc", "żółć", "abcdefgh", "badcfehg", "projcet", "project", "projects", "xyz"];
        let mut rows = Default::default();
        for x in words {
            for y in words {
                let (a, b): (Vec<char>, Vec<char>) = (x.chars().collect(), y.chars().collect());
                let full = edits(&a, &b);
                for max in 0..4 {
                    assert_eq!(edits_within(&a, &b, max, &mut rows), (full <= max).then_some(full), "{x} {y} {max}");
                }
            }
        }
    }

    #[test]
    fn folded_scores_match_plain_ones() {
        let names = ["Quarterly project summary 00001.txt", "Google Chrome", "Żółć", "Résumé.docx", "Visual Studio Code", "My Steam", "Adobe Photoshop 2025"];
        let queries = ["projcet", "project", "proj sum", "chorme", "zolc", "resume", "vsc", "staem", "photshop", "xyz", "q"];
        let mut sc = Scratch::default();
        for q in queries {
            let q = Query::parse(q);
            for n in names {
                assert_eq!(q.score_folded(&Folded::new(n), &mut sc), q.score(n), "{n}");
            }
        }
        assert!(Folded::new("Résumé").same(&Folded::new("resume")));
    }
}
