// Put a built package on the repository's GitHub releases page.
//
//     node tools/publish-release.mjs [--zip <file>] [--tag <tag>] [--publish]
//                                    [--allow-dirty] [--prerelease] [--notes-only]
//                                    [--note "what changed in this one"]
//
// Separate from `package.mjs` on purpose. Building an archive is repeatable and local; publishing
// one is neither. A release is the artefact other people find months later, the tag is permanent
// in everyone's clone the moment anybody fetches, and `gh release delete` does not un-download it.
// So the two are different commands, and this one **drafts by default** -- `--publish` is a thing
// you have to type.
//
// ## What it refuses, and why each one is worth a refusal
//
//  - **A package built from a dirty tree.** The zip is named for the commit it came from. If the
//    firmware sources had uncommitted changes, nobody -- including you, in a month -- can rebuild
//    those images from that tag. The archive is still perfectly good to hand to a colleague
//    directly; it is being *published against a commit* that makes it a lie. `--allow-dirty`
//    overrides and stamps the notes so the download says so.
//  - **A commit that is not on the remote.** `gh release create --target <sha>` will happily make
//    a tag pointing at an object GitHub does not have, and the release page then shows a tag that
//    resolves nowhere for everyone except the person who made it.
//  - **A tag that already exists.** Answered by picking the next free one rather than failing:
//    this repository's convention is a date with a letter suffix (`2023-08-26`, `2023-08-26B`),
//    and re-releasing on the same day is the ordinary case rather than the exception.
//
// ## Why the notes repeat the quarantine instructions
//
// `README.txt` inside the zip already covers it, and that is exactly one step too late: a file
// downloaded from a GitHub release is quarantined by definition, so the first thing that happens
// to the recipient is the failure the README explains -- and they cannot read the README without
// unzipping the thing macOS has just refused. The instruction has to be on the page they are
// already looking at.

import fs from 'node:fs';
import path from 'node:path';

import { done, fail, main, run, step, tryRun, warn } from '../../tools/lib/proc.mjs';

const app = path.resolve(import.meta.dirname, '..');
const repo = path.resolve(app, '..');
const dist = path.join(repo, 'dist');

function parseArgs(argv) {
  const options = {
    zip: null, tag: null, publish: false, allowDirty: false, prerelease: false, notesOnly: false,
    note: null,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    const next = () => {
      const value = argv[i + 1];
      if (!value) fail(`${arg} needs a value`);
      i += 1;
      return value;
    };
    if (arg === '--zip') options.zip = next();
    else if (arg === '--tag') options.tag = next();
    else if (arg === '--publish') options.publish = true;
    else if (arg === '--allow-dirty') options.allowDirty = true;
    else if (arg === '--prerelease') options.prerelease = true;
    else if (arg === '--notes-only') options.notesOnly = true;
    else if (arg === '--note') options.note = next();
    else fail(`unknown argument \`${arg}\``);
  }
  return options;
}

/** The most recently written PortalTestBench archive in `dist/`. */
function newestZip() {
  if (!fs.existsSync(dist)) fail(`${dist} does not exist. Run: node tools/package.mjs`);
  const candidates = fs
    .readdirSync(dist)
    .filter((name) => /^PortalTestBench-.*\.zip$/.test(name))
    .map((name) => ({ name, at: fs.statSync(path.join(dist, name)).mtimeMs }))
    .sort((a, b) => b.at - a.at);
  if (candidates.length === 0) fail('no PortalTestBench archive in dist/. Run: node tools/package.mjs');
  return path.join(dist, candidates[0].name);
}

/**
 * The commit and platform an archive is named for.
 *
 * Parsed from the filename rather than tracked in a sidecar, because the filename is what a person
 * reads and what a download is called -- a second record could disagree with it, and the one that
 * would be believed is the one in the URL.
 */
