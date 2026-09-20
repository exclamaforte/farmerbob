//! Approach distance over candidate solutions: is a wave N approaches or
//! one approach N times?
//!
//! Bead farmerbob-x81s.8: under autoresearch, eight arms writing the same
//! tiled matmul with different variable names explored one idea at eight
//! times the price, and a pass count reports it as eight successes. That
//! number is actively misleading. Cross-examination ([`crate::crossx`])
//! asks whether test suites discriminate -- a different question, built
//! for a different purpose.
//!
//! Two levels, as the bead demands. Structural distance (normalised token
//! shingles over Python source) is the cheap floor and catches the rename
//! case. It cannot catch two different-looking programs using the same
//! technique, so technique tags extracted from the code itself pair with
//! it: which accelerator backend (triton, raw CUDA, torch.compile, SDPA,
//! or plain eager), which non-default precision markers, and -- for
//! triton/CUDA only -- the tiling factors.
//!
//! Clustering rule, stated once: same tag signature means same cluster,
//! whatever the structural distance (different-looking, same technique).
//! There is deliberately no cross-signature structural merge: at a fixed
//! threshold it cannot be well-formed (one added line moves the distance
//! 0.5 in a tiny file and 0.03 in a large one), and what it would catch
//! is either renames (already identical signatures) or genuine technique
//! differences (correctly split). Structural distance still pairs with
//! tags as each cluster's diameter: tight means renames, loose means the
//! same technique written differently. Tags dominate, so stuffing dead
//! code cannot manufacture diversity -- and a collapsed wave reports as
//! one approach, not N successes. This module is pure: tokenize, tag,
//! distance, cluster.

use std::collections::{BTreeMap, BTreeSet};

/// Shingle width over the normalised token stream.
pub const SHINGLE_WIDTH: usize = 5;

/// Python keywords, kept verbatim while every other identifier normalises
/// to `N`. `match`/`case`/`type` are soft keywords; treating them as
/// keywords only narrows matches between same-shaped code, which is the
/// safe direction for a distance floor.
const KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "case", "class",
    "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if",
    "import", "in", "is", "lambda", "match", "nonlocal", "not", "or", "pass", "raise", "return",
    "try", "type", "while", "with", "yield",
];

/// One normalised token: keywords and operators verbatim, `N` for any
/// other name, `0` for any number, `S` for any string, `\n` for newlines.
pub fn tokenize_py(src: &str) -> Vec<String> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            out.push("\n".to_string());
            i += 1;
        } else if c.is_whitespace() {
            i += 1;
        } else if c == '#' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '\\' && i + 1 < chars.len() && chars[i + 1] == '\n' {
            i += 2;
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            // String prefixes (r/b/f/u combos) rejoin their quote below;
            // a bare prefix word is just a name.
            if is_string_prefix(&word) && i < chars.len() && (chars[i] == '\'' || chars[i] == '"') {
                i = skip_string(&chars, i);
                out.push("S".to_string());
            } else if KEYWORDS.contains(&word.as_str()) {
                out.push(word);
            } else {
                out.push("N".to_string());
            }
        } else if c.is_ascii_digit()
            || (c == '.' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit())
        {
            i = skip_number(&chars, i);
            out.push("0".to_string());
        } else if c == '\'' || c == '"' {
            i = skip_string(&chars, i);
            out.push("S".to_string());
        } else if let Some(op) = match_op(&chars, i) {
            out.push(op.to_string());
            i += op.len();
        } else {
            i += 1;
        }
    }
    out
}

fn is_string_prefix(word: &str) -> bool {
    let lower: String = word.to_lowercase();
    !lower.is_empty()
        && lower.len() <= 3
        && lower
            .chars()
            .all(|c| matches!(c, 'r' | 'b' | 'f' | 'u' | 't'))
        && lower
            .chars()
            .any(|c| matches!(c, 'r' | 'b' | 'f' | 'u' | 't'))
}

fn skip_string(chars: &[char], quote_at: usize) -> usize {
    let quote = chars[quote_at];
    let triple =
        quote_at + 2 < chars.len() && chars[quote_at + 1] == quote && chars[quote_at + 2] == quote;
    let mut i = quote_at + if triple { 3 } else { 1 };
    while i < chars.len() {
        if chars[i] == '\\' {
            i += 2;
            continue;
        }
        if triple {
            if chars[i] == quote
                && i + 2 < chars.len()
                && chars[i + 1] == quote
                && chars[i + 2] == quote
            {
                return i + 3;
            }
            if chars[i] == '\n' {
                // Tolerate an unterminated triple quote: stop pretending.
            }
            i += 1;
        } else {
            if chars[i] == quote {
                return i + 1;
            }
            if chars[i] == '\n' {
                return i;
            }
            i += 1;
        }
    }
    i
}

fn skip_number(chars: &[char], mut i: usize) -> usize {
    if chars[i] == '0'
        && i + 1 < chars.len()
        && matches!(chars[i + 1], 'x' | 'X' | 'o' | 'O' | 'b' | 'B')
    {
        i += 2;
        while i < chars.len() && (chars[i].is_ascii_hexdigit() || chars[i] == '_') {
            i += 1;
        }
        return i;
    }
    while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '_') {
        i += 1;
    }
    if i < chars.len() && chars[i] == '.' {
        i += 1;
        while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '_') {
            i += 1;
        }
    }
    if i < chars.len() && matches!(chars[i], 'e' | 'E') {
        let mut j = i + 1;
        if j < chars.len() && matches!(chars[j], '+' | '-') {
            j += 1;
        }
        if j < chars.len() && chars[j].is_ascii_digit() {
            i = j;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '_') {
                i += 1;
            }
        }
    }
    if i < chars.len() && matches!(chars[i], 'j' | 'J') {
        i += 1;
    }
    i
}

