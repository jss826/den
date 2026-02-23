// Den - SVG アイコンライブラリ
// すべてのアイコンは currentColor ベースでテーマ追従
const DenIcons = (() => {
  const S = 16; // デフォルトサイズ

  /**
   * SVG ラッパー文字列を生成する。
   * @param {string} inner - SVG 内部要素の HTML 文字列
   * @param {number} [size] - width/height（デフォルト 16）
   * @returns {string} 完全な SVG 文字列
   */
  function svg(inner, size) {
    const s = size || S;
    return `<svg width="${s}" height="${s}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${inner}</svg>`;
  }

  // --- UI アイコン ---

  function filePlus(size) {
    return svg('<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="12" y1="18" x2="12" y2="12"/><line x1="9" y1="15" x2="15" y2="15"/>', size);
  }

  function folderPlus(size) {
    return svg('<path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/><line x1="12" y1="11" x2="12" y2="17"/><line x1="9" y1="14" x2="15" y2="14"/>', size);
  }

  function upload(size) {
    return svg('<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="17 8 12 3 7 8"/><line x1="12" y1="3" x2="12" y2="15"/>', size);
  }

  function refresh(size) {
    return svg('<polyline points="23 4 23 10 17 10"/><path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"/>', size);
  }

  function gear(size) {
    return svg('<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09a1.65 1.65 0 0 0-1.08-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09a1.65 1.65 0 0 0 1.51-1.08 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1.08z"/>', size);
  }

  function chevronLeft(size) {
    return svg('<polyline points="15 18 9 12 15 6"/>', size);
  }

  function chevronRight(size) {
    return svg('<polyline points="9 18 15 12 9 6"/>', size);
  }

  function terminal(size) {
    return svg('<polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/>', size);
  }

  function download(size) {
    return svg('<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/>', size);
  }

  function panelLeft(size) {
    return svg('<rect x="3" y="3" width="18" height="18" rx="2" ry="2"/><line x1="9" y1="3" x2="9" y2="21"/>', size);
  }

  function panelRight(size) {
    return svg('<rect x="3" y="3" width="18" height="18" rx="2" ry="2"/><line x1="15" y1="3" x2="15" y2="21"/>', size);
  }

  function snippet(size) {
    return svg('<polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"/>', size);
  }

  function clipboard(size) {
    return svg('<path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"/><rect x="8" y="2" width="8" height="4" rx="1" ry="1"/>', size);
  }

  // --- ファイルツリー アイコン ---

  function folder(size) {
    return svg('<path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>', size);
  }

  function file(size) {
    return svg('<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/>', size);
  }

  // --- ファイルタイプ別アイコン色マッピング ---

  const fileTypeColors = {
    js: '#f7df1e', mjs: '#f7df1e', cjs: '#f7df1e',
    jsx: '#61dafb', tsx: '#3178c6',
    ts: '#3178c6', mts: '#3178c6',
    rs: '#dea584',
    py: '#3572a5',
    go: '#00add8',
    rb: '#cc342d',
    java: '#b07219', kt: '#a97bff',
    c: '#7b8894', h: '#7b8894', cpp: '#f34b7d', hpp: '#f34b7d',
    cs: '#178600',
    swift: '#f05138',
    css: '#563d7c', scss: '#c6538c', less: '#1d365d',
    html: '#e34c26', htm: '#e34c26',
    vue: '#41b883', svelte: '#ff3e00',
    json: '#a8b1c1', yaml: '#cb171e', yml: '#cb171e', toml: '#9c4121',
    xml: '#0060ac', svg: '#ffb13b',
    md: '#083fa1', mdx: '#083fa1',
    sh: '#89e051', bash: '#89e051', zsh: '#89e051', ps1: '#2b5797',
    sql: '#e38c00',
    docker: '#384d54', dockerfile: '#384d54',
    lock: '#8b95a3', gitignore: '#f05032',
    env: '#ecd53f',
    png: '#a4c639', jpg: '#a4c639', jpeg: '#a4c639', gif: '#a4c639', webp: '#a4c639', ico: '#a4c639',
    wasm: '#654ff0',
    txt: '#8b95a3',
  };

  /** 拡張子からアイコンカラーを返す（未知の拡張子は null） */
  function fileColor(filename) {
    const ext = filename.split('.').pop().toLowerCase();
    return fileTypeColors[ext] || null;
  }

  return {
    filePlus, folderPlus, upload, refresh, gear, terminal,
    chevronLeft, chevronRight, download,
    panelLeft, panelRight,
    snippet, clipboard,
    folder, file, fileColor, svg,
  };
})();

if (typeof module !== 'undefined') module.exports = DenIcons;
