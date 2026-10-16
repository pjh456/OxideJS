//! 日历核心层：非 ISO 历法与 ISO 日期（proleptic Gregorian 纪元日）的双向换算、
//! 年长/闰年/月长/纪元/月码等字段查询。
//!
//! 本层只服务纯算术历法（无数据表）。hebrew 采用 keviyah（四门表）算法，
//! 与 ICU4X `calendrical_calculations::hebrew_keviyah` 同源；后续历法（偏移族、
//! 数据表族）按历法族逐步补入。
//!
//! 对象存储约定：PlainDate 等对象槽位恒存 ISO 年/月/日，日历 ID 决定 getter
//! 如何解释该 ISO 日期；from 路径把历法字段换算成 ISO 后入槽。

use super::common::{civil_from_days, days_from_civil};

/// hebrew 历法纪元（Tishrei 1 年 1）对应的 ISO 纪元日（1970-01-01 为 0）。
/// 由 RataDie -1373427（0001-01-01 为 1）换算：-1373427 - 719163。
const HEBREW_EPOCH_DAYS: i128 = -2_092_590;

/// Molad Beharad（纪元首朔）相对周首（周六 18:00）的 ḥalakim 偏移。
const MOLAD_BEHERAD_OFFSET: i64 = 31_524;
/// 一个朔望月的 ḥalakim 时长（29 日 12 时 793 分）。
const HEBREW_LUNATION_TIME: i64 = 765_433;
/// 一周的 ḥalakim 数（1080 × 24 × 7）。
const HALAKIM_IN_WEEK: i64 = 181_440;

/// d-h-p 记法转 ḥalakim（d 为 1 起周几，h 为 0-23 时，p 为 0-1079 分）。
const fn hal(d: i32, h: i32, p: i32) -> i64 {
    ((d - 1) * 24 + h) as i64 * 1080 + p as i64
}

/// 四门表：按 Metonic 周期年型分四组，每组 7 个 ḥalakim 分界，
/// 决定该年 molad 落在哪个 keviyah。
const FOUR_GATES_LMINUSONE: [i64; 7] = [
    hal(7, 18, 0),
    hal(1, 9, 204),
    hal(2, 18, 0),
    hal(3, 9, 204),
    hal(5, 9, 204),
    hal(5, 18, 0),
    hal(6, 9, 204),
];
const FOUR_GATES_LPLUSONE: [i64; 7] = [
    hal(7, 18, 0),
    hal(1, 9, 204),
    hal(2, 15, 589),
    hal(3, 9, 204),
    hal(5, 9, 204),
    hal(5, 18, 0),
    hal(6, 0, 408),
];
const FOUR_GATES_LPLUSMINUSONE: [i64; 7] = [
    hal(7, 18, 0),
    hal(1, 9, 204),
    hal(2, 15, 589),
    hal(3, 9, 204),
    hal(5, 9, 204),
    hal(5, 18, 0),
    hal(6, 9, 204),
];
const FOUR_GATES_LEAP: [i64; 7] = [
    hal(7, 18, 0),
    hal(1, 20, 491),
    hal(2, 18, 0),
    hal(3, 18, 0),
    hal(4, 11, 695),
    hal(5, 18, 0),
    hal(6, 20, 491),
];

/// 年型：定 Heshvan 与 Kislev 月长（缺 29/29、常 29/30、全 30/30）。
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
enum YearType {
    Deficient = -1,
    Regular = 0,
    Complete = 1,
}

/// 新年首日允许的周几（Rosh Hashanah 推迟规则后只剩四种）。
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
enum StartOfYear {
    Monday = 2,
    Tuesday = 3,
    Thursday = 5,
    Saturday = 7,
}

/// 一年的 keviyah（14 种）：新年周几 + 年型 + 逾越节周几。
/// 判别值按四门表位置排列，闰年 keviyah 排在平年之后（+7）。
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
enum Keviyah {
    // 平年
    BetHetGimel = 0,    // 2D3
    BetShinHe = 1,      // 2C5
    GimelKafHe = 2,     // 3R5
    HeKafZayin = 3,     // 5R7
    HeShinAlef = 4,     // 5C1
    ZayinHetAlef = 5,   // 7D1
    ZayinShinGimel = 6, // 7C3
    // 闰年
    BetHetHe = 7,       // 2D5
    BetShinZayin = 8,   // 2C7
    GimelKafZayin = 9,  // 3R7
    HeHetAlef = 10,     // 5D1
    HeShinGimel = 11,   // 5C3
    ZayinHetGimel = 12, // 7D3
    ZayinShinHe = 13,   // 7C5
}

