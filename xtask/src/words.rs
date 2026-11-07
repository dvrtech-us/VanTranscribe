use crate::{EvalResult, fail};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Normalization {
    ExactWhitespace,
    UnicodeLowercasePunctuationToSpace,
}
pub fn tokens(text: &str, normalization: Normalization) -> Vec<String> {
    match normalization {
        Normalization::ExactWhitespace => text.split_whitespace().map(str::to_owned).collect(),
        Normalization::UnicodeLowercasePunctuationToSpace => text
            .chars()
            .flat_map(char::to_lowercase)
            .map(|c| if c.is_alphanumeric() { c } else { ' ' })
            .collect::<String>()
            .split_whitespace()
            .map(str::to_owned)
            .collect(),
    }
}
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct WordScore {
    pub reference_words: u64,
    pub hypothesis_words: u64,
    pub substitutions: u64,
    pub deletions: u64,
    pub insertions: u64,
    pub wer: Option<f64>,
}
impl WordScore {
    pub fn finish(&mut self) {
        self.wer = (self.reference_words > 0).then(|| {
            (self.substitutions + self.deletions + self.insertions) as f64
                / self.reference_words as f64
        });
    }
    pub fn accumulate(&mut self, other: &Self) {
        self.reference_words += other.reference_words;
        self.hypothesis_words += other.hypothesis_words;
        self.substitutions += other.substitutions;
        self.deletions += other.deletions;
        self.insertions += other.insertions;
        self.finish();
    }
}
#[derive(Clone, Copy, Default)]
struct Cell {
    s: u64,
    d: u64,
    i: u64,
}
impl Cell {
    fn cost(self) -> u64 {
        self.s + self.d + self.i
    }
}
/// Levenshtein path counts. On equal edit distance prefer match/substitution,
/// then deletion, then insertion; WER itself is independent of this tie rule.
pub fn score(
    reference: &str,
    hypothesis: &str,
    normalization: Normalization,
) -> EvalResult<WordScore> {
    let r = tokens(reference, normalization);
    let h = tokens(hypothesis, normalization);
    if r.len() > 100_000 || h.len() > 100_000 {
        return Err(fail(
            "WER unit exceeds100000 words; split into paired utterances",
        ));
    }
    if r.len().saturating_mul(h.len()) > 100_000_000 {
        return Err(fail(
            "WER pair exceeds100million alignment cells; split into reference-paired utterance files",
        ));
    }
    let mut prior: Vec<_> = (0..=h.len())
        .map(|i| Cell {
            i: i as u64,
            ..Default::default()
        })
        .collect();
    let mut row = vec![Cell::default(); h.len() + 1];
    for (a, word) in r.iter().enumerate() {
        row[0] = Cell {
            d: a as u64 + 1,
            ..Default::default()
        };
        for (b, other) in h.iter().enumerate() {
            if word == other {
                row[b + 1] = prior[b];
                continue;
            }
            let mut substitution = prior[b];
            substitution.s += 1;
            let mut deletion = prior[b + 1];
            deletion.d += 1;
            let mut insertion = row[b];
            insertion.i += 1;
            row[b + 1] = [substitution, deletion, insertion]
                .into_iter()
                .min_by_key(|cell| cell.cost())
                .unwrap();
        }
        std::mem::swap(&mut prior, &mut row);
    }
    let last = prior[h.len()];
    let mut result = WordScore {
        reference_words: r.len() as u64,
        hypothesis_words: h.len() as u64,
        substitutions: last.s,
        deletions: last.d,
        insertions: last.i,
        wer: None,
    };
    result.finish();
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hand_counted_edits_and_explicit_normalization() {
        let result = score(
            "One, TWO three four",
            "one two five",
            Normalization::UnicodeLowercasePunctuationToSpace,
        )
        .unwrap();
        assert_eq!(result.reference_words, 4);
        assert_eq!(
            result.substitutions + result.deletions + result.insertions,
            2
        );
        assert_eq!(result.wer, Some(0.5));
        assert_eq!(
            tokens(
                "don't re-enter CAFÉ",
                Normalization::UnicodeLowercasePunctuationToSpace
            ),
            ["don", "t", "re", "enter", "café"]
        );
        assert_ne!(
            score("Hi", "hi", Normalization::ExactWhitespace)
                .unwrap()
                .wer,
            Some(0.0)
        );
    }
    #[test]
    fn empty_reference_keeps_insertions_without_inventing_a_rate() {
        let r = score("", "extra words", Normalization::ExactWhitespace).unwrap();
        assert_eq!(r.insertions, 2);
        assert_eq!(r.wer, None);
        assert_eq!(
            score("one two", "", Normalization::ExactWhitespace)
                .unwrap()
                .deletions,
            2
        );
    }
}