fn match_op(chars: &[char], i: usize) -> Option<&'static str> {
    let rest: String = chars[i..chars.len().min(i + 3)].iter().collect();
    for op in ["...", "**=", "//=", "<<=", ">>="] {
        if rest.starts_with(op) {
            return Some(op);
        }
    }
    let rest2: String = chars[i..chars.len().min(i + 2)].iter().collect();
    for op in [
        "**", "//", "<<", ">>", "<=", ">=", "==", "!=", "+=", "-=", "*=", "/=", "%=", "&=", "|=",
        "^=", "->", ":=",
    ] {
        if rest2.starts_with(op) {
            return Some(op);
        }
    }
    match chars[i] {
        '(' | ')' | '[' | ']' | '{' | '}' | ',' | ':' | '.' | ';' | '@' | '=' | '+' | '-' | '*'
        | '/' | '%' | '<' | '>' | '&' | '|' | '^' | '~' => Some(match chars[i] {
            '(' => "(",
            ')' => ")",
            '[' => "[",
            ']' => "]",
            '{' => "{",
            '}' => "}",
            ',' => ",",
            ':' => ":",
            '.' => ".",
            ';' => ";",
            '@' => "@",
            '=' => "=",
            '+' => "+",
            '-' => "-",
            '*' => "*",
            '/' => "/",
            '%' => "%",
            '<' => "<",
            '>' => ">",
            '&' => "&",
            '|' => "|",
            '^' => "^",
            _ => "~",
        }),
        _ => None,
    }
}

/// Width-5 shingles over a normalised token stream, as a set.
pub fn shingles(tokens: &[String]) -> BTreeSet<Vec<String>> {
    if tokens.len() < SHINGLE_WIDTH {
        return BTreeSet::from([tokens.to_vec()]);
    }
    tokens
        .windows(SHINGLE_WIDTH)
        .map(<[String]>::to_vec)
        .collect()
}

/// Jaccard similarity of two shingle sets: 1.0 for two empty streams,
/// 0.0 when exactly one is empty.
pub fn similarity(a: &BTreeSet<Vec<String>>, b: &BTreeSet<Vec<String>>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f64;
    let union = (a.len() + b.len()) as f64 - inter;
    if union <= 0.0 {
        return 1.0;
    }
    inter / union
}

/// Structural distance: one minus similarity.
pub fn distance(a: &BTreeSet<Vec<String>>, b: &BTreeSet<Vec<String>>) -> f64 {
    1.0 - similarity(a, b)
}

/// Technique signature of one candidate: the approach it exhibits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    /// Accelerator backends in use, sorted (`triton`, `cuda-ext`,
    /// `torch-compile`, `sdpa`, `sdpa-<variant>`, or `eager` when none).
    pub backend: Vec<String>,
    /// Non-default precision markers, sorted (`fp16`, `bf16`, `tf32`).
    /// Plain fp32 is default noise and tags nothing.
    pub precision: Vec<String>,
    /// Tiling factors as (name, value), sorted. Extracted only for
    /// triton/CUDA backends, where they mean something; an eager file
    /// mentioning BLOCK_SIZE is not a tiled kernel.
    pub tiling: Vec<(String, i64)>,
}

impl Signature {
    /// The cluster key: exact-match identity of the approach.
    pub fn key(&self) -> String {
        let tiling: Vec<String> = self
            .tiling
            .iter()
            .map(|(n, v)| format!("{n}={v}"))
            .collect();
        format!(
            "backend:{}|precision:{}|tiling:{}",
            self.backend.join(","),
            self.precision.join(","),
            tiling.join(",")
        )
    }

    /// Human-readable tags for the report.
    pub fn tags(&self) -> Vec<String> {
        let mut tags = self.backend.clone();
        tags.extend(self.precision.clone());
        tags.extend(self.tiling.iter().map(|(n, v)| format!("{n}={v}")));
        tags
    }
}

/// Extract the technique signature from Python source.
///
/// Runs on the normalised token stream, so comments can neither tag nor
/// hide: a `triton` in a comment is prose, not an approach. String
/// literals are `S` for the same reason. Deliberately silent about the
/// torch operator under test (bead farmerbob-x81s.5's reasoning applies
/// here too: the starting candidate embodies it, so it discriminates
/// nothing).
pub fn signature(src: &str) -> Signature {
    // Vocabulary recovered from raw code words outside comments and
    // string literals (see code_words): the normalised token stream
    // proves structure, these prove technique.
    let code_tokens = code_words(src);
    let has = |w: &str| code_tokens.iter().any(|t| t == w);
    // Dotted-path suffix: `x.to(torch.float16)` and `torch.float16`
    // alike end in `.float16`.
    let has_path = |w: &str| {
        code_tokens
            .iter()
            .any(|t| t == w || t.ends_with(&format!(".{w}")))
    };
    let has_any_path = |ws: &[&str]| ws.iter().any(|w| has_path(w));

    let mut backend: BTreeSet<String> = BTreeSet::new();
    // Import roots recovered from raw source lines (comments stripped).
    let import_roots = import_roots_of(src);
    if import_roots.iter().any(|r| r == "triton")
        || has("triton.jit")
        || [
            "tl.load",
            "tl.store",
            "tl.dot",
            "tl.arange",
            "tl.program_id",
            "tl.constexpr",
        ]
        .iter()
        .any(|w| has(w))
    {
        backend.insert("triton".to_string());
    }
    if has_any_path(&["cpp_extension", "load_inline"]) {
        backend.insert("cuda-ext".to_string());
    }
    if has("torch.compile") {
        backend.insert("torch-compile".to_string());
    }
    if has_any_path(&["scaled_dot_product_attention", "sdpa_kernel", "SDPBackend"]) {
        backend.insert("sdpa".to_string());
        for variant in [
            "MATH",
            "FLASH_ATTENTION",
            "EFFICIENT_ATTENTION",
            "CUDNN_ATTENTION",
        ] {
            if has_path(variant) {
                backend.insert(format!("sdpa-{}", variant.to_lowercase()));
            }
        }
    }
    if backend.is_empty() {
        backend.insert("eager".to_string());
    }

    let mut precision: BTreeSet<String> = BTreeSet::new();
    if has_path("float16") {
        precision.insert("fp16".to_string());
    }
    if has_path("bfloat16") {
        precision.insert("bf16".to_string());
    }
    if has_path("allow_tf32") {
        precision.insert("tf32".to_string());
    }

    let mut tiling: BTreeSet<(String, i64)> = BTreeSet::new();
    if backend.contains("triton") || backend.contains("cuda-ext") {
        tiling = tiling_of(src);
    }

    Signature {
        backend: backend.into_iter().collect(),
        precision: precision.into_iter().collect(),
        tiling: tiling.into_iter().collect(),
    }
}

