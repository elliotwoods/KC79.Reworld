// Streaming NDJSON reading: never buffers whole files.

import fs from 'node:fs';
import readline from 'node:readline';

/**
 * Async-iterate parsed events from one or more NDJSON files in order.
 * Silently skips malformed lines (a crashed writer can truncate the tail).
 */
export async function* readEvents(filePaths) {
  for (const filePath of filePaths) {
    const stream = fs.createReadStream(filePath, { encoding: 'utf8' });
    const lines = readline.createInterface({ input: stream, crlfDelay: Infinity });
    for await (const line of lines) {
      if (!line) continue;
      try {
        yield JSON.parse(line);
      } catch {
        // truncated/corrupt line
      }
    }
  }
}
