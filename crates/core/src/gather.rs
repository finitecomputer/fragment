//! Ordered maps and sets made by inserting, not by `collect()`: a
//! `BTreeMap` (and `serde_json`'s `Map`, one here) or `BTreeSet` collected
//! sorts its items first, with a stable sort compiled once for every item
//! and iterator type, about 5 KB of the cell's Wasm each (its budget:
//! FIN-327). Inserting each instead makes the same collection, ordered by
//! key, the last value of a repeated key kept, as `collect()` keeps it.

/// `gathered()`: `collect()`'s collection, by `Extend` (inserting each);
/// `try_gathered()`: `collect::<Result<_, _>>()`'s, its first error or
/// the collection.
pub trait Gathered: Iterator + Sized {
    fn gathered<C: Default + Extend<Self::Item>>(self) -> C {
        let mut c = C::default();
        c.extend(self);
        c
    }

    fn try_gathered<T, E, C: Default + Extend<T>>(self) -> Result<C, E>
    where
        Self: Iterator<Item = Result<T, E>>,
    {
        let mut c = C::default();
        // bounded by the iterator, which the caller bounds
        for item in self {
            c.extend(Some(item?));
        }
        Ok(c)
    }
}

impl<I: Iterator> Gathered for I {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    /// Goal: what `gathered()` makes is what `collect()` makes. Method:
    /// pairs with a repeated key (the last value kept), out of order, into
    /// a map, a JSON object and a set.
    #[test]
    fn gathered_is_collected() {
        let pairs = || vec![("b".to_string(), 2), ("a".to_string(), 1), ("b".to_string(), 3), ("c".to_string(), 0)].into_iter();
        let gathered: BTreeMap<String, i32> = pairs().gathered();
        assert_eq!(gathered, pairs().collect::<BTreeMap<_, _>>());
        assert_eq!(gathered["b"], 3, "the last value of a repeated key");
        let object: serde_json::Map<String, serde_json::Value> = pairs().map(|(k, v)| (k, v.into())).gathered();
        assert_eq!(object, pairs().map(|(k, v)| (k, v.into())).collect::<serde_json::Map<_, _>>());
        let set: BTreeSet<String> = pairs().map(|(k, _)| k).gathered();
        assert_eq!(set.into_iter().collect::<Vec<_>>(), ["a", "b", "c"]);
        let tried = || pairs().map(|(k, v)| if v == 0 { Err(format!("{k} is zero")) } else { Ok((k, v)) });
        assert_eq!(tried().try_gathered::<_, _, BTreeMap<String, i32>>(), tried().collect::<Result<BTreeMap<_, _>, _>>(), "the first error");
        assert_eq!(tried().take(3).try_gathered::<_, _, BTreeMap<String, i32>>(), Ok(pairs().take(3).collect()), "or the whole collection");
    }
}