impl Keviyah {
    /// 年型（keviyah 第二字母：ח 缺 / כ 常 / ש 全）。
    fn year_type(self) -> YearType {
        match self {
            Keviyah::BetHetGimel => YearType::Deficient,
            Keviyah::BetShinHe => YearType::Complete,
            Keviyah::GimelKafHe => YearType::Regular,
            Keviyah::HeKafZayin => YearType::Regular,
            Keviyah::HeShinAlef => YearType::Complete,
            Keviyah::ZayinHetAlef => YearType::Deficient,
            Keviyah::ZayinShinGimel => YearType::Complete,
            Keviyah::BetHetHe => YearType::Deficient,
            Keviyah::BetShinZayin => YearType::Complete,
            Keviyah::GimelKafZayin => YearType::Regular,
            Keviyah::HeHetAlef => YearType::Deficient,
            Keviyah::HeShinGimel => YearType::Complete,
            Keviyah::ZayinHetGimel => YearType::Deficient,
            Keviyah::ZayinShinHe => YearType::Complete,
        }
    }

    /// 新年首日周几（keviyah 首字母：ב 一 / ג 二 / ה 五 / ז 七）。
    fn start_of_year(self) -> StartOfYear {
        match self {
            Keviyah::BetHetGimel => StartOfYear::Monday,
            Keviyah::BetShinHe => StartOfYear::Monday,
            Keviyah::GimelKafHe => StartOfYear::Tuesday,
            Keviyah::HeKafZayin => StartOfYear::Thursday,
            Keviyah::HeShinAlef => StartOfYear::Thursday,
            Keviyah::ZayinHetAlef => StartOfYear::Saturday,
            Keviyah::ZayinShinGimel => StartOfYear::Saturday,
            Keviyah::BetHetHe => StartOfYear::Monday,
            Keviyah::BetShinZayin => StartOfYear::Monday,
            Keviyah::GimelKafZayin => StartOfYear::Tuesday,
            Keviyah::HeHetAlef => StartOfYear::Thursday,
            Keviyah::HeShinGimel => StartOfYear::Thursday,
            Keviyah::ZayinHetGimel => StartOfYear::Saturday,
            Keviyah::ZayinShinHe => StartOfYear::Saturday,
        }
    }

    /// 闰年 keviyah（判别值 ≥ 7）。
    fn is_leap(self) -> bool {
        self >= Keviyah::BetHetHe
    }

    /// 第 ordinal_month 个序月（1 起，Tishrei 为 1）的月长。
    fn month_len(self, ordinal_month: u8) -> u8 {
        let year_type = self.year_type();
        let is_leap = self.is_leap();
        // 闰年中 Adar 之后的序月对应上一个月号。
        let month_number = ordinal_month - (is_leap && ordinal_month >= 6) as u8;
        29 + (month_number % 2
                    // Heshvan 在全年型为长月
                    + (ordinal_month == 2 && year_type == YearType::Complete) as u8
                    // Kislev 在缺年型为短月
                    - (ordinal_month == 3 && year_type == YearType::Deficient) as u8)
    }

    /// 第 ordinal_month 个序月之前的累计天数。
    fn days_preceding(self, ordinal_month: u8) -> u16 {
        let year_type = self.year_type();
        let is_leap = self.is_leap();
        let month_number = ordinal_month - (is_leap && ordinal_month > 6) as u8;
        29 * (ordinal_month as u16 - 1)
            + (month_number / 2
                    // Adar I 为长月
                    + (is_leap && ordinal_month > 6) as u8
                    // Heshvan 在全年型为长月
                    + (ordinal_month > 2 && year_type == YearType::Complete) as u8
                    // Kislev 在缺年型为短月
                    - (ordinal_month > 3 && year_type == YearType::Deficient) as u8) as u16
    }

