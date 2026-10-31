//! Decoys: sections of made-up words with random unit vectors, added to a
//! corpus to measure the engines at a larger size. Random vectors sit near
//! orthogonal to every question's, and their words are no language's, so
//! they cost each engine its work without moving its answers much.

use crate::corpus::{Page, Section, Sectioned};

const SYLLABLES: [&str; 20] = ["ka", "lo", "mi", "ten", "ru", "sa", "vo", "dre", "ni", "pel", "qua", "zo", "bri", "fen", "go", "hul", "ix", "jor", "wex", "yu"];
const WORDS_PER_SECTION: usize = 150;

/// A small deterministic generator (xorshift64*), so runs compare.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// A standard normal draw (Box–Muller).
    fn normal(&mut self) -> f32 {
        let (u, v) = (self.unit().max(f64::MIN_POSITIVE), self.unit());
        ((-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()) as f32
    }

    fn word(&mut self) -> String {
        (0..2 + self.next() % 2).map(|_| SYLLABLES[(self.next() % SYLLABLES.len() as u64) as usize]).collect()
    }
}

/// `n` one-section decoy pages, and their vectors of `dims`.
pub fn make(n: usize, dims: usize) -> (Sectioned, Vec<Vec<f32>>) {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut pages = Vec::with_capacity(n);
    let mut vectors = Vec::with_capacity(n);
    for i in 0..n {
        let path = format!("~decoys/{i:06}.md");
        let body: Vec<String> = (0..WORDS_PER_SECTION).map(|_| rng.word()).collect();
        let section = Section {
            id: format!("{path}#0"),
            path: path.clone(),
            title: format!("{} {}", rng.word(), rng.word()),
            ancestry: Vec::new(),
            heading: None,
            body: body.join(" "),
        };
        let mut v: Vec<f32> = (0..dims).map(|_| rng.normal()).collect();
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter_mut().for_each(|x| *x /= norm);
        pages.push((Page { path, text: String::new() }, vec![section]));
        vectors.push(v);
    }
    (pages, vectors)
}

#[cfg(test)]
mod tests {
    #[test]
    fn decoys_are_stable_and_unit() {
        let (a, va) = super::make(3, 8);
        let (b, vb) = super::make(3, 8);
        assert_eq!(a[2].1[0].body, b[2].1[0].body);
        assert_eq!(va, vb);
        assert!(va.iter().all(|v| (v.iter().map(|x| x * x).sum::<f32>() - 1.0).abs() < 1e-5));
    }
}
