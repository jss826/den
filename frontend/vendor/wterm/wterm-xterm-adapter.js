// wterm adapter for Den — wraps WTerm with xterm.js-compatible API surface.
// Runs alongside xterm.js and restty; selected via DenSettings.terminal_renderer.

/* global Toast */

import { WTerm } from './wterm.bundle.js';

// Single source of truth for cache-busting the vendored bundle, CSS, and the
// adapter's own dynamic import URL in terminal-adapter.js.
const WTERM_VERSION = '15';

let _cssInjected = false;
function ensureCss() {
  if (_cssInjected) return;
  _cssInjected = true;
  const link = document.createElement('link');
  link.rel = 'stylesheet';
  link.href = `/vendor/wterm/wterm.css?v=${WTERM_VERSION}`;
  document.head.appendChild(link);
}

function noopDisposable() {
  return { dispose() {} };
}

/**
 * Map an xterm-style theme object to wterm CSS custom properties.
 * wterm uses `--term-bg`, `--term-fg`, `--term-cursor`, and `--term-color-0..15`.
 */
const ANSI_COLOR_KEYS = [
  'black', 'red', 'green', 'yellow', 'blue', 'magenta', 'cyan', 'white',
  'brightBlack', 'brightRed', 'brightGreen', 'brightYellow',
  'brightBlue', 'brightMagenta', 'brightCyan', 'brightWhite',
];

function applyThemeToElement(el, theme) {
  if (!el || !theme) return;
  const s = el.style;
  if (theme.background) s.setProperty('--term-bg', theme.background);
  if (theme.foreground) s.setProperty('--term-fg', theme.foreground);
  if (theme.cursor) s.setProperty('--term-cursor', theme.cursor);
  for (let i = 0; i < ANSI_COLOR_KEYS.length; i++) {
    const val = theme[ANSI_COLOR_KEYS[i]];
    if (val) s.setProperty(`--term-color-${i}`, val);
  }
}

function applyFontToElement(el, fontFamily, fontSize) {
  if (!el) return;
  const s = el.style;
  if (fontFamily) s.setProperty('--term-font-family', fontFamily);
  if (fontSize) s.setProperty('--term-font-size', `${fontSize}px`);
}

/**
 * DenWtermTerminal — xterm.js-compatible facade over `WTerm`.
 * Defers WTerm construction until `open(parent)` to match xterm.js lifecycle
 * (`new Terminal(options)` then `term.open(el)`).
 */
class DenWtermTerminal {
  parser = { registerOscHandler(_id, _handler) { return noopDisposable(); } };

  cols = 80;
  rows = 24;

  constructor(options = {}) {
    const { theme, fontFamily, fontSize, cursorBlink } = options;
    this._theme = theme || null;
    this._fontFamily = fontFamily || null;
    this._fontSize = fontSize || 15;
    this._cursorBlink = cursorBlink !== false;
    this._dataListeners = new Set();
    this._resizeListeners = new Set();
    this._titleListeners = new Set();
    this._wterm = null;
    this._element = null;
    this._inner = null;
    this._parentObserver = null;
    this._externalTextarea = null;
    this._customKeyHandler = null;
    this._charW = null;
    this._charH = null;
    this._relayoutRaf = null;
    this._scrollRaf = null;
    this._ready = false;
    this._writeQueue = [];
    this._disposed = false;
  }

  get element() { return this._element; }
  get wterm() { return this._wterm; }

  // xterm.js-compatible buffer facade; viewportY is derived from actual scroll.
  get buffer() {
    const self = this;
    return {
      active: {
        get viewportY() {
          const rh = self._rowHeight();
          if (!rh || !self._inner) return 0;
          return Math.max(0, Math.floor(self._inner.scrollTop / rh));
        },
      },
    };
  }

