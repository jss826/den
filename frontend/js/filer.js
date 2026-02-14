/* global Auth, FilerTree, FilerEditor */
// Den - ファイラ メインモジュール
// eslint-disable-next-line no-unused-vars
const DenFiler = (() => {
  let token;
  let currentDir = '~';
  let contextMenu = null;

  function init(authToken) {
    token = authToken;

    // エディタ初期化
    FilerEditor.init(
      document.getElementById('filer-editor'),
      document.getElementById('filer-tabs'),
    );

    // ツリー初期化
    FilerTree.init(document.getElementById('filer-tree'), {
      onFileSelect: (path) => FilerEditor.openFile(path),
      onContextMenu: showContextMenu,
      onRootResolved: (resolvedPath) => { currentDir = resolvedPath; },
    });

    // ツールバーボタン
    document.getElementById('filer-new-file').addEventListener('click', promptNewFile);
    document.getElementById('filer-new-folder').addEventListener('click', promptNewFolder);
    document.getElementById('filer-upload').addEventListener('click', showUploadModal);
    document.getElementById('filer-refresh').addEventListener('click', () => FilerTree.refresh());

    // 検索
    const searchInput = document.getElementById('filer-search-input');
    let searchTimeout;
    searchInput.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') {
        clearTimeout(searchTimeout);
        doSearch(searchInput.value.trim());
      }
    });
    searchInput.addEventListener('input', () => {
      clearTimeout(searchTimeout);
      if (searchInput.value.trim().length >= 2) {
        searchTimeout = setTimeout(() => doSearch(searchInput.value.trim()), 500);
      }
    });

    // アップロードモーダル
    document.getElementById('upload-cancel').addEventListener('click', () => {
      document.getElementById('filer-upload-modal').hidden = true;
    });
    document.getElementById('upload-submit').addEventListener('click', doUpload);

    // 検索結果モーダル
    document.getElementById('search-close').addEventListener('click', () => {
      document.getElementById('filer-search-modal').hidden = true;
    });

    // グローバルクリックでコンテキストメニュー閉じる
    document.addEventListener('click', hideContextMenu);

    // ドラッグ&ドロップ アップロード
    initDragDrop();
  }

  // --- ドラッグ&ドロップ アップロード ---

  function initDragDrop() {
    const filerPane = document.getElementById('filer-pane');
    let dragCounter = 0;
    let overlay = null;

    function showDropOverlay() {
      if (overlay) return;
      overlay = document.createElement('div');
      overlay.className = 'filer-drop-overlay';
      overlay.innerHTML = '<div class="filer-drop-content"><div class="filer-drop-icon">\u2B07</div><div>Drop files to upload</div></div>';
      filerPane.appendChild(overlay);
    }

    function hideDropOverlay() {
      if (overlay) {
        overlay.remove();
        overlay = null;
      }
    }

    filerPane.addEventListener('dragenter', (e) => {
      e.preventDefault();
      dragCounter++;
      if (dragCounter === 1) showDropOverlay();
    });

    filerPane.addEventListener('dragover', (e) => {
      e.preventDefault();
      e.dataTransfer.dropEffect = 'copy';
    });

    filerPane.addEventListener('dragleave', (e) => {
      e.preventDefault();
      dragCounter--;
      if (dragCounter <= 0) {
        dragCounter = 0;
        hideDropOverlay();
      }
    });

    filerPane.addEventListener('drop', async (e) => {
      e.preventDefault();
      dragCounter = 0;
      hideDropOverlay();

      const files = e.dataTransfer.files;
      if (!files || files.length === 0) return;

      let uploaded = 0;
      for (const file of files) {
        const formData = new FormData();
        formData.append('path', currentDir);
        formData.append('file', file);

        try {
          const resp = await fetch('/api/filer/upload', {
            method: 'POST',
            headers: { 'Authorization': `Bearer ${token}` },
            body: formData,
          });
          if (resp.ok) {
            uploaded++;
          } else {
            const err = await resp.json().catch(() => ({ error: 'Upload failed' }));
            Toast.error(`${file.name}: ${err.error || 'Upload failed'}`);
          }
        } catch {
          Toast.error(`${file.name}: Upload failed`);
        }
      }

      if (uploaded > 0) {
        Toast.success(`Uploaded ${uploaded} file${uploaded > 1 ? 's' : ''}`);
        FilerTree.refresh();
      }
    });
  }

  // --- コンテキストメニュー ---

  function showContextMenu(path, isDir, x, y) {
    hideContextMenu();

    contextMenu = document.createElement('div');
    contextMenu.className = 'context-menu';
    contextMenu.style.left = `${x}px`;
    contextMenu.style.top = `${y}px`;

    const items = [];

    if (isDir) {
      items.push({ label: 'New File Here...', action: () => promptNewFile(path) });
      items.push({ label: 'New Folder Here...', action: () => promptNewFolder(path) });
      items.push({ separator: true });
    }

    if (!isDir) {
      items.push({ label: 'Open', action: () => FilerEditor.openFile(path) });
      items.push({ label: 'Download', action: () => downloadFile(path) });
      items.push({ separator: true });
    }

    items.push({ label: 'Rename...', action: () => promptRename(path) });
    items.push({ separator: true });
    items.push({ label: 'Delete', action: () => doDelete(path), danger: true });

    for (const item of items) {
      if (item.separator) {
        const sep = document.createElement('div');
        sep.className = 'context-menu-separator';
        contextMenu.appendChild(sep);
      } else {
        const el = document.createElement('div');
        el.className = `context-menu-item${item.danger ? ' danger' : ''}`;
        el.textContent = item.label;
        el.addEventListener('click', (e) => {
          e.stopPropagation();
          hideContextMenu();
          item.action();
        });
        contextMenu.appendChild(el);
      }
    }

    document.body.appendChild(contextMenu);

    // 画面外にはみ出さないよう調整
    const rect = contextMenu.getBoundingClientRect();
    if (rect.right > window.innerWidth) {
      contextMenu.style.left = `${window.innerWidth - rect.width - 4}px`;
    }
    if (rect.bottom > window.innerHeight) {
      contextMenu.style.top = `${window.innerHeight - rect.height - 4}px`;
    }
  }

  function hideContextMenu() {
    if (contextMenu) {
      contextMenu.remove();
      contextMenu = null;
    }
  }

  // --- ファイル操作 ---

  function promptNewFile(basePath) {
    const dir = typeof basePath === 'string' ? basePath : currentDir;
    const name = prompt('New file name:');
    if (!name) return;
    const fullPath = joinPath(dir, name);
    apiCall('/api/filer/write', 'PUT', { path: fullPath, content: '' }).then((ok) => {
      if (ok) {
        Toast.success('File created');
        FilerTree.refresh();
        FilerEditor.openFile(fullPath);
      }
    });
  }

  function promptNewFolder(basePath) {
    const dir = typeof basePath === 'string' ? basePath : currentDir;
    const name = prompt('New folder name:');
    if (!name) return;
    const fullPath = joinPath(dir, name);
    apiCall('/api/filer/mkdir', 'POST', { path: fullPath }).then((ok) => {
      if (ok) {
        Toast.success('Folder created');
        FilerTree.refresh();
      }
    });
  }

  function promptRename(path) {
    const oldName = path.split(/[/\\]/).pop();
    const newName = prompt('New name:', oldName);
    if (!newName || newName === oldName) return;
    const parentDir = FilerTree.getParentPath(path);
    const newPath = joinPath(parentDir, newName);
    apiCall('/api/filer/rename', 'POST', { from: path, to: newPath }).then((ok) => {
      if (ok) {
        Toast.success('Renamed');
        FilerEditor.notifyRenamed(path, newPath);
        FilerTree.refresh();
      }
    });
  }

  async function doDelete(path) {
    const name = path.split(/[/\\]/).pop();
    if (!(await Toast.confirm(`Delete "${name}"?`))) return;
    const ok = await apiCallDelete(`/api/filer/delete?path=${enc(path)}`);
    if (ok) {
      Toast.success('Deleted');
      FilerEditor.notifyDeleted(path);
      FilerTree.refresh();
    }
  }

  async function downloadFile(path) {
    try {
      const resp = await fetch(`/api/filer/download?path=${enc(path)}`, {
        headers: { 'Authorization': `Bearer ${token}` },
      });
      if (!resp.ok) {
        const err = await resp.json().catch(() => ({ error: 'Download failed' }));
        Toast.error(err.error || 'Download failed');
        return;
      }
      const blob = await resp.blob();
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = path.split(/[/\\]/).pop() || 'download';
      document.body.appendChild(a);
      a.click();
      a.remove();
      URL.revokeObjectURL(url);
    } catch {
      Toast.error('Download failed');
    }
  }

  // --- アップロード ---

  function showUploadModal() {
    document.getElementById('upload-dest').value = currentDir;
    document.getElementById('upload-file-input').value = '';
    document.getElementById('filer-upload-modal').hidden = false;
  }

  async function doUpload() {
    const fileInput = document.getElementById('upload-file-input');
    const dest = document.getElementById('upload-dest').value;
    const file = fileInput.files[0];
    if (!file) return;

    const submitBtn = document.getElementById('upload-submit');
    await Spinner.button(submitBtn, async () => {
      const formData = new FormData();
      formData.append('path', dest);
      formData.append('file', file);

      try {
        const resp = await fetch('/api/filer/upload', {
          method: 'POST',
          headers: { 'Authorization': `Bearer ${token}` },
          body: formData,
        });
        if (resp.ok) {
          document.getElementById('filer-upload-modal').hidden = true;
          Toast.success('Uploaded');
          FilerTree.refresh();
        } else {
          const err = await resp.json().catch(() => ({ error: 'Upload failed' }));
          Toast.error(err.error || 'Upload failed');
        }
      } catch {
        Toast.error('Upload failed');
      }
    });
  }

  // --- 検索 ---

  async function doSearch(query) {
    if (!query) return;

    const resultsEl = document.getElementById('filer-search-results');
    const modal = document.getElementById('filer-search-modal');
    // 検索中はモーダルを開いてスピナー表示
    resultsEl.innerHTML = '';
    modal.hidden = false;
    const data = await Spinner.wrap(resultsEl, () =>
      apiFetch(`/api/filer/search?path=${enc(currentDir)}&query=${enc(query)}&content=true`)
    );
    if (!data) {
      modal.hidden = true;
      return;
    }

    if (data.length === 0) {
      resultsEl.innerHTML = '<div style="padding:16px;color:var(--border);text-align:center">No results</div>';
    } else {
      for (const r of data) {
        const item = document.createElement('div');
        item.className = 'search-result-item';

        const pathEl = document.createElement('div');
        pathEl.className = 'search-result-path';
        pathEl.textContent = r.path;
        item.appendChild(pathEl);

        if (r.line) {
          const lineEl = document.createElement('span');
          lineEl.className = 'search-result-line';
          lineEl.textContent = `:${r.line}`;
          pathEl.appendChild(lineEl);
        }

        if (r.context) {
          const ctx = document.createElement('div');
          ctx.className = 'search-result-context';
          ctx.textContent = r.context;
          item.appendChild(ctx);
        }

        item.addEventListener('click', () => {
          document.getElementById('filer-search-modal').hidden = true;
          if (!r.is_dir) {
            FilerEditor.openFile(r.path);
          }
        });

        resultsEl.appendChild(item);
      }
    }
  }

  // --- ユーティリティ ---

  function joinPath(parent, name) {
    const sep = parent.includes('/') ? '/' : '\\';
    return parent.endsWith(sep) ? parent + name : parent + sep + name;
  }

  function enc(s) {
    return encodeURIComponent(s);
  }

  async function apiFetch(url) {
    try {
      const resp = await fetch(url, {
        headers: { 'Authorization': `Bearer ${token}` },
      });
      if (!resp.ok) {
        const err = await resp.json().catch(() => null);
        if (err) Toast.error(err.error);
        return null;
      }
      return resp.json();
    } catch {
      return null;
    }
  }

  async function apiCall(url, method, body) {
    try {
      const resp = await fetch(url, {
        method,
        headers: {
          'Authorization': `Bearer ${token}`,
          'Content-Type': 'application/json',
        },
        body: JSON.stringify(body),
      });
      if (!resp.ok) {
        const err = await resp.json().catch(() => null);
        if (err) Toast.error(err.error);
        return false;
      }
      return true;
    } catch {
      return false;
    }
  }

  async function apiCallDelete(url) {
    try {
      const resp = await fetch(url, {
        method: 'DELETE',
        headers: { 'Authorization': `Bearer ${token}` },
      });
      if (!resp.ok) {
        const err = await resp.json().catch(() => null);
        if (err) Toast.error(err.error);
        return false;
      }
      return true;
    } catch {
      return false;
    }
  }

  return { init };
})();
