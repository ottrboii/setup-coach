//! Where Setup Coach keeps its files.

use std::path::PathBuf;

/// `%APPDATA%\SetupCoach` on Windows, `$XDG_CONFIG_HOME/SetupCoach` (or
/// `~/.config/SetupCoach`) elsewhere.
pub fn data_dir() -> PathBuf {
    if let Some(a) = std::env::var_os("APPDATA") {
        return PathBuf::from(a).join("SetupCoach");
    }
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(x).join("SetupCoach");
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config").join("SetupCoach")).unwrap_or_else(|| PathBuf::from("SetupCoach"))
}

/// `%APPDATA%\obs-studio` (Windows) or `~/.config/obs-studio`.
pub fn obs_dir() -> Option<PathBuf> {
    if let Some(a) = std::env::var_os("APPDATA") {
        return Some(PathBuf::from(a).join("obs-studio"));
    }
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("obs-studio"))
}

/// Where the installer puts our VST3 bundle.
pub fn vst3_path() -> Option<String> {
    #[cfg(windows)]
    {
        let base = std::env::var("CommonProgramFiles").unwrap_or_else(|_| r"C:\Program Files\Common Files".into());
        Some(format!(r"{base}\VST3\SetupCoachVoice.vst3"))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// UTC timestamp `YYYY-MM-DDTHH:MM:SSZ` without extra dependencies.
pub fn now_iso() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, (rem % 3600) / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    #[test]
    fn iso_format() {
        let s = super::now_iso();
        assert_eq!(s.len(), 20);
        assert!(s.starts_with("20"));
        assert!(s.ends_with('Z'));
    }
}
