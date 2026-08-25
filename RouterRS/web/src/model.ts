// Shared page-side helpers: parameter hooks, the action-counter idiom, selection, and
// telemetry-ring access. Controls bind by schema path only — labels, units, ranges and enum
// names come from the Rust schema, never from this file.

import { useBus, useParam, useTelemetry } from '@auroravision/av-gui/runtime';
import type { TelemetryRing } from '@auroravision/av-gui/runtime';

export function useEnumName(path: string): string {
  const p = useParam<number>(path);
  return p.decl?.variants.find((v) => v.value === p.value)?.name ?? 'unknown';
}
export const useText = (path: string) => useParam<string>(path).value ?? '';
export const useNumber = (path: string) => useParam<number>(path).value ?? 0;
export const useBool = (path: string) => !!useParam<boolean>(path).value;
export const useVec2 = (path: string): [number, number] => {
  const v = useParam<number[]>(path).value;
  return Array.isArray(v) && v.length >= 2 ? [v[0], v[1]] : [0, 0];
};

/** The telemetry ring behind a schema path, or null before the schema arrives. */
export function useRing(path: string): TelemetryRing | null {
  const bus = useBus();
  const { ringIndex } = useTelemetry(path);
  return ringIndex >= 0 ? (bus.rings[ringIndex] ?? null) : null;
}

/**
 * The newest complete sample of a wide channel as a zero-copy view into the ring, or null
 * before the first sample. Valid until the next push wraps over it, so callers copy what
 * they keep (the canvases read it inside one draw).
 */
export function latestRow(ring: TelemetryRing | null): Float32Array | null {
  if (!ring || ring.writePos === 0) return null;
  const slot = (ring.writePos - 1) % ring.capacity;
  return ring.data.subarray(slot * ring.width, (slot + 1) * ring.width);
}

/** Per-portal telemetry slot offsets, parsed from the published JSON. */
export function useSlotOffsets(): number[] {
  const text = useText('/installation/slot_offsets');
  try {
    const parsed = JSON.parse(text);
    return Array.isArray(parsed) ? parsed.map(Number) : [];
  } catch {
    return [];
  }
}

// ---------------------------------------------------------------------- selection

export type SelectKind = 'installation' | 'column' | 'portal' | 'source';

export function useSelection() {
  const kind = useParam<number>('/ui/select/kind');
  const col = useParam<number>('/ui/select/col');
  const portal = useParam<number>('/ui/select/portal');
  const source = useParam<number>('/ui/select/source');
  const kindName = (kind.decl?.variants.find((v) => v.value === kind.value)?.name ??
    'installation') as SelectKind;
  return {
    kind: kindName,
    col: col.value ?? 0,
    portal: portal.value ?? 1,
    source: source.value ?? 0,
    selectInstallation: () => kind.set(0),
    selectColumn: (c: number) => {
      col.set(c);
      kind.set(1);
    },
    selectPortal: (c: number, target: number) => {
      col.set(c);
      portal.set(target);
      kind.set(2);
    },
    selectSource: (s: number) => {
      source.set(s);
      kind.set(3);
    },
  };
}

// ---------------------------------------------------------------------- HTTP documents

/** Fetch a JSON document from the app's own HTTP surface. */
export async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, init);
  if (!response.ok) throw new Error(`${path}: ${response.status}`);
  return (await response.json()) as T;
}

export const postCommand = (body: Record<string, unknown>) =>
  api('/api/router/command', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