/// Raw code words outside comments and string literals. The normalised
/// token stream proves structure; these prove vocabulary.
fn code_words(src: &str) -> Vec<String> {
    let mut words = Vec::new();
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    // Dotted paths kept whole (`torch.compile`, `tl.load`).
    let mut current = String::new();
    let flush = |current: &mut String, words: &mut Vec<String>| {
        if !current.is_empty() {
            words.push(std::mem::take(current));
        }
    };
    while i < chars.len() {
        let c = chars[i];
        if c == '#' {
            flush(&mut current, &mut words);
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '\'' || c == '"' {
            flush(&mut current, &mut words);
            // Skip the literal, honouring escapes and triple quotes.
            let quote = c;
            let triple = i + 2 < chars.len() && chars[i + 1] == quote && chars[i + 2] == quote;
            i += if triple { 3 } else { 1 };
            while i < chars.len() {
                if chars[i] == '\\' {
                    i += 2;
                    continue;
                }
                if triple {
                    if chars[i] == quote
                        && i + 2 < chars.len()
                        && chars[i + 1] == quote
                        && chars[i + 2] == quote
                    {
                        i += 3;
                        break;
                    }
                    i += 1;
                } else if chars[i] == quote {
                    i += 1;
                    break;
                } else if chars[i] == '\n' {
                    break;
                } else {
                    i += 1;
                }
            }
        } else if c.is_ascii_alphanumeric() || c == '_' {
            current.push(c);
            i += 1;
        } else if c == '.' {
            // Keep dotted paths whole; a leading/trailing dot ends the word.
            if !current.is_empty()
                && i + 1 < chars.len()
                && (chars[i + 1].is_ascii_alphanumeric() || chars[i + 1] == '_')
            {
                current.push(c);
            } else {
                flush(&mut current, &mut words);
            }
            i += 1;
        } else {
            flush(&mut current, &mut words);
            i += 1;
        }
    }
    flush(&mut current, &mut words);
    words
}

/// Import root modules (`import a.b as c` -> `a`), skipping comments and
/// string literals.
fn import_roots_of(src: &str) -> Vec<String> {
    let mut roots = Vec::new();
    for line in src.lines() {
        let code = line.split('#').next().unwrap_or("").trim();
        let rest = code
            .strip_prefix("import ")
            .or_else(|| code.strip_prefix("from "));
        if let Some(rest) = rest {
            let root = rest.split(['.', ' ', '\t']).next().unwrap_or("").trim();
            if !root.is_empty() {
                roots.push(root.to_string());
            }
        }
    }
    roots
}

/// Tiling factors: BLOCK*/TILE* names bound to integer literals.
/// Three shapes, all common in the wild: plain assignments
/// (`BLOCK_M = 128`), annotated assignments (`BLOCK_M: int = 128`), and
/// kernel arguments (`def k(..., BLOCK_M: tl.constexpr = 128, ...)` --
/// where real triton kernels actually put them). Comparisons and
/// anything not shaped name-and-integer do not count.
fn tiling_of(src: &str) -> BTreeSet<(String, i64)> {
    let mut out = BTreeSet::new();
    for line in src.lines() {
        let code = line.split('#').next().unwrap_or("");
        let stripped = strip_strings(code);
        for (name, rest) in tiling_names(&stripped) {
            if let Some(value) = binding_value(&rest) {
                out.insert((name, value));
            }
        }
    }
    out
}

/// Every BLOCK*/TILE* identifier in the line with the text after it.
fn tiling_names(line: &str) -> Vec<(String, String)> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_alphabetic() || chars[i] == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if word.starts_with("BLOCK") || word.starts_with("TILE") {
                out.push((word, chars[i..].iter().collect()));
            }
        } else {
            i += 1;
        }
    }
    out
}

/// The integer bound by `= N` after an optional `: annotation`.
/// Comparisons (`==`, `>=`, `<=`, `!=`) are not bindings.
fn binding_value(rest: &str) -> Option<i64> {
    let mut chars = rest.chars().peekable();
    // Optional `: annotation` first.
    if chars.peek() == Some(&':') {
        chars.next();
        // Annotations never contain a bare `=`; the binding `=` ends them.
        // Bracketed defaults (`x: dict = {...}`) cannot be integers anyway.
        let mut depth = 0usize;
        loop {
            match chars.next() {
                Some('=') if depth == 0 => break,
                Some('[') | Some('(') => depth += 1,
                Some(']') | Some(')') if depth > 0 => depth -= 1,
                Some(_) => {}
                None => return None,
            }
        }
    } else {
        // Skip whitespace, demand exactly one `=`.
        let mut it = chars.clone();
        while matches!(it.peek(), Some(' ' | '\t')) {
            it.next();
        }
        if it.next() != Some('=') || it.peek() == Some(&'=') {
            return None;
        }
        chars = it;
    }
    let number: String = chars
        .skip_while(|c| *c == ' ' || *c == '\t')
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if number.is_empty() {
        return None;
    }
    number.parse::<i64>().ok()
}

/// The line with its single- and triple-quoted string spans removed.
fn strip_strings(code: &str) -> String {
    let chars: Vec<char> = code.chars().collect();
    let mut stripped = String::with_capacity(code.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c != '\'' && c != '"' {
            stripped.push(c);
            i += 1;
            continue;
        }
        let triple = i + 2 < chars.len() && chars[i + 1] == c && chars[i + 2] == c;
        i += if triple { 3 } else { 1 };
        while i < chars.len() {
            if chars[i] == '\\' {
                i += 2;
                continue;
            }
            if triple {
                if chars[i] == c && i + 2 < chars.len() && chars[i + 1] == c && chars[i + 2] == c {
                    i += 3;
                    break;
                }
                i += 1;
            } else if chars[i] == c {
                i += 1;
                break;
            } else if chars[i] == '\n' {
                break;
            } else {
                i += 1;
            }
        }
    }
    stripped
}

