//! Git names for the timelines the app makes (spec §5.1): `savestates/<adjective>-<noun>`.

use crate::timelines::APP_PREFIX;

const ADJECTIVES: &[&str] = &[
    "graceful", "brave", "bright", "calm", "cheerful", "clever", "cozy", "curious", "daring",
    "eager", "gentle", "glad", "golden", "happy", "honest", "humble", "jolly", "kind", "lively",
    "lucky", "merry", "mighty", "modest", "noble", "patient", "playful", "polite", "proud",
    "quick", "quiet", "radiant", "shiny", "silly", "sleepy", "smart", "sunny", "swift", "tender",
    "tidy", "trusty", "warm", "wise", "witty", "zesty",
];

const NOUNS: &[&str] = &[
    "elephant", "badger", "beaver", "bison", "camel", "cheetah", "dolphin", "eagle", "falcon",
    "ferret", "giraffe", "hedgehog", "heron", "koala", "lemur", "leopard", "lion", "llama", "lynx",
    "meerkat", "moose", "otter", "owl", "panda", "parrot", "pelican", "penguin", "puffin",
    "rabbit", "raccoon", "robin", "salmon", "seal", "sparrow", "squirrel", "swan", "tiger",
    "toucan", "turtle", "walrus", "whale", "wombat", "zebra",
];

/// How many seeded picks are tried before a number is added to the last one.
const PICKS: usize = 8;

/// A fresh name for a new app timeline that none of `taken` (branch names) uses. Pass a seed
/// from the clock (its nanoseconds); the same seed and `taken` give the same name.
pub fn new_app_branch_name(taken: &[String], seed: u64) -> String {
    name_from(ADJECTIVES, NOUNS, taken, seed)
}

/// [`new_app_branch_name`] with its own word lists. Tries [`PICKS`] seeded picks, then adds
/// `-2`, `-3`… to the last pick until the name is free.
pub(crate) fn name_from(adjs: &[&str], nouns: &[&str], taken: &[String], seed: u64) -> String {
    let mut state = seed;
    let mut last = String::new();
    for _ in 0..PICKS {
        let adj = adjs[pick(&mut state, adjs.len())];
        let noun = nouns[pick(&mut state, nouns.len())];
        last = format!("{APP_PREFIX}{adj}-{noun}");
        if is_free(&last, taken) {
            return last;
        }
    }
    (2u64..)
        .map(|n| format!("{last}-{n}"))
        .find(|name| is_free(name, taken))
        .unwrap_or(last)
}

/// A name git can create next to `taken`: not the same name in any letter case (refs are files
/// on Windows), and neither a folder of another branch nor inside one.
fn is_free(name: &str, taken: &[String]) -> bool {
    let name = name.to_ascii_lowercase();
    taken.iter().all(|t| {
        let t = t.to_ascii_lowercase();
        t != name && !t.starts_with(&format!("{name}/")) && !name.starts_with(&format!("{t}/"))
    })
}

/// The next index below `len` from a SplitMix64 sequence (a well-mixed, dependency-free PRNG).
fn pick(state: &mut u64, len: usize) -> usize {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    // `len` is small, so the bias of a plain remainder does not matter.
    usize::try_from(z % len as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{name_from, new_app_branch_name};

    #[test]
    fn new_names_are_prefixed_words_avoiding_taken_ones() {
        let shape = regex::Regex::new(r"^savestates/[a-z]+-[a-z]+$").unwrap();
        let taken: Vec<String> = (0..50u64)
            .map(|seed| new_app_branch_name(&[], seed))
            .collect();
        for seed in 0..200u64 {
            let name = new_app_branch_name(&taken, seed.wrapping_mul(7_919) + 1_000);
            assert!(shape.is_match(&name), "{name}");
            assert!(!taken.contains(&name), "{name} is taken");
        }

        let adjs = ["calm", "bold"];
        let nouns = ["fox", "owl"];
        let all: Vec<String> = adjs
            .iter()
            .flat_map(|a| nouns.iter().map(move |n| format!("savestates/{a}-{n}")))
            .collect();
        for seed in 0..20u64 {
            let name = name_from(&adjs, &nouns, &all, seed);
            assert!(name.ends_with("-2"), "{name}");
            assert!(
                all.contains(&name.trim_end_matches("-2").to_owned()),
                "{name}"
            );
        }
    }
}
