/**
 * The session log, pinned to the bottom of the window.
 *
 * Everything the bench has heard, in one place: the module's own log lines (firmware prose over
 * the serial menu, or the drained log outbox over RS485), the bench's own notes, and faults.
 * They are interleaved in arrival order deliberately — "the link opened, then the module said
 * this" is the sequence that answers most questions, and splitting the two streams into
 * separate panes is how you lose it.
 *
 * # Why this is fetched, not a parameter
 *
 * A log is a document, not a control surface. It travels over `/api/bench/log?from=<cursor>`,
 * which is the same endpoint an agent reads — so the page and `ptb` see exactly the same
 * stream, and neither can quietly diverge from what the worker recorded.
 *
 * The cursor is what makes it cheap: each poll asks only for what it has not already seen, so a
 * page left open for an eight-hour soak is not re-downloading the whole session every second.
 */

import { useEffect, useLayoutEffect, useRef, useState } from 'react';

export interface LogLine {
  seq: number;
  at_ms: number;
  level: number;
  source: string;
  message: string;
}

/** How many lines the page keeps. The NDJSON session file is the durable record. */
const MAX_LINES = 2000;

const POLL_MS = 400;

/** Firmware log levels, from `PortalFW/src/Logger.h`. */
function toneFor(level: number): string {
  if (level >= 20) return 'error';
  if (level >= 10) return 'warn';
  return 'status';
}

function timestamp(atMs: number): string {
  const total = Math.floor(atMs / 1000);
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${String(minutes).padStart(2, '0')}:${String(seconds).padStart(2, '0')}`;
}

export function SessionLog() {
  const [lines, setLines] = useState<LogLine[]>([]);
  const [dropped, setDropped] = useState(0);
  const cursor = useRef(0);
  const scroller = useRef<HTMLDivElement>(null);
  /**
   * Whether to follow the tail.
   *
   * Set false the moment someone scrolls up: yanking the view back to the bottom while a person
   * is reading an error that scrolled past is the single most irritating thing a log pane can
   * do. Re-arms when they scroll back down.
   */
  const following = useRef(true);

  useEffect(() => {
    let cancelled = false;

    const poll = async () => {
      try {
        const response = await fetch(`/api/bench/log?from=${cursor.current}`, { cache: 'no-store' });
        if (!response.ok || cancelled) return;
        const body = (await response.json()) as { next: number; lines: LogLine[] };
        if (cancelled || !body.lines?.length) {
          if (body?.next !== undefined) cursor.current = body.next;
          return;
        }
        cursor.current = body.next;
        setLines((previous) => {
          const merged = [...previous, ...body.lines];
          if (merged.length <= MAX_LINES) return merged;
          // Count what scrolled off rather than dropping it silently: a gap the reader does not
          // know about turns a partial record into a wrong one.
          setDropped((d) => d + (merged.length - MAX_LINES));
          return merged.slice(merged.length - MAX_LINES);
        });
      } catch {
        // The bench is gone or restarting. The status bar already says so; a log pane that
        // spams its own connection errors buries the module's output, which is the point of it.
      }
    };

    void poll();
    const timer = setInterval(() => void poll(), POLL_MS);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, []);

  // Before paint, so the tail does not visibly jump.
  useLayoutEffect(() => {
    const element = scroller.current;
    if (element && following.current) {
      element.scrollTop = element.scrollHeight;
    }
  }, [lines]);

  const onScroll = () => {
    const element = scroller.current;
    if (!element) return;
    const distance = element.scrollHeight - element.scrollTop - element.clientHeight;
    following.current = distance < 24;
  };

  return (
    <section className="session-log" data-av-surface="session-log">
      <header className="session-log-head">
        <span className="label-caps">Session log</span>
        <span className="session-log-count">
          {lines.length} line{lines.length === 1 ? '' : 's'}
          {dropped > 0 && ` · ${dropped} scrolled off`}
        </span>
      </header>
      <div className="session-log-lines" ref={scroller} onScroll={onScroll}>
        {lines.length === 0 ? (
          <p className="session-log-empty">
            Nothing yet. Connect a transport — the module&apos;s own output appears here.
          </p>
        ) : (
          lines.map((line) => (
            <div key={line.seq} className="session-log-line" data-tone={toneFor(line.level)}>
              <span className="session-log-time">{timestamp(line.at_ms)}</span>
              <span className="session-log-source">{line.source}</span>
              <span className="session-log-message">{line.message}</span>
            </div>
          ))
        )}
      </div>
    </section>
  );
}