/// One candidate: an arm and its source.
#[derive(Debug, Clone)]
pub struct Candidate<'a> {
    /// Arm name, for the report.
    pub arm: &'a str,
    /// Candidate source.
    pub src: &'a str,
}

/// Signature groups under construction: member arms and their shingle
/// sets, for the diameter computation.
type SignatureGroup = (Vec<String>, Vec<BTreeSet<Vec<String>>>);

/// One approach cluster: arms that landed on the same approach.
#[derive(Debug, Clone, PartialEq)]
pub struct Cluster {
    /// Member arms, sorted.
    pub arms: Vec<String>,
    /// The shared tag signature.
    pub signature: String,
    /// Max pairwise structural distance within the cluster: near zero
    /// means renames of one program, higher means the same technique
    /// written differently. Zero for singletons.
    pub diameter: f64,
}

/// Cluster candidates by approach: exact signature match.
///
/// Deterministic: arms sorted within clusters; clusters ordered by size
/// descending, then first arm ascending.
pub fn cluster<'a>(candidates: &[Candidate<'a>]) -> Vec<Cluster> {
    if candidates.is_empty() {
        return Vec::new();
    }
    let mut by_key: BTreeMap<String, SignatureGroup> = BTreeMap::new();
    for candidate in candidates {
        let sig = signature(candidate.src);
        let shingles = shingles(&tokenize_py(candidate.src));
        let entry = by_key.entry(sig.key()).or_default();
        entry.0.push(candidate.arm.to_string());
        entry.1.push(shingles);
    }
    let mut clusters: Vec<Cluster> = by_key
        .into_iter()
        .map(|(signature, (mut arms, shingle_sets))| {
            arms.sort();
            let mut diameter: f64 = 0.0;
            for (i, a) in shingle_sets.iter().enumerate() {
                for b in &shingle_sets[i + 1..] {
                    diameter = diameter.max(distance(a, b));
                }
            }
            Cluster {
                arms,
                signature,
                diameter,
            }
        })
        .collect();
    clusters.sort_by(|a, b| b.arms.len().cmp(&a.arms.len()).then(a.arms.cmp(&b.arms)));
    clusters
}

/// Render the per-wave report. Names every cluster's arms and tags; a
/// wave that collapsed to one approach says so, never as N successes.
pub fn render(total: usize, clusters: &[Cluster]) -> String {
    let mut out = format!(
        "{} {}, {} {}",
        total,
        plural(total, "candidate", "candidates"),
        clusters.len(),
        plural(clusters.len(), "approach", "approaches")
    );
    if clusters.len() == 1 && total > 1 {
        out.push_str(" (COLLAPSED: one approach, not N successes)");
    }
    for (i, cluster) in clusters.iter().enumerate() {
        out.push_str(&format!(
            "\ncluster {} ({}: {}): {} [diameter {:.2}]",
            i + 1,
            count(cluster.arms.len(), "arm"),
            cluster.arms.join(", "),
            cluster.signature,
            cluster.diameter
        ));
    }
    out
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        one.to_string()
    } else {
        many.to_string()
    }
}

fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EAGER_A: &str = "import torch\n\ndef forward(x):\n    return torch.relu(x) + 1\n";
    const EAGER_A_RENAMED: &str =
        "import torch\n\ndef forward(tensor):\n    return torch.relu(tensor) + 1\n";

    /// Renames normalise away: identical shingle sets, zero distance.
    #[test]
    fn renames_are_structurally_identical() {
        let a = shingles(&tokenize_py(EAGER_A));
        let b = shingles(&tokenize_py(EAGER_A_RENAMED));
        assert_eq!(a, b);
        assert_eq!(distance(&a, &b), 0.0);
    }

    /// Comments vanish leaving only their newline, and string contents
    /// never leak vocabulary: a `triton` in prose is not an approach.
    #[test]
    fn comments_and_strings_are_invisible() {
        assert_eq!(
            tokenize_py("# triton fp16\nx = 1\n"),
            tokenize_py("\nx = 1\n")
        );
        assert_eq!(
            tokenize_py("s = \"float16 triton\"\n"),
            tokenize_py("s = 'other'\n")
        );
        let noisy = "# triton fp16 BLOCK=64\nimport torch  # torch.compile\nx = 1\ns = \"float16 triton\"\n";
        let sig = signature(noisy);
        assert_eq!(sig.backend, vec!["eager"]);
        assert!(sig.precision.is_empty());
        assert!(sig.tiling.is_empty());
    }

    /// Numbers normalise: 128 vs 256 is the same shape of code.
    #[test]
    fn numbers_normalise() {
        let a = tokenize_py("BLOCK = 128\n");
        let b = tokenize_py("BLOCK = 256\n");
        assert_eq!(a, b);
    }

    /// Jaccard identities: empty pairs match, half-empty pairs do not.
    #[test]
    fn similarity_identities() {
        let empty: BTreeSet<Vec<String>> = BTreeSet::new();
        assert_eq!(similarity(&empty, &empty), 1.0);
        let one = shingles(&tokenize_py("x = 1\n"));
        assert_eq!(similarity(&one, &empty), 0.0);
        assert_eq!(similarity(&one, &one), 1.0);
    }

    /// Technique tags: triton, torch.compile, SDPA variant, half, tf32.
    #[test]
    fn technique_markers_tag() {
        let triton = "import triton\nimport triton.language as tl\n@triton.jit\ndef k(a):\n    tl.store(a, 1)\n";
        assert!(signature(triton).backend.contains(&"triton".to_string()));
        let tc = "import torch\nm = torch.compile(model)\n";
        assert!(signature(tc).backend.contains(&"torch-compile".to_string()));
        let sdpa = "import torch.nn.functional as F\nwith sdpa_kernel(SDPBackend.MATH):\n    out = F.scaled_dot_product_attention(q, k, v)\n";
        let sig = signature(sdpa);
        assert!(sig.backend.contains(&"sdpa".to_string()));
        assert!(sig.backend.contains(&"sdpa-math".to_string()));
        let half = "x = x.to(torch.float16)\n";
        assert!(signature(half).precision.contains(&"fp16".to_string()));
        let tf32 = "torch.backends.cuda.matmul.allow_tf32 = True\n";
        assert!(signature(tf32).precision.contains(&"tf32".to_string()));
        assert_eq!(signature(EAGER_A).backend, vec!["eager"]);
    }

    /// Tiling factors extract for triton, never for eager (an eager file
    /// mentioning BLOCK_SIZE is not a tiled kernel, and a gamer stuffing
    /// BLOCK_X into eager code manufactures nothing).
    #[test]
    fn tiling_is_gated_on_accelerator_backends() {
        let triton = "import triton\n@triton.jit\ndef k(a):\n    BLOCK_M = 128\n    x = 1\n";
        assert_eq!(signature(triton).tiling, vec![("BLOCK_M".to_string(), 128)]);
        // Kernel arguments are where real triton kernels put them.
        let args = "import triton\n@triton.jit\ndef k(a, BLOCK_M: tl.constexpr = 256, BLOCK_N: int = 64):\n    x = 1\n";
        assert_eq!(
            signature(args).tiling,
            vec![("BLOCK_M".to_string(), 256), ("BLOCK_N".to_string(), 64)]
        );
        // Comparisons are not bindings.
        let cmp = "import triton\n@triton.jit\ndef k(a):\n    ok = BLOCK_M == 128\n";
        assert!(signature(cmp).tiling.is_empty());
        let eager = "BLOCK_X = 1\nimport torch\ndef f(x):\n    return x\n";
        assert!(signature(eager).tiling.is_empty());
        assert_eq!(signature(eager).backend, vec!["eager"]);
    }

    /// Eight renames collapse to one cluster: one approach, not N
    /// successes -- the bead's headline case.
    #[test]
    fn a_renamed_wave_collapses() {
        let arms: Vec<String> = (0..8).map(|i| format!("arm{i}")).collect();
        let sources: Vec<String> = (0..8)
            .map(|i| {
                format!("import torch\n\ndef forward_v{i}(x):\n    return torch.relu(x) + 1\n")
            })
            .collect();
        let candidates: Vec<Candidate> = arms
            .iter()
            .zip(sources.iter())
            .map(|(arm, src)| Candidate { arm, src })
            .collect();
        let clusters = cluster(&candidates);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].arms.len(), 8);
        let report = render(candidates.len(), &clusters);
        assert!(report.contains("8 candidates, 1 approach"), "{report}");
        assert!(report.contains("COLLAPSED"), "{report}");
    }

    /// Dead code cannot manufacture diversity: a copy plus a junk
    /// function shares the signature, so it stays in the cluster.
    #[test]
    fn dead_code_does_not_split_a_cluster() {
        let junk = "import torch\n\ndef forward(x):\n    return torch.relu(x) + 1\n\ndef helper_unused(y):\n    z = y * 2\n    return z\n";
        let candidates = [
            Candidate {
                arm: "a",
                src: EAGER_A,
            },
            Candidate {
                arm: "b",
                src: junk,
            },
        ];
        let clusters = cluster(&candidates);
        assert_eq!(clusters.len(), 1, "{clusters:?}");
    }

    /// Different techniques split, and the report names arms per cluster.
    #[test]
    fn techniques_split_and_arms_are_named() {
        let tf32 = "import torch\ntorch.backends.cuda.matmul.allow_tf32 = True\nout = F.scaled_dot_product_attention(q, k, v)\n";
        let candidates = [
            Candidate {
                arm: "a",
                src: EAGER_A,
            },
            Candidate {
                arm: "b",
                src: tf32,
            },
        ];
        let clusters = cluster(&candidates);
        assert_eq!(clusters.len(), 2);
        let report = render(candidates.len(), &clusters);
        assert!(report.contains("2 candidates, 2 approaches"), "{report}");
        assert!(report.contains('a') && report.contains('b'), "{report}");
    }

    /// A precision marker is a technique difference: the fp16 line
    /// splits the cluster even though the code is near-identical.
    /// (Cross-signature structural merging was tried and removed: at a
    /// fixed threshold one added line moves the distance 0.5 in a tiny
    /// file and 0.03 in a large one, so no threshold is well-formed.)
    #[test]
    fn precision_markers_split() {
        let a = "import torch\ndef f(x):\n    return torch.relu(x)\n";
        let b = "import torch\ndef f(x):\n    y = x.to(torch.float16)\n    return torch.relu(y)\n";
        let sa = signature(a);
        let sb = signature(b);
        assert_ne!(sa.key(), sb.key());
        let candidates = [
            Candidate { arm: "a", src: a },
            Candidate { arm: "b", src: b },
        ];
        let clusters = cluster(&candidates);
        assert_eq!(clusters.len(), 2, "{clusters:?}");
    }

    /// Diameter reports tightness: renames score ~0, same technique
    /// written differently scores higher.
    #[test]
    fn diameter_separates_renames_from_rewrites() {
        let renamed = cluster(&[
            Candidate {
                arm: "a",
                src: EAGER_A,
            },
            Candidate {
                arm: "b",
                src: EAGER_A_RENAMED,
            },
        ]);
        assert_eq!(renamed.len(), 1);
        assert_eq!(renamed[0].diameter, 0.0);
        let junk = "import torch\n\ndef forward(x):\n    out = torch.relu(x)\n    tmp = out * 2\n    return out + 1\n";
        let rewritten = cluster(&[
            Candidate {
                arm: "a",
                src: EAGER_A,
            },
            Candidate {
                arm: "b",
                src: junk,
            },
        ]);
        assert_eq!(rewritten.len(), 1);
        assert!(rewritten[0].diameter > 0.0, "{:?}", rewritten[0].diameter);
    }
}

