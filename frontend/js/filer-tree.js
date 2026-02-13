/* global Auth */
// Den - ファイラ ツリービュー
// eslint-disable-next-line no-unused-vars
const FilerTree = (() => {
  let treeEl;
  let onFileSelect; // callback(path)
  let onContextMenu; // callback(path, isDir, x, y)
  let onRootResolved; // callback(resolvedPath) — ルートパス解決通知
  let rootPath = '~';
  // expanded: Set<path> — 展開中ディレクトリのパス
  const expanded = new Set();
  let selectedPath = null;

  function init(container, callbacks) {
    treeEl = container;
    onFileSelect = callbacks.onFileSelect;
    onContextMenu = callbacks.onContextMenu;
    onRootResolved = callbacks.onRootResolved;
    loadDir(rootPath);
  }

  function setRoot(path) {
    rootPath = path;
    expanded.clear();
    selectedPath = null;
    loadDir(rootPath);
  }

  async function loadDir(dirPath) {
    const data = await apiFetch(`/api/filer/list?path=${enc(dirPath)}&show_hidden=false`);
    if (!data) return;
    // ルートの場合はツリー全体を描画
    if (dirPath === rootPath) {
      treeEl.innerHTML = '';
      renderEntries(treeEl, data.entries, data.path, 0);
      if (onRootResolved) onRootResolved(data.path);
    } else {
      // サブディレクトリの場合は子要素のみ更新
      const childrenEl = treeEl.querySelector(`[data-children="${CSS.escape(dirPath)}"]`);
      if (childrenEl) {
        childrenEl.innerHTML = '';
        renderEntries(childrenEl, data.entries, dirPath, getDepth(childrenEl));
      }
    }
  }

  function renderEntries(container, entries, parentPath, depth) {
    for (const entry of entries) {
      const fullPath = joinPath(parentPath, entry.name);
      const item = document.createElement('div');

      // ツリーアイテム行
      const row = document.createElement('div');
      row.className = 'tree-item';
      row.style.paddingLeft = `${8 + depth * 16}px`;
      row.dataset.path = fullPath;
      row.dataset.isDir = entry.is_dir;

      if (fullPath === selectedPath) row.classList.add('selected');

      // 展開トグル
      const toggle = document.createElement('span');
      toggle.className = 'tree-toggle';
      if (entry.is_dir) {
        toggle.textContent = expanded.has(fullPath) ? '\u25BE' : '\u25B8';
      }
      row.appendChild(toggle);

      // アイコン
      const icon = document.createElement('span');
      icon.className = 'tree-icon';
      icon.textContent = entry.is_dir ? '\uD83D\uDCC1' : fileIcon(entry.name);
      row.appendChild(icon);

      // 名前
      const name = document.createElement('span');
      name.className = `tree-name${entry.is_dir ? ' dir' : ''}`;
      name.textContent = entry.name;
      row.appendChild(name);

      // クリック
      row.addEventListener('click', () => {
        if (entry.is_dir) {
          toggleDir(fullPath);
        } else {
          selectFile(fullPath);
        }
      });

      // 右クリック
      row.addEventListener('contextmenu', (e) => {
        e.preventDefault();
        if (onContextMenu) onContextMenu(fullPath, entry.is_dir, e.clientX, e.clientY);
      });

      item.appendChild(row);

      // 子要素コンテナ
      if (entry.is_dir) {
        const children = document.createElement('div');
        children.className = `tree-children${expanded.has(fullPath) ? ' expanded' : ''}`;
        children.dataset.children = fullPath;
        item.appendChild(children);
      }

      container.appendChild(item);
    }
  }

  async function toggleDir(dirPath) {
    if (expanded.has(dirPath)) {
      expanded.delete(dirPath);
      const childrenEl = treeEl.querySelector(`[data-children="${CSS.escape(dirPath)}"]`);
      if (childrenEl) childrenEl.classList.remove('expanded');
      // トグルアイコン更新
      updateToggle(dirPath, false);
    } else {
      expanded.add(dirPath);
      const childrenEl = treeEl.querySelector(`[data-children="${CSS.escape(dirPath)}"]`);
      if (childrenEl) {
        childrenEl.classList.add('expanded');
        if (childrenEl.children.length === 0) {
          await loadDir(dirPath);
        }
      }
      updateToggle(dirPath, true);
    }
  }

  function updateToggle(dirPath, isExpanded) {
    const row = treeEl.querySelector(`.tree-item[data-path="${CSS.escape(dirPath)}"]`);
    if (row) {
      const toggle = row.querySelector('.tree-toggle');
      if (toggle) toggle.textContent = isExpanded ? '\u25BE' : '\u25B8';
    }
  }

  function selectFile(filePath) {
    // 前の選択を解除
    const prev = treeEl.querySelector('.tree-item.selected');
    if (prev) prev.classList.remove('selected');
    // 新しい選択
    selectedPath = filePath;
    const row = treeEl.querySelector(`.tree-item[data-path="${CSS.escape(filePath)}"]`);
    if (row) row.classList.add('selected');
    if (onFileSelect) onFileSelect(filePath);
  }

  async function refresh() {
    // ルートを再描画してから展開済みディレクトリを順次ロード
    await loadDir(rootPath);
    for (const dir of expanded) {
      await loadDir(dir);
    }
  }

  // パスを親に渡す
  function getParentPath(filePath) {
    const sep = filePath.includes('/') ? '/' : '\\';
    const parts = filePath.split(sep);
    parts.pop();
    return parts.join(sep) || sep;
  }

  function refreshDir(dirPath) {
    if (expanded.has(dirPath)) {
      loadDir(dirPath);
    }
  }

  // --- ユーティリティ ---

  function joinPath(parent, name) {
    const sep = parent.includes('/') ? '/' : '\\';
    return parent.endsWith(sep) ? parent + name : parent + sep + name;
  }

  function getDepth(el) {
    let depth = 0;
    let node = el;
    while (node && node !== treeEl) {
      if (node.dataset && node.dataset.children !== undefined) depth++;
      node = node.parentElement;
    }
    return depth;
  }

  function fileIcon(name) {
    const ext = name.split('.').pop().toLowerCase();
    const icons = {
      rs: '\uD83E\uDD80', js: '\uD83D\uDFE8', ts: '\uD83D\uDD35', py: '\uD83D\uDC0D',
      html: '\uD83C\uDF10', css: '\uD83C\uDFA8', json: '{}', md: '\uD83D\uDCDD',
      toml: '\u2699\uFE0F', yaml: '\u2699\uFE0F', yml: '\u2699\uFE0F',
      txt: '\uD83D\uDCC4', sh: '\uD83D\uDCDC', ps1: '\uD83D\uDCDC',
      png: '\uD83D\uDDBC\uFE0F', jpg: '\uD83D\uDDBC\uFE0F', svg: '\uD83D\uDDBC\uFE0F',
      lock: '\uD83D\uDD12',
    };
    return icons[ext] || '\uD83D\uDCC4';
  }

  function enc(s) {
    return encodeURIComponent(s);
  }

  async function apiFetch(url) {
    try {
      const resp = await fetch(url, {
        headers: { 'Authorization': `Bearer ${Auth.getToken()}` },
      });
      if (!resp.ok) return null;
      return resp.json();
    } catch {
      return null;
    }
  }

  return { init, setRoot, refresh, refreshDir, getParentPath, selectFile };
})();
