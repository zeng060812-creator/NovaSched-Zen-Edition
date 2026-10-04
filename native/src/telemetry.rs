use crate::util;

const SCENE_PACKAGE: &str = "com.omarea.vtools";

#[derive(Clone, Debug, Default)]
pub struct SceneTelemetry {
    pub scene_installed: bool,
    pub accessibility_enabled: bool,
    pub scene_process_running: bool,
    pub battery_power_watts: Option<f64>,
    pub battery_current: Option<i64>,
    pub battery_voltage: Option<i64>,
    pub battery_current_path: Option<String>,
    pub battery_voltage_path: Option<String>,
    pub battery_power_path: Option<String>,
    pub issue: Option<String>,
}

impl SceneTelemetry {
    pub fn diagnostic_text(&self) -> String {
        format!(
            "scene_installed={}\nscene_accessibility_enabled={}\nscene_process_running={}\nbattery_current={}\nbattery_voltage={}\nbattery_power_watts={}\nbattery_current_path={}\nbattery_voltage_path={}\nbattery_power_path={}\nreason={}\n",
            self.scene_installed,
            self.accessibility_enabled,
            self.scene_process_running,
            self.battery_current.map(|v| v.to_string()).unwrap_or_else(|| "unavailable".into()),
            self.battery_voltage.map(|v| v.to_string()).unwrap_or_else(|| "unavailable".into()),
            self.battery_power_watts.map(|v| format!("{v:.3}")).unwrap_or_else(|| "unavailable".into()),
            self.battery_current_path.as_deref().unwrap_or("unavailable"),
            self.battery_voltage_path.as_deref().unwrap_or("unavailable"),
            self.battery_power_path.as_deref().unwrap_or("unavailable"),
            self.issue.as_deref().unwrap_or("none"),
        )
    }
}

pub fn inspect_scene() -> SceneTelemetry {
    let mut data = SceneTelemetry {
        scene_installed: package_installed(),
        accessibility_enabled: accessibility_enabled(),
        scene_process_running: process_running(),
        ..Default::default()
    };
    let direct = read_first(&[
        "/sys/class/power_supply/battery/power_now",
        "/sys/class/power_supply/bms/power_now",
        "/sys/class/power_supply/main/power_now",
    ]);
    let mut direct_power_zero = false;
    if let Some((path, power)) = direct {
        data.battery_power_path = Some(path);
        direct_power_zero = power == 0;
        if !direct_power_zero {
            data.battery_power_watts = normalize_power(power);
        }
    }
    if let Some((path, current)) = read_first(&[
        "/sys/class/power_supply/battery/current_now",
        "/sys/class/power_supply/battery/current_avg",
        "/sys/class/power_supply/bms/current_now",
        "/sys/class/power_supply/main/current_now",
    ]) {
        data.battery_current_path = Some(path);
        data.battery_current = Some(current);
    }
    if let Some((path, voltage)) = read_first(&[
        "/sys/class/power_supply/battery/voltage_now",
        "/sys/class/power_supply/battery/voltage_avg",
        "/sys/class/power_supply/bms/voltage_now",
        "/sys/class/power_supply/main/voltage_now",
    ]) {
        data.battery_voltage_path = Some(path);
        data.battery_voltage = Some(voltage);
    }
    if data.battery_power_watts.is_none() {
        data.battery_power_watts = match (data.battery_current, data.battery_voltage) {
            (Some(current), Some(voltage)) => normalize_current_voltage(current, voltage),
            _ => None,
        };
    }
    data.issue = if !data.scene_installed {
        Some("未检测到 Scene 包 com.omarea.vtools".into())
    } else if !data.accessibility_enabled {
        Some("Scene 无障碍服务未启用；帧率/会话统计可能无法采集".into())
    } else if !data.scene_process_running {
        Some("Scene 进程当前未运行；打开 Scene 后再开始会话".into())
    } else if data.battery_power_watts.is_none() {
        Some("系统未暴露可读的电池 current/voltage/power 节点；NovaSched 不会伪造功耗".into())
    } else if direct_power_zero && data.battery_power_watts == Some(0.0) {
        Some("power_now 返回 0，电流×电压也为 0；可能是设备当前无负载，也可能是传感器未更新".into())
    } else {
        None
    };
    data
}

fn package_installed() -> bool {
    crate::scene::availability() == crate::scene::SceneAvailability::Available
}

fn accessibility_enabled() -> bool {
    util::query_command(
        "/system/bin/settings",
        &["get", "secure", "enabled_accessibility_services"],
        std::time::Duration::from_secs(2),
    )
    .map(|text| text.contains(SCENE_PACKAGE))
    .unwrap_or(false)
}

fn process_running() -> bool {
    crate::package_registry::process_running_at(std::path::Path::new("/proc"), SCENE_PACKAGE)
        .unwrap_or(false)
}

fn read_first(paths: &[&str]) -> Option<(String, i64)> {
    paths.iter().find_map(|path| {
        util::read_trimmed(path)
            .ok()
            .and_then(|value| value.parse::<i64>().ok())
            .map(|value| ((*path).to_string(), value))
    })
}

fn normalize_power(raw: i64) -> Option<f64> {
    // power_supply ABI specifies micro-watts, including low and zero readings.
    Some(raw.unsigned_abs() as f64 / 1_000_000.0)
}

fn normalize_current_voltage(current: i64, voltage: i64) -> Option<f64> {
    let current = current.unsigned_abs() as f64;
    let voltage = voltage.unsigned_abs() as f64;
    // Standard current/voltage nodes use micro-amps / micro-volts. Inferring
    // units from magnitude produces huge false power values at low current.
    Some(current * voltage / 1_000_000_000_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_micro_units() {
        assert_eq!(normalize_power(2_000_000), Some(2.0));
        assert_eq!(normalize_current_voltage(500_000, 4_000_000), Some(2.0));
    }
    #[test]
    fn low_current_keeps_the_same_abi_units() {
        assert_eq!(normalize_power(50_000), Some(0.05));
        assert_eq!(normalize_current_voltage(3_000, 4_000_000), Some(0.012));
        assert_eq!(normalize_current_voltage(-3_000, 4_000_000), Some(0.012));
        assert_eq!(normalize_current_voltage(0, 4_000_000), Some(0.0));
    }
}
