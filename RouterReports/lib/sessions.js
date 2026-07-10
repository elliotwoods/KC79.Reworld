// Session discovery: scan the reports directory and pair NDJSON files with
// their summaries. A session id is the UTC stamp in the filename.

import fs from 'node:fs';
import path from 'node:path';

const SESSION_RE = /^session-(\d{8}T\d{6}Z)(?:\.(\d+))?\.ndjson$/;

export function listSessions(dir) {
  let entries;
  try {
    entries = fs.readdirSync(dir);
  } catch {
    return [];
  }
  const sessions = new Map();
  for (const name of entries) {
    const match = SESSION_RE.exec(name);
    if (match) {
      const id = match[1];
      const session = sessions.get(id) ?? { id, ndjsonFiles: [], summaryFile: null };
      session.ndjsonFiles.push(name);
      sessions.set(id, session);
    }
    const summaryMatch = /^session-(\d{8}T\d{6}Z)\.summary\.json$/.exec(name);
    if (summaryMatch) {
      const id = summaryMatch[1];
      const session = sessions.get(id) ?? { id, ndjsonFiles: [], summaryFile: null };
      session.summaryFile = name;
      sessions.set(id, session);
    }
  }

  return [...sessions.values()]
    .filter((s) => s.ndjsonFiles.length > 0)
    .map((s) => {
      s.ndjsonFiles.sort();
      const stats = s.ndjsonFiles.map((f) => fs.statSync(path.join(dir, f)));
      s.bytes = stats.reduce((total, st) => total + st.size, 0);
      s.modified = Math.max(...stats.map((st) => st.mtimeMs));
      if (s.summaryFile) {
        try {
          const summary = JSON.parse(fs.readFileSync(path.join(dir, s.summaryFile), 'utf8'));
          s.meta = {
            start_ts: summary.session?.start_ts,
            duration_ms: summary.session?.duration_ms,
            clean_exit: summary.session?.clean_exit,
            totals: summary.totals,
            faults: (summary.fault_timeline ?? []).reduce((n, f) => n + (f.count ?? 1), 0),
          };
        } catch {
          s.meta = null;
        }
      }
      return s;
    })
    .sort((a, b) => b.id.localeCompare(a.id));
}

export function sessionFiles(dir, id) {
  if (!/^\d{8}T\d{6}Z$/.test(id)) return null;
  const session = listSessions(dir).find((s) => s.id === id);
  return session ?? null;
}