  open(parent) {
    if (this._wterm) throw new Error('Already opened');
    ensureCss();
    this._element = parent;

    if (!parent.style.position || parent.style.position === 'static') {
      parent.style.position = 'relative';
    }
    const inner = document.createElement('div');
    inner.style.position = 'absolute';
    inner.style.inset = '0';
    inner.style.boxSizing = 'border-box';
    parent.appendChild(inner);
    this._inner = inner;

    applyFontToElement(inner, this._fontFamily, this._fontSize);
    if (this._theme) applyThemeToElement(inner, this._theme);

    const wterm = new WTerm(inner, {
      cols: this.cols,
      rows: this.rows,
      autoResize: false,
      cursorBlink: this._cursorBlink,
      onData: (data) => {
        for (const cb of this._dataListeners) {
          try { cb(data); } catch (_) { /* ignore */ }
        }
      },
      onResize: (cols, rows) => {
        if (cols !== this.cols || rows !== this.rows) {
          this.cols = cols;
          this.rows = rows;
          for (const cb of this._resizeListeners) {
            try { cb({ cols, rows }); } catch (_) { /* ignore */ }
          }
        }
      },
      onTitle: (title) => {
        for (const cb of this._titleListeners) {
          try { cb(title); } catch (_) { /* ignore */ }
        }
      },
    });

    this._wterm = wterm;

    wterm.init().then(() => {
      if (this._disposed) return;
      // WTerm's `_lockHeight()` fixes element.style.height when autoResize is
      // off. Clear it so the wrapper keeps `inset: 0` sizing.
      inner.style.height = '';
      this._ready = true;
      this.cols = wterm.cols;
      this.rows = wterm.rows;
      this._relocateTextarea();
      this._setupParentObserver(parent);
      this._relayoutFromParent();
      const queue = this._writeQueue;
      this._writeQueue = [];
      for (const data of queue) {
        try { wterm.write(data); } catch (_) { /* ignore */ }
      }
    }).catch((e) => {
      console.error('[wterm] init failed:', e);
      try {
        if (typeof Toast !== 'undefined') {
          Toast.error('wterm failed to initialize. Switch renderer to xterm.js in Settings and reload.');
        }
      } catch (_) { /* ignore */ }
      this.dispose();
    });
  }

  _relocateTextarea() {
    const ta = this._wterm?.input?.textarea;
    if (!ta) return;
    document.body.appendChild(ta);
    ta.style.position = 'fixed';
    ta.style.left = '0';
    ta.style.bottom = '0';
    ta.style.top = 'auto';
    ta.style.width = '1px';
    ta.style.height = '1px';
    ta.style.opacity = '0';
    ta.style.pointerEvents = 'none';
    ta.style.zIndex = '-1';
    ta.style.fontSize = '16px';
    ta.style.scrollMargin = '0';
    this._externalTextarea = ta;

    this._onTextareaFocus = () => {
      requestAnimationFrame(() => {
        if (window.scrollY !== 0) window.scrollTo(0, 0);
        if (document.documentElement.scrollTop !== 0) document.documentElement.scrollTop = 0;
      });
    };
    ta.addEventListener('focus', this._onTextareaFocus);

    // Custom key event handler (xterm.js compat). Runs first in the capture
    // phase; if it returns false, the event is suppressed before wterm's
    // keydown handler sends it to PTY.
    this._onCustomKey = (e) => {
      if (this._customKeyHandler && this._customKeyHandler(e) === false) {
        e.preventDefault();
        e.stopImmediatePropagation();
      }
    };
    ta.addEventListener('keydown', this._onCustomKey, true);

    // Shift+PageUp/PageDown/Home/End scrollback keys (xterm.js parity).
    this._onScrollKey = (e) => {
      if (!e.shiftKey || e.ctrlKey || e.metaKey || e.altKey) return;
      const inner = this._inner;
      if (!inner) return;
      const page = Math.max(1, Math.floor(inner.clientHeight * 0.9));
      let handled = true;
      if (e.key === 'PageUp') inner.scrollTop = Math.max(0, inner.scrollTop - page);
      else if (e.key === 'PageDown') inner.scrollTop = Math.min(inner.scrollHeight, inner.scrollTop + page);
      else if (e.key === 'Home') inner.scrollTop = 0;
      else if (e.key === 'End') inner.scrollTop = inner.scrollHeight;
      else handled = false;
      if (handled) { e.preventDefault(); e.stopImmediatePropagation(); }
    };
    ta.addEventListener('keydown', this._onScrollKey, true);
  }

