// RouterReports: session report viewer for RouterRS.
//
//   node server.js [--reports <dir>] [--port <port>]
//
// Defaults: reports dir = ../RouterRS/reports (or REPORTS_DIR env),
// port 8090 (or PORT env).

import express from 'express';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { listSessions, sessionFiles } from './lib/sessions.js';
import { readEvents } from './lib/ndjson.js';
import { reduce } from './lib/reduce.js';
import { bucketize } from './lib/bucketize.js';
import { buildFilter } from './lib/filters.js';

const here = path.dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const argValue = (flag) => {
  const index = args.indexOf(flag);
  return index >= 0 ? args[index + 1] : null;
};

const REPORTS_DIR = path.resolve(
  argValue('--reports') ?? process.env.REPORTS_DIR ?? path.join(here, '..', 'RouterRS', 'reports')
);
const PORT = Number(argValue('--port') ?? process.env.PORT ?? 8090);

const app = express();
app.use(express.static(path.join(here, 'public')));

app.get('/api/sessions', (_req, res) => {
  res.json({ reportsDir: REPORTS_DIR, sessions: listSessions(REPORTS_DIR) });
});

function resolveSession(req, res) {
  const session = sessionFiles(REPORTS_DIR, req.params.id);
  if (!session) {
    res.status(404).json({ error: 'session not found' });
    return null;
  }
  return session;
}

// Serve the summary; regenerate from NDJSON if missing or older than the
// newest ndjson part (crashed session). The regenerated copy is cached.
app.get('/api/sessions/:id/summary', async (req, res) => {
  const session = resolveSession(req, res);
  if (!session) return;
  const ndjsonPaths = session.ndjsonFiles.map((f) => path.join(REPORTS_DIR, f));

  if (session.summaryFile && req.query.regenerate !== '1') {
    const summaryPath = path.join(REPORTS_DIR, session.summaryFile);
    const summaryMtime = fs.statSync(summaryPath).mtimeMs;
    if (summaryMtime >= session.modified - 1000) {
      res.sendFile(summaryPath);
      return;
    }
  }
  try {
    const summary = await reduce(readEvents(ndjsonPaths));
    const cachePath = path.join(REPORTS_DIR, `session-${session.id}.summary.json`);
    try {
      fs.writeFileSync(cachePath, JSON.stringify(summary, null, 2));
    } catch {
      // read-only dir is fine; serve without caching
    }
    res.json(summary);
  } catch (error) {
    res.status(500).json({ error: String(error) });
  }
});

// Filtered event stream (NDJSON out, incremental, hard-capped).
app.get('/api/sessions/:id/events', async (req, res) => {
  const session = resolveSession(req, res);
  if (!session) return;
  const filter = buildFilter(req.query);
  const limit = Math.min(Number(req.query.limit ?? 1000), 10_000);
  const offset = Number(req.query.offset ?? 0);

  res.setHeader('content-type', 'application/x-ndjson');
  let matched = 0;
  let sent = 0;
  const paths = session.ndjsonFiles.map((f) => path.join(REPORTS_DIR, f));
  for await (const ev of readEvents(paths)) {
    if (!filter(ev)) continue;
    matched += 1;
    if (matched <= offset) continue;
    res.write(JSON.stringify(ev) + '\n');
    sent += 1;
    if (sent >= limit) break;
  }
  res.end();
});

// Bucketed time series for charts.
app.get('/api/sessions/:id/timeline', async (req, res) => {
  const session = resolveSession(req, res);
  if (!session) return;
  const bucketMs = Math.max(Number(req.query.bucket_ms ?? 60_000), 1000);
  const col = req.query.col != null && req.query.col !== '' ? Number(req.query.col) : null;
  const paths = session.ndjsonFiles.map((f) => path.join(REPORTS_DIR, f));
  try {
    res.json(await bucketize(readEvents(paths), bucketMs, col));
  } catch (error) {
    res.status(500).json({ error: String(error) });
  }
});

// Self-contained static HTML export (summary + timeline inlined).
app.get('/api/sessions/:id/export', async (req, res) => {
  const session = resolveSession(req, res);
  if (!session) return;
  const paths = session.ndjsonFiles.map((f) => path.join(REPORTS_DIR, f));
  try {
    const summary = await reduce(readEvents(paths));
    const timeline = await bucketize(readEvents(paths), 60_000);
    const template = fs.readFileSync(path.join(here, 'public', 'export-template.html'), 'utf8');
    const html = template
      .replace('/*__SUMMARY__*/null', JSON.stringify(summary))
      .replace('/*__TIMELINE__*/null', JSON.stringify(timeline));
    res.setHeader('content-type', 'text/html');
    res.setHeader(
      'content-disposition',
      `attachment; filename="router-report-${session.id}.html"`
    );
    res.send(html);
  } catch (error) {
    res.status(500).json({ error: String(error) });
  }
});

app.listen(PORT, () => {
  console.log(`RouterReports: http://localhost:${PORT}  (reports: ${REPORTS_DIR})`);
});
