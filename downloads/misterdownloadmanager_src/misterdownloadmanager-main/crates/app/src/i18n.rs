//! The interface language (M16, D-019). craft-i18n reads MDM's catalogs (`i18n/<locale>.tsv`,
//! embedded at build time) and looks the English source text up: a missing translation is the
//! English itself. The language follows Windows' display language until the user picks one in
//! View → Language; numbers are grouped and pointed as the interface language writes them
//! (CLDR, through craft-i18n's format), or as the system region does when it is in that same
//! language (#128).
//!
//! `t("Options")` for a label, `tc("menu", "&Options")` where the same English needs different
//! translations, `tn(n, "{n} file", "{n} files")` for counts. Text that is data (file names,
//! addresses, paths) is never looked up; inside a translated sentence it is wrapped in `ltr()` so
//! it keeps its own direction in a right-to-left language.

use std::sync::{OnceLock, RwLock};

pub use craft_i18n::Lang;
use craft_i18n::{Digits, Embedded, Format, I18n, Region};

/// Every catalog in one brotli stream, and each locale's code and range in the unpacked text
/// (build.rs).
static PACKED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/catalogs.br"));
static RANGES: &[(&str, usize, usize)] = include!(concat!(env!("OUT_DIR"), "/catalogs.rs"));

/// The catalogs as craft-i18n reads them: unpacked once (about 6 MB, 11 ms) and kept for the
/// program's life, as craft-i18n wants them all from the start (#25).
fn catalogs() -> &'static Embedded {
    static C: OnceLock<&'static Embedded> = OnceLock::new();
    C.get_or_init(|| {
        let mut text = String::new();
        let unpacked = std::io::Read::read_to_string(
            &mut brotli_decompressor::Decompressor::new(PACKED, 64 * 1024),
            &mut text,
        );
        if let Err(e) = unpacked {
            // Built in at compile time, so this can't happen; English then, rather than a crash.
            eprintln!("translations: unpacking failed ({e})");
            return &[];
        }
        let text: &'static str = Box::leak(text.into_boxed_str());
        let embedded: Vec<(&'static str, &'static str, &'static str)> = RANGES
            .iter()
            .filter_map(|&(code, start, end)| Some((code, "", text.get(start..end)?)))
            .collect();
        Box::leak(embedded.into_boxed_slice())
    })
}

/// The catalogs, indexed once.
pub fn i18n() -> &'static I18n {
    static I: OnceLock<I18n> = OnceLock::new();
    I.get_or_init(|| {
        let catalogs = catalogs();
        let offered: Vec<&str> = catalogs.iter().map(|(code, _, _)| *code).collect();
        let (i, problems) = I18n::new(catalogs, &offered);
        for p in problems {
            eprintln!("translations: {p}");
        }
        i
    })
}

static CURRENT: RwLock<Option<Lang>> = RwLock::new(None);

/// The interface language: the one chosen, else Windows' display language (English when no
/// catalog matches it).
pub fn lang() -> Lang {
    if let Some(l) = *CURRENT.read().unwrap() {
        return l;
    }
    let l = i18n().system_lang();
    use_lang(l);
    l
}

/// Use `code` (a catalog's locale, e.g. `ar`, `pt-br`), or follow Windows again with `None`.
pub fn set_lang(code: Option<&str>) {
    let l = code
        .and_then(|c| i18n().from_code(c))
        .unwrap_or_else(|| i18n().system_lang());
    use_lang(l);
}

/// The interface language becomes `l`, and numbers (counts, sizes, speeds, percentages) are
/// written its way from now on.
fn use_lang(l: Lang) {
    *CURRENT.write().unwrap() = Some(l);
    i18n().set_format(number_format(i18n().code(l), &Region::system()));
}

/// How numbers are written in the interface language `code` (#128): as the system region
/// writes them when it is in that language (Swiss German's `1’234.5`, a separator the user set in
/// Windows), else as CLDR writes the language: German `1.234,5`, French `1 234,5` (narrow
/// no-break space), Hindi `12,34,567`, Arabic `1,234.5` (CLDR writes Arabic without a country in
/// Latin digits; an Egyptian region, `ar-EG`, gives `١٬٢٣٤٫٥`).
pub fn number_format(code: &str, system: &Region) -> Format {
    let language = |tag: &str| {
        tag.split(['-', '_'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
    };
    if language(&system.numbers) == language(code) {
        Format::new(system, Digits::Locale)
    } else {
        Format::from_tag(code)
    }
}

/// The interface language's code (`en`, `ar`…).
pub fn code() -> &'static str {
    i18n().code(lang())
}

/// Is the interface written right to left (Arabic, Persian, Hebrew, Urdu, Uyghur)? Then every
/// window is laid out from the right (winui's surfaces).
pub fn rtl() -> bool {
    i18n().rtl(lang())
}

/// Marks `source` for the catalog (tools/i18n/extract.py) where the lookup happens later, e.g.
/// a menu label chosen here and translated when the menu is drawn. Returns it unchanged.
pub const fn mark(source: &'static str) -> &'static str {
    source
}

/// The translation of `source` (English); `source` itself when there is none.
pub fn t(source: &str) -> &str {
    i18n().t(lang(), source)
}

/// The translation of `source` in `context` (the same English with different meanings).
pub fn tc<'a>(context: &str, source: &'a str) -> &'a str {
    i18n().tc(lang(), context, source)
}

