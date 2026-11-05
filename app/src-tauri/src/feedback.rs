//! Testers' notes ("Tell us what's wrong"): saved locally as JSON lines in
//! `<data dir>\feedback.jsonl`. Never uploaded; the self-test zip already packs
//! the whole data folder, so the file travels with it.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "feedback.jsonl";
const MAX_TEXT_CHARS: usize = 8000;
const TAGS: [&str; 4] = ["confusing", "broken", "looks_wrong", "idea"];

/// What the UI sends.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FeedbackInput {
    pub text: String,
    #[serde(default)]
    pub route: String,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub mic_name: Option<String>,
    #[serde(default)]
    pub camera_name: Option<String>,
}

/// One saved line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FeedbackNote {
    pub at: String,
    pub route: String,
    pub tag: Option<String>,
    pub text: String,
    pub app_version: String,
    pub mic_name: Option<String>,
    pub camera_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FeedbackSaved {
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FeedbackExport {
    pub count: usize,
    /// Plain text of all notes, ready to paste into a message.
    pub text: String,
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE_NAME)
}

pub fn list(data_dir: &Path) -> Vec<FeedbackNote> {
    let Ok(s) = std::fs::read_to_string(path(data_dir)) else { return Vec::new() };
    s.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

pub fn save(data_dir: &Path, input: FeedbackInput, now_iso: &str, app_version: &str) -> Result<FeedbackSaved, String> {
    let text: String = input.text.trim().chars().take(MAX_TEXT_CHARS).collect();
    if text.is_empty() {
        return Err("Write a few words first, then press Save.".into());
    }
    let clean = |s: Option<String>, n: usize| s.map(|v| v.chars().take(n).collect::<String>()).filter(|v| !v.trim().is_empty());
    let note = FeedbackNote {
        at: now_iso.to_string(),
        route: input.route.chars().take(40).collect(),
        tag: input.tag.filter(|t| TAGS.contains(&t.as_str())),
        text,
        app_version: app_version.to_string(),
        mic_name: clean(input.mic_name, 120),
        camera_name: clean(input.camera_name, 120),
    };
    std::fs::create_dir_all(data_dir).map_err(|e| format!("Couldn't save your note ({e})."))?;
    let line = serde_json::to_string(&note).map_err(|e| e.to_string())?;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path(data_dir)).map_err(|e| format!("Couldn't save your note ({e})."))?;
    writeln!(f, "{line}").map_err(|e| format!("Couldn't save your note ({e})."))?;
    Ok(FeedbackSaved { count: list(data_dir).len() })
}

fn tag_label(t: &str) -> &'static str {
    match t {
        "confusing" => "Confusing",
        "broken" => "Broken",
        "looks_wrong" => "Looks wrong",
        "idea" => "Idea",
        _ => "Note",
    }
}

pub fn export(data_dir: &Path) -> FeedbackExport {
    let notes = list(data_dir);
    let mut text = String::new();
    for (i, n) in notes.iter().enumerate() {
        text.push_str(&format!("{}. [{}] {} - screen: {} - v{}", i + 1, n.tag.as_deref().map(tag_label).unwrap_or("Note"), n.at, if n.route.is_empty() { "?" } else { &n.route }, n.app_version));
        if let Some(m) = &n.mic_name {
            text.push_str(&format!(" - mic: {m}"));
        }
        if let Some(c) = &n.camera_name {
            text.push_str(&format!(" - camera: {c}"));
        }
        text.push_str(&format!("\n{}\n\n", n.text));
    }
    FeedbackExport { count: notes.len(), text }
}

/// Write the readable version to `out_dir`; returns the file path.
pub fn export_to_dir(data_dir: &Path, out_dir: &Path) -> Result<PathBuf, String> {
    let e = export(data_dir);
    if e.count == 0 {
        return Err("There are no saved notes yet.".into());
    }
    let p = out_dir.join("SetupCoach-notes.txt");
    std::fs::write(&p, e.text).map_err(|e| format!("Couldn't save the notes file ({e})."))?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sc-feedback-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn input(text: &str) -> FeedbackInput {
        FeedbackInput { text: text.into(), route: "mic".into(), tag: Some("broken".into()), mic_name: Some("Test Mic".into()), camera_name: None }
    }

    #[test]
    fn saves_lists_and_exports() {
        let d = tmp("a");
        assert!(save(&d, input("   "), "2026-11-05T10:00:00Z", "0.1.0").is_err());
        assert_eq!(save(&d, input(" it froze\nafter the test "), "2026-11-05T10:00:00Z", "0.1.0").unwrap().count, 1);
        let mut bad_tag = input("second");
        bad_tag.tag = Some("<script>".into());
        assert_eq!(save(&d, bad_tag, "2026-11-05T10:01:00Z", "0.1.0").unwrap().count, 2);
        let notes = list(&d);
        assert_eq!(notes[0].text, "it froze\nafter the test");
        assert_eq!(notes[0].tag.as_deref(), Some("broken"));
        assert_eq!(notes[1].tag, None);
        // One JSON object per line even with newlines in the text.
        assert_eq!(std::fs::read_to_string(path(&d)).unwrap().lines().count(), 2);
        let e = export(&d);
        assert_eq!(e.count, 2);
        assert!(e.text.contains("[Broken]") && e.text.contains("mic: Test Mic") && e.text.contains("it froze"));
        let out = tmp("b");
        std::fs::create_dir_all(&out).unwrap();
        let p = export_to_dir(&d, &out).unwrap();
        assert!(std::fs::read_to_string(p).unwrap().contains("second"));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn notes_travel_in_the_self_test_zip() {
        let d = tmp("c");
        save(&d, input("hello"), "2026-11-05T10:00:00Z", "0.1.0").unwrap();
        let mut notes = Vec::new();
        let files = crate::selftest::collect_files(&d, None, false, &mut notes);
        assert!(files.iter().any(|(n, _)| n == "SetupCoach/feedback.jsonl"), "{files:?}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
