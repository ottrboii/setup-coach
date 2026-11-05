// App-wide UI configuration.
export const CONFIG = {
  // Affiliate "optional upgrade" links are off until the programme is set up.
  affiliateEnabled: false,
  affiliateBaseUrl: 'https://example.com/go/',
  // Product category → friendly name for the optional upgrade cards.
  products: {
    inline_booster: { name: 'Inline mic booster', blurb: 'A small box between mic and interface that makes a quiet mic louder without adding hiss.' },
    pop_filter: { name: 'Pop filter', blurb: 'A mesh screen that stops puffs of air from popping the mic.' },
    boom_arm: { name: 'Boom arm', blurb: 'Holds the mic right in front of your mouth and off the desk.' },
    shock_mount: { name: 'Shock mount', blurb: 'Stops desk bumps travelling up into the mic.' },
    acoustic_panels: { name: 'Acoustic panels', blurb: 'Soft panels that stop your voice bouncing around the room.' },
    mic_cable: { name: 'New mic cable', blurb: 'A fresh cable fixes most crackles and dropouts.' },
    key_light: { name: 'Key light', blurb: 'A soft, bright light in front of you — the biggest camera upgrade there is.' },
  },
};
