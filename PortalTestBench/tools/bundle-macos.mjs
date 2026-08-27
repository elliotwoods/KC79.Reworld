// Wrap PortalTestBench in the `.app` bundle macOS wants in order to treat it as an application.
//
//     node tools/bundle-macos.mjs [--profile debug|release] [--sign <identity>] [--resources <dir>]
//
// ## What a bundle buys a control-window app, and what it does not
//
// This shell is `tao` + `wry` on WKWebView. It loads no CEF framework, launches no helper
// processes and touches no GPU, so none of the three reasons the framework's own
// `tools/bundle-macos.mjs` gives for a bundle applies here. A bare binary run from a terminal
// opens the window and works -- measured.
//
// What a bundle *does* buy is identity: a Dock icon that is this application rather than the
// terminal's, a name in the menu bar and in Force Quit, a `CFBundleIdentifier` for anything the
// OS keys on it later, and a signature. Those are worth the twenty lines below, and they are the
// whole of it.
//
// It is also what a distributable has to be. `resources_dir()` in `portal-swd` looks for
// `Contents/Resources` beside `Contents/MacOS`, which is how a packaged copy finds the firmware
// and the plans it shipped with.
//
// ## What used to be here
//
// The CEF framework, four suffixed helper bundles, a MoltenVK staging step and a launcher shell
// script that existed only to set `DYLD_LIBRARY_PATH` before the loader read it -- roughly 130 MB
// and every one of them a thing that could be missing on the far machine. They went when the
// window kind went back to `control-window`; the composed-window bundler is still upstream in
// `third_party/av-frameworks/tools/bundle-macos.mjs` if this application ever grows a viewport.
//
// It does not build. A missing binary is reported and skipped.

import fs from 'node:fs';
import path from 'node:path';

import { done, fail, main, step, tryRun, warn } from '../../tools/lib/proc.mjs';

const app = path.resolve(import.meta.dirname, '..');

const APP_NAME = 'PortalTestBench';
const BUNDLE_ID = 'com.kimchiandchips.portal-test-bench';
const BINARY = 'portal-test-bench';
/** The agent's CLI, shipped beside the bench so one package serves a person and a script. */
const COMPANIONS = ['ptb'];

function parseArgs(argv) {
  const options = { profile: 'release', sign: '-', resources: null, out: null };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    const next = () => {
      const value = argv[i + 1];
      if (!value) fail(`${arg} needs a value`);
      i += 1;
      return value;
    };
    if (arg === '--profile') options.profile = next();
    else if (arg === '--sign') options.sign = next();
    else if (arg === '--resources') options.resources = next();
    else if (arg === '--out') options.out = next();
    else fail(`unknown argument \`${arg}\``);
  }
  if (!['debug', 'release'].includes(options.profile)) {
    fail(`--profile must be debug or release, not \`${options.profile}\``);
  }
  return options;
}

