//! Supported profile identities. SMEM IDs are kernel ABI IDs, not QNN IDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Soc {
    Gen1,
    Gen1Plus,
    Gen2,
    Gen3,
    Elite,
    Elite5,
    Gen5,
    Dimensity8350,
    Dimensity8400,
    Dimensity9300Plus,
    Dimensity9400Plus,
    XringO3,
}
pub const SUPPORTED: [Soc; 12] = [
    Soc::Gen1,
    Soc::Gen1Plus,
    Soc::Gen2,
    Soc::Gen3,
    Soc::Elite,
    Soc::Elite5,
    Soc::Gen5,
    Soc::Dimensity8350,
    Soc::Dimensity8400,
    Soc::Dimensity9300Plus,
    Soc::Dimensity9400Plus,
    Soc::XringO3,
];
impl Soc {
    pub fn id(self) -> &'static str {
        match self {
            Self::Gen1 => "SM8450",
            Self::Gen1Plus => "SM8475",
            Self::Gen2 => "SM8550",
            Self::Gen3 => "SM8650",
            Self::Elite => "SM8750",
            Self::Elite5 => "SM8850",
            Self::Gen5 => "SM8845",
            Self::Dimensity8350 => "DIMENSITY8350",
            Self::Dimensity8400 => "DIMENSITY8400",
            Self::Dimensity9300Plus => "MT6989",
            Self::Dimensity9400Plus => "DIMENSITY9400PLUS",
            Self::XringO3 => "XRING_O3",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Gen1 => "Snapdragon 8 Gen 1",
            Self::Gen1Plus => "Snapdragon 8+ Gen 1",
            Self::Gen2 => "Snapdragon 8 Gen 2",
            Self::Gen3 => "Snapdragon 8 Gen 3",
            Self::Elite => "Snapdragon 8 Elite",
            Self::Elite5 => "Snapdragon 8 Elite Gen 5",
            Self::Gen5 => "Snapdragon 8 Gen 5",
            Self::Dimensity8350 => "MediaTek Dimensity 8350",
            Self::Dimensity8400 => "MediaTek Dimensity 8400",
            Self::Dimensity9300Plus => "MediaTek Dimensity 9300+",
            Self::Dimensity9400Plus => "MediaTek Dimensity 9400+",
            Self::XringO3 => "Xiaomi Xring O3",
        }
    }
    pub fn file(self) -> &'static str {
        match self {
            Self::Gen1 => "SM8450.json",
            Self::Gen1Plus => "SM8475.json",
            Self::Gen2 => "SM8550.json",
            Self::Gen3 => "SM8650.json",
            Self::Elite => "SM8750.json",
            Self::Elite5 => "SM8850.json",
            Self::Gen5 => "SM8845.json",
            Self::Dimensity8350 => "DIMENSITY8350.json",
            Self::Dimensity8400 => "DIMENSITY8400.json",
            Self::Dimensity9300Plus => "MT6989.json",
            Self::Dimensity9400Plus => "DIMENSITY9400PLUS.json",
            Self::XringO3 => "XRING_O3.json",
        }
    }
    pub fn anchors(self) -> [i32; 4] {
        match self {
            Self::Gen1 | Self::Gen1Plus => [0, 4, 7, -1],
            Self::Gen2 => [0, 3, 7, -1],
            Self::Gen3 => [0, 2, 5, 7],
            Self::Elite | Self::Elite5 => [0, 6, -1, -1],
            // 骁龙 8 Gen 5（SM8845）：6 性能核 cpu0-5 + 2 超大核 cpu6-7，
            // 与 8 Elite 同为 2+6 Oryon 布局（真机采集：一加 PLR110）。
            Self::Gen5 => [0, 6, -1, -1],
            Self::Dimensity8350 | Self::Dimensity8400 => [0, 4, 7, -1],
            // 天玑 9300+（MT6989）全大核：4×A720 cpu0-3、3×X4 cpu4-6、
            // 1×X4 超大核 cpu7（真机采集：vivo V2417A）。
            Self::Dimensity9300Plus => [0, 4, 7, -1],
            Self::Dimensity9400Plus => [0, 4, 7, -1],
            // The supplied O3 dump numbers A715 first, X3 next, A510 last.
            Self::XringO3 => [4, 0, 1, -1],
        }
    }
    pub fn from_id(id: &str) -> Option<Self> {
        SUPPORTED
            .into_iter()
            .find(|soc| soc.id().eq_ignore_ascii_case(id))
    }
    pub fn elite(self) -> bool {
        matches!(self, Self::Elite | Self::Elite5)
    }
    /// Qualcomm 平台：内核的 WALT 调速器按窗口负载加频，重载游戏下比
    /// schedutil 提频更快；`auto` 档位在骁龙上优先选择 walt。
    pub fn qualcomm(self) -> bool {
        matches!(
            self,
            Self::Gen1
                | Self::Gen1Plus
                | Self::Gen2
                | Self::Gen3
                | Self::Elite
                | Self::Elite5
                | Self::Gen5
        )
    }
}
pub fn marketing_model(value: &str) -> Option<Soc> {
    let lower = value.to_ascii_lowercase().replace('+', "plus");
    let mut compact: String = lower.chars().filter(|c| c.is_alphanumeric()).collect();
    for manufacturer in [
        "qualcomm",
        "mediatek",
        "mtk",
        "xiaomi",
        "小米",
        "联发科",
        "高通",
    ] {
        if let Some(rest) = compact.strip_prefix(manufacturer) {
            compact = rest.into();
            break;
        }
    }
    // 纯营销限定词不改变芯片型号：领先版 / Leading Version / Mobile
    // Platform / for Galaxy 反复剥离至不动点（限定词最多两层嵌套）。
    // 注意只剥“领先版”：骁龙8至尊版领先版 → 骁龙8至尊版，而不是骁龙8。
    for _ in 0..3 {
        if let Some(rest) = compact
            .strip_suffix("mobileplatform")
            .or_else(|| compact.strip_suffix("forgalaxy"))
            .or_else(|| compact.strip_suffix("leadingversion"))
            .or_else(|| compact.strip_suffix("领先版"))
        {
            compact = rest.into();
        } else {
            break;
        }
    }
    match compact.as_str() {
        "snapdragon8gen1" => Some(Soc::Gen1),
        "snapdragon8plusgen1" => Some(Soc::Gen1Plus),
        "snapdragon8gen2" => Some(Soc::Gen2),
        "snapdragon8gen3" => Some(Soc::Gen3),
        "snapdragon8elite" | "骁龙8至尊版" => Some(Soc::Elite),
        "snapdragon8elitegen5" | "骁龙8至尊版5" => Some(Soc::Elite5),
        "snapdragon8gen5" => Some(Soc::Gen5),
        "dimensity8350" | "天玑8350" => Some(Soc::Dimensity8350),
        // 红米 K80 至尊版等 "Dimensity 8400-Ultra" 全大核布局与 8350 相同。
        "dimensity8400" | "dimensity8400ultra" | "天玑8400" | "天玑8400ultra" => {
            Some(Soc::Dimensity8400)
        }
        "dimensity9300plus" | "天玑9300plus" => Some(Soc::Dimensity9300Plus),
        "dimensity9400plus" | "天玑9400plus" => Some(Soc::Dimensity9400Plus),
        "xringo3" | "玄戒o3" => Some(Soc::XringO3),
        _ => None,
    }
}
pub fn kernel_id(value: u32) -> Option<Soc> {
    match value {
        457 | 480 | 482 => Some(Soc::Gen1),
        530 | 531 | 540 => Some(Soc::Gen1Plus),
        519 => Some(Soc::Gen2),
        557 => Some(Soc::Gen3),
        618 => Some(Soc::Elite),
        660 => Some(Soc::Elite5),
        // SM8845（骁龙 8 Gen 5）真机 soc_id：一加 PLR110 采集值 685。
        685 => Some(Soc::Gen5),
        _ => None,
    }
}
/// 解析 `sm8750` / `sm8750-ab` / `sm8750p` / `sm8750p-ayn` 这类型号 token：
/// 基础型号必须是 sm + 数字，后缀由连字符分隔的字母数字段组成（连字符
/// 不能连续）。例如一加 Pad 2 Pro 报告的 SM8750P（骁龙 8 至尊版领先版）
/// 按 SM8750 家族处理；SM8635、SM8650--AB 等属于 Unsupported。
pub enum SmToken {
    NotSm,
    Unsupported,
    Match(Soc),
}
pub fn sm_token(token: &str) -> SmToken {
    let Some(rest) = token.strip_prefix("sm") else {
        return SmToken::NotSm;
    };
    if !rest.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        return SmToken::NotSm;
    }
    let model_end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    let (model, tail) = rest.split_at(model_end);
    let tail = tail.strip_prefix('-').unwrap_or(tail);
    let valid_suffix = tail.is_empty()
        || tail
            .split('-')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric()));
    match Soc::from_id(&format!("sm{model}")) {
        Some(soc) if valid_suffix => SmToken::Match(soc),
        _ => SmToken::Unsupported,
    }
}
/// MTK 平台型号 → SoC：Redmi/小米 以平台码上报 SoC（如 Redmi dali 的
/// MT6991 = 天玑 9400+）。仅收录已确认的映射，未知平台码不猜测。
pub fn mtk_platform(value: &str) -> Option<Soc> {
    let lower = value.to_ascii_lowercase();
    let code = lower.strip_prefix("mt")?;
    if code.is_empty() || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match code {
        // vivo PD2417（天玑 9300+）以 MT6989 上报；红米 dali 以 MT6991 上报 9400+。
        "6989" => Some(Soc::Dimensity9300Plus),
        "6991" => Some(Soc::Dimensity9400Plus),
        _ => None,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn marketing(value: &str) -> Option<Soc> {
        marketing_model(value)
    }
    #[test]
    fn leading_version_and_ultra_bins_map_to_their_base_family() {
        assert_eq!(marketing("Snapdragon 8 Elite Leading Version"), Some(Soc::Elite));
        assert_eq!(marketing("骁龙8至尊版领先版"), Some(Soc::Elite));
        assert_eq!(marketing("骁龙8至尊版"), Some(Soc::Elite));
        assert_eq!(
            marketing("Snapdragon 8 Elite Gen 5 Leading Version"),
            Some(Soc::Elite5)
        );
        assert_eq!(marketing("Dimensity 8400-Ultra"), Some(Soc::Dimensity8400));
        assert_eq!(marketing("天玑8400-ultra"), Some(Soc::Dimensity8400));
        assert_eq!(marketing("天玑8400"), Some(Soc::Dimensity8400));
        assert_eq!(marketing("Dimensity 8400"), Some(Soc::Dimensity8400));
    }
    #[test]
    fn near_miss_families_stay_unsupported() {
        assert_eq!(marketing("Snapdragon 8 Elite 2"), None);
        assert_eq!(marketing("Snapdragon 8s Gen 3"), None);
        assert_eq!(marketing("Snapdragon 8s Gen 5"), None);
        assert_eq!(marketing("Dimensity 9300"), None);
        assert_eq!(marketing("Dimensity 9400"), None);
        assert_eq!(marketing("Dimensity 9400 Ultra"), None);
        assert_eq!(marketing("Dimensity 83500"), None);
        assert_eq!(marketing("Xring O1"), None);
        assert_eq!(marketing("骁龙8gen4"), None);
    }
    #[test]
    fn gen5_and_dimensity9300plus_marketing_names_are_exact() {
        assert_eq!(marketing("Snapdragon 8 Gen 5"), Some(Soc::Gen5));
        assert_eq!(marketing("Snapdragon 8 Gen 5 Mobile Platform"), Some(Soc::Gen5));
        assert_eq!(marketing("Dimensity 9300+"), Some(Soc::Dimensity9300Plus));
        assert_eq!(marketing("天玑9300+"), Some(Soc::Dimensity9300Plus));
        assert_eq!(marketing("MediaTek Dimensity 9300+"), Some(Soc::Dimensity9300Plus));
        assert_eq!(mtk_platform("mt6989"), Some(Soc::Dimensity9300Plus));
        assert_eq!(mtk_platform("mt69890"), None);
        assert_eq!(kernel_id(685), Some(Soc::Gen5));
    }
    #[test]
    fn sm_tokens_split_model_from_alphanumeric_suffixes() {
        assert!(matches!(sm_token("sm8750p"), SmToken::Match(Soc::Elite)));
        assert!(matches!(sm_token("sm8850ac"), SmToken::Match(Soc::Elite5)));
        assert!(matches!(sm_token("sm8750p-ayn"), SmToken::Match(Soc::Elite)));
        assert!(matches!(sm_token("sm8650"), SmToken::Match(Soc::Gen3)));
        assert!(matches!(sm_token("sm8650-ab"), SmToken::Match(Soc::Gen3)));
        assert!(matches!(sm_token("sm8635"), SmToken::Unsupported));
        assert!(matches!(sm_token("sm86500"), SmToken::Unsupported));
        assert!(matches!(sm_token("sm8650--ab"), SmToken::Unsupported));
        assert!(matches!(sm_token("qrd"), SmToken::NotSm));
        assert!(matches!(sm_token("smx"), SmToken::NotSm));
        assert!(matches!(sm_token("sd8750"), SmToken::NotSm));
        assert!(matches!(sm_token("sm"), SmToken::NotSm));
    }
    #[test]
    fn every_supported_soc_has_file_id_and_anchors() {
        for soc in SUPPORTED {
            assert_eq!(
                soc.file().get(..soc.id().len()),
                Some(&soc.id()[..soc.id().len()])
            );
            assert!(soc.anchors().iter().any(|anchor| *anchor >= 0));
        }
    }
}
