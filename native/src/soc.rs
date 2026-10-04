//! Supported profile identities. SMEM IDs are kernel ABI IDs, not QNN IDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Soc {
    Gen1,
    Gen1Plus,
    Gen2,
    Gen3,
    Elite,
    Elite5,
}
pub const SUPPORTED: [Soc; 6] = [
    Soc::Gen1,
    Soc::Gen1Plus,
    Soc::Gen2,
    Soc::Gen3,
    Soc::Elite,
    Soc::Elite5,
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
        }
    }
    pub fn anchors(self) -> [i32; 4] {
        match self {
            Self::Gen1 | Self::Gen1Plus => [0, 4, 7, -1],
            Self::Gen2 => [0, 3, 7, -1],
            Self::Gen3 => [0, 2, 5, 7],
            Self::Elite | Self::Elite5 => [0, 6, -1, -1],
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
}
pub fn marketing_model(value: &str) -> Option<Soc> {
    let lower = value.to_ascii_lowercase().replace('+', "plus");
    let mut compact: String = lower.chars().filter(char::is_ascii_alphanumeric).collect();
    if let Some(rest) = compact.strip_prefix("qualcomm") {
        compact = rest.into();
    }
    for _ in 0..2 {
        if let Some(rest) = compact
            .strip_suffix("mobileplatform")
            .or_else(|| compact.strip_suffix("forgalaxy"))
        {
            compact = rest.into();
        }
    }
    match compact.as_str() {
        "snapdragon8gen1" => Some(Soc::Gen1),
        "snapdragon8plusgen1" => Some(Soc::Gen1Plus),
        "snapdragon8gen2" => Some(Soc::Gen2),
        "snapdragon8gen3" => Some(Soc::Gen3),
        "snapdragon8elite" => Some(Soc::Elite),
        "snapdragon8elitegen5" => Some(Soc::Elite5),
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
        _ => None,
    }
}
