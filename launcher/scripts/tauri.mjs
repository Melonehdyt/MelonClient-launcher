import { spawn } from "node:child_process";
import { homedir } from "node:os";
import { delimiter, join } from "node:path";
import { fileURLToPath } from "node:url";

// Rust is installed per-user under ~/.cargo/bin by rustup. Some shells (and
// terminals opened before Rust was installed) do not have it on PATH yet, so we
// inject it here before handing off to the Tauri CLI.
const cargoBin = join(homedir(), ".cargo", "bin");
const pathKey =
  Object.keys(process.env).find((key) => key.toLowerCase() === "path") || "Path";

const env = { ...process.env };
const merged = `${cargoBin}${delimiter}${env[pathKey] ?? ""}`;
env[pathKey] = merged;
env.PATH = merged;
env.Path = merged;

const root = fileURLToPath(new URL("..", import.meta.url));
const cli = join(root, "node_modules", "@tauri-apps", "cli", "tauri.js");

const child = spawn(process.execPath, [cli, ...process.argv.slice(2)], {
  stdio: "inherit",
  env,
});

child.on("exit", (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
  }
  process.exit(code ?? 1);
});
