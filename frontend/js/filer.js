/* global Auth, FilerTree, FilerEditor, FilerRemote */
// Den - ファイラ メインモジュール
// eslint-disable-next-line no-unused-vars
const DenFiler = (() => {
  let currentDir = '~';
  let contextMenu = null;

  function init() {

    // エディタ初期化
    FilerEditor.init(
      document.getElementById('filer-editor'),
      document.getElementById('filer-tabs'),
    );

    // ツリー初期化
    FilerTree.init(document.getElementById('filer-tree'), {
      onFileSelect: (path) => FilerEditor.openFile(path),
      onContextMenu: showContextMenu,
      onRootResolved: (resolvedPath) => {
        currentDir = resolvedPath;
        renderBreadcrumb(resolvedPath);
        // 初回: ドライブ一覧がまだない場合、ルートドライブから取得
        fetchDrivesIfNeeded(resolvedPath);
      },
      onDrivesLoaded: renderDrives,
      onRename: promptRename,
      onDelete: doDelete,
    });

    // ツールバーボタン
    document.getElementById('filer-new-file').addEventListener('click', promptNewFile);
    document.getElementById('filer-new-folder').addEventListener('click', promptNewFolder);
    document.getElementById('filer-upload').addEventListener('click', showUploadModal);
    document.getElementById('filer-refresh').addEventListener('click', () => FilerTree.refresh());

    // Remote SFTP ボタン
    initRemoteButton();

    // ツリー表示トグル
    initTreeToggle();

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

    // SFTP 接続モーダル
    const sftpAuthType = document.getElementById('sftp-auth-type');
    if (sftpAuthType) sftpAuthType.addEventListener('change', updateAuthFields);
    const sftpCancel = document.getElementById('sftp-connect-cancel');
    if (sftpCancel) sftpCancel.addEventListener('click', () => {
      document.getElementById('sftp-connect-modal').hidden = true;
    });
    const sftpSubmit = document.getElementById('sftp-connect-submit');
    if (sftpSubmit) sftpSubmit.addEventListener('click', doSftpConnect);

    // グローバルクリックでコンテキストメニュー閉じる
    document.addEventListener('click', hideContextMenu);

    // ドラッグ&ドロップ アップロード
    initDragDrop();
  }

  // --- ツリー表示トグル ---

  function initTreeToggle() {
    const btn = document.getElementById('filer-tree-toggle');
    const sideBtn = document.getElementById('filer-tree-side');
    const sidebar = document.querySelector('.filer-sidebar');
    const layout = document.querySelector('.filer-layout');
    if (!btn || !sidebar || !layout) return;

    // localStorage から状態を復元
    let isRight = localStorage.getItem('filer-tree-right') === 'true';
    const wasCollapsed = localStorage.getItem('filer-tree-collapsed') === 'true';

    function updateToggleIcon() {
      const collapsed = sidebar.classList.contains('collapsed');
      btn.setAttribute('aria-expanded', String(!collapsed));
      if (isRight) {
        btn.innerHTML = collapsed ? DenIcons.chevronLeft(14) : DenIcons.chevronRight(14);
      } else {
        btn.innerHTML = collapsed ? DenIcons.chevronRight(14) : DenIcons.chevronLeft(14);
      }
    }

    // 初期状態を適用
    if (isRight) layout.classList.add('tree-right');
    if (wasCollapsed) sidebar.classList.add('collapsed');
    updateToggleIcon();

    btn.addEventListener('click', () => {
      sidebar.classList.toggle('collapsed');
      localStorage.setItem('filer-tree-collapsed', String(sidebar.classList.contains('collapsed')));
      updateToggleIcon();
    });

    // 左右切替
    if (sideBtn) {
      sideBtn.innerHTML = isRight ? DenIcons.panelLeft(14) : DenIcons.panelRight(14);
      sideBtn.addEventListener('click', () => {
        isRight = !isRight;
        layout.classList.toggle('tree-right', isRight);
        localStorage.setItem('filer-tree-right', String(isRight));
        sideBtn.innerHTML = isRight ? DenIcons.panelLeft(14) : DenIcons.panelRight(14);
        updateToggleIcon();
      });
    }
  }

  // --- Remote SFTP 接続 ---

  function initRemoteButton() {
    const btn = document.getElementById('filer-remote-btn');
    if (!btn) return;

    btn.addEventListener('click', () => {
      if (FilerRemote.isRemote()) {
        // 切断確認
        doDisconnect();
      } else {
        showSftpModal();
      }
    });

    // 接続/切断イベント
    document.addEventListener('den:sftp-changed', (e) => {
      const { connected } = e.detail;
      updateRemoteButton();
      const drives = document.getElementById('filer-drives');
      if (connected) {
        // リモートルートでツリー再読込
        if (drives) drives.hidden = true;
        closeAllTabs();
        FilerTree.setRoot('/');
        currentDir = '/';
      } else {
        // ローカルに戻す
        if (drives) drives.hidden = false;
        drivesLoaded = false;
        closeAllTabs();
        FilerTree.setRoot('~');
        currentDir = '~';
      }
    });

    // 初期状態更新
    updateRemoteButton();
  }

  function updateRemoteButton() {
    const btn = document.getElementById('filer-remote-btn');
    if (!btn) return;
    if (FilerRemote.isRemote()) {
      const info = FilerRemote.getInfo();
      btn.textContent = `${info.username}@${info.host}`;
      btn.classList.add('active');
      btn.setAttribute('data-tooltip', 'Disconnect SFTP');
    } else {
      btn.textContent = 'Remote';
      btn.classList.remove('active');
      btn.setAttribute('data-tooltip', 'Connect to remote SFTP');
    }
  }

  function showSftpModal() {
    const modal = document.getElementById('sftp-connect-modal');
    if (!modal) return;
    // フォームリセット
    document.getElementById('sftp-host').value = '';
    document.getElementById('sftp-port').value = '22';
    document.getElementById('sftp-username').value = '';
    document.getElementById('sftp-auth-type').value = 'password';
    document.getElementById('sftp-password').value = '';
    document.getElementById('sftp-key-path').value = '';
    updateAuthFields();
    modal.hidden = false;
    document.getElementById('sftp-host').focus();
  }

  function updateAuthFields() {
    const authType = document.getElementById('sftp-auth-type').value;
    document.getElementById('sftp-password-field').hidden = authType !== 'password';
    document.getElementById('sftp-key-field').hidden = authType !== 'key';
  }

  async function doSftpConnect() {
    const host = document.getElementById('sftp-host').value.trim();
    const port = parseInt(document.getElementById('sftp-port').value, 10) || 22;
    const username = document.getElementById('sftp-username').value.trim();
    const authType = document.getElementById('sftp-auth-type').value;
    const password = document.getElementById('sftp-password').value;
    const keyPath = document.getElementById('sftp-key-path').value.trim();

    if (!host || !username) {
      Toast.error('Host and username are required');
      return;
    }

    const submitBtn = document.getElementById('sftp-connect-submit');
    await Spinner.button(submitBtn, async () => {
      try {
        await FilerRemote.connect(host, port, username, authType, password, keyPath);
        document.getElementById('sftp-connect-modal').hidden = true;
        Toast.success(`Connected to ${username}@${host}`);
      } catch (e) {
        Toast.error(e.message || 'Connection failed');
      }
    });
  }

  async function doDisconnect() {
    if (!(await Toast.confirm('Disconnect from remote SFTP?'))) return;
    await FilerRemote.disconnect();
    Toast.success('Disconnected');
  }

  /** 全タブを閉じる（dirty チェック付き） */
  async function closeAllTabs() {
    if (FilerEditor.hasUnsavedChanges()) {
      if (!(await Toast.confirm('Unsaved changes will be lost. Continue?'))) return;
    }
    FilerEditor.closeAll();
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
      overlay.innerHTML = '<div class="filer-drop-content"><div class="filer-drop-icon">' + DenIcons.download(40) + '</div><div>Drop files to upload</div></div>';
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
          const resp = await fetch(`${FilerRemote.getApiBase()}/upload`, {
            method: 'POST',
            credentials: 'same-origin',
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

  // --- ドライブ切替 ---

  let drivesLoaded = false;

  function renderDrives(drives) {
    drivesLoaded = true;
    const container = document.getElementById('filer-drives');
    if (!container) return;
    container.innerHTML = '';
    if (!drives || drives.length === 0) return;
    drives.forEach(d => {
      const btn = document.createElement('button');
      btn.className = 'dir-drive-btn';
      btn.textContent = d;
      btn.setAttribute('data-tooltip', 'Switch to ' + d);
      btn.addEventListener('click', () => {
        FilerTree.setRoot(d);
        currentDir = d;
      });
      container.appendChild(btn);
    });
  }

  async function fetchDrivesIfNeeded(resolvedPath) {
    if (drivesLoaded) return;
    // ドライブルートを抽出（例: "D:\Documents\..." → "D:\"）
    const match = resolvedPath.match(/^([A-Za-z]:\\)/);
    if (!match) return;
    const driveRoot = match[1];
    const data = await apiFetch(`${FilerRemote.getApiBase()}/list?path=${enc(driveRoot)}&show_hidden=false`);
    if (data && data.drives) {
      renderDrives(data.drives);
    }
  }

  // --- ブレッドクラム ---

  function renderBreadcrumb(dirPath) {
    const container = document.getElementById('filer-breadcrumb');
    if (!container) return;
    container.innerHTML = '';

    const sep = dirPath.includes('/') ? '/' : '\\';
    const parts = dirPath.split(sep).filter(Boolean);

    // Windows ドライブレター対応（例: D:\）
    const isWindows = sep === '\\';

    for (let i = 0; i < parts.length; i++) {
      if (i > 0) {
        const sepEl = document.createElement('span');
        sepEl.className = 'breadcrumb-sep';
        sepEl.textContent = sep;
        container.appendChild(sepEl);
      }

      const isLast = i === parts.length - 1;
      const segment = document.createElement('span');
      segment.className = isLast ? 'breadcrumb-segment breadcrumb-current' : 'breadcrumb-segment';
      segment.textContent = parts[i];

      if (!isLast) {
        // クリックでそのパスにナビゲート
        const targetPath = isWindows
          ? parts.slice(0, i + 1).join(sep) + sep
          : sep + parts.slice(0, i + 1).join(sep);
        segment.setAttribute('role', 'link');
        segment.setAttribute('tabindex', '0');
        segment.addEventListener('click', () => {
          FilerTree.setRoot(targetPath);
          currentDir = targetPath;
        });
        segment.addEventListener('keydown', (e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            FilerTree.setRoot(targetPath);
            currentDir = targetPath;
          }
        });
      }

      container.appendChild(segment);
    }

    // 最後までスクロール
    container.scrollLeft = container.scrollWidth;
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
      items.push({ label: 'Open Terminal Here', action: () => {
        if (window.DenApp) window.DenApp.switchTab('terminal');
        DenTerminal.sendInput('cd "' + path.replace(/"/g, '\\"') + '"\r');
      }});
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

  async function promptNewFile(basePath) {
    const dir = typeof basePath === 'string' ? basePath : currentDir;
    const name = await Toast.prompt('New file name:');
    if (!name) return;
    const fullPath = joinPath(dir, name);
    const ok = await apiCall(`${FilerRemote.getApiBase()}/write`, 'PUT', { path: fullPath, content: '' });
    if (ok) {
      Toast.success('File created');
      FilerTree.refresh();
      FilerEditor.openFile(fullPath);
    }
  }

  async function promptNewFolder(basePath) {
    const dir = typeof basePath === 'string' ? basePath : currentDir;
    const name = await Toast.prompt('New folder name:');
    if (!name) return;
    const fullPath = joinPath(dir, name);
    const ok = await apiCall(`${FilerRemote.getApiBase()}/mkdir`, 'POST', { path: fullPath });
    if (ok) {
      Toast.success('Folder created');
      FilerTree.refresh();
    }
  }

  async function promptRename(path) {
    const oldName = path.split(/[/\\]/).pop();
    const newName = await Toast.prompt('New name:', oldName);
    if (!newName || newName === oldName) return;
    const parentDir = FilerTree.getParentPath(path);
    const newPath = joinPath(parentDir, newName);
    const ok = await apiCall(`${FilerRemote.getApiBase()}/rename`, 'POST', { from: path, to: newPath });
    if (ok) {
      Toast.success('Renamed');
      FilerEditor.notifyRenamed(path, newPath);
      FilerTree.refresh();
    }
  }

  async function doDelete(path) {
    const name = path.split(/[/\\]/).pop();
    if (!(await Toast.confirm(`Delete "${name}"?`))) return;
    const ok = await apiCallDelete(`${FilerRemote.getApiBase()}/delete?path=${enc(path)}`);
    if (ok) {
      Toast.success('Deleted');
      FilerEditor.notifyDeleted(path);
      FilerTree.refresh();
    }
  }

  async function downloadFile(path) {
    try {
      const resp = await fetch(`${FilerRemote.getApiBase()}/download?path=${enc(path)}`, {
        credentials: 'same-origin',
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
        const resp = await fetch(`${FilerRemote.getApiBase()}/upload`, {
          method: 'POST',
          credentials: 'same-origin',
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
      apiFetch(`${FilerRemote.getApiBase()}/search?path=${enc(currentDir)}&query=${enc(query)}&content=true`)
    );
    if (!data) {
      modal.hidden = true;
      return;
    }

    if (data.length === 0) {
      resultsEl.innerHTML = '<div style="padding:16px;color:var(--muted);text-align:center">No results</div>';
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

        item.addEventListener('click', async () => {
          document.getElementById('filer-search-modal').hidden = true;
          if (!r.is_dir) {
            await FilerEditor.openFile(r.path);
            if (r.line) {
              FilerEditor.goToLine(r.path, r.line);
            }
          }
        });

        resultsEl.appendChild(item);
      }
    }
  }

  // --- ユーティリティ ---

  function joinPath(parent, name) {
    // リモートモードでは常に '/' を使用
    const sep = FilerRemote.isRemote() ? '/' : (parent.includes('/') ? '/' : '\\');
    return parent.endsWith(sep) ? parent + name : parent + sep + name;
  }

  function enc(s) {
    return encodeURIComponent(s);
  }

  async function apiFetch(url) {
    try {
      const resp = await fetch(url, {
        credentials: 'same-origin',
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
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' },
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
        credentials: 'same-origin',
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

  function focusSearch() {
    const input = document.getElementById('filer-search-input');
    if (input) {
      input.focus();
      input.select();
    }
  }

  // --- クイックオープン (Ctrl+P) ---

  let quickOpenCleanup = null;

  function showQuickOpen() {
    // 前回のリスナーが残っていれば除去（Esc で閉じた場合のリーク防止）
    if (quickOpenCleanup) {
      quickOpenCleanup();
      quickOpenCleanup = null;
    }

    const modal = document.getElementById('filer-quickopen-modal');
    const input = document.getElementById('quickopen-input');
    const results = document.getElementById('quickopen-results');

    modal.hidden = false;
    input.value = '';
    results.innerHTML = '';
    input.focus();

    let debounceTimer = null;
    let selectedIdx = -1;
    let items = [];

    function renderResults(data) {
      results.innerHTML = '';
      items = data || [];
      selectedIdx = items.length > 0 ? 0 : -1;

      for (let i = 0; i < items.length; i++) {
        const r = items[i];
        const div = document.createElement('div');
        div.className = 'quickopen-item' + (i === selectedIdx ? ' selected' : '');
        div.textContent = r.path;
        div.addEventListener('click', () => {
          openAndClose(r.path);
        });
        results.appendChild(div);
      }
    }

    function updateSelection() {
      const els = results.querySelectorAll('.quickopen-item');
      els.forEach((el, i) => el.classList.toggle('selected', i === selectedIdx));
      if (els[selectedIdx]) {
        els[selectedIdx].scrollIntoView({ block: 'nearest' });
      }
    }

    function openAndClose(path) {
      modal.hidden = true;
      cleanup();
      if (!path.endsWith('/') && !path.endsWith('\\')) {
        FilerEditor.openFile(path);
      }
    }

    function onInput() {
      clearTimeout(debounceTimer);
      const q = input.value.trim();
      if (q.length < 1) {
        results.innerHTML = '';
        items = [];
        selectedIdx = -1;
        return;
      }
      debounceTimer = setTimeout(async () => {
        const data = await apiFetch(
          `${FilerRemote.getApiBase()}/search?path=${enc(currentDir)}&query=${enc(q)}&content=false`
        );
        renderResults(data);
      }, 300);
    }

    function onKeydown(e) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        if (items.length > 0) {
          selectedIdx = (selectedIdx + 1) % items.length;
          updateSelection();
        }
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        if (items.length > 0) {
          selectedIdx = (selectedIdx - 1 + items.length) % items.length;
          updateSelection();
        }
      } else if (e.key === 'Enter') {
        e.preventDefault();
        if (selectedIdx >= 0 && items[selectedIdx]) {
          openAndClose(items[selectedIdx].path);
        }
      }
    }

    function onModalClick(e) {
      if (e.target === modal) {
        modal.hidden = true;
        cleanup();
      }
    }

    function cleanup() {
      clearTimeout(debounceTimer);
      input.removeEventListener('input', onInput);
      input.removeEventListener('keydown', onKeydown);
      modal.removeEventListener('click', onModalClick);
    }

    input.addEventListener('input', onInput);
    input.addEventListener('keydown', onKeydown);
    modal.addEventListener('click', onModalClick);

    quickOpenCleanup = cleanup;
  }

  return { init, focusSearch, showQuickOpen };
})();
