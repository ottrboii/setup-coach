// In-memory app state shared across screens.
export const state = {
  env: null,
  micId: null,
  cameraId: null,
  answers: {},           // MicAnswers + per-question raw answers
  obsInput: 'Mic/Aux',   // OBS input that carries the mic
  quick: null,           // QuickChecks
  applied: null,         // ApplyResult of quick fixes
  micContext: null,
  micConstraints: [],
  micReport: null,
  micVerify: null,
  cameraConstraints: [],
  cameraReport: null,
  cameraApplied: false,
  healthReport: null,
  saved: null,
  coach: null,           // { area: 'mic'|'camera', steps: CoachStep[]|null, index, status: {stepId: 'done'|'skipped'} }
  autoStart: null,       // 'mic' | 'camera' → start that test immediately (retest after coaching)
  scoreBefore: null,     // { mic?: number, camera?: number } score before coaching, for the "up from" chip
};

export function selectedMic() {
  return state.env?.mics?.find((m) => m.id === state.micId) || null;
}
export function selectedCamera() {
  return state.env?.cameras?.find((c) => c.id === state.cameraId) || null;
}

// Finding id → Constraint for "I can't do this" (mirrors core findings.rs).
export const MIC_CONSTRAINT = {
  'mic.too_far': 'cant_move_mic_closer',
  'mic.thin': 'cant_move_mic_closer',
  'mic.not_enough_gain': 'cant_add_clean_gain',
  'mic.background_noise': 'cant_remove_noise_source',
  'mic.reflection': 'cant_fix_reflection',
  'mic.roomy': 'cant_treat_room',
  'mic.level_swings': 'moves_a_lot',
  'mic.plosives': 'cant_fix_plosives',
  'mic.desk_thumps': 'cant_fix_desk_vibration',
};

export const CAMERA_CONSTRAINT = {
  'camera.too_dark_grainy': 'cant_add_light',
  'camera.face_needs_light': 'cant_add_light',
  'camera.underexposed': 'cant_add_light',
  'camera.harsh_shadows': 'cant_add_light',
  'camera.low_fps': 'cant_add_light',
  'camera.backlit': 'cant_move_backlight',
  'camera.background_competes': 'cant_move_backlight',
  'camera.colour_cast': 'cant_fix_mixed_light',
  'camera.face_tint': 'cant_fix_mixed_light',
  'camera.colour_drift': 'cant_fix_mixed_light',
  'camera.camera_low': 'cant_move_camera',
  'camera.framing': 'cant_move_camera',
  'camera.too_close': 'cant_move_camera',
  'camera.too_far': 'cant_move_camera',
};
