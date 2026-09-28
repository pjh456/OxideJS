//! 时区数据源与 TZif 解析器：IANA tzdata 2026b 全量树 vendored 于 `third_party/tzdb/`，
//! 引擎内置 legacy 别名表，不读宿主系统 zoneinfo。
//!
//! 关键约定：
//! - 数据版本钉住 tzdata 2026b，全量树随仓库，保证各环境逐字节一致；legacy 区名
//!   （2026b 已移除）由引擎内 `LEGACY_ALIASES` 表解析，独立于数据版本。
//! - 偏移一律以秒存（i32），保亚分钟精度（LMT、Lord Howe 半小时 DST 等）；
//!   `transitions` 按纪元秒严格升序，最后一次 transition 之后的偏移预存于 `last_offset`。
//! - 闰秒不建模：闰秒表只用于推进解析指针，Temporal 时间线为连续纳秒轴。
//! - 刷新程序：从宿主 /usr/share/zoneinfo 以 `cp -rL` 复制 11 个区域目录加顶层区文件
//!   （排除机器相关的 localtime 与源文件 tzdata.zi / leap-seconds.list），并更新本
//!   注释的版本行。

use std::sync::LazyLock;

use super::common::fixed_offset_seconds;

/// 单个时区的解析结果：transition 表加偏移查找所需的预存值。
///
/// `transitions` 为 `(纪元秒, 该时刻起生效的偏移秒)`，按纪元秒严格升序；
/// 早于首个 transition 的偏移取 `lmt_offset`（首个 time type，即 LMT），
/// 晚于末个 transition 的偏移取 `last_offset`（POSIX TZ 串语义，与末次 transition 一致）。
pub(crate) struct TzifZone {
    lmt_offset: i32,
    transitions: Vec<(i64, i32)>,
    last_offset: i32,
}

/// TZif 解析失败原因。
#[derive(Debug)]
pub(crate) enum TzifError {
    /// 文件在声明的块边界前截断。
    Truncated,
    /// 魔数不是 `TZif`。
    BadMagic,
    /// 版本字节不是 0 / 2 / 3 / 4。
    BadVersion,
}

/// TZif 头的 6 个计数字段（isutcnt / isstdcnt / leapcnt / timecnt / typecnt / charcnt）。
#[derive(Clone, Copy)]
struct Counts {
    isutcnt: u32,
    isstdcnt: u32,
    leapcnt: u32,
    timecnt: u32,
    typecnt: u32,
    charcnt: u32,
}

/// 单个数据块的解析结果：transition 时间、transition 类型索引、本地时间类型偏移。
struct ParsedBlock {
    trans: Vec<i64>,
    ttypes: Vec<u8>,
    types: Vec<i32>,
}

/// 读 44 字节头的 6 个计数字段。
///
/// # 边界与前提
/// - 调用方须保证 `off + 44` 不越界（本函数自行校验）。
///
/// # 注意事项
/// - 计数字段从偏移 20 起（magic 4 + version 1 + 保留 15）。
fn read_counts(bytes: &[u8], off: usize) -> Result<Counts, TzifError> {
    if bytes.len() < off + 44 {
        return Err(TzifError::Truncated);
    }
    let d = |i: usize| u32::from_be_bytes(bytes[off + 20 + i * 4..off + 24 + i * 4].try_into().unwrap());
    Ok(Counts {
        isutcnt: d(0),
        isstdcnt: d(1),
        leapcnt: d(2),
        timecnt: d(3),
        typecnt: d(4),
        charcnt: d(5),
    })
}

/// 数据块总字节数（transition 时间 + 类型 + 本地时间类型记录 + 区名 + 闰秒 + 指示位）。
fn block_size(c: &Counts, time_size: usize) -> usize {
    c.timecnt as usize * time_size
        + c.timecnt as usize
        + c.typecnt as usize * 6
        + c.charcnt as usize
        + c.leapcnt as usize * (time_size + 4)
        + c.isstdcnt as usize
        + c.isutcnt as usize
}

/// 读一个数据块：transition 时间、transition 类型索引、本地时间类型偏移。
///
/// # 边界与前提
/// - 调用方须保证块完整（本函数按 `block_size` 校验总长）。
/// - `time_size` 为 4（v1）或 8（v2+）。
///
/// # 注意事项
/// - transition 类型索引越界（超出 typecnt）视为截断，防越界读。
fn read_block(bytes: &[u8], off: usize, c: &Counts, time_size: usize) -> Result<ParsedBlock, TzifError> {
    let need = off + block_size(c, time_size);
    if bytes.len() < need {
        return Err(TzifError::Truncated);
    }
    let mut p = off;

    // transition 时间（有符号，v1 为 32 位、v2+ 为 64 位）。
    let mut trans = Vec::with_capacity(c.timecnt as usize);
    for _ in 0..c.timecnt {
        let v = if time_size == 8 {
            i64::from_be_bytes(bytes[p..p + 8].try_into().unwrap())
        } else {
            i32::from_be_bytes(bytes[p..p + 4].try_into().unwrap()) as i64
        };
        trans.push(v);
        p += time_size;
    }

    // transition 类型索引（每 transition 一字节）。
    let ttypes = bytes[p..p + c.timecnt as usize].to_vec();
    p += c.timecnt as usize;

    // 本地时间类型记录（6 字节：utoff 4 + isdst 1 + idx 1），只取 utoff。
    let mut types = Vec::with_capacity(c.typecnt as usize);
    for _ in 0..c.typecnt {
        let utoff = i32::from_be_bytes(bytes[p..p + 4].try_into().unwrap());
        types.push(utoff);
        p += 6;
    }

    for &idx in &ttypes {
        if idx >= c.typecnt as u8 {
            return Err(TzifError::Truncated);
        }
    }
    Ok(ParsedBlock { trans, ttypes, types })
}

/// 解析 TZif 二进制为 `TzifZone`。
///
/// # 步骤
/// 1. 校验魔数与版本字节。
/// 2. 读 v1 头；版本 ≥ 2 时跳过 v1 块、读 v2+ 头，一律用 64 位块（1970 年前
///    transition 为负值自然容纳）；版本 1 用 32 位块。
/// 3. 由 transition 类型索引与本地时间类型记录组装 `(纪元秒, 偏移秒)` 表；
///    LMT 取首个 time type，末偏移取末次 transition 的偏移。
///
/// # 边界与前提
/// - 闰秒表按 `leapcnt * (time_size + 4)` 推进指针，不建模。
/// - POSIX TZ 串（footer）读出后丢弃：末偏移已由末次 transition 给出。
///
/// # 注意事项
/// - 纯解析，无副作用；调用方负责把结果挂到静态表。
pub(crate) fn parse_tzif(bytes: &[u8]) -> Result<TzifZone, TzifError> {
    if bytes.len() < 44 {
        return Err(TzifError::Truncated);
    }
    if &bytes[0..4] != b"TZif" {
        return Err(TzifError::BadMagic);
    }
    let version = bytes[4];
    if !matches!(version, 0 | b'2' | b'3' | b'4') {
        return Err(TzifError::BadVersion);
    }

    // v1 头计数（版本 ≥ 2 时 v1 块是占位，只用于算跳过长度）。
    let c1 = read_counts(bytes, 0)?;

    let block = if version >= b'2' {
        let v1_size = block_size(&c1, 4);
        let off = 44 + v1_size;
        if bytes.len() < off + 44 {
            return Err(TzifError::Truncated);
        }
        if &bytes[off..off + 4] != b"TZif" {
            return Err(TzifError::BadMagic);
        }
        let c2 = read_counts(bytes, off)?;
        read_block(bytes, off + 44, &c2, 8)?
    } else {
        read_block(bytes, 44, &c1, 4)?
    };

    // LMT 是首个 time type 的偏移；末偏移取末次 transition 的偏移（POSIX TZ 串语义）。
    let lmt_offset = block.types.first().copied().unwrap_or(0);
    let transitions: Vec<(i64, i32)> = block
        .trans
        .iter()
        .enumerate()
        .map(|(i, &t)| (t, block.types[block.ttypes[i] as usize]))
        .collect();
    let last_offset = transitions.last().map(|&(_, o)| o).unwrap_or(lmt_offset);
    Ok(TzifZone {
        lmt_offset,
        transitions,
        last_offset,
    })
}

