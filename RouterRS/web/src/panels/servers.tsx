// The Servers tab: the legacy C++-era control surfaces (OSC receive, REST) with live status
// and the wire reference an integrator needs. Ports are config-file territory
// (config.json "Receiver"/"Server"), shown here as observed facts.

import { Badge, Panel } from '@auroravision/av-gui/controls';
import { TimeSeries } from '@auroravision/av-gui/charts';
import { useTelemetry } from '@auroravision/av-gui/runtime';
import { Fact } from '../bits';
import { Radio, Server, Terminal } from '../icons';
import { useBool, useNumber } from '../model';

function OscHistory() {
  const { ringIndex } = useTelemetry('/tel/osc');
  if (ringIndex < 0) return null;
  return <TimeSeries channels={[ringIndex]} height={90} />;
}

export function ServersPanel() {
  const oscRunning = useBool('/servers/osc/running');
  const oscPort = useNumber('/servers/osc/port');
  const restRunning = useBool('/servers/rest/running');
  const restPort = useNumber('/servers/rest/port');
  return (
    <div className="stack" data-av-surface="servers">
      <Panel
        title={<><Radio />OSC receiver</>}
        right={<Badge tone={oscRunning ? 'ok' : 'error'}>{oscRunning ? 'listening' : 'off'}</Badge>}
      >
        <Fact label="UDP port" value={oscPort || '—'} />
        <OscHistory />
        <details>
          <summary>Route reference</summary>
          <pre className="wire-reference">{`/move <col> <portal> <x> <y>
/unwind [col [portal]]
/motionProfile <maxVel> [accel]
/setCurrent <amps>
/homeAndZeroLocal
/disableLights
/axesMoveBlock <c0> <c1> <p0> <p1> <a b>...
/axesMoveByInidices <col> <idx> <a> <b>...   (sic)
/<action> | /<col>/<action> | /<col>/<portal>/<action>`}</pre>
        </details>
      </Panel>
      <Panel
        title={<><Server />REST server</>}
        right={
          <Badge tone={restRunning ? 'ok' : 'error'}>{restRunning ? 'listening' : 'off'}</Badge>
        }
      >
        <Fact label="HTTP port" value={restPort || '—'} />
        <details>
          <summary>Route reference</summary>
          <pre className="wire-reference">{`GET /
GET /<col>/<portal>/setPosition/<x>,<y>
GET /<col>/<portal>/getPosition
GET /<col>/<portal>/getTargetPosition
GET /<col>/<portal>/isInPosition
GET /<col>/<portal>/pollPosition
GET /<col>/<portal>/push`}</pre>
        </details>
      </Panel>
      <Panel title={<><Terminal />Agent API</>}>
        <p className="hint-copy">
          Agents drive this router over <code>/api/router/*</code> on this same origin —
          state, diagnostics, logs, ports, and a typed <code>POST /api/router/command</code>.
        </p>
      </Panel>
    </div>
  );
}
