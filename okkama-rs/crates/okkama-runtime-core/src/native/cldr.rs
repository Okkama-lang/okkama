//! `runtime/cldr`: таблицы CLDR как данные рантайма (#223). Генераторы
//! `tools/generate-cldr*.py` пишут их в `cldr-<семейство>.dat` (gzip текста,
//! из `data/`); файл распаковывается и индексируется при первом обращении к
//! любой его таблице. Строка файла — таблица, ключ и поля через табуляцию;
//! имя таблицы начинается с семейства и точки (`numbers.row`). Sparrow берёт
//! этот native через мост (`native_origin`).
use super::Error;
use super::support::{failure, int, int_value, string, string_array, text};
#[cfg(feature = "cldr")]
use crate::data::gunzip;
use crate::data::gunzip_checked;
use crate::{Value, abi::Native};
use std::collections::HashMap;
use std::sync::OnceLock;

/// Таблица → ключ → поля строки (текст после ключа).
type Index = HashMap<&'static str, HashMap<&'static str, &'static str>>;

pub fn handles(native: Native) -> bool {
    matches!(native, Native::CldrCldrFields | Native::CldrCldrField | Native::CldrCldrSize)
}

pub fn execute(native: Native, args: &[Value]) -> Result<Value, Error> {
    let (table, key) = (string(&args[0])?, string(&args[1])?);
    let row = record(native, table, key)?;
    match native {
        // One field (#943): a caller that reads a few fields of a long record pays for those only.
        Native::CldrCldrField => {
            let index = int(&args[2])?.to_i64();
            let at = index.and_then(|index| usize::try_from(index).ok());
            let field = at.and_then(|at| row.and_then(|row| row.split('\t').nth(at)));
            match field {
                Some(field) => Ok(text(field.to_string())),
                None => Err(failure(
                    "core.cldr",
                    "invalid_input",
                    native,
                    format!("CLDR record {table:?} {key:?} has no field {}", index.map_or("out of range".into(), |index| index.to_string())),
                )),
            }
        }
        Native::CldrCldrSize => Ok(int_value(row.map_or(0, |row| row.split('\t').count() as i64))),
        _ => Ok(match row {
            Some(fields) => string_array(fields.split('\t')),
            None => string_array(std::iter::empty::<&str>()),
        }),
    }
}

/// The fields text of `key` in `table`; `None` without such a key.
fn record(native: Native, table: &str, key: &str) -> Result<Option<&'static str>, Error> {
    let family = table.split_once('.').map(|(family, _)| family);
    let at = family.and_then(|family| FILES.iter().position(|(name, _)| *name == family));
    let Some(at) = at else {
        return Err(failure("core.cldr", "invalid_input", native, format!("Unknown CLDR table {table:?}")));
    };
    let Some(index) = index(at) else {
        return Err(failure(
            "core.cldr",
            "unavailable",
            native,
            format!("The CLDR data {} is not loaded", FILES[at].1),
        ));
    };
    let Some(rows) = index.get(table) else {
        return Err(failure("core.cldr", "invalid_input", native, format!("Unknown CLDR table {table:?}")));
    };
    Ok(rows.get(key).copied())
}

/// The families and their files, in `data/`.
pub const FILES: &[(&str, &str)] = &[
    ("numbers", "cldr-numbers.dat"),
    ("dates", "cldr-dates.dat"),
    ("zones", "cldr-zones.dat"),
    ("currencies", "cldr-currencies.dat"),
    ("locales", "cldr-locales.dat"),
];

/// The text of each family, unpacked at its first use (feature `cldr`) or given by [`provide`].
static TEXTS: [OnceLock<String>; 5] = [const { OnceLock::new() }; 5];
static INDEXES: [OnceLock<Index>; 5] = [const { OnceLock::new() }; 5];

/// The tables in the binary, in [`FILES`]' order.
#[cfg(feature = "cldr")]
const EMBEDDED: [&[u8]; 5] = [
    include_bytes!("../../data/cldr-numbers.dat"),
    include_bytes!("../../data/cldr-dates.dat"),
    include_bytes!("../../data/cldr-zones.dat"),
    include_bytes!("../../data/cldr-currencies.dat"),
    include_bytes!("../../data/cldr-locales.dat"),
];

fn family_text(at: usize) -> Option<&'static str> {
    #[cfg(feature = "cldr")]
    let text = Some(TEXTS[at].get_or_init(|| String::from_utf8(gunzip(EMBEDDED[at])).expect("UTF-8 CLDR table")));
    #[cfg(not(feature = "cldr"))]
    let text = TEXTS[at].get();
    text.map(String::as_str)
}

/// The index of the family at `at` in [`FILES`]; `None` while its text is not there.
fn index(at: usize) -> Option<&'static Index> {
    let text = family_text(at)?;
    Some(INDEXES[at].get_or_init(|| build(text)))
}

/// See [`super::provide_cldr`]. A text that is not gzip, not UTF-8, or not CLDR records is refused.
pub(super) fn provide(file: &str, gzip: &[u8]) -> Result<(), String> {
    let at = FILES.iter().position(|(_, name)| *name == file).ok_or_else(|| format!("{file} is not a CLDR file"))?;
    let text = gunzip_checked(gzip).and_then(|bytes| String::from_utf8(bytes).ok()).ok_or_else(|| format!("{file} is not gzip UTF-8 text"))?;
    if !text.lines().all(|line| line.splitn(3, '\t').count() == 3) {
        return Err(format!("{file} has a malformed CLDR record"));
    }
    let _ = TEXTS[at].set(text);
    Ok(())
}

