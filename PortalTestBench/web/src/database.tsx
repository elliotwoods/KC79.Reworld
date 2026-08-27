import { Badge, Banner, Button, ConfirmDialog, EmptyState, Panel } from '@auroravision/av-gui/controls';
import { useParam } from '@auroravision/av-gui/runtime';
import { useEffect, useMemo, useState } from 'react';

interface DeviceRecord { uid: string; idcode: string; dev_id: string; flash_kb: number; option_bytes: string; probe_name: string; probe_serial: string; probe_firmware: string; first_seen_ms: number; last_seen_ms: number }
interface Association { serial: number; uid: string; status: string; active: boolean; created_ms: number; updated_ms: number }
interface Attempt { id: number; at_ms: number; serial: number; uid: string; firmware_version: string; bootloader_sha256: string; application_sha256: string; bundle_sha256: string; provenance: string; outcome: string; detail: string }
interface ActionRow { id: number; at_ms: number; serial?: number; uid?: string; action: string; outcome: string; detail: string }
interface Library { database_ok: boolean; database_error: string; next_serial: number; summary: { devices: number; active: number; provisioned: number; attention: number }; devices: DeviceRecord[]; associations: Association[]; attempts: Attempt[]; actions: ActionRow[] }
interface DeviceRow { device: DeviceRecord; binding?: Association; attempt?: Attempt; searchable: string }
type Correction = { kind: 'reassign'; serial: number; uid: string } | { kind: 'release'; serial: number; uid: string } | { kind: 'next'; serial: number };

const EMPTY: Library = { database_ok: false, database_error: '', next_serial: 1, summary: { devices: 0, active: 0, provisioned: 0, attention: 0 }, devices: [], associations: [], attempts: [], actions: [] };

