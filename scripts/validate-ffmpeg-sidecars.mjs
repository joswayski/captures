import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const REQUIRED_CONFIGURATION_FLAGS = [
  "--disable-autodetect",
  "--disable-gpl",
  "--disable-network",
  "--disable-nonfree",
  "--disable-version3",
  "--enable-audiotoolbox",
  "--enable-videotoolbox",
  "--enable-zlib",
];
const FORBIDDEN_CONFIGURATION_FLAGS = [
  "--enable-gpl",
  "--enable-nonfree",
  "--enable-version3",
  "--enable-libx264",
];
const REQUIRED_ENCODERS = ["aac", "gif", "h264_videotoolbox", "png"];
const REQUIRED_FILTERS = [
  "amix",
  "aresample",
  "atrim",
  "crop",
  "fps",
  "palettegen",
  "paletteuse",
  "scale",
  "volume",
];

function run(binary, args) {
  const result = spawnSync(binary, args, { encoding: "utf8" });
  if (result.error || result.status !== 0) {
    const detail = result.error?.message ?? result.stderr?.trim() ?? `status ${result.status}`;
    throw new Error(`${binary} ${args.join(" ")} failed: ${detail}`);
  }
  return `${result.stdout ?? ""}\n${result.stderr ?? ""}`;
}

function assertIncludes(haystack, needle, label) {
  if (!haystack.includes(needle)) throw new Error(`${label} is missing ${needle}`);
}

function assertNonEmpty(path, label) {
  if (statSync(path).size === 0) throw new Error(`${label} is empty`);
}

function validateSyntheticPipeline(ffmpeg, ffprobe) {
  const directory = mkdtempSync(join(tmpdir(), "captures-ffmpeg-smoke-"));
  const source = join(directory, "source.mp4");
  const poster = join(directory, "poster.png");
  const gif = join(directory, "preview.gif");
  try {
    run(ffmpeg, [
      "-hide_banner", "-loglevel", "error", "-y",
      "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=15",
      "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000",
      "-t", "1", "-shortest", "-c:v", "h264_videotoolbox", "-allow_sw", "1",
      "-b:v", "800k", "-c:a", "aac", source,
    ]);
    run(ffmpeg, [
      "-hide_banner", "-loglevel", "error", "-y", "-ss", "0", "-i", source,
      "-frames:v", "1", "-vf", "scale=160:-2", poster,
    ]);
    run(ffmpeg, [
      "-hide_banner", "-loglevel", "error", "-y", "-i", source,
      "-filter_complex",
      "[0:v]fps=10,scale=160:-2:flags=lanczos,split[s0][s1];[s0]palettegen=max_colors=128[p];[s1][p]paletteuse=dither=sierra2_4a",
      "-loop", "0", gif,
    ]);
    const probe = run(ffprobe, ["-v", "error", "-show_streams", "-of", "json", source]);
    assertIncludes(probe, '"codec_type": "video"', "synthetic recording probe");
    assertIncludes(probe, '"codec_type": "audio"', "synthetic recording probe");
    assertNonEmpty(source, "synthetic recording");
    assertNonEmpty(poster, "synthetic poster");
    assertNonEmpty(gif, "synthetic GIF");
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

export function validateBuildConfigurationText(text) {
  for (const flag of REQUIRED_CONFIGURATION_FLAGS) assertIncludes(text, flag, "FFmpeg build configuration");
  for (const flag of FORBIDDEN_CONFIGURATION_FLAGS) {
    if (text.includes(flag)) throw new Error(`FFmpeg build configuration contains forbidden flag ${flag}`);
  }
}

export function validateSidecars(ffmpeg, ffprobe, buildConfiguration) {
  const configurationText = readFileSync(buildConfiguration, "utf8");
  validateBuildConfigurationText(configurationText);

  const ffmpegVersion = run(ffmpeg, ["-hide_banner", "-version"]);
  const ffprobeVersion = run(ffprobe, ["-hide_banner", "-version"]);
  assertIncludes(ffmpegVersion, "ffmpeg version 8.1.2", "FFmpeg sidecar");
  assertIncludes(ffprobeVersion, "ffprobe version 8.1.2", "ffprobe sidecar");
  for (const flag of REQUIRED_CONFIGURATION_FLAGS) assertIncludes(ffmpegVersion, flag, "FFmpeg sidecar");
  for (const flag of FORBIDDEN_CONFIGURATION_FLAGS) {
    if (ffmpegVersion.includes(flag)) throw new Error(`FFmpeg sidecar contains forbidden flag ${flag}`);
  }

  const encoders = run(ffmpeg, ["-hide_banner", "-encoders"]);
  for (const encoder of REQUIRED_ENCODERS) {
    if (!new RegExp(`\\b${encoder.replaceAll("+", "\\+")}\\b`, "u").test(encoders)) {
      throw new Error(`FFmpeg sidecar is missing required encoder ${encoder}`);
    }
  }

  const filters = run(ffmpeg, ["-hide_banner", "-filters"]);
  for (const filter of REQUIRED_FILTERS) {
    if (!new RegExp(`\\b${filter}\\b`, "u").test(filters)) {
      throw new Error(`FFmpeg sidecar is missing required filter ${filter}`);
    }
  }
  validateSyntheticPipeline(ffmpeg, ffprobe);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [ffmpeg, ffprobe, buildConfiguration] = process.argv.slice(2);
  if (!ffmpeg || !ffprobe || !buildConfiguration) {
    throw new Error(
      "usage: node scripts/validate-ffmpeg-sidecars.mjs <ffmpeg> <ffprobe> <build-configuration>",
    );
  }
  validateSidecars(ffmpeg, ffprobe, buildConfiguration);
  process.stdout.write("Validated the pinned LGPL FFmpeg and ffprobe sidecars.\n");
}