/// Verdicts the harness's own verification can produce, as the ledger
/// records them. Only `correct` can ever be a novelty win.
pub const VERDICTS: &[&str] = &[
    "correct",
    "incorrect",
    "unreliable",
    "stale",
    "void",
    "clock",
    "specialised",
    "reference",
];

/// One verified approach for a task: a signature that passed the
/// harness's own verification at least once.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ApproachEntry {
    /// Exact signature key.
    pub signature: String,
    /// Human-readable tags.
    pub tags: Vec<String>,
    /// Arm that verified it first.
    pub first_arm: String,
}

/// One attempt at a task, whatever its verdict. Adverse attempts are not
/// novelty wins, but they are the negative-results ledger (bead
/// farmerbob-x81s.11) in waiting: harness-measured reasons, not
/// self-reports.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Attempt {
    /// Arm that ran.
    pub arm: String,
    /// Approach signature it exhibited.
    pub signature: String,
    /// Harness verdict, one of [`VERDICTS`].
    pub verdict: String,
    /// Harness detail, verbatim.
    pub detail: String,
    /// Speedup when measured, if any.
    pub speedup: Option<f64>,
    /// Strategy the arm was asked for, one of [`STRATEGIES`] (bead
    /// farmerbob-x81s.10). Absent for attempts recorded before strategies
    /// existed, and for runs dispatched without one.
    #[serde(default)]
    pub asked_strategy: Option<String>,
}

/// Per-task ledger: verified approaches plus every attempt.
///
/// The two axes stay separate in the store: novelty is won by being
/// correct under a new signature, speedup is recorded alongside but
/// never summed in. The moment they are summed, the weighting becomes
/// the thing arms optimise, and the harness measures its own constant.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Ledger {
    /// Task name, as given.
    pub task: String,
    /// Verified approaches by signature key.
    pub approaches: BTreeMap<String, ApproachEntry>,
    /// Every recorded attempt, in arrival order.
    pub attempts: Vec<Attempt>,
}

impl Ledger {
    /// An empty ledger for a task.
    pub fn new(task: &str) -> Self {
        Ledger {
            task: task.to_string(),
            approaches: BTreeMap::new(),
            attempts: Vec::new(),
        }
    }

    /// Record an attempt. Returns true for a novelty win: the verdict is
    /// `correct` (by the harness's own verification -- this metric is
    /// worthless before the anti-cheat beads land) under a signature no
    /// verified approach has. Anything else is recorded and returns
    /// false: an adverse verdict is never novel, however new its
    /// signature, and a repeat is never novel, however fast.
    ///
    /// `asked` carries the dispatched strategy, if one was assigned
    /// (bead farmerbob-x81s.10): what the arm was told to write, next to
    /// what it wrote. Recorded, never scored on.
    pub fn record(
        &mut self,
        arm: &str,
        signature: &Signature,
        verdict: &str,
        detail: &str,
        speedup: Option<f64>,
        asked: Option<&str>,
    ) -> bool {
        let key = signature.key();
        self.attempts.push(Attempt {
            arm: arm.to_string(),
            signature: key.clone(),
            verdict: verdict.to_string(),
            detail: detail.to_string(),
            speedup,
            asked_strategy: asked.map(str::to_string),
        });
        if verdict == "correct" && !self.approaches.contains_key(&key) {
            self.approaches.insert(
                key,
                ApproachEntry {
                    signature: signature.key(),
                    tags: signature.tags(),
                    first_arm: arm.to_string(),
                },
            );
            return true;
        }
        false
    }

    /// Best measured speedup per signature, over correct attempts only.
    pub fn best_speedup(&self, signature: &str) -> Option<f64> {
        self.attempts
            .iter()
            .filter(|a| a.signature == signature && a.verdict == "correct")
            .filter_map(|a| a.speedup)
            .filter(|s| s.is_finite())
            .max_by(f64::total_cmp)
    }