const ZONE_FILES: &[(&str, &[u8])] = &[
    ("Africa/Abidjan", include_bytes!("../../../../third_party/tzdb/Africa/Abidjan")),
    ("Africa/Accra", include_bytes!("../../../../third_party/tzdb/Africa/Accra")),
    ("Africa/Addis_Ababa", include_bytes!("../../../../third_party/tzdb/Africa/Addis_Ababa")),
    ("Africa/Algiers", include_bytes!("../../../../third_party/tzdb/Africa/Algiers")),
    ("Africa/Asmara", include_bytes!("../../../../third_party/tzdb/Africa/Asmara")),
    ("Africa/Bamako", include_bytes!("../../../../third_party/tzdb/Africa/Bamako")),
    ("Africa/Bangui", include_bytes!("../../../../third_party/tzdb/Africa/Bangui")),
    ("Africa/Banjul", include_bytes!("../../../../third_party/tzdb/Africa/Banjul")),
    ("Africa/Bissau", include_bytes!("../../../../third_party/tzdb/Africa/Bissau")),
    ("Africa/Blantyre", include_bytes!("../../../../third_party/tzdb/Africa/Blantyre")),
    ("Africa/Brazzaville", include_bytes!("../../../../third_party/tzdb/Africa/Brazzaville")),
    ("Africa/Bujumbura", include_bytes!("../../../../third_party/tzdb/Africa/Bujumbura")),
    ("Africa/Cairo", include_bytes!("../../../../third_party/tzdb/Africa/Cairo")),
    ("Africa/Casablanca", include_bytes!("../../../../third_party/tzdb/Africa/Casablanca")),
    ("Africa/Ceuta", include_bytes!("../../../../third_party/tzdb/Africa/Ceuta")),
    ("Africa/Conakry", include_bytes!("../../../../third_party/tzdb/Africa/Conakry")),
    ("Africa/Dakar", include_bytes!("../../../../third_party/tzdb/Africa/Dakar")),
    (
        "Africa/Dar_es_Salaam",
        include_bytes!("../../../../third_party/tzdb/Africa/Dar_es_Salaam"),
    ),
    ("Africa/Djibouti", include_bytes!("../../../../third_party/tzdb/Africa/Djibouti")),
    ("Africa/Douala", include_bytes!("../../../../third_party/tzdb/Africa/Douala")),
    ("Africa/El_Aaiun", include_bytes!("../../../../third_party/tzdb/Africa/El_Aaiun")),
    ("Africa/Freetown", include_bytes!("../../../../third_party/tzdb/Africa/Freetown")),
    ("Africa/Gaborone", include_bytes!("../../../../third_party/tzdb/Africa/Gaborone")),
    ("Africa/Harare", include_bytes!("../../../../third_party/tzdb/Africa/Harare")),
    ("Africa/Johannesburg", include_bytes!("../../../../third_party/tzdb/Africa/Johannesburg")),
    ("Africa/Juba", include_bytes!("../../../../third_party/tzdb/Africa/Juba")),
    ("Africa/Kampala", include_bytes!("../../../../third_party/tzdb/Africa/Kampala")),
    ("Africa/Khartoum", include_bytes!("../../../../third_party/tzdb/Africa/Khartoum")),
    ("Africa/Kigali", include_bytes!("../../../../third_party/tzdb/Africa/Kigali")),
    ("Africa/Kinshasa", include_bytes!("../../../../third_party/tzdb/Africa/Kinshasa")),
    ("Africa/Lagos", include_bytes!("../../../../third_party/tzdb/Africa/Lagos")),
    ("Africa/Libreville", include_bytes!("../../../../third_party/tzdb/Africa/Libreville")),
    ("Africa/Lome", include_bytes!("../../../../third_party/tzdb/Africa/Lome")),
    ("Africa/Luanda", include_bytes!("../../../../third_party/tzdb/Africa/Luanda")),
    ("Africa/Lubumbashi", include_bytes!("../../../../third_party/tzdb/Africa/Lubumbashi")),
    ("Africa/Lusaka", include_bytes!("../../../../third_party/tzdb/Africa/Lusaka")),
    ("Africa/Malabo", include_bytes!("../../../../third_party/tzdb/Africa/Malabo")),
    ("Africa/Maputo", include_bytes!("../../../../third_party/tzdb/Africa/Maputo")),
    ("Africa/Maseru", include_bytes!("../../../../third_party/tzdb/Africa/Maseru")),
    ("Africa/Mbabane", include_bytes!("../../../../third_party/tzdb/Africa/Mbabane")),
    ("Africa/Mogadishu", include_bytes!("../../../../third_party/tzdb/Africa/Mogadishu")),
    ("Africa/Monrovia", include_bytes!("../../../../third_party/tzdb/Africa/Monrovia")),
    ("Africa/Nairobi", include_bytes!("../../../../third_party/tzdb/Africa/Nairobi")),
    ("Africa/Ndjamena", include_bytes!("../../../../third_party/tzdb/Africa/Ndjamena")),
    ("Africa/Niamey", include_bytes!("../../../../third_party/tzdb/Africa/Niamey")),
    ("Africa/Nouakchott", include_bytes!("../../../../third_party/tzdb/Africa/Nouakchott")),
    ("Africa/Ouagadougou", include_bytes!("../../../../third_party/tzdb/Africa/Ouagadougou")),
    ("Africa/Porto-Novo", include_bytes!("../../../../third_party/tzdb/Africa/Porto-Novo")),
    ("Africa/Sao_Tome", include_bytes!("../../../../third_party/tzdb/Africa/Sao_Tome")),
    ("Africa/Timbuktu", include_bytes!("../../../../third_party/tzdb/Africa/Timbuktu")),
    ("Africa/Tripoli", include_bytes!("../../../../third_party/tzdb/Africa/Tripoli")),
    ("Africa/Tunis", include_bytes!("../../../../third_party/tzdb/Africa/Tunis")),
    ("Africa/Windhoek", include_bytes!("../../../../third_party/tzdb/Africa/Windhoek")),
    ("America/Adak", include_bytes!("../../../../third_party/tzdb/America/Adak")),
    ("America/Anchorage", include_bytes!("../../../../third_party/tzdb/America/Anchorage")),
    ("America/Anguilla", include_bytes!("../../../../third_party/tzdb/America/Anguilla")),
    ("America/Antigua", include_bytes!("../../../../third_party/tzdb/America/Antigua")),
    ("America/Araguaina", include_bytes!("../../../../third_party/tzdb/America/Araguaina")),
    (
        "America/Argentina/Buenos_Aires",
        include_bytes!("../../../../third_party/tzdb/America/Argentina/Buenos_Aires"),
    ),
    (
        "America/Argentina/Catamarca",
        include_bytes!("../../../../third_party/tzdb/America/Argentina/Catamarca"),
    ),
    (
        "America/Argentina/Cordoba",
        include_bytes!("../../../../third_party/tzdb/America/Argentina/Cordoba"),
    ),
    (
        "America/Argentina/Jujuy",
        include_bytes!("../../../../third_party/tzdb/America/Argentina/Jujuy"),
    ),
    (
        "America/Argentina/La_Rioja",
        include_bytes!("../../../../third_party/tzdb/America/Argentina/La_Rioja"),
    ),
    (
        "America/Argentina/Mendoza",
        include_bytes!("../../../../third_party/tzdb/America/Argentina/Mendoza"),
    ),
    (
        "America/Argentina/Rio_Gallegos",
        include_bytes!("../../../../third_party/tzdb/America/Argentina/Rio_Gallegos"),
    ),
    (
        "America/Argentina/Salta",
        include_bytes!("../../../../third_party/tzdb/America/Argentina/Salta"),
    ),
    (
        "America/Argentina/San_Juan",
        include_bytes!("../../../../third_party/tzdb/America/Argentina/San_Juan"),
    ),
    (
        "America/Argentina/San_Luis",
        include_bytes!("../../../../third_party/tzdb/America/Argentina/San_Luis"),
    ),
    (
        "America/Argentina/Tucuman",
        include_bytes!("../../../../third_party/tzdb/America/Argentina/Tucuman"),
    ),
    (
        "America/Argentina/Ushuaia",
        include_bytes!("../../../../third_party/tzdb/America/Argentina/Ushuaia"),
    ),
    ("America/Aruba", include_bytes!("../../../../third_party/tzdb/America/Aruba")),
    ("America/Asuncion", include_bytes!("../../../../third_party/tzdb/America/Asuncion")),
    ("America/Atikokan", include_bytes!("../../../../third_party/tzdb/America/Atikokan")),
    ("America/Atka", include_bytes!("../../../../third_party/tzdb/America/Atka")),
    ("America/Bahia", include_bytes!("../../../../third_party/tzdb/America/Bahia")),
    (
        "America/Bahia_Banderas",
        include_bytes!("../../../../third_party/tzdb/America/Bahia_Banderas"),
    ),
    ("America/Barbados", include_bytes!("../../../../third_party/tzdb/America/Barbados")),
    ("America/Belem", include_bytes!("../../../../third_party/tzdb/America/Belem")),
    ("America/Belize", include_bytes!("../../../../third_party/tzdb/America/Belize")),
    (
        "America/Blanc-Sablon",
        include_bytes!("../../../../third_party/tzdb/America/Blanc-Sablon"),
    ),
    ("America/Boa_Vista", include_bytes!("../../../../third_party/tzdb/America/Boa_Vista")),
    ("America/Bogota", include_bytes!("../../../../third_party/tzdb/America/Bogota")),
    ("America/Boise", include_bytes!("../../../../third_party/tzdb/America/Boise")),
    (
        "America/Cambridge_Bay",
        include_bytes!("../../../../third_party/tzdb/America/Cambridge_Bay"),
    ),
    (
        "America/Campo_Grande",
        include_bytes!("../../../../third_party/tzdb/America/Campo_Grande"),
    ),
    ("America/Cancun", include_bytes!("../../../../third_party/tzdb/America/Cancun")),
    ("America/Caracas", include_bytes!("../../../../third_party/tzdb/America/Caracas")),
    ("America/Cayenne", include_bytes!("../../../../third_party/tzdb/America/Cayenne")),
    ("America/Cayman", include_bytes!("../../../../third_party/tzdb/America/Cayman")),
    ("America/Chicago", include_bytes!("../../../../third_party/tzdb/America/Chicago")),
    ("America/Chihuahua", include_bytes!("../../../../third_party/tzdb/America/Chihuahua")),
    (
        "America/Ciudad_Juarez",
        include_bytes!("../../../../third_party/tzdb/America/Ciudad_Juarez"),
    ),
    (
        "America/Coral_Harbour",
        include_bytes!("../../../../third_party/tzdb/America/Coral_Harbour"),
    ),
    ("America/Costa_Rica", include_bytes!("../../../../third_party/tzdb/America/Costa_Rica")),
    ("America/Coyhaique", include_bytes!("../../../../third_party/tzdb/America/Coyhaique")),
    ("America/Creston", include_bytes!("../../../../third_party/tzdb/America/Creston")),
    ("America/Cuiaba", include_bytes!("../../../../third_party/tzdb/America/Cuiaba")),
    ("America/Curacao", include_bytes!("../../../../third_party/tzdb/America/Curacao")),
    (
        "America/Danmarkshavn",
        include_bytes!("../../../../third_party/tzdb/America/Danmarkshavn"),
    ),
    ("America/Dawson", include_bytes!("../../../../third_party/tzdb/America/Dawson")),
    (
        "America/Dawson_Creek",
        include_bytes!("../../../../third_party/tzdb/America/Dawson_Creek"),
    ),
    ("America/Denver", include_bytes!("../../../../third_party/tzdb/America/Denver")),
    ("America/Detroit", include_bytes!("../../../../third_party/tzdb/America/Detroit")),
    ("America/Dominica", include_bytes!("../../../../third_party/tzdb/America/Dominica")),
    ("America/Edmonton", include_bytes!("../../../../third_party/tzdb/America/Edmonton")),
    ("America/Eirunepe", include_bytes!("../../../../third_party/tzdb/America/Eirunepe")),
    ("America/El_Salvador", include_bytes!("../../../../third_party/tzdb/America/El_Salvador")),
    ("America/Ensenada", include_bytes!("../../../../third_party/tzdb/America/Ensenada")),
    ("America/Fort_Nelson", include_bytes!("../../../../third_party/tzdb/America/Fort_Nelson")),
    ("America/Fortaleza", include_bytes!("../../../../third_party/tzdb/America/Fortaleza")),
    ("America/Glace_Bay", include_bytes!("../../../../third_party/tzdb/America/Glace_Bay")),
    ("America/Goose_Bay", include_bytes!("../../../../third_party/tzdb/America/Goose_Bay")),
    ("America/Grand_Turk", include_bytes!("../../../../third_party/tzdb/America/Grand_Turk")),
    ("America/Grenada", include_bytes!("../../../../third_party/tzdb/America/Grenada")),
    ("America/Guadeloupe", include_bytes!("../../../../third_party/tzdb/America/Guadeloupe")),
    ("America/Guatemala", include_bytes!("../../../../third_party/tzdb/America/Guatemala")),
    ("America/Guayaquil", include_bytes!("../../../../third_party/tzdb/America/Guayaquil")),
    ("America/Guyana", include_bytes!("../../../../third_party/tzdb/America/Guyana")),
    ("America/Halifax", include_bytes!("../../../../third_party/tzdb/America/Halifax")),
    ("America/Havana", include_bytes!("../../../../third_party/tzdb/America/Havana")),
    ("America/Hermosillo", include_bytes!("../../../../third_party/tzdb/America/Hermosillo")),
    (
        "America/Indiana/Indianapolis",
        include_bytes!("../../../../third_party/tzdb/America/Indiana/Indianapolis"),
    ),
    (
        "America/Indiana/Knox",
        include_bytes!("../../../../third_party/tzdb/America/Indiana/Knox"),
    ),
    (
        "America/Indiana/Marengo",
        include_bytes!("../../../../third_party/tzdb/America/Indiana/Marengo"),
    ),
    (
        "America/Indiana/Petersburg",
        include_bytes!("../../../../third_party/tzdb/America/Indiana/Petersburg"),
    ),
    (
        "America/Indiana/Tell_City",
        include_bytes!("../../../../third_party/tzdb/America/Indiana/Tell_City"),
    ),
    (
        "America/Indiana/Vevay",
        include_bytes!("../../../../third_party/tzdb/America/Indiana/Vevay"),
    ),
    (
        "America/Indiana/Vincennes",
        include_bytes!("../../../../third_party/tzdb/America/Indiana/Vincennes"),
    ),
    (
        "America/Indiana/Winamac",
        include_bytes!("../../../../third_party/tzdb/America/Indiana/Winamac"),
    ),
    ("America/Inuvik", include_bytes!("../../../../third_party/tzdb/America/Inuvik")),
    ("America/Iqaluit", include_bytes!("../../../../third_party/tzdb/America/Iqaluit")),
    ("America/Jamaica", include_bytes!("../../../../third_party/tzdb/America/Jamaica")),
    ("America/Juneau", include_bytes!("../../../../third_party/tzdb/America/Juneau")),
    (
        "America/Kentucky/Louisville",
        include_bytes!("../../../../third_party/tzdb/America/Kentucky/Louisville"),
    ),
    (
        "America/Kentucky/Monticello",
        include_bytes!("../../../../third_party/tzdb/America/Kentucky/Monticello"),
    ),
    ("America/Kralendijk", include_bytes!("../../../../third_party/tzdb/America/Kralendijk")),
    ("America/La_Paz", include_bytes!("../../../../third_party/tzdb/America/La_Paz")),
    ("America/Lima", include_bytes!("../../../../third_party/tzdb/America/Lima")),
    ("America/Los_Angeles", include_bytes!("../../../../third_party/tzdb/America/Los_Angeles")),
    (
        "America/Lower_Princes",
        include_bytes!("../../../../third_party/tzdb/America/Lower_Princes"),
    ),
    ("America/Maceio", include_bytes!("../../../../third_party/tzdb/America/Maceio")),
    ("America/Managua", include_bytes!("../../../../third_party/tzdb/America/Managua")),
    ("America/Manaus", include_bytes!("../../../../third_party/tzdb/America/Manaus")),
    ("America/Marigot", include_bytes!("../../../../third_party/tzdb/America/Marigot")),
    ("America/Martinique", include_bytes!("../../../../third_party/tzdb/America/Martinique")),
    ("America/Matamoros", include_bytes!("../../../../third_party/tzdb/America/Matamoros")),
    ("America/Mazatlan", include_bytes!("../../../../third_party/tzdb/America/Mazatlan")),
    ("America/Menominee", include_bytes!("../../../../third_party/tzdb/America/Menominee")),
    ("America/Merida", include_bytes!("../../../../third_party/tzdb/America/Merida")),
    ("America/Metlakatla", include_bytes!("../../../../third_party/tzdb/America/Metlakatla")),
    ("America/Mexico_City", include_bytes!("../../../../third_party/tzdb/America/Mexico_City")),
    ("America/Miquelon", include_bytes!("../../../../third_party/tzdb/America/Miquelon")),
    ("America/Moncton", include_bytes!("../../../../third_party/tzdb/America/Moncton")),
    ("America/Monterrey", include_bytes!("../../../../third_party/tzdb/America/Monterrey")),
    ("America/Montevideo", include_bytes!("../../../../third_party/tzdb/America/Montevideo")),
    ("America/Montreal", include_bytes!("../../../../third_party/tzdb/America/Montreal")),
    ("America/Montserrat", include_bytes!("../../../../third_party/tzdb/America/Montserrat")),
    ("America/Nassau", include_bytes!("../../../../third_party/tzdb/America/Nassau")),
    ("America/New_York", include_bytes!("../../../../third_party/tzdb/America/New_York")),
    ("America/Nipigon", include_bytes!("../../../../third_party/tzdb/America/Nipigon")),
    ("America/Nome", include_bytes!("../../../../third_party/tzdb/America/Nome")),
    ("America/Noronha", include_bytes!("../../../../third_party/tzdb/America/Noronha")),
    (
        "America/North_Dakota/Beulah",
        include_bytes!("../../../../third_party/tzdb/America/North_Dakota/Beulah"),
    ),
    (
        "America/North_Dakota/Center",
        include_bytes!("../../../../third_party/tzdb/America/North_Dakota/Center"),
    ),
    (
        "America/North_Dakota/New_Salem",
        include_bytes!("../../../../third_party/tzdb/America/North_Dakota/New_Salem"),
    ),
    ("America/Nuuk", include_bytes!("../../../../third_party/tzdb/America/Nuuk")),
    ("America/Ojinaga", include_bytes!("../../../../third_party/tzdb/America/Ojinaga")),
    ("America/Panama", include_bytes!("../../../../third_party/tzdb/America/Panama")),
    ("America/Pangnirtung", include_bytes!("../../../../third_party/tzdb/America/Pangnirtung")),
    ("America/Paramaribo", include_bytes!("../../../../third_party/tzdb/America/Paramaribo")),
    ("America/Phoenix", include_bytes!("../../../../third_party/tzdb/America/Phoenix")),
    (
        "America/Port-au-Prince",
        include_bytes!("../../../../third_party/tzdb/America/Port-au-Prince"),
    ),
    (
        "America/Port_of_Spain",
        include_bytes!("../../../../third_party/tzdb/America/Port_of_Spain"),
    ),
    ("America/Porto_Acre", include_bytes!("../../../../third_party/tzdb/America/Porto_Acre")),
    ("America/Porto_Velho", include_bytes!("../../../../third_party/tzdb/America/Porto_Velho")),
    ("America/Puerto_Rico", include_bytes!("../../../../third_party/tzdb/America/Puerto_Rico")),
    (
        "America/Punta_Arenas",
        include_bytes!("../../../../third_party/tzdb/America/Punta_Arenas"),
    ),
    ("America/Rainy_River", include_bytes!("../../../../third_party/tzdb/America/Rainy_River")),
    (
        "America/Rankin_Inlet",
        include_bytes!("../../../../third_party/tzdb/America/Rankin_Inlet"),
    ),
    ("America/Recife", include_bytes!("../../../../third_party/tzdb/America/Recife")),
    ("America/Regina", include_bytes!("../../../../third_party/tzdb/America/Regina")),
    ("America/Resolute", include_bytes!("../../../../third_party/tzdb/America/Resolute")),
    ("America/Rio_Branco", include_bytes!("../../../../third_party/tzdb/America/Rio_Branco")),
    (
        "America/Santa_Isabel",
        include_bytes!("../../../../third_party/tzdb/America/Santa_Isabel"),
    ),
    ("America/Santarem", include_bytes!("../../../../third_party/tzdb/America/Santarem")),
    ("America/Santiago", include_bytes!("../../../../third_party/tzdb/America/Santiago")),
    (
        "America/Santo_Domingo",
        include_bytes!("../../../../third_party/tzdb/America/Santo_Domingo"),
    ),
    ("America/Sao_Paulo", include_bytes!("../../../../third_party/tzdb/America/Sao_Paulo")),
    (
        "America/Scoresbysund",
        include_bytes!("../../../../third_party/tzdb/America/Scoresbysund"),
    ),
    ("America/Shiprock", include_bytes!("../../../../third_party/tzdb/America/Shiprock")),
    ("America/Sitka", include_bytes!("../../../../third_party/tzdb/America/Sitka")),
    (
        "America/St_Barthelemy",
        include_bytes!("../../../../third_party/tzdb/America/St_Barthelemy"),
    ),
    ("America/St_Johns", include_bytes!("../../../../third_party/tzdb/America/St_Johns")),
    ("America/St_Kitts", include_bytes!("../../../../third_party/tzdb/America/St_Kitts")),
    ("America/St_Lucia", include_bytes!("../../../../third_party/tzdb/America/St_Lucia")),
    ("America/St_Thomas", include_bytes!("../../../../third_party/tzdb/America/St_Thomas")),
    ("America/St_Vincent", include_bytes!("../../../../third_party/tzdb/America/St_Vincent")),
    (
        "America/Swift_Current",
        include_bytes!("../../../../third_party/tzdb/America/Swift_Current"),
    ),
    ("America/Tegucigalpa", include_bytes!("../../../../third_party/tzdb/America/Tegucigalpa")),
    ("America/Thule", include_bytes!("../../../../third_party/tzdb/America/Thule")),
    ("America/Thunder_Bay", include_bytes!("../../../../third_party/tzdb/America/Thunder_Bay")),
    ("America/Tijuana", include_bytes!("../../../../third_party/tzdb/America/Tijuana")),
    ("America/Toronto", include_bytes!("../../../../third_party/tzdb/America/Toronto")),
    ("America/Tortola", include_bytes!("../../../../third_party/tzdb/America/Tortola")),
    ("America/Vancouver", include_bytes!("../../../../third_party/tzdb/America/Vancouver")),
    ("America/Virgin", include_bytes!("../../../../third_party/tzdb/America/Virgin")),
    ("America/Whitehorse", include_bytes!("../../../../third_party/tzdb/America/Whitehorse")),
    ("America/Winnipeg", include_bytes!("../../../../third_party/tzdb/America/Winnipeg")),
    ("America/Yakutat", include_bytes!("../../../../third_party/tzdb/America/Yakutat")),
    ("America/Yellowknife", include_bytes!("../../../../third_party/tzdb/America/Yellowknife")),
    ("Antarctica/Casey", include_bytes!("../../../../third_party/tzdb/Antarctica/Casey")),
    ("Antarctica/Davis", include_bytes!("../../../../third_party/tzdb/Antarctica/Davis")),
    (
        "Antarctica/DumontDUrville",
        include_bytes!("../../../../third_party/tzdb/Antarctica/DumontDUrville"),
    ),
    (
        "Antarctica/Macquarie",
        include_bytes!("../../../../third_party/tzdb/Antarctica/Macquarie"),
    ),
    ("Antarctica/Mawson", include_bytes!("../../../../third_party/tzdb/Antarctica/Mawson")),
    ("Antarctica/McMurdo", include_bytes!("../../../../third_party/tzdb/Antarctica/McMurdo")),
    ("Antarctica/Palmer", include_bytes!("../../../../third_party/tzdb/Antarctica/Palmer")),
    ("Antarctica/Rothera", include_bytes!("../../../../third_party/tzdb/Antarctica/Rothera")),
    ("Antarctica/Syowa", include_bytes!("../../../../third_party/tzdb/Antarctica/Syowa")),
    ("Antarctica/Troll", include_bytes!("../../../../third_party/tzdb/Antarctica/Troll")),
    ("Antarctica/Vostok", include_bytes!("../../../../third_party/tzdb/Antarctica/Vostok")),
    ("Arctic/Longyearbyen", include_bytes!("../../../../third_party/tzdb/Arctic/Longyearbyen")),
    ("Asia/Aden", include_bytes!("../../../../third_party/tzdb/Asia/Aden")),
    ("Asia/Almaty", include_bytes!("../../../../third_party/tzdb/Asia/Almaty")),
    ("Asia/Amman", include_bytes!("../../../../third_party/tzdb/Asia/Amman")),
    ("Asia/Anadyr", include_bytes!("../../../../third_party/tzdb/Asia/Anadyr")),
    ("Asia/Aqtau", include_bytes!("../../../../third_party/tzdb/Asia/Aqtau")),
    ("Asia/Aqtobe", include_bytes!("../../../../third_party/tzdb/Asia/Aqtobe")),
    ("Asia/Ashgabat", include_bytes!("../../../../third_party/tzdb/Asia/Ashgabat")),
    ("Asia/Atyrau", include_bytes!("../../../../third_party/tzdb/Asia/Atyrau")),
    ("Asia/Baghdad", include_bytes!("../../../../third_party/tzdb/Asia/Baghdad")),
    ("Asia/Bahrain", include_bytes!("../../../../third_party/tzdb/Asia/Bahrain")),
    ("Asia/Baku", include_bytes!("../../../../third_party/tzdb/Asia/Baku")),
    ("Asia/Bangkok", include_bytes!("../../../../third_party/tzdb/Asia/Bangkok")),
    ("Asia/Barnaul", include_bytes!("../../../../third_party/tzdb/Asia/Barnaul")),
    ("Asia/Beirut", include_bytes!("../../../../third_party/tzdb/Asia/Beirut")),
    ("Asia/Bishkek", include_bytes!("../../../../third_party/tzdb/Asia/Bishkek")),
    ("Asia/Brunei", include_bytes!("../../../../third_party/tzdb/Asia/Brunei")),
    ("Asia/Chita", include_bytes!("../../../../third_party/tzdb/Asia/Chita")),
    ("Asia/Chongqing", include_bytes!("../../../../third_party/tzdb/Asia/Chongqing")),
    ("Asia/Colombo", include_bytes!("../../../../third_party/tzdb/Asia/Colombo")),
    ("Asia/Damascus", include_bytes!("../../../../third_party/tzdb/Asia/Damascus")),
    ("Asia/Dhaka", include_bytes!("../../../../third_party/tzdb/Asia/Dhaka")),
    ("Asia/Dili", include_bytes!("../../../../third_party/tzdb/Asia/Dili")),
    ("Asia/Dubai", include_bytes!("../../../../third_party/tzdb/Asia/Dubai")),
    ("Asia/Dushanbe", include_bytes!("../../../../third_party/tzdb/Asia/Dushanbe")),
    ("Asia/Famagusta", include_bytes!("../../../../third_party/tzdb/Asia/Famagusta")),
    ("Asia/Gaza", include_bytes!("../../../../third_party/tzdb/Asia/Gaza")),
    ("Asia/Harbin", include_bytes!("../../../../third_party/tzdb/Asia/Harbin")),
    ("Asia/Hebron", include_bytes!("../../../../third_party/tzdb/Asia/Hebron")),
    ("Asia/Ho_Chi_Minh", include_bytes!("../../../../third_party/tzdb/Asia/Ho_Chi_Minh")),
    ("Asia/Hong_Kong", include_bytes!("../../../../third_party/tzdb/Asia/Hong_Kong")),
    ("Asia/Hovd", include_bytes!("../../../../third_party/tzdb/Asia/Hovd")),
    ("Asia/Irkutsk", include_bytes!("../../../../third_party/tzdb/Asia/Irkutsk")),
    ("Asia/Istanbul", include_bytes!("../../../../third_party/tzdb/Asia/Istanbul")),
    ("Asia/Jakarta", include_bytes!("../../../../third_party/tzdb/Asia/Jakarta")),
    ("Asia/Jayapura", include_bytes!("../../../../third_party/tzdb/Asia/Jayapura")),
    ("Asia/Jerusalem", include_bytes!("../../../../third_party/tzdb/Asia/Jerusalem")),
    ("Asia/Kabul", include_bytes!("../../../../third_party/tzdb/Asia/Kabul")),
    ("Asia/Kamchatka", include_bytes!("../../../../third_party/tzdb/Asia/Kamchatka")),
    ("Asia/Karachi", include_bytes!("../../../../third_party/tzdb/Asia/Karachi")),
    ("Asia/Kashgar", include_bytes!("../../../../third_party/tzdb/Asia/Kashgar")),
    ("Asia/Kathmandu", include_bytes!("../../../../third_party/tzdb/Asia/Kathmandu")),
    ("Asia/Khandyga", include_bytes!("../../../../third_party/tzdb/Asia/Khandyga")),
    ("Asia/Kolkata", include_bytes!("../../../../third_party/tzdb/Asia/Kolkata")),
    ("Asia/Krasnoyarsk", include_bytes!("../../../../third_party/tzdb/Asia/Krasnoyarsk")),
    ("Asia/Kuala_Lumpur", include_bytes!("../../../../third_party/tzdb/Asia/Kuala_Lumpur")),
    ("Asia/Kuching", include_bytes!("../../../../third_party/tzdb/Asia/Kuching")),
    ("Asia/Kuwait", include_bytes!("../../../../third_party/tzdb/Asia/Kuwait")),
    ("Asia/Macau", include_bytes!("../../../../third_party/tzdb/Asia/Macau")),
    ("Asia/Magadan", include_bytes!("../../../../third_party/tzdb/Asia/Magadan")),
    ("Asia/Makassar", include_bytes!("../../../../third_party/tzdb/Asia/Makassar")),
    ("Asia/Manila", include_bytes!("../../../../third_party/tzdb/Asia/Manila")),
    ("Asia/Muscat", include_bytes!("../../../../third_party/tzdb/Asia/Muscat")),
    ("Asia/Nicosia", include_bytes!("../../../../third_party/tzdb/Asia/Nicosia")),
    ("Asia/Novokuznetsk", include_bytes!("../../../../third_party/tzdb/Asia/Novokuznetsk")),
    ("Asia/Novosibirsk", include_bytes!("../../../../third_party/tzdb/Asia/Novosibirsk")),
    ("Asia/Omsk", include_bytes!("../../../../third_party/tzdb/Asia/Omsk")),
    ("Asia/Oral", include_bytes!("../../../../third_party/tzdb/Asia/Oral")),
    ("Asia/Phnom_Penh", include_bytes!("../../../../third_party/tzdb/Asia/Phnom_Penh")),
    ("Asia/Pontianak", include_bytes!("../../../../third_party/tzdb/Asia/Pontianak")),
    ("Asia/Pyongyang", include_bytes!("../../../../third_party/tzdb/Asia/Pyongyang")),
    ("Asia/Qatar", include_bytes!("../../../../third_party/tzdb/Asia/Qatar")),
    ("Asia/Qostanay", include_bytes!("../../../../third_party/tzdb/Asia/Qostanay")),
    ("Asia/Qyzylorda", include_bytes!("../../../../third_party/tzdb/Asia/Qyzylorda")),
    ("Asia/Riyadh", include_bytes!("../../../../third_party/tzdb/Asia/Riyadh")),
    ("Asia/Sakhalin", include_bytes!("../../../../third_party/tzdb/Asia/Sakhalin")),
    ("Asia/Samarkand", include_bytes!("../../../../third_party/tzdb/Asia/Samarkand")),
    ("Asia/Seoul", include_bytes!("../../../../third_party/tzdb/Asia/Seoul")),
    ("Asia/Shanghai", include_bytes!("../../../../third_party/tzdb/Asia/Shanghai")),
    ("Asia/Singapore", include_bytes!("../../../../third_party/tzdb/Asia/Singapore")),
    ("Asia/Srednekolymsk", include_bytes!("../../../../third_party/tzdb/Asia/Srednekolymsk")),
    ("Asia/Taipei", include_bytes!("../../../../third_party/tzdb/Asia/Taipei")),
    ("Asia/Tashkent", include_bytes!("../../../../third_party/tzdb/Asia/Tashkent")),
    ("Asia/Tbilisi", include_bytes!("../../../../third_party/tzdb/Asia/Tbilisi")),
    ("Asia/Tehran", include_bytes!("../../../../third_party/tzdb/Asia/Tehran")),
    ("Asia/Tel_Aviv", include_bytes!("../../../../third_party/tzdb/Asia/Tel_Aviv")),
    ("Asia/Thimphu", include_bytes!("../../../../third_party/tzdb/Asia/Thimphu")),
    ("Asia/Tokyo", include_bytes!("../../../../third_party/tzdb/Asia/Tokyo")),
    ("Asia/Tomsk", include_bytes!("../../../../third_party/tzdb/Asia/Tomsk")),
    ("Asia/Ulaanbaatar", include_bytes!("../../../../third_party/tzdb/Asia/Ulaanbaatar")),
    ("Asia/Urumqi", include_bytes!("../../../../third_party/tzdb/Asia/Urumqi")),
    ("Asia/Ust-Nera", include_bytes!("../../../../third_party/tzdb/Asia/Ust-Nera")),
    ("Asia/Vientiane", include_bytes!("../../../../third_party/tzdb/Asia/Vientiane")),
    ("Asia/Vladivostok", include_bytes!("../../../../third_party/tzdb/Asia/Vladivostok")),
    ("Asia/Yakutsk", include_bytes!("../../../../third_party/tzdb/Asia/Yakutsk")),
    ("Asia/Yangon", include_bytes!("../../../../third_party/tzdb/Asia/Yangon")),
    ("Asia/Yekaterinburg", include_bytes!("../../../../third_party/tzdb/Asia/Yekaterinburg")),
    ("Asia/Yerevan", include_bytes!("../../../../third_party/tzdb/Asia/Yerevan")),
    ("Atlantic/Azores", include_bytes!("../../../../third_party/tzdb/Atlantic/Azores")),
    ("Atlantic/Bermuda", include_bytes!("../../../../third_party/tzdb/Atlantic/Bermuda")),
    ("Atlantic/Canary", include_bytes!("../../../../third_party/tzdb/Atlantic/Canary")),
    ("Atlantic/Cape_Verde", include_bytes!("../../../../third_party/tzdb/Atlantic/Cape_Verde")),
    ("Atlantic/Faroe", include_bytes!("../../../../third_party/tzdb/Atlantic/Faroe")),
    ("Atlantic/Jan_Mayen", include_bytes!("../../../../third_party/tzdb/Atlantic/Jan_Mayen")),
    ("Atlantic/Madeira", include_bytes!("../../../../third_party/tzdb/Atlantic/Madeira")),
    ("Atlantic/Reykjavik", include_bytes!("../../../../third_party/tzdb/Atlantic/Reykjavik")),
    (
        "Atlantic/South_Georgia",
        include_bytes!("../../../../third_party/tzdb/Atlantic/South_Georgia"),
    ),
    ("Atlantic/St_Helena", include_bytes!("../../../../third_party/tzdb/Atlantic/St_Helena")),
    ("Atlantic/Stanley", include_bytes!("../../../../third_party/tzdb/Atlantic/Stanley")),
    ("Australia/Adelaide", include_bytes!("../../../../third_party/tzdb/Australia/Adelaide")),
    ("Australia/Brisbane", include_bytes!("../../../../third_party/tzdb/Australia/Brisbane")),
    (
        "Australia/Broken_Hill",
        include_bytes!("../../../../third_party/tzdb/Australia/Broken_Hill"),
    ),
    ("Australia/Canberra", include_bytes!("../../../../third_party/tzdb/Australia/Canberra")),
    ("Australia/Currie", include_bytes!("../../../../third_party/tzdb/Australia/Currie")),
    ("Australia/Darwin", include_bytes!("../../../../third_party/tzdb/Australia/Darwin")),
    ("Australia/Eucla", include_bytes!("../../../../third_party/tzdb/Australia/Eucla")),
    ("Australia/Hobart", include_bytes!("../../../../third_party/tzdb/Australia/Hobart")),
    ("Australia/Lindeman", include_bytes!("../../../../third_party/tzdb/Australia/Lindeman")),
    ("Australia/Lord_Howe", include_bytes!("../../../../third_party/tzdb/Australia/Lord_Howe")),
    ("Australia/Melbourne", include_bytes!("../../../../third_party/tzdb/Australia/Melbourne")),
    ("Australia/Perth", include_bytes!("../../../../third_party/tzdb/Australia/Perth")),
    ("Australia/Sydney", include_bytes!("../../../../third_party/tzdb/Australia/Sydney")),
    (
        "Australia/Yancowinna",
        include_bytes!("../../../../third_party/tzdb/Australia/Yancowinna"),
    ),
    ("Etc/GMT", include_bytes!("../../../../third_party/tzdb/Etc/GMT")),
    ("Etc/GMT+0", include_bytes!("../../../../third_party/tzdb/Etc/GMT+0")),
    ("Etc/GMT+1", include_bytes!("../../../../third_party/tzdb/Etc/GMT+1")),
    ("Etc/GMT+10", include_bytes!("../../../../third_party/tzdb/Etc/GMT+10")),
    ("Etc/GMT+11", include_bytes!("../../../../third_party/tzdb/Etc/GMT+11")),
    ("Etc/GMT+12", include_bytes!("../../../../third_party/tzdb/Etc/GMT+12")),
    ("Etc/GMT+2", include_bytes!("../../../../third_party/tzdb/Etc/GMT+2")),
    ("Etc/GMT+3", include_bytes!("../../../../third_party/tzdb/Etc/GMT+3")),
    ("Etc/GMT+4", include_bytes!("../../../../third_party/tzdb/Etc/GMT+4")),
    ("Etc/GMT+5", include_bytes!("../../../../third_party/tzdb/Etc/GMT+5")),
    ("Etc/GMT+6", include_bytes!("../../../../third_party/tzdb/Etc/GMT+6")),
    ("Etc/GMT+7", include_bytes!("../../../../third_party/tzdb/Etc/GMT+7")),
    ("Etc/GMT+8", include_bytes!("../../../../third_party/tzdb/Etc/GMT+8")),
    ("Etc/GMT+9", include_bytes!("../../../../third_party/tzdb/Etc/GMT+9")),
    ("Etc/GMT-0", include_bytes!("../../../../third_party/tzdb/Etc/GMT-0")),
    ("Etc/GMT-1", include_bytes!("../../../../third_party/tzdb/Etc/GMT-1")),
    ("Etc/GMT-10", include_bytes!("../../../../third_party/tzdb/Etc/GMT-10")),
    ("Etc/GMT-11", include_bytes!("../../../../third_party/tzdb/Etc/GMT-11")),
    ("Etc/GMT-12", include_bytes!("../../../../third_party/tzdb/Etc/GMT-12")),
    ("Etc/GMT-13", include_bytes!("../../../../third_party/tzdb/Etc/GMT-13")),
    ("Etc/GMT-14", include_bytes!("../../../../third_party/tzdb/Etc/GMT-14")),
    ("Etc/GMT-2", include_bytes!("../../../../third_party/tzdb/Etc/GMT-2")),
    ("Etc/GMT-3", include_bytes!("../../../../third_party/tzdb/Etc/GMT-3")),
    ("Etc/GMT-4", include_bytes!("../../../../third_party/tzdb/Etc/GMT-4")),
    ("Etc/GMT-5", include_bytes!("../../../../third_party/tzdb/Etc/GMT-5")),
    ("Etc/GMT-6", include_bytes!("../../../../third_party/tzdb/Etc/GMT-6")),
    ("Etc/GMT-7", include_bytes!("../../../../third_party/tzdb/Etc/GMT-7")),
    ("Etc/GMT-8", include_bytes!("../../../../third_party/tzdb/Etc/GMT-8")),
    ("Etc/GMT-9", include_bytes!("../../../../third_party/tzdb/Etc/GMT-9")),
    ("Etc/GMT0", include_bytes!("../../../../third_party/tzdb/Etc/GMT0")),
    ("Etc/Greenwich", include_bytes!("../../../../third_party/tzdb/Etc/Greenwich")),
    ("Etc/UCT", include_bytes!("../../../../third_party/tzdb/Etc/UCT")),
    ("Etc/UTC", include_bytes!("../../../../third_party/tzdb/Etc/UTC")),
    ("Etc/Universal", include_bytes!("../../../../third_party/tzdb/Etc/Universal")),
    ("Etc/Zulu", include_bytes!("../../../../third_party/tzdb/Etc/Zulu")),
    ("Europe/Amsterdam", include_bytes!("../../../../third_party/tzdb/Europe/Amsterdam")),
    ("Europe/Andorra", include_bytes!("../../../../third_party/tzdb/Europe/Andorra")),
    ("Europe/Astrakhan", include_bytes!("../../../../third_party/tzdb/Europe/Astrakhan")),
    ("Europe/Athens", include_bytes!("../../../../third_party/tzdb/Europe/Athens")),
    ("Europe/Belfast", include_bytes!("../../../../third_party/tzdb/Europe/Belfast")),
    ("Europe/Belgrade", include_bytes!("../../../../third_party/tzdb/Europe/Belgrade")),
    ("Europe/Berlin", include_bytes!("../../../../third_party/tzdb/Europe/Berlin")),
    ("Europe/Bratislava", include_bytes!("../../../../third_party/tzdb/Europe/Bratislava")),
    ("Europe/Brussels", include_bytes!("../../../../third_party/tzdb/Europe/Brussels")),
    ("Europe/Bucharest", include_bytes!("../../../../third_party/tzdb/Europe/Bucharest")),
    ("Europe/Budapest", include_bytes!("../../../../third_party/tzdb/Europe/Budapest")),
    ("Europe/Busingen", include_bytes!("../../../../third_party/tzdb/Europe/Busingen")),
    ("Europe/Chisinau", include_bytes!("../../../../third_party/tzdb/Europe/Chisinau")),
    ("Europe/Copenhagen", include_bytes!("../../../../third_party/tzdb/Europe/Copenhagen")),
    ("Europe/Dublin", include_bytes!("../../../../third_party/tzdb/Europe/Dublin")),
    ("Europe/Gibraltar", include_bytes!("../../../../third_party/tzdb/Europe/Gibraltar")),
    ("Europe/Guernsey", include_bytes!("../../../../third_party/tzdb/Europe/Guernsey")),
    ("Europe/Helsinki", include_bytes!("../../../../third_party/tzdb/Europe/Helsinki")),
    ("Europe/Isle_of_Man", include_bytes!("../../../../third_party/tzdb/Europe/Isle_of_Man")),
    ("Europe/Istanbul", include_bytes!("../../../../third_party/tzdb/Europe/Istanbul")),
    ("Europe/Jersey", include_bytes!("../../../../third_party/tzdb/Europe/Jersey")),
    ("Europe/Kaliningrad", include_bytes!("../../../../third_party/tzdb/Europe/Kaliningrad")),
    ("Europe/Kirov", include_bytes!("../../../../third_party/tzdb/Europe/Kirov")),
    ("Europe/Kyiv", include_bytes!("../../../../third_party/tzdb/Europe/Kyiv")),
    ("Europe/Lisbon", include_bytes!("../../../../third_party/tzdb/Europe/Lisbon")),
    ("Europe/Ljubljana", include_bytes!("../../../../third_party/tzdb/Europe/Ljubljana")),
    ("Europe/London", include_bytes!("../../../../third_party/tzdb/Europe/London")),
    ("Europe/Luxembourg", include_bytes!("../../../../third_party/tzdb/Europe/Luxembourg")),
    ("Europe/Madrid", include_bytes!("../../../../third_party/tzdb/Europe/Madrid")),
    ("Europe/Malta", include_bytes!("../../../../third_party/tzdb/Europe/Malta")),
    ("Europe/Mariehamn", include_bytes!("../../../../third_party/tzdb/Europe/Mariehamn")),
    ("Europe/Minsk", include_bytes!("../../../../third_party/tzdb/Europe/Minsk")),
    ("Europe/Monaco", include_bytes!("../../../../third_party/tzdb/Europe/Monaco")),
    ("Europe/Moscow", include_bytes!("../../../../third_party/tzdb/Europe/Moscow")),
    ("Europe/Nicosia", include_bytes!("../../../../third_party/tzdb/Europe/Nicosia")),
    ("Europe/Oslo", include_bytes!("../../../../third_party/tzdb/Europe/Oslo")),
    ("Europe/Paris", include_bytes!("../../../../third_party/tzdb/Europe/Paris")),
    ("Europe/Podgorica", include_bytes!("../../../../third_party/tzdb/Europe/Podgorica")),
    ("Europe/Prague", include_bytes!("../../../../third_party/tzdb/Europe/Prague")),
    ("Europe/Riga", include_bytes!("../../../../third_party/tzdb/Europe/Riga")),
    ("Europe/Rome", include_bytes!("../../../../third_party/tzdb/Europe/Rome")),
    ("Europe/Samara", include_bytes!("../../../../third_party/tzdb/Europe/Samara")),
    ("Europe/San_Marino", include_bytes!("../../../../third_party/tzdb/Europe/San_Marino")),
    ("Europe/Sarajevo", include_bytes!("../../../../third_party/tzdb/Europe/Sarajevo")),
    ("Europe/Saratov", include_bytes!("../../../../third_party/tzdb/Europe/Saratov")),
    ("Europe/Simferopol", include_bytes!("../../../../third_party/tzdb/Europe/Simferopol")),
    ("Europe/Skopje", include_bytes!("../../../../third_party/tzdb/Europe/Skopje")),
    ("Europe/Sofia", include_bytes!("../../../../third_party/tzdb/Europe/Sofia")),
    ("Europe/Stockholm", include_bytes!("../../../../third_party/tzdb/Europe/Stockholm")),
    ("Europe/Tallinn", include_bytes!("../../../../third_party/tzdb/Europe/Tallinn")),
    ("Europe/Tirane", include_bytes!("../../../../third_party/tzdb/Europe/Tirane")),
    ("Europe/Tiraspol", include_bytes!("../../../../third_party/tzdb/Europe/Tiraspol")),
    ("Europe/Ulyanovsk", include_bytes!("../../../../third_party/tzdb/Europe/Ulyanovsk")),
    ("Europe/Vaduz", include_bytes!("../../../../third_party/tzdb/Europe/Vaduz")),
    ("Europe/Vatican", include_bytes!("../../../../third_party/tzdb/Europe/Vatican")),
    ("Europe/Vienna", include_bytes!("../../../../third_party/tzdb/Europe/Vienna")),
    ("Europe/Vilnius", include_bytes!("../../../../third_party/tzdb/Europe/Vilnius")),
    ("Europe/Volgograd", include_bytes!("../../../../third_party/tzdb/Europe/Volgograd")),
    ("Europe/Warsaw", include_bytes!("../../../../third_party/tzdb/Europe/Warsaw")),
    ("Europe/Zagreb", include_bytes!("../../../../third_party/tzdb/Europe/Zagreb")),
    ("Europe/Zurich", include_bytes!("../../../../third_party/tzdb/Europe/Zurich")),
    ("Factory", include_bytes!("../../../../third_party/tzdb/Factory")),
    ("GMT", include_bytes!("../../../../third_party/tzdb/GMT")),
    ("Indian/Antananarivo", include_bytes!("../../../../third_party/tzdb/Indian/Antananarivo")),
    ("Indian/Chagos", include_bytes!("../../../../third_party/tzdb/Indian/Chagos")),
    ("Indian/Christmas", include_bytes!("../../../../third_party/tzdb/Indian/Christmas")),
    ("Indian/Cocos", include_bytes!("../../../../third_party/tzdb/Indian/Cocos")),
    ("Indian/Comoro", include_bytes!("../../../../third_party/tzdb/Indian/Comoro")),
    ("Indian/Kerguelen", include_bytes!("../../../../third_party/tzdb/Indian/Kerguelen")),
    ("Indian/Mahe", include_bytes!("../../../../third_party/tzdb/Indian/Mahe")),
    ("Indian/Maldives", include_bytes!("../../../../third_party/tzdb/Indian/Maldives")),
    ("Indian/Mauritius", include_bytes!("../../../../third_party/tzdb/Indian/Mauritius")),
    ("Indian/Mayotte", include_bytes!("../../../../third_party/tzdb/Indian/Mayotte")),
    ("Indian/Reunion", include_bytes!("../../../../third_party/tzdb/Indian/Reunion")),
    ("Pacific/Apia", include_bytes!("../../../../third_party/tzdb/Pacific/Apia")),
    ("Pacific/Auckland", include_bytes!("../../../../third_party/tzdb/Pacific/Auckland")),
    (
        "Pacific/Bougainville",
        include_bytes!("../../../../third_party/tzdb/Pacific/Bougainville"),
    ),
    ("Pacific/Chatham", include_bytes!("../../../../third_party/tzdb/Pacific/Chatham")),
    ("Pacific/Chuuk", include_bytes!("../../../../third_party/tzdb/Pacific/Chuuk")),
    ("Pacific/Easter", include_bytes!("../../../../third_party/tzdb/Pacific/Easter")),
    ("Pacific/Efate", include_bytes!("../../../../third_party/tzdb/Pacific/Efate")),
    ("Pacific/Fakaofo", include_bytes!("../../../../third_party/tzdb/Pacific/Fakaofo")),
    ("Pacific/Fiji", include_bytes!("../../../../third_party/tzdb/Pacific/Fiji")),
    ("Pacific/Funafuti", include_bytes!("../../../../third_party/tzdb/Pacific/Funafuti")),
    ("Pacific/Galapagos", include_bytes!("../../../../third_party/tzdb/Pacific/Galapagos")),
    ("Pacific/Gambier", include_bytes!("../../../../third_party/tzdb/Pacific/Gambier")),
    ("Pacific/Guadalcanal", include_bytes!("../../../../third_party/tzdb/Pacific/Guadalcanal")),
    ("Pacific/Guam", include_bytes!("../../../../third_party/tzdb/Pacific/Guam")),
    ("Pacific/Honolulu", include_bytes!("../../../../third_party/tzdb/Pacific/Honolulu")),
    ("Pacific/Johnston", include_bytes!("../../../../third_party/tzdb/Pacific/Johnston")),
    ("Pacific/Kanton", include_bytes!("../../../../third_party/tzdb/Pacific/Kanton")),
    ("Pacific/Kiritimati", include_bytes!("../../../../third_party/tzdb/Pacific/Kiritimati")),
    ("Pacific/Kosrae", include_bytes!("../../../../third_party/tzdb/Pacific/Kosrae")),
    ("Pacific/Kwajalein", include_bytes!("../../../../third_party/tzdb/Pacific/Kwajalein")),
    ("Pacific/Majuro", include_bytes!("../../../../third_party/tzdb/Pacific/Majuro")),
    ("Pacific/Marquesas", include_bytes!("../../../../third_party/tzdb/Pacific/Marquesas")),
    ("Pacific/Midway", include_bytes!("../../../../third_party/tzdb/Pacific/Midway")),
    ("Pacific/Nauru", include_bytes!("../../../../third_party/tzdb/Pacific/Nauru")),
    ("Pacific/Niue", include_bytes!("../../../../third_party/tzdb/Pacific/Niue")),
    ("Pacific/Norfolk", include_bytes!("../../../../third_party/tzdb/Pacific/Norfolk")),
    ("Pacific/Noumea", include_bytes!("../../../../third_party/tzdb/Pacific/Noumea")),
    ("Pacific/Pago_Pago", include_bytes!("../../../../third_party/tzdb/Pacific/Pago_Pago")),
    ("Pacific/Palau", include_bytes!("../../../../third_party/tzdb/Pacific/Palau")),
    ("Pacific/Pitcairn", include_bytes!("../../../../third_party/tzdb/Pacific/Pitcairn")),
    ("Pacific/Pohnpei", include_bytes!("../../../../third_party/tzdb/Pacific/Pohnpei")),
    (
        "Pacific/Port_Moresby",
        include_bytes!("../../../../third_party/tzdb/Pacific/Port_Moresby"),
    ),
    ("Pacific/Rarotonga", include_bytes!("../../../../third_party/tzdb/Pacific/Rarotonga")),
    ("Pacific/Saipan", include_bytes!("../../../../third_party/tzdb/Pacific/Saipan")),
    ("Pacific/Samoa", include_bytes!("../../../../third_party/tzdb/Pacific/Samoa")),
    ("Pacific/Tahiti", include_bytes!("../../../../third_party/tzdb/Pacific/Tahiti")),
    ("Pacific/Tarawa", include_bytes!("../../../../third_party/tzdb/Pacific/Tarawa")),
    ("Pacific/Tongatapu", include_bytes!("../../../../third_party/tzdb/Pacific/Tongatapu")),
    ("Pacific/Wake", include_bytes!("../../../../third_party/tzdb/Pacific/Wake")),
    ("Pacific/Wallis", include_bytes!("../../../../third_party/tzdb/Pacific/Wallis")),
    ("Pacific/Yap", include_bytes!("../../../../third_party/tzdb/Pacific/Yap")),
    ("UTC", include_bytes!("../../../../third_party/tzdb/UTC")),
    ("posixrules", include_bytes!("../../../../third_party/tzdb/posixrules")),
];

