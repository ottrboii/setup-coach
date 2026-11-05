//! Matching a Windows device to a profile: USB VID/PID first, then the
//! endpoint/device name, then a generic fallback by connection type. When the
//! match is uncertain we return photo-friendly questions instead of guessing.

use super::schema::*;
use super::ProfileDb;
use serde::{Deserialize, Serialize};

/// What the Windows app knows about a device.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeviceIdentity {
    /// Windows endpoint / device friendly name,
    /// e.g. "Microphone (Yeti Stereo Microphone)".
    pub name: String,
    #[serde(default)]
    pub vid: Option<u16>,
    #[serde(default)]
    pub pid: Option<u16>,
    #[serde(default)]
    pub connection_hint: Option<ConnectionHint>,
}

impl DeviceIdentity {
    pub fn named(name: &str) -> Self {
        DeviceIdentity { name: name.to_string(), ..Default::default() }
    }
    pub fn with_usb(mut self, vid: u16, pid: u16) -> Self {
        self.vid = Some(vid);
        self.pid = Some(pid);
        self
    }
    pub fn with_hint(mut self, hint: ConnectionHint) -> Self {
        self.connection_hint = Some(hint);
        self
    }
    fn usb_id(&self) -> Option<UsbId> {
        Some(UsbId { vid: self.vid?, pid: self.pid? })
    }
}

/// How the device is attached, as far as Windows can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionHint {
    Usb,
    /// USB audio interface / mixer (XLR mic behind it).
    UsbAudioInterface,
    Bluetooth,
    /// Onboard audio jack (Realtek etc.).
    Analog35,
    Wireless24,
    Builtin,
    CaptureCard,
    Virtual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Matched by USB VID/PID.
    Exact,
    /// Matched by name pattern.
    NameMatch,
    /// No specific profile: generic class fallback.
    Generic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionId {
    /// Name matched several models: which one is it?
    WhichModel,
    /// A mic behind an interface/mixer: which mic is it?
    WhichMicOnInterface,
    WhichSideDoYouSpeakInto,
    WhichPatternSelected,
    SwitchPositions,
    /// Is there a Cloudlifter/FetHead-style box between mic and interface?
    BoosterInChain,
    WhichHeadsetModel,
    /// Is the device's own processing (Auto Level, noise cancel, presets) on?
    OnboardProcessing,
    WhichCameraBody,
    WhichPhoneApp,
}

/// A question for the user, ideally answered by tapping a photo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Question {
    pub id: QuestionId,
    pub prompt: String,
    /// Which photo/illustration to show (asset key), if any.
    #[serde(default)]
    pub photo_hint: Option<String>,
    /// Answer choices; empty = picker over the profile list.
    #[serde(default)]
    pub options: Vec<String>,
    /// Which control this answers when a profile has several (e.g. `bass_tilt`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// "What does this mean / where is it?" (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<Help>,
}

/// Always-available escape hatch on every closed-choice question.
pub const NOT_SURE: &str = "I'm not sure";

impl Question {
    fn new(id: QuestionId, prompt: &str) -> Self {
        Question { id, prompt: prompt.to_string(), photo_hint: None, options: Vec::new(), key: None, help: None }
    }
    /// Closed-choice questions always end with "I'm not sure".
    fn with_not_sure(mut self) -> Self {
        if !self.options.is_empty() && !self.options.iter().any(|o| o.eq_ignore_ascii_case(NOT_SURE)) {
            self.options.push(NOT_SURE.to_string());
        }
        self
    }
    fn photo(mut self, key: &str) -> Self {
        self.photo_hint = Some(key.to_string());
        self
    }
    fn options(mut self, opts: Vec<String>) -> Self {
        self.options = opts;
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchResult<'a, T> {
    pub profile: &'a T,
    pub confidence: Confidence,
    pub needs_questions: Vec<Question>,
}

/// A capture endpoint matched as both a mic and (possibly) an interface.
#[derive(Debug, Clone, PartialEq)]
pub struct InputMatch<'a> {
    pub mic: MatchResult<'a, MicProfile>,
    pub interface: Option<MatchResult<'a, InterfaceProfile>>,
}

