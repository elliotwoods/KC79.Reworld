/**
 * Firmware handed to the bench, rather than firmware the bench went looking for.
 *
 * The picker beside this lists what `portal_swd::artefacts::discover_in` found by walking a
 * repository-shaped tree. That is the right list for a bench standing in a checkout and no list at
 * all for the ordinary case of being sent one file — a colleague's build, a bisect candidate, a
 * release candidate off CI. Before this, the only way in was `PORTAL_FIRMWARE_DIR`, read once at
 * startup, so using it meant quitting the application.
 *
 * # The bytes go up, not a path
 *
 * A drop hands the page a `File`, not a filename, and that is fortunate rather than limiting: the
 * same page runs in a WKWebView, in a WebView2 window and in a browser on somebody else's desk,
 * and only the first two could ever have produced a path this host could open. Uploading the bytes
 * is the one answer that works in all three — and the only one that works at all when the operator
 * is not in the room. `RouterRS`' firmware panel reached the same conclusion by the same route.
 *
 * # Two targets, and why the classifier still wins
 *
 * The overlay offers the two banks as separate drop targets, which is how an operator resolves a
 * file the bench cannot identify. It is not how they override one it *can*: `staging::stage`
 * refuses to move a confidently-classified image into the other bank, because dropping an
 * application on the bootloader target is a slip of the pointer and obeying it writes over the one
 * bank a board cannot recover from on its own.
 */

import { Badge, DropOverlay, DropStrip, useFileDrop, type DropTarget } from '@auroravision/av-gui/controls';
import { useCallback, useEffect, useState } from 'react';
import { dropVerdict, type DropResult } from './bench-model';

const TARGETS: DropTarget[] = [
  { id: 'bootloader', label: 'Bootloader bank', hint: 'PortalBootloader — 0x08000000' },
  { id: 'application', label: 'Application bank', hint: 'PortalFW — 0x08004000 or 0x08006000' },
];

/** How long a successful drop stays on screen before it gets out of the way. */
const SETTLE_MS = 2600;

async function upload(file: File, bank: string | null): Promise<DropResult> {
  const local = dropVerdict(file.name, file.size);
  if (local) return { name: file.name, ok: false, detail: local };
  const query = new URLSearchParams({ name: file.name, bank: bank ?? 'auto' });
  try {
    const response = await fetch(`/api/bench/firmware/dropped?${query}`, {
      method: 'POST',
      body: await file.arrayBuffer(),
    });
    const body = await response.json();
    if (!response.ok || !body.ok) {
      return { name: file.name, ok: false, detail: body.error ?? `refused (${response.status})` };
    }
    return {
      name: file.name,
      ok: true,
      region: body.region,
      banner: body.banner ?? undefined,
      bytes: body.bytes,
      hasElf: !!body.has_elf,
      fits: body.fits !== false,
    };
  } catch (error) {
    // The host restarting mid-drop is the realistic case, and "the bench did not answer" is a
    // different thing from "the bench refused this file". Saying which is the difference between
    // trying again and looking for another build.
    return { name: file.name, ok: false, detail: `the bench did not answer: ${String(error)}` };
  }
}

/**
 * The one way in, for both gestures.
 *
 * The overlay lives at the top of the page and the Browse button lives inside the firmware panel,
 * which is two places in the tree with no common ancestor worth lifting state to. Without this they
 * would be two paths: a drop would get the reading/identified/refused rows and a browse would get
 * silence, and the accessible route would be the one that told you least. So the strip publishes
 * here and the overlay subscribes.
 */
type Submit = (files: File[], bank: string | null) => void;
let submitter: Submit | null = null;

/** Hand files to the overlay, wherever in the tree the caller is. */
export function submitFirmware(files: File[], bank: string | null = null) {
  submitter?.(files, bank);
}

/**
 * The window-wide drop surface. Mounted once, at the top of the page, because the gesture is
 * "drop it on the bench" rather than "find the firmware panel first".
 */
export function FirmwareDrop() {
  const [results, setResults] = useState<DropResult[]>([]);
  const [busy, setBusy] = useState(0);

  const take = useCallback((files: File[], bank: string | null) => {
    setResults(files.map((file) => ({ name: file.name, ok: null })));
    setBusy(files.length);
    for (const file of files) {
      void upload(file, bank).then((result) => {
        setResults((previous) =>
          previous.map((row) => (row.name === result.name && row.ok === null ? result : row)),
        );
        setBusy((count) => {
          const left = count - 1;
          // Clear only when every file has landed, and only when none of them needs reading.
          // A refusal is the answer the operator is waiting for; taking it away on a timer would
          // leave them with a file that did nothing and no reason why.
          if (left === 0) {
            window.setTimeout(() => {
              setResults((rows) => (rows.every((row) => row.ok) ? [] : rows));
            }, SETTLE_MS);
          }
          return left;
        });
      });
    }
  }, []);

  const drop = useFileDrop({ onFiles: take });
  useEffect(() => {
    submitter = take;
    return () => {
      if (submitter === take) submitter = null;
    };
  }, [take]);
  const open = drop.dragging || results.length > 0;

  return (
    <DropOverlay
      open={open}
      drop={drop}
      targets={drop.dragging ? TARGETS : []}
      title={results.length > 0 ? 'Firmware' : 'Drop firmware to load it'}
      hint={
        results.length > 0
          ? undefined
          : `${drop.count === 1 ? 'A .bin or .elf' : `${drop.count} files`} — dropped anywhere, the bench works out which bank. Drop on a bank to name one.`
      }
      onDismiss={busy === 0 && results.length > 0 ? () => setResults([]) : undefined}
    >
      {results.map((result) => (
        <div className="firmware-drop-result" key={result.name} data-ok={result.ok ?? undefined}>
          <span className="firmware-drop-copy">
            <strong>{result.name}</strong>
            <small>{describe(result)}</small>
          </span>
          {result.ok === null ? (
            <Badge tone="active">reading…</Badge>
          ) : result.ok ? (
            <Badge tone={result.fits ? 'ok' : 'warn'}>{result.region}</Badge>
          ) : (
            <Badge tone="error">refused</Badge>
          )}
        </div>
      ))}
    </DropOverlay>
  );
}

function describe(result: DropResult): string {
  if (result.ok === null) return 'reading and identifying…';
  if (!result.ok) return result.detail ?? 'refused';
  const size = `${((result.bytes ?? 0) / 1024).toFixed(1)} kB`;
  const parts = [result.banner, size];
  if (result.hasElf) parts.push('ELF kept, so the run-check has a liveness symbol');
  if (!result.fits) parts.push('too large for its bank');
  else parts.push(`selected as the ${result.region} image`);
  return parts.filter(Boolean).join(' · ');
}

/** The standing affordance under the two banks, so the gesture is discoverable without a drag. */
export function FirmwareDropStrip({ disabled }: { disabled: boolean }) {
  return (
    <DropStrip
      accept=".bin,.elf"
      multiple
      disabled={disabled}
      label="Drop firmware here"
      hint="A .bin or .elf, bootloader or application — anywhere on the window works too."
      onFiles={(files) => submitFirmware(files)}
    />
  );
}
