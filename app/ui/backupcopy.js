// Plain-language wording for backup items and A/B errors. The backend sends stable
// `kind` / `code` values (app/API.md); the words live here. No numbers.

const KIND_WORDS = {
  obs_filters: 'The filters on a mic in OBS',
  obs_source_settings: 'How a source is set up in OBS',
  obs_mute: 'Whether a source is muted in OBS',
  obs_monitoring: 'Whether you hear a source through OBS',
  obs_setting: 'A stream or recording setting in OBS',
  obs_video: 'The picture size OBS streams at',
  obs_scene_file: 'Your OBS scenes',
  obs_profile_file: 'Your OBS settings file',
  obs_websocket: "OBS's remote-control switch",
  windows_setting: 'A Windows sound setting',
  mic_level: 'Your mic volume in Windows',
  camera: 'Your webcam picture settings',
  voice_chain: 'Your Setup Coach voice settings',
  goxlr: 'Your GoXLR mic setting',
};

/** Plain name of a saved item. Unknown kinds fall back to the backend's English label. */
export function itemWords(item) {
  return KIND_WORDS[item.kind] || item.label;
}

/** The specific name (which source, which setting) when it adds something; '' otherwise. */
export function itemDetail(item) {
  if (!KIND_WORDS[item.kind]) return '';
  return item.label || '';
}

const AB_WORDS = {
  no_first_test: {
    title: 'Nothing to compare yet',
    text: 'We keep your first mic test to compare with. Run the mic test once, apply your new settings, then come back here.',
    action: 'mic',
  },
  mic_missing: {
    title: "We can't find that mic",
    text: "It isn't plugged in, and we have no recordings of it. Plug it in, then run the mic test.",
    action: 'mic',
  },
  no_final_take: {
    title: 'Your new sound is not recorded yet',
    text: 'Apply your new settings, then read the passage once more. After that you can save both clips.',
    action: 'ab-record',
  },
  no_recording: { title: "The recording didn't start", text: 'Please try the reading again.', action: null },
  wrong_mic: { title: 'That was a different mic', text: 'Please read the passage again with the mic you tested first.', action: null },
  too_short: { title: 'That was too short', text: 'Please read the whole passage again.', action: null },
  no_read_step: { title: "We didn't catch the reading", text: 'Please read the passage again.', action: null },
  no_applied_chain: {
    title: 'Apply your new settings first',
    text: 'Your new sound is made from your new settings, so they need to be applied before the second reading.',
    action: 'mic-apply',
  },
  unreadable: {
    title: "A saved recording can't be opened",
    text: 'Run the mic test again and we will save a fresh one.',
    action: 'mic',
  },
};

const ACTION_LABEL = { mic: 'Test my mic', 'ab-record': 'Read the passage again', 'mic-apply': 'Apply my settings' };

/** { title, text, action, actionLabel } for an error from get_ab / record_final / export_ab. */
export function abErrorWords(err) {
  const w = AB_WORDS[err && err.code];
  if (!w) return { title: "That didn't work", text: (err && err.message) || 'Something went wrong. Please try again.', action: null, actionLabel: '' };
  return { ...w, actionLabel: w.action ? ACTION_LABEL[w.action] : '' };
}
