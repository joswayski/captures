import { execFileSync } from "node:child_process";
import {
  copyFileSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  statSync,
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
    renderer: "wgpu",
    suffix: "-setup.exe",
    bundles: "nsis",
  },
  "x86_64-unknown-linux-gnu": {
    platform: "linux-x86_64",
    renderer: "wgpu",
    suffix: ".AppImage",
    bundles: "appimage,deb",
  },
};

export function updaterArtifactName(target, version) {
  if (!TARGETS[target] || !/^\d+\.\d+\.\d+$/u.test(version))
    throw new Error("Invalid native Preview target/version");
  return `Captures-${version}-${target}${TARGETS[target].suffix}`;
}

export function bundleConfiguration(target, version, root = ROOT) {
  updaterArtifactName(target, version);
  const recording = JSON.parse(
    readFileSync(join(root, "apps/desktop/src-tauri/tauri.recording.conf.json"), "utf8"),
  );
  const configuration = {
    version,
    build: { beforeBuildCommand: null, beforeBundleCommand: null, frontendDist: null },
    bundle: {
      ...recording.bundle,
      externalBin: [...recording.bundle.externalBin, "binaries/native_preview_update_helper"],
      windows: { webviewInstallMode: { type: "skip" } },
    },
  };
  if (TARGETS[target].renderer === "appkit") {
    configuration.bundle.macOS = { infoPlist: join(root, "target/native-preview/Info.plist") };
    configuration.bundle.resources[
      join(root, "apps/native/macos/.build/release/CapturesNative_CapturesNative.bundle")
    ] = "CapturesNative_CapturesNative.bundle";
  }
  return configuration;
}

function run(command, arguments_, options = {}) {
  return execFileSync(command, arguments_, { cwd: ROOT, stdio: "inherit", ...options });
}

export function build(target, version, unsigned = false) {
  const specification = TARGETS[target];
  updaterArtifactName(target, version);
  const shipping = JSON.parse(
    readFileSync(join(ROOT, "apps/desktop/src-tauri/tauri.conf.json"), "utf8"),
  );
  const publicKey = Buffer.from(shipping.plugins.updater.pubkey, "base64").toString("utf8");
  if (!publicKey.startsWith("untrusted comment:") || publicKey.trim().split("\n").length !== 2)
    throw new Error("Invalid shipping updater public key");
  const env = {
    ...process.env,
    CARGO_TARGET_DIR: join(ROOT, "target"),
    CAPTURES_NATIVE_PREVIEW_VERSION: version,
    CAPTURES_NATIVE_PREVIEW_PUBLIC_KEY: publicKey,
  };
  const release = join(ROOT, "target", target, "release");
  mkdirSync(release, { recursive: true });
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
  const helper = join(
    ROOT,
    "apps/desktop/src-tauri/binaries",
    `native_preview_update_helper-${target}${exe}`,
  );
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
      "apps/desktop/src-tauri/Info.plist",
      "target/native-preview/Info.plist",
    ]);
  } else {
    run(process.execPath, ["apps/native/prepare.mjs", "--output", "apps/native/wgpu/resources"], {
      env,
    });
    const nativeTarget = join(ROOT, "target/native-wgpu-preview");
    run(
      "cargo",
      [
        "+1.95.0",
        "build",
        "--locked",
        "--release",
        "--manifest-path",
        "apps/native/wgpu/Cargo.toml",
        "--bin",
        "captures-wgpu-workbench",
        "--target-dir",
        nativeTarget,
        "--target",
        target,
      ],
      { env },
    );
    nativeBinary = join(nativeTarget, target, "release", `captures-wgpu-workbench${exe}`);
  }
  // Refuse a Tauri binary, a development workbench, or a stale release version.
  const info = JSON.parse(
    execFileSync(nativeBinary, ["--preview-build-info"], { env, encoding: "utf8" }),
  );
  if (info.renderer !== specification.renderer || info.version !== version)
    throw new Error("Native Preview binary identity/version mismatch");
  copyFileSync(nativeBinary, join(release, `captures${exe}`));
  const config = join(ROOT, "target/native-preview-bundle.json");
  const configuration = bundleConfiguration(target, version);
  if (unsigned) configuration.bundle.createUpdaterArtifacts = false;
  writeFileSync(config, `${JSON.stringify(configuration, null, 2)}\n`);
  // The existing CLI is ONLY a build-time installer bundler. No Tauri app build,
  // frontend assets, application shim or WebView runtime enters the executable.
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
    { cwd: join(ROOT, "apps/desktop"), env },
  );
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
  for (const [platform, name] of entries) {
    const path = join(directory, name);
    const signature = readFileSync(`${path}.sig`, "utf8").trim();
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
    platforms[platform] = {
      url: `https://github.com/${repository}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(name)}`,
      signature,
      size: statSync(path).size,
    };
  }
  return { version, notes, platforms };
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  const [operation, target, version, output] = process.argv.slice(2);
  if (operation === "build") build(target, version, output === "--unsigned");
  else if (operation === "collect") collect(target, version, resolve(output));
  else if (operation === "manifest") {
    const directory = resolve(target);
    writeFileSync(
      join(directory, "latest.json"),
      `${JSON.stringify(manifest(directory, version, process.env.GITHUB_REPOSITORY, process.env.TAG, process.env.RELEASE_NOTES ?? ""), null, 2)}\n`,
    );
  } else
    throw new Error(
      "usage: native-preview-build.mjs build|collect TARGET VERSION [OUTPUT] | manifest DIRECTORY VERSION",
    );
}
