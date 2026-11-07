// Which screens does the chosen setup profile need? Uses SetupProfileInfo.irrelevant (API.md "Setup profiles").
// Nothing here blocks a screen: a hidden screen still opens from a link or a finding.
import { api } from './api.js';
import { state } from './state.js';

// Route -> finding areas it only produces findings for. Hidden when ALL its areas are marked irrelevant
// and none of the profile's `keep` ids sit in those areas.
const ROUTE_AREAS = { camera: ['camera'], sync: ['stream'], health: ['stream'] };

export const PROFILE_EVENT = 'sc-profile-changed';

export function activeInfo() {
  return state.profileInfos?.find((p) => p.id === state.profile) || null;
}

/** Load the session's profile once at start (cheap, cached). Failure means "hide nothing". */
export async function ensureProfileForNav() {
  try {
    if (!state.profile) state.profile = await api.getSetupProfile();
    if (!state.profileInfos) { state.profileInfos = await api.listSetupProfiles(''); state.profileDevice = ''; }
  } catch (_) { /* no profile info: show everything */ }
}

/** True when the active profile says this route only produces findings it ignores. */
export function isRouteHidden(route) {
  const areas = ROUTE_AREAS[route];
  const irr = activeInfo()?.irrelevant;
  if (!areas || !irr?.areas?.length) return false;
  const keep = irr.keep || [];
  return areas.every((a) => irr.areas.includes(a) && !keep.some((id) => id.startsWith(`${a}.`)));
}

export function profileChanged() { window.dispatchEvent(new Event(PROFILE_EVENT)); }