  _setupParentObserver(parent) {
    if (this._parentObserver) return;
    // Debounced — see _relayoutFromParent. Coalesces with NoopFitAddon.fit()
    // bursts driven by DenTerminal.scheduleFit().
    this._parentObserver = new ResizeObserver(() => this._relayoutFromParent());
    this._parentObserver.observe(parent);
  }

  /** Measure char size once; invalidate on font changes via `_invalidateCharCache`. */
  _measureCharSize() {
    if (this._charW && this._charH) return { charW: this._charW, charH: this._charH };
    const inner = this._inner;
    if (!inner) return { charW: 0, charH: 0 };
    const probe = document.createElement('span');
    probe.textContent = 'W';
    probe.style.cssText = 'position:absolute;visibility:hidden;white-space:pre;';
    inner.appendChild(probe);
    const pr = probe.getBoundingClientRect();
    probe.remove();
    if (!pr.width || !pr.height) return { charW: 0, charH: 0 };
    this._charW = pr.width;
    this._charH = pr.height;
    return { charW: this._charW, charH: this._charH };
  }

  _invalidateCharCache() {
    this._charW = null;
    this._charH = null;
  }

  _rowHeight() {
    if (this._charH) return this._charH;
    if (this._inner) {
      const v = parseFloat(getComputedStyle(this._inner).getPropertyValue('--term-row-height'));
      if (v) return v;
    }
    return 17;
  }

  /** Coalesce multiple relayout requests (parent ResizeObserver + scheduleFit
   *  + font/option changes) into a single rAF-scheduled reconciliation. */
  _relayoutFromParent() {
    if (this._relayoutRaf != null || this._disposed) return;
    this._relayoutRaf = requestAnimationFrame(() => {
      this._relayoutRaf = null;
      this._doRelayout();
    });
  }

  _doRelayout() {
    const inner = this._inner;
    const wterm = this._wterm;
    if (!inner || !wterm) return;
    const cs = getComputedStyle(inner);
    const padX = (parseFloat(cs.paddingLeft) || 0) + (parseFloat(cs.paddingRight) || 0);
    const padY = (parseFloat(cs.paddingTop) || 0) + (parseFloat(cs.paddingBottom) || 0);
    const width = inner.clientWidth - padX;
    const height = inner.clientHeight - padY;
    if (width <= 0 || height <= 0) return;

    const { charW, charH } = this._measureCharSize();
    if (!charW || !charH) return;

    const cols = Math.max(1, Math.floor(width / charW));
    const rows = Math.max(1, Math.floor(height / charH));
    const prevScrollTop = inner.scrollTop;
    const atBottom = inner.scrollTop + inner.clientHeight >= inner.scrollHeight - 5;
    if (cols !== this.cols || rows !== this.rows) {
      try { wterm.resize(cols, rows); } catch (_) { /* ignore */ }
    }
    if (this._scrollRaf == null) {
      this._scrollRaf = requestAnimationFrame(() => {
        this._scrollRaf = null;
        if (atBottom) {
          inner.scrollTop = inner.scrollHeight;
        } else if (prevScrollTop > 0 && inner.scrollTop !== prevScrollTop) {
          inner.scrollTop = prevScrollTop;
        }
      });
    }
    if (inner.style.overflowY !== 'auto') inner.style.overflowY = 'auto';
    if (inner.style.overflowX !== 'hidden') inner.style.overflowX = 'hidden';
  }

