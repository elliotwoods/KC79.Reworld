// Small shared components: the action-counter button, fact rows, the grouped broadcast
// toolbar, danger confirmation, and the column heartbeat/outbox chips.

import { Button } from '@auroravision/av-gui/controls';
import { useParam } from '@auroravision/av-gui/runtime';
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { fit, palette, useRafDraw } from './canvas';
import { withAlpha } from './grid';
import { AlertTriangle, Clock, iconForAction, type IconComponent } from './icons';
import { heartbeatAlpha } from './math';
import { latestRow, useRing } from './model';

/**
 * A monotonic-counter action: pressing bumps the i64; the bridge acts on the change.
 *
 * The icon is resolved from the path's leaf rather than passed in, because every action in
 * the app is already addressed by path — so the map in `icons.tsx` is the single place the
 * vocabulary lives, and a call site cannot give one action two different glyphs in two
 * panels. `icon` overrides it for the handful of buttons whose path does not name the act
 * (the pilot D-pad, whose four directions are one parameter).
 */
export function Action({
  path,
  children,
  why,
  variant,
  className,
  icon,
  'aria-label': ariaLabel,
}: {
  path: string;
  /** Optional: an icon-only action (the renderer's remove button) has no text child. */
  children?: ReactNode;
  why?: string | null;
  variant?: 'default' | 'primary' | 'danger' | 'quiet';
  className?: string;
  icon?: IconComponent | null;
  /** Required when there is no `children` — the glyph is `aria-hidden`, so nothing else names it. */
  'aria-label'?: string;
}) {
  const p = useParam<number>(path);
  const disabled = !!why || !p.decl;
  const Glyph = icon === undefined ? iconForAction(path) : icon;
  return (
    <span className={className} title={why ?? p.decl?.label ?? path}>
      <Button
        variant={variant}
        disabled={disabled}
        aria-label={ariaLabel}
        onClick={() => p.set((p.value ?? 0) + 1)}
      >
        {Glyph && <Glyph />}
        {children}
      </Button>
    </span>
  );
}

/** A danger action that needs a second press within 3 s (Reboot, Erase flash). */
export function ConfirmAction({ path, children }: { path: string; children: ReactNode }) {
  const p = useParam<number>(path);
  const Glyph = iconForAction(path);
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    if (!armed) return;
    const timer = setTimeout(() => setArmed(false), 3000);
    return () => clearTimeout(timer);
  }, [armed]);
  return (
    <Button
      variant="danger"
      disabled={!p.decl}
      onClick={() => {
        if (armed) {
          p.set((p.value ?? 0) + 1);
          setArmed(false);
        } else {
          setArmed(true);
        }
      }}
    >
      {/* Armed, the glyph changes with the label: the warning triangle is the state, and it
          is what distinguishes an armed button from one that merely says something long. */}
      {armed ? <AlertTriangle /> : Glyph && <Glyph />}
      {armed ? 'Press again to confirm' : children}
    </Button>
  );
}

export function Fact({ label, value, tone }: { label: string; value: ReactNode; tone?: string }) {
  return (
    <div className={`fact${tone ? ` is-${tone}` : ''}`}>
      <span className="fact-label">{label}</span>
      <span className="fact-value">{value}</span>
    </div>
  );
}

/**
 * The Poll + 12 broadcast hardware actions, grouped for scanability, complete for parity.
 * `prefix` scopes the counters: `/installation`, `/columns/N`, `/portal`.
 */
