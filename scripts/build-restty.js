// restty vendor build script
// Usage: node scripts/build-restty.js
//
// Copies restty npm package files into frontend/vendor/restty/ and applies
// Den-specific patches to the chunk-*.js bootstrap (WebGL2 antialiasing and
// removal of fontIndex===0 restriction so fallback CJK glyphs are width-clamped).
//
// The chunk-*.js filename is content-hash dependent (changes per upstream build).
// restty.js / xterm.js already reference the new filename internally, so a plain
// copy keeps everything wired. The Den-authored restty-xterm-adapter.js is left
// untouched.
//
// Patches are applied with exact string match + asserted occurrence count, so
// any upstream API drift fails the build instead of silently dropping a patch.

const fs = require('fs');
const path = require('path');

const pkgDir = path.join(__dirname, '..', 'node_modules', 'restty');
const distDir = path.join(pkgDir, 'dist');
const outDir = path.join(__dirname, '..', 'frontend', 'vendor', 'restty');

const FILES_TO_COPY = ['restty.js', 'xterm.js'];
const PATCHES = [
  {
    description: 'enable WebGL2 antialiasing',
    find: 'antialias: false',
    replace: 'antialias: true',
    expectedOccurrences: 1,
  },
  {
    description: 'remove fontIndex===0 restriction so fallback CJK glyphs are width-clamped',
    find: '!symbolLike && fontIndex === 0',
    replace: '!symbolLike',
    expectedOccurrences: 2,
  },
];

function fail(msg) {
  console.error(`build-restty: ${msg}`);
  process.exit(1);
}

function readPkgVersion() {
  const pkg = JSON.parse(fs.readFileSync(path.join(pkgDir, 'package.json'), 'utf8'));
  return pkg.version;
}

function findChunkFile() {
  const matches = fs.readdirSync(distDir).filter((f) => /^chunk-[a-z0-9]+\.js$/.test(f));
  if (matches.length === 0) fail('no chunk-*.js found in node_modules/restty/dist');
  if (matches.length > 1) fail(`multiple chunk-*.js found: ${matches.join(', ')}`);
  return matches[0];
}

function removeOldChunks() {
  if (!fs.existsSync(outDir)) return;
  for (const f of fs.readdirSync(outDir)) {
    if (/^chunk-[a-z0-9]+\.js$/.test(f)) {
      fs.unlinkSync(path.join(outDir, f));
      console.log(`  removed stale ${f}`);
    }
  }
}

function copyFile(src, destName) {
  const dest = path.join(outDir, destName);
  fs.copyFileSync(src, dest);
  console.log(`  copied ${destName}`);
}

function countOccurrences(haystack, needle) {
  let count = 0;
  let idx = 0;
  while ((idx = haystack.indexOf(needle, idx)) !== -1) {
    count++;
    idx += needle.length;
  }
  return count;
}

function applyPatches(filePath) {
  let content = fs.readFileSync(filePath, 'utf8');
  for (const patch of PATCHES) {
    const found = countOccurrences(content, patch.find);
    if (found !== patch.expectedOccurrences) {
      fail(
        `patch "${patch.description}" expected ${patch.expectedOccurrences} match(es) ` +
        `for ${JSON.stringify(patch.find)} but found ${found} in ${path.basename(filePath)}. ` +
        `Upstream API may have changed.`
      );
    }
    content = content.split(patch.find).join(patch.replace);
    console.log(`  patched: ${patch.description} (${found} replacement${found === 1 ? '' : 's'})`);
  }
  fs.writeFileSync(filePath, content);
}

function main() {
  if (!fs.existsSync(distDir)) fail('node_modules/restty not installed. run `npm install` first.');
  if (!fs.existsSync(outDir)) fs.mkdirSync(outDir, { recursive: true });

  const version = readPkgVersion();
  console.log(`Building restty vendor bundle (npm restty@${version})`);

  removeOldChunks();

  for (const f of FILES_TO_COPY) {
    const src = path.join(distDir, f);
    if (!fs.existsSync(src)) fail(`missing ${src}`);
    copyFile(src, f);
  }

  const chunkName = findChunkFile();
  copyFile(path.join(distDir, chunkName), chunkName);

  const licenseSrc = path.join(pkgDir, 'LICENSE');
  if (fs.existsSync(licenseSrc)) copyFile(licenseSrc, 'LICENSE');

  applyPatches(path.join(outDir, chunkName));

  console.log('restty vendor bundle built.');
}

main();