const LEGACY_ALIASES: &[(&str, &str)] = &[
    ("Africa/Accra", "Africa/Abidjan"),
    ("Africa/Addis_Ababa", "Africa/Nairobi"),
    ("Africa/Asmara", "Africa/Nairobi"),
    ("Africa/Asmera", "Africa/Nairobi"),
    ("Africa/Bamako", "Africa/Abidjan"),
    ("Africa/Bangui", "Africa/Lagos"),
    ("Africa/Banjul", "Africa/Abidjan"),
    ("Africa/Blantyre", "Africa/Maputo"),
    ("Africa/Brazzaville", "Africa/Lagos"),
    ("Africa/Bujumbura", "Africa/Maputo"),
    ("Africa/Conakry", "Africa/Abidjan"),
    ("Africa/Dakar", "Africa/Abidjan"),
    ("Africa/Dar_es_Salaam", "Africa/Nairobi"),
    ("Africa/Djibouti", "Africa/Nairobi"),
    ("Africa/Douala", "Africa/Lagos"),
    ("Africa/Freetown", "Africa/Abidjan"),
    ("Africa/Gaborone", "Africa/Maputo"),
    ("Africa/Harare", "Africa/Maputo"),
    ("Africa/Kampala", "Africa/Nairobi"),
    ("Africa/Kigali", "Africa/Maputo"),
    ("Africa/Kinshasa", "Africa/Lagos"),
    ("Africa/Libreville", "Africa/Lagos"),
    ("Africa/Lome", "Africa/Abidjan"),
    ("Africa/Luanda", "Africa/Lagos"),
    ("Africa/Lubumbashi", "Africa/Maputo"),
    ("Africa/Lusaka", "Africa/Maputo"),
    ("Africa/Malabo", "Africa/Lagos"),
    ("Africa/Maseru", "Africa/Johannesburg"),
    ("Africa/Mbabane", "Africa/Johannesburg"),
    ("Africa/Mogadishu", "Africa/Nairobi"),
    ("Africa/Niamey", "Africa/Lagos"),
    ("Africa/Nouakchott", "Africa/Abidjan"),
    ("Africa/Ouagadougou", "Africa/Abidjan"),
    ("Africa/Porto-Novo", "Africa/Lagos"),
    ("Africa/Timbuktu", "Africa/Abidjan"),
    ("America/Anguilla", "America/Puerto_Rico"),
    ("America/Antigua", "America/Puerto_Rico"),
    ("America/Argentina/ComodRivadavia", "America/Argentina/Catamarca"),
    ("America/Aruba", "America/Puerto_Rico"),
    ("America/Atikokan", "America/Panama"),
    ("America/Atka", "America/Adak"),
    ("America/Blanc-Sablon", "America/Puerto_Rico"),
    ("America/Buenos_Aires", "America/Argentina/Buenos_Aires"),
    ("America/Catamarca", "America/Argentina/Catamarca"),
    ("America/Cayman", "America/Panama"),
    ("America/Coral_Harbour", "America/Panama"),
    ("America/Cordoba", "America/Argentina/Cordoba"),
    ("America/Creston", "America/Phoenix"),
    ("America/Curacao", "America/Puerto_Rico"),
    ("America/Dominica", "America/Puerto_Rico"),
    ("America/Ensenada", "America/Tijuana"),
    ("America/Fort_Wayne", "America/Indiana/Indianapolis"),
    ("America/Godthab", "America/Nuuk"),
    ("America/Grenada", "America/Puerto_Rico"),
    ("America/Guadeloupe", "America/Puerto_Rico"),
    ("America/Indianapolis", "America/Indiana/Indianapolis"),
    ("America/Jujuy", "America/Argentina/Jujuy"),
    ("America/Knox_IN", "America/Indiana/Knox"),
    ("America/Kralendijk", "America/Puerto_Rico"),
    ("America/Louisville", "America/Kentucky/Louisville"),
    ("America/Lower_Princes", "America/Puerto_Rico"),
    ("America/Marigot", "America/Puerto_Rico"),
    ("America/Mendoza", "America/Argentina/Mendoza"),
    ("America/Montreal", "America/Toronto"),
    ("America/Montserrat", "America/Puerto_Rico"),
    ("America/Nassau", "America/Toronto"),
    ("America/Nipigon", "America/Toronto"),
    ("America/Pangnirtung", "America/Iqaluit"),
    ("America/Port_of_Spain", "America/Puerto_Rico"),
    ("America/Porto_Acre", "America/Rio_Branco"),
    ("America/Rainy_River", "America/Winnipeg"),
    ("America/Rosario", "America/Argentina/Cordoba"),
    ("America/Santa_Isabel", "America/Tijuana"),
    ("America/Shiprock", "America/Denver"),
    ("America/St_Barthelemy", "America/Puerto_Rico"),
    ("America/St_Kitts", "America/Puerto_Rico"),
    ("America/St_Lucia", "America/Puerto_Rico"),
    ("America/St_Thomas", "America/Puerto_Rico"),
    ("America/St_Vincent", "America/Puerto_Rico"),
    ("America/Thunder_Bay", "America/Toronto"),
    ("America/Tortola", "America/Puerto_Rico"),
    ("America/Virgin", "America/Puerto_Rico"),
    ("America/Yellowknife", "America/Edmonton"),
    ("Antarctica/DumontDUrville", "Pacific/Port_Moresby"),
    ("Antarctica/McMurdo", "Pacific/Auckland"),
    ("Antarctica/South_Pole", "Pacific/Auckland"),
    ("Antarctica/Syowa", "Asia/Riyadh"),
    ("Arctic/Longyearbyen", "Europe/Berlin"),
    ("Asia/Aden", "Asia/Riyadh"),
    ("Asia/Ashkhabad", "Asia/Ashgabat"),
    ("Asia/Bahrain", "Asia/Qatar"),
    ("Asia/Brunei", "Asia/Kuching"),
    ("Asia/Calcutta", "Asia/Kolkata"),
    ("Asia/Choibalsan", "Asia/Ulaanbaatar"),
    ("Asia/Chongqing", "Asia/Shanghai"),
    ("Asia/Chungking", "Asia/Shanghai"),
    ("Asia/Dacca", "Asia/Dhaka"),
    ("Asia/Harbin", "Asia/Shanghai"),
    ("Asia/Istanbul", "Europe/Istanbul"),
    ("Asia/Kashgar", "Asia/Urumqi"),
    ("Asia/Katmandu", "Asia/Kathmandu"),
    ("Asia/Kuala_Lumpur", "Asia/Singapore"),
    ("Asia/Kuwait", "Asia/Riyadh"),
    ("Asia/Macao", "Asia/Macau"),
    ("Asia/Muscat", "Asia/Dubai"),
    ("Asia/Phnom_Penh", "Asia/Bangkok"),
    ("Asia/Rangoon", "Asia/Yangon"),
    ("Asia/Saigon", "Asia/Ho_Chi_Minh"),
    ("Asia/Tel_Aviv", "Asia/Jerusalem"),
    ("Asia/Thimbu", "Asia/Thimphu"),
    ("Asia/Ujung_Pandang", "Asia/Makassar"),
    ("Asia/Ulan_Bator", "Asia/Ulaanbaatar"),
    ("Asia/Vientiane", "Asia/Bangkok"),
    ("Atlantic/Faeroe", "Atlantic/Faroe"),
    ("Atlantic/Jan_Mayen", "Europe/Berlin"),
    ("Atlantic/Reykjavik", "Africa/Abidjan"),
    ("Atlantic/St_Helena", "Africa/Abidjan"),
    ("Australia/ACT", "Australia/Sydney"),
    ("Australia/Canberra", "Australia/Sydney"),
    ("Australia/Currie", "Australia/Hobart"),
    ("Australia/LHI", "Australia/Lord_Howe"),
    ("Australia/NSW", "Australia/Sydney"),
    ("Australia/North", "Australia/Darwin"),
    ("Australia/Queensland", "Australia/Brisbane"),
    ("Australia/South", "Australia/Adelaide"),
    ("Australia/Tasmania", "Australia/Hobart"),
    ("Australia/Victoria", "Australia/Melbourne"),
    ("Australia/West", "Australia/Perth"),
    ("Australia/Yancowinna", "Australia/Broken_Hill"),
    ("Brazil/Acre", "America/Rio_Branco"),
    ("Brazil/DeNoronha", "America/Noronha"),
    ("Brazil/East", "America/Sao_Paulo"),
    ("Brazil/West", "America/Manaus"),
    ("CET", "Europe/Brussels"),
    ("CST6CDT", "America/Chicago"),
    ("Canada/Atlantic", "America/Halifax"),
    ("Canada/Central", "America/Winnipeg"),
    ("Canada/Eastern", "America/Toronto"),
    ("Canada/Mountain", "America/Edmonton"),
    ("Canada/Newfoundland", "America/St_Johns"),
    ("Canada/Pacific", "America/Vancouver"),
    ("Canada/Saskatchewan", "America/Regina"),
    ("Canada/Yukon", "America/Whitehorse"),
    ("Chile/Continental", "America/Santiago"),
    ("Chile/EasterIsland", "Pacific/Easter"),
    ("Cuba", "America/Havana"),
    ("EET", "Europe/Athens"),
    ("EST", "America/Panama"),
    ("EST5EDT", "America/New_York"),
    ("Egypt", "Africa/Cairo"),
    ("Eire", "Europe/Dublin"),
    ("Etc/GMT+0", "Etc/GMT"),
    ("Etc/GMT-0", "Etc/GMT"),
    ("Etc/GMT0", "Etc/GMT"),
    ("Etc/Greenwich", "Etc/GMT"),
    ("Etc/Ignored", "Etc/UTC"),
    ("Etc/UCT", "Etc/UTC"),
    ("Etc/Universal", "Etc/UTC"),
    ("Etc/Zulu", "Etc/UTC"),
    ("Europe/Amsterdam", "Europe/Brussels"),
    ("Europe/Belfast", "Europe/London"),
    ("Europe/Bratislava", "Europe/Prague"),
    ("Europe/Busingen", "Europe/Zurich"),
    ("Europe/Copenhagen", "Europe/Berlin"),
    ("Europe/Guernsey", "Europe/London"),
    ("Europe/Isle_of_Man", "Europe/London"),
    ("Europe/Jersey", "Europe/London"),
    ("Europe/Kiev", "Europe/Kyiv"),
    ("Europe/Ljubljana", "Europe/Belgrade"),
    ("Europe/Luxembourg", "Europe/Brussels"),
    ("Europe/Mariehamn", "Europe/Helsinki"),
    ("Europe/Monaco", "Europe/Paris"),
    ("Europe/Nicosia", "Asia/Nicosia"),
    ("Europe/Oslo", "Europe/Berlin"),
    ("Europe/Podgorica", "Europe/Belgrade"),
    ("Europe/San_Marino", "Europe/Rome"),
    ("Europe/Sarajevo", "Europe/Belgrade"),
    ("Europe/Skopje", "Europe/Belgrade"),
    ("Europe/Stockholm", "Europe/Berlin"),
    ("Europe/Tiraspol", "Europe/Chisinau"),
    ("Europe/Uzhgorod", "Europe/Kyiv"),
    ("Europe/Vaduz", "Europe/Zurich"),
    ("Europe/Vatican", "Europe/Rome"),
    ("Europe/Zagreb", "Europe/Belgrade"),
    ("Europe/Zaporozhye", "Europe/Kyiv"),
    ("GB", "Europe/London"),
    ("GB-Eire", "Europe/London"),
    ("GMT+0", "Etc/GMT"),
    ("GMT-0", "Etc/GMT"),
    ("GMT0", "Etc/GMT"),
    ("Greenwich", "Etc/GMT"),
    ("HST", "Pacific/Honolulu"),
    ("Hongkong", "Asia/Hong_Kong"),
    ("Iceland", "Africa/Abidjan"),
    ("Indian/Antananarivo", "Africa/Nairobi"),
    ("Indian/Christmas", "Asia/Bangkok"),
    ("Indian/Cocos", "Asia/Yangon"),
    ("Indian/Comoro", "Africa/Nairobi"),
    ("Indian/Kerguelen", "Indian/Maldives"),
    ("Indian/Mahe", "Asia/Dubai"),
    ("Indian/Mayotte", "Africa/Nairobi"),
    ("Indian/Reunion", "Asia/Dubai"),
    ("Iran", "Asia/Tehran"),
    ("Israel", "Asia/Jerusalem"),
    ("Jamaica", "America/Jamaica"),
    ("Japan", "Asia/Tokyo"),
    ("Kwajalein", "Pacific/Kwajalein"),
    ("Libya", "Africa/Tripoli"),
    ("MET", "Europe/Brussels"),
    ("MST", "America/Phoenix"),
    ("MST7MDT", "America/Denver"),
    ("Mexico/BajaNorte", "America/Tijuana"),
    ("Mexico/BajaSur", "America/Mazatlan"),
    ("Mexico/General", "America/Mexico_City"),
    ("NZ", "Pacific/Auckland"),
    ("NZ-CHAT", "Pacific/Chatham"),
    ("Navajo", "America/Denver"),
    ("PRC", "Asia/Shanghai"),
    ("PST8PDT", "America/Los_Angeles"),
    ("Pacific/Chuuk", "Pacific/Port_Moresby"),
    ("Pacific/Enderbury", "Pacific/Kanton"),
    ("Pacific/Funafuti", "Pacific/Tarawa"),
    ("Pacific/Johnston", "Pacific/Honolulu"),
    ("Pacific/Majuro", "Pacific/Tarawa"),
    ("Pacific/Midway", "Pacific/Pago_Pago"),
    ("Pacific/Pohnpei", "Pacific/Guadalcanal"),
    ("Pacific/Ponape", "Pacific/Guadalcanal"),
    ("Pacific/Saipan", "Pacific/Guam"),
    ("Pacific/Samoa", "Pacific/Pago_Pago"),
    ("Pacific/Truk", "Pacific/Port_Moresby"),
    ("Pacific/Wake", "Pacific/Tarawa"),
    ("Pacific/Wallis", "Pacific/Tarawa"),
    ("Pacific/Yap", "Pacific/Port_Moresby"),
    ("Poland", "Europe/Warsaw"),
    ("Portugal", "Europe/Lisbon"),
    ("ROC", "Asia/Taipei"),
    ("ROK", "Asia/Seoul"),
    ("Singapore", "Asia/Singapore"),
    ("Turkey", "Europe/Istanbul"),
    ("UCT", "Etc/UTC"),
    ("US/Alaska", "America/Anchorage"),
    ("US/Aleutian", "America/Adak"),
    ("US/Arizona", "America/Phoenix"),
    ("US/Central", "America/Chicago"),
    ("US/East-Indiana", "America/Indiana/Indianapolis"),
    ("US/Eastern", "America/New_York"),
    ("US/Hawaii", "Pacific/Honolulu"),
    ("US/Indiana-Starke", "America/Indiana/Knox"),
    ("US/Michigan", "America/Detroit"),
    ("US/Mountain", "America/Denver"),
    ("US/Pacific", "America/Los_Angeles"),
    ("US/Samoa", "Pacific/Pago_Pago"),
    ("UTC", "Etc/UTC"),
    ("Universal", "Etc/UTC"),
    ("W-SU", "Europe/Moscow"),
    ("WET", "Europe/Lisbon"),
    ("Zulu", "Etc/UTC"),
];