    /// Strategy report: per asked strategy, attempts with arms,
    /// verdicts, and asked-versus-produced match (bead farmerbob-x81s.10).
    /// Unassigned attempts group last. Both directions queryable: which
    /// strategies pay on this task, and which arms do what they are told.
    pub fn strategy_report(&self) -> String {
        let mut out = format!("{}: strategy report", self.task);
        for (name, _) in STRATEGIES {
            let mine: Vec<&Attempt> = self
                .attempts
                .iter()
                .filter(|a| a.asked_strategy.as_deref() == Some(*name))
                .collect();
            if mine.is_empty() {
                continue;
            }
            let mut line = format!(
                "\nstrategy {name} ({} {}):",
                mine.len(),
                plural(mine.len(), "attempt", "attempts")
            );
            for attempt in mine {
                let backend = backend_of_key(&attempt.signature);
                let standing = match strategy_match(name, &backend) {
                    Some(true) => "MATCH",
                    Some(false) => "MISS",
                    None => "UNASKABLE",
                };
                let speed = attempt
                    .speedup
                    .map(|s| format!(" {s:.3}x"))
                    .unwrap_or_default();
                line.push_str(&format!(
                    " {} {}{} [{}];",
                    attempt.arm, attempt.verdict, speed, standing
                ));
            }
            out.push_str(&line);
        }
        let unassigned: Vec<&Attempt> = self
            .attempts
            .iter()
            .filter(|a| a.asked_strategy.is_none())
            .collect();
        if !unassigned.is_empty() {
            out.push_str(&format!(
                "\nno strategy ({} {}): {}",
                unassigned.len(),
                plural(unassigned.len(), "attempt", "attempts"),
                unassigned
                    .iter()
                    .map(|a| a.arm.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        out
    }

    /// Render the ledger: approaches with first arms and best speedups,
    /// then attempt counts. Novelty and speed side by side, never summed.
    pub fn render(&self) -> String {
        let mut out = format!(
            "{}: {} approaches, {} attempts",
            self.task,
            self.approaches.len(),
            self.attempts.len()
        );
        for (key, entry) in &self.approaches {
            let attempts = self.attempts.iter().filter(|a| a.signature == *key).count();
            let best = self
                .best_speedup(key)
                .map(|s| format!("{s:.3}x"))
                .unwrap_or_else(|| "unmeasured".to_string());
            out.push_str(&format!(
                "\napproach [{}] by {} ({} attempts, best {}): {}",
                entry.tags.join(", "),
                entry.first_arm,
                attempts,
                best,
                key
            ));
        }
        out
    }

    /// Spec-ready digest of what this task already taught the harness
    /// (bead farmerbob-x81s.11): verified approaches with the bar to beat,
    /// then adverse attempts grouped by signature with the harness's own
    /// measured reason each failed. Written for paste-injection into later
    /// specs, the way the spec critique feeds an implementation turn:
    /// every line is a harness measurement, never an agent self-report.
    /// Empty ledgers digest to one line saying so -- a spec carrying that
    /// line is honest about being the first wave.
    pub fn brief(&self) -> String {
        let mut out = format!(
            "## What this task already taught the harness ({} verified {}, {} recorded attempts)",
            self.approaches.len(),
            plural(self.approaches.len(), "approach", "approaches"),
            self.attempts.len()
        );
        if self.approaches.is_empty() && self.attempts.is_empty() {
            out.push_str("\nNothing yet: this is the first wave. Explore freely.");
            return out;
        }
        for entry in self.approaches.values() {
            let best = self
                .best_speedup(&entry.signature)
                .map(|s| format!("best measured {s:.3}x -- beat it or try elsewhere"))
                .unwrap_or_else(|| "no speedup measured yet".to_string());
            out.push_str(&format!(
                "\n- TRIED [{}] by {}: verified correct, {}. Do not re-explore it.",
                entry.tags.join(", "),
                entry.first_arm,
                best
            ));
        }
        // Adverse attempts, grouped by signature: what failed, and what
        // the harness itself measured about it.
        let mut adverse: BTreeMap<&str, Vec<&Attempt>> = BTreeMap::new();
        for attempt in &self.attempts {
            if attempt.verdict != "correct" {
                adverse
                    .entry(attempt.signature.as_str())
                    .or_default()
                    .push(attempt);
            }
        }
        for (signature, attempts) in &adverse {
            // Verdict, arm, and the harness's measured detail: why it
            // failed, in the harness's own words, never a self-report
            // (record() takes no self-report input at all).
            let reasons: BTreeSet<String> = attempts
                .iter()
                .map(|a| format!("{} ({}): {}", a.verdict, a.arm, a.detail))
                .collect();
            let tags = tags_of_key(signature);
            out.push_str(&format!(
                "\n- DEAD END [{}]: {}. Do not repeat it; a rerun must explain what changed.",
                if tags.is_empty() {
                    "(no technique markers)".to_string()
                } else {
                    tags.join(", ")
                },
                reasons.into_iter().collect::<Vec<_>>().join("; ")
            ));
        }
        out
    }
}

/// Backend tags for a signature key (`backend:a,b|...`), for the
/// asked-versus-produced check.
fn backend_of_key(key: &str) -> Vec<String> {
    key.split('|')
        .find(|section| section.starts_with("backend:"))
        .map(|section| {
            section["backend:".len()..]
                .split(',')
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Human tags for a signature key (`backend:a,b|precision:c|tiling:`),
/// for ledgers whose adverse signatures never earned an approach entry.
fn tags_of_key(key: &str) -> Vec<String> {
    key.split('|')
        .filter_map(|section| section.split(':').nth(1))
        .flat_map(|list| list.split(','))
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod ledger_tests {
    use super::*;

    fn sig_eager() -> Signature {
        signature("import torch\ndef f(x):\n    return x\n")
    }

    fn sig_tf32() -> Signature {
        signature("import torch\ntorch.backends.cuda.matmul.allow_tf32 = True\nout = f(x)\n")
    }

    /// A correct candidate under a new signature is a novelty win, and
    /// the second correct arrival under it is seen.
    #[test]
    fn novelty_win_then_seen() {
        let mut ledger = Ledger::new("t");
        assert!(ledger.record("a", &sig_eager(), "correct", "ok", Some(1.0), None));
        assert!(!ledger.record("b", &sig_eager(), "correct", "ok", Some(1.7), None));
        assert_eq!(ledger.approaches.len(), 1);
        assert_eq!(ledger.attempts.len(), 2);
        assert_eq!(ledger.best_speedup(&sig_eager().key()), Some(1.7));
    }

    /// An adverse verdict is never novel, however new its signature --
    /// and it is still recorded, with the harness's reason.
    #[test]
    fn adverse_verdicts_are_recorded_never_novel() {
        let mut ledger = Ledger::new("t");
        assert!(!ledger.record("a", &sig_tf32(), "incorrect", "values", None, None));
        assert!(!ledger.record("b", &sig_tf32(), "specialised", "heldout", None, None));
        assert!(ledger.approaches.is_empty());
        assert_eq!(ledger.attempts.len(), 2);
        // A later correct arrival under that signature IS novel: no
        // verified approach has it yet.
        assert!(ledger.record("c", &sig_tf32(), "correct", "ok", Some(0.9), None));
        assert_eq!(ledger.approaches.len(), 1);
    }

    /// Speedup never buys novelty: a faster repeat is seen, a slower
    /// first is novel. The axes stay separate in the store.
    #[test]
    fn speedup_is_recorded_never_summed() {
        let mut ledger = Ledger::new("t");
        assert!(ledger.record("slow", &sig_eager(), "correct", "ok", Some(0.5), None));
        assert!(!ledger.record("fast", &sig_eager(), "correct", "ok", Some(3.0), None));
        let rendered = ledger.render();
        assert!(rendered.contains("1 approaches"), "{rendered}");
        assert!(rendered.contains("slow"), "{rendered}");
        assert!(rendered.contains("3.000x"), "{rendered}");
    }

    /// The verdict vocabulary is closed: VERDICTS is what record accepts.
    #[test]
    fn verdict_vocabulary_is_closed_and_correct_first() {
        assert!(VERDICTS.contains(&"correct"));
        assert_eq!(VERDICTS.len(), 8);
    }

    /// An empty ledger briefs to one honest line: the spec carrying it
    /// is the first wave.
    #[test]
    fn empty_ledger_briefs_first_wave() {
        let brief = Ledger::new("t").brief();
        assert!(brief.contains("first wave"), "{brief}");
    }

    /// The brief names verified approaches with the bar to beat, and
    /// dead ends with the harness's own reasons -- never self-reports
    /// (there is no self-report input to record() at all).
    #[test]
    fn brief_carries_approaches_and_dead_ends() {
        let mut ledger = Ledger::new("t");
        ledger.record("a", &sig_tf32(), "correct", "ok", Some(1.7), None);
        ledger.record(
            "b",
            &sig_eager(),
            "incorrect",
            "mismatch: Output mismatch",
            None,
            None,
        );
        ledger.record("c", &sig_eager(), "specialised", "heldout", None, None);
        let brief = ledger.brief();
        assert!(brief.contains("TRIED"), "{brief}");
        assert!(brief.contains("tf32"), "{brief}");
        assert!(brief.contains("1.700x"), "{brief}");
        assert!(brief.contains("DEAD END"), "{brief}");
        assert!(brief.contains("incorrect (b)"), "{brief}");
        assert!(brief.contains("specialised (c)"), "{brief}");
        assert!(brief.contains("Output mismatch"), "{brief}");
    }
}

/// Named dispatch strategies with the backend markers they promise
/// (bead farmerbob-x81s.10): coverage is assigned, not an accident of
/// temperature. Each strategy names what the arm was asked for; the
/// produced signature says what it actually wrote.
///
/// The set is closed like [`VERDICTS`]: a typo must not silently invent
/// a strategy, or strategy-to-outcome dissolves into singletons. It is
/// also initial, not exhaustive -- kernel-family strategies for the
/// present testbed. Extend it with the markers in hand, never by guess.
///
/// `fusion` promises a fused kernel by any means: triton or a raw CUDA
/// extension both fuse. `eager-baseline` promises stock torch: the
/// control arm, there to price the others against.
pub const STRATEGIES: &[(&str, &[&str])] = &[
    ("sdpa-math", &["sdpa", "sdpa-math"]),
    ("sdpa-flash", &["sdpa", "sdpa-flash"]),
    ("sdpa-efficient", &["sdpa", "sdpa-efficient"]),
    ("sdpa-cudnn", &["sdpa", "sdpa-cudnn"]),
    ("triton-tiled", &["triton"]),
    ("torch-compile", &["torch-compile"]),
    ("fusion", &["triton", "cuda-ext"]),
    ("eager-baseline", &["eager"]),
];

/// Whether produced backend tags honor an asked strategy: every promised
/// marker present. An arm asked to fuse that writes the same tiled
/// kernel as everyone else misses here -- a finding about the arm,
/// invisible without the metric.
pub fn strategy_match(asked: &str, produced_backend: &[String]) -> Option<bool> {
    let (_, expected) = STRATEGIES.iter().find(|(name, _)| *name == asked)?;
    if asked == "fusion" {
        // Any fusing backend honors fusion; requiring all would demand
        // the arm write two kernels.
        Some(
            expected
                .iter()
                .any(|m| produced_backend.iter().any(|t| t == m)),
        )
    } else {
        Some(
            expected
                .iter()
                .all(|m| produced_backend.iter().any(|t| t == m)),
        )
    }
}

#[cfg(test)]
mod strategy_tests {
    use super::*;

    fn backend(tags: &[&str]) -> Vec<String> {
        tags.iter().map(|t| t.to_string()).collect()
    }

    /// Asked and produced agree, including the fusion alternation; an
    /// unknown strategy is unaskable, never a mismatch.
    #[test]
    fn match_semantics() {
        assert_eq!(
            strategy_match("sdpa-math", &backend(&["sdpa", "sdpa-math"])),
            Some(true)
        );
        assert_eq!(
            strategy_match("sdpa-math", &backend(&["sdpa"])),
            Some(false)
        );
        assert_eq!(
            strategy_match("sdpa-math", &backend(&["eager"])),
            Some(false)
        );
        assert_eq!(strategy_match("fusion", &backend(&["triton"])), Some(true));
        assert_eq!(
            strategy_match("fusion", &backend(&["cuda-ext"])),
            Some(true)
        );
        assert_eq!(strategy_match("fusion", &backend(&["eager"])), Some(false));
        assert_eq!(
            strategy_match("eager-baseline", &backend(&["eager"])),
            Some(true)
        );
        assert_eq!(strategy_match("nope", &backend(&["eager"])), None);
    }

    /// The strategy report groups by asked strategy with MATCH/MISS and
    /// lists the unassigned last: both directions queryable.
    #[test]
    fn strategy_report_groups_and_judges() {
        let mut ledger = Ledger::new("t");
        let eager = signature("import torch\ndef f(x):\n    return x\n");
        let tf32sig = signature(
            "import torch\ntorch.backends.cuda.matmul.allow_tf32 = True\ndef f(x):\n    return x\n",
        );
        ledger.record(
            "a",
            &eager,
            "correct",
            "ok",
            Some(1.0),
            Some("eager-baseline"),
        );
        ledger.record(
            "b",
            &tf32sig,
            "correct",
            "ok",
            Some(0.9),
            Some("triton-tiled"),
        );
        ledger.record("c", &eager, "correct", "ok", Some(1.1), None);
        let report = ledger.strategy_report();
        assert!(report.contains("strategy eager-baseline"), "{report}");
        assert!(report.contains("a correct 1.000x [MATCH]"), "{report}");
        assert!(report.contains("strategy triton-tiled"), "{report}");
        assert!(report.contains("b correct 0.900x [MISS]"), "{report}");
        assert!(report.contains("no strategy (1 attempt): c"), "{report}");
    }
}
