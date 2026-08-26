// Firmware update: artefact discovery, browser upload (a WKWebView file input yields
// content, not a path — the bytes go up, the server mints the path), then flash / erase /
// run through the same command queue everything else uses. One component serves both the
// per-column and the mass (every connected column) flows, exactly the C++ split.
//
// The upload half is a drop as well as a button. Same reasoning as the file input it sits beside,
// carried one step further: if the bytes have to travel anyway, the gesture that hands them over
// may as well be the one people already use for files. `useFileDrop` listens on the window rather
// than on this panel, so a drop lands wherever the operator happens to be looking — and it is the
// framework's, so the `dragover`-must-not-set-state rule (`docs/constraints.md` §5) is kept in one
// place rather than re-remembered here.
//
// A `.elf` is accepted too and flattened server-side. It is at least as likely to be the file
// somebody sends as the `.bin` — it is what a debugger session leaves behind and what CI publishes
// first — and refusing it for its extension when the bytes are right there is a poor answer.

import { Badge, Banner, Button, DropOverlay, DropStrip, Panel, useFileDrop } from '@auroravision/av-gui/controls';
import { useEffect, useState } from 'react';
import { AlertTriangle, Check, Eraser, Film, HardDriveUpload, Play, RefreshCw } from './icons';
import { api } from './model';

interface Artefact {
  name: string;
  path: string;
  bytes: number;
}

export function FirmwarePanel({ col }: { col: number | null }) {
  const [artefacts, setArtefacts] = useState<Artefact[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [status, setStatus] = useState<{ tone: 'info' | 'warn' | 'error'; text: string } | null>(
    null,
  );
  const [armedErase, setArmedErase] = useState(false);

  const refresh = () => {
    api<{ artefacts: Artefact[] }>('/api/router/firmware')
      .then((doc) => setArtefacts(doc.artefacts))
      .catch(() => setArtefacts([]));
  };
  useEffect(refresh, []);
  useEffect(() => {
    if (!armedErase) return;
    const timer = setTimeout(() => setArmedErase(false), 3000);
    return () => clearTimeout(timer);
  }, [armedErase]);

  const scope = col == null ? 'every connected column' : `column ${col + 1}`;

  // Mounted per panel, and that is fine: the mass and per-column views are never on screen at the
  // same time, so exactly one of these is ever listening.
  const drop = useFileDrop({
    onFiles: (files) => {
      for (const file of files) void upload(file);
    },
  });

  const upload = async (file: File) => {
    const extension = file.name.slice(file.name.lastIndexOf('.')).toLowerCase();
    if (extension !== '.bin' && extension !== '.elf') {
      setStatus({ tone: 'error', text: `${file.name} is not a firmware image — drop a .bin or a .elf` });
      return;
    }
    try {
      const bytes = await file.arrayBuffer();
      const result = await api<{ ok: boolean; path?: string; error?: string }>(
        `/api/router/firmware?name=${encodeURIComponent(file.name)}`,
        { method: 'POST', body: bytes },
      );
      if (result.ok && result.path) {
        setSelected(result.path);
        setStatus({ tone: 'info', text: `${file.name} uploaded (${file.size} bytes)` });
        refresh();
      } else {
        setStatus({ tone: 'error', text: result.error ?? 'upload failed' });
      }
    } catch (error) {
      setStatus({ tone: 'error', text: String(error) });
    }
  };

  const operate = async (op: 'flash' | 'erase' | 'run') => {
    const body =
      op === 'flash'
        ? { op, path: selected, col: col ?? undefined }
        : { op, col: col ?? undefined };
    try {
      const result = await api<{ ok: boolean; error?: string }>('/api/router/firmware/flash', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      });
      setStatus(
        result.ok
          ? { tone: 'info', text: `${op} queued for ${scope} — watch the outbox drain` }
          : { tone: 'error', text: result.error ?? `${op} refused` },
      );
    } catch (error) {
      setStatus({ tone: 'error', text: String(error) });
    }
  };

  return (
    <div data-av-surface="firmware">
      <DropOverlay
        open={drop.dragging}
        drop={drop}
        title="Drop firmware to upload it"
        hint="A .bin or .elf. An ELF is flattened to its flash image on the way in."
      />
      <Panel title={<><HardDriveUpload />{col == null ? 'Mass firmware update' : 'Firmware update'}</>}>
        <div className="device-picker">
          {artefacts.map((artefact) => (
            <button
              key={artefact.path}
              type="button"
              className="choice-row"
              data-selected={selected === artefact.path}
              onClick={() => setSelected(artefact.path)}
            >
              <span className="choice-mark">{selected === artefact.path && <Check />}</span>
              <span className="choice-copy">
                <strong>{artefact.name}</strong>
                <small>{artefact.bytes} bytes · {artefact.path}</small>
              </span>
            </button>
          ))}
          {artefacts.length === 0 && (
            <p className="placeholder">No .bin artefacts found — upload one below.</p>
          )}
        </div>
        <DropStrip
          accept=".bin,.elf"
          label="Drop firmware here"
          hint="A .bin or .elf — anywhere on the window works too."
          onFiles={(files) => {
            for (const file of files) void upload(file);
          }}
        />
        <div className="row wrap">
          <Button variant="quiet" onClick={refresh}>
            <RefreshCw />
            Rescan
          </Button>
        </div>
        <div className="row wrap">
          <Button variant="primary" disabled={!selected} onClick={() => void operate('flash')}>
            <HardDriveUpload />
            Flash {col == null ? 'all' : `column ${col + 1}`}
          </Button>
          <Button
            variant="danger"
            onClick={() => {
              if (armedErase) {
                setArmedErase(false);
                void operate('erase');
              } else {
                setArmedErase(true);
              }
            }}
          >
            {armedErase ? <AlertTriangle /> : <Eraser />}
            {armedErase ? 'Press again to erase' : 'Erase flash'}
          </Button>
          <Button variant="quiet" onClick={() => void operate('run')}>
            <Play />
            Run application
          </Button>
          {col == null && <Badge variant="plain">10 ms gap · 6 repetitions</Badge>}
        </div>
        {status && <Banner tone={status.tone}>{status.text}</Banner>}
      </Panel>
    </div>
  );
}

/** Server-side media listing for the FilePlayer source (plus a free path field). */
export function FilePicker({ index }: { index: number }) {
  const [files, setFiles] = useState<Artefact[]>([]);
  const [path, setPath] = useState('');
  useEffect(() => {
    api<{ files: Artefact[] }>('/api/router/files')
      .then((doc) => setFiles(doc.files))
      .catch(() => setFiles([]));
  }, []);
  const choose = (file: string) =>
    api('/api/router/command', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ op: 'set_source_params', index, params: { file } }),
    }).catch(() => {});
  return (
    <div className="device-picker">
      {files.map((file) => (
        <button
          key={file.path}
          type="button"
          className="choice-row"
          onClick={() => void choose(file.path)}
        >
          <span className="choice-mark" />
          <span className="choice-copy">
            <strong>{file.name}</strong>
            <small>{(file.bytes / (1024 * 1024)).toFixed(1)} MB</small>
          </span>
        </button>
      ))}
      <div className="row wrap">
        <input
          className="custom-device"
          placeholder="path to a video file…"
          value={path}
          onChange={(event) => setPath(event.target.value)}
          aria-label="Video file path"
        />
        <Button variant="quiet" disabled={!path} onClick={() => void choose(path)}>
          <Film />
          Load
        </Button>
      </div>
    </div>
  );
}
