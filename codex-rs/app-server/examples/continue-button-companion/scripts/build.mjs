import { mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { stripTypeScriptTypes } from "node:module";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const srcDir = path.join(root, "src");
const distDir = path.join(root, "dist");

export async function build() {
  await rm(distDir, { recursive: true, force: true });
  await mkdir(distDir, { recursive: true });
  await buildDirectory(srcDir, distDir);
}

async function buildDirectory(sourceDir, targetDir) {
  await mkdir(targetDir, { recursive: true });
  const entries = await readdir(sourceDir, { withFileTypes: true });

  for (const entry of entries) {
    const sourcePath = path.join(sourceDir, entry.name);
    const targetPath = path.join(targetDir, entry.name.replace(/\.ts$/, ".js"));

    if (entry.isDirectory()) {
      await buildDirectory(sourcePath, targetPath);
      continue;
    }

    if (!entry.isFile() || !entry.name.endsWith(".ts")) {
      continue;
    }

    const source = await readFile(sourcePath, "utf8");
    const stripped = stripTypeScriptTypes(source, { mode: "strip" });
    const browserSource = stripped.replaceAll(".ts\";", ".js\";").replaceAll(".ts';", ".js';");
    await writeFile(targetPath, browserSource, "utf8");
  }
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  await build();
}
