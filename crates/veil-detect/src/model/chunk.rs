//! Chunking and input assembly for the layer-3 model. Pure logic, compiled without the `model` feature so
//! it is always tested.

use std::ops::Range;

/// Token windows over `n` text tokens: each window holds at most `window` tokens, consecutive windows
/// overlap by `overlap`, and at most `max_chunks` windows are produced (the rest of a very long text is
/// covered by the fast layers only).
pub fn windows(n: usize, window: usize, overlap: usize, max_chunks: usize) -> Vec<Range<usize>> {
    if n == 0 || window == 0 || max_chunks == 0 {
        return Vec::new();
    }
    let stride = window.saturating_sub(overlap).max(1);
    let mut out = Vec::new();
    let mut start = 0;
    loop {
        let end = (start + window).min(n);
        out.push(start..end);
        if end == n || out.len() == max_chunks {
            break;
        }
        start += stride;
    }
    out
}

/// `prefix + chunk + suffix` as model input ids.
pub fn assemble(prefix: &[i64], chunk: &[u32], suffix: &[i64]) -> Vec<i64> {
    let mut ids = Vec::with_capacity(prefix.len() + chunk.len() + suffix.len());
    ids.extend_from_slice(prefix);
    ids.extend(chunk.iter().map(|&t| t as i64));
    ids.extend_from_slice(suffix);
    ids
}

/// A padded batch: row-major ids and attention mask with shape `[rows, cols]`.
#[derive(Debug, PartialEq)]
pub struct Batch {
    pub ids: Vec<i64>,
    pub mask: Vec<i64>,
    pub rows: usize,
    pub cols: usize,
}

pub fn pad_batch(rows: &[Vec<i64>], pad_id: i64) -> Batch {
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut ids = Vec::with_capacity(rows.len() * cols);
    let mut mask = Vec::with_capacity(rows.len() * cols);
    for r in rows {
        ids.extend_from_slice(r);
        mask.extend(std::iter::repeat_n(1, r.len()));
        ids.extend(std::iter::repeat_n(pad_id, cols - r.len()));
        mask.extend(std::iter::repeat_n(0, cols - r.len()));
    }
    Batch { ids, mask, rows: rows.len(), cols }
}

/// Max over rows for each column of a row-major `[rows, n]` score matrix.
pub fn max_per_column(scores: &[f32], n: usize) -> Vec<f32> {
    let mut out = vec![f32::NEG_INFINITY; n];
    if n == 0 {
        return out;
    }
    for row in scores.chunks_exact(n) {
        for (o, &s) in out.iter_mut().zip(row) {
            *o = o.max(s);
        }
    }
    out
}

pub fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// Split text into the words GLiNER-style processors use: `\w+(?:[-_]\w+)*|\S`.
pub fn split_words(text: &str) -> Vec<&str> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"\w+(?:[-_]\w+)*|\S").expect("word regex"));
    re.find_iter(text).map(|m| m.as_str()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_cover_text_with_overlap() {
        assert_eq!(windows(10, 4, 1, 32), vec![0..4, 3..7, 6..10]);
        assert_eq!(windows(3, 4, 1, 32), vec![0..3]);
        assert_eq!(windows(0, 4, 1, 32), Vec::<Range<usize>>::new());
        assert_eq!(windows(100, 10, 2, 3), vec![0..10, 8..18, 16..26]);
        // Overlap >= window still advances.
        assert_eq!(windows(3, 2, 5, 32), vec![0..2, 1..3]);
    }

    #[test]
    fn real_sizes() {
        let w = windows(2000, 512 - 40, 64, 32);
        assert_eq!(w.first().unwrap().start, 0);
        assert_eq!(w.last().unwrap().end, 2000);
        assert!(w.iter().all(|r| r.len() <= 472));
    }

    #[test]
    fn assemble_and_pad() {
        let a = assemble(&[1, 2], &[7, 8, 9], &[3]);
        assert_eq!(a, vec![1, 2, 7, 8, 9, 3]);
        let b = pad_batch(&[a, vec![1, 2, 7, 3]], 0);
        assert_eq!(b.rows, 2);
        assert_eq!(b.cols, 6);
        assert_eq!(b.ids, vec![1, 2, 7, 8, 9, 3, 1, 2, 7, 3, 0, 0]);
        assert_eq!(b.mask, vec![1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0]);
    }

    #[test]
    fn column_max_and_sigmoid() {
        assert_eq!(max_per_column(&[0.1, 0.9, 0.5, 0.2], 2), vec![0.5, 0.9]);
        assert!((sigmoid(0.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn words_split_like_gliner() {
        assert_eq!(split_words("Hello, world! my-key_1 ok."), vec!["Hello", ",", "world", "!", "my-key_1", "ok", "."]);
    }
}
