// Plain-language copy and display config for every live coaching task.
// Main view: no jargon. Numbers only appear in the Advanced drawer.

/**
 * domain: the gauge range [lo, hi] in the task's unit (fallback when the
 * backend sends no target); lowLabel/highLabel: words at each end of the gauge.
 */
export const TASKS = {
  set_gain: {
    animation: 'knob',
    title: 'Get your level into the green',
    why: 'Too quiet and viewers turn you up (and hear hiss); too loud and laughs crackle. The green zone gives you room for both.',
    todo: ['Talk like you do on stream — keep going the whole time.', 'Turn the gain knob slowly in the direction of the arrow.', 'Stop when the bar sits in the green and stays there.'],
    autoTodo: ['Just keep talking like you do on stream.', "We're turning your mic's level ourselves — watch it land in the green.", "Don't touch any knobs for a moment."],
    good: 'Good sounds like: your normal voice fills the green; a big laugh pops into yellow but never red.',
    domain: [-48, 0], lowLabel: 'quiet', highLabel: 'too loud', unit: 'dBFS',
  },
  find_distance: {
    animation: 'mic_distance',
    title: 'Find the sweet spot for your mic',
    why: 'Distance is the biggest thing for how your voice sounds. Close sounds warm and clear; far away sounds thin, echoey and picks up your keyboard.',
    todo: ['Keep talking normally.', 'Move the mic (or yourself) slowly in the direction of the arrow.', 'Stop when the mic sits inside the green band and hold it there.'],
    good: 'Good sounds like: full and close, like the person is next to you — not like they\'re across the room.',
    domain: [0, 40], lowLabel: 'very close', highLabel: 'far away', unit: 'cm',
  },
  angle_off_axis: {
    animation: 'mic_angle',
    title: 'Stop the pops',
    why: "Puffs of air from P and B hit the mic and thump. Talking just past the mic lets the air fly by — your voice still goes in.",
    todo: ['Say "pop, pop, Peter Piper" a few times.', 'Turn the mic a little to the side (or move it just below your mouth).', 'Keep saying pops until the puffs stop thumping.'],
    good: 'Good sounds like: crisp P\'s with no thud or "wind" after them.',
    domain: [-30, 10], lowLabel: 'no pops', highLabel: 'big pops', unit: 'dB',
  },
  quiet_room: {
    animation: 'noise_null',
    title: 'Quiet the background',
    why: "Fans, PCs and air-con hum under everything you say. Pointing the mic's 'deaf' back at the noise cuts it a lot — for free.",
    todo: ['Stay silent for a moment.', 'Turn off or move anything noisy you can (desk fan, heater).', 'Turn the back of the mic toward the noisiest thing — usually your PC.'],
    good: 'Good sounds like: silence when you\'re quiet, not a whoosh.',
    domain: [-80, -30], lowLabel: 'silent', highLabel: 'noisy', unit: 'dBFS',
  },
  keyboard_null: {
    animation: 'keyboard_null',
    title: 'Make your keyboard quieter on stream',
    why: 'Viewers hear every click if your mic points at your keyboard. Aiming the mic away (and closer to you) fixes most of it.',
    todo: ['Type normally — don\'t talk.', 'Turn the back of the mic toward your keyboard and bring it closer to your mouth.', 'Keep typing until the clicks drop into the green.'],
    good: 'Good sounds like: soft, distant ticks — not loud clacks.',
    domain: [-45, 0], lowLabel: 'faint', highLabel: 'loud clicks', unit: 'dB',
  },
  talk_into_logo: {
    animation: 'logo_side',
    title: 'Talk into the right side of the mic',
    why: 'Your mic listens from the side with the logo, not from the top. From the top you sound muffled and far away.',
    todo: ['Keep talking.', 'Turn the mic so the side with the logo faces your mouth.', 'Watch the bar rise as your voice gets clearer.'],
    good: 'Good sounds like: clear and present, with crisp S and T sounds.',
    domain: [-12, 6], lowLabel: 'muffled', highLabel: 'clear', unit: 'dB',
  },
  light_face: {
    animation: 'light_move',
    title: 'Get light on your face',
    why: 'The camera exposes for the brightest thing. If the window or wall is brighter than you, you become a silhouette.',
    todo: ['Turn on a lamp or light in front of you — or face the window.', 'If a window is behind you, close the blinds a little.', 'Watch your face brighten in the preview.'],
    good: 'Good looks like: your face is the brightest thing in the picture, with no shiny white patches.',
    domain: [-3, 2], lowLabel: 'darker than background', highLabel: 'brighter than background', unit: 'stops',
  },
  light_direction: {
    animation: 'light_move',
    title: 'Even out the shadows',
    why: 'One light straight above or from one side gives dark eye sockets and a half-black face. Moving it a little toward the darker side fixes it.',
    todo: ['Move your main light in the direction of the arrow.', 'Keep it slightly above your eyes, pointing down at your face.', 'Stop when both sides of your face look similar.'],
    good: 'Good looks like: soft shadows — one side slightly darker, both clearly visible.',
    domain: [1, 6], lowLabel: 'even', highLabel: 'harsh', unit: ':1',
  },
  white_balance: {
    animation: 'paper',
    title: 'Get your colours right',
    why: 'Mixed lights (a warm lamp + a blue window + RGB) tint your skin. A white sheet of paper shows us exactly what colour your light is.',
    todo: ['Hold a plain white sheet of paper inside the box on screen.', 'Turn off coloured or RGB lights that shine on you.', "Hold still — we'll lock the colour in."],
    good: 'Good looks like: the paper looks white in the preview — not orange, blue or green.',
    domain: [0, 20], lowLabel: 'neutral', highLabel: 'tinted', unit: '%',
  },
  camera_height: {
    animation: 'camera_height',
    title: 'Put the camera at eye level',
    why: 'A camera below your face looks up your nose and makes you look tired. At eye level you look like you\'re talking to viewers.',
    todo: ['Raise or lower the camera in the direction of the arrow — a stack of books works.', 'Look at your main screen like you do on stream.', 'Stop when the line through your eyes turns green.'],
    good: 'Good looks like: you\'re looking straight ahead, eyes about a third from the top of the picture.',
    domain: [-25, 25], lowLabel: 'camera low', highLabel: 'camera high', unit: '°',
  },
  framing: {
    animation: 'framing',
    title: 'Frame yourself nicely',
    why: 'Too close distorts your face; too far and viewers can\'t see your expressions. Head and shoulders, centred, is the sweet spot.',
    todo: ['Move yourself (or the camera) in the direction of the arrow.', 'Aim for head and shoulders, a little space above your head.', 'Hold still when the frame turns green.'],
    good: 'Good looks like: head and shoulders, centred, eyes about a third from the top.',
    domain: [0, 1], lowLabel: 'off', highLabel: 'framed', unit: '',
  },
  flicker: {
    animation: 'flicker',
    title: 'Stop the flicker',
    why: 'Lights pulse with your power grid. If the camera isn\'t set to match, dark bands roll through your picture.',
    todo: ["We're changing your camera's anti-flicker setting.", 'Sit still for a few seconds while we check.', 'If bands remain, a dimmable or cheap LED bulb may be the cause.'],
    good: 'Good looks like: a steady picture with no rolling bands or pulsing.',
    domain: [0, 1], lowLabel: 'steady', highLabel: 'flickering', unit: '',
  },
  check_setting: {
    animation: 'settings',
    title: 'Change one setting',
    why: 'This one lives in another app. We\'ll open it right next to this window and notice the moment it\'s done.',
    todo: [],
    good: "You don't need to come back and tell us — we'll see it change.",
    domain: [0, 1], lowLabel: '', highLabel: '', unit: '',
  },
};

