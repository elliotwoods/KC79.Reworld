// The app's icon set, and the concept map that decides which icon a thing gets.
//
// Geometry is vendored from Lucide (https://lucide.dev) — ISC licence, notice at the foot of
// this file. Only the icons this app actually uses are here. To add one, copy its <path> data
// out of the Lucide source; do not reach for a dependency. That is the same discipline the
// framework applies to its fonts (`fonts.lock`) and its application marks (`icon.lock`): the
// artefact is in the tree, and what it was taken from is written down.
//
// Why Lucide and not FontAwesome, which the openFrameworks Router used: av-frameworks' own
// eighteen application marks (`brand/glyphs/*.svg`) are hand-authored `stroke="currentColor"`
// round-capped SVGs, and Lucide is drawn the same way. FontAwesome Solid's filled shapes would
// sit beside those marks — in the tab strip, next to the favicon — and read as a second set.
// The openFrameworks app is still the reference for *which* concept gets *which* icon; see
// `ACTION_ICONS` below, where its choices win wherever the two predecessors disagreed.

import type { ReactNode } from 'react';

/**
 * One icon.
 *
 * Sized in `em`, not pixels: an icon then scales with whatever contains it, so the same
 * component is right inside a button, a `label-caps` panel head and a `status-item-label`
 * without a size prop at any call site. `currentColor` does the same job for colour — the
 * tone classes (`is-ok`, `is-warn`, `--danger`) already paint these, in both themes.
 *
 * `aria-hidden`, because every icon here sits beside text or beside an element that carries
 * its own `title`/`aria-label`. An icon that became the only content of a control without
 * one of those would be an unlabelled button, so do not remove those attributes.
 */
function icon(paths: ReactNode) {
  return function Icon({ className }: { className?: string }) {
    return (
      <svg
        className={className}
        viewBox="0 0 24 24"
        width="1em"
        height="1em"
        fill="none"
        stroke="currentColor"
        strokeWidth={2}
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
        focusable="false"
      >
        {paths}
      </svg>
    );
  };
}

export type IconComponent = ReturnType<typeof icon>;

// ------------------------------------------------------------------ navigation and modules

export const House = icon(
  <>
    <path d="M15 21v-8a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v8" />
    <path d="M3 10a2 2 0 0 1 .709-1.528l7-5.999a2 2 0 0 1 2.582 0l7 5.999A2 2 0 0 1 21 10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
  </>,
);

export const Image = icon(
  <>
    <rect width="18" height="18" x="3" y="3" rx="2" ry="2" />
    <circle cx="9" cy="9" r="2" />
    <path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21" />
  </>,
);

export const Network = icon(
  <>
    <rect x="16" y="16" width="6" height="6" rx="1" />
    <rect x="2" y="16" width="6" height="6" rx="1" />
    <rect x="9" y="2" width="6" height="6" rx="1" />
    <path d="M5 16v-3a1 1 0 0 1 1-1h12a1 1 0 0 1 1 1v3" />
    <path d="M12 12V8" />
  </>,
);

export const HeartPulse = icon(
  <>
    <path d="M19 14c1.49-1.46 3-3.21 3-5.5A5.5 5.5 0 0 0 16.5 3c-1.76 0-3 .5-4.5 2-1.5-1.5-2.74-2-4.5-2A5.5 5.5 0 0 0 2 8.5c0 2.3 1.5 4.05 3 5.5l7 7Z" />
    <path d="M3.22 13H9.5l.5-1 2 4.5 2-7 1.5 3.5h5.27" />
  </>,
);

export const Layers = icon(
  <>
    <path d="M12.83 2.18a2 2 0 0 0-1.66 0L2.6 6.08a1 1 0 0 0 0 1.83l8.58 3.91a2 2 0 0 0 1.66 0l8.58-3.9a1 1 0 0 0 0-1.83z" />
    <path d="M2 12a1 1 0 0 0 .58.91l8.6 3.91a2 2 0 0 0 1.65 0l8.58-3.9A1 1 0 0 0 22 12" />
    <path d="M2 17a1 1 0 0 0 .58.91l8.6 3.91a2 2 0 0 0 1.65 0l8.58-3.9A1 1 0 0 0 22 17" />
  </>,
);

// ------------------------------------------------------------------------ link and traffic

