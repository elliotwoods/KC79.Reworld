// The Renderer tab: the composited preview and the source stack. Source cards bind by
// `/sources/N/*` path; adding or removing a source re-seals the schema and the store carries
// subscriptions over by path.

import {
  Badge,
  EnumSelect,
  NumberField,
  Panel,
  Row,
  TextField,
  Toggle,
} from '@auroravision/av-gui/controls';
import { useParam } from '@auroravision/av-gui/runtime';
import { Action, Fact } from '../bits';
import { ImagePreview } from '../canvas';
import { FilePicker } from '../firmware';
import { AlertTriangle, Eye, iconForSourceType, Image, Play } from '../icons';
import { useBool, useNumber, useText, useVec2 } from '../model';

function SourceCard({ index }: { index: number }) {
  const type = useText(`/sources/${index}/type`);
  const visible = useParam<boolean>(`/sources/${index}/visible`);
  const renderEnabled = useParam<boolean>(`/sources/${index}/render_enabled`);
  if (!type) return null;
  const TypeGlyph = iconForSourceType(type);
  return (
    <div className={`source-card is-${type.toLowerCase()}`}>
      <div className="source-card-header">
        {/* The type glyph is how a card is found in a tall stack — the word is set small and
            every card's header is otherwise the same shape. */}
        <span className="source-type">
          <TypeGlyph />
          {type}
        </span>
        <button
          type="button"
          className="icon-toggle"
          data-on={!!visible.value}
          title="Visible in preview"
          aria-label="Visible in preview"
          onClick={() => visible.set(!visible.value)}
        >
          <Eye />
        </button>
        <button
          type="button"
          className="icon-toggle"
          data-on={!!renderEnabled.value}
          title="Render enabled"
          aria-label="Render enabled"
          onClick={() => renderEnabled.set(!renderEnabled.value)}
        >
          <Play />
        </button>
        <span className="source-remove">
          {/* `remove` resolves to the X glyph from the action map, so the button has no text
              child at all — hence the explicit label. */}
          <Action path={`/sources/${index}/actions/remove`} variant="quiet" aria-label="Remove source" />
        </span>
      </div>
      <Row label="Alpha">
        <NumberField path={`/sources/${index}/alpha`} />
      </Row>
      <Row label="Style">
        <EnumSelect path={`/sources/${index}/style`} />
      </Row>
      {type === 'Gradient' && (
        <>
          <Row label="Type">
            <EnumSelect path={`/sources/${index}/gradient_type`} />
          </Row>
          <Row label="Wave">
            <EnumSelect path={`/sources/${index}/wave`} />
          </Row>
          <Row label="Frequency">
            <NumberField path={`/sources/${index}/frequency`} />
          </Row>
          <Row label="Speed">
            <NumberField path={`/sources/${index}/speed`} />
          </Row>
          <Row label="Value 1 x">
            <NumberField path={`/sources/${index}/value1`} lane={0} />
          </Row>
          <Row label="Value 1 y">
            <NumberField path={`/sources/${index}/value1`} lane={1} />
          </Row>
          <Row label="Value 2 x">
            <NumberField path={`/sources/${index}/value2`} lane={0} />
          </Row>
          <Row label="Value 2 y">
            <NumberField path={`/sources/${index}/value2`} lane={1} />
          </Row>
        </>
      )}
      {type === 'Text' && (
        <>
          <Row label="Text">
            <TextField path={`/sources/${index}/text`} />
          </Row>
          <Row label="Font">
            <TextField path={`/sources/${index}/font`} />
          </Row>
          <Row label="Size">
            <NumberField path={`/sources/${index}/size`} />
          </Row>
          <Row label="Border">
            <NumberField path={`/sources/${index}/border`} />
          </Row>
          <Row label="Inverse">
            <Toggle path={`/sources/${index}/inverse`} />
          </Row>
        </>
      )}
      {type === 'FilePlayer' && <FilePlayerRows index={index} />}
      {type === 'Spout' && <SpoutRows index={index} />}
    </div>
  );
}

function SpoutRows({ index }: { index: number }) {
  const status = useText(`/sources/${index}/status`);
  return (
    <>
      <Row label="Sender">
        <TextField path={`/sources/${index}/sender_name`} />
      </Row>
      <Fact label="Status" value={status || '—'} />
    </>
  );
}

function FilePlayerRows({ index }: { index: number }) {
  const file = useText(`/sources/${index}/file`);
  const loaded = useBool(`/sources/${index}/loaded`);
  const duration = useNumber(`/sources/${index}/duration_s`);
  const error = useText(`/sources/${index}/error`);
  return (
    <>
      <Fact
        label="File"
        value={file ? file.split(/[\\/]/).pop() : '—'}
        tone={error ? 'error' : undefined}
      />
      <div className="row wrap">
        <Badge tone={loaded ? 'ok' : 'idle'}>{loaded ? 'loaded' : 'no video'}</Badge>
        {duration > 0 && <Badge variant="plain">{duration.toFixed(1)} s</Badge>}
      </div>
      {error && (
        <p className="source-error">
          <AlertTriangle />
          {error}
        </p>
      )}
      <Row label="Play">
        <Toggle path={`/sources/${index}/play`} />
      </Row>
      <Row label="Loop mode">
        <EnumSelect path={`/sources/${index}/loop_mode`} />
      </Row>
      <Row label="Speed">
        <NumberField path={`/sources/${index}/speed`} />
      </Row>
      <Row label="Position">
        <NumberField path={`/sources/${index}/position`} />
      </Row>
      <div className="row wrap">
        <Action path={`/sources/${index}/actions/jump_to_start`}>Jump to start</Action>
        <Action path={`/sources/${index}/actions/clear_file`}>Clear</Action>
      </div>
      <FilePicker index={index} />
    </>
  );
}

export function RendererPanel() {
  const [w, h] = useVec2('/installation/resolution');
  // Sources are discovered by probing declared paths; the schema re-seals on add/remove.
  const count = useSourceCount();
  return (
    <div className="stack" data-av-surface="renderer">
      <Panel
        title={<><Image />Composited output</>}
        right={<Badge variant="plain">{`${Math.round(w)} × ${Math.round(h)}`}</Badge>}
      >
        <ImagePreview height={200} />
        <Row label="Image sampling">
          <Toggle path="/installation/image_enabled" />
        </Row>
      </Panel>
      <div className="source-strip">
        {Array.from({ length: count }, (_, i) => (
          <SourceCard key={i} index={i} />
        ))}
        <div className="source-add">
          <span className="source-add-label">Add source</span>
          <div className="row wrap">
            <Action path="/sources/actions/add_gradient">Gradient</Action>
            <Action path="/sources/actions/add_text">Text</Action>
            <Action path="/sources/actions/add_file_player">File player</Action>
            <Action path="/sources/actions/add_spout">Spout</Action>
          </div>
        </div>
      </div>
    </div>
  );
}

/** How many `/sources/N/type` params the current schema declares. */
function useSourceCount(): number {
  // Probe a generous upper bound; missing paths cost nothing (decl is undefined).
  const probes = [
    useParam(`/sources/0/type`),
    useParam(`/sources/1/type`),
    useParam(`/sources/2/type`),
    useParam(`/sources/3/type`),
    useParam(`/sources/4/type`),
    useParam(`/sources/5/type`),
    useParam(`/sources/6/type`),
    useParam(`/sources/7/type`),
  ];
  let count = 0;
  for (const probe of probes) {
    if (probe.decl) count += 1;
    else break;
  }
  return count;
}
