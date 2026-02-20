/* global CM, Auth, DenMarkdown */
// Den - ファイラ CodeMirror 6 エディタ統合
// eslint-disable-next-line no-unused-vars
const FilerEditor = (() => {
  let editorContainer;
  let tabsContainer;
  // openFiles: Map<path, { view, state, content, dirty }>
  const openFiles = new Map();
  let activePath = null;

  let scrollLeftBtn;
  let scrollRightBtn;
  let renderTabsScheduled = false;

  function init(editorEl, tabsEl) {
    editorContainer = editorEl;
    tabsContainer = tabsEl;

    // スクロールボタン
    const wrapper = tabsContainer.parentElement;
    scrollLeftBtn = wrapper.querySelector('.filer-tabs-scroll.left');
    scrollRightBtn = wrapper.querySelector('.filer-tabs-scroll.right');

    scrollLeftBtn.addEventListener('click', () => {
      tabsContainer.scrollLeft -= 120;
    });
    scrollRightBtn.addEventListener('click', () => {
      tabsContainer.scrollLeft += 120;
    });
    tabsContainer.addEventListener('scroll', updateScrollButtons);

    // Ctrl+S 保存
    document.addEventListener('keydown', (e) => {
      if ((e.ctrlKey || e.metaKey) && e.key === 's') {
        e.preventDefault();
        if (activePath) saveActive();
      }
    });

    // Markdown プレビュートグル
    const mdBtn = document.getElementById('filer-md-preview-toggle');
    if (mdBtn) mdBtn.addEventListener('click', toggleMdPreview);
  }

  const IMAGE_EXTS = ['png', 'jpg', 'jpeg', 'gif', 'webp', 'ico', 'bmp'];

  function getExtension(filePath) {
    const name = filePath.split(/[/\\]/).pop() || '';
    const dotIdx = name.lastIndexOf('.');
    return dotIdx > 0 ? name.slice(dotIdx + 1).toLowerCase() : '';
  }

  function isImageFile(filePath) {
    return IMAGE_EXTS.includes(getExtension(filePath));
  }

  async function openFile(filePath) {
    // 既に開いている場合はアクティブにするだけ
    if (openFiles.has(filePath)) {
      setActive(filePath);
      return;
    }

    // 画像ファイルの場合はプレビュー表示
    if (isImageFile(filePath)) {
      openImagePreview(filePath);
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
    const theme = document.documentElement.getAttribute('data-theme') || 'dark';
    const isDark = !['light', 'solarized-light', 'gruvbox-light'].includes(theme);
    const extensions = [
      ...CM.denSetup,
      ...(isDark ? [CM.oneDark] : []),
      CM.search(),
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

  function openImagePreview(filePath) {
    const wrapper = document.createElement('div');
    wrapper.className = 'filer-image-preview';

    const img = document.createElement('img');
    img.alt = fileName(filePath);

    // 認証ヘッダー付き fetch + blob URL で画像取得（AbortController でキャンセル可能）
    const controller = new AbortController();
    let blobUrl = null;
    fetch(`/api/filer/download?path=${enc(filePath)}`, {
      credentials: 'same-origin',
      signal: controller.signal,
    }).then(resp => {
      if (!resp.ok) { Toast.error('Failed to load image'); return; }
      return resp.blob();
    }).then(blob => {
      if (!blob) return;
      blobUrl = URL.createObjectURL(blob);
      img.src = blobUrl;
    }).catch(() => {});

    wrapper.appendChild(img);

    // 疑似 view オブジェクト（setActive / closeFile で EditorView と同じインターフェース）
    const pseudoView = {
      dom: wrapper,
      destroy() { controller.abort(); if (blobUrl) URL.revokeObjectURL(blobUrl); wrapper.remove(); },
      focus() {},
      state: { doc: { toString() { return ''; } } },
    };

    openFiles.set(filePath, {
      view: pseudoView,
      content: '',
      dirty: false,
      isImage: true,
    });

    renderTabs();
    setActive(filePath);
  }

  function isMarkdownFile(filePath) {
    const ext = getExtension(filePath);
    return ext === 'md' || ext === 'mdx';
  }

  function setActive(filePath) {
    if (!openFiles.has(filePath)) return;

    // 前のエディタ/プレビューを非表示
    if (activePath && openFiles.has(activePath)) {
      const prev = openFiles.get(activePath);
      if (prev.view.dom.parentElement) {
        prev.view.dom.remove();
      }
      if (prev.previewDom && prev.previewDom.parentElement) {
        prev.previewDom.remove();
      }
    }

    activePath = filePath;
    const file = openFiles.get(filePath);

    // welcome メッセージを消す
    const welcome = editorContainer.querySelector('.filer-welcome');
    if (welcome) welcome.remove();

    // Markdown プレビューモード判定
    const mdBtn = document.getElementById('filer-md-preview-toggle');
    if (isMarkdownFile(filePath) && !file.isImage) {
      if (mdBtn) {
        mdBtn.hidden = false;
        mdBtn.innerHTML = file.mdPreview ? '\u270E' : '\u25B6';
        mdBtn.title = file.mdPreview ? 'Edit' : 'Preview';
      }

      if (file.mdPreview) {
        if (!file.previewDom) {
          file.previewDom = document.createElement('div');
          file.previewDom.className = 'filer-md-preview';
        }
        const content = file.view.state.doc.toString();
        if (!file.previewCache || file.previewCache !== content) {
          const render = () => {
            file.previewDom.innerHTML = DenMarkdown.sanitize(DenMarkdown.renderMarkdown(content));
            file.previewCache = content;
          };
          // 大きなファイル（10KB超）は requestIdleCallback で遅延描画しメインスレッドブロックを回避
          if (content.length > 10000 && window.requestIdleCallback) {
            file.previewDom.textContent = 'Rendering\u2026';
            requestIdleCallback(render);
          } else {
            render();
          }
        }
        editorContainer.appendChild(file.previewDom);
      } else {
        editorContainer.appendChild(file.view.dom);
        file.view.focus();
      }
    } else {
      if (mdBtn) mdBtn.hidden = true;
      editorContainer.appendChild(file.view.dom);
      file.view.focus();
    }

    renderTabs();
  }

  function toggleMdPreview() {
    if (!activePath) return;
    const file = openFiles.get(activePath);
    if (!file || file.isImage) return;
    file.mdPreview = !file.mdPreview;
    setActive(activePath);
  }

  async function closeFile(filePath) {
    const file = openFiles.get(filePath);
    if (!file) return;

    if (file.dirty) {
      if (!(await Toast.confirm(`"${fileName(filePath)}" has unsaved changes. Close anyway?`))) {
        return;
      }
    }

    // エディタ/プレビュー破棄
    file.view.destroy();
    if (file.previewDom && file.previewDom.parentElement) {
      file.previewDom.remove();
    }
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

    await Spinner.wrap(editorContainer, async () => {
      const resp = await fetch('/api/filer/write', {
        method: 'PUT',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' },
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
    });
  }

  function fileTypeIcon(path) {
    const ext = getExtension(path);
    const map = {
      js: 'JS', mjs: 'JS', jsx: 'JX', ts: 'TS', tsx: 'TX',
      rs: 'RS', py: 'PY', go: 'GO', rb: 'RB', java: 'JV',
      css: '#', scss: '#', less: '#',
      html: '<>', htm: '<>', xml: '<>', svg: '<>',
      json: '{}', yaml: 'YM', yml: 'YM', toml: 'TM',
      md: '\u00b6', txt: 'Tx',
      png: '\u25A3', jpg: '\u25A3', jpeg: '\u25A3', gif: '\u25A3',
      webp: '\u25A3', ico: '\u25A3', bmp: '\u25A3',
      sh: '$', bash: '$', zsh: '$', ps1: 'PS',
      sql: 'SQ', graphql: 'GQ',
    };
    return map[ext] || null;
  }

  function renderTabs() {
    if (renderTabsScheduled) return;
    renderTabsScheduled = true;
    requestAnimationFrame(renderTabsImmediate);
  }

  function renderTabsImmediate() {
    renderTabsScheduled = false;
    tabsContainer.innerHTML = '';
    for (const [path, file] of openFiles) {
      const tab = document.createElement('div');
      tab.className = `filer-tab${path === activePath ? ' active' : ''}`;
      tab.title = path;

      const icon = fileTypeIcon(path);
      if (icon) {
        const iconSpan = document.createElement('span');
        iconSpan.className = 'filer-tab-icon';
        iconSpan.textContent = icon;
        tab.appendChild(iconSpan);
      }

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
      close.setAttribute('data-tooltip', 'Close');
      close.addEventListener('click', (e) => {
        e.stopPropagation();
        closeFile(path);
      });
      tab.appendChild(close);

      tab.addEventListener('click', () => setActive(path));
      tab.addEventListener('mousedown', (e) => {
        if (e.button === 1) { // 中クリック
          e.preventDefault();
          closeFile(path);
        }
      });
      tabsContainer.appendChild(tab);
    }

    // アクティブタブを可視領域にスクロール（renderTabsImmediate は rAF 内で実行済み）
    const activeTab = tabsContainer.querySelector('.filer-tab.active');
    if (activeTab) {
      activeTab.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    }
    updateScrollButtons();
  }

  function updateScrollButtons() {
    if (!scrollLeftBtn || !scrollRightBtn) return;
    const { scrollLeft, scrollWidth, clientWidth } = tabsContainer;
    const hasOverflow = scrollWidth > clientWidth;
    scrollLeftBtn.hidden = !hasOverflow || scrollLeft <= 0;
    scrollRightBtn.hidden = !hasOverflow || scrollLeft >= scrollWidth - clientWidth - 1;
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
        credentials: 'same-origin',
      });
      if (!resp.ok) return null;
      return resp.json();
    } catch {
      return null;
    }
  }

  /** 指定行にスクロールしてハイライト（事前に openFile 済みであること） */
  function goToLine(filePath, lineNumber) {
    if (!openFiles.has(filePath) || !lineNumber) return;
    const file = openFiles.get(filePath);
    if (activePath !== filePath) setActive(filePath);
    const line = Math.max(1, lineNumber);
    const doc = file.view.state.doc;
    if (line > doc.lines) return;
    const lineInfo = doc.line(line);
    file.view.dispatch({
      selection: { anchor: lineInfo.from, head: lineInfo.to },
      effects: CM.EditorView.scrollIntoView(lineInfo.from, { y: 'center' }),
    });
  }

  return {
    init, openFile, closeFile, saveActive, hasUnsavedChanges,
    notifyRenamed, notifyDeleted, goToLine,
  };
})();
