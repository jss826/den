// CodeMirror 6 vendor bundle build script
// Usage: node scripts/build-codemirror.js

const esbuild = require('esbuild');
const path = require('path');

const entryPoint = path.join(__dirname, 'codemirror-entry.js');
const outDir = path.join(__dirname, '..', 'frontend', 'vendor');

esbuild.build({
  entryPoints: [entryPoint],
  bundle: true,
  format: 'iife',
  globalName: 'CM',
  outfile: path.join(outDir, 'codemirror.js'),
  minify: true,
  sourcemap: false,
  target: ['es2020'],
}).then(() => {
  console.log('CodeMirror bundle built successfully.');
}).catch((err) => {
  console.error('Build failed:', err);
  process.exit(1);
});