fn build(text: &'static str) -> Index {
    let mut index = Index::new();
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(table), Some(key), Some(fields)) = (parts.next(), parts.next(), parts.next())
        else {
            panic!("malformed CLDR record {line:?}");
        };
        index.entry(table).or_default().insert(key, fields);
    }
    index
}

#[cfg(all(test, feature = "cldr"))]
mod tests {
    use super::*;

    fn family(name: &str) -> Option<&'static Index> {
        index(FILES.iter().position(|(family, _)| *family == name)?)
    }

    fn fields(table: &str, key: &str) -> Option<Vec<&'static str>> {
        let (name, _) = table.split_once('.')?;
        let rows = family(name)?.get(table)?;
        rows.get(key).map(|fields| fields.split('\t').collect())
    }

    #[test]
    fn every_family_answers_its_tables() {
        assert_eq!(fields("numbers.digits", "latn"), Some(vec!["0123456789"]));
        assert_eq!(fields("currencies.digits", "JPY"), Some(vec!["0"]));
        assert_eq!(fields("locales.parent", "en-GB"), Some(vec!["en-001"]));
        assert_eq!(fields("locales.likely", "und"), Some(vec!["en-Latn-US"]));
        // CLDR keys zones by their old names.
        assert_eq!(fields("zones.canonical", "Asia/Kolkata"), Some(vec!["Asia/Calcutta"]));
        assert!(fields("dates.profile", "ru").is_some());
        // A row keeps its empty fields.
        assert_eq!(fields("numbers.row", "en").map(|row| row.len()), Some(7));
        assert_eq!(fields("numbers.digits", "no-such-system"), None);
        assert!(family("nowhere").is_none());
    }

    /// #1125: a given file is checked, and a family already there stays; an unknown table is still refused.
    #[test]
    fn a_given_file_is_checked_and_a_loaded_family_stays() {
        assert_eq!(provide("cldr-elsewhere.dat", EMBEDDED[0]), Err("cldr-elsewhere.dat is not a CLDR file".to_string()));
        assert_eq!(provide("cldr-dates.dat", b"not gzip"), Err("cldr-dates.dat is not gzip UTF-8 text".to_string()));
        let mut broken = Vec::new();
        broken.extend_from_slice(&[0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0]);
        broken.extend(miniz_oxide::deflate::compress_to_vec(b"numbers.row\tonly-two-parts\n", 6));
        broken.extend_from_slice(&[0; 8]);
        assert_eq!(provide("cldr-numbers.dat", &broken), Err("cldr-numbers.dat has a malformed CLDR record".to_string()));
        let mut other = broken[..10].to_vec();
        other.extend(miniz_oxide::deflate::compress_to_vec(b"numbers.digits\tlatn\tXYZ\n", 6));
        other.extend_from_slice(&[0; 8]);
        assert_eq!(fields("numbers.digits", "latn"), Some(vec!["0123456789"]));
        assert_eq!(provide("cldr-numbers.dat", &other), Ok(()));
        assert_eq!(fields("numbers.digits", "latn"), Some(vec!["0123456789"]));
        assert!(execute(Native::CldrCldrSize, &[text("numbers.nowhere".to_string()), text("x".to_string())]).is_err());
    }

    /// #943: one field and the size of a record agree with the whole record; a missing field is refused.
    #[test]
    fn a_single_field_and_the_size_agree_with_the_record() {
        let profile = fields("dates.profile", "ru").expect("ru")[0];
        let whole = fields("dates.fields", profile).expect("profile");
        let call = |native, extra: Option<i64>| {
            let mut args = vec![text("dates.fields".to_string()), text(profile.to_string())];
            args.extend(extra.map(int_value));
            execute(native, &args)
        };
        let Ok(size) = call(Native::CldrCldrSize, None) else { panic!("size") };
        assert_eq!(size.as_int().ok().and_then(|n| n.to_i64()), Some(whole.len() as i64));
        for at in [0, 1, whole.len() - 1] {
            let Ok(field) = call(Native::CldrCldrField, Some(at as i64)) else { panic!("field {at}") };
            assert_eq!(field.as_string().ok(), Some(whole[at]));
        }
        assert!(call(Native::CldrCldrField, Some(whole.len() as i64)).is_err());
        assert!(call(Native::CldrCldrField, Some(-1)).is_err());
        let missing = execute(Native::CldrCldrSize, &[text("dates.fields".to_string()), text("no-such".to_string())]);
        assert!(matches!(missing.ok().and_then(|v| v.as_int().ok().and_then(|n| n.to_i64())), Some(0)));
    }
}

/// #1125: without the tables in the binary a family is missing, with its own failure, until its file is given.
#[cfg(all(test, not(feature = "cldr")))]
mod given_tests {
    use super::*;

    fn gzip(text: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0];
        bytes.extend(miniz_oxide::deflate::compress_to_vec(text, 6));
        bytes.extend_from_slice(&[0; 8]);
        bytes
    }

    #[test]
    fn a_family_is_there_once_its_file_is_given() {
        let call = || execute(Native::CldrCldrSize, &[text("locales.parent".to_string()), text("en-GB".to_string())]);
        let Err(Error::Failure(failure)) = call() else { panic!("a missing family fails") };
        assert!(format!("{failure:?}").contains("The CLDR data cldr-locales.dat is not loaded"), "{failure:?}");
        assert_eq!(provide("cldr-locales.dat", &gzip(b"locales.parent\ten-GB\ten-001\n")), Ok(()));
        let Ok(size) = call() else { panic!("given") };
        assert_eq!(size.as_int().ok().and_then(|n| n.to_i64()), Some(1));
    }
}