function describeZip(file) {
  const match = path.basename(file).match(/^PortalTestBench-(.+)-(macos|windows)-(\w+)\.zip$/);
  if (!match) fail(`${path.basename(file)} is not a name this understands: PortalTestBench-<sha>-<platform>-<arch>.zip`);
  const [, describe, platform, arch] = match;
  return { describe, platform, arch, sha: describe.replace(/-dirty$/, ''), dirty: describe.endsWith('-dirty') };
}

/** The next free tag under this repository's date convention: `2026-08-26`, then `B`, `C`, ... */
function nextTag(existing) {
  const today = tryRun('date', ['+%Y-%m-%d']).stdout;
  if (!existing.has(today)) return today;
  for (const suffix of 'BCDEFGHIJKLMNOPQRSTUVWXYZ') {
    if (!existing.has(`${today}${suffix}`)) return `${today}${suffix}`;
  }
  fail(`there are already 27 releases dated ${today}; pass --tag explicitly`);
}

/** The firmware table out of the package's own manifest, so the notes cannot drift from the zip. */
function firmwareTable() {
  const manifest = path.join(app, 'target/package');
  if (!fs.existsSync(manifest)) return null;
  const folder = fs.readdirSync(manifest).find((name) => name.startsWith('PortalTestBench-'));
  if (!folder) return null;
  const file = path.join(manifest, folder, 'FIRMWARE.md');
  if (!fs.existsSync(file)) return null;
  const rows = fs
    .readFileSync(file, 'utf8')
    .split('\n')
    .filter((line) => line.startsWith('| `'));
  return rows.length > 0 ? rows : null;
}

function notesFor({ describe, dirty, arch }, zip, table, note) {
  const size = `${(fs.statSync(zip).size / (1024 * 1024)).toFixed(1)} MB`;
  return `Bench for a single KC79 Portal module: flash it, provision it, drive it, watch it.

Self-contained — the app, the \`ptb\` CLI, the test plans and a full firmware set for both PCB
revisions. Nothing to install: no Homebrew, no Rust, no Node, no PlatformIO, no libusb.

**macOS, Apple silicon (${arch}) · ${size} · built from \`${describe}\`**
${dirty ? '\n> ⚠️ Built from a tree with uncommitted changes in the firmware sources, so these images cannot be rebuilt from this tag. The sha256 of each is below and is what identifies them.\n' : ''}${note ? `\n## What changed\n\n${note}\n` : ''}
## Before it will open

macOS quarantines anything downloaded, and this app is signed ad hoc rather than notarised, so it
will refuse with **"PortalTestBench is damaged and can't be opened"**. It is not damaged — that is
the quarantine flag. Unzip it, then in Terminal type the following **including the trailing space**,
drag \`PortalTestBench.app\` onto the window, and press return:

\`\`\`
xattr -dr com.apple.quarantine 
\`\`\`

No output means it worked. Double-click the app.

Without Terminal: double-click and let it be refused, then System Settings → Privacy & Security →
**Open Anyway** at the bottom. You have to let it fail first, and on macOS 15+ this is the only
route through the interface — right-click → Open no longer works for an app that is not notarised.

\`README.txt\` in the archive covers this and the rest of the workflow.

## Firmware in this build
${
  table
    ? `\n| image | what | load address | bytes | banner | ELF | sha256 |\n|---|---|---|---|---|---|---|\n${table.join('\n')}\n`
    : '\nSee `FIRMWARE.md` in the archive.\n'
}
Pick the application that matches the board — optical is PCB v6, mechanical is PCB v4. The
"legacy base" pair is for a board still carrying a v4/v5 bootloader that is **not** being replaced
in the same pass. Anything else can be dragged onto the window as a \`.bin\` or \`.elf\`.
`;
}

