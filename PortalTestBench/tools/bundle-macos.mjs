// Wrap PortalTestBench in the `.app` bundle macOS requires to run it natively.
//
//     node tools/bundle-macos.mjs [--profile debug|release] [--sign <identity>] [--resources <dir>]
//
// ## Why a bundle is not optional
//
// `cef_initialize` resolves its framework relative to the main bundle -- `av_cef_loader_mac.c`
// looks for `Contents/Frameworks/Chromium Embedded Framework.framework` -- and the helper
// processes need their own bundles so they do not each take a Dock icon. A bare binary run from a
// terminal finds none of it. `--headless` needs none of it either, which is what keeps the fast
// loop fast.
//
// ## Where this came from, and where it deliberately differs
//
// The layout, the plist keys, the four helper suffixes and the inside-out signing order are all
// from `third_party/av-frameworks/tools/bundle-macos.mjs`, which is the source of truth and whose
// comments record what each one cost to find out. That script bundles every `[[bin]]` in the
// *framework's* workspace, which is not this one, so it cannot be called -- this is the same
// shape for one application.
//
// Two things here have no counterpart there:
//
//   1. **`Contents/Resources`.** The plans and firmware a packaged bench ships with. The
//      framework has no products that carry data, so its bundler stages none.
//
//   2. **Vulkan.** `av-gui-shell` composites a macOS composed window through MoltenVK, and
//      `ash::Entry::load()` `dlopen`s `libvulkan.dylib` **by bare name**. The framework's bundler
//      stages CEF and nothing else, so a `.app` built its way runs only on a machine with the
//      LunarG SDK installed in the developer's home directory. That is fine for the framework's
//      own examples and fatal for a distributable. See `stageVulkan` for what is staged and
//      `launcherScript` for why the executable is reached through a stub.
//
// It does not build. A missing binary is reported and skipped, the same reasoning as the
// framework's: a step that compiled would hold the build directory's lock and turn this into a
// multi-minute operation with no cancel.

import fs from 'node:fs';
import path from 'node:path';

import { done, fail, main, run, step, tryRun, warn } from '../../tools/lib/proc.mjs';

const app = path.resolve(import.meta.dirname, '..');
const framework = path.join(app, 'third_party', 'av-frameworks');

const APP_NAME = 'PortalTestBench';
const BUNDLE_ID = 'com.kimchiandchips.portal-test-bench';
/** The real executable, reached through the launcher stub. See `launcherScript`. */
const BINARY = 'portal-test-bench';
/** Shipped beside it: the agent's CLI, and the CEF helper every helper bundle links to. */
const COMPANIONS = ['ptb'];
const HELPER_SOURCE = 'av-gui-subprocess';

/**
 * The helper bundles Chromium launches children from, by name suffix.
 *
 * All four, and the reason is the framework's hardest-won note: `ChildProcessHost::GetChildPath`
 * rewrites `browser_subprocess_path`'s base name for every non-`CHILD_NORMAL` child when bundled,
 * so a renderer comes from `<App> Helper (Renderer).app`. With only the base helper, utilities and
 * the GPU process launch perfectly and **a renderer can never launch** -- `posix_spawnp` returns
 * ENOENT into a `DLOG` that a Release framework does not compile. No crash report, because
 * nothing ever launched; `cef_initialize` succeeds, the page is served, and the window draws its
 * clear colour forever.
 */
const HELPER_SUFFIXES = ['', ' (Renderer)', ' (GPU)', ' (Alerts)'];

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
    CFBundleExecutable: APP_NAME,
    CFBundleName: APP_NAME,
    CFBundleDisplayName: 'Portal Test Bench',
    // The TCC key. Change this and every permission the user granted resets, silently.
    CFBundleIdentifier: BUNDLE_ID,
    CFBundlePackageType: 'APPL',
    CFBundleInfoDictionaryVersion: '6.0',
    CFBundleShortVersionString: version,
    CFBundleVersion: version,
    LSMinimumSystemVersion: '12.0',
    // Without this every pixel is magnified and resampled: the Info.plist equivalent of
    // SetProcessDpiAwarenessContext, and worse, because nothing in the code will remind you.
    NSHighResolutionCapable: true,
    // macOS 15+ prompts for LAN access. Loopback is exempt, so the bench on 127.0.0.1 never sees
    // this; it is here for the day someone serves the page to another machine on the bench.
    NSLocalNetworkUsageDescription: 'Serves the bench interface to other machines on the local network.',
  });
}