/// Lower-case, fold Ø/ø to o, unify dashes and collapse whitespace.
pub fn normalize(s: &str) -> String {
    let folded: String = s
        .chars()
        .map(|c| match c {
            'Ø' | 'ø' => 'o',
            '\u{2010}'..='\u{2015}' => '-',
            c => c,
        })
        .collect::<String>()
        .to_lowercase();
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Best name match: the profiles whose longest matching pattern is longest.
/// Returns all profiles tied at the top (ambiguity -> ask the user).
fn best_by_name<'a, T>(
    items: impl Iterator<Item = &'a T>,
    name: &str,
    patterns: impl Fn(&'a T) -> &'a [String],
) -> Vec<&'a T> {
    let name = normalize(name);
    let mut best: Vec<&'a T> = Vec::new();
    let mut best_len = 0usize;
    for item in items {
        let len = patterns(item)
            .iter()
            .map(|p| normalize(p))
            .filter(|p| !p.is_empty() && name.contains(p.as_str()))
            .map(|p| p.chars().count())
            .max()
            .unwrap_or(0);
        if len == 0 {
            continue;
        }
        if len > best_len {
            best_len = len;
            best.clear();
            best.push(item);
        } else if len == best_len {
            best.push(item);
        }
    }
    best
}

fn contains_any(name: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| name.contains(n))
}

impl ProfileDb {
    /// Match a capture endpoint to a mic profile.
    pub fn match_mic(&self, dev: &DeviceIdentity) -> Option<MatchResult<'_, MicProfile>> {
        let name = normalize(&dev.name);
        let generic = |class: MicClass, questions: Vec<Question>| {
            self.generic_mic(class).map(|profile| MatchResult {
                profile,
                confidence: Confidence::Generic,
                needs_questions: questions,
            })
        };

        // 1. Bluetooth Hands-Free: always the BT generic (the warning matters
        //    more than the model).
        if dev.connection_hint == Some(ConnectionHint::Bluetooth)
            || contains_any(&name, &["hands-free", "hands free", "handsfree"])
        {
            return generic(MicClass::HeadsetBluetooth, vec![]);
        }

        let specific = || self.mics.iter().filter(|m| m.generic_class.is_none());

        // 2. USB VID/PID.
        if let Some(id) = dev.usb_id()
            && let Some(profile) = specific().find(|m| m.usb_ids.contains(&id))
        {
            return Some(MatchResult {
                profile,
                confidence: Confidence::Exact,
                needs_questions: mic_questions(profile),
            });
        }

        // 3. Name patterns.
        let hits = best_by_name(specific(), &dev.name, |m| &m.endpoint_name_patterns);
        if let Some(first) = hits.first() {
            let mut questions = mic_questions(first);
            if hits.len() > 1 {
                questions.insert(0, which_model(hits.iter().map(|m| model_label(&m.brand, &m.model))));
            }
            return Some(MatchResult { profile: first, confidence: Confidence::NameMatch, needs_questions: questions });
        }

        // 4. A mic behind an interface/mixer: we can't see the mic itself.
        if dev.connection_hint == Some(ConnectionHint::UsbAudioInterface)
            || self.match_interface(dev).is_some()
        {
            return generic(
                MicClass::XlrDynamicLowOutput,
                vec![
                    Question::new(QuestionId::WhichMicOnInterface, "Which mic is plugged into your interface?")
                        .photo("xlr_mic_lineup"),
                    Question::new(
                        QuestionId::BoosterInChain,
                        "Is there a small box (like a Cloudlifter or FetHead) between your mic and the interface?",
                    )
                    .photo("inline_booster")
                    .options(vec!["Yes".into(), "No".into(), "Not sure".into()]),
                ],
            );
        }

        // 5. Heuristics from the name / connection.
        if dev.connection_hint == Some(ConnectionHint::Builtin)
            || contains_any(&name, &["microphone array", "webcam", "camera", "integrated", "internal mic"])
        {
            return generic(MicClass::BuiltinMic, vec![]);
        }
        if dev.connection_hint == Some(ConnectionHint::Wireless24) {
            return generic(MicClass::HeadsetUsbWireless, vec![which_headset()]);
        }
        if contains_any(&name, &["headset", "headphone", "earphone"]) {
            let class = if dev.connection_hint == Some(ConnectionHint::Analog35) {
                MicClass::HeadsetAnalog
            } else {
                MicClass::HeadsetUsbWireless
            };
            return generic(class, vec![which_headset()]);
        }
        if dev.connection_hint == Some(ConnectionHint::Analog35)
            || contains_any(&name, &["realtek", "high definition audio", "line in"])
        {
            return generic(
                MicClass::HeadsetAnalog,
                vec![Question::new(
                    QuestionId::WhichHeadsetModel,
                    "Is this a headset mic, or a separate mic plugged into the pink jack?",
                )
                .photo("pink_jack")],
            );
        }

