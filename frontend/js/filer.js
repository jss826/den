/* global DenClipboard, FilerTree, FilerEditor, FilerRemote */
// Den - ファイラ メインモジュール
// eslint-disable-next-line no-unused-vars
const DenFiler = (() => {
  let currentDir = '~';
  let contextMenu = null;
  const SHOW_HIDDEN_STORAGE_KEY = 'den:filer:show_hidden';

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

    // 隠しファイル表示トグル
    initHiddenToggle();

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

    initDenConnectModal();

    // SSH Bookmarks
    const bookmarkSelect = document.getElementById('sftp-bookmark-select');
    if (bookmarkSelect) bookmarkSelect.addEventListener('change', onBookmarkSelect);
    const bookmarkSave = document.getElementById('sftp-bookmark-save');
    if (bookmarkSave) bookmarkSave.addEventListener('click', onBookmarkSave);
    const bookmarkDelete = document.getElementById('sftp-bookmark-delete');
    if (bookmarkDelete) bookmarkDelete.addEventListener('click', onBookmarkDelete);

    // グローバルクリックでコンテキストメニュー閉じる
    document.addEventListener('click', hideContextMenu);

    // ドラッグ&ドロップ アップロード
    initDragDrop();
  }

  // --- 隠しファイル表示トグル ---

  function isShowHiddenEnabled() {
    return localStorage.getItem(SHOW_HIDDEN_STORAGE_KEY) === 'true';
  }

  function initHiddenToggle() {
    const btn = document.getElementById('filer-toggle-hidden');
    if (!btn) return;

    let showHidden = isShowHiddenEnabled();

    function update() {
      btn.innerHTML = showHidden ? DenIcons.eye(16) : DenIcons.eyeOff(16);
      btn.setAttribute('data-tooltip', showHidden ? 'Hide Hidden Files' : 'Show Hidden Files');
      btn.classList.toggle('active', showHidden);
    }

    update();
    btn.addEventListener('click', () => {
      showHidden = !showHidden;
      localStorage.setItem(SHOW_HIDDEN_STORAGE_KEY, String(showHidden));
      update();
      FilerTree.refresh();
    });
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

  // --- Remote source selector (SFTP + Remote Den) ---

  let remoteDropdown = null;

  function initRemoteButton() {
    const btn = document.getElementById('filer-remote-btn');
    if (!btn) return;

    btn.addEventListener('click', (e) => {
      e.stopPropagation();
      if (remoteDropdown) {
        closeRemoteDropdown();
      } else {
        showRemoteDropdown(btn);
      }
    });

    // Close dropdown on outside click
    document.addEventListener('click', () => closeRemoteDropdown());

    // Remote source changed event
    document.addEventListener('den:remote-changed', (e) => {
      const { mode } = e.detail;
      updateRemoteButton();
      const drives = document.getElementById('filer-drives');
      if (mode !== 'local') {
        if (drives) drives.hidden = true;
        closeAllTabs();
        FilerTree.setRoot('/');
        currentDir = '/';
      } else {
        if (drives) drives.hidden = false;
        drivesLoaded = false;
        closeAllTabs();
        FilerTree.setRoot('~');
        currentDir = '~';
      }
    });

    updateRemoteButton();
  }

  function updateRemoteButton() {
    const btn = document.getElementById('filer-remote-btn');
    if (!btn) return;
    const info = FilerRemote.getInfo();
    const denConns = FilerRemote.getDenConnections();
    const denCount = Object.keys(denConns).length;
    if (info.mode === 'relay') {
      btn.textContent = info.hostPort || 'Relay';
      btn.classList.add('active');
      btn.setAttribute('data-tooltip', `Connected via relay ${info.relayHostPort || ''}`);
    } else if (info.mode === 'den') {
      const activeConn = denConns[FilerRemote.getActiveDenId()];
      const label = activeConn?.displayName || activeConn?.hostPort || 'Remote Den';
      btn.textContent = denCount > 1 ? `${label} (+${denCount - 1})` : label;
      btn.classList.add('active');
      btn.setAttribute('data-tooltip', `${denCount} Den connection${denCount !== 1 ? 's' : ''}`);
    } else if (info.mode === 'sftp') {
      btn.textContent = `${info.username}@${info.host}`;
      btn.classList.add('active');
      btn.setAttribute('data-tooltip', 'Connected via SFTP');
    } else if (denCount > 0) {
      // Den connections exist but filer is in local mode
      btn.textContent = `Remote (${denCount})`;
      btn.classList.add('active');
      btn.setAttribute('data-tooltip', `${denCount} Den connection${denCount !== 1 ? 's' : ''} (filer: local)`);
    } else {
      btn.textContent = 'Remote';
      btn.classList.remove('active');
      btn.setAttribute('data-tooltip', 'Connect to remote file system');
    }
  }

  function closeRemoteDropdown() {
    if (remoteDropdown) {
      remoteDropdown.remove();
      remoteDropdown = null;
    }
  }

  async function showRemoteDropdown(anchorBtn) {
    closeRemoteDropdown();
    const menu = document.createElement('div');
    menu.className = 'new-session-menu';
    menu.addEventListener('click', (e) => e.stopPropagation());

    const info = FilerRemote.getInfo();

    // Relay disconnect
    if (info.mode === 'relay') {
      const label = `${info.hostPort || 'target'} via ${info.relayHostPort || 'relay'}`;
      const disconnItem = document.createElement('div');
      disconnItem.className = 'new-session-menu-item disconnect';
      disconnItem.textContent = `Disconnect ${label}`;
      disconnItem.addEventListener('click', () => {
        closeRemoteDropdown();
        doRelayDisconnect();
      });
      menu.appendChild(disconnItem);

      const sep = document.createElement('div');
      sep.className = 'new-session-menu-separator';
      menu.appendChild(sep);
    }

    // SFTP disconnect
    if (info.mode === 'sftp') {
      const label = `${info.username}@${info.host}`;
      const disconnItem = document.createElement('div');
      disconnItem.className = 'new-session-menu-item disconnect';
      disconnItem.textContent = `Disconnect ${label}`;
      disconnItem.addEventListener('click', () => {
        closeRemoteDropdown();
        doDisconnect();
      });
      menu.appendChild(disconnItem);

      const sep = document.createElement('div');
      sep.className = 'new-session-menu-separator';
      menu.appendChild(sep);
    }

    // Per-Den-connection disconnect items
    const denConns = FilerRemote.getDenConnections();
    const denIds = Object.keys(denConns);
    if (denIds.length > 0) {
      for (const connId of denIds) {
        const conn = denConns[connId];
        const label = conn.displayName || conn.hostPort || connId;
        const disconnItem = document.createElement('div');
        disconnItem.className = 'new-session-menu-item disconnect';
        disconnItem.textContent = `Disconnect ${label}`;
        disconnItem.addEventListener('click', () => {
          closeRemoteDropdown();
          doDenDisconnect(connId);
        });
        menu.appendChild(disconnItem);
      }

      const sep = document.createElement('div');
      sep.className = 'new-session-menu-separator';
      menu.appendChild(sep);
    }

    const denItem = document.createElement('div');
    denItem.className = 'new-session-menu-item';
    denItem.textContent = 'Quick Connect Den\u2026';
    denItem.addEventListener('click', () => {
      closeRemoteDropdown();
      showDenModal();
    });
    menu.appendChild(denItem);

    const denSep = document.createElement('div');
    denSep.className = 'new-session-menu-separator';
    menu.appendChild(denSep);

    // SFTP Connect
    const sftpItem = document.createElement('div');
    sftpItem.className = 'new-session-menu-item';
    sftpItem.textContent = 'SFTP Connect\u2026';
    sftpItem.addEventListener('click', () => {
      closeRemoteDropdown();
      showSftpModal();
    });
    menu.appendChild(sftpItem);

    const rect = anchorBtn.getBoundingClientRect();
    menu.style.position = 'fixed';
    menu.style.top = `${rect.bottom + 2}px`;
    menu.style.left = `${rect.left}px`;
    menu.style.zIndex = '1000';
    document.body.appendChild(menu);
    remoteDropdown = menu;
  }

  // --- SSH Bookmarks ---

  function renderBookmarkSelect(selectedLabel) {
    const select = document.getElementById('sftp-bookmark-select');
    const deleteBtn = document.getElementById('sftp-bookmark-delete');
    if (!select) return;
    const bookmarks = DenSettings.get('ssh_bookmarks') || [];
    select.innerHTML = '';
    const placeholder = document.createElement('option');
    placeholder.value = '';
    placeholder.textContent = '-- Saved Hosts --';
    select.appendChild(placeholder);
    bookmarks.forEach((b) => {
      const opt = document.createElement('option');
      opt.value = b.label;
      opt.textContent = b.label;
      if (b.label === selectedLabel) opt.selected = true;
      select.appendChild(opt);
    });
    if (deleteBtn) deleteBtn.hidden = !select.value;
  }

  function onBookmarkSelect() {
    const select = document.getElementById('sftp-bookmark-select');
    const deleteBtn = document.getElementById('sftp-bookmark-delete');
    const bookmarks = DenSettings.get('ssh_bookmarks') || [];
    const label = select.value;
    if (deleteBtn) deleteBtn.hidden = !label;
    const b = bookmarks.find(bk => bk.label === label);
    if (!b) return;
    document.getElementById('sftp-host').value = b.host;
    document.getElementById('sftp-port').value = String(b.port || 22);
    document.getElementById('sftp-username').value = b.username;
    document.getElementById('sftp-auth-type').value = b.auth_type || 'password';
    document.getElementById('sftp-password').value = '';
    document.getElementById('sftp-key-path').value = b.key_path || '';
    document.getElementById('sftp-initial-dir').value = b.initial_dir || '';
    updateAuthFields();
  }

  async function onBookmarkSave() {
    const host = document.getElementById('sftp-host').value.trim();
    const username = document.getElementById('sftp-username').value.trim();
    if (!host || !username) {
      Toast.error('Host and username are required to save');
      return;
    }
    const port = parseInt(document.getElementById('sftp-port').value, 10) || 22;
    if (port < 1 || port > 65535) {
      Toast.error('Port must be 1\u201365535');
      return;
    }
    const defaultLabel = `${username}@${host}`;
    const rawLabel = await Toast.prompt('Bookmark name:', defaultLabel);
    if (!rawLabel) return;
    const label = rawLabel.trim();
    if (!label) {
      Toast.error('Bookmark name cannot be empty');
      return;
    }

    const entry = {
      label,
      host,
      port,
      username,
      auth_type: document.getElementById('sftp-auth-type').value,
      key_path: document.getElementById('sftp-key-path').value.trim() || null,
      initial_dir: document.getElementById('sftp-initial-dir').value.trim() || null,
    };

    const bookmarks = (DenSettings.get('ssh_bookmarks') || []).slice();
    const existIdx = bookmarks.findIndex(b => b.label === label);
    if (existIdx >= 0) {
      if (!(await Toast.confirm(`A bookmark named "${label}" already exists. Overwrite?`))) return;
      bookmarks[existIdx] = entry;
    } else {
      if (bookmarks.length >= 50) {
        Toast.error('Bookmark limit reached (max 50)');
        return;
      }
      bookmarks.push(entry);
    }
    const ok = await DenSettings.save({ ssh_bookmarks: bookmarks });
    if (ok) {
      Toast.success('Bookmark saved');
      renderBookmarkSelect(label);
    } else {
      Toast.error('Failed to save bookmark');
    }
  }

  async function onBookmarkDelete() {
    const select = document.getElementById('sftp-bookmark-select');
    const bookmarks = (DenSettings.get('ssh_bookmarks') || []).slice();
    const label = select.value;
    const idx = bookmarks.findIndex(b => b.label === label);
    if (idx < 0) return;
    if (!(await Toast.confirm(`Delete bookmark "${label}"?`))) return;
    bookmarks.splice(idx, 1);
    const ok = await DenSettings.save({ ssh_bookmarks: bookmarks });
    if (ok) {
      Toast.success('Bookmark deleted');
      renderBookmarkSelect(null);
    } else {
      Toast.error('Failed to delete bookmark');
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
    renderBookmarkSelect(null);
    modal.hidden = false;
    document.getElementById('sftp-host').focus();
  }

  let denConnectInitialized = false;
  function initDenConnectModal() {
    if (denConnectInitialized) return;
    denConnectInitialized = true;
    const denCancel = document.getElementById('den-connect-cancel');
    if (denCancel) denCancel.addEventListener('click', () => {
      document.getElementById('den-connect-modal').hidden = true;
    });
    const denSubmit = document.getElementById('den-connect-submit');
    if (denSubmit) denSubmit.addEventListener('click', doDenConnect);
    const denUseRelay = document.getElementById('den-use-relay');
    if (denUseRelay) denUseRelay.addEventListener('change', () => {
      const section = document.getElementById('den-relay-section');
      if (section) section.hidden = !denUseRelay.checked;
    });
  }

  function populateDenUrlDatalist() {
    const datalist = document.getElementById('den-connect-url-list');
    if (!datalist) return;
    DenTlsTrust.list().then((certs) => {
      datalist.innerHTML = '';
      for (const hostPort of Object.keys(certs).sort()) {
        const opt = document.createElement('option');
        opt.value = 'https://' + hostPort;
        const name = certs[hostPort].display_name;
        if (name) opt.label = name;
        datalist.appendChild(opt);
      }
    }).catch(() => {});
  }

  function showDenModal(defaultUrl) {
    const modal = document.getElementById('den-connect-modal');
    if (!modal) return;
    const urlInput = document.getElementById('den-connect-url');
    const passwordInput = document.getElementById('den-connect-password');
    urlInput.value = defaultUrl || '';
    passwordInput.value = '';
    // Reset relay fields
    const relayCheckbox = document.getElementById('den-use-relay');
    if (relayCheckbox) relayCheckbox.checked = false;
    const relaySection = document.getElementById('den-relay-section');
    if (relaySection) relaySection.hidden = true;
    const relayUrl = document.getElementById('den-relay-url');
    if (relayUrl) relayUrl.value = '';
    const relayPassword = document.getElementById('den-relay-password');
    if (relayPassword) relayPassword.value = '';
    // Populate URL datalist from trusted certificates
    populateDenUrlDatalist();
    modal.hidden = false;
    urlInput.focus();
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
        if (e.message !== 'Connection cancelled') {
          Toast.error(e.message || 'Connection failed');
        }
      }
    });
  }

  async function doDenConnect() {
    const url = document.getElementById('den-connect-url').value.trim();
    const password = document.getElementById('den-connect-password').value;
    if (!url || !password) {
      Toast.error('Target URL and password are required');
      return;
    }

    const useRelay = document.getElementById('den-use-relay')?.checked;
    if (useRelay) {
      const relayUrl = document.getElementById('den-relay-url')?.value.trim();
      const relayPassword = document.getElementById('den-relay-password')?.value;
      if (!relayUrl || !relayPassword) {
        Toast.error('Relay URL and password are required');
        return;
      }
    }

    const submitBtn = document.getElementById('den-connect-submit');
    await Spinner.button(submitBtn, async () => {
      try {
        let data;
        if (useRelay) {
          const relayUrl = document.getElementById('den-relay-url').value.trim();
          const relayPassword = document.getElementById('den-relay-password').value;
          data = await FilerRemote.connectDenViaRelay(relayUrl, relayPassword, url, password);
          document.getElementById('den-connect-modal').hidden = true;
          Toast.success(`Connected to ${data.target_host_port || url} via relay`);
        } else {
          data = await FilerRemote.connectDen(url, password);
          document.getElementById('den-connect-modal').hidden = true;
          Toast.success(`Connected to ${data.host_port || url}`);
        }
      } catch (e) {
        if (e.message !== 'Connection cancelled') {
          Toast.error(e.message || 'Connection failed');
        }
      }
    });
  }

  async function doDisconnect() {
    if (!(await Toast.confirm('Disconnect from remote SFTP?'))) return;
    await FilerRemote.disconnect();
    Toast.success('Disconnected');
  }

  async function doDenDisconnect(connectionId) {
    if (!(await Toast.confirm('Disconnect from remote Den?'))) return;
    await FilerRemote.disconnectDen(connectionId);
    Toast.success('Disconnected');
  }

  async function doRelayDisconnect() {
    if (!(await Toast.confirm('Disconnect relay connection?'))) return;
    await FilerRemote.disconnectRelay();
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
    const showHidden = isShowHiddenEnabled();
    const data = await apiFetch(`${FilerRemote.getApiBase()}/list?path=${enc(driveRoot)}&show_hidden=${showHidden}`);
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

    items.push({ label: 'Copy Path', action: async () => {
      try {
        await DenClipboard.write(path);
        Toast.success('Path copied');
      } catch {
        Toast.error('Failed to copy path');
      }
    }});
    items.push({ separator: true });
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
    const showHidden = isShowHiddenEnabled();
    const data = await Spinner.wrap(resultsEl, () =>
      apiFetch(
        `${FilerRemote.getApiBase()}/search?path=${enc(currentDir)}&query=${enc(query)}&content=true&show_hidden=${showHidden}`
      )
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
        const showHidden = isShowHiddenEnabled();
        const data = await apiFetch(
          `${FilerRemote.getApiBase()}/search?path=${enc(currentDir)}&query=${enc(q)}&content=false&show_hidden=${showHidden}`
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

  return { init, initDenConnectModal, showDenModal, focusSearch, showQuickOpen };
})();