/// 区表：首次使用时解析全部 vendored 区文件，按名排序供二分查找。
struct ZoneTable {
    zones: Vec<(&'static str, TzifZone)>,
}

static ZONE_TABLE: LazyLock<ZoneTable> = LazyLock::new(|| {
    let mut zones = ZONE_FILES
        .iter()
        .filter_map(|(name, bytes)| parse_tzif(bytes).ok().map(|z| (*name, z)))
        .collect::<Vec<_>>();
    zones.sort_by(|a, b| a.0.cmp(b.0));
    ZoneTable { zones }
});

/// 按 IANA 区名（含 legacy 别名）查区数据。
///
/// # 步骤
/// 1. 先解 legacy 别名（`LEGACY_ALIASES` 二分），未命中则原名。
/// 2. 在区表二分查找，命中返回其 `TzifZone`。
///
/// # 边界与前提
/// - 别名只解一层：别名目标是现行区名，不再二次解别名。
///
/// # 注意事项
/// - 首次调用触发全量解析（毫秒级），之后为纯二分。
pub(crate) fn tz_zone_data(name: &str) -> Option<&'static TzifZone> {
    let target = match LEGACY_ALIASES.binary_search_by(|a| a.0.cmp(name)) {
        Ok(i) => LEGACY_ALIASES[i].1,
        Err(_) => name,
    };
    match ZONE_TABLE.zones.binary_search_by(|z| z.0.cmp(target)) {
        Ok(i) => Some(&ZONE_TABLE.zones[i].1),
        Err(_) => None,
    }
}

