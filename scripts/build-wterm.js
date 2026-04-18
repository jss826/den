// wterm vendor bundle build script
// Usage: node scripts/build-wterm.js
//
// Outputs (under frontend/vendor/wterm/):
//   wterm.bundle.js — esbuild ESM bundle of @wterm/dom (WASM inlined)
//   wterm.css       — upstream @wterm/dom/src/terminal.css + wterm.den.css
//
// Den-specific CSS overrides belong in wterm.den.css so they survive a rebuild.

const esbuild = require('esbuild');
const fs = require('fs');
const path = require('path');

const entryPoint = path.join(__dirname, 'wterm-entry.js');
const outDir = path.join(__dirname, '..', 'frontend', 'vendor', 'wterm');
const cssSrc = path.join(__dirname, '..', 'node_modules', '@wterm', 'dom', 'src', 'terminal.css');
const denOverrideSrc = path.join(outDir, 'wterm.den.css');
const cssDest = path.join(outDir, 'wterm.css');

if (!fs.existsSync(outDir)) fs.mkdirSync(outDir, { recursive: true });

function buildCss() {
  const upstream = fs.readFileSync(cssSrc, 'utf8');
  const denOverrides = fs.existsSync(denOverrideSrc)
    ? fs.readFileSync(denOverrideSrc, 'utf8')
    : '';
  const combined = `/* === upstream: @wterm/dom/src/terminal.css === */\n${upstream}\n\n/* === Den overrides (edit frontend/vendor/wterm/wterm.den.css, not this file) === */\n${denOverrides}`;
  fs.writeFileSync(cssDest, combined);
}

esbuild.build({
  entryPoints: [entryPoint],
  bundle: true,
  format: 'esm',
  outfile: path.join(outDir, 'wterm.bundle.js'),
  minify: true,
  sourcemap: false,
  target: ['es2020'],
}).then(() => {
  buildCss();
  console.log('wterm bundle built:', path.join(outDir, 'wterm.bundle.js'));
  console.log('wterm CSS built:   ', cssDest);
}).catch((err) => {
  console.error('Build failed:', err);
  process.exit(1);
});