/**
 * The RS485 link: columns, connections, the bus.
 *
 * Not Lucide's `cable`, and not for want of trying it — five interlocking paths measured as
 * an unreadable tangle in the status bar and the panel heads, both of which draw at 14px.
 * This is the hub-and-spoke of the framework's own `brand/glyphs/router.svg` instead: one
 * node holding the hardware, a line out to each thing on the bus. Three strokes and a dot
 * survive 14px, and it is the mark this application already wears in the taskbar.
 *
 * The brand glyph also puts a dot on each spoke's far end. Those are dropped here, measured
 * rather than judged: at 14px a 1.75-unit dot is the same ~2px as the round line cap already
 * sitting there, so all three were invisible in the status bar and changed nothing but bytes.
 * They earn their place in the taskbar mark, which is drawn from 16px to 256px.
 */
export const Cable = icon(
  <>
    <path d="M12 12 5 5" />
    <path d="m12 12 7-7" />
    <path d="M12 12v8" />
    <circle cx="12" cy="12" r="2.5" fill="currentColor" stroke="none" />
  </>,
);

export const PlugZap = icon(
  <>
    <path d="M6.3 20.3a2.4 2.4 0 0 0 3.4 0L12 18l-6-6-2.3 2.3a2.4 2.4 0 0 0 0 3.4Z" />
    <path d="m2 22 3-3" />
    <path d="M7.5 13.5 10 11" />
    <path d="M10.5 16.5 13 14" />
    <path d="m18 3-4 4h6l-4 4" />
  </>,
);

export const Unplug = icon(
  <>
    <path d="m19 5 3-3" />
    <path d="m2 22 3-3" />
    <path d="M6.3 20.3a2.4 2.4 0 0 0 3.4 0L12 18l-6-6-2.3 2.3a2.4 2.4 0 0 0 0 3.4Z" />
    <path d="M7.5 13.5 10 11" />
    <path d="M10.5 16.5 13 14" />
    <path d="M12 6 18 12l2.3-2.3a2.4 2.4 0 0 0 0-3.4l-2.6-2.6a2.4 2.4 0 0 0-3.4 0Z" />
  </>,
);

/**
 * Broadcast: an action sent to every portal at once.
 *
 * Lucide's `antenna` was the obvious pick and measured unreadable — its four diagonals land
 * about two pixels apart at the 14px the panel heads draw at, and fuse into a smudge. This
 * is the radiating form instead (a source and two waves), which is three well-separated
 * shapes and survives the same size. `brand/marks.json` records the identical finding for
 * the application marks: thin strokes close together fuse before they get small.
 */
export const Antenna = icon(
  <>
    <path d="M4 11a9 9 0 0 1 9 9" />
    <path d="M4 4a16 16 0 0 1 16 16" />
    <circle cx="5" cy="19" r="1.5" fill="currentColor" stroke="none" />
  </>,
);

export const Radio = icon(
  <>
    <path d="M16.247 7.761a6 6 0 0 1 0 8.478" />
    <path d="M19.075 4.933a10 10 0 0 1 0 14.134" />
    <path d="M4.925 19.067a10 10 0 0 1 0-14.134" />
    <path d="M7.753 16.239a6 6 0 0 1 0-8.478" />
    <circle cx="12" cy="12" r="2" />
  </>,
);

export const Server = icon(
  <>
    <rect width="20" height="8" x="2" y="2" rx="2" ry="2" />
    <rect width="20" height="8" x="2" y="14" rx="2" ry="2" />
    <line x1="6" x2="6.01" y1="6" y2="6" />
    <line x1="6" x2="6.01" y1="18" y2="18" />
  </>,
);

export const ArrowUpDown = icon(
  <>
    <path d="m21 16-4 4-4-4" />
    <path d="M17 20V4" />
    <path d="m3 8 4-4 4 4" />
    <path d="M7 4v16" />
  </>,
);

export const Send = icon(
  <>
    <path d="M14.536 21.686a.5.5 0 0 0 .937-.024l6.5-19a.496.496 0 0 0-.635-.635l-19 6.5a.5.5 0 0 0-.024.937l7.93 3.18a2 2 0 0 1 1.112 1.11z" />
    <path d="m21.854 2.147-10.94 10.939" />
  </>,
);

export const Terminal = icon(
  <>
    <path d="M12 19h8" />
    <path d="m4 17 6-6-6-6" />
  </>,
);

// ------------------------------------------------------------------------- state and fault