async function postCommand(body: Record<string, unknown>) {
  const response = await fetch('/api/bench/command', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error((await response.json()).error ?? `request refused (${response.status})`);
}

function download(name: string, body: string, type: string) {
  const url = URL.createObjectURL(new Blob([body], { type }));
  const anchor = document.createElement('a'); anchor.href = url; anchor.download = name; anchor.click(); URL.revokeObjectURL(url);
}

const csv = (value: unknown) => `"${String(value ?? '').replaceAll('"', '""')}"`;
const ago = (at: number) => at > 0 ? new Date(at).toLocaleString() : '—';
const short = (uid: string) => uid.length > 12 ? `…${uid.slice(-12)}` : uid;
const tone = (status?: string) => status === 'provisioned' ? 'ok' : status === 'failed' ? 'error' : status === 'reserved' ? 'warn' : 'idle';
const validSerial = (value: string) => Number.isInteger(Number(value)) && Number(value) > 0 && Number(value) < 0xffff_ffff;

export function DatabaseTab() {
  const fixtureUid = useParam<string>('/mcu/uid').value ?? '';
  const provisionSerial = useParam<number>('/provision/serial_to_provision');
  const [library, setLibrary] = useState<Library>(EMPTY);
  const [loadError, setLoadError] = useState('');
  const [query, setQuery] = useState('');
  const [status, setStatus] = useState('all');
  const [sort, setSort] = useState<'recent' | 'serial' | 'uid'>('recent');
  const [selectedUid, setSelectedUid] = useState('');
  const [page, setPage] = useState(0);
  const [correction, setCorrection] = useState<Correction | null>(null);
  const [serialDraft, setSerialDraft] = useState('');
  const [nextDraft, setNextDraft] = useState('');
  const [note, setNote] = useState('');
  const [notice, setNotice] = useState('');

  useEffect(() => {
    let active = true;
    const load = async () => {
      try {
        const response = await fetch('/api/bench/provision/library', { cache: 'no-store' });
        if (!response.ok) throw new Error(`library unavailable (${response.status})`);
        const value = await response.json() as Library;
        if (active) { setLibrary(value); setLoadError(''); setNextDraft((current) => current || String(value.next_serial)); }
      } catch (error) { if (active) setLoadError(String(error)); }
    };
    void load(); const id = window.setInterval(load, 2000);
    return () => { active = false; window.clearInterval(id); };
  }, []);

  const rows = useMemo(() => {
    const needle = query.trim().toLowerCase();
    const uids = new Set([...library.devices.map((item) => item.uid), ...library.associations.map((item) => item.uid), ...library.attempts.map((item) => item.uid), ...library.actions.flatMap((item) => item.uid ? [item.uid] : [])]);
    const result: DeviceRow[] = [...uids].map((uid) => {
      const device = library.devices.find((item) => item.uid === uid) ?? { uid, idcode: '', dev_id: '', flash_kb: 0, option_bytes: '', probe_name: '', probe_serial: '', probe_firmware: '', first_seen_ms: 0, last_seen_ms: 0 };
      const binding = library.associations.find((item) => item.uid === device.uid && item.active);
      const attempt = library.attempts.find((item) => item.uid === device.uid);
      const related = library.actions.filter((item) => item.uid === device.uid).slice(0, 10);
      const searchable = [device.uid, device.idcode, device.dev_id, device.probe_name, device.probe_serial, binding?.serial, binding?.status, attempt?.firmware_version, attempt?.outcome, attempt?.bundle_sha256, ...related.flatMap((item) => [item.action, item.outcome, item.detail])].join(' ').toLowerCase();
      return { device, binding, attempt, searchable };
    }).filter((row) => (!needle || row.searchable.includes(needle)) && (status === 'all' || (status === 'unbound' ? !row.binding : row.binding?.status === status)));
    result.sort((a, b) => sort === 'serial' ? (a.binding?.serial ?? Number.MAX_SAFE_INTEGER) - (b.binding?.serial ?? Number.MAX_SAFE_INTEGER) : sort === 'uid' ? a.device.uid.localeCompare(b.device.uid) : b.device.last_seen_ms - a.device.last_seen_ms);
    return result;
  }, [library, query, status, sort]);
  useEffect(() => { setPage(0); }, [query, status, sort]);
  const pages = Math.max(1, Math.ceil(rows.length / 50));
  const visible = rows.slice(page * 50, page * 50 + 50);
  const selected = rows.find((row) => row.device.uid === selectedUid) ?? rows.find((row) => row.device.uid === fixtureUid) ?? rows[0];
  const associations = selected ? library.associations.filter((row) => row.uid === selected.device.uid) : [];
  const attempts = selected ? library.attempts.filter((row) => row.uid === selected.device.uid) : [];
  const actions = selected ? library.actions.filter((row) => row.uid === selected.device.uid || (selected.binding && row.serial === selected.binding.serial)) : [];

  const applyCorrection = async () => {
    if (!correction) return;
    try {
      if (correction.kind === 'reassign') await postCommand({ op: 'reassign_library_serial', serial: correction.serial, uid: correction.uid });
      if (correction.kind === 'release') await postCommand({ op: 'supersede_library_binding', serial: correction.serial, uid: correction.uid });
      if (correction.kind === 'next') await postCommand({ op: 'set_next_serial', serial: correction.serial });
      setNotice('Correction queued. The audit timeline will update when the worker commits it.'); setCorrection(null);
    } catch (error) { setNotice(String(error)); setCorrection(null); }
  };
  const exportRows = (format: 'json' | 'csv') => {
    const data = rows.map(({ device, binding, attempt }) => ({ uid: device.uid, serial: binding?.serial, status: binding?.status ?? 'unbound', last_seen_ms: device.last_seen_ms, idcode: device.idcode, dev_id: device.dev_id, probe: device.probe_name, firmware: attempt?.firmware_version, outcome: attempt?.outcome }));
    if (format === 'json') download('portal-hardware-library-filtered.json', JSON.stringify(data, null, 2), 'application/json');
    else download('portal-hardware-library-filtered.csv', ['uid,serial,status,last_seen_ms,idcode,dev_id,probe,firmware,outcome', ...data.map((row) => Object.values(row).map(csv).join(','))].join('\n'), 'text/csv');
  };

  return <div className="database-workspace" data-av-surface="hardware-library">
    <Panel title="Hardware library" right={<Badge tone={library.database_ok ? 'ok' : 'error'}>{library.database_ok ? 'database online' : 'database offline'}</Badge>}>
      {(loadError || !library.database_ok) && <Banner tone="error">{loadError || library.database_error || 'The provisioning database is unavailable.'}</Banner>}
      {notice && <Banner tone="info">{notice}</Banner>}
      <div className="database-summary">
        <div><small>Known MCUs</small><strong>{library.summary.devices}</strong></div><div><small>Active bindings</small><strong>{library.summary.active}</strong></div><div><small>Provisioned</small><strong>{library.summary.provisioned}</strong></div><div data-attention={library.summary.attention > 0}><small>Need attention</small><strong>{library.summary.attention}</strong></div>
        <div className="database-next"><small>Next serial floor</small><strong>{library.next_serial}</strong><span><input type="number" min={library.next_serial + 1} value={nextDraft} onChange={(event) => setNextDraft(event.target.value)} aria-label="Next serial floor" /><Button disabled={!validSerial(nextDraft) || Number(nextDraft) <= library.next_serial} onClick={() => setCorrection({ kind: 'next', serial: Number(nextDraft) })}>Advance</Button></span></div>
      </div>
      {fixtureUid && <div className="fixture-library-link"><Badge tone="active">on fixture</Badge><code>{fixtureUid}</code><Button variant="quiet" onClick={() => { setSelectedUid(fixtureUid); setQuery(fixtureUid); }}>Focus record</Button></div>}
      <div className="database-toolbar"><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search serial, UID, firmware, hash, probe, or event…" aria-label="Search hardware library" /><select value={status} onChange={(event) => setStatus(event.target.value)} aria-label="Filter by binding status"><option value="all">All states</option><option value="provisioned">Provisioned</option><option value="reserved">Reserved</option><option value="failed">Failed</option><option value="unbound">Unbound</option></select><select value={sort} onChange={(event) => setSort(event.target.value as typeof sort)} aria-label="Sort hardware library"><option value="recent">Recently seen</option><option value="serial">Serial number</option><option value="uid">MCU UID</option></select><Button variant="quiet" onClick={() => exportRows('csv')}>CSV</Button><Button variant="quiet" onClick={() => exportRows('json')}>JSON</Button></div>
      <div className="database-explorer">
        <div className="database-list">{visible.length === 0 ? <EmptyState inline detail="No hardware records match this view." /> : <table><thead><tr><th>Serial</th><th>MCU</th><th>State</th><th>Latest evidence</th><th>Last seen</th></tr></thead><tbody>{visible.map((row) => <tr key={row.device.uid} data-selected={selected?.device.uid === row.device.uid || undefined} data-fixture={row.device.uid === fixtureUid || undefined} tabIndex={0} onClick={() => setSelectedUid(row.device.uid)} onKeyDown={(event) => { if (event.key === 'Enter' || event.key === ' ') setSelectedUid(row.device.uid); }}><td><code>{row.binding?.serial ?? '—'}</code></td><td><code title={row.device.uid}>{short(row.device.uid)}</code>{row.device.uid === fixtureUid && <Badge tone="active">fixture</Badge>}</td><td><Badge tone={tone(row.binding?.status)}>{row.binding?.status ?? 'unbound'}</Badge></td><td><strong>{row.attempt?.firmware_version || row.device.dev_id || 'observed'}</strong><small>{row.attempt?.outcome || `${row.device.flash_kb} kB`}</small></td><td><small>{ago(row.device.last_seen_ms)}</small></td></tr>)}</tbody></table>}
          <footer><span>{rows.length} records · page {page + 1} of {pages}</span><span><Button variant="quiet" disabled={page === 0} onClick={() => setPage((value) => value - 1)}>Previous</Button><Button variant="quiet" disabled={page + 1 >= pages} onClick={() => setPage((value) => value + 1)}>Next</Button></span></footer>
        </div>
        <div className="database-detail">{!selected ? <EmptyState inline detail="Select a hardware record to inspect it." /> : <>
          <header><div><small>MCU UID</small><code title={selected.device.uid}>{selected.device.uid}</code></div><Badge tone={tone(selected.binding?.status)}>{selected.binding?.status ?? 'unbound'}</Badge></header>
          <section><h3>Identity and fixture evidence</h3><dl><dt>IDCODE / DEV_ID</dt><dd>{selected.device.idcode || '—'} · {selected.device.dev_id || '—'}</dd><dt>Flash / option bytes</dt><dd>{selected.device.flash_kb} kB · {selected.device.option_bytes || '—'}</dd><dt>Probe</dt><dd>{[selected.device.probe_name, selected.device.probe_serial, selected.device.probe_firmware].filter(Boolean).join(' · ') || '—'}</dd><dt>Seen</dt><dd>{ago(selected.device.first_seen_ms)} — {ago(selected.device.last_seen_ms)}</dd></dl></section>
          <section><h3>Registry administration</h3><div className="database-admin"><input type="number" min="1" value={serialDraft} onChange={(event) => setSerialDraft(event.target.value)} placeholder={String(selected.binding?.serial ?? library.next_serial)} aria-label="Serial to assign" /><Button disabled={!validSerial(serialDraft) || Number(serialDraft) === selected.binding?.serial} onClick={() => setCorrection({ kind: 'reassign', serial: Number(serialDraft), uid: selected.device.uid })}>Assign serial</Button>{selected.binding && <Button variant="danger" onClick={() => setCorrection({ kind: 'release', serial: selected.binding!.serial, uid: selected.device.uid })}>Release binding</Button>}{selected.binding && <Button variant="quiet" disabled={selected.device.uid !== fixtureUid} title={selected.device.uid === fixtureUid ? 'Use this binding for the board on the fixture' : 'This MCU is not on the fixture'} onClick={() => provisionSerial.set(selected.binding!.serial)}>Use for fixture</Button>}</div><small>Registry corrections never write the physical board. A corrected binding remains reserved until provisioning verifies it.</small></section>
          <section><h3>Serial lineage</h3>{associations.length ? associations.map((row) => <div className="database-event" key={`${row.serial}-${row.created_ms}`}><code>{row.serial}</code><span><strong>{row.status}</strong><small>{ago(row.updated_ms)}</small></span><Badge tone={row.active ? tone(row.status) : 'idle'}>{row.active ? 'active' : 'historical'}</Badge></div>) : <small>No serial associations.</small>}</section>
          <section><h3>Flash attempts</h3>{attempts.length ? attempts.slice(0, 20).map((item) => <details key={item.id}><summary><span><strong>{item.firmware_version || 'unknown firmware'}</strong><small>{ago(item.at_ms)}</small></span><Badge tone={item.outcome === 'failed' ? 'error' : item.outcome.startsWith('pending') ? 'warn' : 'ok'}>{item.outcome}</Badge></summary><dl><dt>Bundle</dt><dd><code>{item.bundle_sha256 || '—'}</code></dd><dt>Bootloader</dt><dd><code>{item.bootloader_sha256 || '—'}</code></dd><dt>Application</dt><dd><code>{item.application_sha256 || '—'}</code></dd><dt>Provenance</dt><dd>{item.provenance || '—'}</dd><dt>Detail</dt><dd>{item.detail || '—'}</dd></dl></details>) : <small>No flash attempts recorded.</small>}</section>
          <section><h3>Audit timeline</h3><div className="database-note"><input value={note} onChange={(event) => setNote(event.target.value)} placeholder="Add an operator note…" /><Button disabled={!note.trim()} onClick={() => { void postCommand({ op: 'add_library_note', serial: selected.binding?.serial, uid: selected.device.uid, detail: note.trim() }).then(() => { setNote(''); setNotice('Note queued.'); }).catch((error) => setNotice(String(error))); }}>Add note</Button></div>{actions.length ? actions.slice(0, 40).map((item) => <div className="database-event" key={item.id}><small>{ago(item.at_ms)}</small><span><strong>{item.action}</strong><small>{item.detail || item.outcome}</small></span><Badge tone={item.outcome === 'failed' ? 'error' : item.outcome === 'pending' ? 'warn' : 'idle'}>{item.outcome}</Badge></div>) : <small>No audit events match this device.</small>}</section>
        </>}</div>
      </div>
    </Panel>
    <ConfirmDialog open={!!correction} title={correction?.kind === 'release' ? 'Release registry binding?' : correction?.kind === 'next' ? 'Advance the serial floor?' : 'Reassign registry serial?'} body={<><p>{correction?.kind === 'release' ? `Serial ${correction.serial} will become available, while its lineage remains in the database.` : correction?.kind === 'next' ? `Future automatic allocation will begin at serial ${correction.serial}. This cannot lower the counter.` : `Serial ${correction?.serial} will be reserved for MCU ${correction?.kind === 'reassign' ? correction.uid : ''}. Conflicting active bindings will be superseded.`}</p><Banner tone="warn">This changes the registry only. It does not write or verify any physical board.</Banner></>} confirmLabel={correction?.kind === 'release' ? 'Release binding' : 'Apply correction'} tone="danger" onConfirm={() => { void applyCorrection(); }} onCancel={() => setCorrection(null)} />
  </div>;
}