export function BroadcastActions({ prefix }: { prefix: string }) {
  const groups: [string, [string, string][]][] = [
    ['status', [['poll', 'Poll'], ['ping', 'Ping']]],
    [
      'identify',
      [
        ['flash_leds', 'Flash lights'],
        ['lights_on', 'Lights on'],
        ['lights_off', 'Lights off'],
      ],
    ],
    [
      'motion',
      [
        ['home', 'Home routine'],
        ['go_home', 'Go home'],
        ['see_through', 'See through'],
        ['unjam', 'Unjam'],
        ['escape', 'Escape routine'],
      ],
    ],
    [
      'setup',
      [
        ['init', 'Initialise'],
        ['calibrate', 'Calibrate'],
      ],
    ],
  ];
  return (
    <div className="broadcast-actions">
      {groups.map(([group, actions]) => (
        <span key={group} className="action-group" data-group={group}>
          {actions.map(([leaf, label]) => (
            <Action key={leaf} path={`${prefix}/actions/${leaf}`}>
              {label}
            </Action>
          ))}
        </span>
      ))}
      <span className="action-group" data-group="danger">
        <ConfirmAction path={`${prefix}/actions/reboot`}>Reboot</ConfirmAction>
      </span>
    </div>
  );
}

/** Rx/Tx heartbeat dots for one column, fading over 300 ms with clock extrapolation. */
export function ColumnHeartbeats({ col }: { col: number }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const link = useRing('/tel/columns/link');
  const arrival = useRef<{ writePos: number; at: number }>({ writePos: -1, at: 0 });
  useRafDraw(() => {
    const el = canvas.current;
    if (!el) return;
    const ctx = fit(el);
    if (!ctx) return;
    const pal = palette();
    ctx.clearRect(0, 0, 26, 12);
    const row = latestRow(link);
    if (link && arrival.current.writePos !== link.writePos) {
      arrival.current = { writePos: link.writePos, at: performance.now() };
    }
    const extra = link ? performance.now() - arrival.current.at : 0;
    if (!row) return;
    const rxAlpha = heartbeatAlpha(row[col * 4] + extra);
    const txAlpha = heartbeatAlpha(row[col * 4 + 1] + extra);
    if (rxAlpha > 0) {
      ctx.fillStyle = withAlpha(pal.ok, rxAlpha);
      ctx.beginPath();
      ctx.arc(6, 6, 3, 0, Math.PI * 2);
      ctx.fill();
    }
    if (txAlpha > 0) {
      ctx.fillStyle = withAlpha(pal.accent, txAlpha);
      ctx.beginPath();
      ctx.arc(18, 6, 3, 0, Math.PI * 2);
      ctx.fill();
    }
  });
  return (
    <canvas
      ref={canvas}
      style={{ width: 26, height: 12 }}
      title="Rx (green) / Tx (blue) heartbeats"
      aria-label={`Column ${col + 1} link heartbeats`}
    />
  );
}

/**
 * The outbox depth chip with the 1.5 s anti-strobe hold: the value display is held after
 * the outbox last held packets so it doesn't flicker at the transmit cadence. Amber > 2.
 * `quiet` renders nothing while the held value is zero — for the wall's slim column
 * headers, where an idle outbox is not worth a chip.
 */
export function OutboxChip({ col, quiet = false }: { col: number; quiet?: boolean }) {
  const link = useRing('/tel/columns/link');
  const [display, setDisplay] = useState(0);
  const hold = useRef<{ value: number; at: number }>({ value: 0, at: 0 });
  useEffect(() => {
    const timer = setInterval(() => {
      const row = latestRow(link);
      const value = row ? row[col * 4 + 2] : 0;
      const now = performance.now();
      if (value > 0) {
        hold.current = { value, at: now };
      }
      const shown = now - hold.current.at < 1500 ? hold.current.value : value;
      setDisplay((previous) => (previous === shown ? previous : shown));
    }, 250);
    return () => clearInterval(timer);
  }, [link, col]);
  if (quiet && display <= 0) return null;
  return (
    <span className={`chip${display > 2 ? ' is-warn' : ''}`} title="Outbox depth">
      <Clock />
      {/* The glyph replaces the word in the quiet form: the wall's column headers have room
          for a chip but not for a label, and "ob 3" was the abbreviation that bought it. */}
      {quiet ? display : `outbox ${display}`}
    </span>
  );
}
