// wterm vendor bundle build script
// Usage: node scripts/build-wterm.js

const esbuild = require('esbuild');
const fs = require('fs');
const path = require('path');

const entryPoint = path.join(__dirname, 'wterm-entry.js');
const outDir = path.join(__dirname, '..', 'frontend', 'vendor', 'wterm');
const cssSrc = path.join(__dirname, '..', 'node_modules', '@wterm', 'dom', 'src', 'terminal.css');
const cssDest = path.join(outDir, 'wterm.css');

if (!fs.existsSync(outDir)) fs.mkdirSync(outDir, { recursive: true });

esbuild.build({
  entryPoints: [entryPoint],
  bundle: true,
  format: 'esm',
  outfile: path.join(outDir, 'wterm.bundle.js'),
  minify: true,
  sourcemap: false,
  target: ['es2020'],
}).then(() => {
  fs.copyFileSync(cssSrc, cssDest);
  console.log('wterm bundle built:', path.join(outDir, 'wterm.bundle.js'));
  console.log('wterm CSS copied:', cssDest);
}).catch((err) => {
  console.error('Build failed:', err);
  process.exit(1);
});
