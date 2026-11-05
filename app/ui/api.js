// Backend bridge. Uses Tauri when available; otherwise a built-in mock so the
// whole UI can be clicked through in a normal browser (see mock.js).
import { createMock } from './mock.js';

const tauri = typeof window !== 'undefined' ? window.__TAURI__ : undefined;
export const isMock = !tauri || !tauri.core;
const mock = isMock ? createMock() : null;

/** Call a backend command. Rejects with a plain-language message. */
export async function call(cmd, args = {}) {
  try {
    if (isMock) return await mock.invoke(cmd, args);
    return await tauri.core.invoke(cmd, args);
  } catch (e) {
    // Most commands reject with a plain string; A/B commands reject with { code, message }.
    const err = new Error(typeof e === 'string' ? e : (e && e.message) || 'Something went wrong.');
    if (e && typeof e === 'object' && typeof e.code === 'string') err.code = e.code;
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
  startHealthMonitor: () => call('start_health_monitor'),
  stopHealthMonitor: () => call('stop_health_monitor'),
  saveSetup: (name) => call('save_setup', { name }),
  reapplySavedSetup: () => call('reapply_saved_setup'),
  // Settings vault and A/B recordings
  listBackups: () => call('list_backups'),
  restoreBackup: (id) => call('restore_backup', { id }),
  previewRestore: (id) => call('preview_restore', { id }),
  obsRunning: () => call('obs_running'),
  recordFinal: (deviceId, context, constraints) => call('record_final', { deviceId, context, constraints }),
  getAb: (deviceId) => call('get_ab', { deviceId }),
  exportAb: (deviceId) => call('export_ab', { deviceId }),
  // Live coaching
  coachPlan: (area) => call('coach_plan', { area }),
  startCoach: (step) => call('start_coach', { step }),
  stopCoach: () => call('stop_coach'),
  openForUser: (target) => call('open_for_user', { target }),
};
