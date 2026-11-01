use std::collections::HashSet;

use regex::Regex;
use savestates_i18n::TRANSLATIONS_CSV;

fn reader() -> csv::Reader<&'static [u8]> {
    csv::ReaderBuilder::new().from_reader(TRANSLATIONS_CSV.as_bytes())
}

fn records() -> Vec<csv::StringRecord> {
    reader()
        .records()
        .collect::<Result<Vec<_>, _>>()
        .expect("translations.csv parses as RFC 4180")
}

#[test]
fn header_is_key_pt_br_en() {
    let mut rdr = reader();
    let header = rdr.headers().expect("header").clone();
    assert_eq!(header.iter().collect::<Vec<_>>(), ["key", "pt-BR", "en"]);
}

#[test]
fn every_record_has_three_non_empty_fields() {
    for record in records() {
        assert_eq!(record.len(), 3, "record {record:?} must have 3 fields");
        for field in record.iter() {
            assert!(!field.trim().is_empty(), "empty field in {record:?}");
        }
    }
}

#[test]
fn keys_are_unique_and_lowercase_dotted() {
    let shape = Regex::new(r"^[a-z0-9_]+(\.[a-z0-9_]+)*$").expect("regex");
    let mut seen = HashSet::new();
    for record in records() {
        let key = record.get(0).expect("key column");
        assert!(shape.is_match(key), "key {key:?} is not lowercase dotted");
        assert!(seen.insert(key.to_string()), "duplicate key {key:?}");
    }
}

#[test]
fn every_one_has_an_other() {
    let keys: HashSet<String> = records()
        .iter()
        .map(|r| r.get(0).expect("key column").to_string())
        .collect();
    for key in &keys {
        if let Some(stem) = key.strip_suffix(".one") {
            assert!(
                keys.contains(&format!("{stem}.other")),
                "{key} has no {stem}.other"
            );
        }
    }
}