/// 按 IANA 区名（含 legacy 别名）查规范区名。
///
/// # 步骤
/// 1. 先解 legacy 别名（`LEGACY_ALIASES` 二分），未命中则原名。
/// 2. 在区表二分查找，命中返回其规范名（`&'static str`）。
///
/// # 边界与前提
/// - 别名只解一层：别名目标是现行区名，不再二次解别名。
pub(crate) fn tz_canonical_name(name: &str) -> Option<&'static str> {
    let target = match LEGACY_ALIASES.binary_search_by(|a| a.0.cmp(name)) {
        Ok(i) => LEGACY_ALIASES[i].1,
        Err(_) => name,
    };
    ZONE_TABLE
        .zones
        .binary_search_by(|z| z.0.cmp(target))
        .ok()
        .map(|i| ZONE_TABLE.zones[i].0)
}

/// 查某区在 `epoch_s`（纪元秒）生效的 UTC 偏移（秒）。
///
/// # 步骤
/// 1. 查区数据，未命中返回 `None`。
/// 2. `partition_point` 找最后一个 `time <= epoch_s` 的 transition：
///    无（早于首个）取 `lmt_offset`，有取该 transition 的偏移。
///
/// # 边界与前提
/// - 早于首个 transition 取 LMT；晚于末个 transition 取末偏移（`last_offset`
///   与末次 transition 一致，故 `partition_point` 已覆盖）。
fn tz_offset_seconds_of(z: &TzifZone, epoch_s: i64) -> i64 {
    if z.transitions.is_empty() {
        return i64::from(z.last_offset);
    }
    let idx = z.transitions.partition_point(|t| t.0 <= epoch_s);
    if idx == 0 {
        i64::from(z.lmt_offset)
    } else if idx == z.transitions.len() {
        i64::from(z.last_offset)
    } else {
        i64::from(z.transitions[idx - 1].1)
    }
}