    /// 年内第 day_of_year 天（1 起）折算为（序月, 月内日）。
    fn month_day_for(self, day_of_year: u16) -> (u8, u8) {
        // 以 30 除（而非 29）以覆盖年初所有前月皆 30 天的情形。
        let mut month = ((day_of_year - 1) / 30) as u8 + 1;
        let mut days_before_month = self.days_preceding(month);
        let mut last_day_of_month = self.days_preceding(month + 1);
        while day_of_year > last_day_of_month {
            month += 1;
            days_before_month = last_day_of_month;
            last_day_of_month = self.days_preceding(month + 1);
        }
        (month, (day_of_year - days_before_month) as u8)
    }

    /// 年长（平年 353/354/355，闰年 383/384/385）。
    fn year_length(self) -> u16 {
        let base = if self.is_leap() { 384 } else { 354 };
        (base + self.year_type() as i16) as u16
    }
}

/// Metonic 周期年型（19 年 7 闰：3、6、8、11、14、17、19 闰）。
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
enum MetonicCycleType {
    LMinusOne,
    LPlusOne,
    LPlusMinusOne,
    Leap,
}

impl MetonicCycleType {
    fn for_h_year(h_year: i128) -> Self {
        let remainder = h_year.rem_euclid(19);
        match remainder {
            2 | 5 | 10 | 13 | 16 => Self::LMinusOne,
            1 | 4 | 9 | 12 | 15 => Self::LPlusOne,
            7 | 18 => Self::LPlusMinusOne,
            _ => Self::Leap,
        }
    }
}

/// 年 h_year 的 Tishrei 朔前累计月数（Metonic 周期：235 月 / 19 年）。
fn months_preceding_molad(h_year: i128) -> i128 {
    (235 * (h_year - 1) + 1).div_euclid(19)
}

/// 年 h_year 的 Tishrei 朔：(自 Beharad 周起的周数, 周内 ḥalakim)。
fn molad_details(h_year: i128) -> (i128, i64) {
    let months_preceding = months_preceding_molad(h_year);
    let molad = i128::from(MOLAD_BEHERAD_OFFSET) + months_preceding * i128::from(HEBREW_LUNATION_TIME);
    (
        molad.div_euclid(i128::from(HALAKIM_IN_WEEK)),
        molad.rem_euclid(i128::from(HALAKIM_IN_WEEK)) as i64,
    )
}

/// 四门表查询：按年型与周内 ḥalakim 定 keviyah。
fn keviyah_for(cycle_type: MetonicCycleType, in_week: i64) -> Keviyah {
    let gate = match cycle_type {
        MetonicCycleType::LMinusOne => &FOUR_GATES_LMINUSONE,
        MetonicCycleType::LPlusOne => &FOUR_GATES_LPLUSONE,
        MetonicCycleType::LPlusMinusOne => &FOUR_GATES_LPLUSMINUSONE,
        MetonicCycleType::Leap => &FOUR_GATES_LEAP,
    };
    // 首项跨周（周尾与周首同属一个 keviyah），其余按上界切分。
    let keviyot = if in_week >= gate[0] || in_week < gate[1] {
        (Keviyah::BetHetGimel, Keviyah::BetHetHe)
    } else if in_week < gate[2] {
        (Keviyah::BetShinHe, Keviyah::BetShinZayin)
    } else if in_week < gate[3] {
        (Keviyah::GimelKafHe, Keviyah::GimelKafZayin)
    } else if in_week < gate[4] {
        (Keviyah::HeKafZayin, Keviyah::HeHetAlef)
    } else if in_week < gate[5] {
        (Keviyah::HeShinAlef, Keviyah::HeShinGimel)
    } else if in_week < gate[6] {
        (Keviyah::ZayinHetAlef, Keviyah::ZayinHetGimel)
    } else {
        (Keviyah::ZayinShinGimel, Keviyah::ZayinShinHe)
    };
    if cycle_type == MetonicCycleType::Leap {
        keviyot.1
    } else {
        keviyot.0
    }
}

/// 一年的全部信息：keviyah + 自 Beharad 周起的周数。
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
struct YearInfo {
    keviyah: Keviyah,
    weeks_since_beharad: i128,
}

