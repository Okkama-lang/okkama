//! `runtime/cldr`: таблицы CLDR как данные рантайма (#223). Генераторы
//! `tools/generate-cldr*.py` пишут их в `cldr-<семейство>.dat` (gzip текста,
//! из `data/`); файл распаковывается и индексируется при первом обращении к
//! любой его таблице. Строка файла — таблица, ключ и поля через табуляцию;
//! имя таблицы начинается с семейства и точки (`numbers.row`). Sparrow берёт
//! этот native через мост (`native_origin`).
use super::Error;
use super::support::{failure, int, int_value, string, string_array, text};
use crate::data::gunzip;
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
    let Some(rows) = table
        .split_once('.')
        .and_then(|(family, _)| index(family))
        .and_then(|index| index.get(table))
    else {
        return Err(failure(
            "core.cldr",
            "invalid_input",
            native,
            format!("Unknown CLDR table {table:?}"),
        ));
    };
    Ok(rows.get(key).copied())
}

/// Индекс семейства; `None` — такого семейства нет.
fn index(family: &str) -> Option<&'static Index> {
    macro_rules! families {
        ($($name:literal => $file:literal),* $(,)?) => {
            match family {
                $($name => {
                    static TEXT: OnceLock<String> = OnceLock::new();
                    static INDEX: OnceLock<Index> = OnceLock::new();
                    Some(INDEX.get_or_init(|| {
                        build(TEXT.get_or_init(|| {
                            String::from_utf8(gunzip(include_bytes!(concat!(
                                "../../../okkama-runtime-core/data/",
                                $file
                            ))))
                            .expect("UTF-8 CLDR table")
                        }))
                    }))
                })*
                _ => None,
            }
        };
    }
    families!(
        "numbers" => "cldr-numbers.dat",
        "dates" => "cldr-dates.dat",
        "zones" => "cldr-zones.dat",
        "currencies" => "cldr-currencies.dat",
        "locales" => "cldr-locales.dat",
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(table: &str, key: &str) -> Option<Vec<&'static str>> {
        let (family, _) = table.split_once('.')?;
        let rows = index(family)?.get(table)?;
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
        assert!(index("nowhere").is_none());
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