/// A count: `one` / `other` templates with `{n}` (plural rules of the language).
pub fn tn(n: u64, one: &str, other: &str) -> String {
    i18n().trn(lang(), n, one, other)
}

/// Fill a translated template's `{name}` slots.
pub fn fmt(template: &str, args: &[(&str, &str)]) -> String {
    craft_i18n::fmt(template, args)
}

/// Data that reads left to right in every language (a path, an address, a size, a date, a
/// speed): a left-to-right isolate, inside a translated sentence or alone.
pub fn ltr(value: &str) -> String {
    craft_i18n::ltr(value)
}

/// Data with a direction of its own (a file name, a description) shown alone where interface
/// text goes: a first-strong isolate, so an Arabic name reads right to left in an English window
/// and a Latin one left to right in an Arabic window (interface text is laid out in the
/// interface's direction).
pub fn isolate(value: &str) -> String {
    craft_i18n::isolate(value)
}

/// A whole number as the interface language writes it (English: plain digits, as IDM):
/// `10200`, `10.200`, `10 200`.
pub fn integer(n: u64) -> String {
    if code() == "en" {
        n.to_string()
    } else {
        i18n().format().integer(n)
    }
}

/// `v` with `decimals` digits after the decimal mark, as the interface language writes it
/// (English: `1234.50`, as IDM; German `1.234,50`).
pub fn decimal(v: f64, decimals: usize) -> String {
    if code() == "en" {
        format!("{v:.decimals$}")
    } else {
        i18n().format().number(v, decimals)
    }
}

/// `ratio` as a percentage with `decimals` digits, as the interface language writes it (English:
/// `13.38%`, as IDM; German `13,38 %`, Turkish `%13,38`).
pub fn percent(ratio: f64, decimals: usize) -> String {
    if code() == "en" {
        format!("{:.decimals$}%", 100.0 * ratio)
    } else {
        i18n().format().percent(ratio, decimals)
    }
}

/// Every language offered, English first, each by its own name, for View → Language.
pub fn languages() -> Vec<(Lang, &'static str, &'static str)> {
    let i = i18n();
    i.languages()
        .into_iter()
        .map(|l| {
            (
                l,
                i.code(l),
                info(i.code(l)).map_or(i.name(l), |x| x.native),
            )
        })
        .collect()
}

/// A row of `i18n/languages.tsv`: the code, the English name, the language's own name and its
/// flag (lipis/flag-icons, SVG).
pub struct Info {
    pub code: &'static str,
    pub english: &'static str,
    pub native: &'static str,
    pub flag: &'static [u8],
}

type Row = (&'static str, &'static str, &'static str, &'static [u8]);
static TABLE: &[Row] = include!(concat!(env!("OUT_DIR"), "/languages.rs"));

/// The table's row for `code` (a catalog's locale, any case).
pub fn info(code: &str) -> Option<Info> {
    TABLE
        .iter()
        .find(|r| r.0.eq_ignore_ascii_case(code))
        .map(|&(code, english, native, flag)| Info {
            code,
            english,
            native,
            flag,
        })
}