function helperPlist(suffix, version) {
  const id = suffix ? `.${suffix.slice(2, -1).toLowerCase()}` : '';
  return plist({
    CFBundleExecutable: `${APP_NAME} Helper${suffix}`,
    CFBundleName: `${APP_NAME} Helper${suffix}`,
    CFBundleDisplayName: `${APP_NAME} Helper${suffix}`,
    CFBundleIdentifier: `${BUNDLE_ID}.helper${id}`,
    CFBundlePackageType: 'APPL',
    CFBundleInfoDictionaryVersion: '6.0',
    CFBundleShortVersionString: version,
    CFBundleVersion: version,
    // No Dock icon, no menu bar. A renderer process that took one would give this application as
    // many Dock icons as Chromium has processes.
    LSUIElement: true,
  });
}

/**
 * The launcher stub, and why the executable is not reached directly.
 *
 * `ash::Entry::load()` `dlopen`s `libvulkan.dylib` by bare name, and dyld resolves a bare name
 * against `DYLD_LIBRARY_PATH` -- which it reads **once, at process launch**. So a Rust `set_var`
 * inside `main` is too late by construction: the variable is already cached and the `dlopen` that
 * follows ignores it. `LSEnvironment` in `Info.plist` is the documented alternative and is
 * ignored by modern macOS.
 *
 * That leaves one mechanism that works: a process that sets the variables and then `exec`s the
 * real binary, so dyld reads them at *its* launch. It costs a `sh` in the process tree and it is
 * the difference between a `.app` that runs anywhere and one that runs on the machine that built
 * it.
 *
 * `exec "$@"` passes through every argument, so `--headless`, `--simulate` and `--port` all still
 * reach the binary when someone runs it from a terminal.
 */
function launcherScript() {
  return `#!/bin/sh
# Set the Vulkan loader's environment, then become the real binary.
#
# Not a convenience wrapper. dyld reads DYLD_LIBRARY_PATH once at process launch, and the Vulkan
# loader is dlopen'd by bare name from inside the process -- so these have to be set *before*
# the executable starts, and nothing inside it can do that for itself. See bundle-macos.mjs.
here=$(cd -- "$(dirname -- "$0")" && pwd)
frameworks="$here/../Frameworks"

if [ -f "$frameworks/libvulkan.1.dylib" ]; then
    DYLD_LIBRARY_PATH="$frameworks\${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}"
    export DYLD_LIBRARY_PATH
fi
if [ -f "$frameworks/vulkan/icd.d/MoltenVK_icd.json" ] && [ -z "$VK_ICD_FILENAMES" ]; then
    VK_ICD_FILENAMES="$frameworks/vulkan/icd.d/MoltenVK_icd.json"
    export VK_ICD_FILENAMES
fi

exec "$here/${BINARY}" "$@"
`;
}

/** Hard-link, falling back to a copy. The CEF framework is ~349 MB. */
function linkTree(src, dst) {
  fs.mkdirSync(dst, { recursive: true });
  for (const entry of fs.readdirSync(src, { withFileTypes: true })) {
    const from = path.join(src, entry.name);
    const to = path.join(dst, entry.name);
    if (entry.isSymbolicLink()) {
      const target = fs.readlinkSync(from);
      fs.rmSync(to, { force: true, recursive: true });
      fs.symlinkSync(target, to);
    } else if (entry.isDirectory()) {
      linkTree(from, to);
    } else {
      fs.rmSync(to, { force: true });
      try {
        fs.linkSync(from, to);
      } catch {
        fs.copyFileSync(from, to);
      }
    }
  }
}

function hardLink(src, dst) {
  fs.rmSync(dst, { force: true });
  try {
    fs.linkSync(src, dst);
  } catch {
    fs.copyFileSync(src, dst);
  }
}