/// 查某区在 `epoch_s`（纪元秒）生效的 UTC 偏移（秒）。
pub(crate) fn tz_offset_seconds(zone: &str, epoch_s: i64) -> Option<i64> {
    tz_zone_data(zone).map(|z| tz_offset_seconds_of(z, epoch_s))
}

/// GetNamedTimeZoneEpochNanoseconds（规范 14.6.3，宿主定义）：墙历时刻的全部候选纪元。
///
/// # 步骤
/// 1. 取墙历（按 UTC 解释）相邻的三个区间偏移：所在区间、下一 transition 后、
///    上一 transition 前。
/// 2. 对每个偏移 o 构造候选 e = wall - o；校验 e 落在 o 的区间内
///    （offset_at(e) == o），成立才收。
/// 3. 去重升序返回（重叠 = 2 个、间隙 = 0 个、无歧义 = 1 个）。
///
/// # 边界与前提
/// - 区表未命中返回 `None`（调用方决定错误）。
/// - 偏移量域 ±24 小时，有效候选必在墙历（按 UTC 解释）±24 小时内，
///   三个相邻区间覆盖全部候选。
pub(crate) fn named_zone_possible_epoch_ns(zone: &str, wall_ns: i128) -> Option<Vec<i128>> {
    let z = tz_zone_data(zone)?;
    let wall_s = wall_ns.div_euclid(1_000_000_000) as i64;
    let idx = z.transitions.partition_point(|t| t.0 <= wall_s);
    let current = if idx == 0 { z.lmt_offset } else { z.transitions[idx - 1].1 };
    let mut candidates = Vec::with_capacity(3);
    let mut consider = |offset: i32| {
        let candidate = wall_ns - i128::from(offset) * 1_000_000_000;
        if tz_offset_seconds_of(z, wall_s - i64::from(offset)) == i64::from(offset) {
            candidates.push(candidate);
        }
    };
    consider(current);
    if idx < z.transitions.len() {
        consider(z.transitions[idx].1);
    }
    if idx > 0 {
        let previous = if idx - 1 == 0 { z.lmt_offset } else { z.transitions[idx - 2].1 };
        consider(previous);
    }
    candidates.sort_unstable();
    candidates.dedup();
    Some(candidates)
}