  write(data, callback) {
    // ConPTY DSR probe: reply CPR via onData so the server-side PTY unblocks.
    if (typeof data === 'string' && data.includes('\x1b[6n')) {
      const cpr = '\x1b[1;1R';
      for (const cb of this._dataListeners) {
        try { cb(cpr); } catch (_) { /* ignore */ }
      }
    }
    if (data instanceof ArrayBuffer) data = new Uint8Array(data);
    if (data instanceof Uint8Array) {
      if (data.includes(7)) data = data.filter((b) => b !== 7);
    } else if (typeof data === 'string') {
      if (data.includes('\x07')) data = data.replace(/\x07/g, '');
    } else {
      data = String(data);
    }

    if (!this._ready) {
      this._writeQueue.push(data);
      callback?.();
      return;
    }
    try { this._wterm?.write(data); } catch (e) { console.warn('[wterm] write failed:', e); }
    callback?.();
  }

  writeln(data = '', callback) {
    if (data instanceof Uint8Array || data instanceof ArrayBuffer) {
      // Fresh decoder per call: writeln delivers complete lines, so partial
      // UTF-8 state must not leak across calls.
      const bytes = data instanceof ArrayBuffer ? new Uint8Array(data) : data;
      data = new TextDecoder('utf-8', { fatal: false }).decode(bytes);
    }
    this.write(`${data}\r\n`, callback);
  }

  // --- Event listeners (xterm.js compatible) ---

  onData(cb) {
    this._dataListeners.add(cb);
    return { dispose: () => this._dataListeners.delete(cb) };
  }

  onResize(cb) {
    this._resizeListeners.add(cb);
    return { dispose: () => this._resizeListeners.delete(cb) };
  }

  onTitleChange(cb) {
    this._titleListeners.add(cb);
    return { dispose: () => this._titleListeners.delete(cb) };
  }

  onBinary(_cb) { return noopDisposable(); }

  // --- Terminal control ---

  focus() {
    const inner = this._inner;
    const prev = inner?.scrollTop;
    this._wterm?.focus();
    if (inner != null && prev != null) {
      requestAnimationFrame(() => {
        if (inner.scrollTop !== prev) inner.scrollTop = prev;
      });
    }
  }
  blur() { this._inner?.blur?.(); }

  clear() { this.write('\x1b[2J\x1b[H'); }

  reset() { this.write('\x1bc'); }

  resize(cols, rows) {
    this.cols = cols;
    this.rows = rows;
    try { this._wterm?.resize(cols, rows); } catch (_) { /* before init */ }
  }

  // --- Options (theme, font) ---

  get options() {
    const self = this;
    if (!this._optionsProxy) {
      this._optionsProxy = new Proxy({}, {
        get(_t, prop) {
          if (prop === 'fontSize') return self._fontSize;
          if (prop === 'fontFamily') return self._fontFamily;
          if (prop === 'theme') return self._theme;
          return undefined;
        },
        set(_t, prop, value) {
          if (prop === 'theme') {
            self._theme = value;
            applyThemeToElement(self._inner, value);
          } else if (prop === 'fontSize') {
            self._fontSize = value;
            applyFontToElement(self._inner, null, value);
            self._invalidateCharCache();
            self._relayoutFromParent();
          } else if (prop === 'fontFamily') {
            self._fontFamily = value;
            applyFontToElement(self._inner, value, null);
            self._invalidateCharCache();
            self._relayoutFromParent();
          } else if (prop === 'scrollback') {
            // wterm manages scrollback internally and exposes no API
          }
          return true;
        },
      });
    }
    return this._optionsProxy;
  }

  set options(next) {
    if (!next) return;
    if (next.theme) {
      this._theme = next.theme;
      applyThemeToElement(this._inner, next.theme);
    }
    if (next.fontSize) {
      this._fontSize = next.fontSize;
      applyFontToElement(this._inner, null, next.fontSize);
      this._invalidateCharCache();
      this._relayoutFromParent();
    }
    if (next.fontFamily) {
      this._fontFamily = next.fontFamily;
      applyFontToElement(this._inner, next.fontFamily, null);
      this._invalidateCharCache();
      this._relayoutFromParent();
    }
  }

