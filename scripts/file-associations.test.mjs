import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

function read(path) {
  return readFileSync(join(root, path), "utf8");
}

test("installed copies register Open With for editor-compatible media, not as the default app", () => {
  const config = JSON.parse(read("apps/desktop/src-tauri/tauri.conf.json"));
  const associations = config.bundle.fileAssociations;
  assert.ok(Array.isArray(associations), "fileAssociations must be set");

  const extensions = associations.flatMap((association) => association.ext).sort();
  assert.deepEqual(extensions, ["gif", "jpeg", "jpg", "mp4", "png", "webm", "webp"]);

  for (const association of associations) {
    assert.equal(association.role, "Editor", association.name);
    assert.equal(
      association.rank,
      "Alternate",
      `${association.name} must stay an Open With choice, not the default handler`,
    );
    assert.equal(typeof association.mimeType, "string", association.name);
  }

  assert.equal(
    config.bundle.linux.deb.desktopTemplate,
    "./linux/captures.desktop",
  );
  assert.equal(
    config.bundle.linux.rpm.desktopTemplate,
    "./linux/captures.desktop",
  );

  const desktop = read("apps/desktop/src-tauri/linux/captures.desktop");
  assert.match(desktop, /^Exec=\{\{exec\}\} %U$/mu);
  assert.match(desktop, /^MimeType=\{\{mime_type\}\}$/mu);
});