/// 找 0 候选墙历所落间隙的 transition，返回间隙前后偏移（秒）。
///
/// # 步骤
/// 1. 墙历（按 UTC 解释）在 transition 前区间：查下一 transition 是否正偏移跳变
///    且墙历落间隙 (t + o_before, t + o_after)。
/// 2. 墙历在 transition 后区间：查上一 transition 同条件。
///
/// # 边界与前提
/// - 区表未命中或墙历不落在任何间隙内返回 `None`（调用方决定错误）。
pub(crate) fn tz_gap_offsets(zone: &str, wall_ns: i128) -> Option<(i64, i64)> {
    let z = tz_zone_data(zone)?;
    let wall_s = wall_ns.div_euclid(1_000_000_000) as i64;
    let idx = z.transitions.partition_point(|t| t.0 <= wall_s);
    // 墙历在 transition 前区间：间隙由下一 transition 产生。
    // 间隙起点（墙历 = t + o_before）候选集为空，计入间隙。
    if idx < z.transitions.len() {
        let (t, offset_after) = z.transitions[idx];
        let offset_before = if idx == 0 { z.lmt_offset } else { z.transitions[idx - 1].1 };
        if offset_after > offset_before
            && wall_ns >= i128::from(t) * 1_000_000_000 + i128::from(offset_before) * 1_000_000_000
            && wall_ns < i128::from(t) * 1_000_000_000 + i128::from(offset_after) * 1_000_000_000
        {
            return Some((i64::from(offset_before), i64::from(offset_after)));
        }
    }
    // 墙历在 transition 后区间：间隙由上一 transition 产生。
    if idx > 0 {
        let (t, offset_after) = z.transitions[idx - 1];
        let offset_before = if idx - 1 == 0 { z.lmt_offset } else { z.transitions[idx - 2].1 };
        if offset_after > offset_before
            && wall_ns >= i128::from(t) * 1_000_000_000 + i128::from(offset_before) * 1_000_000_000
            && wall_ns < i128::from(t) * 1_000_000_000 + i128::from(offset_after) * 1_000_000_000
        {
            return Some((i64::from(offset_before), i64::from(offset_after)));
        }
    }
    None
}

