import { mkdirSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const distDir = path.join(__dirname, "dist");
mkdirSync(distDir, { recursive: true });

console.log("1. Bundling agent.ts with esbuild...");
const sdkPath = path.resolve(__dirname, "../upstream/strands-harness-sdk/strands-ts/src/index.ts");

await esbuild.build({
  entryPoints: [path.join(__dirname, "src/agent.ts")],
  bundle: true,
  format: "esm",
  platform: "browser",
  target: "es2022",
  nodePaths: [path.join(__dirname, "node_modules")],
  define: {
    "import.meta.vitest": "undefined",
    "process.env.NODE_ENV": '"production"',
  },
  alias: {
    "@strands-agents/sdk": sdkPath,
  },
  external: [
    "child_process",
    "fs",
    "node:*",
    "path",
  ],
  outfile: path.join(distDir, "bundle.js"),
  logLevel: "info",
});

console.log("✓ Bundle generated at dist/bundle.js");

console.log("2. Componentizing bundle to WASI 0.2 Component...");
let componentizeModule;
try {
  componentizeModule = await import("@bytecodealliance/componentize-js");
} catch {
  componentizeModule = await import("@chaynabors/componentize-js");
}

const componentize = componentizeModule.componentize || componentizeModule.default?.componentize;
if (!componentize) {
  throw new Error("Unable to locate componentize function from componentize-js package");
}

try {
  const { _setPreopens } = await import("@bytecodealliance/preview2-shim/filesystem");
  if (_setPreopens) {
    _setPreopens({ "/": "/" });
  }
} catch {
  // Ignore if preview2-shim filesystem not available
}

let bundleSource = readFileSync(path.join(distDir, "bundle.js"), "utf-8");
// Normalize regex character classes that fail in SpiderMonkey builds without full ICU:
bundleSource = bundleSource.replace(/\[\^\\p\{L\}\\p\{N\}_\]\+/g, "[^a-zA-Z0-9_]+");
bundleSource = bundleSource.replace(/\^\[\\p\{Cc\}\\p\{Cf\}\\p\{Zs\}\]\+/g, "^[\\s]+");
// Polyfill top-level location for cfworker dereference during Wizer pre-warming
bundleSource = bundleSource.replace(/var initialBaseURI = [^;]+;/g, 'var initialBaseURI = new URL("https://github.com/cfworker");');
writeFileSync(path.join(distDir, "bundle.js"), bundleSource);

const witDir = path.resolve(__dirname, "wit");

const { component } = await componentize(bundleSource, {
  witPath: witDir,
  worldName: "strand-agent",
  // The world exports `taffee:agent/worker`, not wasi:http/incoming-handler; outbound fetch ('http')
  // stays enabled.
  disableFeatures: ["fetch-event"],
  // No `debug: { bindings: true }`: in componentize-js 0.23.0 that path logs an
  // undefined `tmpDir` (componentize.js:209) and throws before Wizer runs.
});

const outWasm = path.join(distDir, "strand-agent.wasm");
writeFileSync(outWasm, component);

console.log(`✓ WASI 0.2 component emitted successfully: ${outWasm}`);