/**
 * MoltenVK, from the LunarG SDK on the building machine into the bundle.
 *
 * Reported rather than assumed: without a Vulkan SDK the bundle is still built, still signed and
 * still runs `--headless`, and the native window fails at `Gpu::new` with `NoAdapter`. That is a
 * bundle that works on this machine and not on anyone else's, so it has to be said out loud at
 * the moment it is produced rather than discovered by whoever it is handed to.
 *
 * The ICD JSON is rewritten rather than copied: its `library_path` points at the SDK's own
 * `libMoltenVK.dylib` by absolute path, which does not exist on the far machine. A relative path
 * is resolved by the loader against the JSON's own directory, which is exactly what is wanted.
 */
function stageVulkan(frameworksDir) {
  const sdk = process.env.VULKAN_SDK;
  if (!sdk) {
    warn(
      'VULKAN_SDK is unset, so no Vulkan loader was staged.\n' +
        '    This bundle will run --headless anywhere and will fail to open its window on any\n' +
        '    machine without the LunarG SDK installed, reporting NoAdapter.\n' +
        '    Install the SDK, source third_party/av-frameworks/tools/setup-env-macos.sh, re-run.',
    );
    return false;
  }

  const loader = path.join(sdk, 'lib', 'libvulkan.1.dylib');
  const moltenvk = path.join(sdk, 'lib', 'libMoltenVK.dylib');
  const missing = [loader, moltenvk].filter((f) => !fs.existsSync(f));
  if (missing.length) {
    warn(`VULKAN_SDK is set but these are missing, so nothing was staged:\n    ${missing.join('\n    ')}`);
    return false;
  }

  hardLink(loader, path.join(frameworksDir, 'libvulkan.1.dylib'));
  hardLink(moltenvk, path.join(frameworksDir, 'libMoltenVK.dylib'));

  const icdDir = path.join(frameworksDir, 'vulkan', 'icd.d');
  fs.mkdirSync(icdDir, { recursive: true });
  fs.writeFileSync(
    path.join(icdDir, 'MoltenVK_icd.json'),
    // Relative, so the loader resolves it against this file's directory rather than against a
    // path on the machine that built the bundle.
    `${JSON.stringify(
      {
        file_format_version: '1.0.0',
        ICD: { library_path: '../../libMoltenVK.dylib', api_version: '1.2.0', is_portability_driver: true },
      },
      null,
      2,
    )}\n`,
  );

  step('Staged MoltenVK (libvulkan.1, libMoltenVK, ICD manifest)');
  return true;
}

function codesign(sign, target) {
  const result = tryRun('codesign', ['--force', '--sign', sign, '--timestamp=none', target]);
  if (!result.ok) warn(`codesign ${path.basename(target)}: ${result.stderr}`);
}

