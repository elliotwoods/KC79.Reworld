// The Router control page: title bar, workspace tabs, the centre panel, the selection
// inspector, and the status bar. Layout only — every control binds a schema path and every
// live mark reads a telemetry ring.

import { EnumSelect, PageBar, StatusBar, StatusItem, TitleBar, Toggle } from '@auroravision/av-gui/controls';
import { mount, usePage, useParam, useSchema } from '@auroravision/av-gui/runtime';
import '@auroravision/av-gui/styles.css';
import { useEffect, useRef } from 'react';
import { Action } from './bits';
import {
  AlertTriangle,
  ArrowUpDown,
  Cable,
  FileText,
  HeartPulse,
  House,
  Image,
  Network,
  Radio,
  Server,
  Terminal,
} from './icons';
import { Inspector, useKeyboardShortcuts } from './inspector';
import { formatBytes } from './math';
import { useBool, useEnumName, useNumber, useText } from './model';
import { DiagnosticsPanel } from './panels/diagnostics';
import { InstallationPanel } from './panels/installation';
import { RendererPanel } from './panels/renderer';
import { ServersPanel } from './panels/servers';

type Tab = 'installation' | 'renderer' | 'servers' | 'diagnostics';

// The four workspaces are Pages on the framework bar at the bottom of the window (av-frameworks
// `docs/patterns.md` §18). The icons stay this app's own set; the bar takes any node.
const PAGES = [
  { id: 'installation', label: 'Installation', icon: <House /> },
  { id: 'renderer', label: 'Renderer', icon: <Image /> },
  { id: 'servers', label: 'Servers', icon: <Network /> },
  { id: 'diagnostics', label: 'Diagnostics', icon: <HeartPulse /> },
] as const satisfies ReadonlyArray<{ id: Tab; label: string; icon: unknown }>;

function useHeartbeat() {
  // The page's liveness counter, bumped once a second while any page is open.
  const heartbeat = useParam<number>('/ui/heartbeat');
  const ref = useRef(heartbeat);
  ref.current = heartbeat;
  useEffect(() => {
    const timer = setInterval(() => ref.current.set((ref.current.value ?? 0) + 1), 1000);
    return () => clearInterval(timer);
  }, []);
}

function App() {
  const schema = useSchema();
  const [tab, setTab] = usePage<Tab>('router', PAGES, 'installation');
  const simulated = useBool('/app/simulated');
  const transmit = useEnumName('/installation/messaging/transmit');
  const connectedColumns = useConnectedColumns();
  const totalColumns = useNumber('/installation/arrangement/columns');
  const txRate = useNumber('/stats/tx_per_s');
  const rxRate = useNumber('/stats/rx_per_s');
  const faulty = useNumber('/health/faulty_units');
  const oscRunning = useBool('/servers/osc/running');
  const oscPort = useNumber('/servers/osc/port');
  const restRunning = useBool('/servers/rest/running');
  const restPort = useNumber('/servers/rest/port');
  const sessionFile = useText('/report/session_file');
  const fileBytes = useNumber('/report/file_bytes');
  const verbose = useBool('/report/verbose');
  useKeyboardShortcuts();

  return (
    <div className="app app--filled router">
      <TitleBar
        title="Router"
        sub={schema ? (simulated ? 'simulated installation' : transmit.toLowerCase()) : 'connecting'}
      />
      <div className="router-topbar">
        <span className="topbar-controls">
          <label className="topbar-field">
            Transmit
            <EnumSelect path="/installation/messaging/transmit" />
          </label>
          <label className="topbar-field">
            Image
            <Toggle path="/installation/image_enabled" />
          </label>
          <Action path="/installation/actions/save_config" variant="quiet">
            Save config
          </Action>
          {simulated && <span className="chip is-warn">SIM</span>}
        </span>
      </div>
      <div className="router-main">
        <main className="router-center">
          {tab === 'installation' && <InstallationPanel />}
          {tab === 'renderer' && <RendererPanel />}
          {tab === 'servers' && <ServersPanel />}
          {tab === 'diagnostics' && <DiagnosticsPanel />}
        </main>
        <Inspector />
      </div>
      <PageBar
        label="Router pages"
        pages={PAGES.map((page) => (page.id === 'diagnostics' ? { ...page, count: faulty || undefined } : page))}
        value={tab}
        onChange={setTab}
      />
      <div data-av-surface="status">
        <StatusBar stream={null}>
          {/* The glyph leads each label rather than replacing it. A status bar is read at a
              glance for *change*, and the icon is what the eye lands on first; the word is
              still there because "cable" and "antenna" are not distinguishable at 12px. */}
          <StatusItem
            label={<><Cable />columns</>}
            value={`${connectedColumns}/${totalColumns}`}
            tone={connectedColumns === totalColumns ? 'ok' : connectedColumns > 0 ? 'warn' : 'error'}
          />
          <StatusItem label={<><ArrowUpDown />tx/rx</>} value={`${txRate.toFixed(0)} / ${rxRate.toFixed(0)} per s`} />
          {faulty > 0 && <StatusItem label={<><AlertTriangle />faulty</>} value={String(faulty)} tone="error" />}
          <StatusItem label={<><Radio />OSC</>} value={oscRunning ? `:${oscPort}` : 'off'} tone={oscRunning ? 'ok' : 'warn'} />
          <StatusItem label={<><Server />REST</>} value={restRunning ? `:${restPort}` : 'off'} tone={restRunning ? 'ok' : 'warn'} />
          <StatusItem
            label={<><FileText />session</>}
            value={`${sessionFile.split(/[\\/]/).pop() ?? '—'} · ${formatBytes(fileBytes)}`}
          />
          {verbose && <StatusItem label={<><Terminal />log</>} value="VERBOSE" tone="warn" />}
        </StatusBar>
      </div>
    </div>
  );
}

function useConnectedColumns(): number {
  // Probe the first eight declared columns; count connected ones.
  const flags = [
    useBool('/columns/0/rs485/connected'),
    useBool('/columns/1/rs485/connected'),
    useBool('/columns/2/rs485/connected'),
    useBool('/columns/3/rs485/connected'),
    useBool('/columns/4/rs485/connected'),
    useBool('/columns/5/rs485/connected'),
    useBool('/columns/6/rs485/connected'),
    useBool('/columns/7/rs485/connected'),
  ];
  const total = useNumber('/installation/arrangement/columns');
  return flags.slice(0, Math.max(0, total)).filter(Boolean).length;
}

function Root() {
  useHeartbeat();
  return <App />;
}

mount(<Root />);