/** Fallback text per hint kind (backend usually sends its own `hint.text`). */
export const HINT_TEXT = {
  closer: 'A little closer…', farther: 'Back off a touch…', turn_up: 'Turn it up…', turn_down: 'Turn it down…', stop: 'Stop — right there!',
  angle_away: 'Turn the mic a bit to the side…', face_logo: 'Turn the logo side toward you…',
  move_light_left: 'Move the light to your left…', move_light_right: 'Move the light to your right…', move_light_up: 'Raise the light a little…',
  add_light: 'Add some light in front of you…', dim_background: 'Make the background darker…',
  raise_camera: 'Raise the camera…', lower_camera: 'Lower the camera…',
  move_left: 'Move a little to your left…', move_right: 'Move a little to your right…', move_back: 'Sit back a little…', move_forward: 'Come a little closer…',
  keep_talking: 'Keep talking…', be_quiet: 'Stay quiet for a moment…', type_now: 'Type on your keyboard…', say_pops: 'Say "pop, pop, Peter Piper"…', wait: 'One moment…', none: '',
};

/** Friendly phase words. */
export const PHASE_TEXT = {
  waiting: 'Getting ready', working: 'Adjusting', almost: 'Almost there', hold: 'Hold it there', done: 'Perfect', stuck: "Let's try another way",
};

/** Area each task belongs to. */
export const TASK_AREA = {
  set_gain: 'mic', find_distance: 'mic', angle_off_axis: 'mic', quiet_room: 'mic', keyboard_null: 'mic', talk_into_logo: 'mic',
  light_face: 'camera', light_direction: 'camera', white_balance: 'camera', camera_height: 'camera', framing: 'camera', flicker: 'camera', check_setting: 'any',
};

/** Pretty label for a reading key in the Advanced drawer. */
export function readingLabel(key) {
  return key.replace(/_/g, ' ').replace(/\bdbfs\b/i, 'dBFS').replace(/\bdb\b/i, 'dB').replace(/^./, (c) => c.toUpperCase());
}