main(() => {
  const options = parseArgs(process.argv.slice(2));
  if (!tryRun('gh', ['--version']).ok) {
    fail('the GitHub CLI is not installed. `brew install gh`, then `gh auth login`.');
  }
  const auth = tryRun('gh', ['auth', 'status']);
  if (!auth.ok) fail(`not authenticated to GitHub:\n${auth.stderr || auth.stdout}`);

  const zip = options.zip ? path.resolve(options.zip) : newestZip();
  if (!fs.existsSync(zip)) fail(`${zip} does not exist`);
  const meta = describeZip(zip);
  step(`Archive: ${path.basename(zip)}`);

  if (meta.dirty && !options.allowDirty) {
    fail(
      `this package was built from a tree with uncommitted firmware changes (\`${meta.describe}\`).\n` +
        '  Published against a tag, its images could never be rebuilt from that commit. Commit the\n' +
        '  firmware sources and repackage, or pass --allow-dirty to publish it with a warning in\n' +
        '  the notes. Handing the zip to somebody directly needs neither.',
    );
  }

  // A tag pointing at a commit the remote does not have resolves nowhere for everybody else.
  //
  // Two failures, kept apart because they lead to different actions: an object this clone has
  // never seen means the archive was built somewhere else or renamed, and one it has but the
  // remote does not means push. Folding them into "push it first" sends you to a command that
  // will not help.
  if (!tryRun('git', ['-C', repo, 'cat-file', '-e', `${meta.sha}^{commit}`]).ok) {
    fail(
      `there is no commit \`${meta.sha}\` in this clone.\n` +
        '  The archive was built somewhere else, or its name has been edited. Repackage here, or\n' +
        '  pass --tag and a --zip whose name matches a commit this repository has.',
    );
  }
  const onRemote = tryRun('git', ['-C', repo, 'branch', '-r', '--contains', meta.sha]);
  if (!onRemote.ok || !onRemote.stdout.trim()) {
    fail(
      `commit ${meta.sha} exists here but is not on any remote branch.\n` +
        '  Push it first: the release tag would point at an object GitHub does not have, and the\n' +
        '  release page would show a tag that resolves nowhere for everybody except you.',
    );
  }
  // `gh release create --target` documents "branch or full commit SHA", and the archive is named
  // with an abbreviated one. Expanded here rather than hoping the short form is accepted -- a
  // wrong guess fails at the moment somebody is trying to ship.
  const full = tryRun('git', ['-C', repo, 'rev-parse', `${meta.sha}^{commit}`]).stdout;
  if (!full) fail(`could not expand ${meta.sha} to a full commit sha`);

  const existing = new Set(
    (tryRun('gh', ['release', 'list', '--limit', '200', '--json', 'tagName', '--jq', '.[].tagName']).stdout || '')
      .split('\n')
      .filter(Boolean),
  );
  const tag = options.tag ?? nextTag(existing);
  if (options.tag && existing.has(tag)) fail(`release \`${tag}\` already exists`);
  step(`Tag: ${tag}  →  ${full}`);

  const notes = notesFor(meta, zip, firmwareTable(), options.note);
  const notesFile = path.join(app, 'target', 'release-notes.md');
  fs.mkdirSync(path.dirname(notesFile), { recursive: true });
  fs.writeFileSync(notesFile, notes);
  if (options.notesOnly) {
    done(`Notes written to ${notesFile}`);
    console.log('');
    console.log(notes);
    return;
  }

  if (!options.publish) {
    warn('Drafting. Nothing is public until you press Publish on the page, or re-run with --publish.');
  }
  run('gh', [
    'release', 'create', tag, zip,
    '--repo', 'elliotwoods/KC79.Reworld',
    '--target', full,
    '--title', `PortalTestBench ${tag}`,
    '--notes-file', notesFile,
    ...(options.publish ? [] : ['--draft']),
    ...(options.prerelease ? ['--prerelease'] : []),
  ], { cwd: repo });

  const url = tryRun('gh', ['release', 'view', tag, '--json', 'url', '--jq', '.url'], { cwd: repo }).stdout;
  console.log('');
  done(`${options.publish ? 'Published' : 'Drafted'} ${tag}`);
  if (url) console.log(`  ${url}`);
  if (!options.publish) console.log('  Review it, then press Publish release on that page.');
});