export const Activity = icon(
  <path d="M22 12h-2.48a2 2 0 0 0-1.93 1.46l-2.35 8.36a.25.25 0 0 1-.48 0L9.24 2.18a.25.25 0 0 0-.48 0l-2.35 8.36A2 2 0 0 1 4.49 12H2" />,
);

export const AlertTriangle = icon(
  <>
    <path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3" />
    <path d="M12 9v4" />
    <path d="M12 17h.01" />
  </>,
);

export const Bug = icon(
  <>
    <path d="m8 2 1.88 1.88" />
    <path d="M14.12 3.88 16 2" />
    <path d="M9 7.13v-1a3.003 3.003 0 1 1 6 0v1" />
    <path d="M12 20c-3.3 0-6-2.7-6-6v-3a4 4 0 0 1 4-4h4a4 4 0 0 1 4 4v3c0 3.3-2.7 6-6 6" />
    <path d="M12 20v-9" />
    <path d="M6.53 9C4.6 8.8 3 7.1 3 5" />
    <path d="M6 13H2" />
    <path d="M3 21c0-2.1 1.7-3.9 3.8-4" />
    <path d="M20.97 5c0 2.1-1.6 3.8-3.5 4" />
    <path d="M22 13h-4" />
    <path d="M17.2 17c2.1.1 3.8 1.9 3.8 4" />
  </>,
);

export const Clock = icon(
  <>
    <path d="M12 6v6l4 2" />
    <circle cx="12" cy="12" r="10" />
  </>,
);

export const CheckCircle = icon(
  <>
    <path d="M21.801 10A10 10 0 1 1 17 3.335" />
    <path d="m9 11 3 3L22 4" />
  </>,
);

export const Check = icon(<path d="M20 6 9 17l-5-5" />);

export const X = icon(
  <>
    <path d="M18 6 6 18" />
    <path d="m6 6 12 12" />
  </>,
);

// ----------------------------------------------------------------------------- hardware acts

export const LocateFixed = icon(
  <>
    <line x1="2" x2="5" y1="12" y2="12" />
    <line x1="19" x2="22" y1="12" y2="12" />
    <line x1="12" x2="12" y1="2" y2="5" />
    <line x1="12" x2="12" y1="19" y2="22" />
    <circle cx="12" cy="12" r="7" />
    <circle cx="12" cy="12" r="3" />
  </>,
);

export const Siren = icon(
  <>
    <path d="M7 18v-6a5 5 0 1 1 10 0v6" />
    <path d="M5 21a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-1a2 2 0 0 0-2-2H7a2 2 0 0 0-2 2z" />
    <path d="M21 12h1" />
    <path d="M18.5 4.5 18 5" />
    <path d="M2 12h1" />
    <path d="M12 2v1" />
    <path d="m4.929 4.929.707.707" />
  </>,
);

export const Lightbulb = icon(
  <>
    <path d="M15 14c.2-1 .7-1.7 1.5-2.5 1-.9 1.5-2.2 1.5-3.5A6 6 0 0 0 6 8c0 1 .2 2.2 1.5 3.5.7.7 1.3 1.5 1.5 2.5" />
    <path d="M9 18h6" />
    <path d="M10 22h4" />
  </>,
);

export const LightbulbOff = icon(
  <>
    <path d="M16.8 11.2c.8-.9 1.2-2 1.2-3.2a6 6 0 0 0-9.3-5" />
    <path d="m2 2 20 20" />
    <path d="M6.3 6.3a4.67 4.67 0 0 0 1.2 5.2c.7.7 1.3 1.5 1.5 2.5" />
    <path d="M9 18h6" />
    <path d="M10 22h4" />
  </>,
);

export const Eye = icon(
  <>
    <path d="M2.062 12.348a1 1 0 0 1 0-.696 10.75 10.75 0 0 1 19.876 0 1 1 0 0 1 0 .696 10.75 10.75 0 0 1-19.876 0" />
    <circle cx="12" cy="12" r="3" />
  </>,
);

export const Target = icon(
  <>
    <circle cx="12" cy="12" r="10" />
    <circle cx="12" cy="12" r="6" />
    <circle cx="12" cy="12" r="2" />
  </>,
);

export const Crosshair = icon(
  <>
    <circle cx="12" cy="12" r="10" />
    <line x1="22" x2="18" y1="12" y2="12" />
    <line x1="6" x2="2" y1="12" y2="12" />
    <line x1="12" x2="12" y1="6" y2="2" />
    <line x1="12" x2="12" y1="22" y2="18" />
  </>,
);

