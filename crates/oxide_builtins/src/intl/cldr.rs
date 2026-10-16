//! CLDR 36.1 语言标签规范化与 UTS35 结构校验（纯函数，不依赖 VmHost）。
//!
//! 映射表与 test262 harness 的 CLDR 36.1 表逐位一致（见 `tables` 模块）；
//! 结构校验遵循 UTS35（`is_structurally_valid`），不接受 BCP47 宽松形式。
//! 规范化结果等于输入当且仅当输入是规范化的合法标签（`canonicalize_locale`
//! 的不动点即合法性判据）。

use std::sync::OnceLock;

use regex::Regex;

use super::tables::*;

/// UTS35 languageTag 正则（大小写不敏感）。
///
/// 与 harness 的 `languageTagRE` 逐字一致：language 后接可选 script / region /
/// variants，再重复可选 extensions（unicode / transformed / other），最后可选
/// privateuse。
const LANGUAGE_TAG_PATTERN: &str = r"^(?i:(([a-z]{2,3}|[a-z]{5,8})(-([a-z]{4}))?(-([a-z]{2}|[0-9]{3}))?(-([a-z0-9]{5,8}|(?:[0-9][a-z0-9]{3})))*(-((u((-([a-z0-9][a-z](-[a-z0-9]{3,8})*))+|((-([a-z0-9]{3,8}))+(-([a-z0-9][a-z](-[a-z0-9]{3,8})*))*)))|(t((-(([a-z]{2,3}|[a-z]{5,8})(-([a-z]{4}))?(-([a-z]{2}|[0-9]{3}))?(-([a-z0-9]{5,8}|(?:[0-9][a-z0-9]{3})))*)(-([a-z][0-9](-[a-z0-9]{3,8})+))*)|(-([a-z][0-9](-[a-z0-9]{3,8})+))+))|(([0-9]|[a-sv-wy-z])(-[a-z0-9]{2,8})+)))*(-(x(-[a-z0-9]{1,8})+))?))$";

fn language_tag_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(LANGUAGE_TAG_PATTERN).expect("语言标签正则须可编译"))
}

/// 判断子标签是否为单例（singleton）：1 字符，数字或 `[a-wy-z]`（大小写不敏感）。
fn is_singleton(subtag: &str) -> bool {
    let bytes = subtag.as_bytes();
    bytes.len() == 1 && matches!(bytes[0], b'0'..=b'9' | b'a'..=b'w' | b'y'..=b'z')
}