main(() => {
  const options = parseArgs(process.argv.slice(2));
  const targetDir = path.join(app, 'target', options.profile);
  if (!fs.existsSync(targetDir)) {
    fail(`no ${path.relative(app, targetDir)} -- run \`node tools/build.mjs\` first`);
  }

  const binary = path.join(targetDir, BINARY);
  if (!fs.existsSync(binary)) {
    fail(`${BINARY} is not built in ${options.profile}. Run: node tools/build.mjs${options.profile === 'release' ? ' --release' : ''}`);
  }

  const version = JSON.parse(
    tryRun('cargo', ['metadata', '--no-deps', '--format-version', '1', '--manifest-path', path.join(app, 'Cargo.toml')])
      .stdout || '{"packages":[]}',
  ).packages?.find((p) => p.name === BINARY)?.version ?? '0.1.0';

  const bundleRoot = options.out ? path.resolve(options.out) : path.join(targetDir, 'bundle');
  const bundle = path.join(bundleRoot, `${APP_NAME}.app`);
  fs.rmSync(bundle, { recursive: true, force: true });

  const contents = path.join(bundle, 'Contents');
  const macos = path.join(contents, 'MacOS');
  const resources = path.join(contents, 'Resources');
  const frameworks = path.join(contents, 'Frameworks');
  for (const dir of [macos, resources, frameworks]) fs.mkdirSync(dir, { recursive: true });

  step(`Bundling ${APP_NAME} ${version} (${options.profile})`);
  fs.writeFileSync(path.join(contents, 'Info.plist'), appPlist(version));
  fs.writeFileSync(path.join(contents, 'PkgInfo'), 'APPL????');

  // The launcher takes CFBundleExecutable's name; the real binary sits beside it under its own.
  fs.writeFileSync(path.join(macos, APP_NAME), launcherScript(), { mode: 0o755 });
  hardLink(binary, path.join(macos, BINARY));
  for (const name of COMPANIONS) {
    const from = path.join(targetDir, name);
    if (fs.existsSync(from)) hardLink(from, path.join(macos, name));
    else warn(`${name} is not built; the bundle will not carry it`);
  }

  // --- resources -------------------------------------------------------------------------
  if (options.resources) {
    const from = path.resolve(options.resources);
    if (!fs.existsSync(from)) fail(`--resources ${from} does not exist`);
    linkTree(from, resources);
    step(`Staged resources from ${path.relative(app, from) || from}`);
  } else {
    warn('No --resources given: this bundle carries no plans and no firmware. Use tools/package.mjs.');
  }

  // --- CEF -------------------------------------------------------------------------------
  const cefSrc = path.join(targetDir, 'Frameworks', 'Chromium Embedded Framework.framework');
  const cefDst = path.join(frameworks, 'Chromium Embedded Framework.framework');
  if (fs.existsSync(cefSrc)) {
    linkTree(cefSrc, cefDst);
    step('Staged the CEF framework');
  } else {
    warn(
      `No CEF framework at ${path.relative(app, cefSrc)}.\n` +
        '    The bundle will build and a native run will fail at the loader.\n' +
        `    node ${path.join('third_party', 'av-frameworks', 'tools', 'fetch-cef.mjs')} && node tools/build.mjs`,
    );
  }

  const vulkanStaged = stageVulkan(frameworks);

  // --- helpers ---------------------------------------------------------------------------
  const helperExe = path.join(targetDir, HELPER_SOURCE);
  const helpers = [];
  if (fs.existsSync(helperExe)) {
    for (const suffix of HELPER_SUFFIXES) {
      const helperApp = path.join(frameworks, `${APP_NAME} Helper${suffix}.app`);
      fs.mkdirSync(path.join(helperApp, 'Contents', 'MacOS'), { recursive: true });
      fs.writeFileSync(path.join(helperApp, 'Contents', 'Info.plist'), helperPlist(suffix, version));
      fs.writeFileSync(path.join(helperApp, 'Contents', 'PkgInfo'), 'APPL????');
      hardLink(helperExe, path.join(helperApp, 'Contents', 'MacOS', `${APP_NAME} Helper${suffix}`));
      helpers.push(helperApp);
    }
  }

  // --- signing ---------------------------------------------------------------------------
  //
  // Inside-out, which codesign requires: a nested bundle signed after its container invalidates
  // the container's signature. The CEF framework is signed too, and that is not tidiness --
  // `linkTree` reproduces it file by file, which does not reproduce the seal the distribution
  // shipped, so `--verify --deep` reports "code has no resources but signature indicates they
  // must be present" unless it is re-sealed.
  step(`Signing (${options.sign === '-' ? 'ad-hoc' : options.sign})`);
  if (fs.existsSync(cefDst)) codesign(options.sign, cefDst);
  for (const helper of helpers) codesign(options.sign, helper);
  codesign(options.sign, bundle);

  // Signed, then checked -- because `codesign` succeeding says the signature was written, not
  // that it is valid, and an invalid one costs a renderer process rather than an error message.
  const verified = tryRun('codesign', ['--verify', '--deep', '--strict', bundle]);
  if (!verified.ok) {
    fail(`the bundle is not validly signed:\n${verified.stderr}`);
  }

  console.log('');
  // The helper count is reported because a bundle missing one still launches, still serves, and
  // simply never renders -- so "ok" alone has already once meant "ships a window that cannot draw".
  done(`${APP_NAME}.app  helpers ${helpers.length}/${HELPER_SUFFIXES.length}  signature valid`);
  if (helpers.length !== HELPER_SUFFIXES.length) {
    warn('helper binary missing; this app cannot render');
  }
  if (!vulkanStaged) {
    warn('no Vulkan loader inside; this app cannot open its window on a machine without the SDK');
  }
  console.log(`  ${bundle}`);
});