export const CircleDot = icon(
  <>
    <circle cx="12" cy="12" r="10" />
    <circle cx="12" cy="12" r="1" />
  </>,
);

// `hammer` and `log-out` are the openFrameworks app's choices for Unjam and Escape; the iced
// GUI used `wrench` and `unlock`, which say "settings" and "security" rather than "a stuck
// mechanism" and "leave the routine you are in".
export const Hammer = icon(
  <>
    <path d="m15 12-8.373 8.373a1 1 0 1 1-3-3L12 9" />
    <path d="m18 15 4-4" />
    <path d="m21.5 11.5-1.914-1.914A2 2 0 0 1 19 8.172V7l-2.26-2.26a6 6 0 0 0-4.202-1.756L9 2.96l.92.82A6.18 6.18 0 0 1 12 8.4V10l2 2h1.172a2 2 0 0 1 1.414.586L18.5 14.5" />
  </>,
);

export const LogOut = icon(
  <>
    <path d="m16 17 5-5-5-5" />
    <path d="M21 12H9" />
    <path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" />
  </>,
);

export const Power = icon(
  <>
    <path d="M12 2v10" />
    <path d="M18.4 6.6a9 9 0 1 1-12.77.04" />
  </>,
);

export const Flag = icon(
  <>
    <path d="M4 15s1-1 4-1 5 2 8 2 4-1 4-1V3s-1 1-4 1-5-2-8-2-4 1-4 1z" />
    <line x1="4" x2="4" y1="22" y2="15" />
  </>,
);

export const Ruler = icon(
  <>
    <path d="M21.3 8.7 8.7 21.3a1 1 0 0 1-1.4 0l-4.6-4.6a1 1 0 0 1 0-1.4L15.3 2.7a1 1 0 0 1 1.4 0l4.6 4.6a1 1 0 0 1 0 1.4" />
    <path d="m7.5 10.5 2 2" />
    <path d="m10.5 7.5 2 2" />
    <path d="m13.5 4.5 2 2" />
    <path d="m4.5 13.5 2 2" />
  </>,
);

export const Wrench = icon(
  <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z" />,
);

export const CircleGauge = icon(
  <>
    <path d="M15.6 2.7a10 10 0 1 0 5.7 5.7" />
    <circle cx="12" cy="12" r="2" />
    <path d="M13.4 10.6 19 5" />
  </>,
);

export const Cpu = icon(
  <>
    <path d="M12 20v2" />
    <path d="M12 2v2" />
    <path d="M17 20v2" />
    <path d="M17 2v2" />
    <path d="M2 12h2" />
    <path d="M2 17h2" />
    <path d="M2 7h2" />
    <path d="M20 12h2" />
    <path d="M20 17h2" />
    <path d="M20 7h2" />
    <path d="M7 20v2" />
    <path d="M7 2v2" />
    <rect x="4" y="4" width="16" height="16" rx="2" />
    <rect x="8" y="8" width="8" height="8" rx="1" />
  </>,
);

export const SlidersHorizontal = icon(
  <>
    <path d="m10 7-6.76 0" />
    <circle cx="12" cy="7" r="2" />
    <path d="m20.76 7-6.76 0" />
    <path d="m6 17-2.76 0" />
    <circle cx="8" cy="17" r="2" />
    <path d="m20.76 17-10.76 0" />
  </>,
);

// ------------------------------------------------------------------------- timers and tests

export const CirclePlay = icon(
  <>
    <circle cx="12" cy="12" r="10" />
    <path d="m9 8 6 4-6 4Z" />
  </>,
);

export const CircleStop = icon(
  <>
    <circle cx="12" cy="12" r="10" />
    <rect x="9" y="9" width="6" height="6" rx="1" />
  </>,
);

export const FlaskConical = icon(
  <>
    <path d="M14 2v6a2 2 0 0 0 .245.96l5.51 10.08A2 2 0 0 1 18 22H6a2 2 0 0 1-1.755-2.96l5.51-10.08A2 2 0 0 0 10 8V2" />
    <path d="M6.453 15h11.094" />
    <path d="M8.5 2h7" />
  </>,
);

export const Play = icon(<path d="M5 5a2 2 0 0 1 3.008-1.728l11.997 6.998a2 2 0 0 1 .003 3.458l-12 7A2 2 0 0 1 5 19z" />);