function plist(entries) {
  const body = Object.entries(entries)
    .map(([k, v]) =>
      typeof v === 'boolean' ? `\t<key>${k}</key>\n\t<${v}/>` : `\t<key>${k}</key>\n\t<string>${v}</string>`,
    )
    .join('\n');
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
${body}
</dict>
</plist>
`;
}

function appPlist(version) {
  return plist({
    CFBundleExecutable: BINARY,
    CFBundleName: APP_NAME,
    CFBundleDisplayName: 'Portal Test Bench',
    CFBundleIdentifier: BUNDLE_ID,
    CFBundlePackageType: 'APPL',
    CFBundleInfoDictionaryVersion: '6.0',
    CFBundleShortVersionString: version,
    CFBundleVersion: version,
    LSMinimumSystemVersion: '12.0',
    // Without this every pixel is magnified and resampled: the Info.plist equivalent of
    // SetProcessDpiAwarenessContext, and worse, because nothing in the code will remind you.
    NSHighResolutionCapable: true,
    // macOS 15+ prompts for LAN access. Loopback is exempt, so a bench on 127.0.0.1 never sees
    // this; it is here for the day someone serves the page to another machine on the bench.
    NSLocalNetworkUsageDescription: 'Serves the bench interface to other machines on the local network.',
  });
}

function copyTree(src, dst) {
  fs.mkdirSync(dst, { recursive: true });
  for (const entry of fs.readdirSync(src, { withFileTypes: true })) {
    const from = path.join(src, entry.name);
    const to = path.join(dst, entry.name);
    if (entry.isDirectory()) copyTree(from, to);
    else fs.copyFileSync(from, to);
  }
}

function copyExecutable(src, dst) {
  // A real codesign rewrites the Mach-O signature. A hard link here therefore lets packaging
  // mutate target/{profile}, and signing a companion in place can invalidate a concurrently used
  // build artefact. A distributable must be an independent copy before signing begins.
  fs.copyFileSync(src, dst);
}

main(() => {
  const options = parseArgs(process.argv.slice(2));
  const targetDir = path.join(app, 'target', options.profile);
  const binary = path.join(targetDir, BINARY);
  if (!fs.existsSync(binary)) {
    fail(
      `${BINARY} is not built in ${options.profile}.\n` +
        `  Run: node tools/build.mjs${options.profile === 'release' ? ' --release' : ''}`,
    );
  }

  const version =
    JSON.parse(
      tryRun('cargo', [
        'metadata', '--no-deps', '--format-version', '1',
        '--manifest-path', path.join(app, 'Cargo.toml'),
      ], { cwd: app }).stdout || '{"packages":[]}',
    ).packages?.find((p) => p.name === BINARY)?.version ?? '0.1.0';

  const bundleRoot = options.out ? path.resolve(options.out) : path.join(targetDir, 'bundle');
  const bundle = path.join(bundleRoot, `${APP_NAME}.app`);
  fs.rmSync(bundle, { recursive: true, force: true });

  const contents = path.join(bundle, 'Contents');
  const macos = path.join(contents, 'MacOS');
  const resources = path.join(contents, 'Resources');
  fs.mkdirSync(macos, { recursive: true });
  fs.mkdirSync(resources, { recursive: true });

  step(`Bundling ${APP_NAME} ${version} (${options.profile})`);
  fs.writeFileSync(path.join(contents, 'Info.plist'), appPlist(version));
  fs.writeFileSync(path.join(contents, 'PkgInfo'), 'APPL????');

  copyExecutable(binary, path.join(macos, BINARY));
  for (const name of COMPANIONS) {
    const from = path.join(targetDir, name);
    if (fs.existsSync(from)) copyExecutable(from, path.join(macos, name));
    else warn(`${name} is not built; the bundle will not carry it`);
  }

  if (options.resources) {
    const from = path.resolve(options.resources);
    if (!fs.existsSync(from)) fail(`--resources ${from} does not exist`);
    copyTree(from, resources);
    step('Staged plans and firmware into Contents/Resources');
  } else {
    warn('No --resources given: this bundle carries no plans and no firmware. Use tools/package.mjs.');
  }

  // Ad hoc unless told otherwise. Signed *after* everything is in place, because a bundle whose
  // contents change afterwards is a bundle whose signature no longer matches -- and then verified,
  // because `codesign` succeeding says the signature was written, not that it is valid.
  step(`Signing (${options.sign === '-' ? 'ad-hoc' : options.sign})`);
  const signatureArgs = options.sign === '-'
    ? ['--force', '--sign', '-', '--timestamp=none']
    : ['--force', '--options', 'runtime', '--timestamp', '--sign', options.sign];

  // Sign nested code before the containing bundle. The main executable is signed as part of the
  // bundle; companions are independent Mach-O code and notarisation validates them separately.
  for (const name of COMPANIONS) {
    const companion = path.join(macos, name);
    if (fs.existsSync(companion)) {
      const nested = tryRun('codesign', [...signatureArgs, companion]);
      if (!nested.ok) fail(`codesign ${name}: ${nested.stderr}`);
    }
  }
  const signed = tryRun('codesign', [...signatureArgs, bundle]);
  if (!signed.ok) warn(`codesign: ${signed.stderr}`);

  const verified = tryRun('codesign', ['--verify', '--deep', '--strict', bundle]);
  if (!verified.ok) fail(`the bundle is not validly signed:\n${verified.stderr}`);

  const size = tryRun('du', ['-sh', bundle]).stdout.split(/\s+/)[0];
  console.log('');
  done(`${APP_NAME}.app  ${size}  signature valid`);
  console.log(`  ${bundle}`);
});