impl YearInfo {
    /// 计算年 h_year 的 YearInfo。
    fn compute_for(h_year: i128) -> Self {
        let (mut weeks_since_beharad, in_week) = molad_details(h_year);
        let cycle_type = MetonicCycleType::for_h_year(h_year);
        let keviyah = keviyah_for(cycle_type, in_week);
        // 周六（Hebrew Saturday）后六小时无条件推迟到周一，跨入新周。
        if in_week >= hal(7, 18, 0) {
            weeks_since_beharad += 1;
        }
        Self { keviyah, weeks_since_beharad }
    }

    /// 新年首日（Tishrei 1）的 ISO 纪元日。
    fn new_year(self) -> i128 {
        // Beharad 周从周一开始。
        const BEHARAD_START_OF_YEAR: i128 = StartOfYear::Monday as i128;
        let days_since_beharad =
            self.weeks_since_beharad * 7 + self.keviyah.start_of_year() as i128 - BEHARAD_START_OF_YEAR;
        HEBREW_EPOCH_DAYS + days_since_beharad
    }

    /// 含日 rd 的年份：先以平均年长（35975351/98496）近似，再校正。
    fn year_containing_rd(rd: i128) -> (Self, i128) {
        let mut h_year = (rd - HEBREW_EPOCH_DAYS) * 98496 / 35975351 + (rd >= HEBREW_EPOCH_DAYS) as i128;
        let mut year = Self::compute_for(h_year);
        if rd < year.new_year() {
            h_year -= 1;
            year = Self::compute_for(h_year);
        } else if rd >= year.new_year() + year.keviyah.year_length() as i128 {
            h_year += 1;
            year = Self::compute_for(h_year);
        }
        (year, h_year)
    }
}

// ───────────────────────── hebrew 对外接口 ─────────────────────────

/// hebrew 是否闰年（19 年周期 3、6、8、11、14、17、19 闰）。
pub(crate) fn hebrew_is_leap(h_year: i128) -> bool {
    YearInfo::compute_for(h_year).keviyah.is_leap()
}

/// hebrew 年长（353/354/355/383/384/385）。
pub(crate) fn hebrew_year_length(h_year: i128) -> i128 {
    YearInfo::compute_for(h_year).keviyah.year_length() as i128
}

/// hebrew 某序月的月长。
pub(crate) fn hebrew_days_in_month(h_year: i128, ordinal_month: u32) -> i128 {
    YearInfo::compute_for(h_year).keviyah.month_len(ordinal_month as u8) as i128
}

/// hebrew 年内的月数（平年 12、闰年 13）。
pub(crate) fn hebrew_months_in_year(h_year: i128) -> i128 {
    12 + YearInfo::compute_for(h_year).keviyah.is_leap() as i128
}

/// hebrew 纪元恒为 "am"（Anno Mundi），eraYear 即年号。
pub(crate) const HEBREW_ERA: &str = "am";

/// hebrew 日期（年, 序月, 日）→ ISO 纪元日。
pub(crate) fn hebrew_to_iso_days(h_year: i128, ordinal_month: u32, day: u32) -> i128 {
    let year_info = YearInfo::compute_for(h_year);
    year_info.new_year() + year_info.keviyah.days_preceding(ordinal_month as u8) as i128 + (day - 1) as i128
}

/// ISO 纪元日 → hebrew 日期（年, 序月, 日）。
pub(crate) fn iso_days_to_hebrew(iso_days: i128) -> (i128, u32, u32) {
    let (year_info, h_year) = YearInfo::year_containing_rd(iso_days);
    let day_of_year = (iso_days - year_info.new_year()) as u16 + 1;
    let (month, day) = year_info.keviyah.month_day_for(day_of_year);
    (h_year, month as u32, day as u32)
}

/// hebrew 日期（年, 序月, 日）→ ISO 日期分量。
pub(crate) fn hebrew_to_iso(h_year: i128, ordinal_month: u32, day: u32) -> (i32, u32, u32) {
    let (y, m, d) = civil_from_days(hebrew_to_iso_days(h_year, ordinal_month, day));
    (y as i32, m as u32, d as u32)
}

/// ISO 日期分量 → hebrew 日期（年, 序月, 日）。
pub(crate) fn iso_to_hebrew(iso_year: i32, iso_month: u32, iso_day: u32) -> (i128, u32, u32) {
    iso_days_to_hebrew(days_from_civil(i128::from(iso_year), i128::from(iso_month), i128::from(iso_day)))
}

