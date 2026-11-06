// Backend bridge. Uses Tauri when available; otherwise a built-in mock so the
// whole UI can be clicked through in a normal browser (see mock.js).
import { createMock } from './mock.js';

const tauri = typeof window !== 'undefined' ? window.__TAURI__ : undefined;
export const isMock = !tauri || !tauri.core;
const mock = isMock ? createMock() : null;

// Commands that must answer quickly: if the backend never replies (a hung or
// crashed task) the user gets a plain error with a retry, not an endless wait.
const LIMITS_MS = { get_environment: 60000, connect_obs: 30000, run_quick_checks: 60000, apply_actions: 120000 };

/** Write a line to the app log (%APPDATA%\SetupCoach\logs\ui.log). Never throws. */
export function logUi(level, source, message) {
  try {
    const text = String(message ?? '').slice(0, 2000);
    if (typeof console !== 'undefined') console[level === 'error' ? 'error' : 'warn'](`[${source}] ${text}`);
    if (isMock) return mock.invoke('log_ui', { level, source, message: text }).catch(() => {});
    return tauri.core.invoke('log_ui', { level, source, message: text }).catch(() => {});
  } catch (_) { /* logging must never break the UI */ }
  return undefined;
}

/** Call a backend command. Rejects with a plain-language message. */
export async function call(cmd, args = {}) {
  try {
    const run = isMock ? mock.invoke(cmd, args) : tauri.core.invoke(cmd, args);
    const limit = LIMITS_MS[cmd];
    if (!limit) return await run;
    let timer;
    const timeout = new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error('This is taking longer than it should. Please try again.')), limit);
    });
    try { return await Promise.race([run, timeout]); } finally { clearTimeout(timer); }
  } catch (e) {
    // Most commands reject with a plain string; A/B and camera-test commands reject with { code, message }.
    const err = new Error(typeof e === 'string' ? e : (e && e.message) || 'Something went wrong.');
    if (e && typeof e === 'object' && typeof e.code === 'string') err.code = e.code;
    logUi('error', `backend:${cmd}`, err.message);
    throw err;
  }
}

/** Subscribe to a backend event. Returns an unsubscribe function. */
export async function on(event, handler) {
  if (isMock) return mock.listen(event, handler);
  const un = await tauri.event.listen(event, (e) => handler(e.payload));
  return un;
}

export const api = {
  getEnvironment: () => call('get_environment'),
  connectObs: (password) => call('connect_obs', password ? { password } : {}),
  runQuickChecks: () => call('run_quick_checks'),
  logUi,
  saveFeedback: (input) => call('save_feedback', { input }),
  getFeedback: () => call('get_feedback'),
  saveFeedbackToDesktop: () => call('save_feedback_to_desktop'),
  applyActions: (actionIds) => call('apply_actions', { actionIds }),
  micContext: (deviceId, answers) => call('mic_context', { deviceId, answers }),
  startMicTest: (deviceId) => call('start_mic_test', { deviceId }),
  markStep: (kind, edge) => call('mark_step', { kind, edge }),
  finishMicTest: (context, constraints) => call('finish_mic_test', { context, constraints }),
  rescanMic: (context, constraints) => call('rescan_mic', { context, constraints }),
  applyMicChain: (chain, obsInput) => call('apply_mic_chain', { chain, obsInput }),
  verifyMic: () => call('verify_mic'),
  playPreview: (which, phone) => call('play_preview', { which, phone }),
  stopPreview: () => call('stop_preview'),
  cameraState: (cameraId) => call('camera_state', { cameraId }),
  startCameraTest: (cameraId) => call('start_camera_test', { cameraId }),
  captureCameraStep: (step, seconds) => call('capture_camera_step', { step, seconds }),
  finishCameraTest: (constraints) => call('finish_camera_test', { constraints }),
  applyCameraSettings: (cameraId, settings) => call('apply_camera_settings', { cameraId, settings }),
  cameraPictureStatus: (cameraId) => call('camera_picture_status', { cameraId }),
  freeCamera: (cameraId) => call('free_camera', { cameraId }),
  restoreFreedCamera: () => call('restore_freed_camera'),
  startSyncTest: (deviceId, cameraId) => call('start_sync_test', { deviceId, cameraId }),
  finishSyncTest: () => call('finish_sync_test'),
  stopSyncTest: () => call('stop_sync_test'),
  applySyncFix: () => call('apply_sync_fix'),
  undoSyncFix: () => call('undo_sync_fix'),
  startDriftCheck: (deviceId, cameraId, intervalS) => call('start_drift_check', { deviceId, cameraId, intervalS }),
  startDriftClap: () => call('start_drift_clap'),
  finishDriftClap: () => call('finish_drift_clap'),
  driftStatus: () => call('drift_status'),
  stopDriftCheck: () => call('stop_drift_check'),
  getMains: () => call('get_mains'),
  setMains: (hz) => call('set_mains', { hz }),
  startHealthMonitor: () => call('start_health_monitor'),
  stopHealthMonitor: () => call('stop_health_monitor'),
  saveSetup: (name) => call('save_setup', { name }),
  reapplySavedSetup: () => call('reapply_saved_setup'),
  // Settings vault and A/B recordings
  listBackups: () => call('list_backups'),
  restoreBackup: (id) => call('restore_backup', { id }),
  previewRestore: (id) => call('preview_restore', { id }),
  obsRunning: () => call('obs_running'),
  // What mic is plugged into this device?
  listMicProfiles: () => call('list_mic_profiles'),
  getMicBehind: (deviceId) => call('get_mic_behind', { deviceId }),
  setMicBehind: (deviceId, choice) => call('set_mic_behind', { deviceId, choice }),
  recordFinal: (deviceId, context, constraints) => call('record_final', { deviceId, context, constraints }),
  getAb: (deviceId) => call('get_ab', { deviceId }),
  exportAb: (deviceId) => call('export_ab', { deviceId }),
  // Live coaching
  coachPlan: (area) => call('coach_plan', { area }),
  startCoach: (step) => call('start_coach', { step }),
  stopCoach: () => call('stop_coach'),
  openForUser: (target) => call('open_for_user', { target }),
};