/// The code of Windows' display language as MDM offers it (English when none fits).
pub fn system_code() -> &'static str {
    i18n().code(i18n().system_lang())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LRI: char = '\u{2066}';
    const FSI: char = '\u{2068}';
    const PDI: char = '\u{2069}';

    /// Every embedded catalog parses with no problem (craft-i18n's own checks: placeholders,
    /// plural forms, the trailing ellipsis…), and every language of i18n/languages.tsv has one.
    #[test]
    fn every_catalog_is_clean_and_offered() {
        let problems = i18n().check_all();
        assert!(problems.is_empty(), "{}", problems.join("\n"));
        for row in TABLE.iter().filter(|r| r.0 != "en") {
            assert!(
                i18n().from_code(row.0).is_some(),
                "no catalog for {}",
                row.0
            );
        }
    }

    #[test]
    fn data_goes_in_as_isolates() {
        // A path or an address reads left to right; a file name takes its own direction.
        assert_eq!(ltr(r"C:\dl\a.zip"), format!(r"{LRI}C:\dl\a.zip{PDI}"));
        assert_eq!(isolate("ملف.zip"), format!("{FSI}ملف.zip{PDI}"));
        // In a right-to-left sentence the values stay isolated (one isolate is kept, not
        // doubled), so a Latin name stays where the translator put it.
        let s = fmt("حذف \"{name}\"؟", &[("name", &isolate("report.zip"))]);
        assert_eq!(s, format!("حذف \"{FSI}report.zip{PDI}\"؟"));
        // English text with English values gets no marks; an Arabic name in an English
        // sentence is isolated, so it can't reorder the sentence.
        assert_eq!(
            fmt("Delete \"{name}\"?", &[("name", "a.zip")]),
            "Delete \"a.zip\"?"
        );
        assert_eq!(
            fmt("Delete \"{name}\"?", &[("name", "ملف.zip")]),
            format!("Delete \"{FSI}ملف.zip{PDI}\"?")
        );
    }

    #[test]
    fn right_to_left_languages() {
        let i = i18n();
        for code in ["ar", "fa", "he", "ur", "ug"] {
            if let Some(l) = i.from_code(code) {
                assert!(i.rtl(l), "{code}");
            }
        }
        assert!(!i.rtl(Lang::EN));
    }

    const NNBSP: char = '\u{202f}';

    /// #128: numbers are grouped and pointed as the interface language writes them, whatever the
    /// system's region (here the United States'), and as the region does when it is in the same
    /// language.
    #[test]
    fn numbers_follow_the_interface_language() {
        let us = Region::from_tag("en-US");
        for (code, n, count, size) in [
            ("en", 10_200, "10,200".to_owned(), "1,234.50".to_owned()),
            ("de", 10_200, "10.200".to_owned(), "1.234,50".to_owned()),
            (
                "fr",
                10_200,
                format!("10{NNBSP}200"),
                format!("1{NNBSP}234,50"),
            ),
            // CLDR writes Arabic without a country in Latin digits, grouped the English way.
            ("ar", 10_200, "10,200".to_owned(), "1,234.50".to_owned()),
            // Hindi groups by lakh and crore.
            (
                "hi",
                10_010_200,
                "1,00,10,200".to_owned(),
                "1,234.50".to_owned(),
            ),
        ] {
            let f = number_format(code, &us);
            assert_eq!(f.integer(n), count, "{code}");
            assert_eq!(f.number(1234.5, 2), size, "{code}");
        }
        // A region in the interface's language keeps its own way, and a separator the user set.
        let swiss = Region::from_tag("de-CH");
        assert_eq!(
            number_format("de", &swiss).integer(10_200),
            Format::from_tag("de-CH").integer(10_200)
        );
        assert_ne!(number_format("de", &swiss).integer(10_200), "10.200");
        let egypt = Region::from_tag("ar-EG");
        assert_eq!(number_format("ar", &egypt).integer(10_200), "١٠٬٢٠٠");
        assert_eq!(number_format("de", &egypt).integer(10_200), "10.200");
        let mut own = Region::from_tag("de-DE");
        own.grouping = Some(Some('\''));
        assert_eq!(number_format("de", &own).integer(10_200), "10'200");
        // Catalog codes with a country or a script are read too.
        assert_eq!(number_format("pt-br", &us).integer(10_200), "10.200");
        assert_eq!(number_format("zh-hans", &us).integer(10_200), "10,200");
    }

    /// Counts, sizes and speeds take their numbers from that format, beside the catalogs' units
    /// (the way the list, the progress window and the playlist dialog write them).
    #[test]
    fn counts_sizes_and_speeds_in_the_interface_language() {
        let us = Region::from_tag("en-US");
        let codes: Vec<&str> = catalogs().iter().map(|c| c.0).collect();
        let (i, _) = I18n::new(catalogs(), &codes);
        let fr_count = format!("10{NNBSP}200");
        for (code, count, size, speed) in [
            ("de", "10.200", "1,50", "512,000"),
            ("fr", fr_count.as_str(), "1,50", "512,000"),
            ("ar", "10,200", "1.50", "512.000"),
            ("hi", "10,200", "1.50", "512.000"),
        ] {
            let l = i.from_code(code).expect(code);
            i.set_format(number_format(code, &us));
            let videos = craft_i18n::strip_isolates(&i.trn(l, 10_200, "{n} video", "{n} videos"));
            assert!(videos.contains(count), "{code}: {videos}");
            let mb = craft_i18n::strip_isolates(&i.measure(l, 1.5, 2, "{n} MB"));
            assert!(mb.contains(size), "{code}: {mb}");
            let rate = craft_i18n::strip_isolates(&i.measure(l, 512.0, 3, "{n} KB/sec"));
            assert!(rate.contains(speed), "{code}: {rate}");
        }
    }
}
