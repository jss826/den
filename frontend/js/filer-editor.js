/* global CM, Auth */
// Den - ファイラ CodeMirror 6 エディタ統合
// eslint-disable-next-line no-unused-vars
const FilerEditor = (() => {
  let editorContainer;
  let tabsContainer;
  // openFiles: Map<path, { view, state, content, dirty }>
  const openFiles = new Map();
  let activePath = null;

  function init(editorEl, tabsEl) {
    editorContainer = editorEl;
    tabsContainer = tabsEl;

    // Ctrl+S 保存
    document.addEventListener('keydown', (e) => {
      if ((e.ctrlKey || e.metaKey) && e.key === 's') {
        e.preventDefault();
        if (activePath) saveActive();
      }
    });
  }

  async function openFile(filePath) {
    // 既に開いている場合はアクティブにするだけ
    if (openFiles.has(filePath)) {
      setActive(filePath);
      return;
    }

    // API からファイル読み込み
    const data = await apiFetch(`/api/filer/read?path=${enc(filePath)}`);
    if (!data) return;

    if (data.is_binary) {
      Toast.warn('Binary files cannot be edited');
      return;
    }

    // CodeMirror インスタンス作成
    const lang = detectLanguage(filePath);
    const extensions = [
      ...CM.denSetup,
      CM.oneDark,
      CM.EditorView.updateListener.of((update) => {
        if (update.docChanged) {
          markDirty(filePath);
        }
      }),
    ];
    if (lang) extensions.push(lang);

    const state = CM.EditorState.create({
      doc: data.content,
      extensions,
    });

    const view = new CM.EditorView({ state });

    openFiles.set(filePath, {
      view,
      content: data.content,
      dirty: false,
    });

    // タブ追加
    renderTabs();
    setActive(filePath);
  }

  function setActive(filePath) {
    if (!openFiles.has(filePath)) return;

    // 前のエディタを非表示
    if (activePath && openFiles.has(activePath)) {
      const prev = openFiles.get(activePath);
      if (prev.view.dom.parentElement) {
        prev.view.dom.remove();
      }
    }

    activePath = filePath;
    const file = openFiles.get(filePath);

    // welcome メッセージを消す
    const welcome = editorContainer.querySelector('.filer-welcome');
    if (welcome) welcome.remove();

    // エディタを表示
    editorContainer.appendChild(file.view.dom);
    file.view.focus();

    renderTabs();
  }

  async function closeFile(filePath) {
    const file = openFiles.get(filePath);
    if (!file) return;

    if (file.dirty) {
      if (!(await Toast.confirm(`"${fileName(filePath)}" has unsaved changes. Close anyway?`))) {
        return;
      }
    }

    // エディタ破棄
    file.view.destroy();
    openFiles.delete(filePath);

    // アクティブファイルが閉じられた場合
    if (activePath === filePath) {
      activePath = null;
      // 別のファイルをアクティブにするか welcome を表示
      const remaining = [...openFiles.keys()];
      if (remaining.length > 0) {
        setActive(remaining[remaining.length - 1]);
      } else {
        editorContainer.innerHTML = '<div class="filer-welcome"><p>Select a file to edit</p></div>';
      }
    }

    renderTabs();
  }

  function markDirty(filePath) {
    const file = openFiles.get(filePath);
    if (file && !file.dirty) {
      file.dirty = true;
      renderTabs();
    }
  }

  async function saveActive() {
    if (!activePath) return;
    const file = openFiles.get(activePath);
    if (!file) return;

    const content = file.view.state.doc.toString();

    const resp = await fetch('/api/filer/write', {
      method: 'PUT',
      headers: {
        'Authorization': `Bearer ${Auth.getToken()}`,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({ path: activePath, content }),
    });

    if (resp.ok) {
      file.content = content;
      file.dirty = false;
      renderTabs();
      Toast.success('Saved');
    } else {
      const err = await resp.json().catch(() => ({ error: 'Save failed' }));
      Toast.error(err.error || 'Save failed');
    }
  }

  function renderTabs() {
    tabsContainer.innerHTML = '';
    for (const [path, file] of openFiles) {
      const tab = document.createElement('div');
      tab.className = `filer-tab${path === activePath ? ' active' : ''}`;

      const name = document.createElement('span');
      name.textContent = fileName(path);
      if (file.dirty) {
        const dot = document.createElement('span');
        dot.className = 'dirty';
        dot.textContent = ' \u25CF';
        name.appendChild(dot);
      }
      tab.appendChild(name);

      const close = document.createElement('button');
      close.className = 'filer-tab-close';
      close.textContent = '\u00D7';
      close.addEventListener('click', (e) => {
        e.stopPropagation();
        closeFile(path);
      });
      tab.appendChild(close);

      tab.addEventListener('click', () => setActive(path));
      tabsContainer.appendChild(tab);
    }
  }

  function hasUnsavedChanges() {
    for (const file of openFiles.values()) {
      if (file.dirty) return true;
    }
    return false;
  }

  // 外部からファイルパスの名前変更を反映
  function notifyRenamed(oldPath, newPath) {
    if (openFiles.has(oldPath)) {
      const file = openFiles.get(oldPath);
      openFiles.delete(oldPath);
      openFiles.set(newPath, file);
      if (activePath === oldPath) activePath = newPath;
      renderTabs();
    }
  }

  function notifyDeleted(path) {
    if (openFiles.has(path)) {
      const file = openFiles.get(path);
      file.view.destroy();
      openFiles.delete(path);
      if (activePath === path) {
        activePath = null;
        const remaining = [...openFiles.keys()];
        if (remaining.length > 0) {
          setActive(remaining[remaining.length - 1]);
        } else {
          editorContainer.innerHTML = '<div class="filer-welcome"><p>Select a file to edit</p></div>';
        }
      }
      renderTabs();
    }
  }

  // --- ユーティリティ ---

  function fileName(path) {
    const sep = path.includes('/') ? '/' : '\\';
    return path.split(sep).pop();
  }

  function detectLanguage(path) {
    const ext = path.split('.').pop().toLowerCase();
    const langs = {
      js: CM.javascript,
      mjs: CM.javascript,
      jsx: () => CM.javascript({ jsx: true }),
      ts: () => CM.javascript({ typescript: true }),
      tsx: () => CM.javascript({ typescript: true, jsx: true }),
      html: CM.html,
      htm: CM.html,
      css: CM.css,
      json: CM.json,
      md: CM.markdown,
      markdown: CM.markdown,
      rs: CM.rust,
      py: CM.python,
      yaml: CM.yaml,
      yml: CM.yaml,
      toml: () => CM.StreamLanguage.define(CM.toml),
    };
    const factory = langs[ext];
    if (!factory) return null;
    return typeof factory === 'function' ? factory() : factory();
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

  return {
    init, openFile, closeFile, saveActive, hasUnsavedChanges,
    notifyRenamed, notifyDeleted,
  };
})();