  // --- Addon / key / selection APIs consumed by Den ---

  loadAddon(addon) { addon?.activate?.(this); }

  /** xterm.js-compatible handler: return false to suppress the key. */
  attachCustomKeyEventHandler(handler) {
    this._customKeyHandler = typeof handler === 'function' ? handler : null;
  }

  /** Select a range starting at (col, row) spanning `length` chars.
   *  `row` is an absolute buffer row (scrollback included). Implemented at
   *  row granularity: selection extends from the start row to the row that
   *  contains the `length`-th character, giving copy-to-clipboard users a
   *  useful approximation even when per-char DOM traversal is not feasible. */
  select(col, row, length) {
    const inner = this._inner;
    if (!inner) return;
    const grid = inner.querySelector('.term-grid');
    if (!grid) return;
    const rowEls = grid.children;
    if (row < 0 || row >= rowEls.length || !length) return;
    const cols = Math.max(1, this.cols || 80);
    const lastRow = Math.min(rowEls.length - 1, row + Math.floor((col + length - 1) / cols));
    try {
      const range = document.createRange();
      range.setStartBefore(rowEls[row]);
      range.setEndAfter(rowEls[lastRow]);
      const sel = window.getSelection();
      if (!sel) return;
      sel.removeAllRanges();
      sel.addRange(range);
    } catch (_) { /* ignore */ }
  }

  clearSelection() {
    try { window.getSelection()?.removeAllRanges(); } catch (_) { /* ignore */ }
  }

  getSelection() {
    try { return window.getSelection()?.toString() ?? ''; } catch (_) { return ''; }
  }

  refresh(_start, _end) {}
  setOption(_key, _value) {}
  getOption(key) {
    if (key === 'cols') return this.cols;
    if (key === 'rows') return this.rows;
    return undefined;
  }

  dispose() {
    if (this._disposed) return;
    this._disposed = true;
    if (this._relayoutRaf != null) { cancelAnimationFrame(this._relayoutRaf); this._relayoutRaf = null; }
    if (this._scrollRaf != null) { cancelAnimationFrame(this._scrollRaf); this._scrollRaf = null; }
    this._writeQueue.length = 0;
    this._dataListeners.clear();
    this._resizeListeners.clear();
    this._titleListeners.clear();
    this._customKeyHandler = null;
    try { this._parentObserver?.disconnect(); } catch (_) { /* ignore */ }
    try {
      const ta = this._externalTextarea;
      if (ta) {
        if (this._onScrollKey) ta.removeEventListener('keydown', this._onScrollKey, true);
        if (this._onCustomKey) ta.removeEventListener('keydown', this._onCustomKey, true);
        if (this._onTextareaFocus) ta.removeEventListener('focus', this._onTextareaFocus);
      }
      this._externalTextarea?.remove();
    } catch (_) { /* ignore */ }
    try { this._wterm?.destroy(); } catch (_) { /* ignore */ }
    try { this._inner?.remove(); } catch (_) { /* ignore */ }
    this._wterm = null;
    this._element = null;
    this._inner = null;
    this._parentObserver = null;
    this._externalTextarea = null;
  }
}

/** FitAddon stub — delegates to DenWtermTerminal._relayoutFromParent so that
 *  Den's visualViewport handler (app.js → DenTerminal.scheduleFit) can drive
 *  wterm resize when the iOS soft keyboard appears/disappears. Coalesces with
 *  the parent ResizeObserver via the shared rAF scheduler. */
class NoopFitAddon {
  _term = null;
  activate(term) { this._term = term; }
  dispose() { this._term = null; }
  fit() { this._term?._relayoutFromParent?.(); }
  proposeDimensions() {
    const t = this._term;
    if (!t) return null;
    return { cols: t.cols, rows: t.rows };
  }
}

export { DenWtermTerminal, NoopFitAddon, WTERM_VERSION, applyThemeToElement, applyFontToElement };