export const SkipBack = icon(
  <>
    <path d="M20 5a2 2 0 0 0-3.008-1.728l-6.998 4.061a2 2 0 0 0-.003 3.458l7 4.134A2 2 0 0 0 20 13.19z" />
    <path d="M4 20V4" />
  </>,
);

// -------------------------------------------------------------------------- files and tools

export const Save = icon(
  <>
    <path d="M15.2 3a2 2 0 0 1 1.4.6l3.8 3.8a2 2 0 0 1 .6 1.4V19a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z" />
    <path d="M17 21v-7a1 1 0 0 0-1-1H8a1 1 0 0 0-1 1v7" />
    <path d="M7 3v4a1 1 0 0 0 1 1h7" />
  </>,
);

export const Upload = icon(
  <>
    <path d="M12 3v12" />
    <path d="m17 8-5-5-5 5" />
    <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
  </>,
);

export const HardDriveUpload = icon(
  <>
    <path d="m16 6-4-4-4 4" />
    <path d="M12 2v8" />
    <rect width="20" height="8" x="2" y="14" rx="2" />
    <path d="M6 18h.01" />
    <path d="M10 18h.01" />
  </>,
);

export const Eraser = icon(
  <>
    <path d="M21 21H8a2 2 0 0 1-1.42-.587l-3.994-3.999a2 2 0 0 1 0-2.828l10-10a2 2 0 0 1 2.829 0l5.999 6a2 2 0 0 1 0 2.828L12.834 21" />
    <path d="m5.082 11.09 8.828 8.828" />
  </>,
);

export const Trash = icon(
  <>
    <path d="M10 11v6" />
    <path d="M14 11v6" />
    <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6" />
    <path d="M3 6h18" />
    <path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
  </>,
);

export const FileText = icon(
  <>
    <path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z" />
    <path d="M14 2v4a2 2 0 0 0 2 2h4" />
    <path d="M10 9H8" />
    <path d="M16 13H8" />
    <path d="M16 17H8" />
  </>,
);

export const ScrollText = icon(
  <>
    <path d="M15 12h-5" />
    <path d="M15 8h-5" />
    <path d="M19 17V5a2 2 0 0 0-2-2H4" />
    <path d="M8 21h12a2 2 0 0 0 2-2v-1a1 1 0 0 0-1-1H11a1 1 0 0 0-1 1v1a2 2 0 1 1-4 0V5a2 2 0 1 0-4 0v2a1 1 0 0 0 1 1h3" />
  </>,
);

export const RefreshCw = icon(
  <>
    <path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8" />
    <path d="M21 3v5h-5" />
    <path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16" />
    <path d="M8 16H3v5" />
  </>,
);

export const RotateCcw = icon(
  <>
    <path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
    <path d="M3 3v5h5" />
  </>,
);

// ------------------------------------------------------------------------ renderer sources

export const Blend = icon(
  <>
    <circle cx="9" cy="9" r="7" />
    <circle cx="15" cy="15" r="7" />
  </>,
);

export const Type = icon(
  <>
    <path d="M12 4v16" />
    <path d="M4 7V5a1 1 0 0 1 1-1h14a1 1 0 0 1 1 1v2" />
    <path d="M9 20h6" />
  </>,
);

export const Film = icon(
  <>
    <rect width="18" height="18" x="3" y="3" rx="2" />
    <path d="M7 3v18" />
    <path d="M3 7.5h4" />
    <path d="M3 12h18" />
    <path d="M3 16.5h4" />
    <path d="M17 3v18" />
    <path d="M17 7.5h4" />
    <path d="M17 16.5h4" />
  </>,
);

export const Cast = icon(
  <>
    <path d="M2 8V6a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-6" />
    <path d="M2 12a9 9 0 0 1 8 8" />
    <path d="M2 16a5 5 0 0 1 4 4" />
    <line x1="2" x2="2.01" y1="20" y2="20" />
  </>,
);

// -------------------------------------------------------------------------------- chevrons

export const ChevronLeft = icon(<path d="m15 18-6-6 6-6" />);
export const ChevronRight = icon(<path d="m9 18 6-6-6-6" />);
export const ChevronUp = icon(<path d="m18 15-6-6-6 6" />);
export const ChevronDown = icon(<path d="m6 9 6 6 6-6" />);

// ============================================================================ the concept map

