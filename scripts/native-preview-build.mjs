import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  cpSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { basename, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const ROOT = resolve(fileURLToPath(new URL("..", import.meta.url)));
export const TARGETS = {
  "aarch64-apple-darwin": {
    platform: "darwin-aarch64",
    renderer: "appkit",
    suffix: ".app.tar.gz",
    bundles: "app,dmg",
  },
  "x86_64-pc-windows-msvc": {
    platform: "windows-x86_64",
    renderer: "win32",
    host: "apps/native/windows",
    binary: "captures-windows",
    suffix: "-setup.exe",
    bundles: "nsis",
  },
  "x86_64-unknown-linux-gnu": {
    platform: "linux-x86_64",
    renderer: "gtk",
    host: "apps/native/gtk",
    binary: "captures-gtk",
    suffix: ".AppImage",
    // The AppImage is packed by `appImage` below, without Tauri's WebKitGTK.
    bundles: "deb",
  },
};

export function updaterArtifactName(target, version) {
  if (!TARGETS[target] || !/^\d+\.\d+\.\d+$/u.test(version))
    throw new Error("Invalid native release target/version");
  return `Captures-${version}-${target}${TARGETS[target].suffix}`;
}

/** A per-file patch update from `base` (`delta::patch_name`); each is signed
 * under this name, which binds the target, release and starting version. */
export function patchName(target, version, base) {
  return `Captures-${version}-${target}.from-${base}.capturespatch`;
}

/** Patch files in `names` for this release and target, newest base first. */
export function patchFiles(names, target, version) {
  const prefix = `Captures-${version}-${target}.from-`;
  const suffix = ".capturespatch";
  const order = (version) => version.split(".").map(Number);
  return names
    .filter((name) => name.startsWith(prefix) && name.endsWith(suffix))
    .map((name) => ({ name, from: name.slice(prefix.length, -suffix.length) }))
    .filter(({ from }) => /^\d+\.\d+\.\d+$/u.test(from))
    .sort((left, right) => {
      const [a, b] = [order(right.from), order(left.from)];
      return a[0] - b[0] || a[1] - b[1] || a[2] - b[2];
    });
}

// Windows and Linux package the Win32 and GTK hosts, each its own pinned
// 1.95 workspace, never the maintenance-only egui/wgpu workbench. They build
// in the host workspace's own target directory, not under the root `target`:
// CI's Rust cache keeps one dependency set per target directory, and pruned
// the host's dependencies from a directory nested in the root workspace's.
export function nativeHostBuild(target) {
  const { host, binary } = TARGETS[target] ?? {};
  if (!host) throw new Error(`No Win32/GTK native host for ${target}`);
  const targetDirectory = `${host}/target`;
  const exe = target.includes("windows") ? ".exe" : "";
  return {
    prepare: ["apps/native/prepare.mjs", "--output", `${host}/resources`],
    cargo: [
      "+1.95.0",
      "build",
      "--locked",
      "--release",
      "--manifest-path",
      `${host}/Cargo.toml`,
      "--bin",
      binary,
      "--target-dir",
      targetDirectory,
      "--target",
      target,
    ],
    binary: `${targetDirectory}/${target}/release/${binary}${exe}`,
  };
}

// `--side-by-side` test installers install next to an existing Captures: their
// own product name, identifier (install folder, uninstall entry, shortcuts) and
// no file associations, which would re-point the installed app's ProgIDs.
export const SIDE_BY_SIDE = {
  productName: "Captures Native Test",
  identifier: "io.github.joswayski.captures.nativetest",
  // NSIS closes running copies of the main binary by process name, so the
  // test app must not be `captures.exe`. A test build's updater only installs
  // over this identity, never the installed Captures.
  mainBinaryName: "captures-native-test",
};

// `--staging` builds are side-by-side test installs whose updater is on, but
// reads only this feed: the `native-staging` release that
// `.github/workflows/native-staging.yml` publishes. Public clients never read it.
export const STAGING_FEED =
  "https://github.com/joswayski/captures/releases/download/native-staging/latest.json";

// Installer inputs (icons, notices, plists, DMG art, sidecars) and the base
// bundler configuration live here; `build` overlays the release specifics.
export const BUNDLE = "apps/native/bundle";

export function bundleConfiguration(target, version, root = ROOT, sideBySide = false) {
  updaterArtifactName(target, version);
  const base = JSON.parse(readFileSync(join(root, BUNDLE, "tauri.conf.json"), "utf8"));
  const configuration = {
    version,
    bundle: {
      externalBin: [...base.bundle.externalBin, "binaries/native_preview_update_helper"],
      resources: { ...base.bundle.resources },
      windows: { webviewInstallMode: { type: "skip" } },
    },
  };
  if (sideBySide) {
    Object.assign(configuration, SIDE_BY_SIDE);
    configuration.bundle.fileAssociations = [];
  }
  if (TARGETS[target].renderer === "gtk") {
    // The CLI always adds its WebKitGTK/GTK 3 dependencies; the host needs GTK 4.
    configuration.bundle.linux = { deb: { depends: ["libgtk-4-1"] } };
  }
  if (TARGETS[target].renderer === "appkit") {
    configuration.bundle.macOS = {
      infoPlist: join(root, "target/native-preview/Info.plist"),
      // Drag-to-Applications window. The icon centers match the slots drawn in
      // apps/native/bundle/dmg/background.svg. The 32 pt over the picture's
      // 400 pt height is Finder's title bar.
      dmg: {
        background: join(root, "target/native-preview/dmg-background.tiff"),
        windowSize: { width: 660, height: 432 },
        appPosition: { x: 170, y: 200 },
        applicationFolderPosition: { x: 490, y: 200 },
      },
    };
    configuration.bundle.resources[
      join(root, "apps/native/macos/.build/release/CapturesNative_CapturesNative.bundle")
    ] = "CapturesNative_CapturesNative.bundle";
  }
  return configuration;
}

function run(command, arguments_, options = {}) {
  return execFileSync(command, arguments_, { cwd: ROOT, stdio: "inherit", ...options });
}

// Pinned AppImage tools: linuxdeploy collects the GTK 4 libraries, the type2
// runtime starts the image. The helper packs the squashfs itself, the same way
// a patch install does (crates/captures-app/src/updater/preview/appimage.rs).
export const APPIMAGE_TOOLS = {
  "linuxdeploy-x86_64.AppImage": {
    url: "https://github.com/linuxdeploy/linuxdeploy/releases/download/1-alpha-20251107-1/linuxdeploy-x86_64.AppImage",
    sha256: "c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d",
  },
  "linuxdeploy-plugin-gtk.sh": {
    url: "https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/7a3fbc31a9e5075073ff8790f26effbac5f84453/linuxdeploy-plugin-gtk.sh",
    sha256: "b0f4cbc684a0103a9651f0955b635eaea0096b3a66c0f5a2c2aa337960375171",
  },
  "runtime-x86_64": {
    url: "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64",
    sha256: "2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d",
  },
};
// Where the AppDir carries its runtime, for patch installs to repack with.
export const APPIMAGE_RUNTIME = ".captures-appimage-runtime";

function appImageTools() {
  const directory = join(ROOT, "target/appimage-tools");
  mkdirSync(directory, { recursive: true });
  for (const [name, { url, sha256 }] of Object.entries(APPIMAGE_TOOLS)) {
    const path = join(directory, name);
    const digest = () => createHash("sha256").update(readFileSync(path)).digest("hex");
    if (!existsSync(path) || digest() !== sha256) {
      run("curl", [
        "--fail",
        "--location",
        "--silent",
        "--show-error",
        "--retry",
        "3",
        "-o",
        path,
        url,
      ]);
      if (digest() !== sha256) throw new Error(`${name} does not match its pinned SHA-256`);
    }
    chmodSync(path, 0o755);
  }
  return directory;
}

/** The desktop entry Tauri's template rendered for the AppImage and `.deb`. */
export function desktopEntry(base, productName, executable, associations) {
  const mimeTypes = associations.map((association) => association.mimeType).join(";");
  return [
    "[Desktop Entry]",
    `Categories=${base.bundle.category};`,
    `Comment=${base.bundle.shortDescription}`,
    `Exec=${executable} %U`,
    `StartupWMClass=${executable}`,
    `Icon=${executable}`,
    `Name=${productName}`,
    "Terminal=false",
    "Type=Application",
    ...(mimeTypes ? [`MimeType=${mimeTypes}`] : []),
    "",
  ].join("\n");
}

/** The Linux AppImage: the GTK 4 host and its sidecars in the layout Tauri used
 * (`usr/bin`, notices in `usr/lib/<product>`), GTK 4 libraries only. */
function appImage(target, version, mainBinary, configuration, env) {
  const base = JSON.parse(readFileSync(join(ROOT, BUNDLE, "tauri.conf.json"), "utf8"));
  const productName = configuration.productName ?? base.productName;
  const release = join(ROOT, "target", target, "release");
  const appdir = join(release, "appimage", `${productName}.AppDir`);
  rmSync(join(release, "appimage"), { recursive: true, force: true });
  const bin = join(appdir, "usr/bin");
  mkdirSync(bin, { recursive: true });
  copyFileSync(join(release, mainBinary), join(bin, mainBinary));
  for (const sidecar of ["ffmpeg", "ffprobe", "native_preview_update_helper"])
    copyFileSync(join(ROOT, BUNDLE, "binaries", `${sidecar}-${target}`), join(bin, sidecar));
  for (const name of readdirSync(bin)) chmodSync(join(bin, name), 0o755);
  for (const [source, destination] of Object.entries(configuration.bundle.resources))
    cpSync(join(ROOT, BUNDLE, source), join(appdir, "usr/lib", productName, destination), {
      recursive: true,
    });
  for (const [size, icon] of [
    ["32x32", "32x32.png"],
    ["128x128", "128x128.png"],
    ["256x256", "128x128@2x.png"],
  ]) {
    const folder = join(appdir, "usr/share/icons/hicolor", size, "apps");
    mkdirSync(folder, { recursive: true });
    copyFileSync(join(ROOT, BUNDLE, "icons", icon), join(folder, `${mainBinary}.png`));
  }
  const desktop = join(release, "appimage", `${mainBinary}.desktop`);
  writeFileSync(
    desktop,
    desktopEntry(
      base,
      productName,
      mainBinary,
      configuration.bundle.fileAssociations ?? base.bundle.fileAssociations,
    ),
  );
  const tools = appImageTools();
  run(
    join(tools, "linuxdeploy-x86_64.AppImage"),
    [
      "--appdir",
      appdir,
      "--executable",
      join(bin, mainBinary),
      "--desktop-file",
      desktop,
      "--icon-file",
      join(appdir, "usr/share/icons/hicolor/256x256/apps", `${mainBinary}.png`),
      "--plugin",
      "gtk",
    ],
    {
      env: {
        ...env,
        PATH: `${tools}:${env.PATH}`,
        ARCH: "x86_64",
        APPIMAGE_EXTRACT_AND_RUN: "1",
        DEPLOY_GTK_VERSION: "4",
        // Ship the binaries exactly as built and checked.
        NO_STRIP: "1",
      },
    },
  );
  // appimagetool used to add this; file managers show it as the image's icon.
  symlinkSync(`${mainBinary}.png`, join(appdir, ".DirIcon"));
  copyFileSync(join(tools, "runtime-x86_64"), join(appdir, APPIMAGE_RUNTIME));
  chmodSync(join(appdir, APPIMAGE_RUNTIME), 0o644);
  const output = join(release, "bundle/appimage");
  mkdirSync(output, { recursive: true });
  const image = join(output, `${productName}_${version}_amd64.AppImage`);
  rmSync(image, { force: true });
  run(join(release, "native_preview_update_helper"), ["--appimage", appdir, `${image}.partial`]);
  renameSync(`${image}.partial`, image);
}

export function build(target, version, unsigned = false, sideBySide = false, staging = false) {
  if (staging && unsigned) throw new Error("Staging builds publish signed updater artifacts");
  sideBySide ||= staging;
  const specification = TARGETS[target];
  updaterArtifactName(target, version);
  const shipping = JSON.parse(readFileSync(join(ROOT, BUNDLE, "tauri.conf.json"), "utf8"));
  const publicKey = Buffer.from(shipping.plugins.updater.pubkey, "base64").toString("utf8");
  if (!publicKey.startsWith("untrusted comment:") || publicKey.trim().split("\n").length !== 2)
    throw new Error("Invalid shipping updater public key");
  const env = {
    ...process.env,
    CARGO_TARGET_DIR: join(ROOT, "target"),
    CAPTURES_NATIVE_PREVIEW_VERSION: version,
    CAPTURES_NATIVE_PREVIEW_PUBLIC_KEY: publicKey,
    // Side-by-side test builds: no update checks, installs or startup repairs.
    ...(sideBySide ? { CAPTURES_NATIVE_PREVIEW_TEST: "1" } : {}),
    // Staging builds update from the staging feed only.
    ...(staging ? { CAPTURES_NATIVE_PREVIEW_FEED: STAGING_FEED } : {}),
    // With CI=true the DMG bundler skips the Finder script that applies the
    // window layout, leaving an unarranged window with no install hint.
    TAURI_BUNDLER_DMG_IGNORE_CI: "true",
  };
  // Cargo creates this directory. Creating `target/<triple>` first stops Cargo
  // marking it with CACHEDIR.TAG, and CI's Rust cache then deletes its
  // `release` directory before every save, so each release recompiled everything.
  const release = join(ROOT, "target", target, "release");
  const exe = target.includes("windows") ? ".exe" : "";
  run(
    "cargo",
    [
      "build",
      "--locked",
      "--release",
      "--target",
      target,
      "-p",
      "captures-app",
      "--bin",
      "native_preview_update_helper",
    ],
    { env },
  );
  const binaries = join(ROOT, BUNDLE, "binaries");
  mkdirSync(binaries, { recursive: true });
  const helper = join(binaries, `native_preview_update_helper-${target}${exe}`);
  copyFileSync(join(release, `native_preview_update_helper${exe}`), helper);
  let nativeBinary;
  if (specification.renderer === "appkit") {
    run(process.execPath, ["apps/native/prepare.mjs"], { env });
    run(
      "cargo",
      ["build", "--locked", "--release", "--target", target, "-p", "captures-settings-ffi"],
      { env },
    );
    run("swift", ["build", "--package-path", "apps/native/macos", "-c", "release"], {
      env: { ...env, CAPTURES_NATIVE_LIB_DIR: release },
    });
    nativeBinary = join(ROOT, "apps/native/macos/.build/release/CapturesNative");
    mkdirSync(join(ROOT, "target/native-preview"), { recursive: true });
    run("python3", [
      "-c",
      "import plistlib,sys; p=plistlib.load(open(sys.argv[1],'rb')); p['CapturesNativeLive']=True; plistlib.dump(p,open(sys.argv[2],'wb'))",
      `${BUNDLE}/Info.plist`,
      "target/native-preview/Info.plist",
    ]);
    // One TIFF with 1x and 2x pictures at exactly 72 and 144 DPI, so Finder
    // shows a sharp 660x400 pt background on Retina displays.
    run("tiffutil", [
      "-cathidpicheck",
      `${BUNDLE}/dmg/background.png`,
      `${BUNDLE}/dmg/background@2x.png`,
      "-out",
      "target/native-preview/dmg-background.tiff",
    ]);
  } else {
    const host = nativeHostBuild(target);
    run(process.execPath, host.prepare, { env });
    run("cargo", host.cargo, { env });
    nativeBinary = join(ROOT, host.binary);
  }
  // Refuse a Tauri binary, a development workbench, or a stale release version.
  const info = JSON.parse(
    execFileSync(nativeBinary, ["--preview-build-info"], { env, encoding: "utf8" }),
  );
  if (info.renderer !== specification.renderer || info.version !== version)
    throw new Error("Native release binary identity/version mismatch");
  const mainBinary = sideBySide ? SIDE_BY_SIDE.mainBinaryName : "captures";
  copyFileSync(nativeBinary, join(release, `${mainBinary}${exe}`));
  const config = join(ROOT, "target/native-preview-bundle.json");
  const configuration = bundleConfiguration(target, version, ROOT, sideBySide);
  if (unsigned || (sideBySide && !staging) || specification.renderer === "gtk")
    configuration.bundle.createUpdaterArtifacts = false;
  writeFileSync(config, `${JSON.stringify(configuration, null, 2)}\n`);
  // The Tauri CLI is ONLY a build-time installer bundler (DMG, NSIS, deb). No
  // Tauri code, frontend or WebView runtime enters the executable.
  run(
    process.execPath,
    [
      join(ROOT, "node_modules/@tauri-apps/cli/tauri.js"),
      "bundle",
      "--ci",
      "--target",
      target,
      "--bundles",
      specification.bundles,
      "--config",
      config,
    ],
    { cwd: join(ROOT, BUNDLE), env },
  );
  if (specification.renderer === "gtk") appImage(target, version, mainBinary, configuration, env);
}

export function collect(target, version, output) {
  const specification = TARGETS[target];
  const signedName = updaterArtifactName(target, version);
  const bundle = join(ROOT, "target", target, "release/bundle");
  const files = readdirSync(bundle, { recursive: true })
    .map((name) => join(bundle, name))
    .filter((path) => statSync(path).isFile());
  const exactlyOne = (matches) => {
    const selected = files.filter(matches);
    if (selected.length !== 1)
      throw new Error(`Expected one native ${target} artifact, found ${selected.length}`);
    return selected[0];
  };
  const updater = exactlyOne((path) => path.endsWith(specification.suffix));
  mkdirSync(output, { recursive: true });
  const signArtifact = (source, name) => {
    const artifact = join(output, name);
    copyFileSync(source, artifact);
    // The filename binds version + target through CLI 2.11's signed trusted
    // comment, while the signature envelope remains compatible with old Tauri.
    run(process.execPath, [
      join(ROOT, "node_modules/@tauri-apps/cli/tauri.js"),
      "signer",
      "sign",
      artifact,
    ]);
    const suffix = target.includes("windows") ? ".exe" : "";
    run(join(ROOT, "target", target, "release", `native_preview_update_helper${suffix}`), [
      "--verify",
      version,
      artifact,
      `${artifact}.sig`,
    ]);
  };
  signArtifact(updater, signedName);
  if (specification.renderer === "appkit") {
    const dmg = exactlyOne((path) => path.endsWith(".dmg"));
    copyFileSync(dmg, join(output, basename(dmg)));
  } else if (target.includes("linux")) {
    const deb = exactlyOne((path) => path.endsWith(".deb"));
    // Existing Tauri .deb installs select this format even with a generic
    // AppImage fallback; retain their one-time native cutover update route.
    signArtifact(deb, signedName.replace(/\.AppImage$/u, ".deb"));
  }
}

export function manifest(directory, version, repository, tag, notes = "") {
  const platforms = {};
  const entries = Object.entries(TARGETS).map(([target, specification]) => [
    specification.platform,
    updaterArtifactName(target, version),
  ]);
  entries.push([
    "linux-x86_64-deb",
    updaterArtifactName("x86_64-unknown-linux-gnu", version).replace(/\.AppImage$/u, ".deb"),
  ]);
  const url = (name) =>
    `https://github.com/${repository}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(name)}`;
  const signature = (name) => {
    const signature = readFileSync(join(directory, `${name}.sig`), "utf8").trim();
    const text = Buffer.from(signature, "base64").toString("utf8");
    if (
      !text
        .split("\n")
        .some(
          (line) =>
            line.startsWith("trusted comment: ") && line.split("\t").includes(`file:${name}`),
        )
    ) {
      throw new Error(`Updater signature does not bind the native target/version: ${name}`);
    }
    return signature;
  };
  for (const [platform, name] of entries) {
    platforms[platform] = {
      url: url(name),
      signature: signature(name),
      size: statSync(join(directory, name)).size,
    };
  }
  // Patch updates written by native-preview-patches.mjs, each signed under its
  // own name. A patch without its signature is left out.
  const names = readdirSync(directory);
  for (const [target, specification] of Object.entries(TARGETS)) {
    const patches = patchFiles(names, target, version).filter(({ name }) =>
      existsSync(join(directory, `${name}.sig`)),
    );
    if (!patches.length) continue;
    platforms[specification.platform].patches = patches.map(({ name, from }) => ({
      from,
      url: url(name),
      size: statSync(join(directory, name)).size,
      signature: signature(name),
    }));
  }
  return { version, notes, platforms };
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  const [operation, target, version, output] = process.argv.slice(2);
  if (operation === "build") {
    const flags = process.argv.slice(5);
    const known = ["--unsigned", "--side-by-side", "--staging"];
    const unknown = flags.filter((flag) => !known.includes(flag));
    if (unknown.length) throw new Error(`Unknown build flags: ${unknown.join(" ")}`);
    build(
      target,
      version,
      flags.includes("--unsigned"),
      flags.includes("--side-by-side"),
      flags.includes("--staging"),
    );
  } else if (operation === "collect") collect(target, version, resolve(output));
  else if (operation === "manifest") {
    const directory = resolve(target);
    writeFileSync(
      join(directory, "latest.json"),
      `${JSON.stringify(manifest(directory, version, process.env.GITHUB_REPOSITORY, process.env.TAG, process.env.RELEASE_NOTES ?? ""), null, 2)}\n`,
    );
  } else
    throw new Error(
      "usage: native-preview-build.mjs build TARGET VERSION [--unsigned] [--side-by-side | --staging] | collect TARGET VERSION OUTPUT | manifest DIRECTORY VERSION",
    );
}