/// 判断子标签是否为 2-8 位字母数字，供重复变体检测的中间段判定。
fn is_2_8_alnum(subtag: &str) -> bool {
    (2..=8).contains(&subtag.len()) && subtag.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// 判断子标签是否为变体（variant）：5-8 位字母数字，或数字开头共 4 位。
fn is_variant(subtag: &str) -> bool {
    let bytes = subtag.as_bytes();
    let is_alnum = |b: u8| b.is_ascii_alphanumeric();
    (bytes.len() == 4 && bytes[0].is_ascii_digit() && bytes.iter().all(|&b| is_alnum(b)))
        || (bytes.len() >= 5 && bytes.len() <= 8 && bytes.iter().all(|&b| is_alnum(b)))
}

/// 判断子标签是否为 transformed extension 的 key（`^[a-z][0-9]$`，大小写不敏感）。
fn is_transform_key(subtag: &str) -> bool {
    let bytes = subtag.as_bytes();
    bytes.len() == 2 && bytes[0].is_ascii_lowercase() && bytes[1].is_ascii_digit()
}

/// 判断是否存在重复单例子标签（大小写不敏感比较，与 harness 的 `i` 标志一致）。
///
/// 条件：存在 i < j，两个子标签都是单例且取值相同。
fn has_duplicate_singleton(subtags: &[&str]) -> bool {
    let n = subtags.len();
    for i in 0..n {
        if !is_singleton(subtags[i]) {
            continue;
        }
        for j in (i + 1)..n {
            if subtags[i].eq_ignore_ascii_case(subtags[j]) && is_singleton(subtags[j]) {
                return true;
            }
        }
    }
    false
}

/// 判断是否存在重复变体子标签（大小写不敏感比较）。
///
/// 条件：存在 i < j，`subtags[j]` 是变体且与 `subtags[i]` 相同，`subtags[i-1]`
/// 存在且为 2-8 位字母数字，i 与 j 之间的所有子标签均为 2-8 位字母数字。
fn has_duplicate_variant(subtags: &[&str]) -> bool {
    let n = subtags.len();
    for j in 0..n {
        if !is_variant(subtags[j]) {
            continue;
        }
        for i in 0..j {
            if !subtags[i].eq_ignore_ascii_case(subtags[j]) {
                continue;
            }
            if i == 0 || !is_2_8_alnum(subtags[i - 1]) {
                continue;
            }
            if (i + 1..j).all(|k| is_2_8_alnum(subtags[k])) {
                return true;
            }
        }
    }
    false
}

/// UTS35 结构校验：languageTag 正则通过，且截断私有域后无重复单例与重复变体。
pub fn is_structurally_valid(tag: &str) -> bool {
    if !language_tag_re().is_match(tag) {
        return false;
    }
    let truncated = tag.split("-x-").next().unwrap_or("");
    let subtags: Vec<&str> = truncated.split('-').collect();
    !has_duplicate_singleton(&subtags) && !has_duplicate_variant(&subtags)
}

/// CLDR 36.1 语言标签规范化（harness `canonicalizeLanguageTag` 的逐位移植）。
///
/// 步骤：整标映射 → 标准段（language / script / region）解析与映射 → variants
/// 映射与排序 → extensions 规范化（unicode 键排序、transformed 递归规范化）→
/// privateuse 透传 → 重组。
pub fn canonicalize_locale(locale: &str) -> String {
    let locale = locale.to_ascii_lowercase();

    // 整标映射（grandfathered 标签）。
    if let Some((_, mapped)) = TAG_MAPPINGS.iter().find(|(from, _)| *from == locale) {
        return mapped.to_string();
    }

    let subtags: Vec<&str> = locale.split('-').collect();
    let mut i = 0;

    // 标准段：第一个 variant 或 singleton 之前的全部子标签。
    let mut language = String::new();
    let mut script: Option<String> = None;
    let mut region: Option<String> = None;
    while i < subtags.len() {
        let subtag = subtags[i];
        if i == 0 {
            language = subtag.to_string();
        } else if subtag.len() == 2 || subtag.len() == 3 {
            region = Some(subtag.to_ascii_uppercase());
        } else if subtag.len() == 4 && !subtag.starts_with(|c: char| c.is_ascii_digit()) {
            let mut chars = subtag.chars();
            let first = chars.next().map(|c| c.to_ascii_uppercase().to_string()).unwrap_or_default();
            let rest = chars.collect::<String>().to_ascii_lowercase();
            script = Some(format!("{first}{rest}"));
        } else {
            break;
        }
        i += 1;
    }

    // language 映射：简单表优先，未命中查复合表（复合表可补 script / region）。
    if let Some((_, mapped)) = LANGUAGE_MAPPINGS.iter().find(|(from, _)| *from == language) {
        language = mapped.to_string();
    } else if let Some((_, mapped_lang, mapped_script, mapped_region)) =
        COMPLEX_LANGUAGE_MAPPINGS.iter().find(|(from, _, _, _)| *from == language)
    {
        language = mapped_lang.to_string();
        if script.is_none() {
            script = mapped_script.map(|s| s.to_string());
        }
        if region.is_none() {
            region = mapped_region.map(|s| s.to_string());
        }
    }

    // region 映射：简单表优先，未命中查复合表（按 language[-script] 键选择）。
    if let Some(region_raw) = region {
        let mapped = if let Some((_, mapped)) = REGION_MAPPINGS.iter().find(|(from, _)| *from == region_raw) {
            mapped.to_string()
        } else if let Some((_, default, entries)) =
            COMPLEX_REGION_MAPPINGS.iter().find(|(from, _, _)| *from == region_raw)
        {
            let mut key = language.clone();
            if let Some(script) = &script {
                key.push('-');
                key.push_str(script);
            }
            entries
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
                .unwrap_or_else(|| default.to_string())
        } else {
            region_raw
        };
        region = Some(mapped);
    }

    // variants：映射后按字典序排序。
    let mut variants: Vec<String> = Vec::new();
    while i < subtags.len() && subtags[i].len() > 1 {
        let variant = subtags[i];
        match VARIANT_MAPPINGS.iter().find(|(from, _, _)| *from == variant) {
            Some((_, "language", replacement)) => language = replacement.to_string(),
            Some((_, "region", replacement)) => region = Some(replacement.to_string()),
            Some((_, "variant", replacement)) => variants.push(replacement.to_string()),
            Some((_, _, _)) => unreachable!("变体映射类型非法"),
            None => variants.push(variant.to_string()),
        }
        i += 1;
    }
    variants.sort();

    // extensions：unicode 键按 ASCII 排序、transformed 递归规范化、其余透传。
    let mut extensions: Vec<String> = Vec::new();
    while i < subtags.len() && subtags[i] != "x" {
        let extension_start = i;
        i += 1;
        while i < subtags.len() && subtags[i].len() > 1 {
            i += 1;
        }

        let extension_key = subtags[extension_start];
        if extension_key == "u" {
            let mut j = extension_start + 1;
            while j < i && subtags[j].len() > 2 {
                j += 1;
            }
            let mut extension = subtags[extension_start..j].join("-");
            while j < i {
                let key_start = j;
                j += 1;
                while j < i && subtags[j].len() > 2 {
                    j += 1;
                }
                let key = subtags[key_start];
                let value = subtags[key_start + 1..j].join("-");
                let value = if let Some((_, entries)) = UNICODE_MAPPINGS.iter().find(|(k, _)| *k == key) {
                    entries
                        .iter()
                        .find(|(k, _)| *k == value)
                        .map(|(_, v)| v.to_string())
                        .unwrap_or(value)
                } else {
                    value
                };
                extension.push('-');
                extension.push_str(key);
                if !value.is_empty() && value != "true" {
                    extension.push('-');
                    extension.push_str(&value);
                }
            }
            extensions.push(extension);
        } else if extension_key == "t" {
            let mut j = extension_start + 1;
            while j < i && !is_transform_key(subtags[j]) {
                j += 1;
            }
            let mut extension = String::from("t");
            let transform_language = subtags[extension_start + 1..j].join("-");
            if !transform_language.is_empty() {
                extension.push('-');
                extension.push_str(&canonicalize_locale(&transform_language).to_ascii_lowercase());
            }
            // tfield 按字母序排列（UTS35 §3.3.1 规范化要求；true 值不删除）。
            let mut fields: Vec<(String, String)> = Vec::new();
            while j < i {
                let key_start = j;
                j += 1;
                while j < i && subtags[j].len() > 2 {
                    j += 1;
                }
                let key = subtags[key_start].to_string();
                let value = subtags[key_start + 1..j].join("-");
                let value = if let Some((_, entries)) = TRANSFORM_MAPPINGS.iter().find(|(k, _)| *k == key) {
                    entries
                        .iter()
                        .find(|(k, _)| *k == value)
                        .map(|(_, v)| v.to_string())
                        .unwrap_or(value)
                } else {
                    value
                };
                fields.push((key, value));
            }
            fields.sort();
            for (key, value) in fields {
                extension.push('-');
                extension.push_str(&key);
                extension.push('-');
                extension.push_str(&value);
            }
            extensions.push(extension);
        } else {
            extensions.push(subtags[extension_start..i].join("-"));
        }
    }
    extensions.sort();

    // privateuse 透传。
    let private_use = if i < subtags.len() { Some(subtags[i..].join("-")) } else { None };

    // 重组。
    let mut canonical = language;
    if let Some(script) = script {
        canonical.push('-');
        canonical.push_str(&script);
    }
    if let Some(region) = region {
        canonical.push('-');
        canonical.push_str(&region);
    }
    if !variants.is_empty() {
        canonical.push('-');
        canonical.push_str(&variants.join("-"));
    }
    if !extensions.is_empty() {
        canonical.push('-');
        canonical.push_str(&extensions.join("-"));
    }
    if let Some(private_use) = private_use {
        if !canonical.is_empty() {
            canonical.push('-');
            canonical.push_str(&private_use);
        } else {
            canonical = private_use;
        }
    }
    canonical
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalize_vectors() {
        // 与 node + harness canonicalizeLanguageTag 的实测向量对齐。
        assert_eq!(canonicalize_locale("cmn-hans-cn"), "zh-Hans-CN");
        assert_eq!(canonicalize_locale("sh"), "sr-Latn");
        assert_eq!(canonicalize_locale("no"), "nb");
        assert_eq!(canonicalize_locale("cnr"), "sr-ME");
        // 标准段循环中 2-3 位子标签逐个覆盖 region：`oed` 非变体（3 位），
        // 按 region 消费并覆盖 `GB`，与 harness 逐位一致。
        assert_eq!(canonicalize_locale("en-GB-oed"), "en-OED");
        assert_eq!(canonicalize_locale("heploc"), "heploc");
        assert_eq!(canonicalize_locale("aa-aaland"), "aa-AX");
        assert_eq!(canonicalize_locale("es-419"), "es-419");
        assert_eq!(canonicalize_locale("172"), "172");
        assert_eq!(canonicalize_locale("en-t-en-GB"), "en-t-en-gb");
        assert_eq!(canonicalize_locale("ab-cd"), "ab-CD");
        assert_eq!(canonicalize_locale("de-rt"), "de-RT");
        assert_eq!(canonicalize_locale("en-x-u-foo"), "en-x-u-foo");
        assert_eq!(canonicalize_locale("en-a-bar-x-u-foo"), "en-a-bar-x-u-foo");
        assert_eq!(canonicalize_locale("en-x-u-foo-a-bar"), "en-x-u-foo-a-bar");
        assert_eq!(canonicalize_locale("en-a-bar-u-baz-x-u-foo"), "en-a-bar-u-baz-x-u-foo");
        // transformed 扩展：tfield 按字母序排列、tvalue 映射、true 不删除。
        assert_eq!(canonicalize_locale("sl-t-sl-rozaj-biske-1994"), "sl-t-sl-1994-biske-rozaj");
        assert_eq!(canonicalize_locale("DE-T-M0-DIN-K0-QWERTZ"), "de-t-k0-qwertz-m0-din");
        assert_eq!(canonicalize_locale("en-t-m0-true"), "en-t-m0-true");
        assert_eq!(canonicalize_locale("en-t-iw"), "en-t-he");
        assert_eq!(canonicalize_locale("und-Latn-t-und-hani-m0-names"), "und-Latn-t-und-hani-m0-prprname");
        assert_eq!(canonicalize_locale("mo"), "ro");
        assert_eq!(canonicalize_locale("ces"), "cs");
        assert_eq!(canonicalize_locale("hy-arevela"), "hy");
        assert_eq!(canonicalize_locale("hy-arevmda"), "hyw");
        assert_eq!(canonicalize_locale("aar-x-private"), "aa-x-private");
        assert_eq!(canonicalize_locale("heb-x-private"), "he-x-private");
        // unicode 扩展键保持原序（harness 不排序键；test262 允许原序或排序）。
        assert_eq!(canonicalize_locale("it-u-nu-latn-ca-gregory"), "it-u-nu-latn-ca-gregory");
        assert_eq!(canonicalize_locale("th-th-u-nu-thai"), "th-TH-u-nu-thai");
        assert_eq!(canonicalize_locale("art-lojban"), "jbo");
        assert_eq!(canonicalize_locale("zh-guoyu"), "zh");
        assert_eq!(canonicalize_locale("zh-hakka"), "hak");
        assert_eq!(canonicalize_locale("zh-xiang"), "hsn");
        assert_eq!(canonicalize_locale("es-ES-preeuro"), "es-ES-preeuro");
        // `cyrillic` 是脚本名而非变体，映射表无此项，原样透传（8 位子标签
        // 按变体消费）。
        assert_eq!(canonicalize_locale("uz-UZ-cyrillic"), "uz-UZ-cyrillic");
        // harness 变体表无 `posix` 项（UTS35 变体映射不覆盖它），原样透传。
        assert_eq!(canonicalize_locale("posix"), "posix");
        // harness 变体表仅 5 项（aaland/arevela/arevmda/heploc/polytoni），
        // `direct` 无映射，原样透传。
        assert_eq!(canonicalize_locale("hi-direct"), "hi-direct");
        assert_eq!(canonicalize_locale("zh-pinyin"), "zh-pinyin");
        assert_eq!(canonicalize_locale("zh-stroke"), "zh-stroke");
        assert_eq!(canonicalize_locale("de-u-kf"), "de-u-kf");
    }

    #[test]
    fn structural_validity() {
        // 结构合法：languageTag 正则通过且无重复单例/重复变体。
        assert!(is_structurally_valid("en"));
        assert!(is_structurally_valid("en-US"));
        assert!(is_structurally_valid("zh-Hans-CN"));
        assert!(is_structurally_valid("en-x-u-foo"));
        assert!(is_structurally_valid("en-t-en-gb"));
        assert!(is_structurally_valid("th-TH-u-nu-thai"));
        assert!(is_structurally_valid("aa-AX"));
        assert!(is_structurally_valid("en-POSIX"));
        assert!(is_structurally_valid("en-US-latin"));
        assert!(is_structurally_valid("en-GB-1234"));

        // 结构合法但非规范化（规范化判据在 canonicalize_locale 不动点，
        // 本函数不查）：与 harness 的 isCanonicalizedStructurallyValidLanguageTag
        // 不同，后者额外要求输入等于规范化结果。
        assert!(is_structurally_valid("cmn-hans-cn"));
        assert!(is_structurally_valid("no"));
        assert!(is_structurally_valid("sh"));

        // `u`/`t` 是两个不同单例：无重复单例，结构合法（harness 侧判非法
        // 源于扩展排序的规范化判据，不属本函数）。
        assert!(is_structurally_valid("en-u-ca-gregory-t-en"));

        // 结构非法：正则不匹配（OED/oed 3 位非变体、172 非语言、单字符、空串、
        // 区域在脚本前、重复子标签）或重复单例/重复变体。
        assert!(!is_structurally_valid("en-GB-OED"));
        assert!(!is_structurally_valid("en-GB-oed"));
        assert!(!is_structurally_valid("172"));
        assert!(!is_structurally_valid("it-IT-Latn"));
        assert!(!is_structurally_valid("en-u-ca-gregory-u-kf"));
        assert!(!is_structurally_valid("en-172-172"));
        assert!(!is_structurally_valid("a"));
        assert!(!is_structurally_valid("en-GB-xx"));
        assert!(!is_structurally_valid(""));
    }
}