        // 6. Unknown USB mic: most common is a side-address condenser.
        generic(
            MicClass::UsbCondenserSide,
            vec![
                Question::new(QuestionId::WhichModel, "Which mic is this?").photo("usb_mic_lineup"),
                Question::new(QuestionId::WhichSideDoYouSpeakInto, "Which part of the mic do you talk into?")
                    .photo("mic_address")
                    .options(vec!["The side with the logo".into(), "The top".into(), "The end, like a stick".into()]),
            ],
        )
    }

    /// Match a capture/render endpoint to an interface or streamer mixer.
    pub fn match_interface(&self, dev: &DeviceIdentity) -> Option<MatchResult<'_, InterfaceProfile>> {
        if let Some(id) = dev.usb_id()
            && let Some(profile) = self.interfaces.iter().find(|i| i.usb_ids.contains(&id))
        {
            return Some(MatchResult { profile, confidence: Confidence::Exact, needs_questions: vec![] });
        }
        let hits = best_by_name(self.interfaces.iter(), &dev.name, |i| &i.name_patterns);
        let first = hits.first()?;
        let mut questions = Vec::new();
        if hits.len() > 1 {
            questions.push(which_model(hits.iter().map(|i| model_label(&i.brand, &i.model))));
        }
        Some(MatchResult { profile: first, confidence: Confidence::NameMatch, needs_questions: questions })
    }

    /// Match a capture endpoint: the mic profile plus the interface it sits behind.
    pub fn match_input(&self, dev: &DeviceIdentity) -> Option<InputMatch<'_>> {
        Some(InputMatch { mic: self.match_mic(dev)?, interface: self.match_interface(dev) })
    }

    /// Match a video device to a camera profile.
    pub fn match_camera(&self, dev: &DeviceIdentity) -> Option<MatchResult<'_, CameraProfile>> {
        let name = normalize(&dev.name);
        let with_questions = |profile: &CameraProfile| camera_questions(profile);
        let specific = || self.cameras.iter().filter(|c| !c.generic);

        if let Some(id) = dev.usb_id()
            && let Some(profile) = specific().find(|c| c.usb_ids.contains(&id))
        {
            return Some(MatchResult { profile, confidence: Confidence::Exact, needs_questions: with_questions(profile) });
        }

        let hits = best_by_name(specific(), &dev.name, |c| &c.name_patterns);
        if let Some(first) = hits.first() {
            let mut questions = with_questions(first);
            if hits.len() > 1 {
                questions.insert(0, which_model(hits.iter().map(|c| model_label(&c.brand, &c.model))));
            }
            return Some(MatchResult { profile: first, confidence: Confidence::NameMatch, needs_questions: questions });
        }

        let class = match dev.connection_hint {
            Some(ConnectionHint::CaptureCard) => CameraClass::CaptureCard,
            Some(ConnectionHint::Virtual) => CameraClass::Virtual,
            Some(ConnectionHint::Builtin) => CameraClass::Laptop,
            _ if name == "usb video" || name.starts_with("usb video")
                || contains_any(&name, &["capture", "hdmi", "cam link", "live gamer", "hd60", "uvc camera bridge"]) =>
            {
                CameraClass::CaptureCard
            }
            _ if contains_any(&name, &["camo", "droidcam", "iriun", "epoccam", "phone", "windows virtual camera"]) => {
                CameraClass::Phone
            }
            _ if contains_any(&name, &["virtual", "broadcast", "vcam"]) => CameraClass::Virtual,
            _ if contains_any(&name, &["integrated", "truevision", "user facing", "front camera", "facetime", "internal"]) => {
                CameraClass::Laptop
            }
            _ => CameraClass::FullUvc,
        };
        let profile = self.generic_camera(class)?;
        Some(MatchResult { profile, confidence: Confidence::Generic, needs_questions: with_questions(profile) })
    }

    /// Questions a specific mic profile needs answered, for a mic the user
    /// picked by hand (the matcher only builds them for mics it recognised).
    pub fn questions_for_mic(&self, id: &str) -> Vec<Question> {
        self.mic(id).map(mic_questions).unwrap_or_default()
    }

    /// Find a camera body by free text from the picker ("a6000", "Sony ZV-E10").
    pub fn find_camera_body(&self, query: &str) -> Option<&CameraBodyProfile> {
        let q = normalize(query);
        let hits = best_by_name(self.camera_bodies.iter(), &q, |b| &b.aliases);
        if let Some(first) = hits.first() {
            return Some(first);
        }
        self.camera_bodies
            .iter()
            .find(|b| normalize(&b.model) == q || normalize(&b.id) == q || q.contains(&normalize(&b.model)))
    }
}