/// 数值月（平年 1-12、闰年 1-13）→ 序月：hebrew 数值月即序月（闰年 Adar I 为 6、
/// Adar II 为 7、Elul 为 13），恒等映射。越界返回 None。
pub(crate) fn hebrew_ordinal_from_number(h_year: i128, number: u32) -> Option<u32> {
    let max = if hebrew_is_leap(h_year) { 13 } else { 12 };
    (1..=max).contains(&number).then_some(number)
}

/// 序月 → 数值月：hebrew 数值月即序月（平年 1-12、闰年 1-13），恒等映射。
pub(crate) fn hebrew_number_from_ordinal(ordinal_month: u32) -> Option<u32> {
    (1..=13).contains(&ordinal_month).then_some(ordinal_month)
}

/// 月码 → 序月：M05L 仅闰年合法（序月 6）；M06-M12 闰年 +1。
/// 非法（含 M13、非闰年的 M05L、M02L 等）返回 None。
pub(crate) fn hebrew_ordinal_from_month_code(h_year: i128, code: &str) -> Option<u32> {
    let is_leap = hebrew_is_leap(h_year);
    if code.ends_with('L') {
        return (code == "M05L" && is_leap).then_some(6);
    }
    let number: u32 = code[1..3].parse().ok()?;
    if !(1..=12).contains(&number) {
        return None;
    }
    let ordinal = number + (is_leap && number >= 6) as u32;
    Some(ordinal)
}