/**
 * Which icon an action button gets, keyed on the **last segment** of its schema path.
 *
 * Every action in the app is a monotonic counter at `…/actions/<leaf>` rendered through
 * `Action`/`ConfirmAction` in `bits.tsx`, so this one table reaches every button — the
 * thirteen-button broadcast toolbar, the column and portal inspectors, firmware, the renderer
 * source stack — without an icon prop at any call site. A leaf with no entry simply gets no
 * icon, which is the right failure: a new action appears unadorned rather than mislabelled.
 *
 * The choices are the openFrameworks Router's wherever it and the iced GUI disagreed, since
 * that app is the one operators have actually used. Noted per line where they differed.
 */
const ACTION_ICONS: Record<string, IconComponent> = {
  // status
  poll: RefreshCw,
  rebuild_columns: RefreshCw,
  ping: LocateFixed,
  poll_position: LocateFixed,

  // identify
  flash_leds: Siren,
  lights_on: Lightbulb,
  lights_off: LightbulbOff,

  // motion
  home: House,
  home_routine: House,
  home_and_zero: House,
  go_home: Target,
  take_current: Target,
  set_current: Target,
  see_through: Eye,
  see_through_local: Eye,
  unjam: Hammer, // oF `hammer`; the iced GUI had `wrench`
  escape: LogOut, // oF `right-from-bracket`; the iced GUI had `unlock`
  unwind: RotateCcw,
  reset_local: Eraser,
  zero_position: CircleDot, // oF `circle-dot`
  measure_backlash: Ruler, // oF `ruler`

  // setup
  init: Flag, // oF `flag-checkered`; the iced GUI had `settings`
  calibrate: Ruler, // oF `ruler`; the iced GUI had `circle-gauge`
  push: Send,
  push_motion_profile: Upload, // oF `upload`
  push_profile: Upload,

  // timers and self-test
  init_timer: CirclePlay, // oF `circle-play`
  deinit_timer: CircleStop, // oF `circle-stop`
  test_timer: FlaskConical, // oF `vial`
  md_test_routine: Cpu,
  md_test_timer: Cpu,

  // link
  connect: PlugZap,
  disconnect: Unplug,
  clear_outbox: Trash,
  clear_counters: Eraser,

  // report
  mark: Flag,
  write_summary: FileText,

  // renderer
  remove: X,
  clear: Eraser,
  clear_file: Eraser,
  jump_to_start: SkipBack,
  add_gradient: Blend,
  add_text: Type,
  add_file_player: Film,
  add_spout: Cast,

  // firmware and config
  save_config: Save,
  upload: Upload,
  erase: Eraser,
  run: Play,

  // danger
  reboot: Power, // oF `power-off`; the iced GUI had `rotate-cw`
};

/** The icon for an action path, or `undefined` if the leaf has no entry. */
export function iconForAction(path: string): IconComponent | undefined {
  const leaf = path.slice(path.lastIndexOf('/') + 1);
  return ACTION_ICONS[leaf];
}

/**
 * The diagnostics fault feed. Kinds come off the wire, so the fallback is the interesting
 * case: an unrecognised kind is still a fault and still deserves the warning glyph.
 */
export function iconForFault(kind: string): IconComponent {
  switch (kind) {
    case 'ack_timeout':
      return Clock;
    case 'cobs_error':
    case 'msgpack_error':
      return Bug;
    case 'device_disconnect':
    case 'disconnect':
      return Unplug;
    case 'health_transition':
      return HeartPulse;
    default:
      return AlertTriangle;
  }
}

/** Renderer source cards and the four add-source buttons, keyed on the source's type name. */
export function iconForSourceType(type: string): IconComponent {
  switch (type) {
    case 'Gradient':
      return Blend;
    case 'Text':
      return Type;
    case 'FilePlayer':
      return Film;
    case 'Spout':
      return Cast;
    default:
      return Layers;
  }
}

// ---------------------------------------------------------------------------------- licence
//
// The icon geometry above is from Lucide, used under the ISC licence:
//
//   Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2022 as part of Feather
//   (MIT). All other copyright (c) for Lucide are held by Lucide Contributors 2022.
//
//   Permission to use, copy, modify, and/or distribute this software for any purpose with or
//   without fee is hereby granted, provided that the above copyright notice and this
//   permission notice appear in all copies.
//
//   THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO
//   THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT
//   SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR
//   ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION
//   OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE
//   USE OR PERFORMANCE OF THIS SOFTWARE.