fn model_label(brand: &str, model: &str) -> String {
    format!("{brand} {model}")
}

fn which_model(labels: impl Iterator<Item = String>) -> Question {
    let mut opts: Vec<String> = labels.collect();
    opts.dedup();
    Question::new(QuestionId::WhichModel, "Which one do you have?").options(opts)
}

fn which_headset() -> Question {
    Question::new(QuestionId::WhichHeadsetModel, "Which headset is this?").photo("headset_lineup")
}

/// Questions a specific mic profile needs answered before tuning.
fn mic_questions(p: &MicProfile) -> Vec<Question> {
    let mut q = Vec::new();
    if let Address::MultiPattern { default, .. } = p.address {
        q.push(
            Question::new(QuestionId::WhichPatternSelected, "Which pattern is your mic set to? (the symbol on the knob)")
                .photo(&format!("{}_pattern_knob", p.family))
                .options(p.patterns.iter().map(|x| format!("{x:?}")).collect())
                .with_default_note(default),
        );
    }
    if p.address.facing() == Some(Facing::Side) {
        q.push(
            Question::new(QuestionId::WhichSideDoYouSpeakInto, "Which part of the mic do you talk into?")
                .photo(&format!("{}_address", p.family))
                .options(vec!["The side with the logo".into(), "The top".into()]),
        );
    }
    // One exclusive-choice question per switch (never one lumped button).
    for c in &p.controls {
        let mut question = Question::new(QuestionId::SwitchPositions, &c.prompt).options(c.options.clone());
        question.key = Some(c.key.clone());
        question.help = c.help.clone();
        q.push(question);
    }
    if p.onboard_dsp.iter().any(|d| d.kind != DspKind::Limiter) {
        let names: Vec<&str> =
            p.onboard_dsp.iter().filter(|d| d.kind != DspKind::Limiter).map(|d| d.name.as_str()).collect();
        q.push(
            Question::new(
                QuestionId::OnboardProcessing,
                &format!("Is your mic's own sound processing switched on? ({})", names.join(", ")),
            )
            .options(vec!["Yes".into(), "No".into()]),
        );
    }
    q.into_iter().map(Question::with_not_sure).collect()
}

/// Questions a camera profile needs answered (coach-only classes).
fn camera_questions(p: &CameraProfile) -> Vec<Question> {
    match p.class {
        CameraClass::CaptureCard => vec![
            Question::new(QuestionId::WhichCameraBody, "Which camera is plugged into your capture card?")
                .photo("camera_body_lineup"),
        ],
        CameraClass::Phone => vec![
            Question::new(QuestionId::WhichPhoneApp, "Which app turns your phone into a webcam?")
                .options(vec!["Camo".into(), "DroidCam".into(), "Iriun".into(), "Windows Phone Link".into(), "Other".into()]),
        ],
        _ => vec![],
    }
}

trait DefaultNote {
    fn with_default_note(self, default: Pattern) -> Self;
}

impl DefaultNote for Question {
    /// Mention the factory default so the user knows what "unchanged" means.
    fn with_default_note(mut self, default: Pattern) -> Self {
        self.prompt.push_str(&format!(" Out of the box it is {default:?}."));
        self
    }
}