/// 统一时区偏移查找（epoch 依赖）：固定偏移区（数值偏移串）走快速路径恒返常量，
/// IANA 区走 transition 表二分。
///
/// # 步骤
/// 1. 数值偏移串（±HH:MM / ±HH / ±HHMM / ±HH:MM:SS）直接解析为秒返回。
/// 2. 未命中走 `tz_offset_seconds`（IANA 区，含 legacy 别名）。
///
/// # 边界与前提
/// - 固定偏移区是 zone-aware 路径的退化情形：偏移与 epoch 无关，恒返回常量。
/// - 非法区名（非数值偏移串且不在区表）返回 `None`。
pub(crate) fn zone_offset_seconds(zone: &str, epoch_s: i64) -> Option<i64> {
    if let Some(offset) = fixed_offset_seconds(zone) {
        return Some(offset);
    }
    tz_offset_seconds(zone, epoch_s)
}

/// 判断 `name` 是否为可接受的 IANA 区名（含 legacy 别名）。
#[allow(dead_code)]
pub(crate) fn tz_is_iana_zone(name: &str) -> bool {
    tz_zone_data(name).is_some()
}

/// 查 `epoch_s` 之后（严格大于）的下一个 transition 纪元秒。
#[allow(dead_code)]
pub(crate) fn tz_next_transition(zone: &str, epoch_s: i64) -> Option<i64> {
    let z = tz_zone_data(zone)?;
    z.transitions.iter().find(|t| t.0 > epoch_s).map(|t| t.0)
}

/// 查 `epoch_s` 之前（严格小于）的上一个 transition 纪元秒。
#[allow(dead_code)]
pub(crate) fn tz_prev_transition(zone: &str, epoch_s: i64) -> Option<i64> {
    let z = tz_zone_data(zone)?;
    z.transitions.iter().rev().find(|t| t.0 < epoch_s).map(|t| t.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 两个区名是否解析到同一份区数据（指针恒等）。
    fn same_zone(a: &str, b: &str) -> bool {
        match (tz_zone_data(a), tz_zone_data(b)) {
            (Some(x), Some(y)) => std::ptr::eq(x as *const _, y as *const _),
            (None, None) => true,
            _ => false,
        }
    }

    #[test]
    fn new_york_1883_lmt_transition() {
        // 1883 LMT transition（最早 transition，1970 年前，亚分钟 LMT -04:56:02）。
        let z = tz_zone_data("America/New_York").unwrap();
        assert_eq!(z.transitions[0].0, -2717650800);
        assert_eq!(z.lmt_offset, -17762);
        assert_eq!(tz_offset_seconds("America/New_York", -2717650800 - 1), Some(-17762));
        assert_eq!(tz_offset_seconds("America/New_York", -2717650800), Some(-18000));
    }

    #[test]
    fn monrovia_sub_minute_offset() {
        // 亚分钟偏移 -00:44:30；1970-01-01T12:00-00:44:30 对应纪元 45870 秒。
        assert_eq!(tz_offset_seconds("Africa/Monrovia", 45870), Some(-2670));
        assert_eq!(tz_offset_seconds("Africa/Monrovia", 0), Some(-2670));
    }

    #[test]
    fn niue_sub_minute_lmt_transition() {
        // 1952-10-15T23:59:59-11:19:40（亚分钟 LMT -11:19:40 → -11:20:00）。
        assert_eq!(tz_offset_seconds("Pacific/Niue", -543069620 - 1), Some(-40780));
        assert_eq!(tz_offset_seconds("Pacific/Niue", -543069620), Some(-40800));
    }

    #[test]
    fn apia_2011_skip_day() {
        // 2011-12-30 跳变（24 小时日）：-10:00 → +14:00。
        assert_eq!(tz_offset_seconds("Pacific/Apia", 1325239200 - 1), Some(-36000));
        assert_eq!(tz_offset_seconds("Pacific/Apia", 1325239200), Some(50400));
    }

    #[test]
    fn vancouver_2000_dst_start() {
        // 2000-04-02 DST 开始（23 小时日）：-08:00 → -07:00。
        assert_eq!(tz_offset_seconds("America/Vancouver", 954669600 - 1), Some(-28800));
        assert_eq!(tz_offset_seconds("America/Vancouver", 954669600), Some(-25200));
    }

    #[test]
    fn lord_howe_half_hour_dst() {
        // 半小时 DST：+10:30 → +11:00。
        assert_eq!(tz_offset_seconds("Australia/Lord_Howe", 941297400 - 1), Some(37800));
        assert_eq!(tz_offset_seconds("Australia/Lord_Howe", 941297400), Some(39600));
    }

    #[test]
    fn casey_2010_revert_to_utc8() {
        // 2010-03-05 02:00（+11:00 当地）一次性回到 +08:00，源表为一次性日期而非循环规则。
        assert_eq!(tz_offset_seconds("Antarctica/Casey", 1267714800 - 1), Some(39600));
        assert_eq!(tz_offset_seconds("Antarctica/Casey", 1267714800), Some(28800));
    }

    #[test]
    fn riyadh_and_paris_lmt() {
        // 1850 年 Riyadh LMT +03:06:52；1800 年 Paris LMT +00:09:21。
        assert_eq!(tz_offset_seconds("Asia/Riyadh", -3786825600), Some(11212));
        assert_eq!(tz_offset_seconds("Europe/Paris", -5364662400), Some(561));
    }

    #[test]
    fn legacy_alias_hits() {
        // 语料钉住的 legacy 区名命中别名表，解析到同一区数据（指针恒等）。
        assert!(same_zone("Asia/Calcutta", "Asia/Kolkata"));
        assert!(same_zone("Asia/Katmandu", "Asia/Kathmandu"));
        assert!(same_zone("Etc/Ignored", "Etc/UTC"));
        assert!(tz_is_iana_zone("Asia/Calcutta"));
        assert!(tz_is_iana_zone("Etc/Ignored"));
    }

    #[test]
    fn invalid_zone_returns_none() {
        // 非法区名返回 None。
        assert!(tz_zone_data("Mars/Olympus_Mons").is_none());
        assert_eq!(tz_offset_seconds("Not/AZone", 0), None);
        assert!(!tz_is_iana_zone("Not/AZone"));
        assert_eq!(tz_next_transition("Not/AZone", 0), None);
        assert_eq!(tz_prev_transition("Not/AZone", 0), None);
    }

    #[test]
    fn next_and_prev_transition() {
        // transition 前后边界。
        assert_eq!(tz_next_transition("Pacific/Apia", 1325239200 - 1), Some(1325239200));
        assert_eq!(tz_prev_transition("Pacific/Apia", 1325239200), Some(1316872800));
        assert_eq!(tz_prev_transition("Pacific/Apia", -2445424384), None);
        assert_eq!(tz_next_transition("Etc/UTC", 0), None);
    }

    #[test]
    fn parse_tzif_version_header() {
        // 版本头：魔数 / 版本字节 / 截断 / 非法版本。
        assert!(matches!(parse_tzif(b"TZif"), Err(TzifError::Truncated)));
        let mut bad_magic = b"XZif2".to_vec();
        bad_magic.extend_from_slice(&[0u8; 39]);
        assert!(matches!(parse_tzif(&bad_magic), Err(TzifError::BadMagic)));
        let mut bad_ver = b"TZif".to_vec();
        bad_ver.push(b'9');
        bad_ver.extend_from_slice(&[0u8; 40]);
        assert!(matches!(parse_tzif(&bad_ver), Err(TzifError::BadVersion)));
    }
}