/// 序月 → 月码：闰年序月 6 为 M05L，序月 7 及以上 -1。
pub(crate) fn hebrew_month_code(h_year: i128, ordinal_month: u32) -> String {
    let is_leap = hebrew_is_leap(h_year);
    if is_leap && ordinal_month == 6 {
        return "M05L".to_string();
    }
    let number = ordinal_month - (is_leap && ordinal_month >= 6) as u32;
    format!("M{number:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    // 锚点：Tishrei 1 年 3760 应为周六（ICU4X 同款测试向量）。
    #[test]
    fn test_epoch_3760_saturday() {
        let (y, m, d) = hebrew_to_iso(3760, 1, 1);
        // 1 年 8 月 28 日（proleptic Gregorian）
        assert_eq!((y, m, d), (-1, 8, 28));
    }

    // 锚点：Rosh Hashanah 5785 = 2024-10-03（周四），5784 = 2023-09-16（周六），
    // 5786 = 2025-09-23（周二）。新年首日不可能是周日/周三/周五，可自查锚点。
    #[test]
    fn test_rosh_hashanah_anchors() {
        let (y, m, d) = hebrew_to_iso(5784, 1, 1);
        assert_eq!((y, m, d), (2023, 9, 16));
        let (y, m, d) = hebrew_to_iso(5785, 1, 1);
        assert_eq!((y, m, d), (2024, 10, 3));
        let (y, m, d) = hebrew_to_iso(5786, 1, 1);
        assert_eq!((y, m, d), (2025, 9, 23));
    }

    // 双向换算自洽（语料年份区间）。
    #[test]
    fn test_roundtrip() {
        for h_year in [0, 1, 5, 10, 3700, 5781, 5782, 5783, 5784, 5785, 5800] {
            let months = if hebrew_is_leap(h_year) { 13 } else { 12 };
            for month in 1..=months {
                let dim = hebrew_days_in_month(h_year, month) as u32;
                let (y, m, d) = hebrew_to_iso(h_year, month, dim);
                let (hy, hm, hd) = iso_to_hebrew(y, m, d);
                assert_eq!((hy, hm, hd), (h_year, month, dim), "year {h_year} month {month}");
            }
        }
    }

    // keviyah 年长取值合法（语料 validKeviahSymbols 的 6 种年长）。
    #[test]
    fn test_year_lengths_valid() {
        for year in 3700..=5800 {
            let len = hebrew_year_length(year);
            assert!(matches!(len, 353 | 354 | 355 | 383 | 384 | 385), "year {year} -> {len}");
        }
    }

    // 语料 basic-hebrew 5783（平年）逐月长度。
    #[test]
    fn test_month_lengths_5783() {
        let expected = [30, 30, 30, 29, 30, 29, 30, 29, 30, 29, 30, 29];
        for (index, &len) in expected.iter().enumerate() {
            let month = (index + 1) as u32;
            assert_eq!(hebrew_days_in_month(5783, month), len as i128, "month {month}");
        }
        assert_eq!(hebrew_months_in_year(5783), 12);
        assert!(!hebrew_is_leap(5783));
    }

    // 语料 leap-year-hebrew 5784（闰年）逐月长度。
    #[test]
    fn test_month_lengths_5784() {
        let expected = [30, 29, 29, 29, 30, 30, 29, 30, 29, 30, 29, 30, 29];
        for (index, &len) in expected.iter().enumerate() {
            let month = (index + 1) as u32;
            assert_eq!(hebrew_days_in_month(5784, month), len as i128, "month {month}");
        }
        assert_eq!(hebrew_months_in_year(5784), 13);
        assert!(hebrew_is_leap(5784));
    }

    // 语料 calc-epoch-year-hebrew：年 0..10 的 Cheshvan / Kislev 月长
    // （含 ICU4C ICU-23007 的纪元年推迟修正）。
    #[test]
    fn test_epoch_years_cheshvan_kislev() {
        let cheshvan = [29, 30, 30, 29, 29, 30, 30, 29, 29, 30, 29];
        let kislev = [30, 30, 30, 29, 30, 30, 30, 30, 29, 30, 30];
        for (year, (c, k)) in cheshvan.iter().zip(kislev.iter()).enumerate() {
            assert_eq!(hebrew_days_in_month(year as i128, 2), *c as i128, "year {year}");
            assert_eq!(hebrew_days_in_month(year as i128, 3), *k as i128, "year {year}");
        }
    }

    // 月码映射（闰年 5784 / 平年 5783）。
    #[test]
    fn test_month_code_mapping() {
        // 闰年
        assert_eq!(hebrew_month_code(5784, 6), "M05L");
        assert_eq!(hebrew_month_code(5784, 7), "M06");
        assert_eq!(hebrew_month_code(5784, 13), "M12");
        assert_eq!(hebrew_ordinal_from_month_code(5784, "M05L"), Some(6));
        assert_eq!(hebrew_ordinal_from_month_code(5784, "M06"), Some(7));
        assert_eq!(hebrew_ordinal_from_month_code(5784, "M12"), Some(13));
        assert_eq!(hebrew_ordinal_from_month_code(5784, "M13"), None);
        // 平年
        assert_eq!(hebrew_month_code(5783, 6), "M06");
        assert_eq!(hebrew_month_code(5783, 12), "M12");
        assert_eq!(hebrew_ordinal_from_month_code(5783, "M06"), Some(6));
        assert_eq!(hebrew_ordinal_from_month_code(5783, "M05L"), None);
        assert_eq!(hebrew_ordinal_from_month_code(5783, "M12"), Some(12));
    }

    // ICU4X 测试向量：ISO ↔ hebrew 双向换算。
    #[derive(Clone, Copy)]
    struct Vector4x {
        iso: (i32, u32, u32),
        heb: (i32, u32, u32),
    }

    #[test]
    fn test_icu4x_vectors() {
        let cases: [Vector4x; 6] = [
            // iso 为公历年月日，heb 为希伯来年与序数月与日
            Vector4x {
                iso: (2021, 1, 10),
                heb: (5781, 4, 26),
            },
            Vector4x {
                iso: (2021, 9, 10),
                heb: (5782, 1, 4),
            },
            Vector4x {
                iso: (2022, 2, 10),
                heb: (5782, 6, 9),
            }, // Adar I（闰年）
            Vector4x {
                iso: (2022, 3, 10),
                heb: (5782, 7, 7),
            }, // Adar II（闰年）
            Vector4x {
                iso: (2022, 10, 10),
                heb: (5783, 1, 15),
            },
            Vector4x {
                iso: (2022, 12, 25),
                heb: (5783, 4, 1),
            },
        ];
        for v in cases {
            let (iy, im, id) = v.iso;
            let (hy, hm, hd) = v.heb;
            let (y, m, d) = hebrew_to_iso(hy as i128, hm, hd);
            assert_eq!((y, m, d), (iy, im, id), "hebrew {hy}-{hm}-{hd}");
            let (ry, rm, rd) = iso_to_hebrew(iy, im, id);
            assert_eq!((ry, rm, rd), (hy as i128, hm, hd), "iso {iy}-{im}-{id}");
        }
    }
}
